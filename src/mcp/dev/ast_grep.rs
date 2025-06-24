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

// AST-grep execution functionality for the Developer MCP provider

use super::super::{McpFunction, McpToolCall, McpToolResult};
use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::path::Path;

// Maximum number of files allowed after glob expansion to prevent command line overflow
const MAX_EXPANDED_FILES: usize = 1000;

// Group ast-grep output by file for token efficiency while preserving line numbers
fn group_ast_grep_output(output: &str) -> String {
	let lines: Vec<&str> = output.lines().collect();
	let mut result = Vec::new();
	let mut current_file = String::new();
	let mut file_lines = Vec::new();

	for line in lines {
		// ast-grep output format: filename:line_number:column:content
		if let Some(colon_pos) = line.find(':') {
			let filename = &line[..colon_pos];
			let rest = &line[colon_pos + 1..];

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
		} else {
			// Non-matching lines (errors, etc.) - keep as-is
			if !file_lines.is_empty() {
				result.push(format!("{}:\n{}", current_file, file_lines.join("\n")));
				file_lines.clear();
				current_file.clear();
			}
			result.push(line.to_string());
		}
	}

	// Output the last file's lines
	if !file_lines.is_empty() {
		result.push(format!("{}:\n{}", current_file, file_lines.join("\n")));
	}

	if result.is_empty() {
		output.to_string()
	} else {
		result.join("\n\n")
	}
}
// Expand glob patterns to actual file paths
fn expand_glob_patterns(paths: &[String]) -> Result<Vec<String>> {
	let mut expanded_paths = Vec::new();

	crate::log_debug!("Expanding {} glob patterns: {:?}", paths.len(), paths);

	for path in paths {
		// Check if this looks like a glob pattern
		if path.contains('*') || path.contains('?') || path.contains('[') {
			// Use glob to expand the pattern
			match glob::glob(path) {
				Ok(entries) => {
					let mut found_files = false;
					let mut pattern_file_count = 0;
					for entry in entries {
						match entry {
							Ok(path_buf) => {
								if path_buf.is_file() {
									expanded_paths.push(path_buf.to_string_lossy().to_string());
									found_files = true;
									pattern_file_count += 1;
								}
							}
							Err(e) => {
								crate::log_debug!("Glob pattern '{}' entry error: {}", path, e);
							}
						}
					}
					// Log results for this pattern
					if found_files {
						crate::log_debug!(
							"Glob pattern '{}' matched {} files",
							path,
							pattern_file_count
						);
					} else {
						crate::log_debug!("Glob pattern '{}' matched no files", path);
					}
				}
				Err(e) => {
					return Err(anyhow!("Invalid glob pattern '{}': {}", path, e));
				}
			}
		} else {
			// Not a glob pattern, add as-is if it exists or is a directory
			let path_obj = Path::new(path);
			if path_obj.exists() {
				expanded_paths.push(path.clone());
				crate::log_debug!("Added explicit path: {}", path);
			} else {
				crate::log_debug!("Path '{}' does not exist, skipping", path);
			}
		}
	}

	// Deduplicate files in case multiple patterns match the same file
	expanded_paths.sort();
	expanded_paths.dedup();

	crate::log_debug!(
		"Total expanded files after deduplication: {}",
		expanded_paths.len()
	);

	// Check if we have too many files
	if expanded_paths.len() > MAX_EXPANDED_FILES {
		return Err(anyhow!(
			"Too many files expanded from glob patterns: {} files (max allowed: {}). Consider using more specific patterns to reduce the file count.",
			expanded_paths.len(),
			MAX_EXPANDED_FILES
		));
	}

	Ok(expanded_paths)
}

