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

//! File parsing utilities for extracting file references and reading file content
//!
//! This module provides reusable functions for:
//! - Parsing file references from text content (format: filepath:start:end)
//! - Reading specific line ranges from files
//! - Handling errors gracefully for missing files and invalid ranges

use anyhow::{Context, Result};
use regex::Regex;
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::sync::LazyLock;

// Fast context block detection regex - compiled once for performance
// Uses (?s) flag to match across newlines
static CONTEXT_BLOCK_REGEX: LazyLock<Regex> = LazyLock::new(|| {
	Regex::new(r"(?s)<context>.*?</context>").expect("Failed to compile context block regex")
});

/// Extremely fast detection of context blocks in text
/// Returns true if any complete <context>...</context> blocks are found
/// This is used as a gate before expensive parsing operations
pub fn has_context_blocks(text: &str) -> bool {
	CONTEXT_BLOCK_REGEX.is_match(text)
}

/// Represents a line range in a file (1-indexed, inclusive)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineRange {
	pub start: usize,
	pub end: usize,
}

impl LineRange {
	pub fn new(start: usize, end: usize) -> Option<Self> {
		if start > 0 && end >= start && end <= 10000 {
			Some(Self { start, end })
		} else {
			None
		}
	}
}

/// Represents file content with line numbers
#[derive(Debug, Clone)]
pub struct FileContent {
	pub path: String,
	pub lines: Vec<String>,
	pub line_range: LineRange,
	pub error: Option<String>,
}

/// Parse file references from text content
///
/// Supports multiple formats (in priority order):
/// - Context tags: <context>filepath:start:end</context> (PREFERRED)
/// - Code blocks: ```\nfilepath:start:end\n```
/// - Section headers: ## REQUIRED FILE CONTEXTS
/// - Inline references: filepath:start:end
///
/// Returns a HashMap mapping file paths to their line ranges
pub fn parse_file_references(content: &str) -> HashMap<String, Vec<LineRange>> {
	let mut file_refs = HashMap::new();

	// Pre-compile regex patterns for efficiency - Windows drive letter aware
	let context_tag_pattern = Regex::new(r"(?s)<context>(.*?)</context>").unwrap();
	let code_block_pattern =
		Regex::new(r"```(?:\w+)?\s*\n((?:[^\n`]+:[0-9]+:[0-9]+\s*\n?)+)\s*```").unwrap();
	// Windows-aware pattern: allows drive letters (C:) followed by path
	let file_pattern = Regex::new(r"^([A-Za-z]:[^\n]+|[^\n]+):(\d+):(\d+)\s*$").unwrap();
	let general_file_pattern =
		Regex::new(r"(?:^|\s|-)([A-Za-z]:[^\s\n:]+|[^\s\n:]+):(\d+):(\d+)").unwrap();
	let fallback_pattern = Regex::new(r"([A-Za-z]:[^\s:]+|[^\s:]+):(\d+):(\d+)").unwrap();

	// PRIORITY 1: Try to find contexts within <context> tags (NEW preferred format)
	for context_block in context_tag_pattern.captures_iter(content) {
		if let Some(block_content) = context_block.get(1) {
			// Parse each line in the context block
			for line in block_content.as_str().lines() {
				let line = line.trim();
				if line.is_empty() {
					continue;
				}
				if let Some(captures) = file_pattern.captures(line) {
					if let Some((filepath, range)) = extract_file_range(&captures) {
						file_refs
							.entry(filepath)
							.or_insert_with(Vec::new)
							.push(range);
					}
				}
			}
		}
	}

	// PRIORITY 2: Try to find contexts within code blocks (legacy format)
	if file_refs.is_empty() {
		for code_block in code_block_pattern.captures_iter(content) {
			if let Some(block_content) = code_block.get(1) {
				// Parse each line in the code block
				for line in block_content.as_str().lines() {
					let line = line.trim();
					if let Some(captures) = file_pattern.captures(line) {
						if let Some((filepath, range)) = extract_file_range(&captures) {
							file_refs
								.entry(filepath)
								.or_insert_with(Vec::new)
								.push(range);
						}
					}
				}
			}
		}
	}

	// If no code blocks found, fall back to looking for patterns in REQUIRED FILE CONTEXTS section
	if file_refs.is_empty() {
		if let Some(section_start) = content.find("## REQUIRED FILE CONTEXTS") {
			// UTF-8 safe: get substring from section start to end
			let content_after_header = content.chars().skip(section_start).collect::<String>();

			// Find the end of this section (next ## header or end of text)
			let section_end = content_after_header
				.find("\n## ")
				.unwrap_or(content_after_header.chars().count());

			// UTF-8 safe: get substring from start to section end
			let section_content = content_after_header
				.chars()
				.take(section_end)
				.collect::<String>();

			// More flexible pattern for general text (handles paths with spaces/special chars)
			for captures in general_file_pattern.captures_iter(&section_content) {
				if let Some((filepath, range)) = extract_file_range(&captures) {
					file_refs
						.entry(filepath)
						.or_insert_with(Vec::new)
						.push(range);
				}
			}
		}
	}

	// Final fallback: look anywhere in the content (most permissive)
	if file_refs.is_empty() {
		for captures in fallback_pattern.captures_iter(content) {
			if let Some((filepath, range)) = extract_file_range(&captures) {
				file_refs
					.entry(filepath)
					.or_insert_with(Vec::new)
					.push(range);
			}
		}
	}

	// Remove duplicates and sort ranges for each file
	for ranges in file_refs.values_mut() {
		ranges.sort_by_key(|r| r.start);
		ranges.dedup();
	}

	// Limit to maximum 10 files for performance
	let mut file_refs_vec: Vec<_> = file_refs.into_iter().collect();
	file_refs_vec.truncate(10);
	file_refs_vec.into_iter().collect()
}

