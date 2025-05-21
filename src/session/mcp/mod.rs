// MCP Protocol Implementation
// Based on Claude Sonnet protocol for tool use

use serde::{Serialize, Deserialize};
use serde_json::Value;
use anyhow::Result;
use regex::Regex;
use colored;
use std::io::Write;

pub mod dev;
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

	// Look for <function_calls> or <function_calls> blocks (various formats)
	let patterns = [
		// Standard MCP format
		r#"<(antml:)?function_calls>\s*(.+?)\s*</(antml:)?function_calls>"#,
		// Alternative format sometimes used by Claude
		r#"```(json)?\s*\[?\s*\{\s*"tool_name":.+?\}\s*\]?\s*```"#,
		// Another variation
		r#"\{\s*"tool_name":.+?\}\s*"#
	];

	for pattern in patterns {
		if let Some(re) = Regex::new(pattern).ok() {
			for cap in re.captures_iter(content) {
				let json_str = if cap.len() > 1 && cap.get(2).is_some() {
					cap.get(2).unwrap().as_str().to_string()
				} else {
					// For patterns without capture groups, use the whole match
					let matched = cap.get(0).unwrap().as_str();
					// Clean up: remove code blocks and whitespace
					let cleaned = matched.replace("```json", "");
					let cleaned = cleaned.replace("```", "");
					cleaned.trim().to_string()
				};

				// Try to parse as an array
				if let Ok(calls) = serde_json::from_str::<Vec<McpToolCall>>(&json_str) {
					tool_calls.extend(calls);
					continue;
				}

				// Try to parse as a single object
				if let Ok(call) = serde_json::from_str::<McpToolCall>(&json_str) {
					tool_calls.push(call);
					continue;
				}

				// Try to parse with array brackets added
				if let Ok(calls) = serde_json::from_str::<Vec<McpToolCall>>(&format!("[{}]", json_str)) {
					tool_calls.extend(calls);
					continue;
				}

				// Debug: failed to parse tool call JSON
				if cfg!(debug_assertions) {
					println!("Failed to parse tool call JSON: {}", json_str);
				}
			}
		}
	}

	// Additional fallback for Claude-specific format - look for patterns like "I'll use the X tool"
	// followed by function-like calls
	if tool_calls.is_empty() {
		if let Some(re) = Regex::new(r#"(?i)I'?ll use the (\w+) tool.+?\{\s*"tool_name".+?\}"#).ok() {
			for cap in re.captures_iter(content) {
				if let Some(full_match) = cap.get(0) {
					let tool_text = full_match.as_str();
					// Extract the JSON object
					if let Some(start) = tool_text.find('{') {
						let json_part = &tool_text[start..];
						// Find matching closing brace (simple method, doesn't handle nested objects well)
						let mut brace_count = 0;
						let mut end_pos = 0;

						for (i, c) in json_part.char_indices() {
							if c == '{' {
								brace_count += 1;
							} else if c == '}' {
								brace_count -= 1;
								if brace_count == 0 {
									end_pos = i + 1;
									break;
								}
							}
						}

						if end_pos > 0 {
							let json_obj = &json_part[..end_pos];
							if let Ok(call) = serde_json::from_str::<McpToolCall>(json_obj) {
								tool_calls.push(call);
							}
						}
					}
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

		// Print the output content regardless of error state
		if is_error {
			output.push_str(&result_output.bright_red());
		} else {
			output.push_str(&result_output);
		}
	}

	output
}

// Guess the category of a tool based on its name
fn guess_tool_category(tool_name: &str) -> &'static str {
	match tool_name {
		"shell" => "system",
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

// Gather available functions from enabled providers
pub async fn get_available_functions(config: &crate::config::Config) -> Vec<McpFunction> {
	let mut functions = Vec::new();

	// Only gather functions if MCP is enabled
	if !config.mcp.enabled {
		return functions;
	}

	// Add developer tools if enabled
	if config.mcp.providers.contains(&"shell".to_string()) {
		functions.extend(dev::get_all_functions());
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
	if config.mcp.providers.contains(&"shell".to_string()) {
		// Handle developer tools
		if call.tool_name == "shell" {
			let result = dev::execute_shell_command(call).await?;

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
						result: serde_json::json!({
							"output": format!("[Output truncated to save tokens: {} tokens of output were not processed as requested]", estimated_tokens)
						}),
					};
					return Ok(truncated_result);
				}

				// User confirmed, continue with original result
				println!("{}", "Proceeding with full output...".bright_green());
			}

			// Check for auto-cache reached flag (special case for sessions using this tool)
			// This is a simplified approach since we can't easily access the session
			// Look for an env variable signal that might be set by the response processor
			if let Ok(value) = std::env::var("OCTODEV_AUTO_CACHE_TRIGGERED") {
				if value == "1" {
					// Return a result that signals auto-cache has been reached
					let mut modified_result = result.clone();
					// Add a special field to signal that auto-cache was triggered
					if let serde_json::Value::Object(ref mut map) = modified_result.result {
						map.insert("auto_cache_triggered".to_string(), serde_json::Value::Bool(true));
					}
					// Reset the env variable
					std::env::set_var("OCTODEV_AUTO_CACHE_TRIGGERED", "0");
					return Ok(modified_result);
				}
			}

			return Ok(result);
		} else if call.tool_name == "text_editor" {
			let result = dev::execute_text_editor(call).await?;

			// Check if result is large - warn user if it exceeds threshold
			let estimated_tokens = crate::session::estimate_tokens(&format!("{}", result.result));
			if estimated_tokens > config.openrouter.mcp_response_warning_threshold {
				// Create a modified result that warns about the size
				use colored::Colorize;
				println!("{}", format!("! WARNING: Text editor produced a large output ({} tokens)",
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
		} else if call.tool_name == "list_files" {
			return dev::execute_list_files(call).await;
		} else if call.tool_name == "semantic_code" {
			if let Some(store) = store_option {
				return dev::execute_semantic_code(call, store, config).await;
			} else {
				return Err(anyhow::anyhow!("Store not initialized for semantic_code tool"));
			}
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

	// Track if we've hit the auto-cache threshold during tool execution
	let mut auto_cache_triggered = false;

	for call in calls {
		// If auto-cache was triggered, stop executing tools to prevent state inconsistency
		if auto_cache_triggered {
			// Add a message indicating we're stopping tool execution due to cache threshold
			results.push(Ok(McpToolResult {
				tool_name: call.tool_name.clone(),
				result: serde_json::json!({
					"output": "[Tool execution skipped - auto-cache threshold reached. Please continue the conversation to complete the task.]"
				}),
			}));
			continue;
		}

		// Execute the tool call
		let result = execute_tool_call(call, config).await;

		// Check if we need to stop further tool execution
		if let Ok(ref tool_result) = result {
			// Check if this is a special signal indicating auto-cache was triggered
			if let Some(message) = tool_result.result.get("auto_cache_triggered") {
				if message.as_bool().unwrap_or(false) {
					auto_cache_triggered = true;
				}
			}
		}

		results.push(result);
	}

	results
}
