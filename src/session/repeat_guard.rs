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

//! Octomind's own count of tool rounds that repeat a round already made.
//!
//! The hard stop on repeated rounds (`max_identical_rounds`) is the agent
//! loop's own: it counts rounds here, whatever plugins are configured.

use crate::mcp::tool_shape::param_paths;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashSet, VecDeque};
use std::hash::{Hash, Hasher};

/// How many result fingerprints and round fingerprints are remembered.
const REMEMBERED_CAP: usize = 128;

/// What the guard reads from one finished tool call.
pub struct RoundCall<'a> {
	pub tool: &'a str,
	pub parameters: &'a serde_json::Value,
	pub result: &'a str,
	pub is_error: bool,
	pub is_mutation: bool,
	/// The tool runs a free-form command, so its "mutation" is only a guess.
	pub free_form: bool,
}

#[derive(Debug, Default)]
pub struct RepeatGuard {
	/// Tool rounds in a row that repeated a round already made since the last
	/// new result, the first of them included.
	identical_rounds: usize,
	/// Round fingerprints since the last round that brought something new.
	rounds_since_progress: VecDeque<u64>,
	/// Fingerprint of the latest round: what a tree move restarts the count from.
	last_round: Option<u64>,
	/// Fingerprints of the results seen recently, to tell new results from old.
	seen: HashSet<u64>,
	seen_order: VecDeque<u64>,
}

impl RepeatGuard {
	/// Record one finished round of parallel tool calls and return how many
	/// rounds in a row repeated an earlier one. A parallel batch is one model
	/// decision, so it counts once however many calls it holds.
	pub fn note_round(&mut self, calls: &[RoundCall<'_>]) -> usize {
		let mut call_hashes = Vec::with_capacity(calls.len());
		let mut round_novel = false;
		for call in calls {
			let (call_hash, novel) = self.note_call(call);
			call_hashes.push(call_hash);
			round_novel |= novel;
		}

		// The order of parallel calls carries no meaning.
		call_hashes.sort_unstable();
		let mut hasher = DefaultHasher::new();
		call_hashes.hash(&mut hasher);
		let round_hash = hasher.finish();

		if round_novel {
			self.rounds_since_progress.clear();
		}
		let repeated = !round_novel && self.rounds_since_progress.contains(&round_hash);
		self.identical_rounds = if repeated { self.identical_rounds + 1 } else { 1 };
		if !repeated {
			self.rounds_since_progress.push_back(round_hash);
			if self.rounds_since_progress.len() > REMEMBERED_CAP {
				self.rounds_since_progress.pop_front();
			}
		}
		self.last_round = Some(round_hash);
		self.identical_rounds
	}

	/// The round just recorded changed the working tree: real work its results
	/// may not show. Restarts the count, because the stop must never end a turn
	/// that is making changes.
	pub fn note_tree_moved(&mut self) {
		self.rounds_since_progress.clear();
		self.rounds_since_progress.extend(self.last_round);
		self.identical_rounds = 1;
	}

	/// A new user turn starts. What was seen stays: results do not become new
	/// because the user spoke.
	pub fn reset_streak(&mut self) {
		self.identical_rounds = 0;
		self.rounds_since_progress.clear();
		self.last_round = None;
	}

	pub fn identical_rounds(&self) -> usize {
		self.identical_rounds
	}

	/// Returns the call's fingerprint for the round and whether it brought
	/// something new.
	fn note_call(&mut self, call: &RoundCall<'_>) -> (u64, bool) {
		let result = canonical_loop_result(call.tool, call.result);

		// A call repeats when tool, arguments, result and error flag all match.
		let mut hasher = DefaultHasher::new();
		call.tool.hash(&mut hasher);
		call.parameters.to_string().hash(&mut hasher);
		result.hash(&mut hasher);
		call.is_error.hash(&mut hasher);
		let call_hash = hasher.finish();

		// A result is new when the target it addresses (the path-like
		// arguments) returned something not seen before. Rewording the other
		// arguments does not make it new.
		let mut hasher = DefaultHasher::new();
		call.tool.hash(&mut hasher);
		param_paths(call.parameters).hash(&mut hasher);
		result.hash(&mut hasher);
		call.is_error.hash(&mut hasher);
		let result_hash = hasher.finish();
		let fresh = self.seen.insert(result_hash);
		if fresh {
			self.seen_order.push_back(result_hash);
			if self.seen_order.len() > REMEMBERED_CAP {
				if let Some(oldest) = self.seen_order.pop_front() {
					self.seen.remove(&oldest);
				}
			}
		}

		// A successful write is progress even when its output repeats. A free-form
		// command only looks like a write, so it earns no such credit.
		let novel = fresh || (!call.is_error && call.is_mutation && !call.free_form);
		(call_hash, novel)
	}
}

/// A tap launch answers with a fresh run id every time; masking it lets a
/// relaunch of the same delegation count as the same call.
fn canonical_loop_result<'a>(tool: &str, result: &'a str) -> std::borrow::Cow<'a, str> {
	if tool != "tap" {
		return std::borrow::Cow::Borrowed(result);
	}
	let Ok(mut value) = serde_json::from_str::<serde_json::Value>(result) else {
		return std::borrow::Cow::Borrowed(result);
	};
	match value.get_mut("id") {
		Some(id) if id.as_str().is_some_and(|id| id.starts_with("tap-")) => {
			*id = serde_json::Value::String("tap-[id]".to_string());
			std::borrow::Cow::Owned(value.to_string())
		}
		_ => std::borrow::Cow::Borrowed(result),
	}
}

/// Characters of each repeated call and its result quoted in the stop notice:
/// enough to recognise the loop, short enough for a parent agent's handback.
const LOOP_STOP_PREVIEW_CHARS: usize = 300;

/// The turn's final answer once `max_identical_rounds` is reached.
/// Deterministic and self-contained: a parent agent receiving it as the
/// handback, or the user reading it later, sees what repeated without the log.
pub fn loop_stop_note(
	rounds: usize,
	calls: &[crate::mcp::McpToolCall],
	results: &[crate::mcp::McpToolResult],
) -> String {
	let mut note = format!(
		"Stopped by the supervisor: the last {rounds} tool rounds repeated the same calls and got the same results, so this turn ended instead of spending more requests on it."
	);
	for call in calls {
		let result = results
			.iter()
			.find(|r| r.tool_id == call.tool_id)
			.map(|r| r.extract_content())
			.unwrap_or_default();
		note.push_str(&format!(
			"\nRepeated: `{}` {}\nResult: {}",
			call.tool_name,
			crate::utils::text::truncate(&call.parameters.to_string(), LOOP_STOP_PREVIEW_CHARS),
			crate::utils::text::truncate(&result, LOOP_STOP_PREVIEW_CHARS)
		));
	}
	note.push_str("\nRepeating it will not change the result; continuing needs a different approach.");
	note
}

#[cfg(test)]
#[path = "repeat_guard_tests.rs"]
mod tests;
