// Copyright 2025 Muvon Un Limited
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

/// Truncate content based on token count with smart boundary detection
///
/// This is adapted from the shell module's truncation logic
///
/// # Arguments
/// * `content` - The content to truncate
/// * `max_tokens` - Maximum tokens allowed
///
/// # Returns
/// Truncated content with clear indication if truncated
pub fn truncate_content_smart(content: &str, max_tokens: usize) -> String {
	let token_count = estimate_tokens(content);

	if token_count <= max_tokens {
		return content.to_string();
	}

	// Simple truncation - cut at character boundary
	// Estimate roughly where to cut (tokens are ~4 chars average)
	let estimated_chars = max_tokens * 3; // Conservative estimate
	let truncated = if content.len() > estimated_chars {
		&content[..estimated_chars]
	} else {
		content
	};

	// Find last newline to avoid cutting mid-line
	let last_newline = truncated.rfind('\n').unwrap_or(truncated.len());
	let final_truncated = &truncated[..last_newline];

	format!(
        "{}\n\n[Content truncated - {} tokens estimated, max {} allowed. Use more specific commands to reduce output size]",
        final_truncated,
        token_count,
        max_tokens
    )
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
