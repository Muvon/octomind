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

// Shell execution functionality for the Developer MCP provider

use super::super::{McpFunction, McpToolCall, McpToolResult};
use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::fs::OpenOptions;
use std::io::Write;

// Function to add command to shell history
fn add_to_shell_history(command: &str) -> Result<()> {
	// Get the shell and history file path
	let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
	let home = std::env::var("HOME")?;

	// Try to get HISTFILE environment variable first, fallback to default locations
	let history_file = if let Ok(histfile) = std::env::var("HISTFILE") {
		histfile
	} else if shell.contains("zsh") {
		format!("{}/.zsh_history", home)
	} else if shell.contains("bash") {
		format!("{}/.bash_history", home)
	} else if shell.contains("fish") {
		format!("{home}/.local/share/fish/fish_history")
	} else {
		// Default to bash history
		format!("{}/.bash_history", home)
	};

	// For zsh, we need to add timestamp and format correctly
	let history_entry = if shell.contains("zsh") {
		let timestamp = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.unwrap_or_default()
			.as_secs();
		format!(": {timestamp}:0;{command}\n")
	} else if shell.contains("fish") {
		let timestamp = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.unwrap_or_default()
			.as_secs();
		format!("- cmd: {command}\n  when: {timestamp}\n")
	} else {
		// Bash format
		format!("{command}\n")
	};

	// Append to history file
	match OpenOptions::new()
		.create(true)
		.append(true)
		.open(&history_file)
	{
		Ok(mut file) => {
			let _ = file.write_all(history_entry.as_bytes());
			let _ = file.flush();
		}
		Err(_) => {
			// If we can't write to history file, just continue silently
			// This prevents the tool from failing if history file is not writable
		}
	}

	Ok(())
}

// Define the shell function for the MCP protocol with enhanced description
pub fn get_shell_function() -> McpFunction {
	McpFunction {
		name: "shell".to_string(),
		description: "Execute a command in the shell.

This will return the output and error concatenated into a single string, as
you would see from running on the command line. There will also be an indication
of if the command succeeded or failed.

Parameters:
- `command`: The shell command to execute (required)
- `background`: Run command in background and return PID instead of waiting for completion (default: false)

**Working Directory:**
All commands execute from the current working directory.
NO `cd` command is required when you working with current project files.
REMEMBER that each command you run has NO knowledge of previous runs, other context, or variables that were set BEFORE in another command.

**Output Truncation:**
Output size is controlled by global mcp_response_tokens_threshold setting.
Avoid commands that produce large outputs (like `cat large_file` or `find /` without filters).
Use more specific commands to reduce output size if responses are truncated.

**Background Execution:**
When `background` is true, the command runs in the background and returns immediately with the process PID.
Background processes continue running until explicitly killed or the main application exits.
Use the returned PID with `kill <pid>` command to terminate background processes.
NO need to append `&` to your command when using `background: true` - this is handled automatically.

**Important**: Each shell command runs in its own process. Things like directory changes or
sourcing files do not persist between tool calls. So you may need to repeat them each time by
stringing together commands, e.g. `cd example && ls` or `source env/bin/activate && pip install numpy`

**Important**: Use ripgrep - `rg` - when you need to locate a file or a code reference, other solutions
may show ignored or hidden files. For example *do not* use `find` or `ls -r`
- List files by name: `rg --files | rg <filename>`
- List files that contain a regex: `rg '<regex>' -l`

Examples:
- Foreground: `{\"command\": \"ls -la\"}`
- Background: `{\"command\": \"python -m http.server 8000\", \"background\": true}`
- Kill background: `{\"command\": \"kill 12345\"}` (where 12345 is the returned PID)
".to_string(),
		parameters: json!({
			"type": "object",
			"properties": {
				"command": {
					"type": "string",
					"description": "The shell command to execute (runs from current working directory)"
				},
				"background": {
					"type": "boolean",
					"default": false,
					"description": "Run command in background and return PID instead of waiting for completion (no need to append '&')"
				}
			},
			"required": ["command"]
		}),
	}
}

