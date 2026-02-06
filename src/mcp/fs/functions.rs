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
			- `line_numbers`: Show line numbers for content search (default: true)
			- `context`: Number of context lines to show around matches (default: 0)

			Note: Response size is controlled by global mcp_response_tokens_threshold setting.
			Use specific patterns and filters to reduce output size if responses are truncated.

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
			- content parameter contains RAW FILE CONTENT - use actual whitespace characters, not escape sequences (tabs=actual tabs, NOT \\t)!
			- FOR MULTIPLE EDITS: Use batch_edit tool instead if file hasn't been modified yet

			Available commands:

			`view`: Examine file content or list directory contents
			- View entire file: `{\"command\": \"view\", \"path\": \"src/main.rs\"}`
			- View specific lines: `{\"command\": \"view\", \"path\": \"src/main.rs\", \"lines\": [10, 20]}`
			- List directory: `{\"command\": \"view\", \"path\": \"src/\"}`
			- Returns content as plain text with line numbers (1-indexed) for editing reference
			- Smart elision: when using lines parameter, shows context with [...X lines more] indicators

			`create`: Create new file with specified content
			- `{\"command\": \"create\", \"path\": \"src/new_module.rs\", \"content\": \"pub fn hello() {\\n    println!(\\\"Hello!\\\");\\n}\"}`
			- Creates parent directories if they don't exist
			- Returns error if file already exists to prevent accidental overwrites

			`str_replace`: Replace specific string in file with new content
			- `{\"command\": \"str_replace\", \"path\": \"src/main.rs\", \"old_text\": \"fn old_name()\", \"new_text\": \"fn new_name()\"}`
			- The old_text must match exactly, including whitespace and indentation
			- Returns error if string appears 0 times or more than once for safety
			- Content-based replacement - works regardless of line numbers
			- Use when exact text is known but line numbers uncertain

			`insert`: Insert text at specific location in file
			- `{\"command\": \"insert\", \"path\": \"src/main.rs\", \"insert_after_line\": 5, \"content\": \"    // New comment\\n    let x = 10;\"}`
			- insert_after_line specifies the line number after which to insert (0 for beginning of file)
			- WARNING: Changes line numbers for all content AFTER insertion point

			`line_replace`: Replace content within specific line range
			- `{\"command\": \"line_replace\", \"path\": \"src/main.rs\", \"lines\": [5, 8], \"content\": \"fn updated_function() {\\n    // New implementation\\n}\"}`
			- Replaces lines from lines[0] to lines[1] (inclusive, 1-indexed)
			- FASTEST option - 3x faster than str_replace (no content searching)
			- **REMOVE LINES**: Use empty content (\"\" or \"\") to remove lines completely
			- **USEFUL FOR REFACTORING**: Extract code with `extract_lines`, then remove original with `line_replace` + empty content
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
			- Use `view` to see file structure and get line numbers
			- For single changes: use `line_replace` ONCE per file
			- If more edits needed: `view` again to get fresh line numbers, then `line_replace` again

			CHOOSE line_replace when:
			- You know exact line numbers and need to change one or more lines with new
			- Want 3x faster performance (no content searching needed)
			- ONLY ONE line_replace per file before re-viewing
			- File has already been modified (line numbers changed)

			CHOOSE str_replace when:
			- Modifying existing code/content (not building new)
			- You know exact text content but not line numbers
			- Changing existing function implementations or config values
			- File has been modified and you can't trust line numbers

			CHOOSE batch_edit when:
			- Multiple edits needed on UNMODIFIED file (2+ operations)
			- File hasn't been edited yet in this session
			- Need atomic operations (all succeed or all fail)
			- Use 'insert' for adding lines, 'replace' ONLY for lines that change content
			- NEVER retype unchanged lines in replace operations

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
					"enum": ["view", "view_many", "create", "str_replace", "insert", "line_replace", "undo_edit"],
					"description": "The operation to perform: view, view_many, create, str_replace, insert, line_replace, undo_edit"
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
				"lines": {
					"type": "array",
					"items": {"type": "integer"},
					"minItems": 2,
					"maxItems": 2,
					"description": "Line range [start_line, end_line] for viewing or replacing (1-indexed, inclusive). Supports negative indexing: -1 for last line"
				},
				"content": {
					"type": "string",
					"description": "Content for create, insert, or line_replace operations. Raw text with actual whitespace (not escape sequences)"
				},
				"old_text": {
					"type": "string",
					"description": "Text to find and replace (must match exactly including whitespace) - for str_replace command"
				},
				"new_text": {
					"type": "string",
					"description": "Replacement text for str_replace command. Raw text with actual whitespace (not escape sequences)"
				},
				"insert_after_line": {
					"type": "integer",
					"minimum": 0,
					"description": "Insert content after this line number (0 = beginning of file, N = after line N)"
				},
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

// Define the batch_edit function - extracted from text_editor for simplicity
pub fn get_batch_edit_function() -> McpFunction {
	McpFunction {
		name: "batch_edit".to_string(),
		description: "Perform multiple line-based operations on a SINGLE file using original line numbers.

			This tool performs multiple insert and replace operations on a single file in one atomic operation.
			All operations use the ORIGINAL file line numbers before any modifications, ensuring line stability.

			**WHEN TO USE:**
			- Multiple edits in single file (2+ changes)
			- When you need to edit a file that hasn't been modified yet in this session
			- For atomic operations that must all succeed or all fail

			**WHEN NOT TO USE:**
			- After ANY other file edit (text_editor, str_replace, etc.) - line numbers will be wrong
			- For single operations - use text_editor instead
			- On files that have been modified - get fresh line numbers first

			**CRITICAL USAGE RULES:**
			- NEVER retype unchanged lines in replace operations - only replace lines that actually change
			- Use 'insert' to ADD new lines between existing ones
			- Use 'replace' ONLY when changing existing line content
			- Replace operation REMOVES lines [start, end] and INSERTS new content in their place
			- If lines X and Y stay the same, DO NOT include them in replace range

			**Operation Types:**
			- `insert`: line_range is single number N = insert AFTER line N (0 = beginning)
			- `replace`: line_range is [start, end] = REMOVE lines start-end, INSERT new content

			**WRONG vs RIGHT Examples:**

			❌ WRONG - Adding import by retyping all lines 1-5:
			File has:
			  1: use std::fs;
			  2: use std::io;
			  3: 
			  4: fn main() {
			Bad: {\"operation\": \"replace\", \"line_range\": [1, 3], \"content\": \"use std::fs;\\nuse std::io;\\nuse std::path::PathBuf;\"}
			Problem: Retyped lines 1-2 that didn't change!

			✅ RIGHT - Use insert to add new line:
			{\"operation\": \"insert\", \"line_range\": 2, \"content\": \"use std::path::PathBuf;\"}
			Result: New import added after line 2, lines 1-2 unchanged

			❌ WRONG - Changing one line by replacing entire block:
			File has:
			  10: let x = 5;
			  11: let y = 10;
			  12: let z = 15;
			Bad: {\"operation\": \"replace\", \"line_range\": [10, 12], \"content\": \"let x = 5;\\nlet y = 20;\\nlet z = 15;\"}
			Problem: Retyped lines 10 and 12 that didn't change!

			✅ RIGHT - Replace only the line that changes:
			{\"operation\": \"replace\", \"line_range\": [11, 11], \"content\": \"let y = 20;\"}
			Result: Only line 11 changed, lines 10 and 12 unchanged

			**Parameters:**
			- `path` (string): Path to the file to edit
			- `operations` (array): Array of operations to perform

			**Maximum 50 operations per call for performance**".to_string(),
		parameters: json!({
			"type": "object",
			"properties": {
				"path": {
					"type": "string",
					"description": "Path to the file to edit"
				},
				"operations": {
					"type": "array",
					"items": {
						"type": "object",
						"properties": {
							"operation": {
								"type": "string",
								"enum": ["insert", "replace"],
								"description": "Type of operation: 'insert' (after line) or 'replace' (line range)"
							},
							"line_range": {
								"oneOf": [
									{
										"type": "integer",
										"minimum": 0,
										"description": "Single line number for insert (0=beginning, N=after line N)"
									},
									{
										"type": "array",
										"items": {"type": "integer", "minimum": 1},
										"minItems": 1,
										"maxItems": 2,
										"description": "Line range [start] or [start, end] (1-indexed, inclusive)"
									}
								],
								"description": "CRITICAL: Line numbers from ORIGINAL file content (before any modifications). Insert: single number (after which line). Replace: [start, end] range (inclusive, 1-indexed). DO NOT USE if file was modified - line numbers will be wrong!"
							},
							"content": {
								"type": "string",
								"description": "Raw content to insert or replace with (no escaping needed - use actual tabs/spaces)"
							}
						},
						"required": ["operation", "line_range", "content"]
					},
					"maxItems": 50,
					"description": "Array of operations for batch_edit on SINGLE file. All line_range values reference ORIGINAL file content. DO NOT USE after any file modifications!"
				}
			},
			"required": ["path", "operations"]
		}),
	}
}

// Get all available filesystem functions
pub fn get_all_functions() -> Vec<McpFunction> {
	vec![
		get_text_editor_function(),
		get_batch_edit_function(),
		get_list_files_function(),
		get_extract_lines_function(),
	]
}
