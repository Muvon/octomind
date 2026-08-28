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

// Shared truncation utilities for smart content display across MCP tools

/// Find the largest byte index ≤ `index` that is a valid UTF-8 char boundary.
/// Equivalent to `str::floor_char_boundary` (stable in Rust 1.91+), provided here
/// for MSRV 1.82 compatibility.
#[inline]
pub fn floor_char_boundary(s: &str, index: usize) -> usize {
	if index >= s.len() {
		s.len()
	} else {
		let mut i = index;
		while i > 0 && !s.is_char_boundary(i) {
			i -= 1;
		}
		i
	}
}
use crate::session::estimate_tokens;

/// Format content with line numbers and smart elision for display
///
/// This function provides sophisticated truncation with context preservation:
/// - Shows first few lines, then [... X lines more], then requested content,
///   then [... X lines more], then last few lines
/// - Maintains proper line numbering from the source
///
/// # Arguments
/// * `lines` - The lines to format
/// * `start_line_number` - The actual line number of the first line (1-indexed)
/// * `view_range` - Optional range (start, end) for smart elision, both 1-indexed
///
/// # Returns
/// Formatted string with line numbers and smart elision
pub fn format_content_with_line_numbers(
	lines: &[&str],
	start_line_number: usize,
	view_range: Option<(usize, i64)>,
) -> String {
	if let Some((start, end)) = view_range {
		// Handle view_range parameter with smart elision
		let start_idx = if start == 0 {
			0
		} else {
			start.saturating_sub(1)
		}; // Convert to 0-indexed
		let end_idx = if end == -1 {
			lines.len()
		} else {
			(end as usize).min(lines.len())
		};

		if start_idx >= lines.len() || start_idx > end_idx {
			// Return error info for invalid ranges
			return if start_idx >= lines.len() {
				format!(
					"Start line {} exceeds content length ({} lines)",
					start,
					lines.len()
				)
			} else {
				format!(
					"Start line {} must be less than or equal to end line {}",
					start, end
				)
			};
		}

		// Smart elision: show context around the requested range
		let mut result_lines = Vec::new();

		// Show lines before the range if there's a significant gap
		if start_idx > 3 {
			// Show first few lines
			for (i, line) in lines.iter().enumerate().take(2) {
				result_lines.push(format!("{}: {}", start_line_number + i, line));
			}
			if start_idx > 5 {
				result_lines.push(format!("[...{} lines more]", start_idx - 2));
			} else {
				// Show the gap lines
				for (i, line) in lines.iter().enumerate().take(start_idx).skip(2) {
					result_lines.push(format!("{}: {}", start_line_number + i, line));
				}
			}
		} else {
			// Show all lines from beginning to start
			for (i, line) in lines.iter().enumerate().take(start_idx) {
				result_lines.push(format!("{}: {}", start_line_number + i, line));
			}
		}

		// Show the requested range
		for (i, line) in lines.iter().enumerate().take(end_idx).skip(start_idx) {
			result_lines.push(format!("{}: {}", start_line_number + i, line));
		}

		// Show lines after the range if there's a significant gap
		let remaining_lines = lines.len() - end_idx;
		if remaining_lines > 3 {
			if remaining_lines > 5 {
				result_lines.push(format!("[...{} lines more]", remaining_lines - 2));
				// Show last few lines
				for (i, line) in lines.iter().enumerate().skip(lines.len() - 2) {
					result_lines.push(format!("{}: {}", start_line_number + i, line));
				}
			} else {
				// Show the remaining lines
				for (i, line) in lines.iter().enumerate().skip(end_idx) {
					result_lines.push(format!("{}: {}", start_line_number + i, line));
				}
			}
		} else {
			// Show all remaining lines
			for (i, line) in lines.iter().enumerate().skip(end_idx) {
				result_lines.push(format!("{}: {}", start_line_number + i, line));
			}
		}

		result_lines.join("\n")
	} else {
		// Show entire content with line numbers
		lines
			.iter()
			.enumerate()
			.map(|(i, line)| format!("{}: {}", start_line_number + i, line))
			.collect::<Vec<_>>()
			.join("\n")
	}
}

