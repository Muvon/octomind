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
		format!("{}/.local/share/fish/fish_history", home)
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
		format!(": {}:0;{}\n", timestamp, command)
	} else if shell.contains("fish") {
		let timestamp = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.unwrap_or_default()
			.as_secs();
		format!("- cmd: {}\n  when: {}\n", command, timestamp)
	} else {
		// Bash format
		format!("{}\n", command)
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
".to_string(),
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

// Execute a shell command
pub async fn execute_shell_command(
	call: &McpToolCall,
	cancellation_token: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> Result<McpToolResult> {
	use std::sync::atomic::Ordering;
	use tokio::process::Command as TokioCommand;

	// Extract command parameter
	let command = match call.parameters.get("command") {
		Some(Value::String(cmd)) => cmd.clone(),
		_ => return Err(anyhow!("Missing or invalid 'command' parameter")),
	};

	// Check for cancellation before starting
	if let Some(ref token) = cancellation_token {
		if token.load(Ordering::SeqCst) {
			return Err(anyhow!("Shell command execution cancelled"));
		}
	}

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

	// Configure the command
	cmd.stdout(std::process::Stdio::piped())
		.stderr(std::process::Stdio::piped())
		.stdin(std::process::Stdio::null())
		.kill_on_drop(true); // CRITICAL: Kill process when dropped

	// Spawn the process
	let child = cmd
		.spawn()
		.map_err(|e| anyhow!("Failed to spawn command: {}", e))?;

	// Get the process ID for potential killing
	let child_id = child.id();

	// Create a cancellation future
	let cancellation_future = async {
		if let Some(ref token) = cancellation_token {
			loop {
				tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
				if token.load(Ordering::SeqCst) {
					return true; // Indicate cancellation occurred
				}
			}
		} else {
			std::future::pending::<bool>().await
		}
	};

	// Race between command completion and cancellation
	let output = tokio::select! {
			result = child.wait_with_output() => {
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
							format!(
								"{}

Error: {}",
								stdout, stderr
							)
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
				}
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
		}
		cancelled = cancellation_future => {
			if cancelled {
				// Try to kill the process using system commands if we have the PID
				if let Some(pid) = child_id {
					#[cfg(unix)]
					{
						// On Unix systems, try to kill the process using system commands
						let _ = std::process::Command::new("kill")
							.args(["-TERM", &pid.to_string()])
							.output();
						// Give it a moment to terminate gracefully
						std::thread::sleep(std::time::Duration::from_millis(100));
						let _ = std::process::Command::new("kill")
							.args(["-KILL", &pid.to_string()])
							.output();
					}
					#[cfg(windows)]
					{
						// On Windows, use taskkill
						let _ = std::process::Command::new("taskkill")
							.args(["/F", "/PID", &pid.to_string()])
							.output();
					}
				}

				json!({
					"success": false,
					"output": "Command execution cancelled by user (Ctrl+C)",
					"code": -1,
					"parameters": {
						"command": command
					},
					"message": "Command execution cancelled by user"
				})
			} else {
				// This shouldn't happen, but handle it gracefully
				json!({
					"success": false,
					"output": "Unexpected cancellation state",
					"code": -1,
					"parameters": {
						"command": command
					},
					"message": "Unexpected cancellation state"
				})
			}
		}
	};

	Ok(McpToolResult {
		tool_name: "shell".to_string(),
		tool_id: call.tool_id.clone(),
		result: output,
	})
}
