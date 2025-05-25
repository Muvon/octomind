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

// Define the text editor function for modifying files - comprehensive file editing capabilities
pub fn get_text_editor_function() -> McpFunction {
	McpFunction {
		name: "text_editor".to_string(),
		description: "Perform text editing operations on files with comprehensive file manipulation capabilities.

			The `command` parameter specifies the operation to perform. Available commands:

			**view**: Examine content of a file or list directory contents
			- View entire file: `{\"command\": \"view\", \"path\": \"src/main.rs\"}`
			- View specific lines: `{\"command\": \"view\", \"path\": \"src/main.rs\", \"view_range\": [10, 20]}`
			- List directory: `{\"command\": \"view\", \"path\": \"src/\"}`
			- The view_range parameter is optional and specifies start and end line numbers (1-indexed)
			- Returns content with line numbers for precise editing reference

			**create**: Create a new file with specified content
			- `{\"command\": \"create\", \"path\": \"src/new_module.rs\", \"file_text\": \"pub fn hello() {\\n    println!(\\\"Hello!\\\");\\n}\"}`
			- Creates parent directories if they don't exist
			- Returns error if file already exists to prevent accidental overwrites

			**str_replace**: Replace a specific string in a file with new content
			- `{\"command\": \"str_replace\", \"path\": \"src/main.rs\", \"old_str\": \"fn old_name()\", \"new_str\": \"fn new_name()\"}`
			- The old_str must match exactly, including whitespace and indentation
			- Returns error if string appears 0 times or more than once for safety
			- Automatically saves file history for undo operations

			**insert**: Insert text at a specific location in a file
			- `{\"command\": \"insert\", \"path\": \"src/main.rs\", \"insert_line\": 5, \"new_str\": \"    // New comment\\n    let x = 10;\"}`
			- insert_line specifies the line number after which to insert (0 for beginning of file)
			- new_str can contain multiple lines using \\n
			- Line numbers are 1-indexed for intuitive operation

			**line_replace**: Replace content within a specific line range
			- `{\"command\": \"line_replace\", \"path\": \"src/main.rs\", \"start_line\": 5, \"end_line\": 8, \"new_text\": \"fn updated_function() {\\n    // New implementation\\n}\"}`
			- Replaces lines from start_line to end_line (inclusive, 1-indexed)
			- More precise than str_replace when you know exact line numbers
			- Ideal for replacing function implementations, code blocks, or configuration sections

			**view_many**: View multiple files simultaneously for comprehensive analysis
			- `{\"command\": \"view_many\", \"paths\": [\"src/main.rs\", \"src/lib.rs\", \"tests/test.rs\"]}`
			- Returns content with line numbers for all files in a single operation
			- Efficient for understanding relationships between files, code analysis, and refactoring
			- Includes binary file detection, size limits, and error resilience
			- Maximum 50 files per request to maintain performance

			**undo_edit**: Revert the most recent edit made to a specified file
			- `{\"command\": \"undo_edit\", \"path\": \"src/main.rs\"}`
			- Available for str_replace, insert, and line_replace operations
			- Restores the file to its state before the last edit

			**Error Handling:**
			- File not found: Returns descriptive error message
			- Multiple matches: Returns error asking for more specific context
			- No matches: Returns error with suggestion to check the text
			- Permission errors: Returns permission denied message
			- Line range errors: Validates line numbers exist in file

			**Best Practices:**
			- Use view with line ranges to examine specific sections before editing
			- Always verify file content before making changes
			- Use str_replace for content-based replacements
			- Use line_replace when you know exact line positions
			- Use insert for adding new code at specific locations
			- Use create for new files and modules".to_string(),
		parameters: json!({
			"type": "object",
			"required": ["command", "path"],
			"properties": {
				"command": {
					"type": "string",
					"enum": ["view", "view_many", "create", "str_replace", "insert", "line_replace", "undo_edit"],
					"description": "The operation to perform: view, view_many, create, str_replace, insert, line_replace, or undo_edit"
				},
				"path": {
					"type": "string",
					"description": "Absolute path to the file or directory (not used for view_many command)"
				},
				"paths": {
					"type": "array",
					"items": {"type": "string"},
					"maxItems": 50,
					"description": "Array of absolute file paths for view_many command"
				},
				"view_range": {
					"type": "array",
					"items": {"type": "integer"},
					"minItems": 2,
					"maxItems": 2,
					"description": "Optional array of two integers [start_line, end_line] for viewing specific lines (1-indexed, -1 for end means read to end of file)"
				},
				"file_text": {
					"type": "string",
					"description": "Content to write when creating a new file"
				},
				"old_str": {
					"type": "string",
					"description": "Text to replace (must match exactly including whitespace)"
				},
				"new_str": {
					"type": "string",
					"description": "Replacement text for str_replace or text to insert for insert command"
				},
				"insert_line": {
					"type": "integer",
					"minimum": 0,
					"description": "Line number after which to insert text (0 for beginning of file, 1-indexed)"
				},
				"start_line": {
					"type": "integer",
					"minimum": 1,
					"description": "Starting line number for line_replace command (1-indexed)"
				},
				"end_line": {
					"type": "integer",
					"minimum": 1,
					"description": "Ending line number for line_replace command (1-indexed, inclusive)"
				},
				"new_text": {
					"type": "string",
					"description": "New content to replace the specified line range in line_replace command"
				}
			}
		}),
	}
}

