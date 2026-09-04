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

/// Tokens held back from `max_tokens` to pay for the notice, so a truncated
/// result FITS its budget. This is what makes truncation idempotent: the second
/// pass sees content already inside the cap and returns it untouched.
///
/// It replaces asking the CONTENT whether it had been truncated
/// (`content.contains(TRUNCATION_NOTICE_TAG)`) — a question any payload could
/// answer "yes" to by merely quoting a notice, and payloads do quote them:
/// session transcripts and spill files store earlier notices verbatim. One
/// `view` of a session file therefore switched off its own 6k cap and admitted
/// 142k tokens as a single tool message, which no compression could drain (it
/// sits in the protected live exchange) and no request could carry — the turn
/// had nowhere to go but the ceiling error.
const NOTICE_TOKEN_RESERVE: usize = 400;

/// How far back from the end to look for a notice we wrote. Comfortably over
/// the longest notice (~620 bytes with a spill path), and nowhere near enough
/// to see a quotation buried in a large body.
const NOTICE_TAIL_SCAN_BYTES: usize = 4096;

/// Does this content END with a notice we wrote? Only consulted when the cap is
/// too small to hold a notice, where the budget arithmetic cannot make
/// truncation idempotent on its own. Scoped to the tail because the tail is the
/// only place a notice of ours can be: scanning the whole body is exactly what
/// let a payload that merely QUOTES a notice switch its own cap off.
fn tail_carries_notice(content: &str) -> bool {
	let from = floor_char_boundary(
		content,
		content.len().saturating_sub(NOTICE_TAIL_SCAN_BYTES),
	);
	content[from..].contains(TRUNCATION_NOTICE_TAG)
}

/// Truncate an MCP tool response to fit within `max_tokens`.
///
/// Truncation is NOT an error — the call succeeded and returned usable data, so
/// the result stays a success. We keep the first tokens that fit once the
/// notice is paid for (see `NOTICE_TOKEN_RESERVE`) and append that
/// prominent, actionable notice (recency: it sits right before the model's
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

	// A budget below the notice's own size cannot contain a truncated result, so
	// the arithmetic below cannot make a second pass a no-op. Only there do we
	// fall back to recognizing our own notice.
	if max_tokens <= NOTICE_TOKEN_RESERVE && tail_carries_notice(content) {
		return (content.to_string(), false);
	}

	let token_count = estimate_tokens(content);
	if token_count <= max_tokens {
		return (content.to_string(), false);
	}

	// The notice is paid for out of the budget, not added on top of it, so the
	// returned content lands inside `max_tokens` and the check above makes a
	// second pass over it a no-op. That is the whole idempotency mechanism.
	//
	// Never more than half the budget, though: a cap smaller than the notice
	// would otherwise return one token of content and a paragraph of apology —
	// and small caps are real (tests, tight budgets), which is where a fixed
	// reserve cut a dedup placeholder down to "[d".
	let reserve = NOTICE_TOKEN_RESERVE.min(max_tokens / 2);
	let shown = max_tokens.saturating_sub(reserve).max(1);
	let truncated = crate::session::truncate_to_tokens(content, shown);
	let omitted = token_count.saturating_sub(shown);
	let hint = truncation_hint(tool_name);

	// Lossless path: spill the full body to a session file and hand back a path
	// handle so the model can read the exact span it needs instead of losing the
	// tail. Falls back to the lossy notice when there is no session (CLI/tests) or
	// the write fails — both still carry the tag + tool-specific narrowing hint.
	let notice = match crate::utils::spill::write_spill(tool_name, content) {
		Some(path) => format!(
			"\n\n──────────\n{TRUNCATION_NOTICE_TAG}: showing only the first ~{shown} of ~{token_count} tokens (~{omitted} cut from the end). Identical arguments return this same truncated output, so re-running wastes a turn. The full output was saved here:\n  {path}\nTo get the rest, read the span you need from that file (a line range or a single symbol), or run a search tool against it to jump to the exact pattern. To return less at the source next time, {hint}.",
			path = path.display(),
		),
		None => format!(
			"\n\n──────────\n{TRUNCATION_NOTICE_TAG}: showing only the first ~{shown} of ~{token_count} tokens (~{omitted} cut from the end). The cut tail is not in this result, and identical arguments return this same truncated output, so re-running wastes a turn. To get the rest, re-issue a narrower call that returns less — {hint}. If what you need is in the shown portion above, read or search it there instead of re-fetching."
		),
	};

	(format!("{truncated}{notice}"), true)
}

#[cfg(test)]
#[path = "truncation_tests.rs"]
mod tests;
