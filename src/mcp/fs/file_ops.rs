// File operations module - handling file viewing, creation, and basic manipulation

use std::path::Path;
use serde_json::json;
use anyhow::{Result, anyhow};
use tokio::fs as tokio_fs;
use super::super::{McpToolCall, McpToolResult};
use super::core::detect_language;

// View the content of a file following Anthropic specification - with line numbers and view_range support
pub async fn view_file_spec(call: &McpToolCall, path: &Path, view_range: Option<(usize, i64)>) -> Result<McpToolResult> {
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

	if path.is_dir() {
		// List directory contents
		let mut entries = Vec::new();
		let read_dir = tokio_fs::read_dir(path).await.map_err(|e| anyhow!("Permission denied. Cannot read directory: {}", e))?;
		let mut dir_entries = read_dir;

		while let Some(entry) = dir_entries.next_entry().await.map_err(|e| anyhow!("Error reading directory: {}", e))? {
			let name = entry.file_name().to_string_lossy().to_string();
			let is_dir = entry.file_type().await.map_err(|e| anyhow!("Error reading file type: {}", e))?.is_dir();
			entries.push(if is_dir { format!("{}/", name) } else { name });
		}

		entries.sort();
		let content = entries.join("\n");

		return Ok(McpToolResult {
			tool_name: "text_editor".to_string(),
			tool_id: call.tool_id.clone(),
			result: json!({
				"content": content,
				"type": "directory"
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

	// Check file size to avoid loading very large files
	let metadata = tokio_fs::metadata(path).await.map_err(|e| anyhow!("Permission denied. Cannot read file: {}", e))?;
	if metadata.len() > 1024 * 1024 * 5 {  // 5MB limit
		return Ok(McpToolResult {
			tool_name: "text_editor".to_string(),
			tool_id: call.tool_id.clone(),
			result: json!({
				"error": "File is too large (>5MB)",
				"is_error": true
			}),
		});
	}

	// Read the file content
	let content = tokio_fs::read_to_string(path).await.map_err(|e| anyhow!("Permission denied. Cannot read file: {}", e))?;
	let lines: Vec<&str> = content.lines().collect();

	let (content_with_numbers, displayed_lines) = if let Some((start, end)) = view_range {
		// Handle view_range parameter
		let start_idx = if start == 0 { 0 } else { start.saturating_sub(1) }; // Convert to 0-indexed
		let end_idx = if end == -1 {
			lines.len()
		} else {
			(end as usize).min(lines.len())
		};

		if start_idx >= lines.len() {
			return Ok(McpToolResult {
				tool_name: "text_editor".to_string(),
				tool_id: call.tool_id.clone(),
				result: json!({
					"error": format!("Start line {} exceeds file length ({} lines)", start, lines.len()),
					"is_error": true
				}),
			});
		}

		let selected_lines = &lines[start_idx..end_idx];
		let content_with_nums = selected_lines
			.iter()
			.enumerate()
			.map(|(i, line)| format!("{}: {}", start_idx + i + 1, line))
			.collect::<Vec<_>>()
			.join("\n");

		(content_with_nums, end_idx - start_idx)
	} else {
		// Show entire file with line numbers
		let content_with_nums = lines
			.iter()
			.enumerate()
			.map(|(i, line)| format!("{}: {}", i + 1, line))
			.collect::<Vec<_>>()
			.join("\n");

		(content_with_nums, lines.len())
	};

	Ok(McpToolResult {
		tool_name: "text_editor".to_string(),
		tool_id: call.tool_id.clone(),
		result: json!({
			"content": content_with_numbers,
			"lines": displayed_lines,
			"total_lines": lines.len()
		}),
	})
}

// Create a new file following Anthropic specification
pub async fn create_file_spec(call: &McpToolCall, path: &Path, content: &str) -> Result<McpToolResult> {
	// Check if file already exists
	if path.exists() {
		return Ok(McpToolResult {
			tool_name: "text_editor".to_string(),
			tool_id: call.tool_id.clone(),
			result: json!({
				"error": "File already exists",
				"is_error": true
			}),
		});
	}

	// Create parent directories if they don't exist
	if let Some(parent) = path.parent() {
		if !parent.exists() {
			tokio_fs::create_dir_all(parent).await.map_err(|e| anyhow!("Permission denied. Cannot create directories: {}", e))?;
		}
	}

	// Write the content to the file
	tokio_fs::write(path, content).await.map_err(|e| anyhow!("Permission denied. Cannot write to file: {}", e))?;

	Ok(McpToolResult {
		tool_name: "text_editor".to_string(),
		tool_id: call.tool_id.clone(),
		result: json!({
			"content": format!("File created successfully with {} bytes", content.len()),
			"path": path.to_string_lossy(),
			"size": content.len()
		}),
	})
}

// View multiple files simultaneously as part of text_editor tool
pub async fn view_many_files_spec(call: &McpToolCall, paths: &[String]) -> Result<McpToolResult> {
	let mut files = Vec::with_capacity(paths.len());
	let mut failures = Vec::new();
	let mut total_size = 0u64;

	// Process each file in the list with efficient memory usage
	for path_str in paths {
		let path = Path::new(&path_str);
		let path_display = path.display().to_string();

		// Check if file exists and is a regular file
		if !path.exists() {
			failures.push(format!("File does not exist: {}", path_display));
			continue;
		}

		if !path.is_file() {
			failures.push(format!("Not a regular file: {}", path_display));
			continue;
		}

		// Check file size - avoid loading very large files
		let metadata = match tokio_fs::metadata(path).await {
			Ok(meta) => {
				if meta.len() > 1024 * 1024 * 5 { // 5MB limit
					failures.push(format!("File too large (>5MB): {}", path_display));
					continue;
				}
				meta
			},
			Err(e) => {
				failures.push(format!("Cannot read metadata for {}: {}", path_display, e));
				continue;
			}
		};

		// Check if file is binary
		if let Ok(sample) = tokio_fs::read(&path).await {
			let sample_size = sample.len().min(512);
			let null_count = sample[..sample_size].iter().filter(|&&b| b == 0).count();
			if null_count > sample_size / 10 {
				failures.push(format!("Binary file skipped: {}", path_display));
				continue;
			}
		}

		// Read file content with error handling
		let content = match tokio_fs::read_to_string(path).await {
			Ok(content) => content,
			Err(e) => {
				failures.push(format!("Cannot read content of {}: {}", path_display, e));
				continue;
			}
		};

		// Get language from extension for syntax highlighting
		let ext = path.extension()
			.and_then(|e| e.to_str())
			.unwrap_or("");

		// Add line numbers to content
		let lines: Vec<&str> = content.lines().collect();
		let content_with_numbers = lines
			.iter()
			.enumerate()
			.map(|(i, line)| format!("{}: {}", i + 1, line))
			.collect::<Vec<_>>()
			.join("\n");

		// Add file info to collection - only store what we need
		files.push(json!({
			"path": path_display,
			"content": content_with_numbers,
			"lines": lines.len(),
			"size": metadata.len(),
			"lang": detect_language(ext),
		}));

		total_size += metadata.len();
	}

	// Create optimized result
	Ok(McpToolResult {
		tool_name: "text_editor".to_string(),
		tool_id: call.tool_id.clone(),
		result: json!({
			"success": !files.is_empty(),
			"files": files,
			"count": files.len(),
			"total_size": total_size,
			"failed": failures,
		}),
	})
}

// View multiple files simultaneously with optimized token usage
pub async fn view_many_files(call: &McpToolCall, paths: &[String]) -> Result<McpToolResult> {
	let mut files = Vec::with_capacity(paths.len());
	let mut failures = Vec::new();
	let mut total_size = 0u64;

	// Process each file in the list with efficient memory usage
	for path_str in paths {
		let path = Path::new(&path_str);
		let path_display = path.display().to_string();

		// Check if file exists and is a regular file
		if !path.exists() {
			failures.push(format!("File does not exist: {}", path_display));
			continue;
		}

		if !path.is_file() {
			failures.push(format!("Not a regular file: {}", path_display));
			continue;
		}

		// Check file size - avoid loading very large files
		let metadata = match tokio_fs::metadata(path).await {
			Ok(meta) => {
				if meta.len() > 1024 * 1024 * 5 { // 5MB limit
					failures.push(format!("File too large (>5MB): {}", path_display));
					continue;
				}
				meta
			},
			Err(e) => {
				failures.push(format!("Cannot read metadata for {}: {}", path_display, e));
				continue;
			}
		};

		// Check if file is binary
		if let Ok(sample) = tokio_fs::read(&path).await {
			let sample_size = sample.len().min(512);
			let null_count = sample[..sample_size].iter().filter(|&&b| b == 0).count();
			if null_count > sample_size / 10 {
				failures.push(format!("Binary file skipped: {}", path_display));
				continue;
			}
		}

		// Read file content with error handling
		let content = match tokio_fs::read_to_string(path).await {
			Ok(content) => content,
			Err(e) => {
				failures.push(format!("Cannot read content of {}: {}", path_display, e));
				continue;
			}
		};

		// Get language from extension for syntax highlighting
		let ext = path.extension()
			.and_then(|e| e.to_str())
			.unwrap_or("");

		// Add line numbers to content
		let lines: Vec<&str> = content.lines().collect();
		let content_with_numbers = lines
			.iter()
			.enumerate()
			.map(|(i, line)| format!("{}: {}", i + 1, line))
			.collect::<Vec<_>>()
			.join("\n");

		// Add file info to collection - only store what we need
		files.push(json!({
			"path": path_display,
			"content": content_with_numbers,
			"lines": lines.len(),
			"size": metadata.len(),
			"lang": detect_language(ext),
		}));

		total_size += metadata.len();
	}

	// Create optimized result
	Ok(McpToolResult {
		tool_name: "view_many".to_string(),
		tool_id: call.tool_id.clone(),
		result: json!({
			"success": !files.is_empty(),
			"files": files,
			"count": files.len(),
			"total_size": total_size,
			"failed": failures,
		}),
	})
}