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
}
