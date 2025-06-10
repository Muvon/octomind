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

// Text editing module - handling string replacement, line operations, and insertions

use super::super::{McpToolCall, McpToolResult};
use super::core::save_file_history;
use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::path::Path;
use tokio::fs as tokio_fs;

// Replace a string in a file following Anthropic specification
pub async fn str_replace_spec(
	call: &McpToolCall,
	path: &Path,
	old_str: &str,
	new_str: &str,
) -> Result<McpToolResult> {
	if !path.exists() {
		return Ok(McpToolResult {
			tool_name: "text_editor".to_string(),
			tool_id: call.tool_id.clone(),
			result: json!({
				"error": "File not found",
				"is_error": true
			}),
		});
	}

	// Read the file content
	let content = tokio_fs::read_to_string(path)
		.await
		.map_err(|e| anyhow!("Permission denied. Cannot read file: {}", e))?;

	// Check if old_str appears in the file
	let occurrences = content.matches(old_str).count();
	if occurrences == 0 {
		return Ok(McpToolResult {
			tool_name: "text_editor".to_string(),
			tool_id: call.tool_id.clone(),
			result: json!({
				"error": "No match found for replacement. Please check your text and try again.",
				"is_error": true
			}),
		});
	}
	if occurrences > 1 {
		return Ok(McpToolResult {
			tool_name: "text_editor".to_string(),
			tool_id: call.tool_id.clone(),
			result: json!({
				"error": format!("Found {} matches for replacement text. Please provide more context to make a unique match.", occurrences),
				"is_error": true
			}),
		});
	}

	// Save the current content for undo
	save_file_history(path).await?;

	// Replace the string
	let new_content = content.replace(old_str, new_str);

	// Write the new content
	tokio_fs::write(path, new_content)
		.await
		.map_err(|e| anyhow!("Permission denied. Cannot write to file: {}", e))?;

	Ok(McpToolResult {
		tool_name: "text_editor".to_string(),
		tool_id: call.tool_id.clone(),
		result: json!({
			"content": "Successfully replaced text at exactly one location.",
			"path": path.to_string_lossy()
		}),
	})
}

// Insert text at a specific location in a file following Anthropic specification
pub async fn insert_text_spec(
	call: &McpToolCall,
	path: &Path,
	insert_line: usize,
	new_str: &str,
) -> Result<McpToolResult> {
	if !path.exists() {
		return Ok(McpToolResult {
			tool_name: "text_editor".to_string(),
			tool_id: call.tool_id.clone(),
			result: json!({
				"error": "File not found",
				"is_error": true
			}),
		});
	}

	// Read the file content
	let content = tokio_fs::read_to_string(path)
		.await
		.map_err(|e| anyhow!("Permission denied. Cannot read file: {}", e))?;
	let mut lines: Vec<&str> = content.lines().collect();

	// Validate insert_line
	if insert_line > lines.len() {
		return Ok(McpToolResult {
			tool_name: "text_editor".to_string(),
			tool_id: call.tool_id.clone(),
			result: json!({
				"error": format!("Insert line {} exceeds file length ({} lines)", insert_line, lines.len()),
				"is_error": true
			}),
		});
	}

	// Save the current content for undo
	save_file_history(path).await?;

	// Split new content into lines
	let new_lines: Vec<&str> = new_str.lines().collect();

	// Insert the new lines
	let insert_index = insert_line; // 0 means beginning, 1 means after line 1, etc.
	lines.splice(insert_index..insert_index, new_lines);

	// Join lines back to string
	let new_content = lines.join("\n");

	// Add final newline if original file had one
	let final_content = if content.ends_with('\n') {
		format!("{}\n", new_content)
	} else {
		new_content
	};

	// Write the new content
	tokio_fs::write(path, final_content)
		.await
		.map_err(|e| anyhow!("Permission denied. Cannot write to file: {}", e))?;

	Ok(McpToolResult {
		tool_name: "text_editor".to_string(),
		tool_id: call.tool_id.clone(),
		result: json!({
			"content": format!("Successfully inserted {} lines at line {}", new_str.lines().count(), insert_line),
			"path": path.to_string_lossy(),
			"lines_inserted": new_str.lines().count()
		}),
	})
}

