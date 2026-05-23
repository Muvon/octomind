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
// AI invocation in `mod.rs` so prompt tuning and AI orchestration can evolve
// independently.
//
// Prompt design follows three proven techniques:
//   1. XML-tag structure for the system prompt (Anthropic: Claude is tuned
//      to attend to XML-delimited sections; reduces section drift).
//   2. Longform-at-top, query-at-bottom layout in the user message
//      (Anthropic: queries at the end can lift quality up to 30% on
//      large/complex inputs).
//   3. Positive phrasing throughout (multiple 2024–2026 studies on the
//      "Pink Elephant" problem: "do not X" routinely degrades performance
//      versus the equivalent "do Y").
// Sentence/paragraph caps are kept (not word caps): models can plan to a
// sentence but cannot reliably count tokens/words.

use super::knowledge::strip_file_context_from_summary;
use crate::session::chat::file_context;
use crate::session::chat::session::ChatSession;

/// Build the system and user prompt for the compression AI call.
///
/// Returns `(system_content, user_content)`.
///
/// Single prompt path — `force` only swaps the trailing `<response_directive>`
/// line (direct summary vs. YES/NO gate). Everything else is identical, so
/// prompt drift between the two paths is impossible.
pub(super) fn build_compression_prompt(
	session: &ChatSession,
	messages_to_compress: &[crate::session::Message],
	force: bool,
	target_ratio: f64,
) -> (String, String) {
	// Response directive — the only branch between force / non-force.
	// Phrased positively: state the desired action, not the forbidden one.
	let response_directive = if force {
		"Write the summary directly. Start your response with the SESSION CONTEXT section."
	} else {
		"If older exchanges can be compressed without losing information needed to continue, write YES on the first line, then the summary. \
If the transcript is already minimal, respond with the single line: NO"
	};

	let system_content = format!(
		"<role>
You are a conversation compressor. Your job: read a conversation transcript and produce a faithful summary so the session can continue with full working context.
</role>

<priorities>
1. The user's MOST RECENT request is the active task — preserve it precisely.
2. Messages tagged [RECENT] reflect current state — paraphrase closely, keep concrete details.
3. Older exchanges and tool activity are secondary — compress them aggressively into one-liners.
4. Copy file paths, line numbers, identifiers, and error strings verbatim from the transcript.
</priorities>

<output_format>
Write a document with these sections, in this order:

SESSION CONTEXT (1 sentence): what brought the session to this point.

CURRENT TASK (1–2 sentences): the user's most recent active request — highlight it as the primary focus.

PROGRESS (2–4 sentences): what was completed for the current task, what is in progress, what was the outcome.

ANALYSIS FINDINGS (3–6 bullets): conclusions from code analysis, debugging, or investigation. Cover what was discovered (root causes, patterns, behaviors), the specific code locations involved, and the conclusions drawn from tool results. Capture these so the next turn skips re-deriving them.

RECENT EXCHANGES (one bullet per [RECENT] turn): closely paraphrase each [RECENT] user/assistant pair. Keep concrete details and decisions intact.

KEY ENTITIES (copy values verbatim from the transcript):
- Files/paths: exact paths with line numbers
- Names: identifiers, function names, variable names, config keys
- Errors/issues: error strings and current status
- Decisions: choices made with their reasoning

NEXT STEPS (1–2 sentences): the concrete action that advances the current task.
</output_format>

<file_context_rules>
List the files the next turn will need to read. These are auto-loaded from disk and re-injected after the summary, so the session retains real file content across compressions.

Format: one entry per line as filepath:startline:endline. Paths from project root. Line numbers between 1 and 10000. Maximum 5 ranges. Prioritize files actively being edited or analyzed.

<context>
filepath:N:N
</context>
</file_context_rules>

<critical_knowledge_rules>
Record insights that must survive future compressions — an architectural decision, a hidden constraint, a user preference, a root-cause finding. Include only truly critical knowledge (not routine progress). 2–3 sentences each.

<knowledge>
Critical insight here (2–3 sentences max).
</knowledge>
</critical_knowledge_rules>

<response_directive>
{response_directive}
</response_directive>"
	);

	// USER: longform transcript first, task instruction last (Anthropic
	// long-context best practice: query-at-bottom gains up to 30% on
	// complex inputs).
	//
	// Building a transcript (not raw messages) prevents the model from
	// continuing the tool-calling loop — it sees text to analyze, not a
	// live conversation to participate in.
	//
	// RECENCY MARKER: the last 8 messages (min 4, max 8) are tagged [RECENT]
	// so the AI knows to preserve them with the highest fidelity. Capped at
	// 8 to prevent the RECENT window from growing so large it defeats
	// compression on long sessions.
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

	// 1. Prior critical knowledge — short meta-context, placed before the
	//    transcript so the model reads the transcript already aware of
	//    must-preserve facts.
	if !session.critical_knowledge.is_empty() {
		user_content.push_str("<prior_knowledge>\n");
		user_content
			.push_str("From earlier compressions of this session — these facts must survive into the new summary:\n");
		for (i, knowledge) in session.critical_knowledge.iter().enumerate() {
			user_content.push_str(&format!("{}. {}\n", i + 1, knowledge));
		}
		user_content.push_str("</prior_knowledge>\n\n");
	}

	// 2. Transcript — the longform data, opens with a short reader hint then
	//    the labelled exchanges. Wrapped in <transcript> so the model knows
	//    exactly which span is the input to summarize.
	user_content.push_str("<transcript>\n");
	user_content.push_str(
		"Messages tagged [RECENT] are the most recent and most important — preserve them with highest fidelity. \
[USER] and [ASSISTANT] turns are primary signal. [TOOL CALL] and [TOOL RESULT] entries are secondary context.\n\n",
	);

	// Collect file references from tool calls for context preservation
	// These can be re-read on demand after compression
	let mut file_refs: Vec<String> = Vec::new();

	for (idx, msg) in messages_to_compress.iter().enumerate() {
		let recent = if idx >= recent_start { "[RECENT] " } else { "" };
		match msg.role.as_str() {
			"system" => {} // skip system — already in our system message
			"assistant" => {
				// Include text content; summarize tool calls as one-liners with key arg.
				// CRITICAL: If this is a prior compressed summary, strip the FILE CONTEXT
				// section before including it — file content will be re-read fresh by the
				// new compression. Including stale XML bloats the prompt and causes the AI
				// to re-embed the same file bytes in every subsequent summary.
				let assistant_text = if msg
					.content
					.starts_with("## Conversation Summary [COMPRESSED:")
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

						// Extract a short key-arg hint (first path/query/command arg) so the
						// AI understands what the tool was operating on, not just its name.
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
									// Try common key-arg field names in priority order
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

						// Extract file references from tool arguments
						// These allow the model to re-read files after compression
						if let Some(args) = call.get("function").and_then(|f| f.get("arguments")) {
							file_context::extract_file_refs_from_args(name, args, &mut file_refs);
						}
					}
				}
			}
			"tool" => {
				let name = msg.name.as_deref().unwrap_or("tool");
				let content = msg.content.trim();
				// Preserve both the start (tool name/context) and the end (errors/results).
				// Errors typically appear at the tail — head-only truncation hides them.
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
				// user messages — always include, never drop
				if !msg.content.trim().is_empty() {
					user_content.push_str(&format!("{}[USER]: {}\n", recent, msg.content.trim()));
				}
			}
		}
	}

	// Close the <transcript> block.
	user_content.push_str("</transcript>\n");

	// 3. File references extracted from tool calls — candidate ranges the
	//    next turn can re-read on demand. Placed between the transcript and
	//    the task so the model sees them while deciding which files to
	//    include under <file_context_rules>.
	if !file_refs.is_empty() {
		let merged_refs = file_context::merge_file_refs(&file_refs);
		if !merged_refs.is_empty() {
			user_content.push_str("\n<file_references>\n");
			user_content
				.push_str("Files touched by tool calls in this transcript (candidates for the <context> tag in your output):\n");
			for ref_str in merged_refs.iter().take(10) {
				user_content.push_str(&format!("- {}\n", ref_str));
			}
			user_content.push_str("</file_references>\n");
		}
	}

	// 4. Task instruction — placed at the BOTTOM of the user message
	//    (Anthropic long-context guidance: query-at-end lifts quality on
	//    complex inputs). Concrete compression target and writing posture.
	user_content.push_str(&format!(
		"\n<task>\n\
Compress the transcript above to roughly {pct}% of its original size ({ratio:.1}x compression). Be {agg} in what you preserve.\n\
Follow the output format from your instructions exactly. Apply the response directive verbatim.\n\
</task>",
		pct = reduction_pct,
		ratio = target_ratio,
		agg = aggressiveness,
	));

	(system_content, user_content)
}
