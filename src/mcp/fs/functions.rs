// Function definitions module - MCP function specifications

use serde_json::json;
use super::super::McpFunction;

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
			- `{\"command\": \"line_replace\", \"path\": \"src/main.rs\", \"view_range\": [5, 8], \"new_str\": \"fn updated_function() {\\n    // New implementation\\n}\"}`
			- Replaces lines from view_range[0] to view_range[1] (inclusive, 1-indexed)
			- More precise than str_replace when you know exact line numbers
			- Returns snippet of replaced content for verification
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
					"description": "Replacement text for str_replace, text to insert for insert command, or new content for line_replace command"
				},
				"insert_line": {
					"type": "integer",
					"minimum": 0,
					"description": "Line number after which to insert text (0 for beginning of file, 1-indexed)"
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

// Get all available filesystem functions
pub fn get_all_functions() -> Vec<McpFunction> {
	vec![
		get_text_editor_function(),
		get_list_files_function(),
		get_html2md_function(),
	]
}
