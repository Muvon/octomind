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

// File context parsing and generation for session continuation

use anyhow::Result;
use regex::Regex;
use std::path::Path;

/// Collect meaningful user requests from session history for continuation context
/// Filters out system-generated messages and keeps last 5-7 user requests
pub fn collect_user_request_history(messages: &[crate::session::Message]) -> String {
	let mut user_requests = Vec::new();

	for message in messages {
		// Only collect user messages
		if message.role != "user" {
			continue;
		}

		// Skip system-generated continuation requests
		if message
			.content
			.contains("CRITICAL: Session approaching token limits")
		{
			continue;
		}

		// Skip empty or very short messages
		let content = message.content.trim();
		if content.is_empty() || content.len() < 10 {
			continue;
		}

		// Skip session commands (starting with /)
		if content.starts_with('/') {
			continue;
		}

		// Truncate very long requests for readability
		let truncated_content = if content.len() > 200 {
			format!("{}...", content.chars().take(200).collect::<String>())
		} else {
			content.to_string()
		};

		user_requests.push(truncated_content);
	}

	// Keep only the last 5-7 requests to avoid overwhelming context
	if user_requests.len() > 7 {
		user_requests = user_requests[user_requests.len() - 7..].to_vec();
	}

	if user_requests.is_empty() {
		return "No specific user requests found in session history.".to_string();
	}

	// Format as numbered list with proactive language
	let mut result = String::new();
	for (i, request) in user_requests.iter().enumerate() {
		result.push_str(&format!("{}. {}\n", i + 1, request));
	}

	result
}

/// Parse file context requirements from AI summary response
/// Expected format: filename:startline:endline (in code blocks or specific sections)
pub fn parse_file_contexts(summary_content: &str) -> Vec<(String, usize, usize)> {
	let mut contexts = Vec::new();

	// Pre-compile regex patterns outside loops
	let code_block_pattern =
		Regex::new(r"```(?:\w+)?\s*\n((?:[^\n`]+:[0-9]+:[0-9]+\s*\n?)+)\s*```").unwrap();
	let file_pattern = Regex::new(r"^([^:\n]+):(\d+):(\d+)\s*$").unwrap();
	let general_file_pattern = Regex::new(r"(?:^|\s|-)([^\s\n:]+):(\d+):(\d+)").unwrap();
	let fallback_pattern = Regex::new(r"([^\s:]+):(\d+):(\d+)").unwrap();

	// First, try to find contexts within code blocks (preferred format)
	for code_block in code_block_pattern.captures_iter(summary_content) {
		if let Some(block_content) = code_block.get(1) {
			// Parse each line in the code block
			for line in block_content.as_str().lines() {
				let line = line.trim();
				if let Some(captures) = file_pattern.captures(line) {
					if let (Some(filename), Some(start_str), Some(end_str)) =
						(captures.get(1), captures.get(2), captures.get(3))
					{
						if let (Ok(start_line), Ok(end_line)) = (
							start_str.as_str().parse::<usize>(),
							end_str.as_str().parse::<usize>(),
						) {
							let filename = filename.as_str().trim().to_string();

							// Validate line range and filename
							if start_line > 0
								&& end_line >= start_line
								&& end_line <= 10000 && !filename.is_empty()
							{
								contexts.push((filename, start_line, end_line));
							}
						}
					}
				}
			}
		}
	}

	// If no code blocks found, fall back to looking for patterns in REQUIRED FILE CONTEXTS section
	if contexts.is_empty() {
		// Look for the specific section header and parse content after it using simple string operations
		if let Some(section_start) = summary_content.find("## REQUIRED FILE CONTEXTS") {
			let content_after_header = &summary_content[section_start..];

			// Find the end of this section (next ## header or end of text)
			let section_end = content_after_header
				.find("\n## ")
				.unwrap_or(content_after_header.len());

			let section_content = &content_after_header[..section_end];

			// More flexible pattern for general text (handles paths with spaces/special chars)
			for captures in general_file_pattern.captures_iter(section_content) {
				if let (Some(filename), Some(start_str), Some(end_str)) =
					(captures.get(1), captures.get(2), captures.get(3))
				{
					if let (Ok(start_line), Ok(end_line)) = (
						start_str.as_str().parse::<usize>(),
						end_str.as_str().parse::<usize>(),
					) {
						let filename = filename.as_str().trim().to_string();

						// Validate line range and filename
						if start_line > 0
							&& end_line >= start_line
							&& end_line <= 10000 && !filename.is_empty()
						{
							contexts.push((filename, start_line, end_line));
						}
					}
				}
			}
		}
	}

	// Final fallback: look anywhere in the content (most permissive)
	if contexts.is_empty() {
		for captures in fallback_pattern.captures_iter(summary_content) {
			if let (Some(filename), Some(start_str), Some(end_str)) =
				(captures.get(1), captures.get(2), captures.get(3))
			{
				if let (Ok(start_line), Ok(end_line)) = (
					start_str.as_str().parse::<usize>(),
					end_str.as_str().parse::<usize>(),
				) {
					let filename = filename.as_str().to_string();

					// Validate line range
					if start_line > 0 && end_line >= start_line && end_line <= 10000 {
						contexts.push((filename, start_line, end_line));
					}
				}
			}
		}
	}

	// Remove duplicates while preserving order
	let mut unique_contexts = Vec::new();
	for context in contexts {
		if !unique_contexts.contains(&context) {
			unique_contexts.push(context);
		}
	}

	// Limit to maximum 10 file contexts for performance
	unique_contexts.truncate(10);
	unique_contexts
}

