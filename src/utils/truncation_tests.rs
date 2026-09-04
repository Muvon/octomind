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
fn test_mcp_truncation_unlimited() {
	let content = "This is a test content";
	let (result, was_truncated) = truncate_mcp_response_global(content, 0, "view");
	assert_eq!(result, content);
	assert!(!was_truncated);
}

#[test]
fn test_mcp_truncation_under_limit() {
	let content = "Short content";
	let (result, was_truncated) = truncate_mcp_response_global(content, 1000, "view");
	assert_eq!(result, content);
	assert!(!was_truncated);
}

#[test]
fn test_mcp_truncation_over_limit() {
	let content =
		"This is a very long content that should be truncated when it exceeds the token limit. "
			.repeat(100);
	let (result, was_truncated) = truncate_mcp_response_global(&content, 50, "shell");
	assert!(result.contains(TRUNCATION_NOTICE_TAG));
	// Notice carries the tool-specific hint (shell → grep/head/tail).
	assert!(result.contains("grep"));
	assert!(result.len() < content.len());
	assert!(was_truncated);
}

#[test]
fn test_mcp_truncation_is_idempotent() {
	// The notice is paid for out of the budget, so a truncated result fits the
	// cap it was truncated to — which is what makes a second pass return it
	// byte-for-byte (no double notice, no count corruption).
	let content = "x ".repeat(20_000);
	let (once, t1) = truncate_mcp_response_global(&content, 1000, "shell");
	assert!(t1);
	assert!(
		crate::session::estimate_tokens(&once) <= 1000,
		"a truncated result must fit the budget it was truncated to"
	);
	let (twice, t2) = truncate_mcp_response_global(&once, 1000, "shell");
	assert!(!t2);
	assert_eq!(once, twice);
}

/// A cap below the notice's own size cannot produce a result that fits it, so
/// the budget arithmetic alone cannot make a second pass a no-op. The tail
/// fallback must: double-truncating stacked a second notice on the first and
/// cut a dedup placeholder down to "[d".
#[test]
fn test_mcp_truncation_is_idempotent_under_a_cap_smaller_than_the_notice() {
	let content = "x ".repeat(20_000);
	let (once, t1) = truncate_mcp_response_global(&content, 50, "shell");
	assert!(t1);
	let (twice, t2) = truncate_mcp_response_global(&once, 50, "shell");
	assert!(!t2);
	assert_eq!(once, twice, "a second pass must not stack a second notice");
	assert_eq!(
		once.matches(TRUNCATION_NOTICE_TAG).count(),
		1,
		"exactly one notice"
	);
}

/// Reserving a fixed 400 tokens out of a 50-token budget left one token of
/// content — enough to turn our own dedup placeholder into "[d". The reserve
/// never takes more than half.
#[test]
fn test_mcp_truncation_keeps_content_under_a_small_cap() {
	let placeholder = "[duplicate tool call — `skill`: identical args returned the same truncated output already in your context — re-running yields no more. To reach the cut-off part, narrow the request: target a specific subset, add a filter, or ask for fewer items.]";
	assert!(
		crate::session::estimate_tokens(placeholder) > 20,
		"test premise: the payload must exceed the cap"
	);
	let (result, was_truncated) = truncate_mcp_response_global(placeholder, 20, "skill");
	assert!(was_truncated);
	assert!(
		result.contains("duplicate tool call"),
		"the cap must leave usable content, got: {result}"
	);
}

/// A payload that merely QUOTES a truncation notice — a session transcript, a
/// spill file, this repository — must still be capped. Asking the CONTENT
/// whether it had already been truncated let one `view` of a session file carry
/// 142k tokens past a 6k cap, into a live exchange compression may not drain.
#[test]
fn test_mcp_truncation_applies_to_content_that_quotes_a_notice() {
	let quoted = format!("{TRUNCATION_NOTICE_TAG}: showing only the first ~6000 of ~25245 tokens");
	let content = format!("{quoted}\n{}", "payload line\n".repeat(5000));
	let (result, was_truncated) = truncate_mcp_response_global(&content, 1000, "view");
	assert!(was_truncated, "a quoted notice must not switch the cap off");
	assert!(
		crate::session::estimate_tokens(&result) <= 1000,
		"the cap binds whatever the payload happens to contain"
	);
}