// Replace content within a specific line range following text editor specifications
pub async fn line_replace_spec(
	call: &McpToolCall,
	path: &Path,
	view_range: (usize, usize),
	new_str: &str,
) -> Result<McpToolResult> {
	if !path.exists() {
		return Ok(McpToolResult {
			tool_name: "text_editor".to_string(),
			tool_id: call.tool_id.clone(),
			result: json!({
				"error": "File not found",
				"is_error": true
			}),
		});
	}

	if !path.is_file() {
		return Ok(McpToolResult {
			tool_name: "text_editor".to_string(),
			tool_id: call.tool_id.clone(),
			result: json!({
				"error": "Path is not a file",
				"is_error": true
			}),
		});
	}

	let (start_line, end_line) = view_range;

	// Validate line numbers
	if start_line == 0 || end_line == 0 {
		return Ok(McpToolResult {
			tool_name: "text_editor".to_string(),
			tool_id: call.tool_id.clone(),
			result: json!({
				"error": "Line numbers must be 1-indexed (start from 1)",
				"is_error": true
			}),
		});
	}

	if start_line > end_line {
		return Ok(McpToolResult {
			tool_name: "text_editor".to_string(),
			tool_id: call.tool_id.clone(),
			result: json!({
				"error": format!("start_line ({}) must be less than or equal to end_line ({})", start_line, end_line),
				"is_error": true
			}),
		});
	}

	// Read the file content
	let file_content = tokio_fs::read_to_string(path)
		.await
		.map_err(|e| anyhow!("Permission denied. Cannot read file: {}", e))?;
	let mut lines: Vec<&str> = file_content.lines().collect();

	// Validate line ranges exist in file BEFORE accessing the array
	if start_line > lines.len() {
		return Ok(McpToolResult {
			tool_name: "text_editor".to_string(),
			tool_id: call.tool_id.clone(),
			result: json!({
				"error": format!("start_line ({}) exceeds file length ({} lines)", start_line, lines.len()),
				"is_error": true
			}),
		});
	}

	if end_line > lines.len() {
		return Ok(McpToolResult {
			tool_name: "text_editor".to_string(),
			tool_id: call.tool_id.clone(),
			result: json!({
				"error": format!("end_line ({}) exceeds file length ({} lines)", end_line, lines.len()),
				"is_error": true
			}),
		});
	}

	// Capture the original lines that will be replaced for the snippet
	// Ensure end_line doesn't exceed the actual file length to prevent panic
	let safe_end_line = end_line.min(lines.len());
	let original_lines: Vec<String> = lines[start_line - 1..safe_end_line]
		.iter()
		.map(|&line| line.to_string())
		.collect();

	// Save the current content for undo
	save_file_history(path).await?;

	// Split new content into lines
	let new_lines: Vec<&str> = new_str.lines().collect();

	// Convert to 0-indexed for array operations
	let start_idx = start_line - 1;
	let end_idx = end_line; // end_idx is exclusive in splice

	// Replace the lines using splice
	lines.splice(start_idx..end_idx, new_lines);

	// Join lines back to string
	let new_content = lines.join("\n");

	// Add final newline if original file had one
	let final_content = if file_content.ends_with('\n') {
		format!("{}\n", new_content)
	} else {
		new_content
	};

	// Write the new content
	tokio_fs::write(path, final_content)
		.await
		.map_err(|e| anyhow!("Permission denied. Cannot write to file: {}", e))?;

	// Create a snippet showing the replaced lines
	let replaced_snippet = if original_lines.is_empty() {
		"(empty range)".to_string()
	} else if original_lines.len() == 1 {
		original_lines[0].clone()
	} else if original_lines.len() <= 3 {
		original_lines.join("\n")
	} else {
		format!(
			"{}\n... [{} more lines]\n{}",
			original_lines[0],
			original_lines.len() - 2,
			original_lines[original_lines.len() - 1]
		)
	};

	Ok(McpToolResult {
		tool_name: "text_editor".to_string(),
		tool_id: call.tool_id.clone(),
		result: json!({
			"content": format!("Successfully replaced {} lines with {} lines", end_line - start_line + 1, new_str.lines().count()),
			"path": path.to_string_lossy(),
			"lines_replaced": end_line - start_line + 1,
			"new_lines": new_str.lines().count(),
			"replaced_snippet": replaced_snippet
		}),
	})
}

