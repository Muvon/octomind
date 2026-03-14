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
use lazy_static::lazy_static;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::fs as tokio_fs;
use tokio::sync::Mutex;

// Thread-safe file locking infrastructure for concurrent write protection
lazy_static! {
	static ref FILE_LOCKS: Mutex<HashMap<String, Arc<Mutex<()>>>> = Mutex::new(HashMap::new());
}
// Acquire a file-specific lock to prevent concurrent writes to the same file
async fn acquire_file_lock(path: &Path) -> Result<Arc<Mutex<()>>> {
	let path_str = path.to_string_lossy().to_string();

	let mut locks = FILE_LOCKS.lock().await;

	let file_lock = locks
		.entry(path_str)
		.or_insert_with(|| Arc::new(Mutex::new(())))
		.clone();

	Ok(file_lock)
}

// Helper function to resolve line indices, supporting negative indexing
// Negative indices count from the end: -1 = last line, -2 = second-to-last, etc.
fn resolve_line_index(index: i64, total_lines: usize) -> Result<usize, String> {
	if index == 0 {
		return Err("Line numbers are 1-indexed, use 1 for first line".to_string());
	}

	if index > 0 {
		let pos_index = index as usize;
		if pos_index > total_lines {
			return Err(format!(
				"Line {} exceeds file length ({} lines)",
				index, total_lines
			));
		}
		Ok(pos_index)
	} else {
		// Negative indexing: -1 = last line, -2 = second-to-last, etc.
		let from_end = (-index) as usize;
		if from_end > total_lines {
			return Err(format!(
				"Negative index {} exceeds file length ({} lines)",
				index, total_lines
			));
		}
		Ok(total_lines - from_end + 1)
	}
}

// Helper function to resolve line range with negative indexing support
fn resolve_line_range_batch(
	start: i64,
	end: i64,
	total_lines: usize,
) -> Result<(usize, usize), String> {
	let resolved_start = resolve_line_index(start, total_lines)?;
	let resolved_end = resolve_line_index(end, total_lines)?;

	if resolved_start > resolved_end {
		return Err(format!(
			"Start line ({}) cannot be greater than end line ({})",
			start, end
		));
	}

	Ok((resolved_start, resolved_end))
}

// Batch operation structures for the new single-file, multi-operation approach
#[derive(Debug, Clone)]
struct BatchOperation {
	operation_type: OperationType,
	line_range: LineRange,
	content: String,
	operation_index: usize,
}

// Unresolved batch operation with raw line indices (may be negative)
#[derive(Debug, Clone)]
struct UnresolvedBatchOperation {
	operation_type: OperationType,
	line_range: UnresolvedLineRange,
	content: String,
	operation_index: usize,
}

#[derive(Debug, Clone, PartialEq)]
enum OperationType {
	Insert,
	Replace,
}

#[derive(Debug, Clone)]
enum LineRange {
	Single(usize),       // Insert after this line (0 = beginning of file)
	Range(usize, usize), // Replace this range (inclusive, 1-indexed)
}

#[derive(Debug, Clone)]
enum UnresolvedLineRange {
	Single(i64),     // Insert after this line (may be negative)
	Range(i64, i64), // Replace this range (may be negative)
}

// Resolve unresolved line range to actual line range using file length
fn resolve_unresolved_line_range(
	unresolved: &UnresolvedLineRange,
	total_lines: usize,
) -> Result<LineRange, String> {
	match unresolved {
		UnresolvedLineRange::Single(line) => {
			let resolved = resolve_line_index(*line, total_lines)?;
			Ok(LineRange::Single(resolved))
		}
		UnresolvedLineRange::Range(start, end) => {
			let (resolved_start, resolved_end) =
				resolve_line_range_batch(*start, *end, total_lines)?;
			Ok(LineRange::Range(resolved_start, resolved_end))
		}
	}
}

