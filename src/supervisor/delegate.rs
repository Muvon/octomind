// Copyright 2026 Muvon Un Limited
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Delegate gate — handoff quality check before a subagent is spawned.
//!
//! `tap run` and `agent_*` launch a CONTEXT-ISOLATED child: it sees only the
//! prompt string the parent wrote, never the parent's transcript. A thin prompt
//! therefore costs a full re-discovery at best and solves the wrong problem at
//! worst, and nothing downstream can recover the dropped intent.
//!
//! This gate sits at the same pre-spawn seam as guardrails: one cheap-model
//! call per round judges every handoff in that round against the parent's own
//! goal, live request and plan. A handoff that is unfaithful to what the user
//! asked, or not self-contained enough for a fresh session to execute, is
//! REJECTED — the tool never runs and the agent gets the gaps back as a tool
//! error, so it re-authors the prompt with the missing detail.
//!
//! Bounded by `max_revisions`: after that many rejected rounds in a turn the
//! gate passes everything through. A gate that can block forever is worse than
//! a thin prompt.
//!
//! Fail-open everywhere (disabled, no handoffs, LLM error, unparseable
//! response) — the supervisor must never block the agent on its own outage.

use crate::config::Config;
use crate::mcp::McpToolCall;
use crate::session::truncate_to_tokens;
use serde::Deserialize;

/// Cap on the parent-context block handed to the judge.
const TASK_CAP_TOKENS: usize = 2_000;
/// Cap on a single proposed prompt in the judge's input.
const PROMPT_CAP_TOKENS: usize = 4_000;

const SYSTEM_PROMPT: &str = r#"You are a delegation gate. An AI agent is about to hand work to a SUBAGENT that runs in a FRESH, ISOLATED session: the subagent will see ONLY the prompt text below — no conversation history, no prior tool output, no memory of what the parent discovered. Your job is to decide, per handoff, whether that prompt is good enough to send.

Judge each handoff on four criteria:
1. FAITHFUL — the delegated work lies inside what the user actually asked. No invented scope, no silently dropped requirement, no substituted objective.
2. SELF-CONTAINED — everything the subagent needs is IN the text: concrete file paths, symbol/function names, commands, versions, constraints, and the findings the parent already established. Back-references to context the subagent cannot see ("the file we found", "as discussed above", "continue the refactor") are disqualifying.
3. DELIVERABLE — it states what to produce or return, and what "done" looks like.
4. BOUNDED — where the request is ambiguous, it names the scope edge or the explicit non-goals.

Be strict about missing detail, not about style. A prompt that is terse but complete PASSES. A prompt that reads well but omits the paths, the constraint, or the acceptance condition does NOT.

PARENT STANDING INSTRUCTIONS, when present, are durable rules the parent agent operates under: a handoff that delegates work they forbid is unfaithful. Do not reject a handoff for not repeating them — the subagent runs under its own role.

Do not reject for things the parent could not know, for prompts that legitimately ask the subagent to investigate (exploration is a valid deliverable when the scope is named), or for missing detail that only the subagent can discover.

Output ONLY json, no prose:
{"results":[{"id":"<result id>","verdict":"pass"},{"id":"<result id>","verdict":"reject","gaps":["what is missing, and what to add"]}]}

Every input handoff id MUST appear exactly once. Each gap is one concrete, actionable sentence naming the missing information — never a restatement of the criterion."#;

/// One proposed handoff extracted from a tool round.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handoff {
	/// Tool call id — the key the verdict is matched back on.
	pub tool_id: String,
	pub tool_name: String,
	/// Role tag (`tap run`) or agent name (`agent_*`) receiving the work.
	pub target: String,
	/// The prompt/task text the subagent would receive.
	pub prompt: String,
	/// Resuming an existing tap-run: the child keeps its own history, so a
	/// follow-up turn may legitimately reference its earlier work.
	pub resume: bool,
}

