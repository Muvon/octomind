// MCP Protocol Implementation
// Based on Claude Sonnet protocol for tool use

use serde::{Serialize, Deserialize};
use serde_json::Value;
use anyhow::Result;
use regex::Regex;
use colored;

pub mod shell;
pub mod server;
pub mod process;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolCall {
	pub tool_name: String,
	pub parameters: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolResult {
	pub tool_name: String,
	pub result: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpFunction {
	pub name: String,
	pub description: String,
	pub parameters: Value,
}

// Parse a model's response to extract tool calls
pub fn parse_tool_calls(content: &str) -> Vec<McpToolCall> {
	let mut tool_calls = Vec::new();

	// Look for <function_calls> or <function_calls> blocks (Claude format)
	if let Some(re) = Regex::new(r"<(antml:)?function_calls>\s*(.+?)\s*</(antml:)?function_calls>").ok() {
		for cap in re.captures_iter(content) {
			if let Some(json_str) = cap.get(2) {
				// Try to parse as an array or as a single object
				if let Ok(calls) = serde_json::from_str::<Vec<McpToolCall>>(json_str.as_str()) {
					tool_calls.extend(calls);
				} else if let Ok(call) = serde_json::from_str::<McpToolCall>(json_str.as_str()) {
					tool_calls.push(call);
				}
			}
		}
	}

	tool_calls
}

// Format tool results to be shown to the user
pub fn format_tool_results(results: &[McpToolResult]) -> String {
	use colored::*;

	let mut output = String::new();

	for result in results {
		// Determine the category of the tool
		let category = guess_tool_category(&result.tool_name);

		// Create a horizontal separator with tool name
		let title = format!(" {} | {} ",
				result.tool_name.bright_cyan(),
				category.bright_blue())
		;

		let separator_length = 70.max(title.len() + 4);
		let dashes = "─".repeat(separator_length - title.len());

		// Format the separator with colors if terminal supports them
		let separator = format!("\n──{}{}────\n", title, dashes.dimmed());

		output.push_str(&separator);

		// Format the parameters if available
		if let Some(params) = result.result.get("parameters") {
			for (key, value) in params.as_object().unwrap_or(&serde_json::Map::new()) {
				let value_str = if value.is_string() {
					value.as_str().unwrap_or("").to_string()
				} else {
					value.to_string()
				};

				// Truncate long values
				let displayed_value = if value_str.len() > 50 {
					format!("{:.47}...", value_str)
				} else {
					value_str
				};

				output.push_str(&format!("{}: {}\n",
						key.yellow(),
						displayed_value))
			}
		}

		// Format the output
		let result_output = if let Some(output_value) = result.result.get("output") {
			if output_value.is_string() {
				output_value.as_str().unwrap_or("").to_string()
			} else {
				output_value.to_string().replace("\\n", "\n")
			}
		} else {
			result.result.to_string().replace("\\n", "\n")
		};

		// Check if there's an error
		let is_error = if let Some(success) = result.result.get("success") {
			!success.as_bool().unwrap_or(true)
		} else {
			false
		};

		// Just print the output directly without any wrapping
		if is_error {
			output.push_str(&result_output.bright_red());
		}
	}

	output
}

// Guess the category of a tool based on its name
fn guess_tool_category(tool_name: &str) -> &'static str {
	match tool_name {
		"shell" => "system",
		name if name.contains("file") || name.contains("editor") => "developer",
		name if name.contains("search") || name.contains("find") => "search",
		name if name.contains("image") || name.contains("photo") => "media",
		name if name.contains("web") || name.contains("http") => "web",
		name if name.contains("db") || name.contains("database") => "database",
		name if name.contains("browser") => "browser",
		name if name.contains("terminal") => "terminal",
		name if name.contains("video") => "video",
		name if name.contains("audio") => "audio",
		name if name.contains("location") || name.contains("map") => "location",
		name if name.contains("google") => "google",
		name if name.contains("weather") => "weather",
		name if name.contains("calculator") || name.contains("math") => "math",
		name if name.contains("news") => "news",
		name if name.contains("email") => "email",
		name if name.contains("calendar") => "calendar",
		name if name.contains("translate") => "translation",
		name if name.contains("github") => "github",
		name if name.contains("git") => "git",
		_ => "external",
	}
}

// Gather available functions from enabled providers
pub async fn get_available_functions(config: &crate::config::Config) -> Vec<McpFunction> {
	let mut functions = Vec::new();

	// Only gather functions if MCP is enabled
	if !config.mcp.enabled {
		return functions;
	}

	// Add shell function if enabled
	if config.mcp.providers.contains(&"shell".to_string()) {
		functions.push(shell::get_function_definition());
	}

	// Add server functions if any servers are enabled
	if !config.mcp.servers.is_empty() {
		for server in &config.mcp.servers {
			if server.enabled {
				if let Ok(server_functions) = super::mcp::server::get_server_functions(server).await {
					functions.extend(server_functions);
				}
			}
		}
	}

	// Debug output
	// println!("Functions: {:?}", functions);

	functions
}

// Execute a tool call
pub async fn execute_tool_call(call: &McpToolCall, config: &crate::config::Config) -> Result<McpToolResult> {
	// Only execute if MCP is enabled
	if !config.mcp.enabled {
		return Err(anyhow::anyhow!("MCP is not enabled"));
	}

	// Try to execute locally if provider is enabled
	if call.tool_name == "shell" {
		if config.mcp.providers.contains(&"shell".to_string()) {
			return shell::execute_shell_command(call).await;
		} else {
			return Err(anyhow::anyhow!("Shell provider is not enabled"));
		}
	}

	// Try to find a server that can handle this tool
	let mut last_error = anyhow::anyhow!("No servers available to process this tool");
	for server in &config.mcp.servers {
		if server.enabled {
			// Check if this server supports the tool
			if server.tools.is_empty() || server.tools.contains(&call.tool_name) {
				// Try to execute the tool on this server
				match super::mcp::server::execute_tool_call(call, server).await {
					Ok(result) => return Ok(result),
					Err(err) => {
						last_error = err;
						// Continue trying other servers
					}
				}
			}
		}
	}

	// If we get here, no server could handle the tool call
	Err(anyhow::anyhow!("Failed to execute tool '{}': {}", call.tool_name, last_error))
}

// Execute multiple tool calls
pub async fn execute_tool_calls(calls: &[McpToolCall], config: &crate::config::Config) -> Vec<Result<McpToolResult>> {
	let mut results = Vec::new();

	for call in calls {
		results.push(execute_tool_call(call, config).await);
	}

	results
}
