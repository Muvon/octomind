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

// Directory operations module - handling file listing with ripgrep

use super::super::{McpToolCall, McpToolResult};
use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::process::Command;

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
			'.' => {
				// Escape dots for literal match
				regex.push_str("\\.");
			}
			'^' | '$' | '(' | ')' | '[' | ']' | '{' | '}' | '+' | '\\' => {
				// Escape regex special characters
				regex.push('\\');
				regex.push(chars[i]);
			}
			_ => {
				// Regular character
				regex.push(chars[i]);
			}
		}
		i += 1;
	}

	// Add end-of-line anchor to ensure complete filename match
	format!("{}$", regex)
}

// Execute list_files command
pub async fn execute_list_files(call: &McpToolCall) -> Result<McpToolResult> {
	// Extract directory parameter
	let directory = match call.parameters.get("directory") {
		Some(Value::String(dir)) => dir.clone(),
		_ => return Err(anyhow!("Missing or invalid 'directory' parameter")),
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

	// Build the ripgrep command based on the parameters
	let mut cmd_args = Vec::new();

	if let Some(depth) = max_depth {
		cmd_args.push(format!("--max-depth {}", depth));
	}

	// Search for content in files or list files matching pattern
	let (cmd, output_type) = if let Some(ref content_pattern) = content {
		(
			format!(
				"cd '{}' && rg '{}' {}",
				directory,
				content_pattern,
				cmd_args.join(" ")
			),
			"content search",
		)
	} else if let Some(ref name_pattern) = pattern {
		// Convert glob pattern to regex pattern
		let regex_pattern = convert_glob_to_regex(name_pattern);
		(
			format!(
				"cd '{}' && rg --files {} | rg '{}'",
				directory,
				cmd_args.join(" "),
				regex_pattern
			),
			"filename pattern",
		)
	} else {
		// Default: list all files using ripgrep
		(
			format!("cd '{}' && rg --files {}", directory, cmd_args.join(" ")),
			"file listing",
		)
	};

	// Execute the command
	let output = tokio::task::spawn_blocking(move || {
		let output = if cfg!(target_os = "windows") {
			Command::new("cmd").args(["/C", &cmd]).output()
		} else {
			Command::new("sh").args(["-c", &cmd]).output()
		};

		match output {
			Ok(output) => {
				let stdout = String::from_utf8_lossy(&output.stdout).to_string();
				let stderr = String::from_utf8_lossy(&output.stderr).to_string();

				// Parse the output into a list of files
				let files: Vec<&str> = stdout.lines().collect();
				let output_str = if stdout.is_empty() && !stderr.is_empty() {
					stderr
				} else {
					stdout.clone()
				};

				json!({
						"success": output.status.success(),
						"output": output_str,
						"files": files,
						"count": files.len(),
						"type": output_type,
						"parameters": {
						"directory": directory,
						"pattern": pattern,
						"content": content,
						"max_depth": max_depth
					}
				})
			}
			Err(e) => json!({
					"success": false,
					"output": format!("Failed to list files: {}", e),
					"files": [],
					"count": 0,
					"parameters": {
					"directory": directory,
					"pattern": pattern,
					"content": content,
					"max_depth": max_depth
				}
			}),
		}
	})
	.await?;

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
	fn test_glob_to_regex_conversion() {
		// Test single pattern
		assert_eq!(convert_glob_to_regex("*.rs"), ".*?\\.rs$");
		assert_eq!(convert_glob_to_regex("*.py"), ".*?\\.py$");

		// Test multiple patterns (the problematic case)
		assert_eq!(
			convert_glob_to_regex("*.rs|*.py|*.js|*.ts"),
			"(.*?\\.rs$|.*?\\.py$|.*?\\.js$|.*?\\.ts$)"
		);

		// Test pattern with directory
		assert_eq!(convert_glob_to_regex("src/*.rs"), "src/.*?\\.rs$");

		// Test pattern with question mark
		assert_eq!(convert_glob_to_regex("test?.py"), "test.\\.py$");
	}

	#[test]
	fn test_single_glob_to_regex() {
		assert_eq!(convert_single_glob_to_regex("*.rs"), ".*?\\.rs$");
		assert_eq!(convert_single_glob_to_regex("test?.py"), "test.\\.py$");
		assert_eq!(convert_single_glob_to_regex("file.txt"), "file\\.txt$");
		assert_eq!(convert_single_glob_to_regex("*.c"), ".*?\\.c$");
	}
}
