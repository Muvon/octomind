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

// Optimized function definitions module - MCP function specifications with reduced tokens

use super::super::McpFunction;
use serde_json::json;

// Define the list_files function - optimized
pub fn get_list_files_function() -> McpFunction {
	McpFunction {
		name: "list_files".to_string(),
		description: "List files in a directory, with optional pattern matching.

			This tool uses ripgrep for efficient searching that respects .gitignore files.
			You can use it to find files by name pattern or search for files containing specific content.

			PERFORMANCE WARNING: Use filtering to avoid large outputs that consume excessive tokens

			Parameters:
			- `directory`: Target directory to search
			- `pattern`: Optional filename pattern (uses ripgrep syntax)
			- `content`: Optional content search within files
			- `max_depth`: Optional depth limit for directory traversal

			Best Practices:
			- Always use specific patterns - avoid listing entire large directories
			- Use max_depth to limit scope and reduce token usage
			- Combine with content search when looking for specific functionality
			- Filter by file type using patterns like '\\*.rs' or '\\*.toml'

			Examples:
			- Find Rust files: `{\"directory\": \"src\", \"pattern\": \"\\*.rs\"}`
			- Find config files: `{\"directory\": \".\", \"pattern\": \"\\*.toml|\\*.yaml|\\*.json\"}`
			- Search for function: `{\"directory\": \"src\", \"content\": \"fn main\"}`
			- Limited depth: `{\"directory\": \".\", \"max_depth\": 2, \"pattern\": \"\\*.rs\"}`

			Token-Efficient Usage:
			- Use patterns to target specific file types
			- Set max_depth to avoid deep directory traversals
			- Combine with content search for targeted results
			- Prefer multiple specific calls over one broad search"
			.to_string(),
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

// Define the text editor function - DRAMATICALLY OPTIMIZED
pub fn get_text_editor_function() -> McpFunction {
	McpFunction {
		name: "text_editor".to_string(),
		description: "Perform text editing operations on files with comprehensive file manipulation capabilities.

			The `command` parameter specifies the operation to perform.

			CRITICAL: LINE NUMBERS CHANGE AFTER EVERY EDIT OPERATION!
			- After ANY edit (str_replace, insert, line_replace), line numbers become invalid
			- ALWAYS use 'view' command first to get current line numbers before line_replace
			- PREFER line_replace when you know exact lines (fastest), str_replace when you know content

			Available commands:

			`view`: Examine file content or list directory contents
			- View entire file: `{\"command\": \"view\", \"path\": \"src/main.rs\"}`
			- View specific lines: `{\"command\": \"view\", \"path\": \"src/main.rs\", \"view_range\": [10, 20]}`
			- List directory: `{\"command\": \"view\", \"path\": \"src/\"}`
			- Returns content with line numbers for editing reference

			`create`: Create new file with specified content
			- `{\"command\": \"create\", \"path\": \"src/new_module.rs\", \"file_text\": \"pub fn hello() {\\n    println!(\\\"Hello!\\\");\\n}\"}`
			- Creates parent directories if they don't exist
			- Returns error if file already exists to prevent accidental overwrites

			`str_replace`: Replace specific string in file with new content
			- `{\"command\": \"str_replace\", \"path\": \"src/main.rs\", \"old_str\": \"fn old_name()\", \"new_str\": \"fn new_name()\"}`
			- The old_str must match exactly, including whitespace and indentation
			- Returns error if string appears 0 times or more than once for safety
			- Content-based replacement - works regardless of line numbers
			- Use when exact text is known but line numbers uncertain

			`insert`: Insert text at specific location in file
			- `{\"command\": \"insert\", \"path\": \"src/main.rs\", \"insert_line\": 5, \"new_str\": \"    // New comment\\n    let x = 10;\"}`
			- insert_line specifies the line number after which to insert (0 for beginning of file)
			- WARNING: Changes line numbers for all content AFTER insertion point

			`line_replace`: Replace content within specific line range
			- `{\"command\": \"line_replace\", \"path\": \"src/main.rs\", \"view_range\": [5, 8], \"new_str\": \"fn updated_function() {\\n    // New implementation\\n}\"}`
			- Replaces lines from view_range[0] to view_range[1] (inclusive, 1-indexed)
			- FASTEST option - 3x faster than str_replace (no content searching)
			- CRITICAL: Line numbers change after ANY edit operation
			- NEVER use line_replace twice without viewing file between operations
			- ALWAYS use 'view' first to get current line numbers before line_replace

			`view_many`: View multiple files simultaneously
			- `{\"command\": \"view_many\", \"paths\": [\"src/main.rs\", \"src/lib.rs\", \"tests/test.rs\"]}`
			- Returns content with line numbers for all files in a single operation
			- Maximum 50 files per request to maintain performance

			`undo_edit`: Revert most recent edit to specified file
			- `{\"command\": \"undo_edit\", \"path\": \"src/main.rs\"}`
			- Available for str_replace, insert, and line_replace operations

			`batch_edit`: Perform multiple text editing operations in single call
			- `{\"command\": \"batch_edit\", \"operations\": [{\"operation\": \"str_replace\", \"path\": \"src/main.rs\", \"old_str\": \"old\", \"new_str\": \"new\"}, {\"operation\": \"insert\", \"path\": \"src/lib.rs\", \"insert_line\": 5, \"new_str\": \"// New comment\"}]}`
			- ALWAYS USE when making 2+ changes across multiple files
			- ALWAYS USE when making 3+ changes in same file
			- 10x more efficient than individual operations
			- Saves tokens - one tool call instead of many
			- Perfect for: refactoring, consistent changes, multi-file updates
			- Supported operations: str_replace, insert, line_replace
			- MANDATORY for planned multi-file changes

			Error Handling:
			- File not found: Returns descriptive error message
			- Multiple matches: Returns error asking for more specific context
			- No matches: Returns error with suggestion to check the text
			- Permission errors: Returns permission denied message
			- Line range errors: Validates line numbers exist in file

			Best Practices:
			- ALWAYS use 'view' first to get current line numbers before any edit
			- Never assume line numbers from previous operations - they change after every edit

			OPTIMAL WORKFLOW:
			- PLAN FIRST: If 2+ files or 3+ edits → USE batch_edit
			- Use `view` to see file structure and get line numbers
			- For multiple changes: use `batch_edit` (10x more efficient)
			- For single changes: use `line_replace` ONCE per file
			- If more edits needed: `view` again to get fresh line numbers, then `line_replace` again

			CHOOSE batch_edit when:
			- Making 2+ changes across different files
			- Making 3+ changes in same file (any combination of operations)
			- Applying same change pattern across multiple files
			- Want maximum efficiency (10x faster than individual calls)

			CHOOSE line_replace when:
			- You just viewed the file and know exact line numbers
			- Changing single parameters, variable assignments, function calls
			- Want 3x faster performance (no content searching needed)
			- ONLY ONE line_replace per file before re-viewing

			CHOOSE str_replace when:
			- You know exact text content but not line numbers
			- Text might be at different line positions across files
			- Making multiple sequential edits (line numbers become unreliable)

			CRITICAL LINE NUMBER RULES:
			- Line numbers become INVALID after ANY edit operation
			- NEVER use line_replace twice without viewing file between operations
			- After str_replace, insert, or line_replace: line numbers change
			- Always view file again to get fresh line numbers before next line_replace
			- ONE line_replace per file per editing session - then re-view if more edits needed

			General Guidelines:
			- Use insert for adding new code at specific locations
			- Use create for new files and modules
			- Use undo_edit to revert the last operation if needed"
			.to_string(),
		parameters: json!({
			"type": "object",
			"required": ["command", "path"],
			"properties": {
				"command": {
					"type": "string",
					"enum": ["view", "view_many", "create", "str_replace", "insert", "line_replace", "undo_edit", "batch_edit"],
					"description": "The operation to perform: view, view_many, create, str_replace, insert, line_replace, undo_edit, or batch_edit"
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
				},
				"operations": {
					"type": "array",
					"items": {
						"type": "object",
						"required": ["operation", "path"],
						"properties": {
							"operation": {
								"type": "string",
								"enum": ["str_replace", "insert", "line_replace"],
								"description": "Type of operation to perform"
							},
							"path": {
								"type": "string",
								"description": "Path to the file to modify"
							},
							"old_str": {
								"type": "string",
								"description": "Text to replace (required for str_replace)"
							},
							"new_str": {
								"type": "string",
								"description": "New text content (required for all operations)"
							},
							"insert_line": {
								"type": "integer",
								"minimum": 0,
								"description": "Line number after which to insert (required for insert)"
							},
							"view_range": {
								"type": "array",
								"items": {"type": "integer"},
								"minItems": 2,
								"maxItems": 2,
								"description": "Line range [start, end] for line_replace (required for line_replace)"
							}
						}
					},
					"maxItems": 50,
					"description": "Array of operations for batch_edit command (maximum 50 operations)"
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

			Output is clean Markdown that preserves the document structure and readability."
			.to_string(),
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
