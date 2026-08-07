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

// Range determination for compression: pick which message indices get drained,
// and price the chosen range in tokens. Pure functions over `&[Message]` /
// `ChatSession`; no LLM call, no persisted state.
//
// Anchor selection is purely structural and re-derived on every call — no
// `first_prompt_idx` cache, no resume-time bootstrap detection, no Some/None
// branching. One deterministic rule:
//
//   anchor = the last message of the immutable preamble, i.e. the index just
//   before the FIRST message that states a task.
//
// "States a task" means a real user turn, a prior compression summary, or a
// prior continuation wrapper — every message shape that can tell the model
// what it is supposed to be doing. Draining from the first of them is what
// makes compaction safe: the session's opening ask, older summaries, and the
// previous cycle's continuation wrapper all fold into the NEW summary instead
// of surviving beside it as competing instructions. Keeping the opening ask
// verbatim (the old fallback rule) made the model abandon the live task and
// re-execute the first thing it was ever asked.
//
// The preamble that survives is exactly the non-task scaffolding: system
// prompt, welcome message, and the `<instructions>` file — none of which
// claims to be the current request.
//
// No tool-skip dance: the message at the anchor is always the one preceding a
// task-stating message, so it can never be an assistant still awaiting tool
// results (a tool result must immediately follow its assistant message).

use crate::session::chat::session::ChatSession;
use anyhow::Result;

/// True when a message can state or restate "what the user wants": a real user
/// turn, a prior compression summary, or a prior continuation wrapper.
///
/// This is the compression boundary. Every such message must end up INSIDE the
/// drained range so that after compaction exactly one statement of the active
/// task survives — the fresh continuation wrapper.
fn states_task(m: &crate::session::Message) -> bool {
	crate::session::is_real_user_task_message(m)
		|| (m.role == "user" && super::apply::is_continuation_message(&m.content))
		|| (m.role == "assistant"
			&& m.name.as_deref() == Some(super::apply::COMPRESSION_MESSAGE_NAME))
}

/// Find the compression range deterministically from message structure.
///
/// Returns `(anchor_idx, end_idx)` where:
/// - `anchor_idx` is KEPT (compression drains `anchor_idx+1..=end_idx`)
/// - `end_idx = messages.len() - 1`
///
/// Returns `(0, 0)` when there is nothing meaningful to compress (no task-
/// stating message, no preamble to anchor on, too few conversational messages,
/// or the anchor is already at the tail).
pub(super) fn find_compression_range(
	messages: &[crate::session::Message],
	force: bool,
) -> Result<(usize, usize)> {
	let Some(first_task) = messages.iter().position(states_task) else {
		return Ok((0, 0));
	};

	// No preamble in front of it (no system prompt) — there is nothing we could
	// keep as an anchor, and index 0 must survive for the drain to be expressible.
	if first_task == 0 {
		return Ok((0, 0));
	}
	let start_idx = first_task - 1;

	let end_idx = messages.len() - 1;

	// Minimum conversation messages to justify compression.
	// Need at least 5 (non-force) or 3 (force/done) to produce a useful summary.
	let min_conv = if force { 3 } else { 5 };
	let conv_count = messages
		.iter()
		.skip(start_idx)
		.filter(|m| m.role == "user" || m.role == "assistant")
		.count();
	if conv_count < min_conv {
		return Ok((0, 0));
	}

	if start_idx >= end_idx {
		return Ok((0, 0));
	}

	Ok((start_idx, end_idx))
}

/// Calculate tokens in message range using accurate token counting.
/// Counts ALL message fields: content, tool_calls, thinking, images, etc.
///
/// CRITICAL: The range [start_idx, end_idx] must match the messages that will
/// actually be removed. In compression, remove_messages_in_range drains
/// start_idx+1..=end_idx, so callers should pass (start_idx+1, end_idx).
pub(super) fn calculate_range_tokens(
	session: &ChatSession,
	start_idx: usize,
	end_idx: usize,
) -> Result<u64> {
	let mut total_tokens = 0u64;

	if start_idx >= session.session.messages.len() {
		return Err(anyhow::anyhow!("Invalid start_index in range"));
	}

	if end_idx >= session.session.messages.len() {
		return Err(anyhow::anyhow!("Invalid end_index in range"));
	}

	for i in start_idx..=end_idx {
		if let Some(message) = session.session.messages.get(i) {
			let tokens = crate::session::estimate_message_tokens(message) as u64;
			total_tokens += tokens;
		}
	}

	Ok(total_tokens)
}