/// Format extracted content with proper line numbers and smart truncation
///
/// # Arguments
/// * `lines` - The extracted lines
/// * `start_line` - The actual line number of the first extracted line (1-indexed)
/// * `max_display_lines` - Optional maximum lines to display before truncation
///
/// # Returns
/// Formatted string with proper line numbers and smart truncation
pub fn format_extracted_content_smart(
	lines: &[&str],
	start_line: usize,
	max_display_lines: Option<usize>,
) -> String {
	let max_lines = max_display_lines.unwrap_or(50); // Default to 50 lines

	if lines.len() <= max_lines {
		// Show all lines with proper numbering
		lines
			.iter()
			.enumerate()
			.map(|(i, line)| format!("{}: {}", start_line + i, line))
			.collect::<Vec<_>>()
			.join("\n")
	} else {
		// Apply smart truncation: show first part, elision, last part
		let show_first = (max_lines * 2) / 3; // Show 2/3 at start
		let show_last = max_lines - show_first - 1; // Reserve 1 line for elision marker

		let mut result_lines = Vec::new();

		// Show first lines
		for (i, line) in lines.iter().enumerate().take(show_first) {
			result_lines.push(format!("{}: {}", start_line + i, line));
		}

		// Add elision marker
		let hidden_lines = lines.len() - show_first - show_last;
		result_lines.push(format!("[...{} lines more]", hidden_lines));

		// Show last lines
		let skip_count = lines.len() - show_last;
		for (i, line) in lines.iter().enumerate().skip(skip_count) {
			result_lines.push(format!("{}: {}", start_line + i, line));
		}

		result_lines.join("\n")
	}
}

/// Simple line-based truncation for tool outputs
///
/// This is adapted from the tool_display module's logic
///
/// # Arguments
/// * `content` - The content to truncate
/// * `max_lines` - Maximum lines to show
/// * `max_chars` - Maximum characters to show
///
/// # Returns
/// Truncated content with indication if truncated
pub fn truncate_tool_output_smart(content: &str, max_lines: usize, max_chars: usize) -> String {
	let lines: Vec<&str> = content.lines().collect();

	if lines.len() <= max_lines && content.chars().count() <= max_chars {
		// Small output: show as-is
		content.to_string()
	} else if lines.len() > max_lines {
		// Many lines: show first N lines + summary
		let show_lines = max_lines.saturating_sub(1); // Reserve 1 line for summary
		let mut result = lines
			.iter()
			.take(show_lines)
			.cloned()
			.collect::<Vec<_>>()
			.join("\n");
		result.push_str(&format!(
			"\n... [{} more lines]",
			lines.len().saturating_sub(show_lines)
		));
		result
	} else {
		// Long single line or few long lines: truncate by characters
		let truncated: String = content.chars().take(max_chars.saturating_sub(3)).collect();
		format!("{}...", truncated)
	}
}

/// Sentinel marking an MCP tool response as truncated. Downstream code (the
/// dedup escalation and the idempotency guard below) detects truncated content
/// by this tag, so it MUST stay stable and distinctive.
pub const TRUNCATION_NOTICE_TAG: &str = "⚠️ MCP RESPONSE TRUNCATED";

/// Tool-specific, actionable advice for narrowing output when a response is
/// truncated. The old generic tail ("use more specific commands") was ignored
/// by the model; concrete per-tool knobs give it a deterministic next step
/// instead of blindly re-running or tweaking arguments.
pub fn truncation_hint(tool_name: &str) -> &'static str {
	match tool_name {
		"view" | "text_editor" | "read" | "extract_lines" => {
			"request a specific line range (view_range / offset+limit) or a single symbol instead of the whole file"
		}
		"view_signatures" => "request fewer files — pass specific paths, one or a few at a time",
		"shell" => {
			"narrow the output: pipe through grep/head/tail, target a subpath, or redirect to a file and read ranges"
		}
		"list_files" | "workdir" => {
			"target a specific subdirectory or add a name/glob filter instead of listing everything"
		}
		"ast_grep" => "tighten the pattern or restrict the search to a specific path",
		name if name.contains("search") || name.contains("find") || name.contains("graphrag") => {
			"use a more specific query and request fewer results"
		}
		_ => "narrow the request: target a specific subset, add a filter, or ask for fewer items",
	}
}

