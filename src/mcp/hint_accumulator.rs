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

// Hint accumulator — collects tool-misuse hints during a tool execution round.
//
// Tools call push_hint() instead of appending ⚠️ text to their result content.
// After all parallel tool results are collected, the session layer calls drain_hints()
// and injects a single user-role message so the AI sees the guidance without
// polluting individual tool result strings.
//
// Session-scoped: each session has its own hints accumulator, keyed by session ID.
// The push/drain/has functions check current_session_id() to route to the correct
// session's accumulator. Falls back to global CLI accumulator when not in a session.

use std::sync::Mutex;

static HINTS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Push a hint into the accumulator. Deduplication is applied at drain time.
/// Routes to session-scoped accumulator if in a session, otherwise global CLI.
pub fn push_hint(hint: &str) {
	if let Some(session_id) = crate::session::context::current_session_id() {
		crate::session::context::push_hint_for_session(&session_id, hint.to_string());
	} else if let Ok(mut hints) = HINTS.lock() {
		hints.push(hint.to_string());
	}
}

/// Drain all accumulated hints for the current context, returning deduplicated list.
/// Routes to session-scoped accumulator if in a session, otherwise global CLI.
/// Clears the accumulator — ready for the next tool execution round.
pub fn drain_hints() -> Vec<String> {
	if let Some(session_id) = crate::session::context::current_session_id() {
		return crate::session::context::drain_hints_for_session(&session_id);
	}
	let Ok(mut hints) = HINTS.lock() else {
		return Vec::new();
	};
	let mut seen = std::collections::HashSet::new();
	let deduped: Vec<String> = hints.drain(..).filter(|h| seen.insert(h.clone())).collect();
	deduped
}

/// Returns true if there are any pending hints (without draining).
/// Routes to session-scoped accumulator if in a session, otherwise global CLI.
pub fn has_hints() -> bool {
	if let Some(session_id) = crate::session::context::current_session_id() {
		return crate::session::context::has_hints_for_session(&session_id);
	}
	HINTS.lock().is_ok_and(|h| !h.is_empty())
}

#[cfg(test)]
#[path = "hint_accumulator_tests.rs"]
mod tests;