// Define the ast_grep function for the MCP protocol with enhanced description
pub fn get_ast_grep_function() -> McpFunction {
	McpFunction {
		name: "ast_grep".to_string(),
		description: "Search and refactor code using AST patterns with ast-grep (sg).

This tool uses ast-grep for efficient and semantic code search and transformation using AST patterns.
AST-grep understands code structure, making it superior to regex for code transformations.

Parameters:
- `pattern`: The AST pattern to search for (required)
- `paths`: Optional array of file paths or glob patterns to search within (default: current directory)
- `language`: Optional language of the code (e.g., 'rust', 'javascript', 'python', 'typescript', 'go', 'java', 'c', 'cpp', 'php')
- `rewrite`: Optional rewrite pattern to apply for refactoring transformations
- `json_output`: Optional boolean to get output in JSON format (default: false)
- `context`: Optional number of lines of context to show around matches (default: 0)
- `max_lines`: Maximum lines to return (default: 20, set to 0 for unlimited)
- `update_all`: Optional boolean to apply rewrites to all matches without confirmation (default: false)

Pattern Syntax:
- Use metavariables like $NAME, $ARGS, $BODY for flexible matching
- Use $$$ for matching any number of statements/expressions
- Patterns match AST structure, not text

Common Examples by Language:

**JavaScript/TypeScript:**
- Function calls: `console.log($$$)` or `$OBJ.$METHOD($$$)`
- Function definitions: `function $NAME($ARGS) { $$$ }`
- Arrow functions: `($ARGS) => $BODY`
- Variable declarations: `const $VAR = $VALUE`
- Import statements: `import $NAME from '$PATH'`

**PHP:**
- Function calls: `$NAME($$$)`
- Method calls: `$OBJ->$METHOD($$$)`
- Class definitions: `class $NAME { $$$ }`
- Variable assignments: `$$VAR = $VALUE`

**Rust:**
- Function calls: `println!($$$)` or `$NAME($$$)`
- Function definitions: `fn $NAME($ARGS) { $$$ }`
- Struct definitions: `struct $NAME { $$$ }`
- Use statements: `use $PATH;`

**Python:**
- Function calls: `print($$$)` or `$OBJ.$METHOD($$$)`
- Function definitions: `def $NAME($ARGS): $$$`
- Class definitions: `class $NAME: $$$`
- Import statements: `import $NAME`

Rewrite Examples:
- Rename functions: pattern `old_func($ARGS)` → rewrite `new_func($ARGS)`
- Add visibility: pattern `fn $NAME($ARGS)` → rewrite `pub fn $NAME($ARGS)`
- Modernize JS: pattern `var $NAME = $VALUE` → rewrite `const $NAME = $VALUE`
- Update method calls: pattern `$OBJ.oldMethod($ARGS)` → rewrite `$OBJ.newMethod($ARGS)`

Usage Examples:
- Find console logs: `{\"pattern\": \"console.log($$$)\", \"language\": \"javascript\"}`
- Rename function: `{\"pattern\": \"oldFunc($ARGS)\", \"rewrite\": \"newFunc($ARGS)\", \"language\": \"javascript\"}`
- Find PHP classes: `{\"pattern\": \"class $NAME\", \"language\": \"php\", \"paths\": [\"src/**/*.php\"]}`
- Search with context: `{\"pattern\": \"TODO\", \"context\": 2}`
".to_string(),
		parameters: json!({
			"type": "object",
			"required": ["pattern"],
			"properties": {
				"pattern": {
					"type": "string",
					"description": "The AST pattern to search for using ast-grep syntax"
				},
				"paths": {
					"type": "array",
					"items": {"type": "string"},
					"description": "Optional array of file paths or glob patterns to search within (default: current directory)"
				},
				"language": {
					"type": "string",
					"description": "Optional language of the code (e.g., 'rust', 'javascript', 'python', 'typescript', 'go', 'java', 'c', 'cpp', 'php')"
				},
				"rewrite": {
					"type": "string",
					"description": "Optional rewrite pattern to apply for refactoring transformations"
				},
				"json_output": {
					"type": "boolean",
					"default": false,
					"description": "Optional boolean to get output in JSON format (default: false)"
				},
				"context": {
					"type": "integer",
					"default": 0,
					"description": "Optional number of lines of context to show around matches (default: 0)"
				},
				"update_all": {
					"type": "boolean",
					"default": false,
					"description": "Optional boolean to apply rewrites to all matches without confirmation (default: false)"
				},
				"max_lines": {
					"type": "integer",
					"default": 20,
					"description": "Maximum lines to return (default: 20, set to 0 for unlimited)"
				}
			}
		}),
	}
}

