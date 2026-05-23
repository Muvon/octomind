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

// Build the (system, user) prompt pair sent to the compression LLM. Pure
// string assembly — no LLM call, no session mutation. Kept apart from the
// AI invocation in `ai.rs` so prompt tuning and AI orchestration can evolve
// independently.
//
// Output shape is enforced by JSON schema (see `schema::build_compression_schema`),
// not by markdown templates embedded in the prompt. As a result this prompt
// is dramatically shorter than the old free-form version — it only carries
// *behavioural guidance* the schema cannot express (priorities, scaffold
// rules, recency semantics).

use super::knowledge::{strip_file_context_from_summary, SUMMARY_TAG_OPEN_PREFIX};
use crate::session::chat::file_context;
use crate::session::chat::session::ChatSession;

/// Build the system and user prompt for the compression AI call.
///
/// Returns `(system_content, user_content)`.
///
/// The system content is byte-identical across every compression call that
/// shares the same `force` value. `ai.rs` flags it as cached so the provider
/// can amortise it across calls — a small but real cost win for sessions
/// that compress multiple times.
pub(super) fn build_compression_prompt(
	session: &ChatSession,
	messages_to_compress: &[crate::session::Message],
	force: bool,
	target_ratio: f64,
) -> (String, String) {
	// The response shape is defined by the JSON schema attached to the call.
	// What this prompt provides is *behaviour*: how to choose what to put in
	// each field, what to carry forward, what to drop. Field-level descriptions
	// live in the schema; cross-field rules and priorities live here.
	let force_directive = if force {
		"\n<forced>\nThe user has explicitly requested compression. Set should_compress to true and fill every field. Refusal is not an option.\n</forced>"
	} else {
		""
	};

	let system_content = format!(
		"<role>
You are a conversation compressor. Read a conversation transcript and emit a faithful structured summary so the session can continue with full working context. Your output is validated against a strict JSON schema — field shapes and constraints are documented there.
</role>

<priorities>
1. The user's MOST RECENT request is the active task — preserve it precisely.
2. Messages tagged [RECENT] reflect current state — paraphrase closely, keep concrete details.
3. Older exchanges and tool activity are secondary — compress them aggressively.
4. File paths, line numbers, identifiers, and error strings — copy verbatim from the transcript.
5. User negative feedback (\"don't do X\", \"stop doing Y\") is the HIGHEST preservation priority — never lose a correction.
</priorities>

<scaffold_rules>
If the transcript contains a prior <conversation_summary id=\"…\">…</conversation_summary> block, treat its content as established facts that must carry forward:
- original_request: copy from the prior summary unchanged. Otherwise quote verbatim from the very first user turn.
- analysis_findings, errors_and_corrections, critical_knowledge: carry forward all prior entries, append new ones.
- progress: extend (do not replace) the prior progress narrative.
- current_task, next_steps: replace based on the most recent transcript.
</scaffold_rules>

<recency>
Messages tagged [RECENT] are the most recent and most important — preserve them with highest fidelity. [USER] and [ASSISTANT] turns are primary signal. [TOOL CALL] and [TOOL RESULT] entries are secondary context.
</recency>{force_directive}",
	);

	// USER message: longform transcript first, task instruction at the bottom
	// (Anthropic long-context best practice: query-at-end can lift quality up
	// to 30% on complex inputs).
	//
	// RECENCY MARKER: the last 8 messages (min 4, max 8) are tagged [RECENT]
	// so the AI knows which span to preserve with highest fidelity. Capped at
	// 8 so the RECENT window can't grow to swallow the whole transcript on
	// long sessions and defeat compression.
	let total_msgs = messages_to_compress.len();
	let recent_count = (total_msgs / 4).clamp(4, 8);
	let recent_start = total_msgs.saturating_sub(recent_count);

	let reduction_pct = ((1.0 - 1.0 / target_ratio) * 100.0) as u32;
	let aggressiveness = if target_ratio >= 4.0 {
		"very aggressive"
	} else if target_ratio >= 2.0 {
		"selective"
	} else {
		"gentle"
	};

	let mut user_content = String::new();

	// 1. Prior critical knowledge — short meta-context that must persist across
	//    compressions. Placed before the transcript so the model reads the
	//    transcript already aware of must-preserve facts. These facts must
	//    appear in the emitted `critical_knowledge` array verbatim.
	if !session.critical_knowledge.is_empty() {
		user_content.push_str("<prior_knowledge>\n");
		user_content
			.push_str("From earlier compressions of this session — these facts must survive into the new summary's critical_knowledge:\n");
		for (i, knowledge) in session.critical_knowledge.iter().enumerate() {
			user_content.push_str(&format!("{}. {}\n", i + 1, knowledge));
		}
		user_content.push_str("</prior_knowledge>\n\n");
	}

	// 2. Transcript — the longform data. Building a labelled text transcript
	//    (not raw messages) keeps the model from continuing the tool-calling
	//    loop — it sees text to analyse, not a live conversation to join.
	user_content.push_str("<transcript>\n");

	let mut file_refs: Vec<String> = Vec::new();

	for (idx, msg) in messages_to_compress.iter().enumerate() {
		let recent = if idx >= recent_start { "[RECENT] " } else { "" };
		match msg.role.as_str() {
			"system" => {} // skip system — already in our system message
			"assistant" => {
				// If this is a prior compressed summary, drop its <file_context>
				// block before re-feeding. The file bytes are stale; the new
				// compression cycle will re-request whatever it still needs via
				// the structured `file_context` field. Re-embedding the old
				// content would bloat the prompt and recursively grow each
				// summary.
				let assistant_text = if msg
					.content
					.trim_start()
					.starts_with(SUMMARY_TAG_OPEN_PREFIX)
				{
					strip_file_context_from_summary(&msg.content)
				} else {
					msg.content.trim().to_string()
				};
				if !assistant_text.is_empty() {
					user_content.push_str(&format!("{}[ASSISTANT]: {}\n", recent, assistant_text));
				}
				if let Some(calls) = msg.tool_calls.as_ref().and_then(|v| v.as_array()) {
					for call in calls {
						let name = call
							.get("function")
							.and_then(|f| f.get("name"))
							.and_then(|n| n.as_str())
							.unwrap_or("unknown");

						let key_arg = call
							.get("function")
							.and_then(|f| f.get("arguments"))
							.and_then(|a| {
								let obj = if let Some(s) = a.as_str() {
									serde_json::from_str::<serde_json::Value>(s).ok()
								} else {
									Some(a.clone())
								};
								obj.and_then(|o| {
									for key in &[
										"path", "paths", "query", "command", "pattern", "content",
										"task",
									] {
										if let Some(v) = o.get(key) {
											let s = match v {
												serde_json::Value::String(s) => s.clone(),
												serde_json::Value::Array(arr) => arr
													.iter()
													.filter_map(|x| x.as_str())
													.take(2)
													.collect::<Vec<_>>()
													.join(", "),
												_ => continue,
											};
											if !s.is_empty() {
												let hint = if s.len() > 80 {
													let end = s
														.char_indices()
														.map(|(i, _)| i)
														.take_while(|&i| i <= 80)
														.last()
														.unwrap_or(0);
													format!("{}\u{2026}", &s[..end])
												} else {
													s
												};
												return Some(hint);
											}
										}
									}
									None
								})
							})
							.unwrap_or_default();

						if key_arg.is_empty() {
							user_content.push_str(&format!("{}[TOOL CALL]: {}\n", recent, name));
						} else {
							user_content.push_str(&format!(
								"{}[TOOL CALL]: {}({})\n",
								recent, name, key_arg
							));
						}

						if let Some(args) = call.get("function").and_then(|f| f.get("arguments")) {
							file_context::extract_file_refs_from_args(name, args, &mut file_refs);
						}
					}
				}
			}
			"tool" => {
				let name = msg.name.as_deref().unwrap_or("tool");
				let content = msg.content.trim();
				// Preserve both the start (tool name/context) and the end
				// (errors/results). Errors typically appear at the tail —
				// head-only truncation hides them.
				let truncated = if content.len() > 1500 {
					let head_end = content
						.char_indices()
						.map(|(i, _)| i)
						.take_while(|&i| i <= 600)
						.last()
						.unwrap_or(0);
					let tail_start = content
						.char_indices()
						.rev()
						.map(|(i, _)| i)
						.take_while(|&i| content.len() - i <= 900)
						.last()
						.unwrap_or(content.len());
					if head_end < tail_start {
						format!(
							"{}\u{2026}[truncated]\u{2026}{}",
							&content[..head_end],
							&content[tail_start..]
						)
					} else {
						content[..head_end].to_string()
					}
				} else {
					content.to_string()
				};
				user_content.push_str(&format!(
					"{}[TOOL RESULT: {}]: {}\n",
					recent, name, truncated
				));
			}
			_ => {
				if !msg.content.trim().is_empty() {
					user_content.push_str(&format!("{}[USER]: {}\n", recent, msg.content.trim()));
				}
			}
		}
	}

	user_content.push_str("</transcript>\n");

	// 3. File references extracted from tool calls — candidate ranges the
	//    next turn can re-read on demand. Placed between the transcript and
	//    the task so the model sees them while populating `file_context`.
	if !file_refs.is_empty() {
		let merged_refs = file_context::merge_file_refs(&file_refs);
		if !merged_refs.is_empty() {
			user_content.push_str("\n<file_references>\n");
			user_content.push_str(
				"Files touched by tool calls in this transcript (candidates for file_context):\n",
			);
			for ref_str in merged_refs.iter().take(10) {
				user_content.push_str(&format!("- {}\n", ref_str));
			}
			user_content.push_str("</file_references>\n");
		}
	}

	// 4. Task instruction — at the BOTTOM (Anthropic long-context guidance:
	//    query-at-end lifts quality on complex inputs).
	user_content.push_str(&format!(
		"\n<task>\n\
Compress the transcript above to roughly {pct}% of its original size ({ratio:.1}x compression). Be {agg} in what you preserve.\n\
Emit a single JSON object conforming to the structured-output schema attached to this request.\n\
</task>",
		pct = reduction_pct,
		ratio = target_ratio,
		agg = aggressiveness,
	));

	(system_content, user_content)
}
