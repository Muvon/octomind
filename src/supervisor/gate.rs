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

//! Verify-gate — when the agent self-reports `done`, an independent pass checks
//! the result against the request before completion is accepted. On gaps the
//! caller injects an advisory and re-runs the turn (bounded). A PASS labels the
//! trajectory so only verified work is learned.

use crate::config::Config;
use std::collections::VecDeque;
use tokio::sync::watch;

const GATE_PROMPT: &str = r#"You are a strict completion verifier. A different agent claims its task is COMPLETE.
Judge the END STATE, not the agent's story: ignore its self-report and stated claim, and
check only what the AGENT FINAL RESULT actually evidences against the USER REQUEST.

You may also receive RECORDED ACTIONS — the runtime's own log of every tool call the agent
actually executed for this task ([mut] = state-changing, [read] = inspection; each line shows
the arguments and an ok/ERROR outcome). The agent cannot edit this log; when present it
outranks the narrative:
- A claim of performed work (created, edited, ran, posted, sent, fixed…) is evidenced only by
  a matching successful recorded action — narrative with no matching action is a gap.
- A claim of verification ("tests pass", "checked X") needs a matching successful recorded
  action; an ERROR outcome on the decisive check is a gap.
- When RECORDED ACTIONS is absent or empty, the task may be pure reasoning — judge the result
  text on its own terms.

You may also receive SESSION CONTEXT — the durable goal this session is pursuing and/or the
live plan checklist. The request may be terse ("continue", "finish it", "fix that"): resolve
what it refers to using this context, and verify against the resolved meaning. An open plan
item is a gap only when the request (so resolved) asks for it — do not demand work beyond
the request's own scope.

Work through every part of the request, one at a time. For each, find the concrete proof it
was done — a recorded action, file path, line or code excerpt, command output, or named test
in the result. A part counts as done only if such evidence is present; a confident or
well-formatted assertion with no locatable artifact does NOT count. Reason first, then decide.
Do not reward length, formatting, or tone — only verifiable substance.

Flag a gap only when a requested part is provably missing, a stated requirement is unmet, or a
claim has no supporting evidence. Each gap must name the specific unmet item.

If every part is evidenced — or you cannot point to a concrete unmet item — output exactly:
<verdict>PASS</verdict>

Otherwise output one line per gap (and nothing else):
<gap>specific missing or unverified item</gap>

Be conservative — only flag real, actionable gaps. If unsure, PASS."#;

/// Outcome of a verification pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateVerdict {
	Pass,
	Gaps(Vec<String>),
}

/// True when a message is a supervisor-injected note (a `<pay-attention>` advisory
/// or a `<recall>` block), not a genuine user turn. Lets the gate find the real
/// task instead of verifying against its own prior advisory.
pub fn is_supervisor_injection(content: &str) -> bool {
	let t = content.trim_start();
	t.starts_with("<pay-attention>") || t.starts_with("<recall>")
}

/// Cap on ledger lines — beyond it the oldest are dropped (and counted in the
/// render) so a very long turn still hands the verifier a bounded block.
const LEDGER_CAP: usize = 128;
/// Args locate the object of an action (path, command, url) — not replay it.
const LEDGER_ARGS_MAX: usize = 120;

/// One executed tool call (or a run of identical consecutive successful calls).
#[derive(Debug)]
struct LedgerEntry {
	tool: String,
	args: String,
	mutation: bool,
	error: bool,
	bytes: usize,
	repeats: usize,
}

/// Runtime-recorded tool log for the current task — the ground truth the
/// verify-gate checks completion claims against. Entries are written by the
/// tool loop from actual executions, so the agent's narrative cannot alter
/// them. Reset on each genuine user turn; gate/steer re-runs (system-managed
/// messages) keep accumulating into the same task slice.
#[derive(Debug, Default)]
pub struct EvidenceLedger {
	entries: VecDeque<LedgerEntry>,
	dropped: usize,
}

