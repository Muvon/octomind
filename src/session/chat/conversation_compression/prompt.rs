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

use super::knowledge::strip_file_context_from_summary;
use crate::session::chat::session::ChatSession;

/// This combines decision + summarization to reduce latency and cost by 50%
/// Ask AI: should we compress AND get summary in ONE call (1-hop optimization)
/// This combines decision + summarization to reduce latency and cost by 50%.
/// When `force=true` the AI is only asked to summarize — it has no right to say NO.
/// Build the system and user prompt for the compression AI call.
///
/// Returns `(system_content, user_content)`.
/// `force=true` produces a direct-summary prompt (no YES/NO gate).
pub(super) fn build_compression_prompt(
	session: &ChatSession,
	messages_to_compress: &[crate::session::Message],
	force: bool,
	target_ratio: f64,
) -> (String, String) {
	// SYSTEM: role identity + instructions (what the model must do and how to respond).
	// Kept separate from the data so the model acts as a compressor, not a session participant.
	//
	// SINGLE PATH: always produce a full summary when compressing — no silent YES/NO-only fallback.
	// The prompt encodes three priorities:
	//   1. CURRENT TASK — the user's most recent request dominates; older tasks compress aggressively.
	//   2. RECENCY — messages marked [RECENT] are preserved with highest fidelity.
	//   3. TOOL CALLS — secondary context, reduced to one-liners.
	let system_content = if force {
		"You are a conversation compressor. \
The user has explicitly requested compression. You MUST produce a summary — do NOT refuse. \
Do not start with YES or NO. Just write the summary directly using the format below.\n\n\
## CRITICAL PRIORITIES\n\n\
**Priority 1 — CURRENT TASK**: The user's MOST RECENT task/request is what matters most. \
If the user pivoted to a new topic mid-conversation, the new topic IS the current intent. \
Older completed/abandoned tasks can be compressed to a single line each.\n\n\
**Priority 2 — RECENCY**: Messages marked [RECENT] represent the current state of work. \
Preserve them with the highest fidelity — quote or closely paraphrase. \
Older messages can be compressed aggressively.\n\n\
**Priority 3 — TOOL CALLS are secondary**: Summarize what was done in one line each.\n\n\
## SUMMARY FORMAT\n\n\
**SESSION CONTEXT** (1 sentence):\n\
Brief overview of the session — what brought us here. Keep it short.\n\n\
**CURRENT TASK** (1-2 sentences):\n\
What is the user working on RIGHT NOW? This is the most recent request — highlight it as the primary focus.\n\n\
**PROGRESS** (2-4 sentences):\n\
What was completed for the current task? What is in progress? What was the outcome?\n\n\
**ANALYSIS FINDINGS** (preserve conclusions — this prevents re-doing work):\n\
Capture key findings from code analysis, debugging, or investigation. Include:\n\
- What was discovered (root causes, patterns, behaviors)\n\
- Specific code locations and what was found there\n\
- Conclusions drawn from tool results\n\
This section is CRITICAL — without it, the AI will re-read the same files to rediscover the same things.\n\n\
**RECENT EXCHANGES** (preserve with high fidelity — the most recent [RECENT] messages):\n\
For each recent user/assistant pair: quote or closely paraphrase.\n\n\
**KEY ENTITIES** (preserve exactly — copy values verbatim):\n\
- Files/paths: exact file paths, line numbers, code locations\n\
- Names: identifiers, function names, variable names, config keys\n\
- Errors/issues: problems encountered and their status\n\
- Decisions: choices made with reasoning\n\n\
**NEXT STEPS** (1-2 sentences):\n\
What needs to happen next to continue the current task?\n\n\
**FILE CONTEXT — files to auto-inject after compression (IMPORTANT):**\n\
Files listed in <context> tags will be AUTO-READ from disk and injected verbatim into the compressed summary. \
This is how the session retains real file content across compressions without re-reading. \
Include any file the session is actively working on or needs to continue.\n\
<context>\n\
filepath:startline:endline\n\
</context>\n\
Rules: <context> tags required; one entry per line as filepath:N:N (no spaces); \
paths from project root; line numbers 1–10000; max 5 ranges; prioritize files being edited or analyzed.\n\n\
**CRITICAL KNOWLEDGE — survives all future compressions:**\n\
If there is critical knowledge that MUST survive future compressions \
(e.g., a key architectural decision, a non-obvious constraint, a user preference, \
analysis conclusions, root cause findings), write it in a <knowledge> tag. \
2-3 sentences MAX. Only include if truly critical — not routine progress.\n\
<knowledge>\n\
Your critical insight here (2-3 sentences max).\n\
</knowledge>\n\n\
"
	} else {
		"You are a conversation compressor. \
Your job is to produce a lossless summary of a conversation transcript so the session can continue \
without losing any important context.\n\n\
## CRITICAL PRIORITIES (read carefully before summarizing)\n\n\
**Priority 1 — CURRENT TASK**: The user's MOST RECENT task/request is what matters most. \
If the user pivoted to a new topic mid-conversation, the new topic IS the current intent. \
Older completed/abandoned tasks can be compressed to a single line each.\n\n\
**Priority 2 — RECENCY**: Messages marked [RECENT] represent the current state of work. \
Preserve them with the highest fidelity — quote or closely paraphrase. \
Older messages without [RECENT] can be compressed aggressively.\n\n\
**Priority 3 — TOOL CALLS are secondary**: Summarize what was done in one line each \
(e.g. 'read file X', 'ran shell command Y, got Z'). Never reproduce full tool output.\n\n\
## WHEN TO ANSWER YES vs NO\n\n\
Answer YES if there are older exchanges that can be compressed without losing information needed \
to continue. Answer NO only if the transcript is already minimal and nothing can be safely reduced.\n\n\
## SUMMARY FORMAT (use when answering YES)\n\n\
**SESSION CONTEXT** (1 sentence):\n\
Brief overview of the session — what brought us here. Keep it short.\n\n\
**CURRENT TASK** (1-2 sentences):\n\
What is the user working on RIGHT NOW? This is the most recent request — highlight it as the primary focus.\n\n\
**PROGRESS** (2-4 sentences):\n\
What was completed for the current task? What is in progress? What was the outcome?\n\n\
**ANALYSIS FINDINGS** (preserve conclusions — this prevents re-doing work):\n\
Capture key findings from code analysis, debugging, or investigation. Include:\n\
- What was discovered (root causes, patterns, behaviors)\n\
- Specific code locations and what was found there\n\
- Conclusions drawn from tool results\n\
This section is CRITICAL — without it, the AI will re-read the same files to rediscover the same things.\n\n\
**RECENT EXCHANGES** (preserve with high fidelity — the most recent [RECENT] messages):\n\
For each recent user/assistant pair: quote or closely paraphrase. Do not compress these.\n\n\
**KEY ENTITIES** (preserve exactly — copy values verbatim):\n\
- Files/paths: exact file paths, line numbers, code locations\n\
- Names: identifiers, function names, variable names, config keys\n\
- Errors/issues: problems encountered and their status\n\
- Decisions: choices made with reasoning\n\n\
**NEXT STEPS** (1-2 sentences):\n\
What needs to happen next to continue the current task?\n\n\
## RESPONSE FORMAT\n\n\
Start with YES or NO on the first line.\n\
If YES, follow immediately with the summary using the sections above:\n\n\
YES\n\
**SESSION CONTEXT**: ...\n\
**CURRENT TASK**: ...\n\
**PROGRESS**: ...\n\
**ANALYSIS FINDINGS**:\n\
- [finding 1]\n\
- [finding 2]\n\
**RECENT EXCHANGES**:\n\
- User: [question] → Assistant: [answer]\n\
**KEY ENTITIES**:\n\
- Files/paths: ...\n\
- Errors/issues: ...\n\
- Decisions: ...\n\
**NEXT STEPS**: ...\n\n\
**FILE CONTEXT — files to auto-inject after compression (IMPORTANT):**\n\
Files listed in <context> tags will be AUTO-READ from disk and injected verbatim into the compressed summary. \
This is how the session retains real file content across compressions without re-reading. \
Include any file the session is actively working on or needs to continue.\n\
<context>\n\
filepath:startline:endline\n\
</context>\n\
Rules: <context> tags required; one entry per line as filepath:N:N (no spaces); \
paths from project root; line numbers 1–10000; max 5 ranges; prioritize files being edited or analyzed.\n\n\
**CRITICAL KNOWLEDGE — survives all future compressions:**\n\
If there is critical knowledge that MUST survive future compressions \
(e.g., a key architectural decision, a non-obvious constraint, a user preference, \
analysis conclusions, root cause findings), write it in a <knowledge> tag. \
2-3 sentences MAX. Only include if truly critical — not routine progress.\n\
<knowledge>\n\
Your critical insight here (2-3 sentences max).\n\
</knowledge>\n\n\
If NO, respond with just: NO"
	}
	.to_string();

	// USER: plain-text transcript of the range being compressed + semantic chunk hints.
	// Building a transcript (not raw messages) prevents the model from continuing the
	// tool-calling loop — it sees text to analyze, not a live conversation to participate in.
	//
	// RECENCY MARKER: the last 8 messages (min 4, max 8) are tagged [RECENT] so the AI
	// knows to preserve them with the highest fidelity. Capped at 8 to prevent the
	// RECENT window from growing so large it defeats compression on long sessions.
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
	let mut user_content = format!(
		"**COMPRESSION TARGET**: Reduce this transcript to ~{}% of its original size ({:.1}x compression). \
Be {} in what you preserve.\n\n\
**Conversation transcript to compress:**\n\
NOTE: Messages marked [RECENT] are the most recent and most important — preserve them with \
highest fidelity. [USER]/[ASSISTANT] pairs are primary signal; [TOOL CALL]/[TOOL RESULT] are \
secondary context.\n\n",
		reduction_pct, target_ratio, aggressiveness,
	);

	// Inject accumulated critical knowledge from prior compressions
	if !session.critical_knowledge.is_empty() {
		user_content
			.push_str("**CRITICAL KNOWLEDGE (from prior compressions — MUST be preserved):**\n");
		for (i, knowledge) in session.critical_knowledge.iter().enumerate() {
			user_content.push_str(&format!("{}. {}\n", i + 1, knowledge));
		}
		user_content.push('\n');
	}

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
							super::file_context::extract_file_refs_from_args(
								name,
								args,
								&mut file_refs,
							);
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

	// Append file references extracted from tool calls
	// These allow the model to re-read critical files after compression
	if !file_refs.is_empty() {
		// Merge overlapping ranges and dedupe
		let merged_refs = super::file_context::merge_file_refs(&file_refs);
		if !merged_refs.is_empty() {
			user_content.push_str("\n**File references (can be re-read on demand):**\n");
			// Limit to prevent bloat
			for ref_str in merged_refs.iter().take(10) {
				user_content.push_str(&format!("- {}\n", ref_str));
			}
		}
	}

	(system_content, user_content)
}
