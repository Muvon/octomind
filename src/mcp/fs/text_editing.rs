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

// Replace lines in a single file
pub async fn line_replace_single_file(call: &McpToolCall, path: &Path, start_line: usize, end_line: usize, content: &str) -> Result<McpToolResult> {
	if !path.exists() {
		return Err(anyhow!("File does not exist: {}", path.display()));
	}

	if !path.is_file() {
		return Err(anyhow!("Path is not a file: {}", path.display()));
	}

	// Validate line numbers
	if start_line == 0 || end_line == 0 {
		return Err(anyhow!("Line numbers must be 1-indexed (start from 1)"));
	}

	if start_line > end_line {
		return Err(anyhow!("start_line ({}) must be less than or equal to end_line ({})", start_line, end_line));
	}

	// Read the file content
	let file_content = tokio_fs::read_to_string(path).await?;
	let mut lines: Vec<&str> = file_content.lines().collect();

	// Validate line ranges exist in file
	if start_line > lines.len() {
		return Err(anyhow!("start_line ({}) exceeds file length ({} lines)", start_line, lines.len()));
	}

	if end_line > lines.len() {
		return Err(anyhow!("end_line ({}) exceeds file length ({} lines)", end_line, lines.len()));
	}

	// Save the current content for undo
	save_file_history(path).await?;

	// Split new content into lines
	let new_lines: Vec<&str> = content.lines().collect();

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
	tokio_fs::write(path, final_content).await?;

	// Return success in the same format as multiple file replacements for consistency
	Ok(McpToolResult {
		tool_name: "line_replace".to_string(),
		tool_id: call.tool_id.clone(),
		result: json!({
			"success": true,
			"files": [{
				"path": path.to_string_lossy(),
				"success": true,
				"lines_replaced": end_line - start_line + 1,
				"start_line": start_line,
				"end_line": end_line,
				"new_lines": content.lines().count()
			}],
			"count": 1
		}),
	})
}

// Replace lines in multiple files
pub async fn line_replace_multiple_files(call: &McpToolCall, paths: &[String], start_lines: &[usize], end_lines: &[usize], contents: &[String]) -> Result<McpToolResult> {
	let mut results = Vec::with_capacity(paths.len());
	let mut failures = Vec::new();

	// Ensure all arrays have matching length
	if paths.len() != start_lines.len() || paths.len() != end_lines.len() || paths.len() != contents.len() {
		return Err(anyhow!(
			"Mismatch in array lengths. Expected {} paths, {} start_lines, {} end_lines, and {} contents to all match.",
			paths.len(), start_lines.len(), end_lines.len(), contents.len()
		));
	}

	// Process each file replacement
	for (idx, path_str) in paths.iter().enumerate() {
		let path = Path::new(path_str);
		let start_line = start_lines[idx];
		let end_line = end_lines[idx];
		let content = &contents[idx];
		let path_display = path.display().to_string();

		// Check if file exists
		if !path.exists() {
			failures.push(format!("File does not exist: {}", path_display));
			continue;
		}

		if !path.is_file() {
			failures.push(format!("Path is not a file: {}", path_display));
			continue;
		}

		// Validate line numbers
		if start_line == 0 || end_line == 0 {
			failures.push(format!("Line numbers must be 1-indexed for {}", path_display));
			continue;
		}

		if start_line > end_line {
			failures.push(format!(
				"start_line ({}) must be <= end_line ({}) for {}",
				start_line, end_line, path_display
			));
			continue;
		}

		// Try to read the file content
		let file_content = match tokio_fs::read_to_string(path).await {
			Ok(content) => content,
			Err(e) => {
				failures.push(format!("Failed to read {}: {}", path_display, e));
				continue;
			}
		};

		let mut lines: Vec<&str> = file_content.lines().collect();

		// Validate line ranges exist in file
		if start_line > lines.len() {
			failures.push(format!(
				"start_line ({}) exceeds file length ({} lines) for {}",
				start_line, lines.len(), path_display
			));
			continue;
		}

		if end_line > lines.len() {
			failures.push(format!(
				"end_line ({}) exceeds file length ({} lines) for {}",
				end_line, lines.len(), path_display
			));
			continue;
		}

		// Try to save history for undo
		if let Err(e) = save_file_history(path).await {
			failures.push(format!("Failed to save history for {}: {}", path_display, e));
			// But continue with the replacement operation
		}

		// Split new content into lines
		let new_lines: Vec<&str> = content.lines().collect();

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
		match tokio_fs::write(path, final_content).await {
			Ok(_) => {
				results.push(json!({
					"path": path_display,
					"success": true,
					"lines_replaced": end_line - start_line + 1,
					"start_line": start_line,
					"end_line": end_line,
					"new_lines": content.lines().count()
				}));
			},
			Err(e) => {
				failures.push(format!("Failed to write to {}: {}", path_display, e));
			}
		};
	}

	// Return success if at least one file was modified
	Ok(McpToolResult {
		tool_name: "line_replace".to_string(),
		tool_id: call.tool_id.clone(),
		result: json!({
			"success": !results.is_empty(),
			"files": results,
			"count": results.len(),
			"failed": failures
		}),
	})
}