impl EvidenceLedger {
	/// Start a fresh task slice (genuine user turn).
	pub fn reset(&mut self) {
		self.entries.clear();
		self.dropped = 0;
	}

	/// Record one executed tool call. Only an identical consecutive repeat of a
	/// successful call collapses into ×N — different args always keep their own
	/// line (a decisive check like a test command must never disappear into a
	/// generic collapsed row), and errors never collapse: each failure is signal.
	pub fn record(
		&mut self,
		tool: &str,
		parameters: &serde_json::Value,
		mutation: bool,
		error: bool,
		bytes: usize,
	) {
		let mut args = parameters.to_string();
		if args.chars().count() > LEDGER_ARGS_MAX {
			args = args.chars().take(LEDGER_ARGS_MAX).collect();
			args.push('…');
		}
		if !error {
			if let Some(last) = self.entries.back_mut() {
				if !last.error && last.tool == tool && last.args == args {
					last.repeats += 1;
					return;
				}
			}
		}
		self.entries.push_back(LedgerEntry {
			tool: tool.to_string(),
			args,
			mutation,
			error,
			bytes,
			repeats: 1,
		});
		if self.entries.len() > LEDGER_CAP {
			self.entries.pop_front();
			self.dropped += 1;
		}
	}

	/// Render the block handed to the verify-gate; empty when nothing ran.
	pub fn render(&self) -> String {
		if self.entries.is_empty() {
			return String::new();
		}
		let mut out = String::new();
		if self.dropped > 0 {
			out.push_str(&format!("(+{} earlier actions dropped)\n", self.dropped));
		}
		for e in &self.entries {
			let kind = if e.mutation { "[mut]" } else { "[read]" };
			let outcome = if e.error { "ERROR" } else { "ok" };
			out.push_str(&format!(
				"{} {} {} → {} ({})",
				kind,
				e.tool,
				e.args,
				outcome,
				fmt_size(e.bytes)
			));
			if e.repeats > 1 {
				out.push_str(&format!(" ×{}", e.repeats));
			}
			out.push('\n');
		}
		out
	}
}

/// Render the SESSION CONTEXT block for the verifier: the durable goal (anchor
/// intent) and the live plan checklist. This is what lets the gate verify a
/// terse follow-up turn ("continue") against the real goal instead of the
/// fragment. Empty when neither exists — short single-task sessions hand the
/// gate nothing extra.
pub fn render_session_context(intent: &str, plan: Option<&str>) -> String {
	let intent = intent.trim();
	let plan = plan.map(str::trim).filter(|p| !p.is_empty());
	if intent.is_empty() && plan.is_none() {
		return String::new();
	}
	let mut s = String::new();
	if !intent.is_empty() {
		s.push_str("Session goal: ");
		s.push_str(intent);
		s.push('\n');
	}
	if let Some(p) = plan {
		s.push_str(p);
		s.push('\n');
	}
	s
}

/// Marker embedded in the plan pre-gate advisory so re-runs within the same
/// turn don't nudge twice (mirrors the mutation pre-gate marker).
pub const PLAN_GATE_MARKER: &str = "octomind:pre_gate_open_plan";

/// Advisory injected when `done` is self-reported while the live plan still
/// has open items — the drift-by-omission failure: parts of the decomposed
/// task silently dropped. Free and deterministic; shares the gate budget.
pub fn format_plan_advisory(open: &[String]) -> String {
	let mut s = format!(
		"<pay-attention>\n<!-- {PLAN_GATE_MARKER} -->\nYou reported done, but your plan still has open items:\n"
	);
	for t in open {
		s.push_str("- ");
		s.push_str(t);
		s.push('\n');
	}
	s.push_str(
		"The task is not done while its plan is open. For each item: do the work and mark it complete (plan `next`), or — if it is already covered or no longer applies — close it out via the plan tool (`next` with a one-line reason, or `done`/`reset` for the whole plan if it is obsolete). Then re-report your status.\n</pay-attention>",
	);
	s
}

