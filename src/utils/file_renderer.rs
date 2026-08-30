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

//! File content rendering utilities for displaying file content in various formats
//!
//! This module provides reusable functions for:
//! - Rendering file content in XML format with proper escaping
//! - Rendering file content in traditional text format for backward compatibility
//! - Handling multiple line ranges per file
//! - Configurable rendering options

use crate::utils::file_parser::{
	parse_file_references, read_multiple_files, FileContent, LineRange,
};
use regex::Regex;
use std::collections::HashMap;
use std::sync::LazyLock;

// Context block extraction regex - compiled once for performance
// Uses (?s) flag to match across newlines
static CONTEXT_EXTRACT_REGEX: LazyLock<Regex> = LazyLock::new(|| {
	Regex::new(r"(?s)<context>(.*?)</context>").expect("Failed to compile context extraction regex")
});

/// Expand context blocks in text by replacing them with rendered file content
/// Finds all <context>...</context> blocks containing file references (file:1:3),
/// reads the actual files, renders as XML, and replaces the context block
pub fn expand_context_blocks(text: &str) -> String {
	let mut result = text.to_string();

	// Find all context blocks and collect them first to avoid iterator invalidation
	let matches: Vec<_> = CONTEXT_EXTRACT_REGEX.find_iter(text).collect();

	// Process matches in reverse order to maintain string indices
	for context_match in matches.iter().rev() {
		let full_match = context_match.as_str();

		// Extract content inside context tags
		if let Some(captures) = CONTEXT_EXTRACT_REGEX.captures(full_match) {
			if let Some(context_content) = captures.get(1) {
				let file_refs_text = context_content.as_str();

				// Parse file references from context content
				let file_refs = parse_file_references(file_refs_text);

				if !file_refs.is_empty() {
					// Read the actual files
					let file_contents = read_multiple_files(&file_refs);

					// Render as XML (this reads the actual file content)
					let expanded_content = render_files_as_xml(&file_contents);

					// Replace the context block with expanded XML content
					let start = context_match.start();
					let end = context_match.end();
					result.replace_range(start..end, &expanded_content);
				} else {
					// If no valid file references found, remove the empty context block
					let start = context_match.start();
					let end = context_match.end();
					result.replace_range(start..end, "");
				}
			}
		}
	}

	result
}

/// Expand `@path` mentions in user input by appending the referenced files
/// rendered as XML (same pipeline as context blocks). A token qualifies when
/// it starts with `@` and the remainder is an existing readable text file —
/// anything else (nonexistent path, directory, binary) is left untouched.
/// The `@path` mention itself stays in place so the model sees where the
/// file was referenced.
pub fn expand_at_file_refs(text: &str) -> String {
	let mut file_contents: HashMap<String, Vec<FileContent>> = HashMap::new();

	for token in text.split_whitespace() {
		let Some(path) = token.strip_prefix('@') else {
			continue;
		};
		if path.is_empty() || file_contents.contains_key(path) {
			continue;
		}
		if !std::path::Path::new(path).is_file() {
			continue;
		}
		// Binary / non-UTF8 files fail here and stay a plain mention.
		let Ok(content) = std::fs::read_to_string(path) else {
			continue;
		};

		// LineRange enforces a 10000-line ceiling; clamp oversized files to it.
		let line_count = content.lines().count().clamp(1, 10000);
		let range = LineRange::new(1, line_count).expect("range within ceiling");
		let lines = content
			.lines()
			.take(line_count)
			.enumerate()
			.map(|(i, line)| format!("{}: {}", i + 1, line))
			.collect();

		file_contents.insert(
			path.to_string(),
			vec![FileContent {
				path: path.to_string(),
				lines,
				line_range: range,
				error: None,
			}],
		);
	}

	if file_contents.is_empty() {
		return text.to_string();
	}
	format!("{}\n\n{}", text, render_files_as_xml(&file_contents))
}

/// Rendering format options
#[derive(Debug, Clone, PartialEq)]
pub enum RenderFormat {
	/// XML format: <content path="..." lines="start:end">content</content>
	Xml,
	/// Traditional text format: === filepath (lines start-end) ===
	Text,
}

/// Rendering configuration options
#[derive(Debug, Clone)]
pub struct RenderOptions {
	pub format: RenderFormat,
	pub show_line_numbers: bool,
	pub include_header: bool,
}

