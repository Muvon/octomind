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
		description: "Perform text editing operations on files.

			The `command` parameter specifies the operation to perform. Allowed options are:
			- `view`: View the content of one or multiple files. The path parameter accepts either a single string path or an array of paths.
			- `write`: Create or overwrite file(s). For single file, use a string path and string file_text. For multiple files, use arrays for both paths and file_texts (must be same length).
			- `str_replace`: Replace a string in a file with a new string.
			- `undo_edit`: Undo the last edit made to a file.

			To use the view command, you can either provide a single file path as a string or an array of file paths to view multiple files at once.
			For example: `{\"path\": \"/path/to/file.txt\"}` or `{\"path\": [\"/path/to/file1.txt\", \"/path/to/file2.txt\"]}`.

			To use the write command for a single file:
`{\"command\": \"write\", \"path\": \"/path/to/file.txt\", \"file_text\": \"content\"}`

To write multiple files at once (paths and file_texts arrays must have the same length):
`{\"command\": \"write\", \"path\": [\"/path/file1.txt\", \"/path/file2.txt\"], \"file_text\": [\"content1\", \"content2\"]}`

			To use the str_replace command, you must specify both `old_str` and `new_str` - the `old_str` needs to exactly match one
			unique section of the original file, including any whitespace. Make sure to include enough context that the match is not
			ambiguous. The entire original string will be replaced with `new_str`.".to_string(),
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
				"old_str": {"type": "string"},
				"new_str": {"type": "string"},
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
					view_file(Path::new(p)).await
				},
				Value::Array(paths) => {
					// Multiple files view
					let path_strings: Result<Vec<String>, _> = paths.iter()
						.map(|p| p.as_str().ok_or_else(|| anyhow!("Invalid path in array")))
						.map(|r| r.map(|s| s.to_string()))
						.collect();

					match path_strings {
						Ok(path_strs) => view_multiple_files(&path_strs).await,
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
					write_file(Path::new(p), &file_text).await
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
								(Ok(paths), Ok(contents)) => write_multiple_files(&paths, &contents).await,
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
			let path = match path_value.as_str() {
				Some(p) => p,
				_ => return Err(anyhow!("'path' parameter must be a string for str_replace operations")),
			};

			let old_str = match call.parameters.get("old_str") {
				Some(Value::String(s)) => s.clone(),
				_ => return Err(anyhow!("Missing or invalid 'old_str' parameter")),
			};
			let new_str = match call.parameters.get("new_str") {
				Some(Value::String(s)) => s.clone(),
				_ => return Err(anyhow!("Missing or invalid 'new_str' parameter")),
			};
			replace_string_in_file(Path::new(path), &old_str, &new_str).await
		},
		"undo_edit" => {
			let path = match path_value.as_str() {
				Some(p) => p,
				_ => return Err(anyhow!("'path' parameter must be a string for undo_edit operations")),
			};

			undo_edit(Path::new(path)).await
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
		result: output,
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
async fn view_file(path: &Path) -> Result<McpToolResult> {
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

	// Return a single file in the same format as multiple files for consistency
	Ok(McpToolResult {
		tool_name: "text_editor".to_string(),
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
	})
}

// Write content to a single file
async fn write_file(path: &Path, content: &str) -> Result<McpToolResult> {
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

	// Return success in the same format as multiple file write for consistency
	Ok(McpToolResult {
		tool_name: "text_editor".to_string(),
		result: json!({
			"success": true,
			"files": [{
				"path": path.to_string_lossy(),
				"success": true,
				"size": content.len()
			}],
			"count": 1
		}),
	})
}

// Write content to multiple files
async fn write_multiple_files(paths: &[String], contents: &[String]) -> Result<McpToolResult> {
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
		result: json!({
			"success": !results.is_empty(),
			"files": results,
			"count": results.len(),
			"failed": failures
		}),
	})
}

// Replace a string in a file
async fn replace_string_in_file(path: &Path, old_str: &str, new_str: &str) -> Result<McpToolResult> {
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

	Ok(McpToolResult {
		tool_name: "text_editor".to_string(),
		result: json!({
			"success": true,
			"output": format!("Successfully replaced string at line {}", start_line),
			"path": path.to_string_lossy(),
			"old_size": old_str.len(),
			"new_size": new_str.len(),
			"line": start_line,
			"parameters": {
				"command": "str_replace",
				"path": path.to_string_lossy(),
				"old_str": old_str,
				"new_str": new_str
			}
		}),
	})
}

// Undo the last edit to a file
async fn undo_edit(path: &Path) -> Result<McpToolResult> {
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
async fn view_multiple_files(paths: &[String]) -> Result<McpToolResult> {
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
	]
}
