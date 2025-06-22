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
		Some(Value::String(p)) => p.clone(),
		_ => return Err(anyhow!("Missing or invalid 'pattern' parameter")),
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

	// Check for cancellation before starting
	if let Some(ref token) = cancellation_token {
		if token.load(Ordering::SeqCst) {
			return Err(anyhow!("AST-grep command execution cancelled"));
		}
	}

	// Build the ast-grep command
	let mut cmd_args = vec!["sg".to_string()];

	// Add pattern
	cmd_args.push("-p".to_string());
	cmd_args.push(pattern.clone());

	// Add language if specified
	if let Some(lang) = &language {
		cmd_args.push("-l".to_string());
		cmd_args.push(lang.clone());
	}

	// Add rewrite if specified
	if let Some(rewrite_pattern) = &rewrite {
		cmd_args.push("--rewrite".to_string());
		cmd_args.push(rewrite_pattern.clone());

		// Add update-all flag if specified for rewrite operations
		if update_all {
			cmd_args.push("--update-all".to_string());
		}
	}

	// Add JSON output if requested
	if json_output {
		cmd_args.push("--json".to_string());
	}

	// Add context if specified
	if context > 0 {
		cmd_args.push("-A".to_string());
		cmd_args.push(context.to_string());
		cmd_args.push("-B".to_string());
		cmd_args.push(context.to_string());
	}

	// Add paths if specified, otherwise default to current directory
	if let Some(file_paths) = &paths {
		cmd_args.extend(file_paths.clone());
	} else {
		cmd_args.push(".".to_string());
	}

	// Join command for execution
	let cmd_str = cmd_args.join(" ");

	// Use tokio::process::Command for better cancellation support
	let mut cmd = if cfg!(target_os = "windows") {
		let mut cmd = TokioCommand::new("cmd");
		cmd.args(["/C", &cmd_str]);
		cmd
	} else {
		let mut cmd = TokioCommand::new("sh");
		cmd.args(["-c", &cmd_str]);
		cmd
	};

	// Configure the command
	cmd.stdout(std::process::Stdio::piped())
		.stderr(std::process::Stdio::piped())
		.stdin(std::process::Stdio::null())
		.kill_on_drop(true); // CRITICAL: Kill process when dropped

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

					// Format the output more clearly with error handling
					let combined = if stderr.is_empty() {
						stdout
					} else if stdout.is_empty() {
						stderr
					} else {
						format!(
							"{}

Error: {}",
							stdout, stderr
						)
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

					json!({
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
							"update_all": update_all
						},
						"message": if success {
							format!("AST-grep {} executed successfully with exit code {}", operation_type, status_code)
						} else {
							format!("AST-grep {} failed with exit code {}", operation_type, status_code)
						}
					})
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
						"update_all": update_all
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
						"update_all": update_all
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
						"update_all": update_all
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