impl Default for RenderOptions {
	fn default() -> Self {
		Self {
			format: RenderFormat::Xml,
			show_line_numbers: true,
			include_header: true,
		}
	}
}

/// Render file contents in XML format
///
/// Takes a HashMap of file paths to their FileContent and renders them
/// in XML format with proper escaping and structure
pub fn render_files_as_xml(file_contents: &HashMap<String, Vec<FileContent>>) -> String {
	let options = RenderOptions {
		format: RenderFormat::Xml,
		..Default::default()
	};
	render_files_with_options(file_contents, &options)
}

/// Render file contents in traditional text format
///
/// Provides backward compatibility with the existing === filepath === format
pub fn render_files_as_text(file_contents: &HashMap<String, Vec<FileContent>>) -> String {
	let options = RenderOptions {
		format: RenderFormat::Text,
		..Default::default()
	};
	render_files_with_options(file_contents, &options)
}

/// Render file contents with custom options
///
/// Main rendering function that supports both XML and text formats
pub fn render_files_with_options(
	file_contents: &HashMap<String, Vec<FileContent>>,
	options: &RenderOptions,
) -> String {
	if file_contents.is_empty() {
		return "No specific file context requested.".to_string();
	}

	let mut result = String::new();

	if options.include_header {
		result.push_str("FILE CONTEXT:\n\n");
	}

	// Sort files by path for consistent output
	let mut sorted_files: Vec<_> = file_contents.iter().collect();
	sorted_files.sort_by_key(|(path, _)| *path);

	for (_filepath, contents) in sorted_files {
		for content in contents {
			match options.format {
				RenderFormat::Xml => {
					render_single_file_xml(&mut result, content);
				}
				RenderFormat::Text => {
					render_single_file_text(&mut result, content);
				}
			}
		}
	}

	result
}

/// Render a single file in XML format
fn render_single_file_xml(result: &mut String, content: &FileContent) {
	if let Some(error) = &content.error {
		// Render error in XML format
		result.push_str(&format!(
			"<content path=\"{}\" lines=\"{}:{}\" error=\"true\">\n{}\n</content>\n\n",
			xml_escape(&content.path),
			content.line_range.start,
			content.line_range.end,
			xml_escape(error)
		));
	} else {
		// Render successful content in XML format
		let lines_str = if content.line_range.start == content.line_range.end {
			content.line_range.start.to_string()
		} else {
			format!("{}:{}", content.line_range.start, content.line_range.end)
		};

		result.push_str(&format!(
			"<content path=\"{}\" lines=\"{}\">\n",
			xml_escape(&content.path),
			lines_str
		));

		for line in &content.lines {
			result.push_str(&xml_escape(line));
			result.push('\n');
		}

		result.push_str("</content>\n\n");
	}
}

/// Render a single file in traditional text format
fn render_single_file_text(result: &mut String, content: &FileContent) {
	result.push_str(&format!(
		"=== {} (lines {}-{}) ===\n",
		content.path, content.line_range.start, content.line_range.end
	));

	if let Some(error) = &content.error {
		result.push_str(&format!("// {}\n", error));
	} else {
		for line in &content.lines {
			result.push_str(line);
			result.push('\n');
		}
	}

	result.push('\n');
}

/// Escape XML special characters
fn xml_escape(text: &str) -> String {
	text.replace('&', "&amp;")
		.replace('<', "&lt;")
		.replace('>', "&gt;")
		.replace('"', "&quot;")
		.replace('\'', "&#39;")
}

/// Merge overlapping or adjacent line ranges for the same file
///
/// This function optimizes rendering by combining ranges that are close together
pub fn merge_line_ranges(ranges: &[LineRange]) -> Vec<LineRange> {
	if ranges.is_empty() {
		return Vec::new();
	}

	let mut sorted_ranges = ranges.to_vec();
	sorted_ranges.sort_by_key(|r| r.start);

	let mut merged = Vec::new();
	let mut current = sorted_ranges[0].clone();

	for range in sorted_ranges.iter().skip(1) {
		// Merge if ranges overlap or are adjacent (within 5 lines)
		if range.start <= current.end + 5 {
			current.end = current.end.max(range.end);
		} else {
			merged.push(current);
			current = range.clone();
		}
	}
	merged.push(current);

	merged
}

#[cfg(test)]
#[path = "file_renderer_tests.rs"]
mod tests;
