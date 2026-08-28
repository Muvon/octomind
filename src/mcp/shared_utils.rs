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

// Shared utilities for MCP tools - consistent truncation logic

/// Apply head-only truncation to a list of lines
///
/// This function provides consistent truncation behavior across MCP tools:
/// - Takes first (max_lines - 1) lines to preserve logical flow
/// - Adds truncation marker with count information
/// - Returns truncated lines and optional truncation info
pub fn apply_head_truncation(lines: &[String], max_lines: usize) -> (Vec<String>, Option<String>) {
	if max_lines > 0 && lines.len() > max_lines {
		let mut truncated = Vec::new();

		// Take first (max_lines - 1) lines to leave room for truncation marker
		truncated.extend(lines.iter().take(max_lines - 1).cloned());

		// Add truncation marker
		let truncated_count = lines.len() - (max_lines - 1);
		truncated.push(format!(
			"[{} lines truncated - use more specific patterns or increase max_lines]",
			truncated_count
		));

		(
			truncated,
			Some(format!(
				"Output truncated: showing {} of {} total lines",
				max_lines,
				lines.len()
			)),
		)
	} else {
		(lines.to_vec(), None)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_no_truncation_when_under_limit() {
		let lines = vec!["line1".to_string(), "line2".to_string()];
		let (result, info) = apply_head_truncation(&lines, 5);

		assert_eq!(result, lines);
		assert!(info.is_none());
	}

	#[test]
	fn test_truncation_when_over_limit() {
		let lines = vec![
			"line1".to_string(),
			"line2".to_string(),
			"line3".to_string(),
			"line4".to_string(),
			"line5".to_string(),
		];
		let (result, info) = apply_head_truncation(&lines, 3);

		assert_eq!(result.len(), 3);
		assert_eq!(result[0], "line1");
		assert_eq!(result[1], "line2");
		assert!(result[2].contains("3 lines truncated"));
		assert!(info.is_some());
		assert!(info.unwrap().contains("showing 3 of 5 total lines"));
	}

	#[test]
	fn test_unlimited_when_max_lines_zero() {
		let lines = vec!["line1".to_string(), "line2".to_string()];
		let (result, info) = apply_head_truncation(&lines, 0);

		assert_eq!(result, lines);
		assert!(info.is_none());
	}

	fn numbered_lines(n: usize) -> Vec<String> {
		(1..=n).map(|i| format!("line{}", i)).collect()
	}

	#[test]
	fn test_empty_input_returns_empty_without_truncation() {
		for max in [0, 1, 5] {
			let (result, info) = apply_head_truncation(&[], max);
			assert!(result.is_empty(), "max={} must not fabricate lines", max);
			assert!(info.is_none(), "max={} must not report truncation", max);
		}
	}

	#[test]
	fn test_exactly_at_limit_is_not_truncated() {
		// Boundary: len == max_lines must pass through untouched.
		let input = numbered_lines(3);
		let (result, info) = apply_head_truncation(&input, 3);
		assert_eq!(result, input);
		assert!(info.is_none());
	}

	#[test]
	fn test_max_lines_one_keeps_only_marker() {
		let (result, info) = apply_head_truncation(&numbered_lines(4), 1);
		assert_eq!(
			result,
			vec![
				"[4 lines truncated - use more specific patterns or increase max_lines]"
					.to_string()
			]
		);
		assert_eq!(
			info.as_deref(),
			Some("Output truncated: showing 1 of 4 total lines")
		);
	}

	#[test]
	fn test_truncation_preserves_head_order_and_exact_marker() {
		let (result, info) = apply_head_truncation(&numbered_lines(10), 4);
		assert_eq!(result.len(), 4);
		assert_eq!(
			result[..3].to_vec(),
			vec![
				"line1".to_string(),
				"line2".to_string(),
				"line3".to_string()
			]
		);
		assert_eq!(
			result[3],
			"[7 lines truncated - use more specific patterns or increase max_lines]"
		);
		assert_eq!(
			info.as_deref(),
			Some("Output truncated: showing 4 of 10 total lines")
		);
	}

	#[test]
	fn test_zero_max_lines_disables_truncation_for_large_input() {
		let input = numbered_lines(100);
		let (result, info) = apply_head_truncation(&input, 0);
		assert_eq!(result.len(), 100);
		assert_eq!(result.last().map(String::as_str), Some("line100"));
		assert!(info.is_none());
	}

	#[test]
	fn test_blank_lines_count_toward_the_limit() {
		let input: Vec<String> = ["a", "", "b", "", "c"]
			.iter()
			.map(|s| s.to_string())
			.collect();
		let (result, info) = apply_head_truncation(&input, 3);
		assert_eq!(result.len(), 3);
		assert_eq!(result[0], "a");
		assert_eq!(result[1], "");
		assert_eq!(
			result[2],
			"[3 lines truncated - use more specific patterns or increase max_lines]"
		);
		assert_eq!(
			info.as_deref(),
			Some("Output truncated: showing 3 of 5 total lines")
		);
	}

	#[test]
	fn test_two_lines_with_max_one_reports_both_truncated() {
		let (result, info) = apply_head_truncation(&["only".to_string(), "second".to_string()], 1);
		assert_eq!(
			result,
			vec![
				"[2 lines truncated - use more specific patterns or increase max_lines]"
					.to_string()
			]
		);
		assert_eq!(
			info.as_deref(),
			Some("Output truncated: showing 1 of 2 total lines")
		);
	}
}
