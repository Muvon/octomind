// MCP Protocol Implementation
// Based on Claude Sonnet protocol for tool use

use serde::{Serialize, Deserialize};
use serde_json::Value;
use anyhow::Result;
use regex::Regex;

pub mod shell;

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
	let mut output = String::new();

	for result in results {
		// Create a horizontal separator with tool name
		let title = format!(" {} | {} ", result.tool_name, guess_tool_category(&result.tool_name));
		let separator_length = 70.max(title.len() + 4);
		let dashes = "─".repeat(separator_length - title.len());

		output.push_str(&format!("\n──{}{}────\n", title, dashes));

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

				output.push_str(&format!("{}: {}\n", key, displayed_value));
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

		output.push_str("\nOutput:\n");
		output.push_str(&format!("```\n{}\n```\n", result_output));
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
		_ => "tool",
	}
}

// Gather available functions from enabled providers
pub fn get_available_functions(config: &crate::config::Config) -> Vec<McpFunction> {
	let mut functions = Vec::new();

	// Only gather functions if MCP is enabled
	if !config.mcp.enabled {
		return functions;
	}

	// Add shell function if enabled
	if config.mcp.providers.contains(&"shell".to_string()) {
		functions.push(shell::get_function_definition());
	}

	functions
}

// Execute a tool call
pub async fn execute_tool_call(call: &McpToolCall, config: &crate::config::Config) -> Result<McpToolResult> {
	// Only execute if MCP is enabled
	if !config.mcp.enabled {
		return Err(anyhow::anyhow!("MCP is not enabled"));
	}

	match call.tool_name.as_str() {
		"shell" => {
			if config.mcp.providers.contains(&"shell".to_string()) {
				shell::execute_shell_command(call).await
			} else {
				Err(anyhow::anyhow!("Shell provider is not enabled"))
			}
		},
		_ => Err(anyhow::anyhow!("Unknown tool: {}", call.tool_name)),
	}
}

// Execute multiple tool calls
pub async fn execute_tool_calls(calls: &[McpToolCall], config: &crate::config::Config) -> Vec<Result<McpToolResult>> {
	let mut results = Vec::new();

	for call in calls {
		results.push(execute_tool_call(call, config).await);
	}

	results
}