#[test]
fn test_floor_char_boundary_ascii_and_edges() {
	assert_eq!(floor_char_boundary("hello", 0), 0);
	assert_eq!(floor_char_boundary("hello", 3), 3);
	assert_eq!(floor_char_boundary("hello", 5), 5); // index == len
	assert_eq!(floor_char_boundary("hello", 100), 5); // index > len clamps to len
	assert_eq!(floor_char_boundary("", 0), 0);
	assert_eq!(floor_char_boundary("", 10), 0);
}

#[test]
fn test_floor_char_boundary_multibyte() {
	// "é" is 2 bytes: boundaries at 0, 1, 3, ...
	assert_eq!(floor_char_boundary("héllo", 2), 1);
	assert_eq!(floor_char_boundary("héllo", 3), 3);
	// "日" is 3 bytes: boundaries at 0, 3, 6, 9
	assert_eq!(floor_char_boundary("日本語", 4), 3);
	assert_eq!(floor_char_boundary("日本語", 6), 6);
	// "😀" is 4 bytes: boundaries at 0, 1, 5, 6
	assert_eq!(floor_char_boundary("a😀b", 3), 1);
	assert_eq!(floor_char_boundary("a😀b", 5), 5);
}

#[test]
fn test_floor_char_boundary_always_lands_on_boundary() {
	let s = "a日b😀c";
	for i in 0..=(s.len() + 2) {
		let b = floor_char_boundary(s, i);
		assert!(b <= i, "index {i}: floor {b} above input");
		assert!(s.is_char_boundary(b), "index {i}: floor {b} not a boundary");
	}
}

#[test]
fn test_format_content_no_range_numbers_all_lines() {
	let lines = ["alpha", "beta", "gamma"];
	assert_eq!(
		format_content_with_line_numbers(&lines, 1, None),
		"1: alpha\n2: beta\n3: gamma"
	);
	// start_line_number offsets every line
	assert_eq!(
		format_content_with_line_numbers(&lines, 100, None),
		"100: alpha\n101: beta\n102: gamma"
	);
	// Empty input yields empty output
	assert_eq!(format_content_with_line_numbers(&[], 1, None), "");
}

#[test]
fn test_format_content_range_full_and_clamped() {
	let owned: Vec<String> = (0..10).map(|i| format!("L{i}")).collect();
	let lines: Vec<&str> = owned.iter().map(|s| s.as_str()).collect();
	let all = "1: L0\n2: L1\n3: L2\n4: L3\n5: L4\n6: L5\n7: L6\n8: L7\n9: L8\n10: L9";

	// end = -1 means "to the end"
	assert_eq!(
		format_content_with_line_numbers(&lines, 1, Some((3, -1))),
		all
	);
	// end beyond the content clamps to the last line
	assert_eq!(
		format_content_with_line_numbers(&lines, 1, Some((2, 100))),
		all
	);
	// start = 0 is treated like line 1
	assert_eq!(
		format_content_with_line_numbers(&lines, 1, Some((0, 10))),
		all
	);
}

#[test]
fn test_format_content_elides_lines_before_range() {
	let owned: Vec<String> = (0..10).map(|i| format!("L{i}")).collect();
	let lines: Vec<&str> = owned.iter().map(|s| s.as_str()).collect();
	// Gap of 6 lines before the range: 2 head lines + marker, lines 3-6 hidden
	assert_eq!(
		format_content_with_line_numbers(&lines, 1, Some((7, 8))),
		"1: L0\n2: L1\n[...4 lines more]\n7: L6\n8: L7\n9: L8\n10: L9"
	);
}

#[test]
fn test_format_content_elides_lines_after_range() {
	let owned: Vec<String> = (0..10).map(|i| format!("L{i}")).collect();
	let lines: Vec<&str> = owned.iter().map(|s| s.as_str()).collect();
	// Range at the top: 3 shown, 5 hidden, last 2 shown
	assert_eq!(
		format_content_with_line_numbers(&lines, 1, Some((1, 3))),
		"1: L0\n2: L1\n3: L2\n[...5 lines more]\n9: L8\n10: L9"
	);
}

#[test]
fn test_format_content_elides_both_sides() {
	let owned: Vec<String> = (0..20).map(|i| format!("L{i}")).collect();
	let lines: Vec<&str> = owned.iter().map(|s| s.as_str()).collect();
	assert_eq!(
			format_content_with_line_numbers(&lines, 1, Some((10, 12))),
			"1: L0\n2: L1\n[...7 lines more]\n10: L9\n11: L10\n12: L11\n[...6 lines more]\n19: L18\n20: L19"
		);
}

