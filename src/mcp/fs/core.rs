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

// Core functionality and shared utilities for file system operations

use super::super::{McpToolCall, McpToolResult};
use crate::mcp::fs::{directory, file_ops, text_editing};
use anyhow::{anyhow, Result};
use lazy_static::lazy_static;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use tokio::fs as tokio_fs;

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
			let mut history_guard = file_history
				.lock()
				.map_err(|_| anyhow!("Failed to acquire lock on file history"))?;

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
		let mut history_guard = file_history
			.lock()
			.map_err(|_| anyhow!("Failed to acquire lock on file history"))?;

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
			let history_guard = file_history
				.lock()
				.map_err(|_| anyhow!("Failed to acquire lock on file history"))?;

			history_guard.get(&path_str).map_or(0, |h| h.len())
		};

		Ok(McpToolResult::success_with_metadata(
			"text_editor".to_string(),
			call.tool_id.clone(),
			format!(
				"Successfully undid the last edit to {}",
				path.to_string_lossy()
			),
			json!({
				"path": path.to_string_lossy(),
				"history_remaining": history_remaining,
				"command": "undo_edit"
			}),
		))
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
pub async fn execute_text_editor(
	call: &McpToolCall,
	cancellation_token: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> Result<McpToolResult> {
	use std::sync::atomic::Ordering;

	// Check for cancellation before starting
	if let Some(ref token) = cancellation_token {
		if token.load(Ordering::SeqCst) {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Text editor operation cancelled".to_string(),
			));
		}
	}

	// Extract command parameter
	let command = match call.parameters.get("command") {
		Some(Value::String(cmd)) => cmd.clone(),
		Some(_) => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Command parameter must be a string".to_string(),
			));
		}
		None => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Missing required 'command' parameter".to_string(),
			));
		}
	};

	// Execute the appropriate command with cancellation checks
	match command.as_str() {
		"view" => {
			// Check for cancellation before view operation
			if let Some(ref token) = cancellation_token {
				if token.load(Ordering::SeqCst) {
					return Ok(McpToolResult::error(call.tool_name.clone(), call.tool_id.clone(), "Text editor operation cancelled".to_string()));
				}
			}

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
			// Check for cancellation before view_many operation
			if let Some(ref token) = cancellation_token {
				if token.load(Ordering::SeqCst) {
					return Ok(McpToolResult::error(call.tool_name.clone(), call.tool_id.clone(), "Text editor operation cancelled".to_string()));
				}
			}

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
			// Check for cancellation before create operation
			if let Some(ref token) = cancellation_token {
				if token.load(Ordering::SeqCst) {
					return Ok(McpToolResult::error(call.tool_name.clone(), call.tool_id.clone(), "Text editor operation cancelled".to_string()));
				}
			}

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
			// Check for cancellation before str_replace operation
			if let Some(ref token) = cancellation_token {
				if token.load(Ordering::SeqCst) {
					return Ok(McpToolResult::error(call.tool_name.clone(), call.tool_id.clone(), "Text editor operation cancelled".to_string()));
				}
			}

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
			// Check for cancellation before insert operation
			if let Some(ref token) = cancellation_token {
				if token.load(Ordering::SeqCst) {
					return Ok(McpToolResult::error(call.tool_name.clone(), call.tool_id.clone(), "Text editor operation cancelled".to_string()));
				}
			}

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
			// Check for cancellation before line_replace operation
			if let Some(ref token) = cancellation_token {
				if token.load(Ordering::SeqCst) {
					return Ok(McpToolResult::error(call.tool_name.clone(), call.tool_id.clone(), "Text editor operation cancelled".to_string()));
				}
			}

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
			text_editing::line_replace_spec(call, Path::new(&path), view_range, &new_str).await
		},
		"undo_edit" => {
			// Check for cancellation before undo_edit operation
			if let Some(ref token) = cancellation_token {
				if token.load(Ordering::SeqCst) {
					return Ok(McpToolResult::error(call.tool_name.clone(), call.tool_id.clone(), "Text editor operation cancelled".to_string()));
				}
			}

			let path = match call.parameters.get("path") {
				Some(Value::String(p)) => p.clone(),
				_ => return Err(anyhow!("Missing or invalid 'path' parameter for undo_edit command")),
			};
			undo_edit(call, Path::new(&path)).await
		},
		"batch_edit" => {
			// Check for cancellation before batch_edit operation
			if let Some(ref token) = cancellation_token {
				if token.load(Ordering::SeqCst) {
					return Ok(McpToolResult::error(call.tool_name.clone(), call.tool_id.clone(), "Text editor operation cancelled".to_string()));
				}
			}

			let (operations_vec, ai_format_warning) = match call.parameters.get("operations") {
				Some(Value::Array(ops)) => {
					// Correct format - AI passed array directly
					if ops.len() > 50 {
						return Err(anyhow!("Too many operations in batch. Maximum 50 operations allowed."));
					}
					(ops.clone(), false)
				},
				Some(Value::String(ops_str)) => {
					// AI incorrectly passed operations as JSON string - try to parse it
					match serde_json::from_str::<Vec<Value>>(ops_str) {
						Ok(parsed_ops) => {
							if parsed_ops.len() > 50 {
								return Err(anyhow!("Too many operations in batch. Maximum 50 operations allowed."));
							}
							crate::log_debug!("AI passed operations as JSON string instead of array - parsing defensively");
							(parsed_ops, true)
						},
						Err(_) => {
							return Err(anyhow!("Invalid 'operations' parameter for batch_edit command - must be an array or valid JSON array string"));
						}
					}
				},
				_ => return Err(anyhow!("Missing or invalid 'operations' parameter for batch_edit command - must be an array")),
			};

			// Create a modified call with the AI format warning flag
			let mut modified_call = call.clone();
			if ai_format_warning {
				modified_call.parameters.as_object_mut().unwrap().insert("_ai_format_warning".to_string(), Value::Bool(true));
			}

			text_editing::batch_edit_spec(&modified_call, &operations_vec).await
		},
		_ => Err(anyhow!("Invalid command: {}. Allowed commands are: view, view_many, create, str_replace, insert, line_replace, undo_edit, batch_edit", command)),
	}
}

