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

// MCP Protocol Implementation

use crate::log_debug;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::Write;
use uuid;

pub mod dev;
pub mod fs;
pub mod process;
pub mod server;

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
		let title = format!(
			" {} | {} ",
			result.tool_name.bright_cyan(),
			category.bright_blue()
		);

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
					param_parts.push(format!(
						"{}: {}",
						key.bright_black(),
						displayed_value.bright_black()
					));
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
pub fn guess_tool_category(tool_name: &str) -> &'static str {
	match tool_name {
		"core" => "system",
		"text_editor" => "developer",
		"list_files" => "filesystem",
		"html2md" => "web",
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
pub fn ensure_tool_call_ids(calls: &mut [McpToolCall]) {
	for call in calls.iter_mut() {
		if call.tool_id.is_empty() {
			call.tool_id = format!("tool_{}", uuid::Uuid::new_v4().simple());
		}
	}
}

// Gather available functions from enabled servers
pub async fn get_available_functions(config: &crate::config::Config) -> Vec<McpFunction> {
	let mut functions = Vec::new();

	// Only gather functions if MCP has any servers configured
	if config.mcp.servers.is_empty() {
		crate::log_debug!("MCP has no servers configured, no functions available");
		return functions;
	}

	// Get enabled servers from the merged config (which should already be filtered by server_refs)
	let enabled_servers: Vec<crate::config::McpServerConfig> =
		config.mcp.servers.values().cloned().collect();
	crate::log_debug!(
		"Found {} enabled servers in merged config",
		enabled_servers.len()
	);

	// DEBUG: Print all server details
	for server in &enabled_servers {
		crate::log_debug!(
			"MCP Server: {} - Type: {:?}, Tools: {:?}",
			server.name,
			server.server_type,
			server.tools
		);
	}

	for server in enabled_servers {
		crate::log_debug!(
			"Processing MCP server: {} (type: {:?})",
			server.name,
			server.server_type
		);

		match server.server_type {
			crate::config::McpServerType::Developer => {
				let server_functions = if server.tools.is_empty() {
					// No tool filtering - get all developer functions
					dev::get_all_functions()
				} else {
					// Filter functions based on allowed tools
					dev::get_all_functions()
						.into_iter()
						.filter(|func| server.tools.contains(&func.name))
						.collect()
				};
				crate::log_debug!(
					"Developer server '{}' provided {} functions",
					server.name,
					server_functions.len()
				);
				for func in &server_functions {
					crate::log_debug!("  - Developer tool: {}", func.name);
				}
				functions.extend(server_functions);
			}
			crate::config::McpServerType::Filesystem => {
				let server_functions = if server.tools.is_empty() {
					// No tool filtering - get all filesystem functions
					fs::get_all_functions()
				} else {
					// Filter functions based on allowed tools
					fs::get_all_functions()
						.into_iter()
						.filter(|func| server.tools.contains(&func.name))
						.collect()
				};
				crate::log_debug!(
					"Filesystem server '{}' provided {} functions",
					server.name,
					server_functions.len()
				);
				for func in &server_functions {
					crate::log_debug!("  - Filesystem tool: {}", func.name);
				}
				functions.extend(server_functions);
			}
			crate::config::McpServerType::External => {
				// Handle external servers
				crate::log_debug!(
					"Attempting to get functions from external server: {}",
					server.name
				);
				match server::get_server_functions(&server).await {
					Ok(server_functions) => {
						let filtered_functions = if server.tools.is_empty() {
							// No tool filtering - get all functions from server
							server_functions
						} else {
							// Filter functions based on allowed tools
							server_functions
								.into_iter()
								.filter(|func| server.tools.contains(&func.name))
								.collect()
						};
						crate::log_debug!(
							"External server '{}' provided {} functions",
							server.name,
							filtered_functions.len()
						);
						for func in &filtered_functions {
							crate::log_debug!("  - External tool: {}", func.name);
						}
						functions.extend(filtered_functions);
					}
					Err(e) => {
						crate::log_debug!(
							"Failed to get functions from external server '{}': {}",
							server.name,
							e
						);
						// Continue with other servers instead of failing completely
					}
				}
			}
		}
	}

	crate::log_debug!("Total functions available: {}", functions.len());
	for func in &functions {
		crate::log_debug!("Available function: {}", func.name);
	}
	functions
}

// Execute a tool call
pub async fn execute_tool_call(
	call: &McpToolCall,
	config: &crate::config::Config,
) -> Result<(McpToolResult, u64)> {
	execute_tool_call_with_cancellation(call, config, None).await
}

// Execute a tool call with cancellation support
pub async fn execute_tool_call_with_cancellation(
	call: &McpToolCall,
	config: &crate::config::Config,
	cancellation_token: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> Result<(McpToolResult, u64)> {
	use std::sync::atomic::Ordering;

	// Debug logging for tool execution
	log_debug!("Debug: Executing tool call: {}", call.tool_name);
	log_debug!(
		"Debug: MCP has {} servers configured",
		config.mcp.servers.len()
	);
	if let Ok(params) = serde_json::to_string_pretty(&call.parameters) {
		log_debug!("Debug: Tool parameters: {}", params);
	}

	// Only execute if MCP has any servers configured
	if config.mcp.servers.is_empty() {
		return Err(anyhow::anyhow!("MCP has no servers configured"));
	}

	// Check for cancellation before starting
	if let Some(ref token) = cancellation_token {
		if token.load(Ordering::SeqCst) {
			return Err(anyhow::anyhow!("Tool execution cancelled"));
		}
	}

	// Track tool execution time
	let tool_start = std::time::Instant::now();

	let result =
		try_execute_tool_call_with_cancellation(call, config, cancellation_token.clone()).await;

	// Calculate tool execution time
	let tool_duration = tool_start.elapsed();
	let tool_time_ms = tool_duration.as_millis() as u64;

	match result {
		Ok(tool_result) => Ok((tool_result, tool_time_ms)),
		Err(e) => Err(e),
	}
}

// Internal function to actually execute the tool call with cancellation support
async fn try_execute_tool_call_with_cancellation(
	call: &McpToolCall,
	config: &crate::config::Config,
	cancellation_token: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> Result<McpToolResult> {
	use std::sync::atomic::Ordering;

	// Only execute if MCP has any servers configured
	if config.mcp.servers.is_empty() {
		return Err(anyhow::anyhow!("MCP has no servers configured"));
	}

	// Check for cancellation before proceeding
	if let Some(ref token) = cancellation_token {
		if token.load(Ordering::SeqCst) {
			return Err(anyhow::anyhow!("Tool execution cancelled"));
		}
	}

	// CRITICAL FIX: Build a tool-to-server mapping to route tools to the correct server
	// This prevents sending tools to servers that don't support them
	let mut tool_to_server_map = std::collections::HashMap::new();
	let available_servers: Vec<crate::config::McpServerConfig> =
		config.mcp.servers.values().cloned().collect();

	// Map internal tools to their appropriate server types
	for server in &available_servers {
		match server.server_type {
			crate::config::McpServerType::Developer => {
				// Map developer tools to this server
				let dev_tools = ["shell"];
				for tool in &dev_tools {
					if server.tools.is_empty() || server.tools.contains(&tool.to_string()) {
						tool_to_server_map.insert(tool.to_string(), server.clone());
					}
				}
			}
			crate::config::McpServerType::Filesystem => {
				// Map filesystem tools to this server
				let fs_tools = ["text_editor", "html2md", "list_files"];
				for tool in &fs_tools {
					if server.tools.is_empty() || server.tools.contains(&tool.to_string()) {
						tool_to_server_map.insert(tool.to_string(), server.clone());
					}
				}
			}
			crate::config::McpServerType::External => {
				// For external servers, we need to query what tools they actually support
				// This should be done during function discovery, but for now we'll handle it dynamically
				// Skip building a static map for external servers - they'll be tried if no internal match
			}
		}
	}

	// STEP 1: Try to find the exact server that handles this tool
	if let Some(target_server) = tool_to_server_map.get(&call.tool_name) {
		crate::log_debug!(
			"Found direct server mapping for tool '{}' -> server '{}' ({:?})",
			call.tool_name,
			target_server.name,
			target_server.server_type
		);

		// Check for cancellation before execution
		if let Some(ref token) = cancellation_token {
			if token.load(Ordering::SeqCst) {
				return Err(anyhow::anyhow!("Tool execution cancelled"));
			}
		}

		// Execute on the target server
		match target_server.server_type {
			crate::config::McpServerType::Developer => match call.tool_name.as_str() {
				"shell" => {
					crate::log_debug!("Executing shell command via developer server");
					let mut result = dev::execute_shell_command_with_cancellation(
						call,
						cancellation_token.clone(),
					)
					.await?;
					result.tool_id = call.tool_id.clone();
					return handle_large_response(result, config);
				}
				_ => {
					return Err(anyhow::anyhow!(
						"Tool '{}' not implemented in developer server",
						call.tool_name
					));
				}
			},
			crate::config::McpServerType::Filesystem => match call.tool_name.as_str() {
				"text_editor" => {
					crate::log_debug!("Executing text_editor via filesystem server");
					let mut result =
						fs::execute_text_editor_with_cancellation(call, cancellation_token.clone())
							.await?;
					result.tool_id = call.tool_id.clone();
					return Ok(result);
				}
				"html2md" => {
					crate::log_debug!("Executing html2md via filesystem server");
					let mut result =
						fs::execute_html2md_with_cancellation(call, cancellation_token.clone())
							.await?;
					result.tool_id = call.tool_id.clone();
					return Ok(result);
				}
				"list_files" => {
					crate::log_debug!("Executing list_files via filesystem server");
					let mut result =
						fs::execute_list_files_with_cancellation(call, cancellation_token.clone())
							.await?;
					result.tool_id = call.tool_id.clone();
					return Ok(result);
				}
				_ => {
					return Err(anyhow::anyhow!(
						"Tool '{}' not implemented in filesystem server",
						call.tool_name
					));
				}
			},
			crate::config::McpServerType::External => {
				// This shouldn't happen with direct mapping, but handle it
				match server::execute_tool_call_with_cancellation(
					call,
					target_server,
					cancellation_token.clone(),
				)
				.await
				{
					Ok(mut result) => {
						result.tool_id = call.tool_id.clone();
						return handle_large_response(result, config);
					}
					Err(err) => {
						return Err(err);
					}
				}
			}
		}
	}

	// STEP 2: If no direct mapping found, try external servers that might support this tool
	let mut _last_error =
		anyhow::anyhow!("No servers available to process tool '{}'", call.tool_name);
	let mut servers_checked = Vec::new();

	for server in available_servers {
		// Skip servers we already checked via direct mapping
		if tool_to_server_map.values().any(|s| s.name == server.name) {
			continue;
		}

		// Only try external servers in this fallback phase
		if let crate::config::McpServerType::External = server.server_type {
			servers_checked.push(format!("{}({:?})", server.name, server.server_type));

			// Check for cancellation between server attempts
			if let Some(ref token) = cancellation_token {
				if token.load(Ordering::SeqCst) {
					return Err(anyhow::anyhow!("Tool execution cancelled"));
				}
			}

			// Check if this server can handle the tool (if tool filtering is enabled)
			if !server.tools.is_empty() && !server.tools.contains(&call.tool_name) {
				crate::log_debug!(
					"External server '{}' skipped - tool '{}' not in allowed tools: {:?}",
					server.name,
					call.tool_name,
					server.tools
				);
				continue; // Skip this server if it doesn't handle this tool
			}

			// Try to execute the tool on this external server
			crate::log_debug!(
				"Trying external server '{}' for tool '{}'",
				server.name,
				call.tool_name
			);
			match server::execute_tool_call_with_cancellation(
				call,
				&server,
				cancellation_token.clone(),
			)
			.await
			{
				Ok(mut result) => {
					crate::log_debug!(
						"Successfully executed tool '{}' on external server '{}'",
						call.tool_name,
						server.name
					);
					result.tool_id = call.tool_id.clone();
					return handle_large_response(result, config);
				}
				Err(err) => {
					crate::log_debug!(
						"External server '{}' failed to execute tool '{}': {}",
						server.name,
						call.tool_name,
						err
					);
					_last_error = err;
					// Continue trying other external servers
				}
			}
		}
	}

	// If we get here, no server could handle the tool call
	Err(anyhow::anyhow!(
		"Unknown tool '{}'. Available tools: {}. Checked servers: {}",
		call.tool_name,
		get_available_tool_names(config).await.join(", "),
		if servers_checked.is_empty() {
			"none (no external servers to try)".to_string()
		} else {
			servers_checked.join(", ")
		}
	))
}

// Helper function to get available tool names for error messages
async fn get_available_tool_names(config: &crate::config::Config) -> Vec<String> {
	let functions = get_available_functions(config).await;
	functions.into_iter().map(|f| f.name).collect()
}

// Helper function to handle large response warnings
fn handle_large_response(
	result: McpToolResult,
	config: &crate::config::Config,
) -> Result<McpToolResult> {
	// Check if result is large - warn user if it exceeds threshold
	let estimated_tokens = crate::session::estimate_tokens(&format!("{}", result.result));
	if estimated_tokens > config.mcp_response_warning_threshold {
		// Create a modified result that warns about the size
		use colored::Colorize;
		println!(
			"{}",
			format!(
				"! WARNING: Tool produced a large output ({} tokens)",
				estimated_tokens
			)
			.bright_yellow()
		);
		println!(
			"{}",
			"This may consume significant tokens and impact your usage limits.".bright_yellow()
		);

		// Ask user for confirmation before proceeding
		print!(
			"{}",
			"Do you want to continue with this large output? [y/N]: ".bright_cyan()
		);
		std::io::stdout().flush().unwrap();

		let mut input = String::new();
		std::io::stdin().read_line(&mut input).unwrap_or_default();

		if !input.trim().to_lowercase().starts_with('y') {
			// CRITICAL FIX: User declined large output. Instead of creating a fake response
			// that might violate MCP schemas, we return an error that will cause the tool_use
			// block to be removed from the conversation entirely. This is MCP-compliant.
			return Err(anyhow::anyhow!("LARGE_OUTPUT_DECLINED_BY_USER: User declined to process large output with {} tokens", estimated_tokens));
		}

		// User confirmed, continue with original result
		println!("{}", "Proceeding with full output...".bright_green());
	}

	Ok(result)
}

// Execute a tool call with layer-specific restrictions
pub async fn execute_layer_tool_call(
	call: &McpToolCall,
	config: &crate::config::Config,
	layer_config: &crate::session::layers::LayerConfig,
) -> Result<(McpToolResult, u64)> {
	// Check if tools are enabled for this layer (has server_refs)
	if layer_config.mcp.server_refs.is_empty() {
		return Err(anyhow::anyhow!("Tool execution is disabled for this layer"));
	}

	// Check if specific tool is allowed for this layer
	if !layer_config.mcp.allowed_tools.is_empty()
		&& !layer_config.mcp.allowed_tools.contains(&call.tool_name)
	{
		return Err(anyhow::anyhow!(
			"Tool '{}' is not allowed for this layer",
			call.tool_name
		));
	}

	// Pass to regular tool execution
	execute_tool_call(call, config).await
}

// Execute multiple tool calls
pub async fn execute_tool_calls(
	calls: &[McpToolCall],
	config: &crate::config::Config,
) -> Vec<Result<(McpToolResult, u64)>> {
	let mut results = Vec::new();

	for call in calls {
		// Execute the tool call
		let result = execute_tool_call(call, config).await;
		results.push(result);
	}

	results
}