#[derive(Deserialize)]
struct GateResponse {
	results: Vec<Entry>,
}

#[derive(Deserialize)]
struct Entry {
	id: String,
	verdict: String,
	#[serde(default)]
	gaps: Vec<String>,
}

/// Extract the subagent handoffs from a tool round. Calls with an empty
/// prompt/task are skipped — the tool itself rejects those with a clearer
/// message than the gate would.
pub fn collect(calls: &[McpToolCall]) -> Vec<Handoff> {
	let mut out = Vec::new();
	for c in calls {
		let (target, prompt, resume) = if c.tool_name == "tap" {
			let action = c
				.parameters
				.get("action")
				.and_then(|v| v.as_str())
				.unwrap_or_default()
				.trim();
			if action != "run" {
				continue;
			}
			let session = c
				.parameters
				.get("session")
				.and_then(|v| v.as_str())
				.map(str::trim)
				.filter(|s| !s.is_empty());
			let role = c
				.parameters
				.get("role")
				.and_then(|v| v.as_str())
				.map(str::trim)
				.filter(|s| !s.is_empty());
			let target = match (role, session) {
				(Some(r), _) => r.to_string(),
				(None, Some(s)) => format!("run {s}"),
				(None, None) => continue,
			};
			let prompt = c
				.parameters
				.get("prompt")
				.and_then(|v| v.as_str())
				.unwrap_or_default();
			(target, prompt, session.is_some())
		} else if let Some(name) = c.tool_name.strip_prefix("agent_") {
			let prompt = c
				.parameters
				.get("task")
				.and_then(|v| v.as_str())
				.unwrap_or_default();
			(name.to_string(), prompt, false)
		} else {
			continue;
		};
		if prompt.trim().is_empty() {
			continue;
		}
		out.push(Handoff {
			tool_id: c.tool_id.clone(),
			tool_name: c.tool_name.clone(),
			target,
			prompt: prompt.trim().to_string(),
			resume,
		});
	}
	out
}

/// Judge every subagent handoff in `calls` against the parent's `task` context.
/// Returns `(tool_id, rejection message)` for each handoff that must NOT be
/// spawned; an empty vec means the whole round is cleared to run.
///
/// `revisions` is how many rounds the gate already rejected in this turn —
/// at or above `max_revisions` it stops judging and lets everything through.
pub async fn gate_round(
	calls: &[McpToolCall],
	config: &Config,
	task: &str,
	role_context: &str,
	revisions: u8,
	operation_rx: tokio::sync::watch::Receiver<bool>,
) -> Vec<(String, String)> {
	let cfg = &config.supervisor.delegate;
	if !config.supervisor.enabled || !cfg.enabled {
		return Vec::new();
	}
	let handoffs = collect(calls);
	if handoffs.is_empty() {
		return Vec::new();
	}
	if revisions >= cfg.max_revisions {
		crate::supervisor::notify(&format!(
			"delegate gate exhausted ({} revisions) — handoff passed through unchecked",
			cfg.max_revisions
		));
		return Vec::new();
	}

	let user = build_prompt(&handoffs, task, role_context);
	crate::supervisor::notify(&format!(
		"checking {} subagent handoff(s): {}",
		handoffs.len(),
		handoffs
			.iter()
			.map(|h| h.target.as_str())
			.collect::<Vec<_>>()
			.join(" · ")
	));
	crate::supervisor::stats::delegate_run();

	let model = cfg.model.clone();
	let response = match crate::supervisor::learning::extract::call_learning_llm(
		config,
		&model,
		SYSTEM_PROMPT.to_string(),
		user,
		crate::supervisor::stats::CallKind::Delegate,
		operation_rx,
	)
	.await
	{
		Ok(r) => r,
		Err(e) => {
			crate::log_debug!("Delegate gate call failed, passing handoff through: {}", e);
			return Vec::new();
		}
	};
	decide(&handoffs, &response)
}

