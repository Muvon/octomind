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

//! Tool result deduplication.
//!
//! Tracks `(tool_name, content)` pairs seen within a session and replaces
//! exact-duplicate tool results with a small placeholder so the model does
//! not re-pay tokens for identical content.
//!
//! The dedup state is keyed by session id (so concurrent sessions stay
//! isolated) and falls back to a `_global_` bucket when there is no session
//! context (CLI/test paths).
//!
//! Hashing uses the standard library's default hasher — collisions are
//! astronomically unlikely for the size of typical sessions, and we are not
//! relying on cryptographic strength.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::{OnceLock, RwLock};

type SessionSet = HashSet<u64>;
type GlobalMap = HashMap<String, SessionSet>;

static DEDUP_STATE: OnceLock<RwLock<GlobalMap>> = OnceLock::new();

fn state() -> &'static RwLock<GlobalMap> {
	DEDUP_STATE.get_or_init(|| RwLock::new(HashMap::new()))
}

fn session_key() -> String {
	crate::session::context::current_session_id().unwrap_or_else(|| "_global_".to_string())
}

fn content_hash(tool_name: &str, content: &str) -> u64 {
	let mut h = std::collections::hash_map::DefaultHasher::new();
	tool_name.hash(&mut h);
	0u8.hash(&mut h); // separator so "ab"+"cd" != "abc"+"d"
	content.hash(&mut h);
	h.finish()
}

/// Has this exact `(tool_name, content)` already been recorded in the
/// current session? `true` means the caller should swap the body for
/// `placeholder()`; `false` means it is the first occurrence and the
/// caller should call `record()` after adding it.
pub fn is_duplicate(tool_name: &str, content: &str) -> bool {
	let key = content_hash(tool_name, content);
	let sk = session_key();
	state()
		.read()
		.unwrap()
		.get(&sk)
		.map(|s| s.contains(&key))
		.unwrap_or(false)
}

/// Mark this `(tool_name, content)` as seen so future identical results in
/// this session are deduplicated.
pub fn record(tool_name: &str, content: &str) {
	let key = content_hash(tool_name, content);
	let sk = session_key();
	state().write().unwrap().entry(sk).or_default().insert(key);
}

/// Stable replacement string for a deduplicated tool result. Kept short so
/// we maximize the token saving.
pub fn placeholder(tool_name: &str) -> String {
	format!(
		"[duplicate result for `{tool_name}` — identical content already in this session, body elided]"
	)
}

/// Drop the dedup state for one session (called on session reset/end).
pub fn clear_session(session_id: &str) {
	state().write().unwrap().remove(session_id);
}

/// Number of distinct tool results recorded in the given session (testing/observability).
#[cfg(test)]
fn session_size(session_id: &str) -> usize {
	state()
		.read()
		.unwrap()
		.get(session_id)
		.map(|s| s.len())
		.unwrap_or(0)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn separator_prevents_concatenation_collision() {
		// "ab" + "cd" must hash differently than "abc" + "d".
		let h1 = content_hash("ab", "cd");
		let h2 = content_hash("abc", "d");
		assert_ne!(h1, h2);
	}

	#[test]
	fn placeholder_includes_tool_name() {
		let s = placeholder("view");
		assert!(s.contains("view"));
		assert!(s.contains("duplicate"));
	}

	#[test]
	fn record_then_is_duplicate_via_global_bucket() {
		// In tests there is no session context, so session_key() returns
		// "_global_". Use a unique tool name per test run so we don't collide
		// with other tests sharing the same bucket.
		let tool = "test_view_42";
		let sid = "_global_".to_string();
		assert!(!is_duplicate(tool, "hello"));
		record(tool, "hello");
		assert!(is_duplicate(tool, "hello"));
		assert!(!is_duplicate(tool, "different"));
		assert!(!is_duplicate("shell_test_42", "hello"));
		// Cleanup so re-runs of the test do not see stale state.
		clear_session(&sid);
	}

	#[test]
	fn clear_session_removes_unrelated_only() {
		// clear_session should be a no-op for ids that have no state.
		clear_session("nonexistent-session-id");
		assert_eq!(session_size("nonexistent-session-id"), 0);
	}
}