/// Compact byte-size hint for a tool result (`412b`, `2.3k`).
fn fmt_size(bytes: usize) -> String {
	if bytes >= 1024 {
		format!("{:.1}k", bytes as f64 / 1024.0)
	} else {
		format!("{bytes}b")
	}
}

/// Verify a self-reported completion. `task` is the user's request, `result` is
/// the agent's final answer, `claim` is the agent's own stated reason from its
/// `done` self-report (checked against the result), `actions` is the rendered
/// [`EvidenceLedger`] (empty when no tools ran — pure-reasoning tasks),
/// `context` is the rendered [`render_session_context`] block (empty when the
/// session has no durable goal or plan yet). Fails open (PASS) on empty input
/// or LLM error — a verifier outage must never block the agent.
pub async fn verify(
	config: &Config,
	task: &str,
	result: &str,
	claim: Option<&str>,
	actions: &str,
	context: &str,
	operation_rx: watch::Receiver<bool>,
) -> GateVerdict {
	if task.trim().is_empty() || result.trim().is_empty() {
		return GateVerdict::Pass;
	}
	let claim_line = match claim {
		Some(c) if !c.trim().is_empty() => format!("\n\nAGENT'S STATED CLAIM: {c}"),
		_ => String::new(),
	};
	let actions_block = if actions.trim().is_empty() {
		String::new()
	} else {
		format!("\n\nRECORDED ACTIONS:\n{actions}")
	};
	let context_block = if context.trim().is_empty() {
		String::new()
	} else {
		format!("\n\nSESSION CONTEXT:\n{context}")
	};
	let user = format!(
		"USER REQUEST:\n{task}{context_block}\n\nAGENT FINAL RESULT:\n{result}{claim_line}{actions_block}"
	);
	// Verify with a deliberately separate (ideally different-family) model — a
	// same-family verifier shares the generator's blind spots and rubber-stamps
	// them. Strict config guarantees this is set; no fallback to the generator.
	let model = config.supervisor.gate.verifier_model.clone();
	match crate::supervisor::learning::extract::call_learning_llm(
		config,
		&model,
		GATE_PROMPT.to_string(),
		user,
		crate::supervisor::stats::CallKind::Gate,
		operation_rx,
	)
	.await
	{
		Ok(resp) => parse_verdict(&resp),
		Err(e) => {
			crate::log_debug!("Verify-gate call failed, accepting: {}", e);
			GateVerdict::Pass
		}
	}
}

fn parse_verdict(resp: &str) -> GateVerdict {
	if resp.contains("<verdict>PASS</verdict>") {
		return GateVerdict::Pass;
	}
	let mut gaps = Vec::new();
	let mut rest = resp;
	while let Some(s) = rest.find("<gap>") {
		let after = &rest[s + 5..];
		let Some(e) = after.find("</gap>") else {
			break;
		};
		let g = after[..e].trim();
		if !g.is_empty() {
			gaps.push(g.to_string());
		}
		rest = &after[e + 6..];
	}
	if gaps.is_empty() {
		GateVerdict::Pass
	} else {
		GateVerdict::Gaps(gaps)
	}
}

