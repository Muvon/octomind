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
//! Errors are NEVER deduped (the caller skips them): identical error text
//! often comes from independent failures the model must see each time, and
//! recording errors could poison the cache for a later successful call whose
//! body happens to match. A *successful* duplicate, by contrast, is surfaced
//! by the caller as an error result carrying [`placeholder`] — so the user
//! (terminal/UI) and the model both see the elision instead of a silent
//! success (see `tool_execution::execute_tools_in_context`).
//!
//! Hashing uses the standard library's default hasher — collisions are
//! astronomically unlikely for the size of typical sessions, and we are not
//! relying on cryptographic strength.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::{OnceLock, RwLock};

type SessionSet = HashSet<u64>;
type GlobalMap = HashMap<String, SessionSet>;

/// Results shorter than this are never deduplicated. Below it the
/// placeholder saves almost nothing, and short outputs are typically
/// verdicts ("[OK] No errors") the model must see verbatim each time it
/// re-runs a check — eliding them turns re-verification into a retry loop
/// (the model never sees the confirmation, so it keeps re-running the
/// command with variations).
const MIN_DEDUP_CONTENT_LEN: usize = 500;

/// Max chars of the original's first/last line quoted in the placeholder.
const SNIPPET_CHARS: usize = 120;

/// Sentinel embedded in every dedup placeholder (see [`placeholder`]). The
/// supervisor's detector keys its consecutive-dedup streak on this substring,
/// tool-agnostically — mirroring how truncation detection keys on its own tag.
pub const DEDUP_NOTICE_TAG: &str = "duplicate tool call";

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
	if content.len() < MIN_DEDUP_CONTENT_LEN {
		return false;
	}
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
/// this session are deduplicated. Content below `MIN_DEDUP_CONTENT_LEN` is
/// never recorded — it is always kept verbatim.
pub fn record(tool_name: &str, content: &str) {
	if content.len() < MIN_DEDUP_CONTENT_LEN {
		return;
	}
	let key = content_hash(tool_name, content);
	let sk = session_key();
	state().write().unwrap().entry(sk).or_default().insert(key);
}

/// First `SNIPPET_CHARS` chars of a line, char-boundary safe.
fn snippet(line: &str) -> String {
	let line = line.trim();
	if line.chars().count() > SNIPPET_CHARS {
		let cut: String = line.chars().take(SNIPPET_CHARS).collect();
		format!("{cut}…")
	} else {
		line.to_string()
	}
}

/// Replacement string for a deduplicated tool result. Quotes the original's
/// first and last non-empty lines so the model can tell WHICH earlier output
/// this duplicates — and see its verdict (test/lint summaries end with the
/// result line) — without re-paying for the full body.
pub fn placeholder(tool_name: &str, content: &str, was_truncated: bool) -> String {
	// A re-run of a call whose output was truncated is the classic retry loop:
	// the model wants the part it could not see. Re-sending the (already
	// truncated) body — or the neutral "body elided" placeholder — starves it
	// and pushes it to tweak arguments and try again, which evades the
	// supervisor's identical-result loop detector. Give a strong stop+narrow
	// directive instead.
	if was_truncated {
		return format!(
			"[ERROR: duplicate tool call — `{tool_name}` already returned this TRUNCATED output earlier in this session. Re-running with the same arguments yields the same truncated result, not more. Do not repeat it; to get the rest, {hint}.]",
			hint = crate::utils::truncation::truncation_hint(tool_name),
		);
	}
	let first = content
		.lines()
		.find(|l| !l.trim().is_empty())
		.map(snippet)
		.unwrap_or_default();
	let last = content
		.lines()
		.rev()
		.find(|l| !l.trim().is_empty())
		.map(snippet)
		.unwrap_or_default();
	if first == last {
		format!(
			"[ERROR: duplicate tool call — `{tool_name}` already returned this exact output earlier in this session, so the body was elided. Do not re-run with the same arguments; reuse the earlier result. It begins: {first}]"
		)
	} else {
		format!(
			"[ERROR: duplicate tool call — `{tool_name}` already returned this exact output earlier in this session, so the body was elided. Do not re-run with the same arguments; reuse the earlier result. It begins: {first} — and ends: {last}]"
		)
	}
}

