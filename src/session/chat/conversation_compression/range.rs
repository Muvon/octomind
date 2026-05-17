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
// and price the chosen range in tokens. Kept apart from the AI decision logic
// in `mod.rs` because these are pure functions over `&[Message]` / `ChatSession`
// with no LLM call or config dependency beyond the session itself.

use crate::session::chat::session::ChatSession;
use anyhow::Result;

/// Find the compression range: anchor to last message (compress-all approach).
///
/// CRITICAL: Must not cut between assistant with tool_calls and its tool results
/// CRITICAL: Compression NEVER goes below first_prompt_idx (INCLUSIVE boundary)
pub(super) fn find_compression_range(
	messages: &[crate::session::Message],
	first_prompt_idx: Option<usize>,
	force: bool,
) -> Result<(usize, usize)> {
	// Find system message index
	let system_idx = messages
		.iter()
		.position(|m| m.role == "system")
		.unwrap_or(0);

	// Start boundary: try to move anchor BEFORE first_prompt_idx so the first user
	// message gets compressed. Without this, the first user message persists raw
	// across all compression cycles — even after the user moved on to new tasks.
	//
	// If instructions file exists at idx-1 (user role), use it as anchor.
	// The first user message then falls into drain range (start_idx+1..=end_idx).
	// Keep old behavior for tool-loops (single user message — it's still active).
	//
	// If not set (e.g. resumed sessions), detect bootstrap messages and skip past them.
	// Bootstrap pattern: system[0] → assistant(welcome)[1] → optional user(instructions)[2]
	// We must NEVER compress the system prompt, welcome message, or instructions file.
	let mut start_idx = match first_prompt_idx {
		Some(idx) => {
			// Check if user sent additional messages after the first prompt.
			// If yes, the first prompt is no longer active and should be compressed.
			let has_subsequent_user = messages.iter().skip(idx + 1).any(|m| m.role == "user");

			if has_subsequent_user
				&& idx > 0 && messages.get(idx - 1).is_some_and(|m| m.role == "user")
			{
				// Instructions file at idx-1 becomes anchor.
				// First user message at idx is now in drain range.
				idx - 1
			} else {
				idx
			}
		}
		None => {
			let mut idx = system_idx + 1;
			// Skip welcome message (assistant immediately after system, WITHOUT tool_calls).
			// A welcome is a simple greeting — if it has tool_calls, it's a working response.
			let has_welcome = idx < messages.len()
				&& messages[idx].role == "assistant"
				&& messages[idx].tool_calls.is_none();
			if has_welcome {
				idx += 1;
			}
			// Skip instructions file ONLY if welcome was present.
			// Bootstrap pattern: system → assistant(welcome) → user(instructions).
			// Without a welcome message, the first user message is a real prompt, not instructions.
			if has_welcome
				&& idx < messages.len()
				&& messages[idx].role == "user"
				&& (idx + 1 >= messages.len() || messages[idx + 1].role == "assistant")
			{
				idx += 1;
			}
			idx
		}
	};

	// CRITICAL: If the anchor message has tool_calls, its tool results immediately follow it.
	// remove_messages_in_range drains start_idx+1..=end_idx — if tool results are in that
	// range they get removed, leaving orphaned tool_use blocks without tool_result.
	// The API then rejects the sequence with "tool_use ids were found without tool_result".
	// Fix: advance start_idx past all tool results that belong to the anchor's tool_calls.
	if let Some(anchor) = messages.get(start_idx) {
		if anchor.role == "assistant" && anchor.tool_calls.is_some() {
			// Skip past consecutive tool messages that follow the anchor
			let mut next = start_idx + 1;
			while next < messages.len() && messages[next].role == "tool" {
				next += 1;
			}
			// next now points to the first non-tool message after the anchor's tool results.
			// That becomes the new anchor (the drain will start at new_start_idx+1).
			if next > start_idx + 1 && next < messages.len() {
				start_idx = next;
			}
		}
	}

	// COMPRESS-ALL APPROACH: Compress everything from start_idx+1 to the last message.
	// Recent user messages are extracted and re-injected after the summary by the caller.
	// This eliminates the old preserve_count / boundary-search complexity and ensures
	// no user messages persist as stale raw artifacts across compression cycles.
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

/// Calculate tokens in message range using accurate token counting
/// This now counts ALL message fields: content, tool_calls, thinking, images, etc.
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

	// Validate range
	if start_idx >= session.session.messages.len() {
		return Err(anyhow::anyhow!("Invalid start_index in range"));
	}

	if end_idx >= session.session.messages.len() {
		return Err(anyhow::anyhow!("Invalid end_index in range"));
	}

	// Count tokens in range [start_idx, end_idx] inclusive
	for i in start_idx..=end_idx {
		if let Some(message) = session.session.messages.get(i) {
			let tokens = crate::session::estimate_message_tokens(message) as u64;
			total_tokens += tokens;
		}
	}

	Ok(total_tokens)
}
