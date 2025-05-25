// Directory operations module - handling file listing with ripgrep

use std::process::Command;
use serde_json::{json, Value};
use anyhow::{Result, anyhow};
use super::super::{McpToolCall, McpToolResult};

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