// Execute list_files command
pub async fn execute_list_files(
	call: &McpToolCall,
	cancellation_token: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> Result<McpToolResult> {
	use std::sync::atomic::Ordering;

	// Check for cancellation before starting
	if let Some(ref token) = cancellation_token {
		if token.load(Ordering::SeqCst) {
			return Err(anyhow!("List files operation cancelled"));
		}
	}

	directory::execute_list_files(call).await
}

// Execute extract_lines command - MCP compliant implementation
pub async fn execute_extract_lines(
	call: &McpToolCall,
	cancellation_token: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> Result<McpToolResult> {
	use std::path::Path;
	use std::sync::atomic::Ordering;
	use tokio::fs;

	// Check for cancellation before starting
	if let Some(ref token) = cancellation_token {
		if token.load(Ordering::SeqCst) {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Extract lines operation cancelled".to_string(),
			));
		}
	}

	// Validate and extract from_path parameter
	let from_path = match call.parameters.get("from_path") {
		Some(Value::String(p)) => {
			if p.trim().is_empty() {
				return Ok(McpToolResult::error(
					call.tool_name.clone(),
					call.tool_id.clone(),
					"Parameter 'from_path' cannot be empty".to_string(),
				));
			}
			p.clone()
		}
		Some(_) => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Parameter 'from_path' must be a string".to_string(),
			));
		}
		None => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Missing required parameter 'from_path'".to_string(),
			));
		}
	};

	// Validate and extract from_range parameter
	let from_range = match call.parameters.get("from_range") {
		Some(Value::Array(arr)) => {
			if arr.len() != 2 {
				return Ok(McpToolResult::error(
					call.tool_name.clone(),
					call.tool_id.clone(),
					"Parameter 'from_range' must be an array with exactly 2 elements".to_string(),
				));
			}

			let start = match arr[0].as_i64() {
				Some(n) if n > 0 => n as usize,
				Some(n) => {
					return Ok(McpToolResult::error(
						call.tool_name.clone(),
						call.tool_id.clone(),
						format!("Start line number must be positive, got: {}", n),
					));
				}
				None => {
					return Ok(McpToolResult::error(
						call.tool_name.clone(),
						call.tool_id.clone(),
						"Start line number must be an integer".to_string(),
					));
				}
			};

			let end = match arr[1].as_i64() {
				Some(n) if n > 0 => n as usize,
				Some(n) => {
					return Ok(McpToolResult::error(
						call.tool_name.clone(),
						call.tool_id.clone(),
						format!("End line number must be positive, got: {}", n),
					));
				}
				None => {
					return Ok(McpToolResult::error(
						call.tool_name.clone(),
						call.tool_id.clone(),
						"End line number must be an integer".to_string(),
					));
				}
			};

			if start > end {
				return Ok(McpToolResult::error(
					call.tool_name.clone(),
					call.tool_id.clone(),
					format!(
						"Start line ({}) cannot be greater than end line ({})",
						start, end
					),
				));
			}

			(start, end)
		}
		Some(_) => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Parameter 'from_range' must be an array".to_string(),
			));
		}
		None => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Missing required parameter 'from_range'".to_string(),
			));
		}
	};

	// Validate and extract append_path parameter
	let append_path = match call.parameters.get("append_path") {
		Some(Value::String(p)) => {
			if p.trim().is_empty() {
				return Ok(McpToolResult::error(
					call.tool_name.clone(),
					call.tool_id.clone(),
					"Parameter 'append_path' cannot be empty".to_string(),
				));
			}
			p.clone()
		}
		Some(_) => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Parameter 'append_path' must be a string".to_string(),
			));
		}
		None => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Missing required parameter 'append_path'".to_string(),
			));
		}
	};

	// Validate and extract append_line parameter
	let append_line = match call.parameters.get("append_line") {
		Some(Value::Number(n)) => match n.as_i64() {
			Some(line) => line,
			None => {
				return Ok(McpToolResult::error(
					call.tool_name.clone(),
					call.tool_id.clone(),
					"Parameter 'append_line' must be an integer".to_string(),
				));
			}
		},
		Some(_) => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Parameter 'append_line' must be an integer".to_string(),
			));
		}
		None => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Missing required parameter 'append_line'".to_string(),
			));
		}
	};

	// Check for cancellation before file operations
	if let Some(ref token) = cancellation_token {
		if token.load(Ordering::SeqCst) {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Extract lines operation cancelled".to_string(),
			));
		}
	}

	// Read source file
	let from_path_obj = Path::new(&from_path);
	if !from_path_obj.exists() {
		return Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("Source file does not exist: {}", from_path),
		));
	}

	let source_content = match fs::read_to_string(&from_path_obj).await {
		Ok(content) => content,
		Err(e) => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				format!("Failed to read source file '{}': {}", from_path, e),
			));
		}
	};

	// Split content into lines and validate range
	let source_lines: Vec<&str> = source_content.lines().collect();
	let total_lines = source_lines.len();

	if from_range.0 > total_lines {
		return Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!(
				"Start line {} exceeds file length ({} lines) in '{}'",
				from_range.0, total_lines, from_path
			),
		));
	}

	if from_range.1 > total_lines {
		return Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!(
				"End line {} exceeds file length ({} lines) in '{}'",
				from_range.1, total_lines, from_path
			),
		));
	}

	// Extract the specified lines (convert to 0-indexed)
	let extracted_lines: Vec<&str> = source_lines[(from_range.0 - 1)..from_range.1].to_vec();

	// Preserve original newline structure by checking if source content ends with newline
	// and if we're extracting the last line
	let source_ends_with_newline = source_content.ends_with('\n');
	let extracting_last_line = from_range.1 == total_lines;

	let extracted_content =
		if extracted_lines.len() == 1 && extracting_last_line && !source_ends_with_newline {
			// Single line extraction from end of file without trailing newline
			extracted_lines[0].to_string()
		} else if extracting_last_line && source_ends_with_newline {
			// Extracting from end and source has trailing newline - preserve it
			format!("{}\n", extracted_lines.join("\n"))
		} else {
			// Normal case - join lines with newlines
			extracted_lines.join("\n")
		};

	// Check for cancellation before target file operations
	if let Some(ref token) = cancellation_token {
		if token.load(Ordering::SeqCst) {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Extract lines operation cancelled".to_string(),
			));
		}
	}

	// Handle target file - create parent directories if needed
	let append_path_obj = Path::new(&append_path);
	if let Some(parent) = append_path_obj.parent() {
		if let Err(e) = fs::create_dir_all(parent).await {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				format!(
					"Failed to create parent directories for '{}': {}",
					append_path, e
				),
			));
		}
	}

	// Read existing target file content or create empty if doesn't exist
	let target_content = if append_path_obj.exists() {
		match fs::read_to_string(&append_path_obj).await {
			Ok(content) => content,
			Err(e) => {
				return Ok(McpToolResult::error(
					call.tool_name.clone(),
					call.tool_id.clone(),
					format!("Failed to read target file '{}': {}", append_path, e),
				));
			}
		}
	} else {
		String::new()
	};

	// Determine insertion logic based on append_line
	let final_content = if append_line == 0 {
		// Insert at beginning
		if target_content.is_empty() {
			extracted_content.clone()
		} else {
			// Check if extracted content already ends with newline
			if extracted_content.ends_with('\n') {
				format!("{}{}", extracted_content, target_content)
			} else {
				format!("{}\n{}", extracted_content, target_content)
			}
		}
	} else if append_line == -1 {
		// Append at end
		if target_content.is_empty() {
			extracted_content.clone()
		} else if target_content.ends_with('\n') {
			format!("{}{}", target_content, extracted_content)
		} else {
			format!("{}\n{}", target_content, extracted_content)
		}
	} else {
		// Insert after specific line
		let target_lines: Vec<&str> = target_content.lines().collect();
		let insert_after = append_line as usize;

		if insert_after > target_lines.len() {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				format!(
					"Insert position {} exceeds target file length ({} lines) in '{}'",
					insert_after,
					target_lines.len(),
					append_path
				),
			));
		}

		let mut new_lines = Vec::new();

		// Add lines before insertion point
		new_lines.extend(target_lines[..insert_after].iter().map(|s| s.to_string()));

		// Add extracted content
		new_lines.extend(extracted_lines.iter().map(|s| s.to_string()));

		// Add remaining lines after insertion point
		if insert_after < target_lines.len() {
			new_lines.extend(target_lines[insert_after..].iter().map(|s| s.to_string()));
		}

		// Preserve target file's newline structure
		let target_ends_with_newline = target_content.ends_with('\n');
		if target_ends_with_newline {
			format!("{}\n", new_lines.join("\n"))
		} else {
			new_lines.join("\n")
		}
	};

	// Write the final content to target file
	if let Err(e) = fs::write(&append_path_obj, &final_content).await {
		return Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("Failed to write to target file '{}': {}", append_path, e),
		));
	}

	// Return success result with useful information
	let lines_extracted = from_range.1 - from_range.0 + 1;
	let position_desc = match append_line {
		0 => "beginning of file".to_string(),
		-1 => "end of file".to_string(),
		n => format!("after line {}", n),
	};

	Ok(McpToolResult::success(
		call.tool_name.clone(),
		call.tool_id.clone(),
		format!(
			"Successfully extracted {} lines (lines {}-{}) from '{}' and appended to '{}' at {}.\n\nExtracted content:\n{}",
			lines_extracted,
			from_range.0,
			from_range.1,
			from_path,
			append_path,
			position_desc,
			extracted_content
		),
	))
}