// Define the view_many function for viewing multiple files simultaneously
pub fn get_view_many_function() -> McpFunction {
	McpFunction {
		name: "view_many".to_string(),
		description: "View multiple files simultaneously with optimized token usage and comprehensive analysis capabilities.

			- **Code Analysis**: Understanding relationships between modules, classes, and functions
			- **Refactoring**: Seeing how changes in one file might affect others
			- **Documentation**: Analyzing multiple source files to write comprehensive documentation
			- **Debugging**: Examining related files to understand complex bugs
			- **Architecture Review**: Getting an overview of project structure and organization

			**Key Benefits:**
			- **Efficient Context Loading**: Read multiple related files in a single operation
			- **Token Optimization**: Batched file reading reduces API call overhead
			- **Consistent Formatting**: All files returned with line numbers for precise reference
			- **Smart Filtering**: Automatic handling of binary files and size limits
			- **Error Resilience**: Continues processing other files even if some fail

			**Usage Examples:**
			- Analyze module structure: `{\"paths\": [\"src/lib.rs\", \"src/main.rs\", \"src/utils.rs\"]}`
			- Review test coverage: `{\"paths\": [\"src/parser.rs\", \"tests/parser_tests.rs\", \"tests/integration_tests.rs\"]}`
			- Configuration analysis: `{\"paths\": [\"Cargo.toml\", \"src/config.rs\", \".env.example\"]}`
			- Documentation review: `{\"paths\": [\"README.md\", \"CONTRIBUTING.md\", \"src/lib.rs\"]}`

			**Response Format:**
			Returns a structured response with:
			- **files**: Array of successfully processed files with content and metadata
			- **failed**: Array of files that couldn't be processed with error descriptions
			- **count**: Number of successfully processed files
			- **total_size**: Combined size of all processed files

			Each file entry includes:
			- **path**: Full file path
			- **content**: File content with line numbers (format: \"1: content\")
			- **lines**: Number of lines in the file
			- **size**: File size in bytes
			- **lang**: Detected programming language for syntax highlighting

			**Limitations and Safeguards:**
			- Files larger than 5MB are skipped to prevent token overflow
			- Binary files are automatically detected and skipped
			- Maximum of 50 files per request to maintain performance
			- Individual file errors don't stop processing of other files

			**Best Practices:**
			- Group related files together for better context
			- Use when you need to understand relationships between files
			- Prefer this over multiple single-file view operations
			- Consider file sizes to avoid token limits
			- Review failed files list to ensure all intended files were processed".to_string(),
		parameters: json!({
			"type": "object",
			"required": ["paths"],
			"properties": {
				"paths": {
					"type": "array",
					"items": {
						"type": "string"
					},
					"description": "Array of absolute file paths to view simultaneously",
					"maxItems": 50
				}
			}
		}),
	}
}

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

			view_file_spec(call, Path::new(&path), view_range).await
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

			view_many_files_spec(call, &paths).await
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
			create_file_spec(call, Path::new(&path), &file_text).await
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
			str_replace_spec(call, Path::new(&path), &old_str, &new_str).await
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
			insert_text_spec(call, Path::new(&path), insert_line, &new_str).await
		},
		"line_replace" => {
			let path = match call.parameters.get("path") {
				Some(Value::String(p)) => p.clone(),
				_ => return Err(anyhow!("Missing or invalid 'path' parameter for line_replace command")),
			};
			let start_line = match call.parameters.get("start_line") {
				Some(Value::Number(n)) => n.as_u64().ok_or_else(|| anyhow!("Invalid 'start_line' parameter"))? as usize,
				_ => return Err(anyhow!("Missing or invalid 'start_line' parameter")),
			};
			let end_line = match call.parameters.get("end_line") {
				Some(Value::Number(n)) => n.as_u64().ok_or_else(|| anyhow!("Invalid 'end_line' parameter"))? as usize,
				_ => return Err(anyhow!("Missing or invalid 'end_line' parameter")),
			};
			let new_text = match call.parameters.get("new_text") {
				Some(Value::String(s)) => s.clone(),
				_ => return Err(anyhow!("Missing or invalid 'new_text' parameter for line_replace command")),
			};
			line_replace_spec(call, Path::new(&path), start_line, end_line, &new_text).await
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
		get_list_files_function(),
		get_html2md_function(),
	]
}

