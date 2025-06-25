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
			- `include_hidden`: Include hidden files/directories starting with '.' (default: false)
			- `max_lines`: Maximum lines to return (default: 20, set to 0 for unlimited)
			- `line_numbers`: Show line numbers for content search (default: true)
			- `context`: Number of context lines to show around matches (default: 0)

			Best Practices:
			- Always use specific patterns - avoid listing entire large directories
			- Use max_depth to limit scope and reduce token usage
			- Combine with content search when looking for specific functionality
			- Filter by file type using patterns like '*.rs' or '*.toml'
			- Use include_hidden=false (default) to exclude dotfiles for cleaner results

			Examples:
			- Find Rust files: `{\"directory\": \"src\", \"pattern\": \"*.rs\"}`
			- Find config files: `{\"directory\": \".\", \"pattern\": \"*.toml|*.yaml|*.json\"}`
			- Search for function: `{\"directory\": \"src\", \"content\": \"fn main\"}`
			- Limited depth: `{\"directory\": \".\", \"max_depth\": 2, \"pattern\": \"*.rs\"}`
			- Include dotfiles: `{\"directory\": \".\", \"pattern\": \".*rc\", \"include_hidden\": true}`
			- Find hidden configs: `{\"directory\": \".\", \"include_hidden\": true, \"pattern\": \"*.json|*.yaml\"}`

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
				},
				"include_hidden": {
					"type": "boolean",
					"default": false,
					"description": "Include hidden files and directories starting with '.' (default: false)"
				},
				"max_lines": {
					"type": "integer",
					"default": 20,
					"description": "Maximum lines to return (default: 20, set to 0 for unlimited)"
				},
				"line_numbers": {
					"type": "boolean",
					"default": true,
					"description": "Show line numbers for content search (default: true)"
				},
				"context": {
					"type": "integer",
					"default": 0,
					"description": "Number of context lines to show around matches (default: 0)"
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
			- new_str contains RAW FILE CONTENT - use actual whitespace characters, not escape sequences!

			Available commands:

			`view`: Examine file content or list directory contents
			- View entire file: `{\"command\": \"view\", \"path\": \"src/main.rs\"}`
			- View specific lines: `{\"command\": \"view\", \"path\": \"src/main.rs\", \"view_range\": [10, 20]}`
			- List directory: `{\"command\": \"view\", \"path\": \"src/\"}`
			- Returns content as plain text with line numbers (1-indexed) for editing reference
			- Smart elision: when using view_range, shows context with [...X lines more] indicators

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
			- **REMOVE LINES**: Use empty `new_str` (\"\" or \"\") to remove lines completely
			- **USEFUL FOR REFACTORING**: Extract code with `extract_lines`, then remove original with `line_replace` + empty `new_str`
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
			- You know exact line numbers and need to change one or more lines with new
			- Want 3x faster performance (no content searching needed)
			- ONLY ONE line_replace per file before re-viewing

			CHOOSE str_replace when:
			- Modifying existing code/content (not building new)
			- You know exact text content but not line numbers
			- Changing existing function implementations or config values

			CHOOSE insert when:
			- Building new documents or adding new sections
			- Adding content that doesn't exist yet
			- Creating plans, documentation, or structured content

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
					"description": "Raw file content exactly as it should appear in the file. Use actual tabs/spaces, NOT escape sequences like \\t or \\n. Copy exact indentation from original code. No string literal escaping needed - this is direct file content."
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

// Define the extract_lines function
pub fn get_extract_lines_function() -> McpFunction {
	McpFunction {
		name: "extract_lines".to_string(),
		description: "Extract lines from a source file and append them to a target file without modifying the source file.

			This tool is perfect for extracting code blocks, functions, or any text sections from one file
			and appending them to another file. The source file remains unchanged.

			**Parameters:**
			- `from_path` (string, required): Path to the source file to extract lines from
			- `from_range` (array, required): Two-element array [start, end] with 1-indexed line numbers (inclusive)
			- `append_path` (string, required): Path to the target file where extracted lines will be appended (auto-created if doesn't exist)
			- `append_line` (integer, required): Position where to append the extracted content:
			  - `0`: Insert at the beginning of the file
			  - `-1`: Append at the very end of the file
			  - `N` (positive): Insert after line N (1-indexed)

			**Examples:**
			- Extract function: `{\"from_path\": \"src/utils.rs\", \"from_range\": [10, 25], \"append_path\": \"src/extracted.rs\", \"append_line\": -1}`
			- Extract to beginning: `{\"from_path\": \"config.toml\", \"from_range\": [1, 5], \"append_path\": \"new_config.toml\", \"append_line\": 0}`
			- Insert after line 3: `{\"from_path\": \"main.rs\", \"from_range\": [50, 60], \"append_path\": \"module.rs\", \"append_line\": 3}`

			**Use Cases:**
			- Extracting functions or code blocks for refactoring
			- Moving configuration sections between files
			- Creating new files with specific content from existing files
			- Building modular code by extracting reusable components

			**Returns:**
			- Success: Information about extracted lines and where they were appended
			- Error: Clear error message if file not found, invalid range, or write permission issues

			**MCP Protocol Compliance:**
			- Proper parameter validation with descriptive error messages
			- Graceful handling of file system errors
			- Returns structured success/error responses".to_string(),
		parameters: json!({
			"type": "object",
			"properties": {
				"from_path": {
					"type": "string",
					"description": "Path to the source file to extract lines from"
				},
				"from_range": {
					"type": "array",
					"items": {"type": "integer"},
					"minItems": 2,
					"maxItems": 2,
					"description": "Two-element array [start, end] with 1-indexed line numbers (inclusive)"
				},
				"append_path": {
					"type": "string",
					"description": "Path to the target file where extracted lines will be appended (auto-created if doesn't exist)"
				},
				"append_line": {
					"type": "integer",
					"description": "Position where to append: 0=beginning, -1=end, N=after line N (1-indexed)"
				}
			},
			"required": ["from_path", "from_range", "append_path", "append_line"]
		}),
	}
}

// Get all available filesystem functions
pub fn get_all_functions() -> Vec<McpFunction> {
	vec![
		get_text_editor_function(),
		get_list_files_function(),
		get_extract_lines_function(),
	]
}
