// Core functionality and shared utilities for file system operations

use std::path::Path;
use std::collections::HashMap;
use std::sync::Mutex;
use serde_json::{json, Value};
use anyhow::{Result, anyhow};
use tokio::fs as tokio_fs;
use lazy_static::lazy_static;
use super::super::{McpToolCall, McpToolResult};
use crate::mcp::fs::{file_ops, text_editing, directory, html_converter};

// Thread-safe lazy initialization of file history using lazy_static
lazy_static! {
	pub static ref FILE_HISTORY: Mutex<HashMap<String, Vec<String>>> = Mutex::new(HashMap::new());
}

// Thread-safe way to get the file history
pub fn get_file_history() -> &'static Mutex<HashMap<String, Vec<String>>> {
	&FILE_HISTORY
}

// Save the current content of a file for undo
pub async fn save_file_history(path: &Path) -> Result<()> {
	if path.exists() {
		// First read the content
		let content = tokio_fs::read_to_string(path).await?;
		let path_str = path.to_string_lossy().to_string();

		// Then update the history with the lock held
		let file_history = get_file_history();
		{
			let mut history_guard = file_history.lock().map_err(|_| anyhow!("Failed to acquire lock on file history"))?;

			let history = history_guard.entry(path_str).or_insert_with(Vec::new);

			// Limit history size to avoid excessive memory usage
			if history.len() >= 10 {
				history.remove(0);
			}

			history.push(content);
		} // Lock is released here
	}
	Ok(())
}

// Undo the last edit to a file
pub async fn undo_edit(call: &McpToolCall, path: &Path) -> Result<McpToolResult> {
	let path_str = path.to_string_lossy().to_string();

	// First retrieve the previous content while holding the lock
	let previous_content = {
		let file_history = get_file_history();
		let mut history_guard = file_history.lock().map_err(|_| anyhow!("Failed to acquire lock on file history"))?;

		if let Some(history) = history_guard.get_mut(&path_str) {
			history.pop()
		} else {
			None
		}
	}; // Lock is released here when history_guard goes out of scope

	// Now we have the previous content or None, and we've released the lock
	if let Some(prev_content) = previous_content {
		// Write the previous content
		tokio_fs::write(path, &prev_content).await?;

		// Get remaining history count
		let history_remaining = {
			let file_history = get_file_history();
			let history_guard = file_history.lock().map_err(|_| anyhow!("Failed to acquire lock on file history"))?;

			history_guard.get(&path_str).map_or(0, |h| h.len())
		};

		Ok(McpToolResult {
			tool_name: "text_editor".to_string(),
			tool_id: call.tool_id.clone(),
			result: json!({
				"success": true,
				"output": format!("Successfully undid the last edit to {}", path.to_string_lossy()),
				"path": path.to_string_lossy(),
				"history_remaining": history_remaining,
				"parameters": {
					"command": "undo_edit",
					"path": path.to_string_lossy()
				}
			}),
		})
	} else {
		Err(anyhow!("No edit history available for this file"))
	}
}

// Helper function to detect language based on file extension
pub fn detect_language(ext: &str) -> &str {
	match ext {
		"rs" => "rust",
		"py" => "python",
		"js" => "javascript",
		"ts" => "typescript",
		"jsx" => "jsx",
		"tsx" => "tsx",
		"html" => "html",
		"css" => "css",
		"json" => "json",
		"md" => "markdown",
		"go" => "go",
		"java" => "java",
		"c" | "h" | "cpp" => "cpp",
		"toml" => "toml",
		"yaml" | "yml" => "yaml",
		"php" => "php",
		"xml" => "xml",
		"sh" => "bash",
		_ => "text",
	}
}

// Main execution functions

