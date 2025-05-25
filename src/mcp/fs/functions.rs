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

// Define the line_replace function for line-based editing
pub fn get_line_replace_function() -> McpFunction {
	McpFunction {
		name: "line_replace".to_string(),
		description: "Replace content at specific line ranges in a single file.

			This tool performs line-based replacements by specifying a line range, making it ideal for precise code modifications without needing exact string matching.

			**Key Benefits:**
			- Line-based targeting eliminates string matching issues
			- Validates line ranges exist before making changes
			- Preserves file structure and formatting outside the target range
			- Returns snippet of replaced content for verification

			**Parameters:**
			`path`: Single file path string
			`view_range`: Array of two integers [start_line, end_line] - 1-indexed, inclusive
			`new_str`: New content to place at the specified line range

			**Usage:**
			```
			{
			\"path\": \"src/main.rs\",
			\"view_range\": [5, 8],
			\"new_str\": \"fn new_function() {\\n    // New implementation\\n}\"
			}
			```
			Replaces lines 5-8 (inclusive) with the new content.

			**Response Format:**
			Returns structured response with:
			- `success`: Operation success status
			- `content`: Human-readable success message
			- `replaced`: Snippet showing what content was replaced

			**Replaced Content Snippet:**
			- Single line: Shows the exact line content
			- 2-3 lines: Shows all lines
			- 4+ lines: Shows first line + \"... [N more lines]\" + last line

			**Line Numbering:**
			- Lines are 1-indexed (first line is line 1)
			- end_line is inclusive (line range includes both start and end)
			- If start_line == end_line, replaces single line
			- Content can span multiple lines using \\n characters

			**Validation:**
			- Verifies file exists and is readable
			- Validates line range exists in file
			- Ensures start_line <= end_line
			- Saves file history for undo operations

			**Best Practices:**
			1. **Use for precise code modifications** when you know exact line numbers
			2. **Combine with text_editor view** to identify target lines
			3. **Use \\n for multi-line content** to maintain proper formatting

			This tool is particularly effective for:
			- Updating function implementations at known locations
			- Replacing class definitions or struct declarations
			- Modifying configuration blocks
			- Updating import statements or dependencies".to_string(),
		parameters: json!({
			"type": "object",
			"required": ["path", "view_range", "new_str"],
			"properties": {
				"path": {
					"type": "string",
					"description": "Absolute path to file"
				},
				"view_range": {
					"type": "array",
					"items": {"type": "integer"},
					"minItems": 2,
					"maxItems": 2,
					"description": "Array of two integers [start_line, end_line] - 1-indexed, inclusive"
				},
				"new_str": {
					"type": "string",
					"description": "New content to place at the specified line range"
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

// Get all available filesystem functions
pub fn get_all_functions() -> Vec<McpFunction> {
	vec![
		get_text_editor_function(),
		get_line_replace_function(),
		get_list_files_function(),
		get_html2md_function(),
	]
}