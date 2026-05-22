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
// branching. Two-rule deterministic ladder:
//
//   1. If a `<instructions>` user message exists, anchor = its LATEST index.
//   2. Else, anchor = first user message index.
//
// No tool-skip dance: under this rule the anchor is always a user-role
// message, never an assistant-with-tool_calls, so its tool results can't
// orphan in the drain range.

use crate::session::chat::session::ChatSession;
use anyhow::Result;

/// User-role messages wrap the instructions file in this tag at injection
/// time (see `prompt_setup.rs`). Detect by literal prefix on trimmed content.
const INSTRUCTIONS_TAG_OPEN: &str = "<instructions>";

/// Find the compression range deterministically from message structure.
///
/// Returns `(anchor_idx, end_idx)` where:
/// - `anchor_idx` is KEPT (compression drains `anchor_idx+1..=end_idx`)
/// - `end_idx = messages.len() - 1`
///
/// Returns `(0, 0)` when there is nothing meaningful to compress (no anchor,
/// too few conversational messages, or anchor already at the tail).
pub(super) fn find_compression_range(
	messages: &[crate::session::Message],
	force: bool,
) -> Result<(usize, usize)> {
	// Anchor: latest <instructions> user message, else first user message.
	let anchor = messages
		.iter()
		.enumerate()
		.rev()
		.find(|(_, m)| {
			m.role == "user" && m.content.trim_start().starts_with(INSTRUCTIONS_TAG_OPEN)
		})
		.map(|(i, _)| i)
		.or_else(|| messages.iter().position(|m| m.role == "user"));

	let start_idx = match anchor {
		Some(i) => i,
		None => return Ok((0, 0)),
	};

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
