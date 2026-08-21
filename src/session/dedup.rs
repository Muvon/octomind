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
//! Tracks `(tool_name, args, content)` triples seen within a session and
//! replaces exact-duplicate tool results with a small placeholder so the model
//! does not re-pay tokens for identical content.
//!
//! `args` (the serialized call parameters) is part of the key so that two
//! genuinely different requests are never collapsed just because the tool
//! returned the same bytes — e.g. a `view` that ignores its `start`/`end`
//! window and returns the whole file would otherwise make every read of that
//! file look like a duplicate of the first.
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

/// Sentinel embedded in every dedup placeholder (see [`placeholder`]). This is
/// the model-facing signal that the body was elided because it already has the
/// output — the inline feedback that makes a separate detector unnecessary.
pub const DEDUP_NOTICE_TAG: &str = "duplicate tool call";

static DEDUP_STATE: OnceLock<RwLock<GlobalMap>> = OnceLock::new();

fn state() -> &'static RwLock<GlobalMap> {
	DEDUP_STATE.get_or_init(|| RwLock::new(HashMap::new()))
}

fn session_key() -> String {
	crate::session::context::current_session_id().unwrap_or_else(|| "_global_".to_string())
}

fn content_hash(tool_name: &str, args: &str, content: &str) -> u64 {
	let mut h = std::collections::hash_map::DefaultHasher::new();
	tool_name.hash(&mut h);
	0u8.hash(&mut h); // separator so "ab"+"cd" != "abc"+"d"
	args.hash(&mut h);
	0u8.hash(&mut h);
	content.hash(&mut h);
	h.finish()
}

/// Has this exact `(tool_name, args, content)` triple already been recorded in
/// the current session? `true` means the caller should swap the body for
/// `placeholder()`; `false` means it is the first occurrence and the caller
/// should call `record()` after adding it. `args` is the serialized call
/// parameters, so a different request (a new range/path) never collides even
/// when the tool returns the same bytes, and the same request whose output has
/// CHANGED (e.g. re-reading a file after editing it) is not falsely elided.
pub fn is_duplicate(tool_name: &str, args: &str, content: &str) -> bool {
	if content.len() < MIN_DEDUP_CONTENT_LEN {
		return false;
	}
	let key = content_hash(tool_name, args, content);
	let sk = session_key();
	state()
		.read()
		.unwrap()
		.get(&sk)
		.map(|s| s.contains(&key))
		.unwrap_or(false)
}

/// Mark this `(tool_name, args, content)` triple as seen so a future identical
/// call (same tool, same args, same output) is deduplicated. Content below
/// `MIN_DEDUP_CONTENT_LEN` is never recorded — it is always kept verbatim.
pub fn record(tool_name: &str, args: &str, content: &str) {
	if content.len() < MIN_DEDUP_CONTENT_LEN {
		return;
	}
	let key = content_hash(tool_name, args, content);
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
			"[duplicate tool call — `{tool_name}`: identical args returned the same truncated output already in your context — re-running yields no more. To reach the cut-off part, {hint}.]",
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
	let fingerprint = if first == last {
		format!("it begins: {first}")
	} else {
		format!("it begins: {first} — and ends: {last}")
	};
	format!(
		"[duplicate tool call — `{tool_name}`: identical args returned the same output you already have ({fingerprint}). Reuse that earlier result; only call again with different args or a different tool if you need something it does not contain.]"
	)
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
		// A field boundary must not be ambiguous: shifting a char across any
		// boundary (tool_name | args | content) must change the hash.
		assert_ne!(content_hash("ab", "x", "cd"), content_hash("abc", "x", "d"));
		assert_ne!(content_hash("t", "ab", "cd"), content_hash("t", "abc", "d"));
		assert_ne!(content_hash("t", "a", "bc"), content_hash("t", "ab", "c"));
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
		// A placeholder that doesn't say it's a duplicate reads as a tool failure:
		// the model re-runs the call instead of using the output it already holds.
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
	fn placeholder_truncated_repeat_redirects_without_echoing_body() {
		// A truncated repeat must NOT echo the body; it must tell the model the
		// re-run yields no more and how to reach the cut-off part instead.
		let s = placeholder("shell", "huge output that was truncated\n", true);
		assert!(s.contains("shell"));
		assert!(s.contains("truncated"));
		assert!(s.contains("yields no more")); // positive-forward deterrent
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
		let args = "{\"path\":\"a.rs\"}";
		let content = "hello\n".repeat(100); // above MIN_DEDUP_CONTENT_LEN
		let other = "different\n".repeat(100);
		assert!(!is_duplicate(tool, args, &content));
		record(tool, args, &content);
		assert!(is_duplicate(tool, args, &content));
		assert!(!is_duplicate(tool, args, &other));
		assert!(!is_duplicate("shell_test_42", args, &content));
		// Cleanup so re-runs of the test do not see stale state.
		clear_session(&sid);
	}

	#[test]
	fn different_args_same_content_not_deduplicated() {
		// The reported bug: two views of the same file at different ranges
		// return identical bytes; they must NOT collide.
		let tool = "test_view_ranges";
		let content = "x\n".repeat(300); // above MIN_DEDUP_CONTENT_LEN
		record(tool, "{\"start\":1,\"end\":100}", &content);
		assert!(is_duplicate(tool, "{\"start\":1,\"end\":100}", &content));
		assert!(!is_duplicate(tool, "{\"start\":100,\"end\":200}", &content));
		clear_session("_global_");
	}

	#[test]
	fn same_args_changed_content_not_deduplicated() {
		// Re-reading a file after editing it: same args, different bytes must
		// reach the model — no stale elision.
		let tool = "test_view_edited";
		let args = "{\"path\":\"a.rs\"}";
		let before = "old\n".repeat(300);
		let after = "new\n".repeat(300);
		record(tool, args, &before);
		assert!(is_duplicate(tool, args, &before));
		assert!(!is_duplicate(tool, args, &after));
		clear_session("_global_");
	}

	#[test]
	fn short_content_is_never_deduplicated() {
		// Verdict-style outputs ("[OK] No errors") must always reach the
		// model verbatim — eliding them causes re-verification loops.
		let tool = "test_shell_short";
		let content = "[OK] No errors";
		assert!(content.len() < MIN_DEDUP_CONTENT_LEN);
		record(tool, "{}", content);
		assert!(!is_duplicate(tool, "{}", content));
		clear_session("_global_");
	}

	#[test]
	fn clear_session_removes_unrelated_only() {
		// clear_session should be a no-op for ids that have no state.
		clear_session("nonexistent-session-id");
		assert_eq!(session_size("nonexistent-session-id"), 0);
	}
}