/// Map the judge's raw reply onto per-handoff rejections. Pure — the whole
/// verdict policy lives here (unparseable ⇒ pass, unknown/missing verdict ⇒
/// pass, reject without a stated gap ⇒ pass, because none of those give the
/// agent anything actionable to rewrite against).
fn decide(handoffs: &[Handoff], response: &str) -> Vec<(String, String)> {
	let Some(parsed) = parse_response(response) else {
		crate::log_debug!("Delegate gate: unparseable response, passing handoff through");
		return Vec::new();
	};

	let mut blocks = Vec::new();
	for h in handoffs {
		let Some(entry) = parsed.results.iter().find(|e| e.id == h.tool_id) else {
			continue;
		};
		if !entry.verdict.eq_ignore_ascii_case("reject") {
			continue;
		}
		let gaps: Vec<String> = entry
			.gaps
			.iter()
			.map(|g| g.trim().to_string())
			.filter(|g| !g.is_empty())
			.collect();
		// A reject with no stated gap is unactionable — treat as pass.
		if gaps.is_empty() {
			continue;
		}
		blocks.push((h.tool_id.clone(), format_rejection(h, &gaps)));
	}

	if !blocks.is_empty() {
		crate::supervisor::stats::delegate_block(blocks.len() as u64);
		crate::supervisor::notify(&format!(
			"handoff rejected ({} of {}) — agent must rewrite the prompt",
			blocks.len(),
			handoffs.len()
		));
	}
	blocks
}

fn build_prompt(handoffs: &[Handoff], task: &str, role_context: &str) -> String {
	let task_block = if task.trim().is_empty() {
		"(parent context unavailable — judge the handoff on self-containment alone and be lenient about faithfulness)".to_string()
	} else {
		truncate_to_tokens(task.trim(), TASK_CAP_TOKENS)
	};
	let role_block = if role_context.trim().is_empty() {
		String::new()
	} else {
		format!(
			"PARENT STANDING INSTRUCTIONS (durable rules the parent operates under):\n{}\n\n",
			truncate_to_tokens(role_context.trim(), TASK_CAP_TOKENS)
		)
	};
	let mut user = format!("PARENT CONTEXT (what the user asked the parent agent to do):\n{task_block}\n\n{role_block}PROPOSED HANDOFFS ({}):\n", handoffs.len());
	for h in handoffs {
		let kind = if h.resume {
			"resume (the subagent keeps its own history from earlier turns, so references to its own prior work are legitimate)"
		} else {
			"new session (the subagent starts with an empty context)"
		};
		user.push_str(&format!(
			"\n=== HANDOFF id={id} via={tool} to={target} kind={kind} ===\n{prompt}\n=== END id={id} ===\n",
			id = h.tool_id,
			tool = h.tool_name,
			target = h.target,
			prompt = truncate_to_tokens(&h.prompt, PROMPT_CAP_TOKENS),
		));
	}
	user
}

/// The tool-error body a rejected handoff returns to the agent.
pub fn format_rejection(handoff: &Handoff, gaps: &[String]) -> String {
	let mut s = format!(
		"[delegate gate] Handoff to '{}' was NOT sent — the prompt is not ready for a subagent that sees none of your context:\n",
		handoff.target
	);
	for g in gaps {
		s.push_str("- ");
		s.push_str(g);
		s.push('\n');
	}
	s.push_str(
		"Re-issue the call with a rewritten prompt that closes each point: state the concrete paths, symbols, commands and constraints you already know, what to return, and where the scope ends. Do not reference anything the subagent cannot see.",
	);
	s
}

