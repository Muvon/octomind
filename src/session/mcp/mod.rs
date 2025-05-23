// MCP Protocol Implementation

use serde::{Serialize, Deserialize};
use serde_json::Value;
use anyhow::Result;
use colored;
use std::io::Write;
use uuid;

pub mod dev;
pub mod fs;
pub mod server;
pub mod process;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolCall {
	pub tool_name: String,
	pub parameters: Value,
	#[serde(default)]
	pub tool_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolResult {
	pub tool_name: String,
	pub result: Value,
	#[serde(default)]
	pub tool_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpFunction {
	pub name: String,
	pub description: String,
	pub parameters: Value,
}

// Format tool results to be shown to the user
pub fn format_tool_results(results: &[McpToolResult]) -> String {
	use colored::*;

	let mut output = String::new();

	for result in results {
		// Determine the category of the tool
		let category = guess_tool_category(&result.tool_name);

		// Create a horizontal separator with tool name and category
		let title = format!(" {} | {} ",
			result.tool_name.bright_cyan(),
			category.bright_blue())
		;

		let separator_length = 70.max(title.len() + 4);
		let dashes = "─".repeat(separator_length - title.len());

		// Format the separator with colors
		let separator = format!("──{}{}────", title, dashes.dimmed());

		output.push_str(&separator);
		output.push('\n');

		// Format the parameters if available and in debug mode
		if let Some(params) = result.result.get("parameters") {
			// Only show parameters in a very condensed format
			if let Some(params_obj) = params.as_object() {
				let mut param_parts = Vec::new();
				for (key, value) in params_obj {
					let value_str = if value.is_string() {
						value.as_str().unwrap_or("").to_string()
					} else {
						value.to_string()
					};

					// Show very short parameter summary
					let displayed_value = if value_str.len() > 30 {
						format!("{:.27}...", value_str)
					} else {
						value_str
					};
					param_parts.push(format!("{}: {}", key.bright_black(), displayed_value.bright_black()));
				}

				if !param_parts.is_empty() && param_parts.join(", ").len() < 60 {
					output.push_str(&format!("{}\n", param_parts.join(", ")));
				}
			}
		}

		// Format the main output content
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

		// Print the output content
		if is_error {
			output.push_str(&result_output.bright_red());
		} else {
			output.push_str(&result_output);
		}

		output.push('\n');
	}

	output
}

// Guess the category of a tool based on its name
fn guess_tool_category(tool_name: &str) -> &'static str {
	match tool_name {
		"core" => "system",
		"text_editor" => "developer",
		"list_files" => "filesystem",
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

// Parse a model's response to extract tool calls - kept for backward compatibility
pub fn parse_tool_calls(_content: &str) -> Vec<McpToolCall> {
	// This function is kept for backward compatibility but is no longer used directly
	// as we now prefer to pass tool calls directly as structs
	Vec::new()
}

// Structure to represent tool responses for OpenAI/Claude format
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResponseMessage {
	pub role: String,
	pub tool_call_id: String,
	pub name: String,
	pub content: String,
}

// Convert tool results to proper messages
pub fn tool_results_to_messages(results: &[McpToolResult]) -> Vec<ToolResponseMessage> {
	let mut messages = Vec::new();

	for result in results {
		messages.push(ToolResponseMessage {
			role: "tool".to_string(),
			tool_call_id: result.tool_id.clone(),
			name: result.tool_name.clone(),
			content: serde_json::to_string(&result.result).unwrap_or_default(),
		});
	}

	messages
}

// Ensure tool calls have valid IDs
pub fn ensure_tool_call_ids(calls: &mut Vec<McpToolCall>) {
	for call in calls.iter_mut() {
		if call.tool_id.is_empty() {
			call.tool_id = format!("tool_{}", uuid::Uuid::new_v4().simple());
		}
	}
}

// Gather available functions from enabled providers
pub async fn get_available_functions(config: &crate::config::Config) -> Vec<McpFunction> {
	let mut functions = Vec::new();

	// Only gather functions if MCP is enabled
	if !config.mcp.enabled {
		return functions;
	}

	// Add developer tools if enabled
	if config.mcp.providers.contains(&"core".to_string()) {
		functions.extend(dev::get_all_functions());
		functions.extend(fs::get_all_functions());
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

	functions
}

// Execute a tool call
pub async fn execute_tool_call(call: &McpToolCall, config: &crate::config::Config) -> Result<McpToolResult> {
	// Debug logging for tool execution
	if config.openrouter.debug {
		use colored::*;
		println!("{}", format!("Debug: Executing tool call: {}", call.tool_name).bright_blue());
		if let Ok(params) = serde_json::to_string_pretty(&call.parameters) {
			println!("{}", format!("Debug: Tool parameters: {}", params).bright_blue());
		}
	}

	// Only execute if MCP is enabled
	if !config.mcp.enabled {
		return Err(anyhow::anyhow!("MCP is not enabled"));
	}

	// Get store for tools that need it - only if semantic_code is needed
	let store_option = if call.tool_name == "semantic_code" {
		Some(crate::store::Store::new().await?)
	} else {
		None
	};

	let result = try_execute_tool_call(call, config, store_option.as_ref()).await;

	// Explicitly drop the store if it was created
	drop(store_option);

	result
}

// Internal function to actually execute the tool call
async fn try_execute_tool_call(call: &McpToolCall, config: &crate::config::Config, store_option: Option<&crate::store::Store>) -> Result<McpToolResult> {
	// Try to execute locally if provider is enabled
	if config.mcp.providers.contains(&"core".to_string()) {
		// Handle developer tools
		if call.tool_name == "core" {
			let mut result = dev::execute_shell_command(call).await?;
			// Add the tool_id to the result
			result.tool_id = call.tool_id.clone();

			// Check if result is large - warn user if it exceeds threshold
			let estimated_tokens = crate::session::estimate_tokens(&format!("{}", result.result));
			if estimated_tokens > config.openrouter.mcp_response_warning_threshold {
				// Create a modified result that warns about the size
				use colored::Colorize;
				println!("{}", format!("! WARNING: Shell command produced a large output ({} tokens)",
					estimated_tokens).bright_yellow());
				println!("{}", "This may consume significant tokens and impact your usage limits.".bright_yellow());

				// Ask user for confirmation before proceeding
				print!("{}", "Do you want to continue with this large output? [y/N]: ".bright_cyan());
				std::io::stdout().flush().unwrap();

				let mut input = String::new();
				std::io::stdin().read_line(&mut input).unwrap_or_default();

				if !input.trim().to_lowercase().starts_with('y') {
					// User declined, return a truncated result with explanation
					let truncated_result = McpToolResult {
						tool_name: result.tool_name,
						tool_id: call.tool_id.clone(),
						result: serde_json::json!({
							"output": format!("[Output truncated to save tokens: {} tokens of output were not processed as requested]", estimated_tokens)
						}),
					};
					return Ok(truncated_result);
				}

				// User confirmed, continue with original result
				println!("{}", "Proceeding with full output...".bright_green());
			}

			return Ok(result);
		} else if call.tool_name == "text_editor" {
			return fs::execute_text_editor(call).await;
		} else if call.tool_name == "html2md" {
			return fs::execute_html2md(call).await;
		} else if call.tool_name == "list_files" {
			return fs::execute_list_files(call).await;
		} else if call.tool_name == "semantic_code" {
			if let Some(store) = store_option {
				return dev::execute_semantic_code(call, store, config).await;
			} else {
				return Err(anyhow::anyhow!("Store not initialized for semantic_code tool"));
			}
		} else if call.tool_name == "graphrag" {
			return dev::execute_graphrag(call, config).await;
		}
	} else {
		return Err(anyhow::anyhow!("Developer tools are not enabled"));
	}

	// Try to find a server that can handle this tool
	let mut last_error = anyhow::anyhow!("No servers available to process this tool");
	for server in &config.mcp.servers {
		if server.enabled {
			// Check if this server supports the tool
			if server.tools.is_empty() || server.tools.contains(&call.tool_name) {
				// Try to execute the tool on this server
				match super::mcp::server::execute_tool_call(call, server).await {
					Ok(result) => {
						// Check if result is large - warn user if it exceeds threshold
						let estimated_tokens = crate::session::estimate_tokens(&format!("{}", result.result));
						if estimated_tokens > config.openrouter.mcp_response_warning_threshold {
							// Create a modified result that warns about the size
							use colored::Colorize;
							println!("{}", format!("! WARNING: External tool produced a large output ({} tokens)",
								estimated_tokens).bright_yellow());
							println!("{}", "This may consume significant tokens and impact your usage limits.".bright_yellow());

							// Ask user for confirmation before proceeding
							print!("{}", "Do you want to continue with this large output? [y/N]: ".bright_cyan());
							std::io::stdout().flush().unwrap();

							let mut input = String::new();
							std::io::stdin().read_line(&mut input).unwrap_or_default();

							if !input.trim().to_lowercase().starts_with('y') {
								// User declined, return a truncated result with explanation
								let truncated_result = McpToolResult {
									tool_name: result.tool_name,
									tool_id: call.tool_id.clone(),
									result: serde_json::json!({
										"output": format!("[Output truncated to save tokens: {} tokens of output were not processed as requested]", estimated_tokens)
									}),
								};
								return Ok(truncated_result);
							}

							// User confirmed, continue with original result
							println!("{}", "Proceeding with full output...".bright_green());
						}

						return Ok(result);
					},
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

// Execute a tool call with layer-specific restrictions
pub async fn execute_layer_tool_call(call: &McpToolCall, config: &crate::config::Config, layer_config: &crate::session::layers::LayerConfig) -> Result<McpToolResult> {
	// Check if tools are enabled for this layer
	if !layer_config.enable_tools {
		return Err(anyhow::anyhow!("Tool execution is disabled for this layer"));
	}

	// Check if specific tool is allowed for this layer
	if !layer_config.allowed_tools.is_empty() && !layer_config.allowed_tools.contains(&call.tool_name) {
		return Err(anyhow::anyhow!("Tool '{}' is not allowed for this layer", call.tool_name));
	}

	// Pass to regular tool execution
	execute_tool_call(call, config).await
}

// Execute multiple tool calls
pub async fn execute_tool_calls(calls: &[McpToolCall], config: &crate::config::Config) -> Vec<Result<McpToolResult>> {
	let mut results = Vec::new();

	for call in calls {
		// Execute the tool call
		let result = execute_tool_call(call, config).await;
		results.push(result);
	}

	results
}