// Replace a string in a file following Anthropic specification
pub async fn str_replace_spec(
	call: &McpToolCall,
	path: &Path,
	old_text: &str,
	new_text: &str,
) -> Result<McpToolResult> {
	if !path.exists() {
		return Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			"File not found".to_string(),
		));
	}

	// Acquire file lock to prevent concurrent writes
	let file_lock = acquire_file_lock(path).await?;
	let _lock_guard = file_lock.lock().await;

	// Read the file content
	let content = tokio_fs::read_to_string(path)
		.await
		.map_err(|e| anyhow!("Permission denied. Cannot read file: {}", e))?;

	// Check if old_text appears in the file
	let occurrences = content.matches(old_text).count();
	if occurrences == 0 {
		return Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			"No match found for replacement. Please check your text and try again. Make sure you are not escaping \\\\t, \\\\n or similiar and pass raw content. Alternatively, use line_replace when you know exactly which line to replace.".to_string(),
		));
	}
	if occurrences > 1 {
		return Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("Found {} matches for replacement text. Please provide more context to make a unique match or use line_replace when you know exactly which line to replace.", occurrences),
		));
	}

	// Save the current content for undo
	save_file_history(path).await?;

	// Replace the string
	let new_content = content.replace(old_text, new_text);

	// Write the new content
	tokio_fs::write(path, &new_content)
		.await
		.map_err(|e| anyhow!("Permission denied. Cannot write to file: {}", e))?;

	// Push hint into accumulator if str_replace matched multiple lines — line_replace is better
	let line_count = old_text.lines().count();
	if line_count > 1 && crate::mcp::tool_map::get_server_for_tool("text_editor").is_some() {
		crate::mcp::hint_accumulator::push_hint(&format!(
			"`str_replace` matched {} lines. Prefer `line_replace` when you know the line range — it's faster and avoids content-search ambiguity.",
			line_count
		));
	}

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
	insert_after_line: usize,
	content: &str,
) -> Result<McpToolResult> {
	if !path.exists() {
		return Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			"File not found".to_string(),
		));
	}

	// Acquire file lock to prevent concurrent writes
	let file_lock = acquire_file_lock(path).await?;
	let _lock_guard = file_lock.lock().await;

	// Read the file content
	let file_content = tokio_fs::read_to_string(path)
		.await
		.map_err(|e| anyhow!("Permission denied. Cannot read file: {}", e))?;
	let mut lines: Vec<&str> = file_content.lines().collect();

	// Validate insert_after_line
	if insert_after_line > lines.len() {
		return Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!(
				"Insert line {} exceeds file length ({} lines)",
				insert_after_line,
				lines.len()
			),
		));
	}

	// Save the current content for undo
	save_file_history(path).await?;

	// Split new content into lines
	let new_lines: Vec<&str> = content.lines().collect();

	// Insert the new lines
	let insert_index = insert_after_line; // 0 means beginning, 1 means after line 1, etc.
	lines.splice(insert_index..insert_index, new_lines);

	// Join lines back to string
	let new_content = lines.join("\n");

	// Add final newline if original file had one
	let final_content = if file_content.ends_with('\n') {
		format!("{}\n", new_content)
	} else {
		new_content
	};

	// Write the new content
	tokio_fs::write(path, &final_content)
		.await
		.map_err(|e| anyhow!("Permission denied. Cannot write to file: {}", e))?;

	Ok(McpToolResult {
		tool_name: "text_editor".to_string(),
		tool_id: call.tool_id.clone(),
		result: json!({
			"content": format!("Successfully inserted {} lines at line {}", content.lines().count(), insert_after_line),
			"path": path.to_string_lossy(),
			"lines_inserted": content.lines().count()
		}),
	})
}