// Execute a text editor command following modern text editor specifications
pub async fn execute_text_editor(call: &McpToolCall) -> Result<McpToolResult> {
	// Extract command parameter
	let command = match call.parameters.get("command") {
		Some(Value::String(cmd)) => cmd.clone(),
		_ => return Err(anyhow!("Missing or invalid 'command' parameter")),
	};

	// Execute the appropriate command
	match command.as_str() {
		"view" => {
			// Extract path parameter for view command
			let path = match call.parameters.get("path") {
				Some(Value::String(p)) => p.clone(),
				_ => return Err(anyhow!("Missing or invalid 'path' parameter for view command")),
			};

			// Check if view_range is specified
			let view_range = call.parameters.get("view_range")
				.and_then(|v| v.as_array())
				.and_then(|arr| {
					if arr.len() == 2 {
						let start = arr[0].as_i64()?;
						let end = arr[1].as_i64()?;
						Some((start as usize, end))
					} else {
						None
					}
				});

			file_ops::view_file_spec(call, Path::new(&path), view_range).await
		},
		"view_many" => {
			// Extract paths parameter for view_many command
			let paths = match call.parameters.get("paths") {
				Some(Value::Array(arr)) => {
					let path_strings: Result<Vec<String>, _> = arr.iter()
						.map(|p| p.as_str().ok_or_else(|| anyhow!("Invalid path in array")))
						.map(|r| r.map(|s| s.to_string()))
						.collect();

					match path_strings {
						Ok(paths) => {
							if paths.len() > 50 {
								return Err(anyhow!("Too many files requested. Maximum 50 files per request."));
							}
							paths
						},
						Err(e) => return Err(e),
					}
				},
				_ => return Err(anyhow!("Missing or invalid 'paths' parameter for view_many command - must be an array of strings")),
			};

			file_ops::view_many_files_spec(call, &paths).await
		},
		"create" => {
			let path = match call.parameters.get("path") {
				Some(Value::String(p)) => p.clone(),
				_ => return Err(anyhow!("Missing or invalid 'path' parameter for create command")),
			};
			let file_text = match call.parameters.get("file_text") {
				Some(Value::String(txt)) => txt.clone(),
				_ => return Err(anyhow!("Missing or invalid 'file_text' parameter for create command")),
			};
			file_ops::create_file_spec(call, Path::new(&path), &file_text).await
		},
		"str_replace" => {
			let path = match call.parameters.get("path") {
				Some(Value::String(p)) => p.clone(),
				_ => return Err(anyhow!("Missing or invalid 'path' parameter for str_replace command")),
			};
			let old_str = match call.parameters.get("old_str") {
				Some(Value::String(s)) => s.clone(),
				_ => return Err(anyhow!("Missing or invalid 'old_str' parameter")),
			};
			let new_str = match call.parameters.get("new_str") {
				Some(Value::String(s)) => s.clone(),
				_ => return Err(anyhow!("Missing or invalid 'new_str' parameter")),
			};
			text_editing::str_replace_spec(call, Path::new(&path), &old_str, &new_str).await
		},
		"insert" => {
			let path = match call.parameters.get("path") {
				Some(Value::String(p)) => p.clone(),
				_ => return Err(anyhow!("Missing or invalid 'path' parameter for insert command")),
			};
			let insert_line = match call.parameters.get("insert_line") {
				Some(Value::Number(n)) => n.as_u64().ok_or_else(|| anyhow!("Invalid 'insert_line' parameter"))? as usize,
				_ => return Err(anyhow!("Missing or invalid 'insert_line' parameter")),
			};
			let new_str = match call.parameters.get("new_str") {
				Some(Value::String(s)) => s.clone(),
				_ => return Err(anyhow!("Missing or invalid 'new_str' parameter for insert command")),
			};
			text_editing::insert_text_spec(call, Path::new(&path), insert_line, &new_str).await
		},
		"line_replace" => {
			let path = match call.parameters.get("path") {
				Some(Value::String(p)) => p.clone(),
				_ => return Err(anyhow!("Missing or invalid 'path' parameter for line_replace command")),
			};
			let view_range = match call.parameters.get("view_range") {
				Some(Value::Array(arr)) => {
					if arr.len() != 2 {
						return Err(anyhow!("'view_range' must be an array of exactly 2 integers for line_replace command"));
					}
					let start = arr[0].as_u64().ok_or_else(|| anyhow!("Invalid start_line in view_range"))? as usize;
					let end = arr[1].as_u64().ok_or_else(|| anyhow!("Invalid end_line in view_range"))? as usize;
					(start, end)
				},
				_ => return Err(anyhow!("Missing or invalid 'view_range' parameter for line_replace command")),
			};
			let new_str = match call.parameters.get("new_str") {
				Some(Value::String(s)) => s.clone(),
				_ => return Err(anyhow!("Missing or invalid 'new_str' parameter for line_replace command")),
			};
			text_editing::line_replace(call, Path::new(&path), view_range, &new_str).await
		},
		"undo_edit" => {
			let path = match call.parameters.get("path") {
				Some(Value::String(p)) => p.clone(),
				_ => return Err(anyhow!("Missing or invalid 'path' parameter for undo_edit command")),
			};
			undo_edit(call, Path::new(&path)).await
		},
		_ => Err(anyhow!("Invalid command: {}. Allowed commands are: view, view_many, create, str_replace, insert, line_replace, undo_edit", command)),
	}
}

