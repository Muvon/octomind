// Shell execution functionality for the Developer MCP provider

use std::process::Command;
use serde_json::{json, Value};
use anyhow::{Result, anyhow};
use super::super::{McpToolCall, McpToolResult, McpFunction};

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