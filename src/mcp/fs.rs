// File System MCP provider - extracted from dev.rs for better modularity
// Handles file operations and HTML to Markdown conversion

use std::path::Path;
use std::process::Command;
use std::collections::HashMap;
use std::sync::Mutex;
use serde_json::{json, Value};
use anyhow::{Result, anyhow};
use tokio::fs as tokio_fs;
use lazy_static::lazy_static;
use super::{McpToolCall, McpToolResult, McpFunction};
use html5ever::parse_document;
use html5ever::tendril::TendrilSink;
use markup5ever_rcdom::{Handle, NodeData, RcDom};
use reqwest;
use url::Url;

// Thread-safe lazy initialization of file history using lazy_static
lazy_static! {
	static ref FILE_HISTORY: Mutex<HashMap<String, Vec<String>>> = Mutex::new(HashMap::new());
}

// Thread-safe way to get the file history
fn get_file_history() -> &'static Mutex<HashMap<String, Vec<String>>> {
	&FILE_HISTORY
}

// Define the list_files function
pub fn get_list_files_function() -> McpFunction {
	McpFunction {
		name: "list_files".to_string(),
		description: "List files in a directory, with optional pattern matching.

			This tool uses ripgrep for efficient searching that respects .gitignore files.
			You can use it to find files by name pattern or search for files containing specific content.

			**⚠️ PERFORMANCE WARNING: Use filtering to avoid large outputs that consume excessive tokens**

			**Parameters:**
			- `directory`: Target directory to search
			- `pattern`: Optional filename pattern (uses ripgrep syntax)
			- `content`: Optional content search within files
			- `max_depth`: Optional depth limit for directory traversal

			**🎯 Best Practices:**
			1. **Always use specific patterns** - avoid listing entire large directories
			2. **Use max_depth** to limit scope and reduce token usage
			3. **Combine with content search** when looking for specific functionality
			4. **Filter by file type** using patterns like '\\*.rs' or '\\*.toml'

			**Examples:**
			- Find Rust files: `{\"directory\": \"src\", \"pattern\": \"\\*.rs\"}`
			- Find config files: `{\"directory\": \".\", \"pattern\": \"\\*.toml|\\*.yaml|\\*.json\"}`
			- Search for function: `{\"directory\": \"src\", \"content\": \"fn main\"}`
			- Limited depth: `{\"directory\": \".\", \"max_depth\": 2, \"pattern\": \"\\*.rs\"}`

			**Token-Efficient Usage:**
			- Use patterns to target specific file types
			- Set max_depth to avoid deep directory traversals
			- Combine with content search for targeted results
			- Prefer multiple specific calls over one broad search".to_string(),
		parameters: json!({
			"type": "object",
			"required": ["directory"],
			"properties": {
				"directory": {
					"type": "string",
					"description": "The directory to list files from"
				},
				"pattern": {
					"type": "string",
					"description": "Optional pattern to match filenames (uses ripgrep)"
				},
				"content": {
					"type": "string",
					"description": "Optional content to search for in files (uses ripgrep)"
				},
				"max_depth": {
					"type": "integer",
					"description": "Maximum depth of directories to descend (default: no limit)"
				}
			}
		}),
	}
}
// Undo the last edit to a file
async fn undo_edit(call: &McpToolCall, path: &Path) -> Result<McpToolResult> {
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

// View multiple files at once with optimized token usage
async fn view_multiple_files(call: &McpToolCall, paths: &[String]) -> Result<McpToolResult> {
	let mut files = Vec::with_capacity(paths.len());
	let mut failures = Vec::new();

	// Process each file in the list with efficient memory usage
	for path_str in paths {
		let path = Path::new(&path_str);
		let path_display = path.display().to_string();

		// Check if file exists and is a regular file
		if !path.exists() || !path.is_file() {
			failures.push(format!("Not a valid file: {}", path_display));
			continue;
		}

		// Check file size - avoid loading very large files
		let metadata = match tokio_fs::metadata(path).await {
			Ok(meta) => {
				if meta.len() > 1024 * 1024 * 5 { // 5MB limit
					failures.push(format!("File too large: {}", path_display));
					continue;
				}
				meta
			},
			Err(_) => {
				failures.push(format!("Cannot read: {}", path_display));
				continue;
			}
		};

		// Read file content with error handling
		let content = match tokio_fs::read_to_string(path).await {
			Ok(content) => content,
			Err(_) => {
				failures.push(format!("Cannot read content: {}", path_display));
				continue;
			}
		};

		// Get language from extension for syntax highlighting
		let ext = path.extension()
			.and_then(|e| e.to_str())
			.unwrap_or("");

		// Add file info to collection - only store what we need
		files.push(json!({
			"path": path_display,
			"content": content,
			"lang": detect_language(ext),
			"size": metadata.len(),
		}));
	}

	// Create optimized result
	Ok(McpToolResult {
		tool_name: "text_editor".to_string(),
		tool_id: call.tool_id.clone(),
		result: json!({
			"success": !files.is_empty(),
			"files": files,
			"count": files.len(),
			"failed": failures,
		}),
	})
}

// Save the current content of a file for undo
async fn save_file_history(path: &Path) -> Result<()> {
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

// Define the line_replace function for line-based editing
pub fn get_line_replace_function() -> McpFunction {
	McpFunction {
		name: "line_replace".to_string(),
		description: "Replace content at specific line ranges in files, optimized for multi-file operations.

			This tool performs line-based replacements by specifying start and end line positions, making it ideal for precise code modifications without needing exact string matching.

			**Key Benefits:**
			- Line-based targeting eliminates string matching issues
			- Supports multi-file operations for consistent changes across codebase
			- Validates line ranges exist before making changes
			- Preserves file structure and formatting outside the target range

			**Parameters:**
			`path`: Single file path string or array of file paths
			`start_line`: Starting line number(s) - 1-indexed, can be single number or array
			`end_line`: Ending line number(s) - 1-indexed, inclusive, can be single number or array  
			`content`: New content to place at the specified line range(s)

			**Single File Usage:**
			```
			{
			\"path\": \"src/main.rs\",
			\"start_line\": 5,
			\"end_line\": 8,
			\"content\": \"fn new_function() {\\n    // New implementation\\n}\"
			}
			```
			Replaces lines 5-8 (inclusive) with the new content.

			**Multiple Files Usage (Recommended):**
			```
			{
			\"path\": [\"src/lib.rs\", \"src/main.rs\", \"tests/test.rs\"],
			\"start_line\": [10, 15, 5],
			\"end_line\": [12, 18, 7],
			\"content\": [
				\"pub struct NewStruct {\\n    field: String,\\n}\",
				\"fn updated_main() {\\n    println!(\\\"Updated\\\");\\n}\",
				\"#[test]\\nfn new_test() {\\n    assert!(true);\\n}\"
			]
			}
			```
			All arrays (paths, start_lines, end_lines, contents) must have equal length.

			**Line Numbering:**
			- Lines are 1-indexed (first line is line 1)
			- end_line is inclusive (line range includes both start and end)
			- If start_line == end_line, replaces single line
			- Content can span multiple lines using \\n characters

			**Validation:**
			- Verifies all files exist and are readable
			- Validates line ranges exist in each file
			- Ensures start_line <= end_line for each operation
			- Saves file history for undo operations

			**Best Practices:**
			1. **Use for precise code modifications** when you know exact line numbers
			2. **Prefer multi-file operations** for consistent changes across files
			3. **Combine with list_files or text_editor view** to identify target lines
			4. **Consider line shifts** when making multiple changes to the same file
			5. **Use \\n for multi-line content** to maintain proper formatting

			This tool is particularly effective for:
			- Updating function implementations at known locations
			- Replacing class definitions or struct declarations
			- Modifying configuration blocks
			- Updating import statements or dependencies
			- Making consistent changes across multiple similar files".to_string(),
		parameters: json!({
			"type": "object",
			"required": ["path", "start_line", "end_line", "content"],
			"properties": {
				"path": {
					"description": "Absolute path to file(s). Can be a single path string or an array of path strings.",
					"oneOf": [
						{
							"type": "string"
						},
						{
							"type": "array",
							"items": {
								"type": "string"
							}
						}
					]
				},
				"start_line": {
					"description": "Starting line number(s) - 1-indexed. Can be a single integer or an array of integers matching path array length.",
					"oneOf": [
						{"type": "integer", "minimum": 1},
						{
							"type": "array",
							"items": {"type": "integer", "minimum": 1}
						}
					]
				},
				"end_line": {
					"description": "Ending line number(s) - 1-indexed, inclusive. Can be a single integer or an array of integers matching path and start_line arrays.",
					"oneOf": [
						{"type": "integer", "minimum": 1},
						{
							"type": "array",
							"items": {"type": "integer", "minimum": 1}
						}
					]
				},
				"content": {
					"description": "New content to place at the specified line range(s). Can be a string for single file or an array of strings matching other arrays.",
					"oneOf": [
						{"type": "string"},
						{
							"type": "array",
							"items": {"type": "string"}
						}
					]
				}
			}
		}),
	}
}

// Define the text editor function for modifying files
pub fn get_text_editor_function() -> McpFunction {
	McpFunction {
		name: "text_editor".to_string(),
		description: "Perform text editing operations on files, optimized for multi-file operations.

			The `command` parameter specifies the operation to perform on the file system. This tool is specifically designed to handle batch operations across multiple files when possible, improving efficiency and consistency.

			**⚠️ IMPORTANT: Always prefer multi-file operations over single calls to reduce token usage and improve performance**

			Available Commands:

			`view`: Examine content of one or multiple files simultaneously
			- **Single file**: `{\"command\": \"view\", \"path\": \"src/main.rs\"}`
			- **🔥 Multiple files** (strongly recommended): `{\"command\": \"view\", \"path\": [\"src/main.rs\", \"src/lib.rs\", \"Cargo.toml\"]}`
			- Returns content of all requested files for comprehensive analysis or reference
			- **Best practice**: Batch related files together for context analysis

			`write`: Create or overwrite one or multiple files in a single operation
			- **Single file**: `{\"command\": \"write\", \"path\": \"src/main.rs\", \"file_text\": \"fn main() {...}\"}`
			- **🔥 Multiple files** (strongly recommended):
			```
			{
			\"command\": \"write\",
			\"path\": [\"src/models.rs\", \"src/views.rs\", \"tests/integration.rs\"],
			\"file_text\": [\"pub struct Model {...}\", \"pub fn render() {...}\", \"#[test] fn test() {...}\"]
			}
			```
			- **Best practice**: Create entire modules or related files together to maintain consistency

			`str_replace`: Perform text replacement across one or multiple files
			- **Single file**:
			```
			{
			\"command\": \"str_replace\",
			\"path\": \"src/lib.rs\",
			\"old_str\": \"fn old_name()\",
			\"new_str\": \"fn new_name()\"
			}
			```
			- **🔥 Multiple files** (strongly recommended):
			```
			{
			\"command\": \"str_replace\",
			\"path\": [\"src/lib.rs\", \"src/main.rs\", \"tests/unit.rs\"],
			\"old_str\": [\"struct OldName\", \"use crate::OldName\", \"OldName::new()\"],
			\"new_str\": [\"struct NewName\", \"use crate::NewName\", \"NewName::new()\"]
			}
			```
			- **Best practice**: Rename/refactor across all affected files in a single operation
			- The `old_str` must exactly match text in the file, including whitespace

			`undo_edit`: Revert the most recent edit made to a specified file
			- `{\"command\": \"undo_edit\", \"path\": \"src/main.rs\"}`
			- Useful for rolling back changes when needed

			**🎯 Performance Guidelines:**
			1. **Always batch related files** - view, write, or modify related files together
			2. **Avoid repeated single-file calls** - combine operations when possible
			3. **Use arrays consistently** - paths, old_strs, new_strs, file_texts must have equal length
			4. **Plan comprehensive changes** - think about all files affected by a change
			5. **Prefer fewer, larger operations** over many small ones

			**💡 Common Multi-File Patterns:**
			- **Module creation**: Write multiple related .rs files together
			- **Refactoring**: Update imports, function names, types across affected files
			- **Configuration updates**: Modify related config files simultaneously
			- **Test updates**: Update source and corresponding test files together

			This tool enables efficient code management across multiple files, supporting comprehensive refactoring and codebase-wide changes with minimal token usage.".to_string(),
		parameters: json!({
			"type": "object",
			"required": ["command", "path"],
			"properties": {
				"path": {
					"description": "Absolute path to file(s). Can be a single path string or an array of path strings.",
					"oneOf": [
						{
							"type": "string"
						},
						{
							"type": "array",
							"items": {
								"type": "string"
							}
						}
					]
				},
				"command": {
					"type": "string",
					"enum": ["view", "write", "str_replace", "undo_edit"],
					"description": "Allowed options are: `view`, `write`, `str_replace`, undo_edit`.",
				},
				"old_str": {
					"description": "String(s) to replace. Can be a single string or an array of strings matching the path array length.",
					"oneOf": [
						{"type": "string"},
						{
							"type": "array",
							"items": {"type": "string"}
						}
					]
				},
				"new_str": {
					"description": "Replacement string(s). Can be a single string or an array of strings matching paths and old_strs.",
					"oneOf": [
						{"type": "string"},
						{
							"type": "array",
							"items": {"type": "string"}
						}
					]
				},
				"file_text": {
					"description": "Content to write to file(s). Can be a string for a single file or an array of strings matching the path array length.",
					"oneOf": [
						{"type": "string"},
						{
							"type": "array",
							"items": {"type": "string"}
						}
					]
				}
			}
		}),
	}
}

// Define the HTML to Markdown conversion function
pub fn get_html2md_function() -> McpFunction {
	McpFunction {
		name: "html2md".to_string(),
		description: "Convert HTML content to Markdown format from URLs or local files.

			This tool converts HTML content from web URLs or local HTML files to clean, readable Markdown.
			It's particularly useful for:
			- Reading documentation from websites in a more consumable format
			- Converting HTML files to Markdown for easier analysis
			- Processing web content without dealing with HTML tags

			The tool automatically detects whether the input is a URL or file path and:
			- Fetches content from URLs and converts to Markdown
			- Reads local HTML files and converts to Markdown
			- Handles proper conversion of HTML elements to Markdown equivalents
			- Cleans up whitespace and formatting
			- Preserves document structure and readability

			Supports multiple inputs for batch processing:
			- Single input: `{\"sources\": \"https://example.com/docs\"}`
			- Multiple inputs: `{\"sources\": [\"./docs/index.html\", \"https://example.com/api\"]}`

			Output is clean Markdown that preserves the document structure and readability.".to_string(),
		parameters: json!({
			"type": "object",
			"required": ["sources"],
			"properties": {
				"sources": {
					"description": "URL(s) or file path(s) to convert from HTML format to Markdown. Can be a single string or an array of strings.",
					"oneOf": [
						{
							"type": "string"
						},
						{
							"type": "array",
							"items": {
								"type": "string"
							}
						}
					]
				}
			}
		}),
	}
}


// Execute a line replace command
pub async fn execute_line_replace(call: &McpToolCall) -> Result<McpToolResult> {
	// Extract path parameter
	let path_value = match call.parameters.get("path") {
		Some(value) => value,
		_ => return Err(anyhow!("Missing 'path' parameter")),
	};

	// Extract start_line parameter
	let start_line_value = match call.parameters.get("start_line") {
		Some(value) => value,
		_ => return Err(anyhow!("Missing 'start_line' parameter")),
	};

	// Extract end_line parameter
	let end_line_value = match call.parameters.get("end_line") {
		Some(value) => value,
		_ => return Err(anyhow!("Missing 'end_line' parameter")),
	};

	// Extract content parameter
	let content_value = match call.parameters.get("content") {
		Some(value) => value,
		_ => return Err(anyhow!("Missing 'content' parameter")),
	};

	// Execute the appropriate command based on parameter types
	match path_value {
		Value::String(p) => {
			// Single file replacement
			let start_line = match start_line_value.as_u64() {
				Some(n) => n as usize,
				_ => return Err(anyhow!("Invalid 'start_line' parameter, must be a positive integer")),
			};
			let end_line = match end_line_value.as_u64() {
				Some(n) => n as usize,
				_ => return Err(anyhow!("Invalid 'end_line' parameter, must be a positive integer")),
			};
			let content = match content_value.as_str() {
				Some(s) => s,
				_ => return Err(anyhow!("Invalid 'content' parameter, must be a string")),
			};
			
			line_replace_single_file(call, Path::new(p), start_line, end_line, content).await
		},
		Value::Array(paths) => {
			// Multiple files replacement
			let start_lines_array = match start_line_value.as_array() {
				Some(arr) => arr,
				_ => return Err(anyhow!("'start_line' must be an array for multiple file operations")),
			};

			let end_lines_array = match end_line_value.as_array() {
				Some(arr) => arr,
				_ => return Err(anyhow!("'end_line' must be an array for multiple file operations")),
			};

			let contents_array = match content_value.as_array() {
				Some(arr) => arr,
				_ => return Err(anyhow!("'content' must be an array for multiple file operations")),
			};

			// Convert arrays to proper types
			let path_strings: Result<Vec<String>, _> = paths.iter()
				.map(|p| p.as_str().ok_or_else(|| anyhow!("Invalid path in array")))
				.map(|r| r.map(|s| s.to_string()))
				.collect();

			let start_lines: Result<Vec<usize>, _> = start_lines_array.iter()
				.map(|n| n.as_u64().ok_or_else(|| anyhow!("Invalid start_line in array")))
				.map(|r| r.map(|n| n as usize))
				.collect();

			let end_lines: Result<Vec<usize>, _> = end_lines_array.iter()
				.map(|n| n.as_u64().ok_or_else(|| anyhow!("Invalid end_line in array")))
				.map(|r| r.map(|n| n as usize))
				.collect();

			let contents: Result<Vec<String>, _> = contents_array.iter()
				.map(|s| s.as_str().ok_or_else(|| anyhow!("Invalid content in array")))
				.map(|r| r.map(|s| s.to_string()))
				.collect();

			match (path_strings, start_lines, end_lines, contents) {
				(Ok(paths), Ok(start_lines), Ok(end_lines), Ok(contents)) => {
					line_replace_multiple_files(call, &paths, &start_lines, &end_lines, &contents).await
				},
				_ => Err(anyhow!("Invalid arrays in parameters")),
			}
		},
		_ => Err(anyhow!("'path' parameter must be a string or array of strings")),
	}
}

// Execute a text editor command
pub async fn execute_text_editor(call: &McpToolCall) -> Result<McpToolResult> {
	// Extract command parameter
	let command = match call.parameters.get("command") {
		Some(Value::String(cmd)) => cmd.clone(),
		_ => return Err(anyhow!("Missing or invalid 'command' parameter")),
	};

	// Extract path parameter
	let path_value = match call.parameters.get("path") {
		Some(value) => value,
		_ => return Err(anyhow!("Missing 'path' parameter")),
	};

	// Execute the appropriate command
	match command.as_str() {
		"view" => {
			// Support either a single path string or an array of paths
			match path_value {
				Value::String(p) => {
					// Single file view
					view_file(call, Path::new(p)).await
				},
				Value::Array(paths) => {
					// Multiple files view
					let path_strings: Result<Vec<String>, _> = paths.iter()
						.map(|p| p.as_str().ok_or_else(|| anyhow!("Invalid path in array")))
						.map(|r| r.map(|s| s.to_string()))
						.collect();

					match path_strings {
						Ok(path_strs) => view_multiple_files(call, &path_strs).await,
						Err(e) => Err(e),
					}
				},
				_ => Err(anyhow!("'path' parameter must be a string or array of strings")),
			}
		},
		"write" => {
			// Support either a single path/content or arrays for multiple files
			match path_value {
				Value::String(p) => {
					// Single file write
					let file_text = match call.parameters.get("file_text") {
						Some(Value::String(txt)) => txt.clone(),
						_ => return Err(anyhow!("Missing or invalid 'file_text' parameter for single file write")),
					};
					write_file(call, Path::new(p), &file_text).await
				},
				Value::Array(paths) => {
					// Multiple files write
					let file_text_value = match call.parameters.get("file_text") {
						Some(value) => value,
						_ => return Err(anyhow!("Missing 'file_text' parameter for write operations")),
					};

					match file_text_value {
						Value::Array(contents) => {
							// Convert path and content arrays to strings
							let path_strings: Result<Vec<String>, _> = paths.iter()
								.map(|p| p.as_str().ok_or_else(|| anyhow!("Invalid path in array")))
								.map(|r| r.map(|s| s.to_string()))
								.collect();

							let content_strings: Result<Vec<String>, _> = contents.iter()
								.map(|c| c.as_str().ok_or_else(|| anyhow!("Invalid content in array")))
								.map(|r| r.map(|s| s.to_string()))
								.collect();

							match (path_strings, content_strings) {
								(Ok(paths), Ok(contents)) => write_multiple_files(call, &paths, &contents).await,
								(Err(e), _) | (_, Err(e)) => Err(e),
							}
						},
						_ => Err(anyhow!("'file_text' must be an array for multiple file writes")),
					}
				},
				_ => Err(anyhow!("'path' parameter must be a string or array of strings")),
			}
		},
		"str_replace" => {
			// Support either a single path or arrays for multiple files
			match path_value {
				Value::String(p) => {
					// Single file replacement
					let old_str = match call.parameters.get("old_str") {
						Some(Value::String(s)) => s.clone(),
						_ => return Err(anyhow!("Missing or invalid 'old_str' parameter")),
					};
					let new_str = match call.parameters.get("new_str") {
						Some(Value::String(s)) => s.clone(),
						_ => return Err(anyhow!("Missing or invalid 'new_str' parameter")),
					};
					replace_string_in_file(call, Path::new(p), &old_str, &new_str).await
				},
				Value::Array(paths) => {
					// Multiple files replacement - preferred method
					let old_str_value = match call.parameters.get("old_str") {
						Some(value) => value,
						_ => return Err(anyhow!("Missing 'old_str' parameter for str_replace operations")),
					};

					let new_str_value = match call.parameters.get("new_str") {
						Some(value) => value,
						_ => return Err(anyhow!("Missing 'new_str' parameter for str_replace operations")),
					};

					match (old_str_value, new_str_value) {
						(Value::Array(old_strs), Value::Array(new_strs)) => {
							// Convert arrays to strings
							let path_strings: Result<Vec<String>, _> = paths.iter()
								.map(|p| p.as_str().ok_or_else(|| anyhow!("Invalid path in array")))
								.map(|r| r.map(|s| s.to_string()))
								.collect();

							let old_str_strings: Result<Vec<String>, _> = old_strs.iter()
								.map(|s| s.as_str().ok_or_else(|| anyhow!("Invalid string in old_str array")))
								.map(|r| r.map(|s| s.to_string()))
								.collect();

							let new_str_strings: Result<Vec<String>, _> = new_strs.iter()
								.map(|s| s.as_str().ok_or_else(|| anyhow!("Invalid string in new_str array")))
								.map(|r| r.map(|s| s.to_string()))
								.collect();

							match (path_strings, old_str_strings, new_str_strings) {
								(Ok(paths), Ok(old_strs), Ok(new_strs)) => {
									str_replace_multiple(call, &paths, &old_strs, &new_strs).await
								},
								_ => Err(anyhow!("Invalid strings in arrays")),
							}
						},
						_ => Err(anyhow!("Both 'old_str' and 'new_str' must be arrays for multiple file replacements")),
					}
				},
				_ => Err(anyhow!("'path' parameter must be a string or array of strings")),
			}
		},
		"undo_edit" => {
			let path = match path_value.as_str() {
				Some(p) => p,
				_ => return Err(anyhow!("'path' parameter must be a string for undo_edit operations")),
			};

			undo_edit(call, Path::new(path)).await
		},
		_ => Err(anyhow!("Invalid command: {}", command)),
	}
}

// Execute list_files command
pub async fn execute_list_files(call: &McpToolCall) -> Result<McpToolResult> {
	// Extract directory parameter
	let directory = match call.parameters.get("directory") {
		Some(Value::String(dir)) => dir.clone(),
		_ => return Err(anyhow!("Missing or invalid 'directory' parameter")),
	};

	// Extract optional parameters
	let pattern = call.parameters.get("pattern")
		.and_then(|v| v.as_str())
		.map(|s| s.to_string());

	let content = call.parameters.get("content")
		.and_then(|v| v.as_str())
		.map(|s| s.to_string());

	let max_depth = call.parameters.get("max_depth")
		.and_then(|v| v.as_u64())
		.map(|n| n as usize);

	// Build the ripgrep command based on the parameters
	let mut cmd_args = Vec::new();

	if let Some(depth) = max_depth {
		cmd_args.push(format!("--max-depth {}", depth));
	}

	// Search for content in files or list files matching pattern
	let (cmd, output_type) = if let Some(ref content_pattern) = content {
		(
			format!("cd '{}' && rg '{}' {}", directory, content_pattern, cmd_args.join(" ")),
			"content search"
		)
	} else if let Some(ref name_pattern) = pattern {
		(
			format!("cd '{}' && rg --files {} | rg '{}'", directory, cmd_args.join(" "), name_pattern),
			"filename pattern"
		)
	} else {
		// Default: list all files using ripgrep
		(
			format!("cd '{}' && rg --files {}", directory, cmd_args.join(" ")),
			"file listing"
		)
	};

	// Execute the command
	let output = tokio::task::spawn_blocking(move || {
		let output = if cfg!(target_os = "windows") {
			Command::new("cmd")
				.args(["/C", &cmd])
				.output()
		} else {
			Command::new("sh")
				.args(["-c", &cmd])
				.output()
		};

		match output {
			Ok(output) => {
				let stdout = String::from_utf8_lossy(&output.stdout).to_string();
				let stderr = String::from_utf8_lossy(&output.stderr).to_string();

				// Parse the output into a list of files
				let files: Vec<&str> = stdout.lines().collect();
				let output_str = if stdout.is_empty() && !stderr.is_empty() {
					stderr
				} else {
					stdout.clone()
				};

				json!({
					"success": output.status.success(),
					"output": output_str,
					"files": files,
					"count": files.len(),
					"type": output_type,
					"parameters": {
					"directory": directory,
					"pattern": pattern,
					"content": content,
					"max_depth": max_depth
				}
			})
			},
			Err(e) => json!({
				"success": false,
				"output": format!("Failed to list files: {}", e),
				"files": [],
				"count": 0,
				"parameters": {
					"directory": directory,
					"pattern": pattern,
					"content": content,
					"max_depth": max_depth
				}
			}),
		}
	}).await?;

	Ok(McpToolResult {
		tool_name: "list_files".to_string(),
		tool_id: call.tool_id.clone(),
		result: output,
	})
}

// Get all available filesystem functions
pub fn get_all_functions() -> Vec<McpFunction> {
	vec![
		get_text_editor_function(),
		get_line_replace_function(),
		get_list_files_function(),
		get_html2md_function(),
	]
}

// Helper function to detect language based on file extension
fn detect_language(ext: &str) -> &str {
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

// View the content of a file - optimized for token usage
async fn view_file(call: &McpToolCall, path: &Path) -> Result<McpToolResult> {
	if !path.exists() {
		return Err(anyhow!("File does not exist: {}", path.display()));
	}

	if !path.is_file() {
		return Err(anyhow!("Path is not a file: {}", path.display()));
	}

	// Check file size to avoid loading very large files
	let metadata = tokio_fs::metadata(path).await?;
	if metadata.len() > 1024 * 1024 * 5 {  // 5MB limit
		return Err(anyhow!("File is too large (>5MB): {}", path.display()));
	}

	// Read the file content
	let content = tokio_fs::read_to_string(path).await?;

	// Detect file type/language for syntax highlighting
	let file_ext = path.extension()
		.and_then(|e| e.to_str())
		.unwrap_or("");

	// Create result
	let result = McpToolResult {
		tool_name: "text_editor".to_string(),
		tool_id: call.tool_id.clone(),
		result: json!({
			"success": true,
			"files": [{
				"path": path.to_string_lossy(),
				"content": content,
				"lang": detect_language(file_ext),
				"size": metadata.len(),
			}],
			"count": 1
		}),
	};

	// Return a single file in the same format as multiple files for consistency
	Ok(result)
}

// Write content to a single file
async fn write_file(call: &McpToolCall, path: &Path, content: &str) -> Result<McpToolResult> {
	// Save the current content for undo if the file exists
	if path.exists() {
		save_file_history(path).await?;
	}

	// Create parent directories if they don't exist
	if let Some(parent) = path.parent() {
		if !parent.exists() {
			tokio_fs::create_dir_all(parent).await?;
		}
	}

	// Write the content to the file
	tokio_fs::write(path, content).await?;

	// Create result
	let result = McpToolResult {
		tool_name: "text_editor".to_string(),
		tool_id: call.tool_id.clone(),
		result: json!({
			"success": true,
			"files": [{
				"path": path.to_string_lossy(),
				"success": true,
				"size": content.len()
			}],
			"count": 1
		}),
	};

	// Return success in the same format as multiple file write for consistency
	Ok(result)
}

// Write content to multiple files
async fn write_multiple_files(call: &McpToolCall, paths: &[String], contents: &[String]) -> Result<McpToolResult> {
	let mut results = Vec::with_capacity(paths.len());
	let mut failures = Vec::new();

	// Ensure paths and contents match in length
	if paths.len() != contents.len() {
		return Err(anyhow!(
			"Mismatch in path and content arrays. Expected {} paths and {} contents to match.",
			paths.len(), contents.len()
		));
	}

	// Process each file in the list
	for (idx, path_str) in paths.iter().enumerate() {
		let path = Path::new(path_str);
		let content = &contents[idx];
		let path_display = path.display().to_string();

		// Try to save history for undo if the file exists
		if path.exists() {
			if let Err(e) = save_file_history(path).await {
				failures.push(format!("Failed to save history for {}: {}", path_display, e));
				// But continue with the write operation
			}
		}

		// Create parent directories if needed
		if let Some(parent) = path.parent() {
			if !parent.exists() {
				if let Err(e) = tokio_fs::create_dir_all(parent).await {
					failures.push(format!("Failed to create directories for {}: {}", path_display, e));
					continue; // Skip this file if we can't create the directory
				}
			}
		}

		// Write the content to the file
		match tokio_fs::write(path, content).await {
			Ok(_) => {
				results.push(json!({
					"path": path_display,
					"success": true,
					"size": content.len()
				}));
			},
			Err(e) => {
				failures.push(format!("Failed to write to {}: {}", path_display, e));
			}
		};
	}

	// Return success if at least one file was written
	Ok(McpToolResult {
		tool_name: "text_editor".to_string(),
		tool_id: call.tool_id.clone(),
		result: json!({
			"success": !results.is_empty(),
			"files": results,
			"count": results.len(),
			"failed": failures
		}),
	})
}

// Replace lines in a single file
async fn line_replace_single_file(call: &McpToolCall, path: &Path, start_line: usize, end_line: usize, content: &str) -> Result<McpToolResult> {
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
async fn line_replace_multiple_files(call: &McpToolCall, paths: &[String], start_lines: &[usize], end_lines: &[usize], contents: &[String]) -> Result<McpToolResult> {
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

// Replace a string in a single file - optimized format for consistency
async fn replace_string_in_file(call: &McpToolCall, path: &Path, old_str: &str, new_str: &str) -> Result<McpToolResult> {
	if !path.exists() {
		return Err(anyhow!("File does not exist: {}", path.display()));
	}

	// Read the file content
	let content = tokio_fs::read_to_string(path).await?;

	// Check if old_str appears exactly once
	let occurrences = content.matches(old_str).count();
	if occurrences == 0 {
		return Err(anyhow!("The string to replace does not exist in the file"));
	}
	if occurrences > 1 {
		return Err(anyhow!("The string to replace appears {} times in the file. It should appear exactly once.", occurrences));
	}

	// Save the current content for undo
	save_file_history(path).await?;

	// Replace the string
	let new_content = content.replace(old_str, new_str);

	// Write the new content
	tokio_fs::write(path, new_content).await?;

	// Return success with context
	let position = content.find(old_str).unwrap();
	let start_line = content[..position].matches('\n').count() + 1;

	// Return in the same format as multiple replacements for consistency
	Ok(McpToolResult {
		tool_name: "text_editor".to_string(),
		tool_id: call.tool_id.clone(),
		result: json!({
			"success": true,
			"files": [{
				"path": path.to_string_lossy(),
				"success": true,
				"line": start_line,
				"old_size": old_str.len(),
				"new_size": new_str.len()
			}],
			"count": 1
		}),
	})
}

// Replace strings in multiple files
async fn str_replace_multiple(call: &McpToolCall, paths: &[String], old_strs: &[String], new_strs: &[String]) -> Result<McpToolResult> {
	let mut results = Vec::with_capacity(paths.len());
	let mut failures = Vec::new();

	// Ensure all arrays have matching length
	if paths.len() != old_strs.len() || paths.len() != new_strs.len() {
		return Err(anyhow!(
			"Mismatch in array lengths. Expected {} paths, {} old strings, and {} new strings to all match.",
			paths.len(), old_strs.len(), new_strs.len()
		));
	}

	// Process each file replacement
	for (idx, path_str) in paths.iter().enumerate() {
		let path = Path::new(path_str);
		let old_str = &old_strs[idx];
		let new_str = &new_strs[idx];
		let path_display = path.display().to_string();

		// Check if file exists
		if !path.exists() {
			failures.push(format!("File does not exist: {}", path_display));
			continue;
		}

		// Try to read the file content
		let content = match tokio_fs::read_to_string(path).await {
			Ok(content) => content,
			Err(e) => {
				failures.push(format!("Failed to read {}: {}", path_display, e));
				continue;
			}
		};

		// Check if old_str appears exactly once
		let occurrences = content.matches(old_str).count();
		if occurrences == 0 {
			failures.push(format!("String to replace does not exist in {}", path_display));
			continue;
		}
		if occurrences > 1 {
			failures.push(format!(
				"String appears {} times in {}. It should appear exactly once.",
				occurrences, path_display
			));
			continue;
		}

		// Try to save history for undo
		if let Err(e) = save_file_history(path).await {
			failures.push(format!("Failed to save history for {}: {}", path_display, e));
			// But continue with the replacement operation
		}

		// Replace the string
		let new_content = content.replace(old_str, new_str);

		// Write the new content
		match tokio_fs::write(path, new_content).await {
			Ok(_) => {
				// Find line number for reporting
				let position = content.find(old_str).unwrap(); // Safe because we checked occurrences
				let start_line = content[..position].matches('\n').count() + 1;

				results.push(json!({
					"path": path_display,
					"success": true,
					"line": start_line,
					"old_size": old_str.len(),
					"new_size": new_str.len()
				}));
			},
			Err(e) => {
				failures.push(format!("Failed to write to {}: {}", path_display, e));
			}
		};
	}

	// Return success if at least one file was modified
	Ok(McpToolResult {
		tool_name: "text_editor".to_string(),
		tool_id: call.tool_id.clone(),
		result: json!({
			"success": !results.is_empty(),
			"files": results,
			"count": results.len(),
			"failed": failures
		}),
	})
}

// Execute HTML to Markdown conversion
pub async fn execute_html2md(call: &McpToolCall) -> Result<McpToolResult> {
	// Extract sources parameter
	let sources_value = match call.parameters.get("sources") {
		Some(value) => value,
		_ => return Err(anyhow!("Missing 'sources' parameter")),
	};

	// Support either a single source string or an array of sources
	match sources_value {
		Value::String(source) => {
			// Single source conversion
			convert_single_html_to_md(call, source).await
		},
		Value::Array(sources) => {
			// Multiple sources conversion
			let source_strings: Result<Vec<String>, _> = sources.iter()
				.map(|s| s.as_str().ok_or_else(|| anyhow!("Invalid source in array")))
				.map(|r| r.map(|s| s.to_string()))
				.collect();

			match source_strings {
				Ok(source_strs) => convert_multiple_html_to_md(call, &source_strs).await,
				Err(e) => Err(e),
			}
		},
		_ => Err(anyhow!("'sources' parameter must be a string or array of strings")),
	}
}

// Convert a single HTML source to Markdown
async fn convert_single_html_to_md(call: &McpToolCall, source: &str) -> Result<McpToolResult> {
	let (html_content, source_type) = fetch_html_content(source).await?;
	let markdown = html_to_markdown(&html_content)?;

	Ok(McpToolResult {
		tool_name: "html2md".to_string(),
		tool_id: call.tool_id.clone(),
		result: json!({
			"success": true,
			"conversions": [{
				"source": source,
				"type": source_type,
				"markdown": markdown,
				"size": markdown.len()
			}],
			"count": 1
		}),
	})
}

// Convert multiple HTML sources to Markdown
async fn convert_multiple_html_to_md(call: &McpToolCall, sources: &[String]) -> Result<McpToolResult> {
	let mut conversions = Vec::with_capacity(sources.len());
	let mut failures = Vec::new();

	for source in sources {
		match fetch_html_content(source).await {
			Ok((html_content, source_type)) => {
				match html_to_markdown(&html_content) {
					Ok(markdown) => {
						conversions.push(json!({
							"source": source,
							"type": source_type,
							"markdown": markdown,
							"size": markdown.len()
						}));
					},
					Err(e) => {
						failures.push(format!("Failed to convert {} to markdown: {}", source, e));
					}
				}
			},
			Err(e) => {
				failures.push(format!("Failed to fetch {}: {}", source, e));
			}
		}
	}

	Ok(McpToolResult {
		tool_name: "html2md".to_string(),
		tool_id: call.tool_id.clone(),
		result: json!({
			"success": !conversions.is_empty(),
			"conversions": conversions,
			"count": conversions.len(),
			"failed": failures
		}),
	})
}

// Fetch HTML content from URL or local file
async fn fetch_html_content(source: &str) -> Result<(String, &'static str)> {
	// Check if source is a URL or file path
	if let Ok(url) = Url::parse(source) {
		if url.scheme() == "http" || url.scheme() == "https" {
			// Fetch from URL
			let response = reqwest::get(source).await?;
			if !response.status().is_success() {
				return Err(anyhow!("HTTP error {}: {}", response.status(), source));
			}
			let html = response.text().await?;
			Ok((html, "url"))
		} else if url.scheme() == "file" {
			// Handle file:// URLs
			let path = url.to_file_path().map_err(|_| anyhow!("Invalid file URL: {}", source))?;
			let html = tokio_fs::read_to_string(&path).await?;
			Ok((html, "file"))
		} else {
			Err(anyhow!("Unsupported URL scheme: {}", url.scheme()))
		}
	} else {
		// Treat as file path
		let path = Path::new(source);
		if !path.exists() {
			return Err(anyhow!("File does not exist: {}", source));
		}
		if !path.is_file() {
			return Err(anyhow!("Path is not a file: {}", source));
		}
		let html = tokio_fs::read_to_string(path).await?;
		Ok((html, "file"))
	}
}

// Convert HTML to Markdown using html5ever parser
fn html_to_markdown(html: &str) -> Result<String> {
	let dom = parse_document(RcDom::default(), Default::default())
		.from_utf8()
		.read_from(&mut html.as_bytes())?;

	let mut markdown = String::new();
	walk_node(&dom.document, &mut markdown, 0)?;

	// Clean up the markdown
	let cleaned = clean_markdown(&markdown);
	Ok(cleaned)
}

// Recursively walk the DOM tree and convert to Markdown
fn walk_node(handle: &Handle, markdown: &mut String, depth: usize) -> Result<()> {
	let node = handle;
	match &node.data {
		NodeData::Document => {
			// Process children
			for child in node.children.borrow().iter() {
				walk_node(child, markdown, depth)?;
			}
		},
		NodeData::Element { name, attrs, .. } => {
			let tag_name = &name.local;
			let attrs = attrs.borrow();

			match tag_name.as_ref() {
				"h1" => {
					markdown.push_str("\n# ");
					process_children(node, markdown, depth)?;
					markdown.push_str("\n\n");
				},
				"h2" => {
					markdown.push_str("\n## ");
					process_children(node, markdown, depth)?;
					markdown.push_str("\n\n");
				},
				"h3" => {
					markdown.push_str("\n### ");
					process_children(node, markdown, depth)?;
					markdown.push_str("\n\n");
				},
				"h4" => {
					markdown.push_str("\n#### ");
					process_children(node, markdown, depth)?;
					markdown.push_str("\n\n");
				},
				"h5" => {
					markdown.push_str("\n##### ");
					process_children(node, markdown, depth)?;
					markdown.push_str("\n\n");
				},
				"h6" => {
					markdown.push_str("\n###### ");
					process_children(node, markdown, depth)?;
					markdown.push_str("\n\n");
				},
				"p" => {
					markdown.push('\n');
					process_children(node, markdown, depth)?;
					markdown.push_str("\n\n");
				},
				"strong" | "b" => {
					markdown.push_str("**");
					process_children(node, markdown, depth)?;
					markdown.push_str("**");
				},
				"em" | "i" => {
					markdown.push('*');
					process_children(node, markdown, depth)?;
					markdown.push('*');
				},
				"code" => {
					markdown.push('`');
					process_children(node, markdown, depth)?;
					markdown.push('`');
				},
				"pre" => {
					markdown.push_str("\n```\n");
					process_children(node, markdown, depth)?;
					markdown.push_str("\n```\n\n");
				},
				"a" => {
					// Find href attribute
					let href = attrs.iter()
						.find(|attr| &*attr.name.local == "href")
						.map(|attr| attr.value.to_string());

					if let Some(url) = href {
						markdown.push('[');
						process_children(node, markdown, depth)?;
						markdown.push_str(&format!("]({})", url));
					} else {
						process_children(node, markdown, depth)?;
					}
				},
				"ul" => {
					markdown.push('\n');
					process_children(node, markdown, depth)?;
					markdown.push('\n');
				},
				"ol" => {
					markdown.push('\n');
					process_children(node, markdown, depth)?;
					markdown.push('\n');
				},
				"li" => {
					if depth > 0 {
						for _ in 0..(depth - 1) {
							markdown.push_str("  ");
						}
					}
					markdown.push_str("- ");
					process_children(node, markdown, depth + 1)?;
					markdown.push('\n');
				},
				"blockquote" => {
					markdown.push_str("\n> ");
					process_children(node, markdown, depth)?;
					markdown.push_str("\n\n");
				},
				"br" => {
					markdown.push_str("  \n");
				},
				"hr" => {
					markdown.push_str("\n---\n\n");
				},
				"img" => {
					// Find src and alt attributes
					let src = attrs.iter()
						.find(|attr| &*attr.name.local == "src")
						.map(|attr| attr.value.to_string());
					let alt = attrs.iter()
						.find(|attr| &*attr.name.local == "alt")
						.map(|attr| attr.value.to_string())
						.unwrap_or_else(|| "".to_string());

					if let Some(url) = src {
						markdown.push_str(&format!("![{}]({})", alt, url));
					}
				},
				// Skip common non-content elements
				"script" | "style" | "head" | "meta" | "link" | "title" => {
					// Don't process children of these elements
				},
				// For all other elements, just process children
				_ => {
					process_children(node, markdown, depth)?;
				}
			}
		},
		NodeData::Text { contents } => {
			let text = contents.borrow().to_string();
			// Clean up whitespace in text nodes
			let cleaned_text = text.trim();
			if !cleaned_text.is_empty() {
				markdown.push_str(cleaned_text);
			}
		},
		_ => {
			// For other node types (comments, etc.), process children
			for child in node.children.borrow().iter() {
				walk_node(child, markdown, depth)?;
			}
		}
	}
	Ok(())
}

// Helper function to process children of a node
fn process_children(node: &Handle, markdown: &mut String, depth: usize) -> Result<()> {
	for child in node.children.borrow().iter() {
		walk_node(child, markdown, depth)?;
	}
	Ok(())
}

// Clean up the generated Markdown
fn clean_markdown(markdown: &str) -> String {
	let mut lines: Vec<&str> = markdown.lines().collect();

	// Remove leading and trailing empty lines
	while let Some(&first) = lines.first() {
		if first.trim().is_empty() {
			lines.remove(0);
		} else {
			break;
		}
	}

	while let Some(&last) = lines.last() {
		if last.trim().is_empty() {
			lines.pop();
		} else {
			break;
		}
	}

	// Collapse multiple consecutive empty lines into at most two
	let mut result = Vec::new();
	let mut empty_count = 0;

	for line in lines {
		if line.trim().is_empty() {
			empty_count += 1;
			if empty_count <= 2 {
				result.push(line);
			}
		} else {
			empty_count = 0;
			result.push(line);
		}
	}

	result.join("\n")
}