// Replace content within a specific line range following text editor specifications
pub async fn line_replace_spec(
	call: &McpToolCall,
	path: &Path,
	lines: (usize, usize),
	content: &str,
) -> Result<McpToolResult> {
	if !path.exists() {
		return Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			"File not found".to_string(),
		));
	}

	if !path.is_file() {
		return Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			"Path is not a file".to_string(),
		));
	}

	let (start_line, end_line) = lines;

	// Validate content for escaped characters — catch both \t and \n independently
	if content.contains("\\t") || content.contains("\\n") {
		return Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			"content must be RAW text — do not escape whitespace (use real tabs/newlines, not \\t or \\n)".to_string(),
		));
	}

	// Validate line numbers
	if start_line == 0 || end_line == 0 {
		return Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			"Line numbers must be 1-indexed (start from 1)".to_string(),
		));
	}

	if start_line > end_line {
		return Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!(
				"start_line ({}) must be less than or equal to end_line ({})",
				start_line, end_line
			),
		));
	}

	// Acquire file lock to prevent concurrent writes
	let file_lock = acquire_file_lock(path).await?;
	let _lock_guard = file_lock.lock().await;

	// Read the file content
	let file_content = tokio_fs::read_to_string(path)
		.await
		.map_err(|e| anyhow!("Permission denied. Cannot read file: {}", e))?;
	let lines: Vec<&str> = file_content.lines().collect();

	// Validate line ranges exist in file BEFORE accessing the array
	if start_line > lines.len() {
		return Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!(
				"start_line ({}) exceeds file length ({} lines)",
				start_line,
				lines.len()
			),
		));
	}

	if end_line > lines.len() {
		return Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!(
				"end_line ({}) exceeds file length ({} lines)",
				end_line,
				lines.len()
			),
		));
	}

	// Capture the original lines that will be replaced for the snippet
	// Ensure end_line doesn't exceed the actual file length to prevent panic
	let safe_end_line = end_line.min(lines.len());
	let original_lines: Vec<String> = lines[start_line - 1..safe_end_line]
		.iter()
		.map(|&line| line.to_string())
		.collect();

	// DUPLICATE DETECTION: block writes where content duplicates an adjacent line —
	// this is the #1 cause of AI-introduced line duplication.
	let content_lines: Vec<&str> = content.lines().collect();

	if !content_lines.is_empty() {
		// First line of new content matches the line immediately before the range → AI included a surrounding line it shouldn't have
		if start_line > 1 {
			let line_before = lines[start_line - 2];
			if content_lines[0].trim() == line_before.trim() && !line_before.trim().is_empty() {
				return Ok(McpToolResult::error(
					call.tool_name.clone(),
					call.tool_id.clone(),
					format!(
						"Duplicate line detected: your content's first line matches line {} (just before the replacement range). \
						Do NOT include surrounding unchanged lines in content — only provide the lines that replace [{}-{}].",
						start_line - 1, start_line, end_line
					),
				));
			}
		}

		// Last line of new content matches the line immediately after the range → same problem on the other side
		if end_line < lines.len() {
			let line_after = lines[end_line];
			let last_content_line = content_lines[content_lines.len() - 1];
			if last_content_line.trim() == line_after.trim() && !line_after.trim().is_empty() {
				return Ok(McpToolResult::error(
					call.tool_name.clone(),
					call.tool_id.clone(),
					format!(
						"Duplicate line detected: your content's last line matches line {} (just after the replacement range). \
						Do NOT include surrounding unchanged lines in content — only provide the lines that replace [{}-{}].",
						end_line + 1, start_line, end_line
					),
				));
			}
		}
	}

	// Save the current content for undo
	save_file_history(path).await?;

	// Simple and correct approach: use the lines array we already have
	// but reconstruct the content properly preserving line endings
	let mut result_parts: Vec<&str> = Vec::new();

	// Add lines before target range
	for line in lines.iter().take(start_line - 1) {
		result_parts.push(*line);
	}

	// Add the replacement content
	result_parts.push(content);

	// Add lines after target range
	for line in lines.iter().skip(end_line) {
		result_parts.push(*line);
	}

	// Detect original line ending style
	let line_ending = if file_content.contains("\r\n") {
		"\r\n"
	} else {
		"\n"
	};

	// Reconstruct content
	let new_content = result_parts.join(line_ending);

	// Preserve final line ending behavior
	let final_content = if file_content.ends_with(line_ending) {
		format!("{}{}", new_content, line_ending)
	} else {
		new_content
	};

	// Write the new content
	tokio_fs::write(path, &final_content)
		.await
		.map_err(|e| anyhow!("Permission denied. Cannot write to file: {}", e))?;

	// Build an annotated diff view so the AI can verify the edit landed correctly.
	// Format:
	//   context lines:  "  NNN: <line>"
	//   removed lines:  "- NNN: <line>"
	//   added lines:    "+ NNN: <line>"  (NNN = new line number after edit)
	//   skipped gap:    "  ..."
	const CONTEXT: usize = 3;

	let new_lines: Vec<&str> = final_content.lines().collect();
	let new_lines_count = content_lines.len();

	let mut diff: Vec<String> = Vec::new();

	// --- context before ---
	let ctx_before_start = start_line.saturating_sub(CONTEXT); // 1-indexed, inclusive
	if ctx_before_start > 1 {
		diff.push("...".to_string());
	}
	for i in ctx_before_start..start_line {
		diff.push(format!("{}: {}", i, lines[i - 1]));
	}

	// --- removed lines (what was there before) ---
	for (i, old_line) in original_lines.iter().enumerate() {
		diff.push(format!("-{}: {}", start_line + i, old_line));
	}

	// --- added lines (what is there now) ---
	// New line numbers start at start_line in the post-edit file
	for (i, new_line) in content_lines.iter().enumerate() {
		diff.push(format!("+{}: {}", start_line + i, new_line));
	}

	// --- context after ---
	// In the new file, lines after the edit start at: start_line + new_lines_count
	let new_after_start = start_line + new_lines_count; // 1-indexed in new file
	let new_after_end = (new_after_start + CONTEXT - 1).min(new_lines.len());
	for new_i in new_after_start..=new_after_end {
		diff.push(format!("{}: {}", new_i, new_lines[new_i - 1]));
	}
	if new_after_end < new_lines.len() {
		diff.push("...".to_string());
	}
	diff.push(format!("[{} lines total]", new_lines.len()));

	Ok(McpToolResult {
		tool_name: "text_editor".to_string(),
		tool_id: call.tool_id.clone(),
		result: json!({
			"diff": diff.join("\n"),
		}),
	})
}