/// Build the out-of-band advisory injected back into the loop on gaps.
pub fn format_advisory(gaps: &[String]) -> String {
	let mut s = String::from(
		"<pay-attention>\nYou reported this task complete, but a verification pass found gaps before it can be accepted as done:\n",
	);
	for g in gaps {
		s.push_str("- ");
		s.push_str(g);
		s.push('\n');
	}
	s.push_str(
		"The task is not done until each gap is closed. For each, do the work, then cite the concrete evidence that closes it — the file and line, the passing test, or the command output. If a gap is already satisfied, point to that exact evidence rather than describing it. If a gap is wrong or out of scope, say so and why. Then re-report your status.\n</pay-attention>",
	);
	s
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn pass_parsed() {
		assert_eq!(parse_verdict("<verdict>PASS</verdict>"), GateVerdict::Pass);
	}

	#[test]
	fn gaps_parsed() {
		let v = parse_verdict("<gap>no tests</gap>\n<gap>missing docs</gap>");
		assert_eq!(
			v,
			GateVerdict::Gaps(vec!["no tests".into(), "missing docs".into()])
		);
	}

	#[test]
	fn no_markers_is_pass() {
		assert_eq!(parse_verdict("looks good to me"), GateVerdict::Pass);
	}

	#[test]
	fn ledger_renders_mutations_reads_and_errors() {
		let mut l = EvidenceLedger::default();
		l.record(
			"edit",
			&serde_json::json!({"path":"src/a.rs"}),
			true,
			false,
			100,
		);
		l.record(
			"shell",
			&serde_json::json!({"command":"cargo test"}),
			false,
			true,
			2048,
		);
		let r = l.render();
		assert!(r.contains(r#"[mut] edit {"path":"src/a.rs"} → ok (100b)"#));
		assert!(r.contains(r#"[read] shell {"command":"cargo test"} → ERROR (2.0k)"#));
	}

	#[test]
	fn ledger_collapses_only_identical_successful_repeats() {
		let mut l = EvidenceLedger::default();
		let p = serde_json::json!({"path":"a"});
		l.record("view", &p, false, false, 10);
		l.record("view", &p, false, false, 10);
		l.record("view", &serde_json::json!({"path":"b"}), false, false, 10);
		let r = l.render();
		assert!(r.contains("×2"));
		assert_eq!(r.lines().count(), 2);
	}

	#[test]
	fn ledger_never_collapses_errors() {
		let mut l = EvidenceLedger::default();
		let p = serde_json::json!({"command":"x"});
		l.record("shell", &p, false, true, 10);
		l.record("shell", &p, false, true, 10);
		assert_eq!(l.render().lines().count(), 2);
	}

	#[test]
	fn ledger_caps_and_counts_dropped() {
		let mut l = EvidenceLedger::default();
		for i in 0..130 {
			l.record("view", &serde_json::json!({ "i": i }), false, false, 1);
		}
		let r = l.render();
		assert!(r.starts_with("(+2 earlier actions dropped)"));
		assert_eq!(r.lines().count(), 129); // 128 entries + dropped header
	}

	#[test]
	fn ledger_truncates_long_args() {
		let mut l = EvidenceLedger::default();
		let big = "x".repeat(500);
		l.record(
			"write",
			&serde_json::json!({ "content": big }),
			true,
			false,
			1,
		);
		assert!(l.render().contains('…'));
	}

	#[test]
	fn session_context_empty_when_no_goal_or_plan() {
		assert_eq!(render_session_context("", None), "");
		assert_eq!(render_session_context("  ", Some("  ")), "");
	}

	#[test]
	fn session_context_renders_goal_and_plan() {
		let c = render_session_context(
			"Ship the feature",
			Some("Live plan (1/2 done):\n✅ a\n🔄 b ← current"),
		);
		assert!(c.starts_with("Session goal: Ship the feature\n"));
		assert!(c.contains("🔄 b ← current"));
		// Each part also renders alone.
		assert_eq!(
			render_session_context("Ship it", None),
			"Session goal: Ship it\n"
		);
		assert_eq!(render_session_context("", Some("plan")), "plan\n");
	}

	#[test]
	fn plan_advisory_lists_items_and_carries_marker() {
		let a = format_plan_advisory(&["wire it up".into(), "add tests".into()]);
		assert!(is_supervisor_injection(&a));
		assert!(a.contains(PLAN_GATE_MARKER));
		assert!(a.contains("- wire it up\n"));
		assert!(a.contains("- add tests\n"));
	}

	#[test]
	fn empty_ledger_renders_empty() {
		let mut l = EvidenceLedger::default();
		assert_eq!(l.render(), "");
		l.record("view", &serde_json::json!({}), false, false, 1);
		l.reset();
		assert_eq!(l.render(), "");
	}
}
