// Directory operations module - handling file listing with ripgrep (FIXED VERSION)

use super::super::{get_thread_working_directory, McpToolCall, McpToolResult};
use anyhow::{anyhow, Result};
use serde_json::json;
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
pub async fn list_directory(call: &McpToolCall, directory: &str) -> Result<McpToolResult> {
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

	let mut cmd = Command::new("rg");

	if let Some(depth) = max_depth {
		cmd.arg("--max-depth").arg(depth.to_string());
	}
	if include_hidden {
		cmd.arg("--hidden");
	}

	let has_content = content.as_ref().is_some_and(|c| !c.trim().is_empty());
	let (output_type, is_content_search) = if has_content {
		let content_pattern = content.as_ref().unwrap();
		if line_numbers {
			cmd.arg("--line-number");
		}
		if context_lines > 0 {
			cmd.arg("--context").arg(context_lines.to_string());
		}
		cmd.arg("-F").arg(content_pattern);
		cmd.arg(directory);
		("content search", true)
	} else {
		cmd.arg("--files");
		cmd.arg(directory);
		("file listing", false)
	};

	let working_dir = get_thread_working_directory();
	cmd.current_dir(&working_dir);

	crate::log_debug!(
		"Executing list_directory ({}): rg {:?}",
		output_type,
		cmd.get_args().collect::<Vec<_>>()
	);

	let directory = directory.to_string();
	let output = tokio::task::spawn_blocking(move || match cmd.output() {
		Ok(output) => {
			let stdout = String::from_utf8_lossy(&output.stdout).to_string();
			let stderr = String::from_utf8_lossy(&output.stderr).to_string();

			if is_content_search {
				let lines: Vec<String> = stdout.lines().map(|s| s.to_string()).collect();
				let grouped_output = group_ripgrep_output(&lines);
				let output_str = if stdout.is_empty() && !stderr.is_empty() {
					stderr
				} else {
					grouped_output
				};
				json!({
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
				})
			} else {
				let mut files: Vec<String> = stdout.lines().map(|s| s.to_string()).collect();
				if let Some(ref name_pattern) = pattern {
					let regex_pattern = convert_glob_to_regex(name_pattern);
					if let Ok(regex) = regex::Regex::new(&regex_pattern) {
						files.retain(|file| regex.is_match(file));
					}
				}
				let files_count = files.len();
				let output_str = if stdout.is_empty() && !stderr.is_empty() {
					stderr
				} else {
					files.join("\n")
				};
				json!({
					"success": output.status.success(),
					"output": output_str,
					"files": files,
					"count": files_count,
					"displayed_count": files_count,
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
				})
			}
		}
		Err(e) => json!({
			"success": false,
			"output": format!("Failed to list directory: {}", e),
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
	})
	.await
	.map_err(|e| anyhow!("Failed to execute directory listing: {}", e))?;

	Ok(McpToolResult {
		tool_name: call.tool_name.clone(),
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
			tool_name: "view".to_string(),
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

	#[tokio::test]
	async fn test_list_files_empty_content_should_list_files() {
		// CRITICAL TEST: When content is empty string "", should do file listing, NOT content search
		use crate::mcp::fs::directory::list_directory;
		use std::fs;
		use tempfile::TempDir;

		// Create a temporary directory with test files
		let temp_dir = TempDir::new().unwrap();
		let temp_path = temp_dir.path();

		// Create some test files
		for i in 1..=5 {
			let file_path = temp_path.join(format!("test_file_{}.txt", i));
			fs::write(&file_path, format!("Content of file {}", i)).unwrap();
		}

		// Create a specific file that would match the pattern
		let config_path = temp_path.join("config.json");
		fs::write(&config_path, "{}").unwrap();

		// Test with EMPTY content string - should do file listing, not content search
		let call = McpToolCall {
			tool_name: "view".to_string(),
			parameters: json!({
				"directory": temp_path.to_str().unwrap(),
				"pattern": "*.json",
				"content": ""  // Empty content - should list files, not search
			}),
			tool_id: "test-call-id".to_string(),
		};

		let result = list_directory(
			&call,
			call.parameters
				.get("directory")
				.and_then(|v| v.as_str())
				.unwrap_or("."),
		)
		.await
		.unwrap();
		let output = result.result.as_object().unwrap();

		// Should be file listing (not content search)
		assert_eq!(output["type"], "file listing");
		assert!(output["success"].as_bool().unwrap());

		// Should have files array (not lines)
		assert!(output.contains_key("files"));
		let files = output["files"].as_array().unwrap();

		// Should find the config.json file via pattern matching
		assert_eq!(
			files.len(),
			1,
			"Should find exactly one file matching *.json pattern"
		);
		assert!(files[0].as_str().unwrap().contains("config.json"));
	}

	#[tokio::test]
	async fn test_list_files_no_content_parameter_should_list_files() {
		// CRITICAL TEST: When content parameter is not provided at all, should do file listing
		use crate::mcp::fs::directory::list_directory;
		use std::fs;
		use tempfile::TempDir;

		// Create a temporary directory with test files
		let temp_dir = TempDir::new().unwrap();
		let temp_path = temp_dir.path();

		// Create some test files
		for i in 1..=5 {
			let file_path = temp_path.join(format!("test_file_{}.txt", i));
			fs::write(&file_path, format!("Content of file {}", i)).unwrap();
		}

		// Create a specific file that would match the pattern
		let config_path = temp_path.join("config.json");
		fs::write(&config_path, "{}").unwrap();

		// Test WITHOUT content parameter - should do file listing, not content search
		let call = McpToolCall {
			tool_name: "view".to_string(),
			parameters: json!({
				"directory": temp_path.to_str().unwrap(),
				"pattern": "*.json"
				// No "content" key at all
			}),
			tool_id: "test-call-id".to_string(),
		};

		let result = list_directory(
			&call,
			call.parameters
				.get("directory")
				.and_then(|v| v.as_str())
				.unwrap_or("."),
		)
		.await
		.unwrap();
		let output = result.result.as_object().unwrap();

		// Should be file listing (not content search)
		assert_eq!(output["type"], "file listing");
		assert!(output["success"].as_bool().unwrap());

		// Should have files array (not lines)
		assert!(output.contains_key("files"));
		let files = output["files"].as_array().unwrap();

		// Should find the config.json file via pattern matching
		assert_eq!(
			files.len(),
			1,
			"Should find exactly one file matching *.json pattern"
		);
		assert!(files[0].as_str().unwrap().contains("config.json"));
	}

	#[tokio::test]
	async fn test_list_files_whitespace_content_should_list_files() {
		// CRITICAL TEST: When content is only whitespace, should do file listing, NOT content search
		use crate::mcp::fs::directory::list_directory;
		use std::fs;
		use tempfile::TempDir;

		// Create a temporary directory with test files
		let temp_dir = TempDir::new().unwrap();
		let temp_path = temp_dir.path();

		// Create some test files
		for i in 1..=5 {
			let file_path = temp_path.join(format!("test_file_{}.txt", i));
			fs::write(&file_path, format!("Content of file {}", i)).unwrap();
		}

		// Create a specific file that would match the pattern
		let config_path = temp_path.join("config.json");
		fs::write(&config_path, "{}").unwrap();

		// Test with whitespace-only content - should do file listing, not content search
		let call = McpToolCall {
			tool_name: "view".to_string(),
			parameters: json!({
				"directory": temp_path.to_str().unwrap(),
				"pattern": "*.json",
				"content": "   "  // Whitespace only - should list files, not search
			}),
			tool_id: "test-call-id".to_string(),
		};

		let result = list_directory(
			&call,
			call.parameters
				.get("directory")
				.and_then(|v| v.as_str())
				.unwrap_or("."),
		)
		.await
		.unwrap();
		let output = result.result.as_object().unwrap();

		// Should be file listing (not content search)
		assert_eq!(output["type"], "file listing");
		assert!(output["success"].as_bool().unwrap());

		// Should have files array (not lines)
		assert!(output.contains_key("files"));
		let files = output["files"].as_array().unwrap();

		// Should find the config.json file via pattern matching
		assert_eq!(
			files.len(),
			1,
			"Should find exactly one file matching *.json pattern"
		);
		assert!(files[0].as_str().unwrap().contains("config.json"));
	}
}