// Batch edit operations - perform multiple text editing operations in a single call
// This is recommended for making changes across multiple files or multiple non-interconnected modifications
pub async fn batch_edit_spec(call: &McpToolCall, operations: &[Value]) -> Result<McpToolResult> {
	let mut results = Vec::new();
	let mut successful_operations = 0;
	let mut failed_operations = 0;
	let mut operation_details = Vec::new();

	for (index, operation) in operations.iter().enumerate() {
		let operation_obj = match operation.as_object() {
			Some(obj) => obj,
			None => {
				failed_operations += 1;
				operation_details.push(json!({
					"operation_index": index,
					"status": "failed",
					"error": "Operation must be an object"
				}));
				continue;
			}
		};

		// Extract operation type
		let op_type = match operation_obj.get("operation").and_then(|v| v.as_str()) {
			Some(op) => op,
			None => {
				failed_operations += 1;
				operation_details.push(json!({
					"operation_index": index,
					"status": "failed",
					"error": "Missing 'operation' field"
				}));
				continue;
			}
		};

		// Extract path
		let path_str = match operation_obj.get("path").and_then(|v| v.as_str()) {
			Some(p) => p,
			None => {
				failed_operations += 1;
				operation_details.push(json!({
					"operation_index": index,
					"status": "failed",
					"error": "Missing 'path' field"
				}));
				continue;
			}
		};

		let path = Path::new(path_str);

		// Create a temporary McpToolCall for individual operations
		let temp_call = McpToolCall {
			tool_id: format!("{}_batch_{}", call.tool_id, index),
			tool_name: call.tool_name.clone(),
			parameters: operation.clone(),
		};

		// Execute the operation based on type
		let operation_result = match op_type {
			"str_replace" => {
				let old_str = match operation_obj.get("old_str").and_then(|v| v.as_str()) {
					Some(s) => s,
					None => {
						failed_operations += 1;
						operation_details.push(json!({
							"operation_index": index,
							"operation": op_type,
							"path": path_str,
							"status": "failed",
							"error": "Missing 'old_str' field for str_replace operation"
						}));
						continue;
					}
				};

				let new_str = match operation_obj.get("new_str").and_then(|v| v.as_str()) {
					Some(s) => s,
					None => {
						failed_operations += 1;
						operation_details.push(json!({
							"operation_index": index,
							"operation": op_type,
							"path": path_str,
							"status": "failed",
							"error": "Missing 'new_str' field for str_replace operation"
						}));
						continue;
					}
				};

				str_replace_spec(&temp_call, path, old_str, new_str).await
			}
			"insert" => {
				let insert_line = match operation_obj.get("insert_line").and_then(|v| v.as_u64()) {
					Some(n) => n as usize,
					None => {
						failed_operations += 1;
						operation_details.push(json!({
							"operation_index": index,
							"operation": op_type,
							"path": path_str,
							"status": "failed",
							"error": "Missing or invalid 'insert_line' field for insert operation"
						}));
						continue;
					}
				};

				let new_str = match operation_obj.get("new_str").and_then(|v| v.as_str()) {
					Some(s) => s,
					None => {
						failed_operations += 1;
						operation_details.push(json!({
							"operation_index": index,
							"operation": op_type,
							"path": path_str,
							"status": "failed",
							"error": "Missing 'new_str' field for insert operation"
						}));
						continue;
					}
				};

				insert_text_spec(&temp_call, path, insert_line, new_str).await
			}
			"line_replace" => {
				let view_range = match operation_obj.get("view_range").and_then(|v| v.as_array()) {
					Some(arr) if arr.len() == 2 => {
						let start = arr[0].as_u64().unwrap_or(0) as usize;
						let end = arr[1].as_u64().unwrap_or(0) as usize;
						if start == 0 || end == 0 {
							failed_operations += 1;
							operation_details.push(json!({
								"operation_index": index,
								"operation": op_type,
								"path": path_str,
								"status": "failed",
								"error": "Invalid 'view_range' - line numbers must be 1-indexed"
							}));
							continue;
						}
						(start, end)
					}
					_ => {
						failed_operations += 1;
						operation_details.push(json!({
							"operation_index": index,
							"operation": op_type,
							"path": path_str,
							"status": "failed",
							"error": "Missing or invalid 'view_range' field for line_replace operation"
						}));
						continue;
					}
				};

				let new_str = match operation_obj.get("new_str").and_then(|v| v.as_str()) {
					Some(s) => s,
					None => {
						failed_operations += 1;
						operation_details.push(json!({
							"operation_index": index,
							"operation": op_type,
							"path": path_str,
							"status": "failed",
							"error": "Missing 'new_str' field for line_replace operation"
						}));
						continue;
					}
				};

				line_replace_spec(&temp_call, path, view_range, new_str).await
			}
			_ => {
				failed_operations += 1;
				operation_details.push(json!({
					"operation_index": index,
					"operation": op_type,
					"path": path_str,
					"status": "failed",
					"error": format!("Unsupported operation type: '{}'. Supported operations: str_replace, insert, line_replace", op_type)
				}));
				continue;
			}
		};

		// Process the result
		match operation_result {
			Ok(result) => {
				successful_operations += 1;
				operation_details.push(json!({
					"operation_index": index,
					"operation": op_type,
					"path": path_str,
					"status": "success",
					"result": result.result
				}));
				results.push(result);
			}
			Err(e) => {
				failed_operations += 1;
				operation_details.push(json!({
					"operation_index": index,
					"operation": op_type,
					"path": path_str,
					"status": "failed",
					"error": e.to_string()
				}));
			}
		}
	}

	// Determine overall success
	let overall_success = failed_operations == 0;
	let summary_message = if overall_success {
		format!(
			"Successfully completed all {} batch operations",
			successful_operations
		)
	} else {
		format!(
			"Completed {} operations successfully, {} failed",
			successful_operations, failed_operations
		)
	};

	Ok(McpToolResult {
		tool_name: "text_editor".to_string(),
		tool_id: call.tool_id.clone(),
		result: json!({
			"content": summary_message,
			"batch_summary": {
				"total_operations": operations.len(),
				"successful_operations": successful_operations,
				"failed_operations": failed_operations,
				"overall_success": overall_success
			},
			"operation_details": operation_details
		}),
	})
}