/// Read file content for specified line ranges with 1-indexed line numbers
fn read_file_context(filepath: &str, start_line: usize, end_line: usize) -> Result<String> {
	use std::fs;
	use std::io::{BufRead, BufReader};

	// Validate file exists and is readable
	if !Path::new(filepath).exists() {
		return Ok(format!("// File not found: {}", filepath));
	}

	let file = fs::File::open(filepath)?;
	let reader = BufReader::new(file);
	let mut result = String::new();

	result.push_str(&format!(
		"=== {} (lines {}-{}) ===\n",
		filepath, start_line, end_line
	));

	for (line_num, line_result) in reader.lines().enumerate() {
		let line_number = line_num + 1; // Convert to 1-indexed

		if line_number < start_line {
			continue;
		}

		if line_number > end_line {
			break;
		}

		match line_result {
			Ok(line_content) => {
				result.push_str(&format!("{}: {}\n", line_number, line_content));
			}
			Err(_) => {
				result.push_str(&format!("{}: // Error reading line\n", line_number));
			}
		}
	}

	result.push('\n');
	Ok(result)
}

/// Generate file context content from parsed file requirements
pub fn generate_file_context_content(file_contexts: &[(String, usize, usize)]) -> String {
	if file_contexts.is_empty() {
		return "No specific file context requested.".to_string();
	}

	let mut context_content = String::new();
	context_content.push_str("FILE CONTEXT:\n\n");

	for (filepath, start_line, end_line) in file_contexts {
		match read_file_context(filepath, *start_line, *end_line) {
			Ok(file_content) => {
				context_content.push_str(&file_content);
			}
			Err(e) => {
				context_content.push_str(&format!(
					"=== {} (lines {}-{}) ===\n// Error reading file: {}\n\n",
					filepath, start_line, end_line, e
				));
			}
		}
	}

	context_content
}
#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_parse_file_contexts_code_block() {
		let summary = r#"
## REQUIRED FILE CONTEXTS
List ALL files needed as context to continue work. Use EXACT format:
```
src/main.rs:1:50
src/lib.rs:100:150
config/settings.toml:10:20
```
		"#;

		let contexts = parse_file_contexts(summary);
		assert_eq!(contexts.len(), 3);
		assert_eq!(contexts[0], ("src/main.rs".to_string(), 1, 50));
		assert_eq!(contexts[1], ("src/lib.rs".to_string(), 100, 150));
		assert_eq!(contexts[2], ("config/settings.toml".to_string(), 10, 20));
	}

	#[test]
	fn test_parse_file_contexts_section() {
		let summary = r#"
## REQUIRED FILE CONTEXTS
The following files need context:
- src/session/mod.rs:200:300
- tests/integration.rs:1:100

## NEXT STEPS
Continue with implementation...
		"#;

		let contexts = parse_file_contexts(summary);
		assert_eq!(contexts.len(), 2);
		assert_eq!(contexts[0], ("src/session/mod.rs".to_string(), 200, 300));
		assert_eq!(contexts[1], ("tests/integration.rs".to_string(), 1, 100));
	}

	#[test]
	fn test_parse_file_contexts_fallback() {
		let summary = r#"
We need to look at src/core.rs:50:100 and also check lib/utils.rs:1:25 for the implementation.
		"#;

		let contexts = parse_file_contexts(summary);
		assert_eq!(contexts.len(), 2);
		assert_eq!(contexts[0], ("src/core.rs".to_string(), 50, 100));
		assert_eq!(contexts[1], ("lib/utils.rs".to_string(), 1, 25));
	}

	#[test]
	fn test_parse_file_contexts_mandatory_format() {
		let summary = r#"
## REQUIRED FILE CONTEXTS
CRITICAL: List ALL files needed as context using EXACT format below.

**MANDATORY FORMAT - Use code block with exact pattern:**
```
src/session/chat/session_continuation.rs:100:200
src/config/mod.rs:50:100
tests/integration_test.rs:1:50
```

**PARSING REQUIREMENTS:**
- Each line must be exactly: filepath:number:number
		"#;

		let contexts = parse_file_contexts(summary);
		assert_eq!(contexts.len(), 3);
		assert_eq!(
			contexts[0],
			(
				"src/session/chat/session_continuation.rs".to_string(),
				100,
				200
			)
		);
		assert_eq!(contexts[1], ("src/config/mod.rs".to_string(), 50, 100));
		assert_eq!(
			contexts[2],
			("tests/integration_test.rs".to_string(), 1, 50)
		);
	}

	#[test]
	fn test_parse_file_contexts_invalid_ranges() {
		let summary = r#"
```
src/main.rs:0:50
src/lib.rs:100:50
src/test.rs:1:20000
```
		"#;

		let contexts = parse_file_contexts(summary);
		// Should filter out invalid ranges (start=0, end<start, end>10000)
		assert_eq!(contexts.len(), 0);
	}
}
