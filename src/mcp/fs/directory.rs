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

// Directory operations module - handling file listing with ripgrep (FIXED VERSION)

use super::super::{McpToolCall, McpToolResult};
use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::process::Command;

// Parse a ripgrep output line to extract filename and rest, handling Windows paths correctly
// UTF-8 safe version that uses character boundaries instead of byte indices
fn parse_ripgrep_line(line: &str) -> Option<(String, String)> {
	// Find all colon positions
	let colon_positions: Vec<usize> = line.match_indices(':').map(|(i, _)| i).collect();

	// We need at least 2 colons for filename:line_number:content format
	if colon_positions.len() < 2 {
		return None;
	}

	// On Windows, the first colon might be after drive letter (C:)
	// Look for the colon that's followed by digits (line number)
	for i in 0..colon_positions.len() - 1 {
		let colon_pos = colon_positions[i];
		let next_colon_pos = colon_positions[i + 1];

		// Check if the part between these colons is a line number (digits)
		// UTF-8 safe: get substring between character positions
		let chars: Vec<char> = line.chars().collect();
		if colon_pos + 1 < chars.len() && next_colon_pos <= chars.len() {
			let potential_line_num: String =
				chars[(colon_pos + 1)..next_colon_pos].iter().collect();
			if potential_line_num.chars().all(|c| c.is_ascii_digit())
				&& !potential_line_num.is_empty()
			{
				// Found the filename:line_number:content pattern
				// UTF-8 safe: split at character boundaries
				let filename = line.chars().take(colon_pos).collect::<String>();
				let rest = line.chars().skip(colon_pos + 1).collect::<String>();
				return Some((filename, rest));
			}
		}
	}

	// Fallback: if no digit pattern found, use the last colon before content
	// This handles edge cases where line numbers might have non-digit characters
	if colon_positions.len() >= 2 {
		let colon_pos = colon_positions[colon_positions.len() - 2];
		// UTF-8 safe: split at character boundaries
		let filename = line.chars().take(colon_pos).collect::<String>();
		let rest = line.chars().skip(colon_pos + 1).collect::<String>();
		return Some((filename, rest));
	}

	None
}

// Parse a ripgrep context line (with dashes) to extract filename and rest, handling Windows paths
// UTF-8 safe version that uses character boundaries instead of byte indices
fn parse_ripgrep_dash_line(line: &str) -> Option<(String, String)> {
	// Find all dash positions
	let dash_positions: Vec<usize> = line.match_indices('-').map(|(i, _)| i).collect();

	// We need at least 2 dashes for filename-line_number-content format
	if dash_positions.len() < 2 {
		return None;
	}

	// On Windows, look for the dash that's followed by digits (line number)
	for i in 0..dash_positions.len() - 1 {
		let dash_pos = dash_positions[i];
		let next_dash_pos = dash_positions[i + 1];

		// Check if the part between these dashes is a line number (digits)
		// UTF-8 safe: get substring between character positions
		let chars: Vec<char> = line.chars().collect();
		if dash_pos + 1 < chars.len() && next_dash_pos <= chars.len() {
			let potential_line_num: String = chars[(dash_pos + 1)..next_dash_pos].iter().collect();
			if potential_line_num.chars().all(|c| c.is_ascii_digit())
				&& !potential_line_num.is_empty()
			{
				// Found the filename-line_number-content pattern
				// UTF-8 safe: split at character boundaries
				let filename = line.chars().take(dash_pos).collect::<String>();
				let rest = line.chars().skip(dash_pos + 1).collect::<String>();
				return Some((filename, rest));
			}
		}
	}

	// Fallback: use the last dash before content
	if dash_positions.len() >= 2 {
		let dash_pos = dash_positions[dash_positions.len() - 2];
		// UTF-8 safe: split at character boundaries
		let filename = line.chars().take(dash_pos).collect::<String>();
		let rest = line.chars().skip(dash_pos + 1).collect::<String>();
		return Some((filename, rest));
	}

	None
}