#[test]
fn test_format_content_small_gaps_shown_inline() {
	let owned: Vec<String> = (0..10).map(|i| format!("L{i}")).collect();
	let lines: Vec<&str> = owned.iter().map(|s| s.as_str()).collect();
	// Gap of 4 before / 4 after: below the 5-line elision threshold, so every
	// line is shown and no "[...]" marker appears
	let result = format_content_with_line_numbers(&lines, 1, Some((5, 6)));
	assert_eq!(
		result,
		"1: L0\n2: L1\n3: L2\n4: L3\n5: L4\n6: L5\n7: L6\n8: L7\n9: L8\n10: L9"
	);
	assert!(!result.contains("[..."));
}

#[test]
fn test_format_content_invalid_ranges() {
	let lines = ["a", "b", "c"];
	// Start beyond the content
	assert_eq!(
		format_content_with_line_numbers(&lines, 1, Some((10, 20))),
		"Start line 10 exceeds content length (3 lines)"
	);
	// Any range into empty content exceeds its length
	assert_eq!(
		format_content_with_line_numbers(&[], 1, Some((1, 5))),
		"Start line 1 exceeds content length (0 lines)"
	);
	let five = ["a", "b", "c", "d", "e"];
	// Start after end
	assert_eq!(
		format_content_with_line_numbers(&five, 1, Some((4, 2))),
		"Start line 4 must be less than or equal to end line 2"
	);
}

#[test]
fn test_format_extracted_under_limit_shows_all() {
	let lines = ["alpha", "beta", "gamma"];
	assert_eq!(
		format_extracted_content_smart(&lines, 1, Some(5)),
		"1: alpha\n2: beta\n3: gamma"
	);
	// start_line offsets every line
	assert_eq!(
		format_extracted_content_smart(&lines, 100, Some(5)),
		"100: alpha\n101: beta\n102: gamma"
	);
	assert_eq!(format_extracted_content_smart(&[], 1, Some(5)), "");
}

#[test]
fn test_format_extracted_exact_limit_boundary() {
	let owned: Vec<String> = (0..5).map(|i| format!("L{i}")).collect();
	let lines: Vec<&str> = owned.iter().map(|s| s.as_str()).collect();
	// Exactly at the limit: everything shown, no elision marker
	assert_eq!(
		format_extracted_content_smart(&lines, 1, Some(5)),
		"1: L0\n2: L1\n3: L2\n4: L3\n5: L4"
	);
	// One line over: floor(2/3·4)=2 head lines, 1 marker line, 1 tail line
	assert_eq!(
		format_extracted_content_smart(&lines, 1, Some(4)),
		"1: L0\n2: L1\n[...2 lines more]\n5: L4"
	);
}

#[test]
fn test_format_extracted_defaults_to_fifty_lines() {
	let owned: Vec<String> = (0..51).map(|i| format!("line{i}")).collect();
	let lines: Vec<&str> = owned.iter().map(|s| s.as_str()).collect();
	// 50 lines fit the default limit exactly
	let shown = format_extracted_content_smart(&lines[..50], 1, None);
	assert!(!shown.contains("[..."));
	assert!(shown.contains("50: line49"));
	// 51 lines: 33 head + marker + 16 tail, lines 34-35 hidden
	let elided = format_extracted_content_smart(&lines, 1, None);
	assert!(elided.contains("[...2 lines more]"));
	assert!(elided.contains("1: line0"));
	assert!(elided.contains("51: line50"));
	assert!(!elided.contains("34: line33"));
}

#[test]
fn test_format_extracted_max_one_degenerates_to_marker_only() {
	let lines = ["a", "b"];
	// max = 1: floor(2/3·1)=0 head and 0 tail lines, leaving only the marker
	assert_eq!(
		format_extracted_content_smart(&lines, 1, Some(1)),
		"[...2 lines more]"
	);
}

#[test]
fn test_truncate_tool_output_small_content_untouched() {
	assert_eq!(truncate_tool_output_smart("", 5, 10), "");
	assert_eq!(truncate_tool_output_smart("hello", 10, 100), "hello");
	// Exact boundaries on both axes: at the limit, nothing is cut
	assert_eq!(truncate_tool_output_smart("abc", 1, 3), "abc");
	assert_eq!(truncate_tool_output_smart("a\nb", 2, 3), "a\nb");
}

