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

#[test]
fn content_hash_is_deterministic_and_sensitive_to_tool_name() {
	assert_eq!(
		content_hash("view", "{}", "body"),
		content_hash("view", "{}", "body")
	);
	assert_ne!(
		content_hash("view", "{}", "body"),
		content_hash("shell", "{}", "body")
	);
}

#[test]
fn min_length_boundary_is_exactly_500_chars() {
	// 499 chars: below the gate, never recorded → never a duplicate.
	let short = "a".repeat(MIN_DEDUP_CONTENT_LEN - 1);
	record("test_gate_short", "{}", &short);
	assert!(!is_duplicate("test_gate_short", "{}", &short));

	// Exactly 500 chars: at the gate, eligible for dedup.
	let exact = "b".repeat(MIN_DEDUP_CONTENT_LEN);
	assert!(!is_duplicate("test_gate_exact", "{}", &exact));
	record("test_gate_exact", "{}", &exact);
	assert!(is_duplicate("test_gate_exact", "{}", &exact));
	clear_session("_global_");
}

#[test]
fn snippet_trims_surrounding_whitespace() {
	assert_eq!(snippet("  hello world  "), "hello world");
	assert_eq!(snippet("\tline\n"), "line");
}

#[test]
fn snippet_keeps_short_lines_verbatim() {
	let exact: String = "x".repeat(SNIPPET_CHARS);
	assert_eq!(
		snippet(&exact),
		exact,
		"line at exactly SNIPPET_CHARS is not cut"
	);
	assert_eq!(snippet("short"), "short");
}

#[test]
fn snippet_truncates_long_lines_at_char_boundary() {
	let line: String = "y".repeat(SNIPPET_CHARS + 50);
	let cut = snippet(&line);
	assert_eq!(
		cut.chars().count(),
		SNIPPET_CHARS + 1,
		"120 chars + ellipsis"
	);
	assert!(cut.ends_with('…'));
	assert!(cut.starts_with(&"y".repeat(SNIPPET_CHARS)));
}

#[test]
fn snippet_is_multibyte_safe() {
	// 130 three-byte chars: char-based truncation must not split a char.
	let line: String = "日".repeat(SNIPPET_CHARS + 10);
	let cut = snippet(&line);
	assert_eq!(cut.chars().count(), SNIPPET_CHARS + 1);
	assert!(cut.chars().all(|c| c == '日' || c == '…'));
}

#[test]
fn snippet_of_empty_line_is_empty() {
	assert_eq!(snippet(""), "");
	assert_eq!(snippet("   "), "");
}

#[test]
fn placeholder_skips_blank_lines_when_fingerprinting() {
	let content = "\n\n   \nfirst real line\nmiddle\n   \nlast real line\n\n";
	let s = placeholder("view", content, false);
	assert!(s.contains("first real line"));
	assert!(s.contains("last real line"));
	assert!(
		s.contains("and ends"),
		"distinct first/last must both be quoted"
	);
}

#[test]
fn placeholder_whitespace_only_content_quotes_a_single_empty_fingerprint() {
	let s = placeholder("view", "\n   \n\n", false);
	assert!(s.contains(DEDUP_NOTICE_TAG));
	assert!(s.contains("it begins:"));
	assert!(
		!s.contains("and ends"),
		"first == last must collapse to one quote"
	);
}

#[test]
fn placeholder_snippets_overlong_fingerprint_lines() {
	let long_line = "a".repeat(SNIPPET_CHARS + 100);
	let content = format!("{long_line}\ntail\n");
	let s = placeholder("view", &content, false);
	assert!(s.contains(&"a".repeat(SNIPPET_CHARS)));
	assert!(s.contains('…'));
	assert!(
		!s.contains(&"a".repeat(SNIPPET_CHARS + 1)),
		"chars past the cut must not leak"
	);
}

#[test]
fn clear_current_session_empties_the_fallback_bucket() {
	// Without a session context the state lives in "_global_";
	// clear_current_session must drop exactly that bucket.
	let tool = "test_clear_current";
	let content = "z\n".repeat(300);
	record(tool, "{}", &content);
	assert!(is_duplicate(tool, "{}", &content));
	clear_current_session();
	assert!(!is_duplicate(tool, "{}", &content));
}

#[test]
fn clearing_one_session_leaves_the_fallback_bucket_intact() {
	let tool = "test_isolation_other_session";
	let content = "w\n".repeat(300);
	record(tool, "{}", &content);
	assert!(is_duplicate(tool, "{}", &content));
	clear_session("some-other-session-id");
	assert!(
		is_duplicate(tool, "{}", &content),
		"unrelated session clear must not touch our state"
	);
	clear_session("_global_");
}