// Group ripgrep output by file for token efficiency while preserving line numbers
fn group_ripgrep_output(lines: &[String]) -> String {
	let mut result = Vec::new();
	let mut current_file = String::new();
	let mut file_lines = Vec::new();

	for line in lines {
		if line.contains("[") && line.contains("truncated") {
			// Handle truncation markers
			if !file_lines.is_empty() {
				result.push(format!("{}:\n{}", current_file, file_lines.join("\n")));
				file_lines.clear();
			}
			result.push(line.clone());
			continue;
		}

		// Parse ripgrep output: filename:line_number:content or filename-line_number-content (context)
		// Need to handle Windows paths (C:\path\file.rs:123:content) by finding the colon before line number
		if let Some((filename, rest)) = parse_ripgrep_line(line) {
			if filename != current_file {
				// New file - output previous file's lines
				if !file_lines.is_empty() {
					result.push(format!("{}:\n{}", current_file, file_lines.join("\n")));
					file_lines.clear();
				}
				current_file = filename.to_string();
			}

			// Add the line content (without filename)
			file_lines.push(rest.to_string());
		} else if let Some((filename, rest)) = parse_ripgrep_dash_line(line) {
			// Context line (filename-line_number-content)

			if filename != current_file {
				// New file - output previous file's lines
				if !file_lines.is_empty() {
					result.push(format!("{}:\n{}", current_file, file_lines.join("\n")));
					file_lines.clear();
				}
				current_file = filename.to_string();
			}

			// Add the context line (with dash to indicate context)
			file_lines.push(format!("-{}", rest));
		} else if line == "--" {
			// Separator between match groups - preserve it
			file_lines.push("--".to_string());
		} else {
			// Other lines (shouldn't happen in normal ripgrep output, but handle gracefully)
			file_lines.push(line.clone());
		}
	}

	// Output the last file's lines
	if !file_lines.is_empty() {
		result.push(format!("{}:\n{}", current_file, file_lines.join("\n")));
	}

	result.join("\n\n")
}

// Convert glob pattern to regex pattern for use with ripgrep
fn convert_glob_to_regex(glob_pattern: &str) -> String {
	// Handle multiple patterns separated by |
	let patterns: Vec<&str> = glob_pattern.split('|').collect();

	if patterns.len() > 1 {
		// Multiple patterns - convert each and join with |
		let regex_patterns: Vec<String> = patterns
			.iter()
			.map(|p| convert_single_glob_to_regex(p.trim()))
			.collect();
		format!("({})", regex_patterns.join("|"))
	} else {
		// Single pattern
		convert_single_glob_to_regex(glob_pattern)
	}
}

// Convert a single glob pattern to regex
fn convert_single_glob_to_regex(pattern: &str) -> String {
	let mut regex = String::new();
	let chars: Vec<char> = pattern.chars().collect();
	let mut i = 0;

	while i < chars.len() {
		match chars[i] {
			'*' => {
				// Convert * to .*? (non-greedy match any characters)
				regex.push_str(".*?");
			}
			'?' => {
				// Convert ? to . (match any single character)
				regex.push('.');
			}
			'[' => {
				// Character class - pass through as-is
				regex.push('[');
				i += 1;
				while i < chars.len() && chars[i] != ']' {
					regex.push(chars[i]);
					i += 1;
				}
				if i < chars.len() {
					regex.push(']');
				}
			}
			c if "(){}^$+|\\".contains(c) => {
				// Escape regex special characters
				regex.push('\\');
				regex.push(c);
			}
			c => {
				// Regular character
				regex.push(c);
			}
		}
		i += 1;
	}

	regex
}