// Execute a line replace command
pub async fn execute_line_replace(call: &McpToolCall) -> Result<McpToolResult> {
	// Extract path parameter (single path only)
	let path = match call.parameters.get("path") {
		Some(Value::String(p)) => p.clone(),
		_ => return Err(anyhow!("Missing or invalid 'path' parameter - must be a string")),
	};

	// Extract view_range parameter
	let view_range = match call.parameters.get("view_range") {
		Some(Value::Array(arr)) => {
			if arr.len() != 2 {
				return Err(anyhow!("'view_range' must be an array of exactly 2 integers"));
			}
			let start = arr[0].as_u64().ok_or_else(|| anyhow!("Invalid start_line in view_range"))? as usize;
			let end = arr[1].as_u64().ok_or_else(|| anyhow!("Invalid end_line in view_range"))? as usize;
			(start, end)
		},
		_ => return Err(anyhow!("Missing or invalid 'view_range' parameter - must be an array of 2 integers")),
	};

	// Extract new_str parameter
	let new_str = match call.parameters.get("new_str") {
		Some(Value::String(s)) => s.clone(),
		_ => return Err(anyhow!("Missing or invalid 'new_str' parameter - must be a string")),
	};

	text_editing::line_replace(call, Path::new(&path), view_range, &new_str).await
}

// Execute list_files command
pub async fn execute_list_files(call: &McpToolCall) -> Result<McpToolResult> {
	directory::execute_list_files(call).await
}

// Execute view_many command for viewing multiple files simultaneously
pub async fn execute_view_many(call: &McpToolCall) -> Result<McpToolResult> {
	// Extract paths parameter
	let paths_value = match call.parameters.get("paths") {
		Some(value) => value,
		_ => return Err(anyhow!("Missing 'paths' parameter")),
	};

	// Extract paths array
	let paths = match paths_value.as_array() {
		Some(arr) => {
			let path_strings: Result<Vec<String>, _> = arr.iter()
				.map(|p| p.as_str().ok_or_else(|| anyhow!("Invalid path in array")))
				.map(|r| r.map(|s| s.to_string()))
				.collect();

			match path_strings {
				Ok(paths) => {
					if paths.len() > 50 {
						return Err(anyhow!("Too many files requested. Maximum 50 files per request."));
					}
					paths
				},
				Err(e) => return Err(e),
			}
		},
		_ => return Err(anyhow!("'paths' parameter must be an array of strings")),
	};

	file_ops::view_many_files(call, &paths).await
}

// Execute HTML to Markdown conversion
pub async fn execute_html2md(call: &McpToolCall) -> Result<McpToolResult> {
	html_converter::execute_html2md(call).await
}