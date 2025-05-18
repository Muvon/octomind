// Shell MCP provider

use std::process::Command;
use serde_json::{json, Value};
use anyhow::Result;
use super::{McpToolCall, McpToolResult, McpFunction};

// Define the shell function for the MCP protocol
pub fn get_function_definition() -> McpFunction {
	McpFunction {
		name: "shell".to_string(),
		description: "Execute a shell command and return its output".to_string(),
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
		_ => return Err(anyhow::anyhow!("Missing or invalid 'command' parameter")),
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
				let combined = if stderr.is_empty() {
					stdout
				} else if stdout.is_empty() {
					stderr
				} else {
					format!("{}

Error: {}", stdout, stderr)
				};

				json!({
					"success": output.status.success(),
					"output": combined,
					"code": output.status.code(),
					"parameters": {
						"command": command
					}
				})
			},
			Err(e) => json!({
				"success": false,
				"output": format!("Failed to execute command: {}", e),
				"code": null,
				"parameters": {
					"command": command
				}
			}),
		}
	}).await?;

	Ok(McpToolResult {
		tool_name: "shell".to_string(),
		result: output,
	})
}