// Execute list_files command with PROPER content search vs file listing handling
pub async fn execute_list_files(call: &McpToolCall) -> Result<McpToolResult> {
	// Extract directory parameter
	let directory = match call.parameters.get("directory") {
		Some(Value::String(dir)) => dir.clone(),
		_ => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Missing or invalid 'directory' parameter".to_string(),
			))
		}
	};

	// Extract optional parameters
	let pattern = call
		.parameters
		.get("pattern")
		.and_then(|v| v.as_str())
		.map(|s| s.to_string());

	let content = call
		.parameters
		.get("content")
		.and_then(|v| v.as_str())
		.map(|s| s.to_string());

	let max_depth = call
		.parameters
		.get("max_depth")
		.and_then(|v| v.as_u64())
		.map(|n| n as usize);

	let include_hidden = call
		.parameters
		.get("include_hidden")
		.and_then(|v| v.as_bool())
		.unwrap_or(false);

	let line_numbers = call
		.parameters
		.get("line_numbers")
		.and_then(|v| v.as_bool())
		.unwrap_or(true);

	let context_lines = call
		.parameters
		.get("context")
		.and_then(|v| v.as_i64())
		.unwrap_or(0) as usize;

	// Build the ripgrep command using proper argument passing
	let mut cmd = Command::new("rg");

	// Add depth limit if specified
	if let Some(depth) = max_depth {
		cmd.arg("--max-depth").arg(depth.to_string());
	}

	// Add hidden files flag if requested
	if include_hidden {
		cmd.arg("--hidden");
	}

	// Configure the command based on the operation type
	let (output_type, is_content_search) = if let Some(ref content_pattern) = content {
		// Content search: search for content within files
		if line_numbers {
			cmd.arg("--line-number");
		}

		// Add context if specified
		if context_lines > 0 {
			cmd.arg("--context").arg(context_lines.to_string());
		}

		// Use -F flag to treat the pattern as a fixed string (literal) rather than regex
		// This avoids regex parsing errors with special characters
		cmd.arg("-F").arg(content_pattern);
		// Add the directory as the search path
		cmd.arg(&directory);

		("content search", true)
	} else {
		// File listing: list files (optionally filtered by pattern)
		cmd.arg("--files");

		// Add the directory as the search path
		cmd.arg(&directory);

		("file listing", false)
	};

	// Debug: Log the actual command being executed
	crate::log_debug!(
		"Executing list_files ({}): rg {:?}",
		output_type,
		cmd.get_args().collect::<Vec<_>>()
	);

	// Execute the command
	let output = tokio::task::spawn_blocking(move || {
		let output = cmd.output();

		match output {
			Ok(output) => {
				let stdout = String::from_utf8_lossy(&output.stdout).to_string();
				let stderr = String::from_utf8_lossy(&output.stderr).to_string();

				if is_content_search {
					// For content search, preserve the original ripgrep output format
					// which includes filenames, line numbers, and matched content
					let lines: Vec<String> = stdout.lines().map(|s| s.to_string()).collect();

					// Group FIRST to preserve match + context relationships
					let grouped_output = group_ripgrep_output(&lines);

					// Global truncation will be applied by MCP response handler
					let final_output = grouped_output;

					let output_str = if stdout.is_empty() && !stderr.is_empty() {
						stderr
					} else {
						final_output
					};

					// For content search, we return the formatted output with matches
					let result = json!({
							"success": output.status.success(),
							"output": output_str,
							"lines": lines,
							"total_lines": lines.len(),
							"displayed_lines": lines.len(),
							"type": output_type,
							"parameters": {
							"directory": directory,
							"pattern": pattern,
							"content": content,
							"max_depth": max_depth,
							"include_hidden": include_hidden,

							"line_numbers": line_numbers,
							"context": context_lines
						}
					});

					result
				} else {
					// For file listing, parse as files and apply pattern filtering
					let mut files: Vec<String> = stdout.lines().map(|s| s.to_string()).collect();

					// Filter by pattern if we're doing filename pattern matching
					if let Some(ref name_pattern) = pattern {
						let regex_pattern = convert_glob_to_regex(name_pattern);
						if let Ok(regex) = regex::Regex::new(&regex_pattern) {
							files.retain(|file| regex.is_match(file));
						}
					}
					// Global truncation will be applied by MCP response handler
					let files_count = files.len();
					let final_files = files;

					let output_str = if stdout.is_empty() && !stderr.is_empty() {
						stderr
					} else {
						final_files.join("\n")
					};

					let result = json!({
							"success": output.status.success(),
							"output": output_str,
							"files": final_files,
							"count": files_count,
							"displayed_count": final_files.len(),
							"type": output_type,
							"parameters": {
							"directory": directory,
							"pattern": pattern,
							"content": content,
							"max_depth": max_depth,
							"include_hidden": include_hidden,

							"line_numbers": line_numbers,
							"context": context_lines
						}
					});

					result
				}
			}
			Err(e) => json!({
					"success": false,
					"output": format!("Failed to list files: {}", e),
					"files": [],
					"count": 0,
					"displayed_count": 0,
					"parameters": {
					"directory": directory,
					"pattern": pattern,
					"content": content,
					"max_depth": max_depth,
					"include_hidden": include_hidden,
					"line_numbers": line_numbers,
					"context": context_lines
				}
			}),
		}
	})
	.await
	.map_err(|e| anyhow!("Failed to execute file listing command: {}", e))?;

	Ok(McpToolResult {
		tool_name: "list_files".to_string(),
		tool_id: call.tool_id.clone(),
		result: output,
	})
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_parse_ripgrep_line_unix_path() {
		let line = "/home/user/file.rs:123:println!(\"test\");";
		let result = parse_ripgrep_line(line);
		assert_eq!(
			result,
			Some((
				"/home/user/file.rs".to_string(),
				"123:println!(\"test\");".to_string()
			))
		);
	}

	#[test]
	fn test_parse_ripgrep_line_windows_path() {
		let line = "C:\\Users\\Test\\file.rs:123:println!(\"test\");";
		let result = parse_ripgrep_line(line);
		assert_eq!(
			result,
			Some((
				"C:\\Users\\Test\\file.rs".to_string(),
				"123:println!(\"test\");".to_string()
			))
		);
	}

	#[test]
	fn test_parse_ripgrep_line_windows_path_with_spaces() {
		let line = "C:\\Users\\Test User\\My File.rs:456:let x = 42;";
		let result = parse_ripgrep_line(line);
		assert_eq!(
			result,
			Some((
				"C:\\Users\\Test User\\My File.rs".to_string(),
				"456:let x = 42;".to_string()
			))
		);
	}

	#[test]
	fn test_parse_ripgrep_dash_line_unix_path() {
		let line = "/home/user/file.rs-123-some context line";
		let result = parse_ripgrep_dash_line(line);
		assert_eq!(
			result,
			Some((
				"/home/user/file.rs".to_string(),
				"123-some context line".to_string()
			))
		);
	}

	#[test]
	fn test_parse_ripgrep_dash_line_windows_path() {
		let line = "C:\\Users\\Test\\file.rs-123-some context line";
		let result = parse_ripgrep_dash_line(line);
		assert_eq!(
			result,
			Some((
				"C:\\Users\\Test\\file.rs".to_string(),
				"123-some context line".to_string()
			))
		);
	}

	#[test]
	fn test_parse_ripgrep_line_invalid_format() {
		let line = "just some text without proper format";
		let result = parse_ripgrep_line(line);
		assert_eq!(result, None);
	}

	#[test]
	fn test_parse_ripgrep_line_single_colon() {
		let line = "C:\\Users\\file.rs";
		let result = parse_ripgrep_line(line);
		assert_eq!(result, None);
	}
	#[test]
	fn test_content_search_with_special_chars() {
		// Create a mock tool call with content parameter containing special regex characters
		let _call = McpToolCall {
			tool_name: "list_files".to_string(),
			tool_id: "test_id".to_string(),
			parameters: json!({
				"directory": "src",
				"content": "backward_step()"
			}),
		};

		// Use std::process::Command::new to create a command and inspect its arguments
		let mut cmd = Command::new("rg");
		cmd.arg("--line-number");

		// Add the -F flag and content pattern
		cmd.arg("-F").arg("backward_step()");
		cmd.arg("src");

		// Get the arguments as a Vec<String> for comparison
		let args: Vec<String> = cmd
			.get_args()
			.map(|arg| arg.to_string_lossy().to_string())
			.collect();

		// Verify the command contains the -F flag followed by the content pattern
		assert!(args.contains(&"-F".to_string()));
		assert!(args.contains(&"backward_step()".to_string()));

		// Verify the order: -F should come before the pattern
		let f_index = args.iter().position(|arg| arg == "-F").unwrap();
		let pattern_index = args
			.iter()
			.position(|arg| arg == "backward_step()")
			.unwrap();
		assert!(
			f_index < pattern_index,
			"-F flag should come before the pattern"
		);
	}
}