#[test]
fn test_truncate_tool_output_by_lines() {
	let content = "l1\nl2\nl3\nl4\nl5";
	// Exactly at the line limit: untouched
	assert_eq!(truncate_tool_output_smart(content, 5, 1000), content);
	// One line over: first max_lines-1 lines + summary of the rest
	assert_eq!(
		truncate_tool_output_smart(content, 3, 1000),
		"l1\nl2\n... [3 more lines]"
	);
}

#[test]
fn test_truncate_tool_output_by_chars() {
	// Exactly at the char limit: untouched
	assert_eq!(truncate_tool_output_smart("abcde", 10, 5), "abcde");
	// One char over: keep max_chars-3 chars + "..."
	assert_eq!(truncate_tool_output_smart("abcdef", 10, 5), "ab...");
	// max_chars == 3 leaves room for the ellipsis only
	assert_eq!(truncate_tool_output_smart("abcdef", 10, 3), "...");
}

#[test]
fn test_truncate_tool_output_unicode_chars() {
	// Char-based truncation must cut at char boundaries, never mid-codepoint
	assert_eq!(truncate_tool_output_smart("日本語日本語", 10, 5), "日本...");
	// 3 chars fit the limit of 3 exactly
	assert_eq!(truncate_tool_output_smart("日本語", 10, 3), "日本語");
}

#[test]
fn test_truncate_tool_output_line_limit_wins_over_chars() {
	// Both limits exceeded: the line strategy applies first
	let content = "aaaa\nbbbb\ncccc\ndddd\neeee";
	assert_eq!(
		truncate_tool_output_smart(content, 3, 10),
		"aaaa\nbbbb\n... [3 more lines]"
	);
}

#[test]
fn test_truncation_hint_matches_each_tool_family() {
	// Reader tools share the line-range advice
	for tool in ["view", "text_editor", "read", "extract_lines"] {
		assert!(truncation_hint(tool).contains("line range"), "{tool}");
	}
	assert!(truncation_hint("view_signatures").contains("fewer files"));
	assert!(truncation_hint("shell").contains("grep"));
	for tool in ["list_files", "workdir"] {
		assert!(truncation_hint(tool).contains("subdirectory"), "{tool}");
	}
	assert!(truncation_hint("ast_grep").contains("pattern"));
	// Substring match catches search-like tools whatever they are called
	for tool in ["semantic_search", "find_references", "graphrag"] {
		assert!(
			truncation_hint(tool).contains("more specific query"),
			"{tool}"
		);
	}
	// Everything else gets the generic advice
	assert!(truncation_hint("plan").contains("narrow the request"));
}

#[test]
fn test_truncation_notice_tag_value_is_stable() {
	// Downstream truncation detection keys on this exact string
	assert_eq!(TRUNCATION_NOTICE_TAG, "⚠️ MCP RESPONSE TRUNCATED");
}

#[test]
fn test_mcp_truncation_empty_content() {
	let (result, was_truncated) = truncate_mcp_response_global("", 100, "view");
	assert_eq!(result, "");
	assert!(!was_truncated);
}

#[test]
fn test_mcp_truncation_exact_token_boundary() {
	let content = "word ".repeat(50);
	let tokens = crate::session::estimate_tokens(&content);
	assert!(tokens >= 2, "test needs a multi-token payload");
	// Exactly at the budget: untouched
	let (at_limit, t1) = truncate_mcp_response_global(&content, tokens, "view");
	assert_eq!(at_limit, content);
	assert!(!t1);
	// Over budget: truncated, and the notice reports the true original size
	let (over, t2) = truncate_mcp_response_global(&content, tokens - 1, "view");
	assert!(t2);
	assert!(over.contains(&format!("of ~{tokens} tokens")));
}

#[test]
fn test_mcp_truncation_unicode_keeps_valid_prefix() {
	let content = "日本語テストデータ".repeat(100);
	let tokens = crate::session::estimate_tokens(&content);
	let (result, was_truncated) = truncate_mcp_response_global(&content, tokens / 2, "view");
	assert!(was_truncated);
	assert!(result.contains(TRUNCATION_NOTICE_TAG));
	// The kept body must be a byte prefix of the original — cutting a
	// multibyte payload must never corrupt a codepoint
	let sep = result
		.find("\n\n──────────\n")
		.expect("notice separator present");
	assert!(content.starts_with(&result[..sep]));
}
