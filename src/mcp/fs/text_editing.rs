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

use std::path::Path;
use serde_json::json;
use anyhow::{Result, anyhow};
use tokio::fs as tokio_fs;
use super::super::{McpToolCall, McpToolResult};
use super::core::save_file_history;

// Replace a string in a file following Anthropic specification
pub async fn str_replace_spec(call: &McpToolCall, path: &Path, old_str: &str, new_str: &str) -> Result<McpToolResult> {
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
	let content = tokio_fs::read_to_string(path).await.map_err(|e| anyhow!("Permission denied. Cannot read file: {}", e))?;

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
	tokio_fs::write(path, new_content).await.map_err(|e| anyhow!("Permission denied. Cannot write to file: {}", e))?;

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
pub async fn insert_text_spec(call: &McpToolCall, path: &Path, insert_line: usize, new_str: &str) -> Result<McpToolResult> {
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
	let content = tokio_fs::read_to_string(path).await.map_err(|e| anyhow!("Permission denied. Cannot read file: {}", e))?;
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
	tokio_fs::write(path, final_content).await.map_err(|e| anyhow!("Permission denied. Cannot write to file: {}", e))?;

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

// Replace content within a specific line range following modern text editor specifications
pub async fn line_replace_spec(call: &McpToolCall, path: &Path, start_line: usize, end_line: usize, new_text: &str) -> Result<McpToolResult> {
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
	let content = tokio_fs::read_to_string(path).await.map_err(|e| anyhow!("Permission denied. Cannot read file: {}", e))?;
	let mut lines: Vec<&str> = content.lines().collect();

	// Validate line ranges exist in file
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

	// Save the current content for undo
	save_file_history(path).await?;

	// Split new content into lines
	let new_lines: Vec<&str> = new_text.lines().collect();

	// Convert to 0-indexed for array operations
	let start_idx = start_line - 1;
	let end_idx = end_line; // end_idx is exclusive in splice

	// Replace the lines using splice
	lines.splice(start_idx..end_idx, new_lines);

	// Join lines back to string
	let new_content = lines.join("\n");

	// Add final newline if original file had one
	let final_content = if content.ends_with('\n') {
		format!("{}\n", new_content)
	} else {
		new_content
	};

	// Write the new content
	tokio_fs::write(path, final_content).await.map_err(|e| anyhow!("Permission denied. Cannot write to file: {}", e))?;

	Ok(McpToolResult {
		tool_name: "text_editor".to_string(),
		tool_id: call.tool_id.clone(),
		result: json!({
			"content": format!("Successfully replaced {} lines with {} lines", end_line - start_line + 1, new_text.lines().count()),
			"path": path.to_string_lossy(),
			"lines_replaced": end_line - start_line + 1,
			"new_lines": new_text.lines().count()
		}),
	})
}

// Replace lines in a single file using view_range and new_str parameters
pub async fn line_replace(call: &McpToolCall, path: &Path, view_range: (usize, usize), new_str: &str) -> Result<McpToolResult> {
	if !path.exists() {
		return Ok(McpToolResult {
			tool_name: "line_replace".to_string(),
			tool_id: call.tool_id.clone(),
			result: json!({
				"error": "File not found",
				"is_error": true
			}),
		});
	}

	if !path.is_file() {
		return Ok(McpToolResult {
			tool_name: "line_replace".to_string(),
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
			tool_name: "line_replace".to_string(),
			tool_id: call.tool_id.clone(),
			result: json!({
				"error": "Line numbers must be 1-indexed (start from 1)",
				"is_error": true
			}),
		});
	}

	if start_line > end_line {
		return Ok(McpToolResult {
			tool_name: "line_replace".to_string(),
			tool_id: call.tool_id.clone(),
			result: json!({
				"error": format!("start_line ({}) must be less than or equal to end_line ({})", start_line, end_line),
				"is_error": true
			}),
		});
	}

	// Read the file content
	let file_content = tokio_fs::read_to_string(path).await.map_err(|e| anyhow!("Permission denied. Cannot read file: {}", e))?;
	let mut lines: Vec<&str> = file_content.lines().collect();

	// Capture the original lines that will be replaced for the snippet
	let original_lines: Vec<String> = lines[start_line - 1..end_line]
		.iter()
		.map(|&line| line.to_string())
		.collect();

	// Validate line ranges exist in file
	if start_line > lines.len() {
		return Ok(McpToolResult {
			tool_name: "line_replace".to_string(),
			tool_id: call.tool_id.clone(),
			result: json!({
				"error": format!("start_line ({}) exceeds file length ({} lines)", start_line, lines.len()),
				"is_error": true
			}),
		});
	}

	if end_line > lines.len() {
		return Ok(McpToolResult {
			tool_name: "line_replace".to_string(),
			tool_id: call.tool_id.clone(),
			result: json!({
				"error": format!("end_line ({}) exceeds file length ({} lines)", end_line, lines.len()),
				"is_error": true
			}),
		});
	}

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
	tokio_fs::write(path, final_content).await.map_err(|e| anyhow!("Permission denied. Cannot write to file: {}", e))?;

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
		tool_name: "line_replace".to_string(),
		tool_id: call.tool_id.clone(),
		result: json!({
			"success": true,
			"content": format!("Successfully replaced {} lines with {} lines", end_line - start_line + 1, new_str.lines().count()),
			"replaced": replaced_snippet
		}),
	})
}