// Check for conflicting operations on the same lines
fn detect_conflicts(operations: &[BatchOperation]) -> Result<(), String> {
	for i in 0..operations.len() {
		for j in (i + 1)..operations.len() {
			let op1 = &operations[i];
			let op2 = &operations[j];

			// Get affected lines for each operation
			let lines1 = get_affected_lines(&op1.line_range);
			let lines2 = get_affected_lines(&op2.line_range);

			// Check for overlap
			for line1 in &lines1 {
				for line2 in &lines2 {
					if line1 == line2 {
						return Err(format!(
							"Conflicting operations: operation {} and {} both affect line {}",
							op1.operation_index, op2.operation_index, line1
						));
					}
				}
			}
		}
	}
	Ok(())
}

// Get all lines affected by an operation
fn get_affected_lines(line_range: &LineRange) -> Vec<usize> {
	match line_range {
		LineRange::Single(line) => {
			// Insert affects the line it inserts after (for conflict detection)
			vec![*line]
		}
		LineRange::Range(start, end) => {
			// Replace affects all lines in the range
			(*start..=*end).collect()
		}
	}
}

// Apply all operations to the original file content
async fn apply_batch_operations(
	original_content: &str,
	operations: &[BatchOperation],
) -> Result<String> {
	let mut lines: Vec<String> = original_content.lines().map(|s| s.to_string()).collect();

	// Sort operations by line position in reverse order to maintain line number stability
	let mut sorted_ops = operations.to_vec();
	sorted_ops.sort_by(|a, b| {
		let pos_a = match &a.line_range {
			LineRange::Single(line) => *line,
			LineRange::Range(start, _) => *start,
		};
		let pos_b = match &b.line_range {
			LineRange::Single(line) => *line,
			LineRange::Range(start, _) => *start,
		};
		pos_b.cmp(&pos_a) // Reverse order
	});

	// Apply operations from highest line number to lowest
	for operation in sorted_ops {
		match operation.operation_type {
			OperationType::Insert => {
				let insert_after = match operation.line_range {
					LineRange::Single(line) => line,
					_ => return Err(anyhow!("Insert operation must use single line number")),
				};

				// Validate line number
				if insert_after > lines.len() {
					return Err(anyhow!(
						"Insert position {} is beyond file length {}",
						insert_after,
						lines.len()
					));
				}

				// Split content by lines and insert
				let content_lines: Vec<String> =
					operation.content.lines().map(|s| s.to_string()).collect();

				if insert_after == 0 {
					// Insert at beginning
					for (i, line) in content_lines.into_iter().enumerate() {
						lines.insert(i, line);
					}
				} else {
					// Insert after specified line
					for (i, line) in content_lines.into_iter().enumerate() {
						lines.insert(insert_after + i, line);
					}
				}
			}
			OperationType::Replace => {
				let (start, end) = match operation.line_range {
					LineRange::Range(start, end) => (start, end),
					LineRange::Single(line) => (line, line), // Single line replacement
				};

				// Validate line range (1-indexed)
				if start == 0 || end == 0 {
					return Err(anyhow!("Line numbers must be 1-indexed (start from 1)"));
				}
				if start > lines.len() || end > lines.len() {
					return Err(anyhow!(
						"Line range [{}, {}] is beyond file length {}",
						start,
						end,
						lines.len()
					));
				}
				if start > end {
					return Err(anyhow!("Invalid line range: start {} > end {}", start, end));
				}

				// Remove the lines to be replaced (convert to 0-indexed)
				let start_idx = start - 1;
				let end_idx = end - 1;

				// Remove lines in reverse order
				for _ in start_idx..=end_idx {
					lines.remove(start_idx);
				}

				// Insert new content
				let content_lines: Vec<String> =
					operation.content.lines().map(|s| s.to_string()).collect();
				for (i, line) in content_lines.into_iter().enumerate() {
					lines.insert(start_idx + i, line);
				}
			}
		}
	}

	// Preserve original file ending format
	let result = lines.join("\n");
	if original_content.ends_with('\n') && !result.ends_with('\n') {
		Ok(format!("{}\n", result))
	} else {
		Ok(result)
	}
}