// Helper function to detect language based on file extension
#[allow(dead_code)]
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

// View the content of a file following Anthropic specification - with line numbers and view_range support
async fn view_file_spec(call: &McpToolCall, path: &Path, view_range: Option<(usize, i64)>) -> Result<McpToolResult> {
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
async fn create_file_spec(call: &McpToolCall, path: &Path, content: &str) -> Result<McpToolResult> {
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

// Replace a string in a file following Anthropic specification
async fn str_replace_spec(call: &McpToolCall, path: &Path, old_str: &str, new_str: &str) -> Result<McpToolResult> {
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
async fn insert_text_spec(call: &McpToolCall, path: &Path, insert_line: usize, new_str: &str) -> Result<McpToolResult> {
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
async fn line_replace_spec(call: &McpToolCall, path: &Path, start_line: usize, end_line: usize, new_text: &str) -> Result<McpToolResult> {
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

// View multiple files simultaneously as part of text_editor tool
async fn view_many_files_spec(call: &McpToolCall, paths: &[String]) -> Result<McpToolResult> {
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

	view_many_files(call, &paths).await
}

// View multiple files simultaneously with optimized token usage
async fn view_many_files(call: &McpToolCall, paths: &[String]) -> Result<McpToolResult> {
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

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;

	async fn create_test_call(command: &str, path: &str, additional_params: serde_json::Value) -> McpToolCall {
		let mut params = json!({
			"command": command,
			"path": path
		});

		if let serde_json::Value::Object(additional) = additional_params {
			if let serde_json::Value::Object(ref mut params_obj) = params {
				for (key, value) in additional {
					params_obj.insert(key, value);
				}
			}
		}

		McpToolCall {
			tool_name: "text_editor".to_string(),
			tool_id: "test_123".to_string(),
			parameters: params,
		}
	}

	#[tokio::test]
	async fn test_view_command_with_line_numbers() {
		let test_file = "/tmp/test_view_ln.rs";
		std::fs::write(test_file, "fn main() {\n    println!(\"Hello!\");\n}").unwrap();

		let call = create_test_call("view", test_file, json!({})).await;
		let result = execute_text_editor(&call).await.unwrap();

		// Check that result contains line numbers
		let content = result.result.get("content").unwrap().as_str().unwrap();
		assert!(content.contains("1: fn main() {"));
		assert!(content.contains("2:     println!(\"Hello!\");"));

		std::fs::remove_file(test_file).ok();
	}

	#[tokio::test]
	async fn test_view_with_range_spec() {
		let test_file = "/tmp/test_view_range_spec.rs";
		std::fs::write(test_file, "line1\nline2\nline3\nline4\nline5").unwrap();

		let call = create_test_call("view", test_file, json!({
			"view_range": [2, 4]
		})).await;
		let result = execute_text_editor(&call).await.unwrap();

		let content = result.result.get("content").unwrap().as_str().unwrap();
		assert!(content.contains("2: line2"));
		assert!(content.contains("3: line3"));
		assert!(content.contains("4: line4"));
		assert!(!content.contains("1: line1"));
		assert!(!content.contains("5: line5"));

		std::fs::remove_file(test_file).ok();
	}

	#[tokio::test]
	async fn test_create_command_spec() {
		let test_file = "/tmp/test_create_spec.rs";
		std::fs::remove_file(test_file).ok(); // Ensure it doesn't exist

		let call = create_test_call("create", test_file, json!({
			"file_text": "pub fn hello() {\n    println!(\"Hello from new file!\");\n}"
		})).await;
		let result = execute_text_editor(&call).await.unwrap();

		assert!(result.result.get("content").unwrap().as_str().unwrap().contains("File created successfully"));
		assert!(Path::new(test_file).exists());

		let content = std::fs::read_to_string(test_file).unwrap();
		assert!(content.contains("pub fn hello()"));

		std::fs::remove_file(test_file).ok();
	}

	#[tokio::test]
	async fn test_str_replace_command_spec() {
		let test_file = "/tmp/test_str_replace_spec.rs";
		std::fs::write(test_file, "fn old_function() {\n    println!(\"old\");\n}").unwrap();

		let call = create_test_call("str_replace", test_file, json!({
			"old_str": "old_function",
			"new_str": "new_function"
		})).await;
		let result = execute_text_editor(&call).await.unwrap();

		assert!(result.result.get("content").unwrap().as_str().unwrap().contains("Successfully replaced"));

		let content = std::fs::read_to_string(test_file).unwrap();
		assert!(content.contains("fn new_function()"));
		assert!(!content.contains("fn old_function()"));

		std::fs::remove_file(test_file).ok();
	}

	#[tokio::test]
	async fn test_insert_command_spec() {
		let test_file = "/tmp/test_insert_spec.rs";
		std::fs::write(test_file, "fn main() {\n    println!(\"Hello!\");\n}").unwrap();

		let call = create_test_call("insert", test_file, json!({
			"insert_line": 1,
			"new_str": "    // This is a comment"
		})).await;
		let result = execute_text_editor(&call).await.unwrap();

		assert!(result.result.get("content").unwrap().as_str().unwrap().contains("Successfully inserted"));

		let content = std::fs::read_to_string(test_file).unwrap();
		let lines: Vec<&str> = content.lines().collect();
		assert_eq!(lines[1], "    // This is a comment");

		std::fs::remove_file(test_file).ok();
	}

	#[tokio::test]
	async fn test_line_replace_command_spec() {
		let test_file = "/tmp/test_line_replace_spec.rs";
		std::fs::write(test_file, "fn main() {\n    println!(\"old\");\n    let x = 1;\n}").unwrap();

		let call = create_test_call("line_replace", test_file, json!({
			"start_line": 2,
			"end_line": 3,
			"new_text": "    println!(\"new!\");\n    let y = 2;"
		})).await;
		let result = execute_text_editor(&call).await.unwrap();

		assert!(result.result.get("content").unwrap().as_str().unwrap().contains("Successfully replaced"));

		let content = std::fs::read_to_string(test_file).unwrap();
		let lines: Vec<&str> = content.lines().collect();
		assert_eq!(lines[1], "    println!(\"new!\");");
		assert_eq!(lines[2], "    let y = 2;");

		std::fs::remove_file(test_file).ok();
	}

	#[tokio::test]
	async fn test_view_many_command_in_text_editor() {
		// Create test files
		let test_file1 = "/tmp/test_view_many_te_1.rs";
		let test_file2 = "/tmp/test_view_many_te_2.rs";
		std::fs::write(test_file1, "fn hello() {\n    println!(\"Hello!\");\n}").unwrap();
		std::fs::write(test_file2, "fn world() {\n    println!(\"World!\");\n}").unwrap();

		let call = McpToolCall {
			tool_name: "text_editor".to_string(),
			tool_id: "test_view_many_te".to_string(),
			parameters: json!({
				"command": "view_many",
				"paths": [test_file1, test_file2]
			}),
		};

		let result = execute_text_editor(&call).await.unwrap();

		assert!(result.result.get("success").unwrap().as_bool().unwrap());
		let files = result.result.get("files").unwrap().as_array().unwrap();
		assert_eq!(files.len(), 2);

		// Check that both files have line numbers
		let file1_content = files[0].get("content").unwrap().as_str().unwrap();
		assert!(file1_content.contains("1: fn hello() {"));

		std::fs::remove_file(test_file1).ok();
		std::fs::remove_file(test_file2).ok();
	}

	#[tokio::test]
	async fn test_error_handling_spec() {
		// Test file not found
		let call = create_test_call("view", "/tmp/nonexistent_file_spec.rs", json!({})).await;
		let result = execute_text_editor(&call).await.unwrap();
		assert!(result.result.get("error").is_some());
		assert_eq!(result.result.get("is_error").unwrap().as_bool().unwrap(), true);
	}
}