/// Truncate an MCP tool response to fit within `max_tokens`.
///
/// Truncation is NOT an error — the call succeeded and returned usable data, so
/// the result stays a success. We keep the FIRST `max_tokens` tokens and append
/// a prominent, actionable notice (recency: it sits right before the model's
/// next turn). The notice states what was cut, the tool-specific way to get the
/// rest, and that re-running with identical arguments returns the SAME output —
/// the single biggest driver of truncation retry loops.
///
/// Returns `(content, was_truncated)`. `max_tokens == 0` disables truncation.
pub fn truncate_mcp_response_global(
	content: &str,
	max_tokens: usize,
	tool_name: &str,
) -> (String, bool) {
	if max_tokens == 0 {
		return (content.to_string(), false);
	}

	// Idempotency guard: every executed result already flows through the single
	// truncation choke point (`handle_large_tool_results`). Content carrying our
	// tag was truncated there; re-truncating would chop the notice and report
	// wrong token counts, so leave it untouched.
	if content.contains(TRUNCATION_NOTICE_TAG) {
		return (content.to_string(), false);
	}

	let token_count = estimate_tokens(content);
	if token_count <= max_tokens {
		return (content.to_string(), false);
	}

	let truncated = crate::session::truncate_to_tokens(content, max_tokens);
	let omitted = token_count.saturating_sub(max_tokens);
	let hint = truncation_hint(tool_name);

	// Lossless path: spill the full body to a session file and hand back a path
	// handle so the model can read the exact span it needs instead of losing the
	// tail. Falls back to the lossy notice when there is no session (CLI/tests) or
	// the write fails — both still carry the tag + tool-specific narrowing hint.
	let notice = match crate::utils::spill::write_spill(tool_name, content) {
		Some(path) => format!(
			"\n\n──────────\n{TRUNCATION_NOTICE_TAG}: showing only the first ~{max_tokens} of ~{token_count} tokens (~{omitted} cut from the end). Identical arguments return this same truncated output, so re-running wastes a turn. The full output was saved here:\n  {path}\nTo get the rest, read the span you need from that file (a line range or a single symbol), or run a search tool against it to jump to the exact pattern. To return less at the source next time, {hint}.",
			path = path.display(),
		),
		None => format!(
			"\n\n──────────\n{TRUNCATION_NOTICE_TAG}: showing only the first ~{max_tokens} of ~{token_count} tokens (~{omitted} cut from the end). The cut tail is not in this result, and identical arguments return this same truncated output, so re-running wastes a turn. To get the rest, re-issue a narrower call that returns less — {hint}. If what you need is in the shown portion above, read or search it there instead of re-fetching."
		),
	};

	(format!("{truncated}{notice}"), true)
}

#[cfg(test)]
mod tests {
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
		let content = "This is a very long content that should be truncated when it exceeds the token limit. ".repeat(100);
		let (result, was_truncated) = truncate_mcp_response_global(&content, 50, "shell");
		assert!(result.contains(TRUNCATION_NOTICE_TAG));
		// Notice carries the tool-specific hint (shell → grep/head/tail).
		assert!(result.contains("grep"));
		assert!(result.len() < content.len());
		assert!(was_truncated);
	}

	#[test]
	fn test_mcp_truncation_is_idempotent() {
		// A result already truncated upstream carries the tag; re-truncating must
		// leave it byte-for-byte intact (no double notice, no count corruption).
		let content = "x ".repeat(1000);
		let (once, t1) = truncate_mcp_response_global(&content, 50, "shell");
		assert!(t1);
		let (twice, t2) = truncate_mcp_response_global(&once, 50, "shell");
		assert!(!t2);
		assert_eq!(once, twice);
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
		// One token under budget: truncated, notice reports both counts
		let (over, t2) = truncate_mcp_response_global(&content, tokens - 1, "view");
		assert!(t2);
		assert!(over.contains(&format!("~{} of ~{} tokens", tokens - 1, tokens)));
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
}