// Parse line_range from JSON value (supports both single number and array, including negative indices)
fn parse_line_range(
	value: &Value,
	operation_type: &OperationType,
) -> Result<UnresolvedLineRange, String> {
	match value {
		Value::Number(n) => {
			let line = n.as_i64().ok_or("Line number must be an integer")?;
			match operation_type {
				// 0 is valid for insert: means "insert at beginning of file"
				OperationType::Insert => Ok(UnresolvedLineRange::Single(line)),
				OperationType::Replace => {
					if line == 0 {
						return Err(
							"Replace line numbers are 1-indexed, use 1 for first line".to_string()
						);
					}
					Ok(UnresolvedLineRange::Range(line, line)) // Single line replace
				}
			}
		}
		Value::Array(arr) => {
			if arr.len() == 1 {
				let line = arr[0].as_i64().ok_or("Line number must be an integer")?;
				match operation_type {
					// 0 is valid for insert: means "insert at beginning of file"
					OperationType::Insert => Ok(UnresolvedLineRange::Single(line)),
					OperationType::Replace => {
						if line == 0 {
							return Err("Replace line numbers are 1-indexed, use 1 for first line"
								.to_string());
						}
						Ok(UnresolvedLineRange::Range(line, line))
					}
				}
			} else if arr.len() == 2 {
				let start = arr[0].as_i64().ok_or("Start line must be an integer")?;
				let end = arr[1].as_i64().ok_or("End line must be an integer")?;
				if start == 0 || end == 0 {
					return Err(
						"Replace line numbers are 1-indexed, use 1 for first line".to_string()
					);
				}
				match operation_type {
					OperationType::Insert => Err(
						"Insert operation cannot use line range - use single line number"
							.to_string(),
					),
					OperationType::Replace => Ok(UnresolvedLineRange::Range(start, end)),
				}
			} else {
				Err("Line range array must have 1 or 2 elements".to_string())
			}
		}
		_ => Err("Line range must be a number or array".to_string()),
	}
}

