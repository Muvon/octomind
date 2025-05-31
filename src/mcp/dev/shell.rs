// Shell execution functionality for the Developer MCP provider

use std::process::Command;
use std::fs::OpenOptions;
use std::io::Write;
use serde_json::{json, Value};
use anyhow::{Result, anyhow};
use super::super::{McpToolCall, McpToolResult, McpFunction};

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
pub async fn execute_shell_command(call: &McpToolCall) -> Result<McpToolResult> {
	execute_shell_command_with_cancellation(call, None).await
}

// Execute a shell command with cancellation support
pub async fn execute_shell_command_with_cancellation(
	call: &McpToolCall,
	cancellation_token: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>
) -> Result<McpToolResult> {
	use std::sync::atomic::Ordering;

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

	// Execute the command with cancellation monitoring
	let cancel_token = cancellation_token.clone();
	let command_clone = command.clone();

	let output = tokio::task::spawn_blocking(move || {
		// Check for cancellation at the start of the blocking task
		if let Some(ref token) = cancel_token {
			if token.load(Ordering::SeqCst) {
				return json!({
					"success": false,
					"output": "Command execution cancelled",
					"code": -1,
					"parameters": {
						"command": command_clone
					},
					"message": "Command execution cancelled by user"
				});
			}
		}

		// Add command to shell history before execution
		let _ = add_to_shell_history(&command_clone);

		let output = if cfg!(target_os = "windows") {
			Command::new("cmd")
				.args(["/C", &command_clone])
				.output()
		} else {
			Command::new("sh")
				.args(["-c", &command_clone])
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
						"command": command_clone
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
					"command": command_clone
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
