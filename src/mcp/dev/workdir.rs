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

// Working directory management for parallel execution isolation

use super::super::{
	get_thread_original_working_directory, get_thread_working_directory,
	set_thread_working_directory, McpFunction, McpToolCall, McpToolResult,
};
use anyhow::Result;
use serde_json::{json, Value};

/// Get the working directory function definition
pub fn get_workdir_function() -> McpFunction {
	McpFunction {
		name: "workdir".to_string(),
		description: "Get or set the working directory for file and shell operations.

This tool manages the runtime working directory used by all MCP tools (shell, text_editor, etc.).

Parameters:
- `path`: Optional path to set as new working directory. If not provided, returns current working directory.
- `reset`: If true, resets to the original project directory (ignores `path` parameter).

Use Cases:
- Parallel execution: Each actor can work in its own isolated git worktree
- Testing: Switch to a test directory before running tests
- Multi-project workflows: Work across multiple related projects

Examples:
- Get current: `{\"path\": null}` or `{}`
- Set new: `{\"path\": \"/path/to/worktree\"}`
- Reset: `{\"reset\": true}`

Note: This changes the working directory for the current thread only.
File paths in subsequent tool calls will be relative to this directory.".to_string(),
		parameters: json!({
			"type": "object",
			"properties": {
				"path": {
					"type": "string",
					"description": "Optional path to set as new working directory. Can be absolute or relative to current working directory."
				},
				"reset": {
					"type": "boolean",
					"default": false,
					"description": "If true, reset to original project directory (ignores 'path' parameter)"
				}
			}
		}),
	}
}

/// Execute working directory command
pub async fn execute_workdir_command(call: &McpToolCall) -> Result<McpToolResult> {
	let reset = call
		.parameters
		.get("reset")
		.and_then(|v| v.as_bool())
		.unwrap_or(false);

	// Reset to original session directory (set at session creation, not process cwd)
	if reset {
		let original_dir = get_thread_original_working_directory();
		set_thread_working_directory(original_dir.clone());

		return Ok(McpToolResult::success(
			"workdir".to_string(),
			call.tool_id.clone(),
			json!({
				"success": true,
				"action": "reset",
				"working_directory": original_dir.to_string_lossy(),
				"message": format!("Working directory reset to: {}", original_dir.display())
			})
			.to_string(),
		));
	}

	// Get or set working directory
	match call.parameters.get("path") {
		Some(Value::String(path_str)) if !path_str.trim().is_empty() => {
			let path_str = path_str.trim();

			// Resolve the path (handle relative paths)
			let new_path = if std::path::Path::new(path_str).is_absolute() {
				std::path::PathBuf::from(path_str)
			} else {
				// Relative to current working directory
				let current = get_thread_working_directory();
				current.join(path_str)
			};

			// Canonicalize to resolve .. and symlinks
			let canonical_path = match new_path.canonicalize() {
				Ok(p) => p,
				Err(e) => {
					return Ok(McpToolResult::error(
						"workdir".to_string(),
						call.tool_id.clone(),
						format!(
							"Path does not exist or is not accessible: {} (error: {})",
							new_path.display(),
							e
						),
					));
				}
			};

			// Verify it's a directory
			if !canonical_path.is_dir() {
				return Ok(McpToolResult::error(
					"workdir".to_string(),
					call.tool_id.clone(),
					format!("Path is not a directory: {}", canonical_path.display()),
				));
			}

			let old_dir = get_thread_working_directory();
			set_thread_working_directory(canonical_path.clone());

			Ok(McpToolResult::success(
				"workdir".to_string(),
				call.tool_id.clone(),
				json!({
					"success": true,
					"action": "set",
					"previous_directory": old_dir.to_string_lossy(),
					"working_directory": canonical_path.to_string_lossy(),
					"message": format!("Working directory changed from {} to {}", old_dir.display(), canonical_path.display())
				}).to_string(),
			))
		}
		Some(_) => Ok(McpToolResult::error(
			"workdir".to_string(),
			call.tool_id.clone(),
			"Parameter 'path' must be a non-empty string".to_string(),
		)),
		None => {
			// Get current working directory
			let current_dir = get_thread_working_directory();

			Ok(McpToolResult::success(
				"workdir".to_string(),
				call.tool_id.clone(),
				json!({
					"success": true,
					"action": "get",
					"working_directory": current_dir.to_string_lossy(),
					"message": format!("Current working directory: {}", current_dir.display())
				})
				.to_string(),
			))
		}
	}
}