// NEW REVOLUTIONARY BATCH_EDIT: Single file, multiple operations, original line numbers
pub async fn batch_edit_spec(call: &McpToolCall, operations: &[Value]) -> Result<McpToolResult> {
	// Extract path from the call parameters - NEW: single file only
	let path_str = match call.parameters.get("path").and_then(|v| v.as_str()) {
		Some(p) => p,
		None => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Missing required 'path' parameter for batch_edit".to_string(),
			));
		}
	};

	let path = super::core::resolve_path(path_str);

	// Check if file exists
	if !path.exists() {
		return Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("File not found: {}", path_str),
		));
	}

	// Acquire file lock to prevent concurrent writes
	let file_lock = acquire_file_lock(&path).await?;
	let _lock_guard = file_lock.lock().await;

	// Read original file content
	let original_content = match tokio_fs::read_to_string(&path).await {
		Ok(content) => content,
		Err(e) => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				format!("Failed to read file '{}': {}", path_str, e),
			));
		}
	};

	// Parse and validate all operations (with unresolved line ranges)
	let mut unresolved_operations = Vec::new();
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
		let op_type_str = match operation_obj.get("operation").and_then(|v| v.as_str()) {
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

		// Parse operation type
		let operation_type = match op_type_str {
			"insert" => OperationType::Insert,
			"replace" => OperationType::Replace,
			_ => {
				failed_operations += 1;
				operation_details.push(json!({
					"operation_index": index,
					"operation": op_type_str,
					"status": "failed",
					"error": format!("Unsupported operation type: '{}'. Supported operations: insert, replace", op_type_str)
				}));
				continue;
			}
		};

		// Extract line_range
		let line_range = match operation_obj.get("line_range") {
			Some(range_value) => match parse_line_range(range_value, &operation_type) {
				Ok(range) => range,
				Err(e) => {
					failed_operations += 1;
					operation_details.push(json!({
						"operation_index": index,
						"operation": op_type_str,
						"status": "failed",
						"error": format!("Invalid 'line_range': {}", e)
					}));
					continue;
				}
			},
			None => {
				failed_operations += 1;
				operation_details.push(json!({
					"operation_index": index,
					"operation": op_type_str,
					"status": "failed",
					"error": "Missing 'line_range' field"
				}));
				continue;
			}
		};

		// Extract content
		let content = match operation_obj.get("content").and_then(|v| v.as_str()) {
			Some(c) => c.to_string(),
			None => {
				failed_operations += 1;
				operation_details.push(json!({
					"operation_index": index,
					"operation": op_type_str,
					"status": "failed",
					"error": "Missing 'content' field"
				}));
				continue;
			}
		};

		// Create unresolved batch operation
		let unresolved_op = UnresolvedBatchOperation {
			operation_type,
			line_range: line_range.clone(),
			content,
			operation_index: index,
		};

		unresolved_operations.push(unresolved_op);

		operation_details.push(json!({
			"operation_index": index,
			"operation": op_type_str,
			"status": "parsed",
			"line_range": match &line_range {
				UnresolvedLineRange::Single(line) => json!(line),
				UnresolvedLineRange::Range(start, end) => json!([start, end]),
			}
		}));
	}

	// If all operations failed during parsing, return error
	if unresolved_operations.is_empty() {
		return Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!(
				"No valid operations found. {} operations failed during parsing.",
				failed_operations
			),
		));
	}

	// Resolve negative line indices now that we have the file content
	let total_lines = original_content.lines().count();
	let mut batch_operations = Vec::new();

	for unresolved_op in unresolved_operations {
		match resolve_unresolved_line_range(&unresolved_op.line_range, total_lines) {
			Ok(resolved_range) => {
				batch_operations.push(BatchOperation {
					operation_type: unresolved_op.operation_type,
					line_range: resolved_range,
					content: unresolved_op.content,
					operation_index: unresolved_op.operation_index,
				});
			}
			Err(err) => {
				return Ok(McpToolResult::error(
					call.tool_name.clone(),
					call.tool_id.clone(),
					format!(
						"Invalid line range in operation {}: {}",
						unresolved_op.operation_index, err
					),
				));
			}
		}
	}

	// Check for conflicts between operations
	if let Err(conflict_error) = detect_conflicts(&batch_operations) {
		return Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			conflict_error,
		));
	}

	// Apply all operations to the original content
	let final_content = match apply_batch_operations(&original_content, &batch_operations).await {
		Ok(content) => content,
		Err(e) => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				format!("Failed to apply operations: {}", e),
			));
		}
	};

	// Save file history for undo functionality
	save_file_history(&path).await?;

	// Write the final content to file
	if let Err(e) = tokio_fs::write(&path, &final_content).await {
		return Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("Failed to write file '{}': {}", path_str, e),
		));
	}

	// Update operation details with success status
	for detail in &mut operation_details {
		if detail["status"] == "parsed" {
			detail["status"] = json!("success");
		}
	}

	let successful_operations = batch_operations.len();
	let total_operations = operations.len();

	Ok(McpToolResult::success_with_metadata(
		call.tool_name.clone(),
		call.tool_id.clone(),
		format!(
			"Successfully applied {} operations to '{}'. All operations used ORIGINAL line numbers from the file content before any modifications.",
			successful_operations, path_str
		),
		json!({
			"path": path_str,
			"batch_summary": {
				"total_operations": total_operations,
				"successful_operations": successful_operations,
				"failed_operations": failed_operations,
				"overall_success": failed_operations == 0
			},
			"operation_details": operation_details
		}),
	))
}