/// Extract file path and line range from regex captures
fn extract_file_range(captures: &regex::Captures) -> Option<(String, LineRange)> {
	if let (Some(filename), Some(start_str), Some(end_str)) =
		(captures.get(1), captures.get(2), captures.get(3))
	{
		if let (Ok(start_line), Ok(end_line)) = (
			start_str.as_str().parse::<usize>(),
			end_str.as_str().parse::<usize>(),
		) {
			let filename = filename.as_str().trim().to_string();

			if !filename.is_empty() {
				if let Some(range) = LineRange::new(start_line, end_line) {
					return Some((filename, range));
				}
			}
		}
	}
	None
}

/// Read specific line ranges from a file
///
/// Returns FileContent with the requested lines or error information
pub fn read_file_lines(filepath: &str, range: &LineRange) -> FileContent {
	// On Windows, convert forward slashes to backslashes for file operations
	#[cfg(target_os = "windows")]
	let normalized_path = filepath.replace('/', "\\");
	#[cfg(not(target_os = "windows"))]
	let normalized_path = filepath.to_string();

	// Validate file exists and is readable
	if !Path::new(&normalized_path).exists() {
		return FileContent {
			path: filepath.to_string(),
			lines: Vec::new(),
			line_range: range.clone(),
			error: Some(format!("File not found: {}", filepath)),
		};
	}

	match read_file_lines_with_range(&normalized_path, range) {
		Ok(lines) => FileContent {
			path: filepath.to_string(),
			lines,
			line_range: range.clone(),
			error: None,
		},
		Err(e) => FileContent {
			path: filepath.to_string(),
			lines: Vec::new(),
			line_range: range.clone(),
			error: Some(format!("Error reading file: {}", e)),
		},
	}
}

/// Read file lines for a specific range
fn read_file_lines_with_range(filepath: &str, range: &LineRange) -> Result<Vec<String>> {
	let file =
		fs::File::open(filepath).with_context(|| format!("Failed to open file: {}", filepath))?;

	let reader = BufReader::new(file);
	let mut lines = Vec::new();

	for (line_num, line_result) in reader.lines().enumerate() {
		let line_number = line_num + 1; // Convert to 1-indexed

		if line_number < range.start {
			continue;
		}

		if line_number > range.end {
			break;
		}

		match line_result {
			Ok(line_content) => {
				lines.push(format!("{}: {}", line_number, line_content));
			}
			Err(e) => {
				lines.push(format!("{}: // Error reading line: {}", line_number, e));
			}
		}
	}

	Ok(lines)
}