/// Parse the judge's json, tolerating a ```json fence or surrounding prose.
fn parse_response(text: &str) -> Option<GateResponse> {
	let json = if let Some(start) = text.find("```json") {
		let after = &text[start + 7..];
		let end = after.find("```")?;
		&after[..end]
	} else if let Some(start) = text.find("```") {
		let after = &text[start + 3..];
		let end = after.find("```")?;
		&after[..end]
	} else {
		let start = text.find('{')?;
		let end = text.rfind('}')?;
		&text[start..=end]
	};
	serde_json::from_str(json.trim()).ok()
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;

	fn call(name: &str, id: &str, params: serde_json::Value) -> McpToolCall {
		McpToolCall {
			tool_name: name.to_string(),
			parameters: params,
			tool_id: id.to_string(),
		}
	}

	#[test]
	fn collects_fresh_tap_run() {
		let c = call(
			"tap",
			"t1",
			json!({"action":"run","role":"developer:general","prompt":"do the thing"}),
		);
		let h = collect(&[c]);
		assert_eq!(h.len(), 1);
		assert_eq!(h[0].target, "developer:general");
		assert_eq!(h[0].prompt, "do the thing");
		assert!(!h[0].resume);
	}

	#[test]
	fn collects_resumed_tap_run_and_marks_it() {
		let c = call(
			"tap",
			"t1",
			json!({"action":"run","session":"tap-dev-1","prompt":"keep going"}),
		);
		let h = collect(&[c]);
		assert_eq!(h.len(), 1);
		assert_eq!(h[0].target, "run tap-dev-1");
		assert!(h[0].resume);
	}

	#[test]
	fn collects_agent_task() {
		let h = collect(&[call(
			"agent_context",
			"a1",
			json!({"task":"map the auth flow"}),
		)]);
		assert_eq!(h.len(), 1);
		assert_eq!(h[0].target, "context");
		assert_eq!(h[0].prompt, "map the auth flow");
	}

	#[test]
	fn ignores_non_handoff_calls() {
		let calls = vec![
			call("tap", "t1", json!({"action":"list"})),
			call("tap", "t2", json!({"action":"discover","intent":"x"})),
			call("shell", "s1", json!({"command":"ls"})),
			// run without role or session — the tool itself rejects this
			call("tap", "t3", json!({"action":"run","prompt":"x"})),
			// empty prompt / task
			call(
				"tap",
				"t4",
				json!({"action":"run","role":"r","prompt":"  "}),
			),
			call("agent_x", "a1", json!({"task":""})),
		];
		assert!(collect(&calls).is_empty());
	}

	#[test]
	fn collects_multiple_handoffs_in_one_round() {
		let calls = vec![
			call(
				"tap",
				"t1",
				json!({"action":"run","role":"a","prompt":"p1"}),
			),
			call("shell", "s1", json!({"command":"ls"})),
			call("agent_b", "a2", json!({"task":"p2"})),
		];
		let h = collect(&calls);
		assert_eq!(h.len(), 2);
		assert_eq!(h[0].tool_id, "t1");
		assert_eq!(h[1].tool_id, "a2");
	}

	#[test]
	fn response_parses_fenced_and_bare_json() {
		let fenced = "prose\n```json\n{\"results\":[{\"id\":\"t1\",\"verdict\":\"pass\"}]}\n```";
		assert_eq!(parse_response(fenced).unwrap().results.len(), 1);
		let bare = "{\"results\":[{\"id\":\"t1\",\"verdict\":\"reject\",\"gaps\":[\"no paths\"]}]}";
		let p = parse_response(bare).unwrap();
		assert_eq!(p.results[0].gaps, vec!["no paths".to_string()]);
		assert!(parse_response("no json here").is_none());
	}

	#[test]
	fn response_defaults_missing_gaps() {
		let p = parse_response("{\"results\":[{\"id\":\"t1\",\"verdict\":\"reject\"}]}").unwrap();
		assert!(p.results[0].gaps.is_empty());
	}

	#[test]
	fn rejection_names_target_and_lists_gaps() {
		let h = Handoff {
			tool_id: "t1".into(),
			tool_name: "tap".into(),
			target: "developer:general".into(),
			prompt: "fix it".into(),
			resume: false,
		};
		let msg = format_rejection(
			&h,
			&[
				"no file paths given".into(),
				"no acceptance criterion".into(),
			],
		);
		assert!(msg.starts_with("[delegate gate] Handoff to 'developer:general' was NOT sent"));
		assert!(msg.contains("- no file paths given\n"));
		assert!(msg.contains("- no acceptance criterion\n"));
		assert!(msg.contains("Re-issue the call"));
	}

	#[test]
	fn prompt_includes_context_and_every_handoff() {
		let h = collect(&[
			call(
				"tap",
				"t1",
				json!({"action":"run","role":"a","prompt":"p1"}),
			),
			call("agent_b", "a2", json!({"task":"p2"})),
		]);
		let p = build_prompt(&h, "Goal: ship the parser");
		assert!(p.contains("Goal: ship the parser"));
		assert!(p.contains("=== HANDOFF id=t1 via=tap to=a kind=new session"));
		assert!(p.contains("=== HANDOFF id=a2 via=agent_b to=b"));
		assert!(p.contains("p1"));
		assert!(p.contains("p2"));
	}

	#[test]
	fn prompt_marks_missing_parent_context() {
		let h = collect(&[call(
			"tap",
			"t1",
			json!({"action":"run","role":"a","prompt":"p1"}),
		)]);
		assert!(build_prompt(&h, "   ").contains("parent context unavailable"));
	}

	#[test]
	fn resumed_handoff_is_labelled_for_the_judge() {
		let h = collect(&[call(
			"tap",
			"t1",
			json!({"action":"run","session":"s","prompt":"p"}),
		)]);
		assert!(build_prompt(&h, "goal").contains("kind=resume"));
	}

	fn two_handoffs() -> Vec<Handoff> {
		collect(&[
			call(
				"tap",
				"t1",
				json!({"action":"run","role":"a","prompt":"p1"}),
			),
			call("agent_b", "a2", json!({"task":"p2"})),
		])
	}

	#[test]
	fn decide_blocks_only_rejected_handoffs() {
		let h = two_handoffs();
		let blocks = decide(
			&h,
			r#"{"results":[{"id":"t1","verdict":"reject","gaps":["name the file paths"]},
			   {"id":"a2","verdict":"pass"}]}"#,
		);
		assert_eq!(blocks.len(), 1);
		assert_eq!(blocks[0].0, "t1");
		assert!(blocks[0].1.contains("name the file paths"));
	}

	#[test]
	fn decide_is_case_insensitive_on_verdict() {
		let h = two_handoffs();
		let blocks = decide(
			&h,
			r#"{"results":[{"id":"t1","verdict":"REJECT","gaps":["g"]},{"id":"a2","verdict":"PASS"}]}"#,
		);
		assert_eq!(blocks.len(), 1);
		assert_eq!(blocks[0].0, "t1");
	}

	#[test]
	fn decide_passes_reject_without_actionable_gaps() {
		let h = two_handoffs();
		// no gaps key, and gaps that are only whitespace — both unactionable
		let blocks = decide(
			&h,
			r#"{"results":[{"id":"t1","verdict":"reject"},{"id":"a2","verdict":"reject","gaps":["  ",""]}]}"#,
		);
		assert!(blocks.is_empty());
	}

	#[test]
	fn decide_passes_unparseable_or_unknown_verdict() {
		let h = two_handoffs();
		assert!(decide(&h, "the model rambled instead of answering").is_empty());
		assert!(decide(&h, r#"{"results":[{"id":"t1","verdict":"maybe"}]}"#).is_empty());
	}

	#[test]
	fn decide_ignores_verdicts_for_unknown_ids() {
		let h = two_handoffs();
		// hallucinated id must not block anything, and must not panic
		let blocks = decide(
			&h,
			r#"{"results":[{"id":"nope","verdict":"reject","gaps":["g"]}]}"#,
		);
		assert!(blocks.is_empty());
	}

	#[test]
	fn decide_passes_handoffs_missing_from_the_verdict() {
		let h = two_handoffs();
		// judge only answered for one id — the other must still run
		let blocks = decide(
			&h,
			r#"{"results":[{"id":"t1","verdict":"reject","gaps":["g"]}]}"#,
		);
		assert_eq!(blocks.len(), 1);
		assert_eq!(blocks[0].0, "t1");
	}

	#[test]
	fn decide_can_block_the_whole_round() {
		let h = two_handoffs();
		let blocks = decide(
			&h,
			r#"{"results":[{"id":"t1","verdict":"reject","gaps":["g1"]},{"id":"a2","verdict":"reject","gaps":["g2"]}]}"#,
		);
		let ids: Vec<&str> = blocks.iter().map(|(id, _)| id.as_str()).collect();
		assert_eq!(ids, vec!["t1", "a2"]);
	}

	/// The shipped template is the schema's single source of truth and parsing
	/// is STRICT — this both pins the delegate defaults and fails loudly if the
	/// section is dropped from the template.
	fn template_config() -> Config {
		toml::from_str(include_str!("../../config-templates/default.toml"))
			.expect("default.toml must parse")
	}

	#[test]
	fn template_ships_delegate_defaults() {
		let c = template_config();
		assert!(c.supervisor.delegate.enabled);
		assert_eq!(c.supervisor.delegate.max_revisions, 2);
		assert!(!c.supervisor.delegate.model.is_empty());
	}

	fn no_cancel() -> tokio::sync::watch::Receiver<bool> {
		tokio::sync::watch::channel(false).1
	}

	/// Every short-circuit below must return BEFORE the model call — these run
	/// with no network and would hang or fail otherwise, which is the point.
	#[tokio::test]
	async fn gate_short_circuits_when_disabled() {
		let calls = vec![call(
			"tap",
			"t1",
			json!({"action":"run","role":"a","prompt":"p"}),
		)];
		let mut c = template_config();
		c.supervisor.delegate.enabled = false;
		assert!(gate_round(&calls, &c, "goal", "", 0, no_cancel())
			.await
			.is_empty());

		let mut c = template_config();
		c.supervisor.enabled = false;
		assert!(gate_round(&calls, &c, "goal", "", 0, no_cancel())
			.await
			.is_empty());
	}

	#[tokio::test]
	async fn gate_short_circuits_without_handoffs() {
		let calls = vec![call("shell", "s1", json!({"command":"ls"}))];
		let c = template_config();
		assert!(gate_round(&calls, &c, "goal", "", 0, no_cancel())
			.await
			.is_empty());
	}

	#[tokio::test]
	async fn gate_passes_through_once_revisions_are_exhausted() {
		let calls = vec![call(
			"tap",
			"t1",
			json!({"action":"run","role":"a","prompt":"p"}),
		)];
		let mut c = template_config();
		c.supervisor.delegate.max_revisions = 2;
		// At and beyond the budget the gate stops judging — no model call, no block.
		assert!(gate_round(&calls, &c, "goal", "", 2, no_cancel())
			.await
			.is_empty());
		assert!(gate_round(&calls, &c, "goal", "", 7, no_cancel())
			.await
			.is_empty());
		// max_revisions = 0 means never judge.
		c.supervisor.delegate.max_revisions = 0;
		assert!(gate_round(&calls, &c, "goal", "", 0, no_cancel())
			.await
			.is_empty());
	}

	#[test]
	fn prompt_caps_oversized_inputs() {
		let huge = "word ".repeat(40_000);
		let h = collect(&[call(
			"tap",
			"t1",
			json!({"action":"run","role":"a","prompt":huge.clone()}),
		)]);
		let p = build_prompt(&h, &huge);
		// Both blocks are capped, so the judge input stays bounded regardless of
		// how large the parent context or the proposed prompt is.
		assert!(p.len() < huge.len());
	}
}
