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

// Reference-parsing patterns, compiled once (parse_file_references is called
// per rendered file-context block, so per-call compilation was pure waste).
static CONTEXT_TAG_PATTERN: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(r"(?s)<context>(.*?)</context>").expect("valid context-tag regex"));
static CODE_BLOCK_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
	Regex::new(r"```(?:\w+)?\s*\n((?:[^\n`]+:[0-9]+:[0-9]+\s*\n?)+)\s*```")
		.expect("valid code-block regex")
});
static FILE_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
	Regex::new(r"^([A-Za-z]:[^\n]+|[^\n]+):(\d+):(\d+)\s*$").expect("valid file-ref regex")
});
static GENERAL_FILE_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
	Regex::new(r"(?:^|\s|-)([A-Za-z]:[^\s\n:]+|[^\s\n:]+):(\d+):(\d+)")
		.expect("valid general file-ref regex")
});
static FALLBACK_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
	Regex::new(r"([A-Za-z]:[^\s:]+|[^\s:]+):(\d+):(\d+)").expect("valid fallback file-ref regex")
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

	// PRIORITY 1: Try to find contexts within <context> tags (NEW preferred format)
	for context_block in CONTEXT_TAG_PATTERN.captures_iter(content) {
		if let Some(block_content) = context_block.get(1) {
			// Parse each line in the context block
			for line in block_content.as_str().lines() {
				let line = line.trim();
				if line.is_empty() {
					continue;
				}
				if let Some(captures) = FILE_PATTERN.captures(line) {
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
		for code_block in CODE_BLOCK_PATTERN.captures_iter(content) {
			if let Some(block_content) = code_block.get(1) {
				// Parse each line in the code block
				for line in block_content.as_str().lines() {
					let line = line.trim();
					if let Some(captures) = FILE_PATTERN.captures(line) {
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
			// `find` returns BYTE offsets, which are always valid char boundaries —
			// slice by bytes. The previous code fed the byte offset to
			// chars().skip()/take() as if it were a char count, corrupting section
			// extraction whenever earlier content contained non-ASCII.
			let content_after_header = &content[section_start..];

			// Find the end of this section (next ## header or end of text)
			let section_end = content_after_header
				.find("\n## ")
				.unwrap_or(content_after_header.len());

			let section_content = &content_after_header[..section_end];

			// More flexible pattern for general text (handles paths with spaces/special chars)
			for captures in GENERAL_FILE_PATTERN.captures_iter(section_content) {
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
		for captures in FALLBACK_PATTERN.captures_iter(content) {
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
#[path = "file_parser_tests.rs"]
mod tests;