/// Read multiple files with their line ranges
///
/// Returns a HashMap mapping file paths to their FileContent
pub fn read_multiple_files(
	file_refs: &HashMap<String, Vec<LineRange>>,
) -> HashMap<String, Vec<FileContent>> {
	let mut results = HashMap::new();

	for (filepath, ranges) in file_refs {
		let mut file_contents = Vec::new();

		for range in ranges {
			let content = read_file_lines(filepath, range);
			file_contents.push(content);
		}

		results.insert(filepath.clone(), file_contents);
	}

	results
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::fs;
	use std::io::Write;
	use tempfile::TempDir;

	fn create_test_file(dir: &TempDir, name: &str, content: &str) -> String {
		let file_path = dir.path().join(name);
		let mut file = fs::File::create(&file_path).unwrap();
		writeln!(file, "{}", content).unwrap();
		file_path.to_string_lossy().to_string()
	}

	#[test]
	fn test_has_context_blocks() {
		// Test with valid context blocks
		assert!(has_context_blocks("<context>src/main.rs:1:10</context>"));
		assert!(has_context_blocks(
			"Some text <context>file.rs:5:15</context> more text"
		));
		assert!(has_context_blocks(
			"<context>\nsrc/main.rs:1:10\nsrc/lib.rs:20:30\n</context>"
		));

		// Test multiple context blocks
		assert!(has_context_blocks(
			"<context>file1.rs:1:5</context> and <context>file2.rs:10:20</context>"
		));

		// Test without context blocks
		assert!(!has_context_blocks("No context blocks here"));
		assert!(!has_context_blocks("src/main.rs:1:10"));
		assert!(!has_context_blocks("Some text without context"));
		assert!(!has_context_blocks(""));

		// Test incomplete/malformed context blocks (should not match)
		assert!(!has_context_blocks("<context>malformed"));
		assert!(!has_context_blocks("malformed</context>"));
		assert!(!has_context_blocks("<context>incomplete"));
		assert!(!has_context_blocks("incomplete</context>"));
	}

	#[test]
	fn test_parse_file_references_code_block() {
		let content = r#"
## REQUIRED FILE CONTEXTS
List ALL files needed as context to continue work. Use EXACT format:
```
src/main.rs:1:50
src/lib.rs:100:150
config/settings.toml:10:20
```
        "#;

		let refs = parse_file_references(content);
		assert_eq!(refs.len(), 3);

		assert_eq!(refs["src/main.rs"].len(), 1);
		assert_eq!(refs["src/main.rs"][0], LineRange { start: 1, end: 50 });

		assert_eq!(refs["src/lib.rs"].len(), 1);
		assert_eq!(
			refs["src/lib.rs"][0],
			LineRange {
				start: 100,
				end: 150
			}
		);

		assert_eq!(refs["config/settings.toml"].len(), 1);
		assert_eq!(
			refs["config/settings.toml"][0],
			LineRange { start: 10, end: 20 }
		);
	}

	#[test]
	fn test_parse_file_references_section() {
		let content = r#"
## REQUIRED FILE CONTEXTS
The following files need context:
- src/session/mod.rs:200:300
- tests/integration.rs:1:100

## NEXT STEPS
Continue with implementation...
        "#;

		let refs = parse_file_references(content);
		assert_eq!(refs.len(), 2);

		assert_eq!(
			refs["src/session/mod.rs"][0],
			LineRange {
				start: 200,
				end: 300
			}
		);
		assert_eq!(
			refs["tests/integration.rs"][0],
			LineRange { start: 1, end: 100 }
		);
	}

	#[test]
	fn test_parse_file_references_fallback() {
		let content = r#"
We need to look at src/core.rs:50:100 and also check lib/utils.rs:1:25 for the implementation.
        "#;

		let refs = parse_file_references(content);
		assert_eq!(refs.len(), 2);

		assert_eq!(
			refs["src/core.rs"][0],
			LineRange {
				start: 50,
				end: 100
			}
		);
		assert_eq!(refs["lib/utils.rs"][0], LineRange { start: 1, end: 25 });
	}

	#[test]
	fn test_parse_file_references_invalid_ranges() {
		let content = r#"
```
src/main.rs:0:50
src/lib.rs:100:50
src/test.rs:1:20000
```
        "#;

		let refs = parse_file_references(content);
		// Should filter out invalid ranges (start=0, end<start, end>10000)
		assert_eq!(refs.len(), 0);
	}

	#[test]
	fn test_line_range_validation() {
		assert!(LineRange::new(1, 10).is_some());
		assert!(LineRange::new(0, 10).is_none()); // start=0 invalid
		assert!(LineRange::new(10, 5).is_none()); // end<start invalid
		assert!(LineRange::new(1, 20000).is_none()); // end>10000 invalid
	}

	#[test]
	fn test_read_file_lines() {
		let temp_dir = TempDir::new().unwrap();
		let file_path =
			create_test_file(&temp_dir, "test.txt", "line1\nline2\nline3\nline4\nline5");

		let range = LineRange::new(2, 4).unwrap();
		let content = read_file_lines(&file_path, &range);

		assert!(content.error.is_none());
		assert_eq!(content.lines.len(), 3);
		assert_eq!(content.lines[0], "2: line2");
		assert_eq!(content.lines[1], "3: line3");
		assert_eq!(content.lines[2], "4: line4");
	}

	#[test]
	fn test_read_file_lines_missing_file() {
		let range = LineRange::new(1, 10).unwrap();
		let content = read_file_lines("nonexistent.txt", &range);

		assert!(content.error.is_some());
		assert!(content.error.unwrap().contains("File not found"));
		assert!(content.lines.is_empty());
	}

	#[test]
	fn test_read_multiple_files() {
		let temp_dir = TempDir::new().unwrap();
		let file1 = create_test_file(&temp_dir, "file1.txt", "line1\nline2\nline3");
		let file2 = create_test_file(&temp_dir, "file2.txt", "lineA\nlineB\nlineC");

		let mut file_refs = HashMap::new();
		file_refs.insert(file1.clone(), vec![LineRange::new(1, 2).unwrap()]);
		file_refs.insert(file2.clone(), vec![LineRange::new(2, 3).unwrap()]);

		let results = read_multiple_files(&file_refs);

		assert_eq!(results.len(), 2);
		assert_eq!(results[&file1].len(), 1);
		assert_eq!(results[&file1][0].lines.len(), 2);
		assert_eq!(results[&file2].len(), 1);
		assert_eq!(results[&file2][0].lines.len(), 2);
	}

	#[test]
	fn test_duplicate_removal() {
		let content = r#"
```
src/main.rs:1:10
src/main.rs:1:10
src/main.rs:5:15
```
        "#;

		let refs = parse_file_references(content);
		assert_eq!(refs.len(), 1);
		assert_eq!(refs["src/main.rs"].len(), 2); // Duplicates removed
		assert_eq!(refs["src/main.rs"][0], LineRange { start: 1, end: 10 });
		assert_eq!(refs["src/main.rs"][1], LineRange { start: 5, end: 15 });
	}

	#[test]
	fn test_parse_context_tags() {
		let content = r#"
## REQUIRED FILE CONTEXTS
<context>
src/session/chat/continuation.rs:100:200
src/config/mod.rs:50:100
tests/integration_test.rs:1:50
</context>
        "#;

		let refs = parse_file_references(content);
		assert_eq!(refs.len(), 3);
		assert_eq!(
			refs["src/session/chat/continuation.rs"][0],
			LineRange {
				start: 100,
				end: 200
			}
		);
		assert_eq!(
			refs["src/config/mod.rs"][0],
			LineRange {
				start: 50,
				end: 100
			}
		);
		assert_eq!(
			refs["tests/integration_test.rs"][0],
			LineRange { start: 1, end: 50 }
		);
	}

	#[test]
	fn test_parse_context_tags_priority() {
		// Context tags should take priority over code blocks
		let content = r#"
<context>
src/main.rs:1:10
</context>

```
src/lib.rs:20:30
```
        "#;

		let refs = parse_file_references(content);
		// Should only parse context tags, not code blocks
		assert_eq!(refs.len(), 1);
		assert!(refs.contains_key("src/main.rs"));
		assert!(!refs.contains_key("src/lib.rs"));
	}

	#[test]
	fn test_parse_context_tags_with_empty_lines() {
		let content = r#"
<context>
src/main.rs:1:10

src/lib.rs:20:30

</context>
        "#;

		let refs = parse_file_references(content);
		assert_eq!(refs.len(), 2);
		assert_eq!(refs["src/main.rs"][0], LineRange { start: 1, end: 10 });
		assert_eq!(refs["src/lib.rs"][0], LineRange { start: 20, end: 30 });
	}
}