// Execute an ast-grep command
pub async fn execute_ast_grep_command(
	call: &McpToolCall,
	cancellation_token: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> Result<McpToolResult> {
	use std::sync::atomic::Ordering;
	use tokio::process::Command as TokioCommand;

	// Extract pattern parameter (required)
	let pattern = match call.parameters.get("pattern") {
		Some(Value::String(p)) => {
			if p.trim().is_empty() {
				return Ok(McpToolResult::error(
					call.tool_name.clone(),
					call.tool_id.clone(),
					"Pattern parameter cannot be empty".to_string(),
				));
			}
			p.clone()
		}
		Some(_) => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Pattern parameter must be a string".to_string(),
			));
		}
		None => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Missing required 'pattern' parameter".to_string(),
			));
		}
	};

	// Extract optional parameters
	let paths = call
		.parameters
		.get("paths")
		.and_then(|v| v.as_array())
		.map(|arr| {
			arr.iter()
				.filter_map(|item| item.as_str().map(|s| s.to_string()))
				.collect::<Vec<String>>()
		});

	let language = call
		.parameters
		.get("language")
		.and_then(|v| v.as_str())
		.map(|s| s.to_string());

	let rewrite = call
		.parameters
		.get("rewrite")
		.and_then(|v| v.as_str())
		.map(|s| s.to_string());

	let json_output = call
		.parameters
		.get("json_output")
		.and_then(|v| v.as_bool())
		.unwrap_or(false);

	let context = call
		.parameters
		.get("context")
		.and_then(|v| v.as_i64())
		.unwrap_or(0);

	let update_all = call
		.parameters
		.get("update_all")
		.and_then(|v| v.as_bool())
		.unwrap_or(false);

	let max_lines = call
		.parameters
		.get("max_lines")
		.and_then(|v| v.as_i64())
		.unwrap_or(20) as usize;

	// Check for cancellation before starting
	if let Some(ref token) = cancellation_token {
		if token.load(Ordering::SeqCst) {
			return Err(anyhow!("AST-grep command execution cancelled"));
		}
	}

	// Build the ast-grep command using proper argument passing
	let mut cmd = TokioCommand::new("sg");

	// Add pattern
	cmd.arg("-p");
	cmd.arg(&pattern);

	// Add language if specified
	if let Some(lang) = &language {
		cmd.arg("-l");
		cmd.arg(lang);
	}

	// Add rewrite if specified
	if let Some(rewrite_pattern) = &rewrite {
		cmd.arg("--rewrite");
		cmd.arg(rewrite_pattern);

		// Add update-all flag if specified for rewrite operations
		if update_all {
			cmd.arg("--update-all");
		}
	}

	// Add JSON output if requested
	if json_output {
		cmd.arg("--json");
	}

	// Add context if specified
	if context > 0 {
		cmd.arg("-A");
		cmd.arg(context.to_string());
		cmd.arg("-B");
		cmd.arg(context.to_string());
	}

	// Expand glob patterns to actual file paths first
	let expanded_paths_result = if let Some(file_paths) = &paths {
		expand_glob_patterns(file_paths)
	} else {
		Ok(vec![])
	};

	// Add paths if specified, otherwise default to current directory
	let actual_file_paths = match expanded_paths_result {
		Ok(expanded_paths) => {
			if expanded_paths.is_empty() && paths.is_some() {
				// If no files found after expansion, fall back to current directory
				crate::log_debug!("No files found after glob expansion, using current directory");
				cmd.arg(".");
				vec![".".to_string()]
			} else if expanded_paths.is_empty() {
				// No paths specified at all, use current directory
				cmd.arg(".");
				vec![".".to_string()]
			} else {
				// Add all expanded paths
				for path in &expanded_paths {
					cmd.arg(path);
				}
				expanded_paths
			}
		}
		Err(e) => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				format!("Failed to expand glob patterns: {}", e),
			));
		}
	};

	// Configure the command
	cmd.stdout(std::process::Stdio::piped())
		.stderr(std::process::Stdio::piped())
		.stdin(std::process::Stdio::null())
		.kill_on_drop(true); // CRITICAL: Kill process when dropped

	// Debug: Log the complete command being executed with all arguments
	let mut debug_args = vec!["-p".to_string(), pattern.clone()];
	if let Some(lang) = &language {
		debug_args.push("-l".to_string());
		debug_args.push(lang.clone());
	}
	if let Some(rewrite_pattern) = &rewrite {
		debug_args.push("--rewrite".to_string());
		debug_args.push(rewrite_pattern.clone());
		if update_all {
			debug_args.push("--update-all".to_string());
		}
	}
	if json_output {
		debug_args.push("--json".to_string());
	}
	if context > 0 {
		debug_args.push("-A".to_string());
		debug_args.push(context.to_string());
		debug_args.push("-B".to_string());
		debug_args.push(context.to_string());
	}

	// Add file arguments to debug info (show first few and count if many)
	let file_count = actual_file_paths.len();
	if file_count <= 5 {
		for path in &actual_file_paths {
			debug_args.push(path.clone());
		}
	} else {
		for path in actual_file_paths.iter().take(3) {
			debug_args.push(path.clone());
		}
		debug_args.push(format!("... and {} more files", file_count - 3));
	}

	crate::log_debug!(
		"Executing ast-grep command: sg {:?} (targeting {} files)",
		debug_args,
		file_count
	);

	// Spawn the process
	let child = cmd
		.spawn()
		.map_err(|e| anyhow!("Failed to spawn ast-grep command: {}", e))?;

	// Get the process ID for potential killing
	let child_id = child.id();

	// Create a cancellation future
	let cancellation_future = async {
		if let Some(ref token) = cancellation_token {
			loop {
				tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
				if token.load(Ordering::SeqCst) {
					return true; // Indicate cancellation occurred
				}
			}
		} else {
			std::future::pending::<bool>().await
		}
	};

	// Race between command completion and cancellation
	let output = tokio::select! {
		result = child.wait_with_output() => {
			match result.map_err(|e| anyhow!("AST-grep command execution failed: {}", e)) {
				Ok(output) => {
					let stdout = String::from_utf8_lossy(&output.stdout).to_string();
					let stderr = String::from_utf8_lossy(&output.stderr).to_string();

					// Group FIRST to preserve file-based organization
					let grouped_output = group_ast_grep_output(&stdout);

					// Then apply truncation to the grouped output
					let output_lines: Vec<String> = grouped_output.lines().map(|s| s.to_string()).collect();
					let (truncated_lines, truncation_info) = crate::mcp::shared_utils::apply_head_truncation(
						&output_lines,
						max_lines
					);

					// Format the final output
					let combined = if stderr.is_empty() {
						truncated_lines.join("\n")
					} else if truncated_lines.is_empty() {
						stderr
					} else {
						format!("{}\n\nError: {}", truncated_lines.join("\n"), stderr)
					};

					// Add detailed execution results including status code
					let status_code = output.status.code().unwrap_or(-1);
					let success = output.status.success();

					// For rewrite operations, provide additional context
					let operation_type = if rewrite.is_some() {
						"rewrite"
					} else {
						"search"
					};

					let mut result = json!({
						"success": success,
						"output": combined,
						"code": status_code,
						"operation": operation_type,
						"parameters": {
							"pattern": pattern,
							"paths": paths,
							"language": language,
							"rewrite": rewrite,
							"json_output": json_output,
							"context": context,
							"update_all": update_all,
							"max_lines": max_lines
						},
						"message": if success {
							format!("AST-grep {} executed successfully with exit code {}", operation_type, status_code)
						} else {
							format!("AST-grep {} failed with exit code {}", operation_type, status_code)
						}
					});

					// Add truncation info if present
					if let Some(info) = truncation_info {
						result["truncation_info"] = json!(info);
					}

					result
				}
				Err(e) => json!({
					"success": false,
					"output": format!("Failed to execute ast-grep command: {}", e),
					"code": -1,
					"operation": if rewrite.is_some() { "rewrite" } else { "search" },
					"parameters": {
						"pattern": pattern,
						"paths": paths,
						"language": language,
						"rewrite": rewrite,
						"json_output": json_output,
						"context": context,
						"update_all": update_all,
						"max_lines": max_lines
					},
					"message": format!("Failed to execute ast-grep command: {}", e)
				}),
			}
		}
		cancelled = cancellation_future => {
			if cancelled {
				// Try to kill the process using system commands if we have the PID
				if let Some(pid) = child_id {
					#[cfg(unix)]
					{
						// On Unix systems, try to kill the process using system commands
						let _ = std::process::Command::new("kill")
							.args(["-TERM", &pid.to_string()])
							.output();
						// Give it a moment to terminate gracefully
						std::thread::sleep(std::time::Duration::from_millis(100));
						let _ = std::process::Command::new("kill")
							.args(["-KILL", &pid.to_string()])
							.output();
					}
					#[cfg(windows)]
					{
						// On Windows, use taskkill
						let _ = std::process::Command::new("taskkill")
							.args(["/F", "/PID", &pid.to_string()])
							.output();
					}
				}

				json!({
					"success": false,
					"output": "AST-grep command execution cancelled by user (Ctrl+C)",
					"code": -1,
					"operation": if rewrite.is_some() { "rewrite" } else { "search" },
					"parameters": {
						"pattern": pattern,
						"paths": paths,
						"language": language,
						"rewrite": rewrite,
						"json_output": json_output,
						"context": context,
						"update_all": update_all,
						"max_lines": max_lines
					},
					"message": "AST-grep command execution cancelled by user"
				})
			} else {
				// This shouldn't happen, but handle it gracefully
				json!({
					"success": false,
					"output": "Unexpected cancellation state",
					"code": -1,
					"operation": if rewrite.is_some() { "rewrite" } else { "search" },
					"parameters": {
						"pattern": pattern,
						"paths": paths,
						"language": language,
						"rewrite": rewrite,
						"json_output": json_output,
						"context": context,
						"update_all": update_all,
						"max_lines": max_lines
					},
					"message": "Unexpected cancellation state"
				})
			}
		}
	};

	Ok(McpToolResult {
		tool_name: "ast_grep".to_string(),
		tool_id: call.tool_id.clone(),
		result: output,
	})
}