/// Drop the dedup state for one session (called on session reset/end).
pub fn clear_session(session_id: &str) {
	state().write().unwrap().remove(session_id);
}

/// Drop the dedup state for the current session (or the CLI/test bucket
/// when there is no session context). Called from every compaction path
/// after compaction succeeds: once messages have been removed, the
/// originals our placeholders point at no longer exist, so future
/// duplicates of the same content must be kept verbatim again.
pub fn clear_current_session() {
	clear_session(&session_key());
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
	fn placeholder_includes_tool_name_and_snippets() {
		let s = placeholder("view", "first line\nmiddle\n[OK] No errors\n", false);
		assert!(s.contains("view"));
		assert!(s.contains("duplicate"));
		assert!(s.contains("first line"));
		assert!(s.contains("[OK] No errors"));
	}

	#[test]
	fn every_placeholder_variant_carries_the_sentinel() {
		// The supervisor's dedup detector keys on DEDUP_NOTICE_TAG; if any
		// placeholder variant stops containing it, dedup steering goes silent.
		let two_line = placeholder("view", "first\nlast\n", false);
		let one_line = placeholder("view", "only\n", false);
		let truncated = placeholder("shell", "huge\n", true);
		for p in [&two_line, &one_line, &truncated] {
			assert!(p.contains(DEDUP_NOTICE_TAG), "missing sentinel in: {p}");
		}
	}

	#[test]
	fn placeholder_single_line_quotes_it_once() {
		let s = placeholder("view", "only line\n", false);
		assert_eq!(s.matches("only line").count(), 1);
	}

	#[test]
	fn placeholder_truncated_repeat_is_a_stop_directive() {
		// A truncated repeat must NOT echo the body; it must tell the model to
		// stop re-running and how to narrow instead.
		let s = placeholder("shell", "huge output that was truncated\n", true);
		assert!(s.contains("shell"));
		assert!(s.contains("TRUNCATED"));
		assert!(s.contains("Do not repeat"));
		assert!(s.contains("grep")); // shell-specific narrowing hint
		assert!(!s.contains("huge output")); // body is not echoed
	}

	#[test]
	fn record_then_is_duplicate_via_global_bucket() {
		// In tests there is no session context, so session_key() returns
		// "_global_". Use a unique tool name per test run so we don't collide
		// with other tests sharing the same bucket.
		let tool = "test_view_42";
		let sid = "_global_".to_string();
		let content = "hello\n".repeat(100); // above MIN_DEDUP_CONTENT_LEN
		let other = "different\n".repeat(100);
		assert!(!is_duplicate(tool, &content));
		record(tool, &content);
		assert!(is_duplicate(tool, &content));
		assert!(!is_duplicate(tool, &other));
		assert!(!is_duplicate("shell_test_42", &content));
		// Cleanup so re-runs of the test do not see stale state.
		clear_session(&sid);
	}

	#[test]
	fn short_content_is_never_deduplicated() {
		// Verdict-style outputs ("[OK] No errors") must always reach the
		// model verbatim — eliding them causes re-verification loops.
		let tool = "test_shell_short";
		let content = "[OK] No errors";
		assert!(content.len() < MIN_DEDUP_CONTENT_LEN);
		record(tool, content);
		assert!(!is_duplicate(tool, content));
		clear_session("_global_");
	}

	#[test]
	fn clear_session_removes_unrelated_only() {
		// clear_session should be a no-op for ids that have no state.
		clear_session("nonexistent-session-id");
		assert_eq!(session_size("nonexistent-session-id"), 0);
	}
}