// Execute a shell command
pub async fn execute_shell_command(call: &McpToolCall) -> Result<McpToolResult> {
	use tokio::process::Command as TokioCommand;

	// Extract command parameter
	let command = match call.parameters.get("command") {
		Some(Value::String(cmd)) => {
			if cmd.trim().is_empty() {
				return Ok(McpToolResult::error(
					call.tool_name.clone(),
					call.tool_id.clone(),
					"Command parameter cannot be empty".to_string(),
				));
			}
			cmd.clone()
		}
		Some(_) => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Command parameter must be a string".to_string(),
			));
		}
		None => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Missing required 'command' parameter".to_string(),
			));
		}
	};

	// Extract background parameter
	let background = call
		.parameters
		.get("background")
		.and_then(|v| v.as_bool())
		.unwrap_or(false);

	// Add command to shell history before execution
	let _ = add_to_shell_history(&command);

	// Use tokio::process::Command for better cancellation support
	let mut cmd = if cfg!(target_os = "windows") {
		let mut cmd = TokioCommand::new("cmd");
		cmd.args(["/C", &command]);
		cmd
	} else {
		let mut cmd = TokioCommand::new("sh");
		cmd.args(["-c", &command]);
		cmd
	};

	// Configure the command based on execution mode
	if background {
		// Background execution: detach process and return PID immediately
		cmd.stdout(std::process::Stdio::null())
			.stderr(std::process::Stdio::null())
			.stdin(std::process::Stdio::null())
			.kill_on_drop(false); // Don't kill when dropped - let it run independently
	} else {
		// Foreground execution: capture output and wait for completion
		cmd.stdout(std::process::Stdio::piped())
			.stderr(std::process::Stdio::piped())
			.stdin(std::process::Stdio::null())
			.kill_on_drop(true); // CRITICAL: Kill process when dropped
	}

	// Spawn the process
	let child = cmd
		.spawn()
		.map_err(|e| anyhow!("Failed to spawn command: {}", e))?;

	// Handle background vs foreground execution
	if background {
		// Background execution: return PID immediately
		let pid = child
			.id()
			.ok_or_else(|| anyhow!("Failed to get process ID"))?;

		// Detach the child process so it continues running independently
		// We do this by forgetting the child handle, which prevents kill_on_drop
		std::mem::forget(child);

		return Ok(McpToolResult {
			tool_name: "shell".to_string(),
			tool_id: call.tool_id.clone(),
			result: json!({
				"success": true,
				"background": true,
				"pid": pid,
				"command": command,
				"message": format!("Command started in background with PID {pid}"),
				"note": format!("Use 'kill {pid}' to terminate this background process if needed")
			}),
		});
	}

	// Foreground execution: wait for completion and return output
	let result = child.wait_with_output().await;
	match result.map_err(|e| anyhow!("Command execution failed: {}", e)) {
		Ok(output) => {
			let stdout = String::from_utf8_lossy(&output.stdout).to_string();
			let stderr = String::from_utf8_lossy(&output.stderr).to_string();

			// Format the output more clearly with error handling
			let combined = if stderr.is_empty() {
				stdout
			} else if stdout.is_empty() {
				stderr
			} else {
				format!("{stdout}\n\nError: {stderr}")
			};

			// Apply global truncation (handled by global MCP response truncation)
			let final_output = combined;

			// Add detailed execution results including status code
			let status_code = output.status.code().unwrap_or(-1);
			let success = output.status.success();

			// MCP Protocol Compliance: Use error() for failed commands, success() for successful ones
			if success {
				// Command succeeded - use success format
				Ok(McpToolResult::success(
					"shell".to_string(),
					call.tool_id.clone(),
					final_output,
				))
			} else {
				// Command failed - use error format per MCP protocol
				Ok(McpToolResult::error(
					"shell".to_string(),
					call.tool_id.clone(),
					format!(
						"Command failed with exit code {status_code}\nCommand: {command}\n\nOutput:\n{final_output}"
					),
				))
			}
		}
		Err(e) => Ok(McpToolResult::error(
			"shell".to_string(),
			call.tool_id.clone(),
			format!("Error: {e}"),
		)),
	}
}
