// Developer MCP provider with enhanced functionality
// Based on the reference implementation with additional developer tools

use std::process::Command;
use std::path::Path;
use std::collections::HashMap;
use std::sync::Mutex;
use serde_json::{json, Value};
use anyhow::{Result, anyhow};
use tokio::fs as tokio_fs;
use lazy_static::lazy_static;
use super::{McpToolCall, McpToolResult, McpFunction};

// Thread-safe lazy initialization of file history using lazy_static
lazy_static! {
	static ref FILE_HISTORY: Mutex<HashMap<String, Vec<String>>> = Mutex::new(HashMap::new());
}

// Thread-safe way to get the file history
fn get_file_history() -> &'static Mutex<HashMap<String, Vec<String>>> {
	&FILE_HISTORY
}

// Define the shell function for the MCP protocol with enhanced description
pub fn get_shell_function() -> McpFunction {
	McpFunction {
		name: "shell".to_string(),
		description: format!("Execute a command in the shell.

This will return the output and error concatenated into a single string, as
you would see from running on the command line. There will also be an indication
of if the command succeeded or failed.

Avoid commands that produce a large amount of output, and consider piping those outputs to files.
If you need to run a long lived command, background it - e.g. `uvicorn main:app &` so that
this tool does not run indefinitely.

**Important**: Each shell command runs in its own process. Things like directory changes or
sourcing files do not persist between tool calls. So you may need to repeat them each time by
stringing together commands, e.g. `cd example && ls` or `source env/bin/activate && pip install numpy`

**Important**: Use ripgrep - `rg` - when you need to locate a file or a code reference, other solutions
may show ignored or hidden files. For example *do not* use `find` or `ls -r`
- List files by name: `rg --files | rg <filename>`
- List files that contain a regex: `rg '<regex>' -l`
"),
		parameters: json!({
			"type": "object",
			"properties": {
				"command": {
					"type": "string",
					"description": "The shell command to execute"
				}
			},
			"required": ["command"]
		}),
	}
}

// Define the text editor function for modifying files
pub fn get_text_editor_function() -> McpFunction {
	McpFunction {
		name: "text_editor".to_string(),
		description: "Perform text editing operations on files, optimized for multi-file operations.

			The `command` parameter specifies the operation to perform on the file system. This tool is specifically designed to handle batch operations across multiple files when possible, improving efficiency and consistency.

			Available Commands:

			`view`: Examine content of one or multiple files simultaneously
			- **Single file**: `{\"command\": \"view\", \"path\": \"src/main.rs\"}`
			- **Multiple files** (recommended): `{\"command\": \"view\", \"path\": [\"src/main.rs\", \"src/lib.rs\", \"Cargo.toml\"]}`
			- Returns content of all requested files for comprehensive analysis or reference

			`write`: Create or overwrite one or multiple files in a single operation
			- **Single file**: `{\"command\": \"write\", \"path\": \"src/main.rs\", \"file_text\": \"fn main() {...}\"}`
			- **Multiple files** (recommended):
			```
			{
			\"command\": \"write\",
			\"path\": [\"src/models.rs\", \"src/views.rs\"],
			\"file_text\": [\"pub struct Model {...}\", \"pub fn render() {...}\"]
			}
			```
			- Paths and file_texts arrays must have matching indices and equal length

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
			- **Multiple files** (recommended):
			```
			{
			\"command\": \"str_replace\",
			\"path\": [\"src/lib.rs\", \"src/main.rs\"],
			\"old_str\": [\"struct OldName\", \"use crate::OldName\"],
			\"new_str\": [\"struct NewName\", \"use crate::NewName\"]
			}
			```
			- All three arrays (paths, old_strs, new_strs) must have equal length
			- The `old_str` must exactly match text in the file, including whitespace
			- Ideal for consistent renaming or updating patterns across the codebase

			`undo_edit`: Revert the most recent edit made to a specified file
			- `{\"command\": \"undo_edit\", \"path\": \"src/main.rs\"}`
			- Useful for rolling back changes when needed

			Best Practices:
			1. **Always prefer multi-file operations** when working with related files
			2. Each string replacement is processed independently
			3. Ensure exact matching for string replacements including whitespace and formatting
			4. Use the array parameters for batch operations rather than making multiple separate calls
			5. For complex refactoring, consider combining view and write operations

			This tool enables efficient code management across multiple files, supporting comprehensive refactoring and codebase-wide changes.".to_string(),
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

// Define the list_files function
pub fn get_list_files_function() -> McpFunction {
	McpFunction {
		name: "list_files".to_string(),
		description: "List files in a directory, with optional pattern matching.
This tool uses ripgrep for efficient searching that respects .gitignore files.
You can use it to find files by name pattern or search for files containing specific content.".to_string(),
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

// Define the semantic_code_function for both view signatures and search
pub fn get_semantic_code_function() -> McpFunction {
	McpFunction {
		name: "semantic_code".to_string(),
		description: "Analyze and search code in the repository using both structural and semantic methods.

This tool can operate in two modes:

1. **Signatures View Mode**: Extracts function/method signatures and other declarations from code files to understand APIs without looking at the entire implementation.
2. **Semantic Search Mode**: Searches for code that matches a natural language query using semantic embeddings.

Use signatures mode when you want to understand what functions/methods are available in specific files.
Use search mode when you want to find specific functionality based on a natural language description of what the code does.

The tool returns results formatted in a clean, token-efficient Markdown output.".to_string(),
		parameters: json!({
			"type": "object",
			"required": ["mode"],
			"properties": {
				"mode": {
					"type": "string",
					"enum": ["signatures", "search"],
					"description": "The mode to use: 'signatures' to view function signatures and declarations, 'search' to perform semantic code search"
				},
				"files": {
					"type": "array",
					"items": {
						"type": "string"
					},
					"description": "[For signatures mode] Files to analyze. Can include glob patterns (e.g., 'src/*.rs', 'src/**/*.py')."
				},
				"query": {
					"type": "string",
					"description": "[For search mode] Natural language query to search for code"
				},
				"expand": {
					"type": "boolean",
					"description": "[For search mode] Whether to expand symbols in search results to include related code",
					"default": false
				}
			}
		}),
	}
}

// Execute a shell command
pub async fn execute_shell_command(call: &McpToolCall) -> Result<McpToolResult> {
	// Extract command parameter
	let command = match call.parameters.get("command") {
		Some(Value::String(cmd)) => cmd.clone(),
		_ => return Err(anyhow!("Missing or invalid 'command' parameter")),
	};

	// Execute the command
	let output = tokio::task::spawn_blocking(move || {
		let output = if cfg!(target_os = "windows") {
			Command::new("cmd")
				.args(["/C", &command])
				.output()
		} else {
			Command::new("sh")
				.args(["-c", &command])
				.output()
		};

		match output {
			Ok(output) => {
				let stdout = String::from_utf8_lossy(&output.stdout).to_string();
				let stderr = String::from_utf8_lossy(&output.stderr).to_string();

				// Format the output more clearly with error handling
				let combined = if stderr.is_empty() {
					stdout
				} else if stdout.is_empty() {
					stderr
				} else {
					format!("{}

Error: {}", stdout, stderr)
				};

				// Add detailed execution results including status code
				let status_code = output.status.code().unwrap_or(-1);
				let success = output.status.success();

				json!({
					"success": success,
					"output": combined,
					"code": status_code,
					"parameters": {
						"command": command
					},
					"message": if success {
						format!("Command executed successfully with exit code {}", status_code)
					} else {
					format!("Command failed with exit code {}", status_code)
			}
			})
			},
			Err(e) => json!({
				"success": false,
				"output": format!("Failed to execute command: {}", e),
				"code": -1,
				"parameters": {
					"command": command
				},
				"message": format!("Failed to execute command: {}", e)
			}),
		}
	}).await?;

	Ok(McpToolResult {
		tool_name: "shell".to_string(),
		tool_id: call.tool_id.clone(),
		result: output,
	})
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
						_ => return Err(anyhow!("'file_text' must be an array for multiple file writes")),
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

// Execute view_signatures command
pub async fn execute_view_signatures(call: &McpToolCall) -> Result<McpToolResult> {
	// Extract files parameter
	let files = match call.parameters.get("files") {
		Some(Value::Array(files_array)) => {
			// Convert JSON array to Vec<String>
			files_array.iter()
				.filter_map(|v| v.as_str().map(|s| s.to_string()))
				.collect::<Vec<String>>()
		},
		_ => return Err(anyhow!("Missing or invalid 'files' parameter, expected an array of strings")),
	};

	if files.is_empty() {
		return Err(anyhow!("No files specified"));
	}

	// Get current directory for resolving paths
	let current_dir = std::env::current_dir()?;

	// Process file patterns and find matching files
	let mut matching_files = Vec::new();

	for pattern in &files {
		// Use glob pattern matching
		let glob_pattern = match globset::Glob::new(pattern) {
			Ok(g) => g.compile_matcher(),
			Err(e) => {
				return Err(anyhow!("Invalid glob pattern '{}': {}", pattern, e));
			}
		};

		// Use ignore crate to respect .gitignore files while finding files
		let walker = ignore::WalkBuilder::new(&current_dir)
			.hidden(false)  // Don't ignore hidden files (unless in .gitignore)
			.git_ignore(true)  // Respect .gitignore files
			.git_global(true) // Respect global git ignore files
			.git_exclude(true) // Respect .git/info/exclude files
			.build();

		for result in walker {
			let entry = match result {
				Ok(entry) => entry,
				Err(_) => continue,
			};

			// Skip directories, only process files
			if !entry.file_type().map_or(false, |ft| ft.is_file()) {
				continue;
			}

			// See if this file matches our pattern
			let relative_path = entry.path().strip_prefix(&current_dir).unwrap_or(entry.path());
			if glob_pattern.is_match(relative_path) {
				matching_files.push(entry.path().to_path_buf());
			}
		}
	}

	if matching_files.is_empty() {
		return Ok(McpToolResult {
			tool_name: "view_signatures".to_string(),
			tool_id: call.tool_id.clone(),
			result: json!({
				"success": true,
				"output": "No matching files found.",
				"files": [],
				"count": 0,
				"parameters": {
					"files": files
				}
			}),
		});
	}

	// Extract signatures from matching files using the indexer module
	let signatures = match crate::indexer::extract_file_signatures(&matching_files) {
		Ok(sigs) => sigs,
		Err(e) => return Err(anyhow!("Failed to extract signatures: {}", e)),
	};

	// Format the results as text output
	let mut output = String::new();

	// Add header information
	if signatures.is_empty() {
		output.push_str("No signatures found.\n");
	} else {
		output.push_str(&format!("Found signatures in {} files:\n\n", signatures.len()));

		// Process each file
		for file in &signatures {
			output.push_str(&format!("╔══════════════════ File: {} ══════════════════\n", file.path));
			output.push_str(&format!("║ Language: {}\n", file.language));

			// Show file comment if available
			if let Some(comment) = &file.file_comment {
				output.push_str("║\n");
				output.push_str("║ File description:\n");
				for line in comment.lines() {
					output.push_str(&format!("║   {}\n", line));
				}
			}

			if file.signatures.is_empty() {
				output.push_str("║\n");
				output.push_str("║ No signatures found in this file.\n");
			} else {
				for signature in &file.signatures {
					output.push_str("║\n");

					// Display line range if it spans multiple lines, otherwise just the start line
					let line_display = if signature.start_line == signature.end_line {
						format!("{}", signature.start_line + 1)
					} else {
						format!("{}-{}", signature.start_line + 1, signature.end_line + 1)
					};

					output.push_str(&format!("║ {} `{}` (line {})\n", signature.kind, signature.name, line_display));

					// Show description if available
					if let Some(desc) = &signature.description {
						output.push_str("║ Description:\n");
						for line in desc.lines() {
							output.push_str(&format!("║   {}\n", line));
						}
					}

					// Format the signature for display
					output.push_str("║ Signature:\n");
					let lines = signature.signature.lines().collect::<Vec<_>>();
					if lines.len() > 1 {
						output.push_str("║ ┌────────────────────────────────────\n");
						for line in lines.iter().take(5) {
							output.push_str(&format!("║ │ {}\n", line));
						}
						// If signature is too long, truncate it
						if lines.len() > 5 {
							output.push_str(&format!("║ │ ... ({} more lines)\n", lines.len() - 5));
						}
						output.push_str("║ └────────────────────────────────────\n");
					} else if !lines.is_empty() {
						output.push_str(&format!("║   {}\n", lines[0]));
					}
				}
			}

			output.push_str("╚════════════════════════════════════════\n\n");
		}
	}

	// Return the result with both text and structured data
	Ok(McpToolResult {
		tool_name: "view_signatures".to_string(),

		tool_id: call.tool_id.clone(),
		result: json!({
			"success": true,
			"output": output,
			"files_analyzed": matching_files.len(),
			"signatures_found": signatures.iter().fold(0, |acc, file| acc + file.signatures.len()),
			"parameters": {
				"files": files
			}
		}),
	})
}

// Execute semantic_code function (both signatures and search modes)
pub async fn execute_semantic_code(call: &McpToolCall, store: &crate::store::Store, config: &crate::config::Config) -> Result<McpToolResult> {
	// Extract mode parameter
	let mode = match call.parameters.get("mode") {
		Some(Value::String(m)) => m.as_str(),
		_ => return Err(anyhow!("Missing or invalid 'mode' parameter. Must be 'signatures' or 'search'")),
	};

	match mode {
		"signatures" => execute_signatures_mode(call).await,
		"search" => execute_search_mode(call, store, config).await,
		_ => Err(anyhow!("Invalid mode: {}. Must be 'signatures' or 'search'", mode)),
	}
}

// Implementation of signatures mode
async fn execute_signatures_mode(call: &McpToolCall) -> Result<McpToolResult> {
	// Extract files parameter
	let files = match call.parameters.get("files") {
		Some(Value::Array(files_array)) => {
			// Convert JSON array to Vec<String>
			files_array.iter()
				.filter_map(|v| v.as_str().map(|s| s.to_string()))
				.collect::<Vec<String>>()
		},
		_ => return Err(anyhow!("Missing or invalid 'files' parameter, expected an array of strings")),
	};

	if files.is_empty() {
		return Err(anyhow!("No files specified"));
	}

	// Get current directory for resolving paths
	let current_dir = std::env::current_dir()?;

	// Process file patterns and find matching files
	let mut matching_files = Vec::new();

	for pattern in &files {
		// Use glob pattern matching
		let glob_pattern = match globset::Glob::new(pattern) {
			Ok(g) => g.compile_matcher(),
			Err(e) => {
				return Err(anyhow!("Invalid glob pattern '{}': {}", pattern, e));
			}
		};

		// Use ignore crate to respect .gitignore files while finding files
		let walker = ignore::WalkBuilder::new(&current_dir)
			.hidden(false)  // Don't ignore hidden files (unless in .gitignore)
			.git_ignore(true)  // Respect .gitignore files
			.git_global(true) // Respect global git ignore files
			.git_exclude(true) // Respect .git/info/exclude files
			.build();

		for result in walker {
			let entry = match result {
				Ok(entry) => entry,
				Err(_) => continue,
			};

			// Skip directories, only process files
			if !entry.file_type().map_or(false, |ft| ft.is_file()) {
				continue;
			}

			// See if this file matches our pattern
			let relative_path = entry.path().strip_prefix(&current_dir).unwrap_or(entry.path());
			if glob_pattern.is_match(relative_path) {
				matching_files.push(entry.path().to_path_buf());
			}
		}
	}

	if matching_files.is_empty() {
		return Ok(McpToolResult {
			tool_name: "semantic_code".to_string(),

			tool_id: call.tool_id.clone(),
			result: json!({
				"success": true,
				"output": "No matching files found.",
				"files": [],
				"count": 0,
				"parameters": {
					"mode": "signatures",
					"files": files
				}
			}),
		});
	}

	// Extract signatures from matching files using the indexer module
	let signatures = match crate::indexer::extract_file_signatures(&matching_files) {
		Ok(sigs) => sigs,
		Err(e) => return Err(anyhow!("Failed to extract signatures: {}", e)),
	};

	// Format the results as markdown
	let markdown_output = crate::indexer::signatures_to_markdown(&signatures);

	// Return the result with both text and structured data
	Ok(McpToolResult {
		tool_name: "semantic_code".to_string(),

		tool_id: call.tool_id.clone(),
		result: json!({
			"success": true,
			"output": markdown_output,
			"files_analyzed": matching_files.len(),
			"signatures_found": signatures.iter().fold(0, |acc, file| acc + file.signatures.len()),
			"parameters": {
				"mode": "signatures",
				"files": files
			}
		}),
	})
}

// Implementation of search mode
async fn execute_search_mode(call: &McpToolCall, store: &crate::store::Store, config: &crate::config::Config) -> Result<McpToolResult> {
	// Extract query parameter
	let query = match call.parameters.get("query") {
		Some(Value::String(q)) => q.clone(),
		_ => return Err(anyhow!("Missing or invalid 'query' parameter, expected a string")),
	};

	if query.trim().is_empty() {
		return Err(anyhow!("Query cannot be empty"));
	}

	// Extract optional expand parameter
	let expand = call.parameters.get("expand")
		.and_then(|v| v.as_bool())
		.unwrap_or(false);

	// Get current directory for checking index
	let current_dir = std::env::current_dir()?;
	let octodev_dir = current_dir.join(".octodev");
	let index_path = octodev_dir.join("storage");

	// Check if we have an index, which is required for search
	if !index_path.exists() {
		return Err(anyhow!("No index found. Please run 'octodev index' first before using search."));
	}

	// Generate embeddings for the query
	let embeddings = match crate::indexer::generate_embeddings(&query, true, config).await {
		Ok(emb) => emb,
		Err(e) => return Err(anyhow!("Failed to generate query embeddings: {}", e)),
	};

	// Search for matching code blocks
	let mut results = match store.get_code_blocks(embeddings).await {
		Ok(res) => res,
		Err(e) => return Err(anyhow!("Failed to search for code blocks: {}", e)),
	};

	// If expand flag is set, expand symbols in the results
	if expand {
		results = match crate::indexer::expand_symbols(store, results).await {
			Ok(expanded) => expanded,
			Err(e) => return Err(anyhow!("Failed to expand symbols: {}", e)),
		};
	}

	// Format the results as markdown
	let markdown_output = crate::indexer::code_blocks_to_markdown(&results);

	// Return the result
	Ok(McpToolResult {
		tool_name: "semantic_code".to_string(),

		tool_id: call.tool_id.clone(),
		result: json!({
			"success": true,
			"output": markdown_output,
			"blocks_found": results.len(),
			"parameters": {
				"mode": "search",
				"query": query,
				"expand": expand
			}
		}),
	})
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

// Undo the last edit to a file
async fn undo_edit(call: &McpToolCall, path: &Path) -> Result<McpToolResult> {
	let path_str = path.to_string_lossy().to_string();

	// First retrieve the previous content while holding the lock
	let previous_content = {
		let file_history = get_file_history();
		let mut history_guard = file_history.lock().map_err(|_| anyhow!("Failed to acquire lock on file history"))?;

		if let Some(history) = history_guard.get_mut(&path_str) {
			if let Some(content) = history.pop() {
				// Return the previous content
				Some(content)
			} else {
				None
			}
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

// Get all available developer functions
pub fn get_all_functions() -> Vec<McpFunction> {
	vec![
		get_shell_function(),
		get_text_editor_function(),
		get_list_files_function(),
		get_semantic_code_function(),
		get_graphrag_function(),
	]
}

// Define the GraphRAG function
pub fn get_graphrag_function() -> McpFunction {
	McpFunction {
		name: "graphrag".to_string(),
		description: "Query and explore the code relationship graph (GraphRAG) built during indexing.

This tool allows you to explore the code knowledge graph that was built during indexing, which contains
code entities and their relationships. This semantic graph helps in understanding complex codebases and
finding connections between different parts of the code.

Operations:
- `search`: Find code nodes that match a semantic query
  - Use with `task_focused: true` for an optimized, token-efficient view focused on your specific task
- `get_node`: Get detailed information about a specific node by ID
- `get_relationships`: Find relationships involving a specific node
- `find_path`: Find paths between two nodes in the graph
- `overview`: Get an overview of the entire graph structure

Use this tool to understand how different parts of the code are related and to explore the codebase
from a structural perspective.".to_string(),
		parameters: json!({
			"type": "object",
			"required": ["operation"],
			"properties": {
				"operation": {
					"type": "string",
					"enum": ["search", "get_node", "get_relationships", "find_path", "overview"],
					"description": "The GraphRAG operation to perform"
				},
				"query": {
					"type": "string",
					"description": "[For search operation] The semantic query to search for"
				},
				"task_focused": {
					"type": "boolean",
					"description": "[For search operation] Whether to use task-focused optimization to provide a more concise, relevant view",
					"default": false
				},
				"node_id": {
					"type": "string",
					"description": "[For get_node/get_relationships operations] The ID of the node to get information about"
				},
				"source_id": {
					"type": "string",
					"description": "[For find_path operation] The ID of the source node"
				},
				"target_id": {
					"type": "string",
					"description": "[For find_path operation] The ID of the target node"
				},
				"max_depth": {
					"type": "integer",
					"description": "[For find_path operation] The maximum path length to consider",
					"default": 3
				}
			}
		}),
	}
}

// Execute GraphRAG operations
pub async fn execute_graphrag(call: &McpToolCall, config: &crate::config::Config) -> Result<McpToolResult> {
	// Extract operation parameter
	let operation = match call.parameters.get("operation") {
		Some(Value::String(op)) => op.as_str(),
		_ => return Err(anyhow!("Missing or invalid 'operation' parameter")),
	};

	// Check if GraphRAG is enabled
	if !config.graphrag.enabled {
		return Ok(McpToolResult {
			tool_name: "graphrag".to_string(),
			tool_id: call.tool_id.clone(),
			result: json!({
				"success": false,
				"error": "GraphRAG is not enabled. Enable it in your configuration and run 'octodev index' to build the knowledge graph.",
				"message": "To enable GraphRAG, add the following to your .octodev/config.toml file:\n\n[graphrag]\nenabled = true\n\nThen run 'octodev index' to build the knowledge graph."
			}),
		});
	}

	// Initialize the GraphBuilder
	let graph_builder = match crate::indexer::GraphBuilder::new(config.clone()).await {
		Ok(builder) => builder,
		Err(e) => return Err(anyhow!("Failed to initialize GraphBuilder: {}", e)),
	};

	// Execute the requested operation
	match operation {
		"search" => execute_graphrag_search(call, &graph_builder).await,
		"get_node" => execute_graphrag_get_node(call, &graph_builder).await,
		"get_relationships" => execute_graphrag_get_relationships(call, &graph_builder).await,
		"find_path" => execute_graphrag_find_path(call, &graph_builder).await,
		"overview" => execute_graphrag_overview(call, &graph_builder).await,
		_ => Err(anyhow!("Invalid operation: {}", operation)),
	}
}

// Search for nodes in the graph
async fn execute_graphrag_search(call: &McpToolCall, graph_builder: &crate::indexer::GraphBuilder) -> Result<McpToolResult> {
    // Extract query parameter
    let query = match call.parameters.get("query") {
        Some(Value::String(q)) => q.clone(),
        _ => return Err(anyhow!("Missing or invalid 'query' parameter for search operation")),
    };

    // Check for task-focused flag
    let task_focused = call.parameters.get("task_focused")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
        
    if task_focused {
        // Use the graph optimizer for task-focused search
        let store = crate::store::Store::new().await?;
        let config = crate::config::Config::load().unwrap_or_default();
        
        // Get the full graph
        let full_graph = graph_builder.get_graph().await?;
        
        // Generate embeddings for the query
        let query_embedding = crate::indexer::generate_embeddings(&query, false, &config).await?;
        
        // Create optimizer with token budget
        let optimizer = crate::indexer::graph_optimization::GraphOptimizer::new(2000);
        
        // Get all code blocks
        let code_blocks = store.get_code_blocks(query_embedding.clone()).await?;
        
        // Generate a task-focused view
        let task_view = optimizer.generate_task_focused_view(
            &query,
            &query_embedding,
            &full_graph,
            &code_blocks
        ).await?;
        
        // Return the optimized view
        return Ok(McpToolResult {
            tool_name: "graphrag".to_string(),
            tool_id: call.tool_id.clone(),
            result: json!({
                "success": true,
                "output": task_view,
                "task_focused": true,
                "parameters": {
                    "operation": "search",
                    "query": query,
                    "task_focused": true
                }
            }),
        });
    }

    // Traditional node search (without task focusing)
    let nodes = graph_builder.search_nodes(&query).await?;

    // Format the results as markdown
    let mut markdown = String::from(format!("# GraphRAG Search Results for '{}'\n\n", query));
    markdown.push_str(&format!("Found {} matching nodes\n\n", nodes.len()));

    // Add each node to the markdown output
    for node in &nodes {
        markdown.push_str(&format!("## {}\n", node.name));
        markdown.push_str(&format!("**ID**: {}\n", node.id));
        markdown.push_str(&format!("**Kind**: {}\n", node.kind));
        markdown.push_str(&format!("**Path**: {}\n", node.path));
        markdown.push_str(&format!("**Description**: {}\n\n", node.description));
    }

    // Return the results
    Ok(McpToolResult {
        tool_name: "graphrag".to_string(),
        tool_id: call.tool_id.clone(),
        result: json!({
            "success": true,
            "output": markdown,
            "count": nodes.len(),
            "nodes": nodes,
            "parameters": {
                "operation": "search",
                "query": query
            }
        }),
    })
}

// Get details about a specific node
async fn execute_graphrag_get_node(call: &McpToolCall, graph_builder: &crate::indexer::GraphBuilder) -> Result<McpToolResult> {
	// Extract node_id parameter
	let node_id = match call.parameters.get("node_id") {
		Some(Value::String(id)) => id.clone(),
		_ => return Err(anyhow!("Missing or invalid 'node_id' parameter for get_node operation")),
	};

	// Get the graph
	let graph = graph_builder.get_graph().await?;

	// Check if the node exists
	let node = match graph.nodes.get(&node_id) {
		Some(node) => node.clone(),
		None => return Err(anyhow!("Node not found: {}", node_id)),
	};

	// Format the result as markdown
	let mut markdown = String::from(format!("# Node: {}\n\n", node.name));
	markdown.push_str(&format!("**ID**: {}\n", node.id));
	markdown.push_str(&format!("**Kind**: {}\n", node.kind));
	markdown.push_str(&format!("**Path**: {}\n", node.path));
	markdown.push_str(&format!("**Description**: {}\n\n", node.description));

	// Add symbols if any
	if !node.symbols.is_empty() {
		markdown.push_str("## Symbols\n\n");
		for symbol in &node.symbols {
			markdown.push_str(&format!("- `{}`\n", symbol));
		}
		markdown.push_str("\n");
	}

	// Return the result
	Ok(McpToolResult {
		tool_name: "graphrag".to_string(),
		tool_id: call.tool_id.clone(),
		result: json!({
			"success": true,
			"output": markdown,
			"node": node,
			"parameters": {
				"operation": "get_node",
				"node_id": node_id
			}
		}),
	})
}

// Get relationships involving a specific node
async fn execute_graphrag_get_relationships(call: &McpToolCall, graph_builder: &crate::indexer::GraphBuilder) -> Result<McpToolResult> {
	// Extract node_id parameter
	let node_id = match call.parameters.get("node_id") {
		Some(Value::String(id)) => id.clone(),
		_ => return Err(anyhow!("Missing or invalid 'node_id' parameter for get_relationships operation")),
	};

	// Get the graph
	let graph = graph_builder.get_graph().await?;

	// Check if the node exists
	if !graph.nodes.contains_key(&node_id) {
		return Err(anyhow!("Node not found: {}", node_id));
	}

	// Find relationships where this node is either source or target
	let relationships: Vec<_> = graph.relationships.iter()
		.filter(|rel| rel.source == node_id || rel.target == node_id)
		.cloned()
		.collect();

	// Format the result as markdown
	let mut markdown = format!("# Relationships for Node: {}\n\n", node_id);
	
	if relationships.is_empty() {
		markdown.push_str("No relationships found for this node.\n");
	} else {
		markdown.push_str(&format!("Found {} relationships:\n\n", relationships.len()));

		// Outgoing relationships
		let outgoing: Vec<_> = relationships.iter()
			.filter(|rel| rel.source == node_id)
			.collect();
		
		if !outgoing.is_empty() {
			markdown.push_str("## Outgoing Relationships\n\n");
			for rel in outgoing {
				let target_name = graph.nodes.get(&rel.target)
					.map(|n| n.name.clone())
					.unwrap_or_else(|| rel.target.clone());
				
				markdown.push_str(&format!("- **{}** → {} ({}): {}\n", 
					rel.relation_type,
					target_name,
					rel.target,
					rel.description));
			}
			markdown.push_str("\n");
		}

		// Incoming relationships
		let incoming: Vec<_> = relationships.iter()
			.filter(|rel| rel.target == node_id)
			.collect();
		
		if !incoming.is_empty() {
			markdown.push_str("## Incoming Relationships\n\n");
			for rel in incoming {
				let source_name = graph.nodes.get(&rel.source)
					.map(|n| n.name.clone())
					.unwrap_or_else(|| rel.source.clone());
				
				markdown.push_str(&format!("- **{}** ← {} ({}): {}\n", 
					rel.relation_type,
					source_name,
					rel.source,
					rel.description));
			}
			markdown.push_str("\n");
		}
	}

	// Return the result
	Ok(McpToolResult {
		tool_name: "graphrag".to_string(),
		tool_id: call.tool_id.clone(),
		result: json!({
			"success": true,
			"output": markdown,
			"relationships": relationships,
			"count": relationships.len(),
			"parameters": {
				"operation": "get_relationships",
				"node_id": node_id
			}
		}),
	})
}

// Find paths between two nodes
async fn execute_graphrag_find_path(call: &McpToolCall, graph_builder: &crate::indexer::GraphBuilder) -> Result<McpToolResult> {
	// Extract parameters
	let source_id = match call.parameters.get("source_id") {
		Some(Value::String(id)) => id.clone(),
		_ => return Err(anyhow!("Missing or invalid 'source_id' parameter for find_path operation")),
	};
	
	let target_id = match call.parameters.get("target_id") {
		Some(Value::String(id)) => id.clone(),
		_ => return Err(anyhow!("Missing or invalid 'target_id' parameter for find_path operation")),
	};
	
	let max_depth = match call.parameters.get("max_depth") {
		Some(Value::Number(n)) => n.as_u64().unwrap_or(3) as usize,
		_ => 3, // Default to depth 3
	};

	// Find paths
	let paths = graph_builder.find_paths(&source_id, &target_id, max_depth).await?;

	// Get the graph for node name lookup
	let graph = graph_builder.get_graph().await?;

	// Format the result as markdown
	let mut markdown = format!("# Paths from '{}' to '{}'\n\n", source_id, target_id);
	
	if paths.is_empty() {
		markdown.push_str("No paths found between these nodes within the specified depth.\n");
	} else {
		markdown.push_str(&format!("Found {} paths with max depth {}:\n\n", paths.len(), max_depth));

		for (i, path) in paths.iter().enumerate() {
			markdown.push_str(&format!("## Path {}\n\n", i + 1));
			
			// Display each node in the path
			for (j, node_id) in path.iter().enumerate() {
				let node_name = graph.nodes.get(node_id)
					.map(|n| n.name.clone())
					.unwrap_or_else(|| node_id.clone());
				
				if j > 0 {
					// Look up the relationship
					let prev_id = &path[j-1];
					let rel = graph.relationships.iter()
						.find(|r| r.source == *prev_id && r.target == *node_id);
					
					if let Some(rel) = rel {
						markdown.push_str(&format!("→ **{}** → ", rel.relation_type));
					} else {
						markdown.push_str("→ ");
					}
				}
				
				markdown.push_str(&format!("`{}` ({})", node_name, node_id));
			}
			markdown.push_str("\n\n");
		}
	}

	// Return the result
	Ok(McpToolResult {
		tool_name: "graphrag".to_string(),
		tool_id: call.tool_id.clone(),
		result: json!({
			"success": true,
			"output": markdown,
			"paths": paths,
			"count": paths.len(),
			"parameters": {
				"operation": "find_path",
				"source_id": source_id,
				"target_id": target_id,
				"max_depth": max_depth
			}
		}),
	})
}

// Get an overview of the graph
async fn execute_graphrag_overview(call: &McpToolCall, graph_builder: &crate::indexer::GraphBuilder) -> Result<McpToolResult> {
	// Get the graph
	let graph = graph_builder.get_graph().await?;

	// Get statistics
	let node_count = graph.nodes.len();
	let relationship_count = graph.relationships.len();

	// Count node types
	let mut node_types = std::collections::HashMap::new();
	for node in graph.nodes.values() {
		*node_types.entry(node.kind.clone()).or_insert(0) += 1;
	}

	// Count relationship types
	let mut rel_types = std::collections::HashMap::new();
	for rel in &graph.relationships {
		*rel_types.entry(rel.relation_type.clone()).or_insert(0) += 1;
	}

	// Format the result as markdown
	let mut markdown = String::from("# GraphRAG Knowledge Graph Overview\n\n");
	markdown.push_str(&format!("The knowledge graph contains {} nodes and {} relationships.\n\n", node_count, relationship_count));

	// Node type statistics
	markdown.push_str("## Node Types\n\n");
	for (kind, count) in node_types.iter() {
		markdown.push_str(&format!("- {}: {} nodes\n", kind, count));
	}
	markdown.push_str("\n");

	// Relationship type statistics
	markdown.push_str("## Relationship Types\n\n");
	for (rel_type, count) in rel_types.iter() {
		markdown.push_str(&format!("- {}: {} relationships\n", rel_type, count));
	}

	// Return the result
	Ok(McpToolResult {
		tool_name: "graphrag".to_string(),
		tool_id: call.tool_id.clone(),
		result: json!({
			"success": true,
			"output": markdown,
			"node_count": node_count,
			"relationship_count": relationship_count,
			"node_types": node_types,
			"relationship_types": rel_types,
			"parameters": {
				"operation": "overview"
			}
		}),
	})
}
