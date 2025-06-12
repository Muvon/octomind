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

// Response processing module - main orchestrator

pub mod tool_execution;
mod tool_result_processor;

use super::{CostTracker, MessageHandler, ToolProcessor};
use crate::config::Config;
use crate::log_debug;
use crate::session::chat::assistant_output::print_assistant_response;
use crate::session::chat::formatting::remove_function_calls;
use crate::session::chat::session::ChatSession;
use crate::session::ProviderExchange;
use anyhow::Result;
use colored::Colorize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

// Helper function to log debug information about the response
fn log_response_debug(
	_config: &Config,
	finish_reason: &Option<String>,
	tool_calls: &Option<Vec<crate::mcp::McpToolCall>>,
) {
	if let Some(ref reason) = finish_reason {
		log_debug!("Processing response with finish_reason: {}", reason);
	}
	if let Some(ref calls) = tool_calls {
		log_debug!("Processing {} tool calls", calls.len());
	}
}

// Helper function to handle final response when no tool calls are present
fn handle_final_response(
	content: &str,
	current_content: &str,
	current_exchange: ProviderExchange,
	chat_session: &mut ChatSession,
	config: &Config,
	role: &str,
) -> Result<()> {
	// Remove any function_calls blocks if they exist but weren't processed earlier
	let clean_content = remove_function_calls(current_content);

	// When adding the final assistant message for a response that involved tool calls,
	// we've already tracked the cost and tokens in the loop above, so we pass None for exchange
	// to avoid double-counting. If this is a direct response with no tool calls, we pass the
	// original exchange to ensure costs are tracked.
	let exchange_for_final = if content == current_content {
		// This is the original content, so use the original exchange for cost tracking
		Some(current_exchange)
	} else {
		// This is a modified content after tool calls, so costs were already tracked
		// in the tool response handling code, so pass None to avoid double counting
		None
	};

	chat_session.add_assistant_message(&clean_content, exchange_for_final, config, role)?;

	// Print assistant response with color
	print_assistant_response(&clean_content, config, role);

	// Display cumulative token usage using CostTracker
	CostTracker::display_session_usage(chat_session);

	Ok(())
}

// Get the actual server name for a tool (async version that matches execution)
async fn get_tool_server_name_async(tool_name: &str, config: &Config) -> String {
	// Use the SAME logic as execution - build the actual tool-to-server map
	let tool_server_map = build_tool_server_map(config).await;

	if let Some(target_server) = tool_server_map.get(tool_name) {
		target_server.name.clone()
	} else {
		// Fallback to category guess if no server found
		crate::mcp::guess_tool_category(tool_name).to_string()
	}
}

// Build a simple tool-to-server lookup map for instant routing (same as in mod.rs)
async fn build_tool_server_map(
	config: &Config,
) -> std::collections::HashMap<String, crate::config::McpServerConfig> {
	let mut tool_map = std::collections::HashMap::new();
	let enabled_servers: Vec<crate::config::McpServerConfig> = config.mcp.servers.to_vec();

	for server in enabled_servers {
		// Get all functions this server provides
		let server_functions = match server.server_type {
			crate::config::McpServerType::Developer => {
				crate::mcp::get_cached_internal_functions("developer", &server.tools, || {
					crate::mcp::dev::get_all_functions()
				})
			}
			crate::config::McpServerType::Filesystem => {
				crate::mcp::get_cached_internal_functions("filesystem", &server.tools, || {
					crate::mcp::fs::get_all_functions()
				})
			}
			crate::config::McpServerType::Agent => {
				// For agent server, get all agent functions based on config
				let server_functions = crate::mcp::agent::get_all_functions(config);
				if server.tools.is_empty() {
					server_functions
				} else {
					server_functions
						.into_iter()
						.filter(|f| server.tools.contains(&f.name))
						.collect()
				}
			}
			crate::config::McpServerType::External => {
				// For external servers, get their actual functions
				match crate::mcp::server::get_server_functions_cached(&server).await {
					Ok(functions) => {
						if server.tools.is_empty() {
							functions // All functions allowed
						} else {
							functions
								.into_iter()
								.filter(|func| server.tools.contains(&func.name))
								.collect()
						}
					}
					Err(_) => Vec::new(), // Server not available, skip
				}
			}
		};

		// Map each function name to this server
		for function in server_functions {
			// CONFIGURATION ORDER PRIORITY: First server wins for each tool
			tool_map
				.entry(function.name)
				.or_insert_with(|| server.clone());
		}
	}

	tool_map
}

// Display tool headers and parameters for all log levels (before execution)
async fn display_tool_headers(config: &Config, tool_calls: &[crate::mcp::McpToolCall]) {
	if !tool_calls.is_empty() {
		// Always log debug info if debug enabled
		log_debug!("Found {} tool calls in response", tool_calls.len());

		// Display headers and parameters for ALL modes
		for call in tool_calls.iter() {
			// Always show the header - use async version for accurate server lookup
			let server_name = get_tool_server_name_async(&call.tool_name, config).await;
			let title = format!(
				" {} | {} ",
				call.tool_name.bright_cyan(),
				server_name.bright_blue()
			);
			let separator_length = 70.max(title.len() + 4);
			let dashes = "─".repeat(separator_length - title.len());
			let separator = format!("──{}{}──", title, dashes.dimmed());
			println!("{}", separator);

			// Show parameters based on log level
			if config.get_log_level().is_info_enabled() || config.get_log_level().is_debug_enabled()
			{
				// Info/Debug mode: Show full parameters
				display_tool_parameters_full(call, config);
			}
			// None mode: No parameters shown
		}
	}
}

// Display tool parameters in full detail (for info/debug modes)
fn display_tool_parameters_full(tool_call: &crate::mcp::McpToolCall, config: &Config) {
	if let Ok(params_obj) = serde_json::from_value::<serde_json::Map<String, serde_json::Value>>(
		tool_call.parameters.clone(),
	) {
		if !params_obj.is_empty() {
			// Find the longest key for column alignment (max 20 chars to prevent excessive spacing)
			let max_key_length = params_obj
				.keys()
				.map(|k| k.len())
				.max()
				.unwrap_or(0)
				.min(20);

			for (key, value) in params_obj.iter() {
				let formatted_value = if config.get_log_level().is_debug_enabled() {
					// Debug mode: Show full value
					format_parameter_value_full(value)
				} else {
					// Info mode: Show smart formatted value
					format_parameter_value_smart(value)
				};

				// Format with proper column alignment and indentation
				println!(
					"{}: {}",
					format!("{:width$}", key, width = max_key_length).bright_blue(),
					formatted_value.white()
				);
			}
		}
	} else {
		// Fallback for non-object parameters (arrays, primitives, etc.)
		let params_str = serde_json::to_string(&tool_call.parameters).unwrap_or_default();
		if params_str != "null" {
			if config.get_log_level().is_debug_enabled() {
				// Debug mode: Show full params
				println!("params: {}", params_str);
			} else if params_str.chars().count() > 100 {
				// Info mode: Truncate long params
				let truncated: String = params_str.chars().take(97).collect();
				println!("params: {}...", truncated);
			} else {
				println!("params: {}", params_str);
			}
		}
	}
}

// Format a parameter value for smart display (info mode)
fn format_parameter_value_smart(value: &serde_json::Value) -> String {
	match value {
		serde_json::Value::String(s) => {
			if s.is_empty() {
				"\"\"".bright_black().to_string()
			} else if s.chars().count() > 100 {
				format!("\"{}...\"", s.chars().take(97).collect::<String>())
			} else if s.contains('\n') {
				// For multiline strings, show first line + indicator
				let lines: Vec<&str> = s.lines().collect();
				let first_line = lines.first().unwrap_or(&"");
				let first_line_chars: Vec<char> = first_line.chars().collect();
				if first_line_chars.len() > 80 {
					format!(
						"\"{}...\" [+{} lines]",
						first_line_chars.into_iter().take(77).collect::<String>(),
						lines.len().saturating_sub(1)
					)
				} else if lines.len() > 1 {
					format!(
						"\"{}\" [+{} lines]",
						first_line,
						lines.len().saturating_sub(1)
					)
				} else {
					format!("\"{}\"", first_line)
				}
			} else {
				format!("\"{}\"", s)
			}
		}
		serde_json::Value::Bool(b) => b.to_string(),
		serde_json::Value::Number(n) => n.to_string(),
		serde_json::Value::Array(arr) => {
			if arr.is_empty() {
				"[]".to_string()
			} else if arr.len() > 3 {
				format!("[{} items]", arr.len())
			} else {
				// Show small arrays inline
				let items: Vec<String> = arr
					.iter()
					.take(3)
					.map(|item| match item {
						serde_json::Value::String(s) => format!(
							"\"{}\"",
							if s.chars().count() > 20 {
								format!("{}...", s.chars().take(17).collect::<String>())
							} else {
								s.clone()
							}
						),
						_ => item.to_string(),
					})
					.collect();
				format!("[{}]", items.join(", "))
			}
		}
		serde_json::Value::Object(obj) => {
			if obj.is_empty() {
				"{}".to_string()
			} else {
				let obj_str = serde_json::to_string(value).unwrap_or_default();
				if obj_str.chars().count() > 100 {
					format!("{{...}} ({} keys)", obj.len())
				} else {
					obj_str
				}
			}
		}
		serde_json::Value::Null => "null".bright_black().to_string(),
	}
}

// Format a parameter value for full display (debug mode)
fn format_parameter_value_full(value: &serde_json::Value) -> String {
	// Debug mode: Show everything without truncation
	match value {
		serde_json::Value::String(s) => format!("\"{}\"", s),
		_ => serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string()),
	}
}

// Helper function to resolve current tool calls
fn resolve_tool_calls(
	current_tool_calls_param: &mut Option<Vec<crate::mcp::McpToolCall>>,
	current_content: &str,
) -> Vec<crate::mcp::McpToolCall> {
	if let Some(calls) = current_tool_calls_param.take() {
		// Use the tool calls from the API response only once
		if !calls.is_empty() {
			calls
		} else {
			crate::mcp::parse_tool_calls(current_content) // Fallback
		}
	} else {
		// For follow-up iterations, parse from content if any new tool calls exist
		crate::mcp::parse_tool_calls(current_content)
	}
}

// Helper function to check for cancellation
fn check_cancellation(operation_cancelled: &Arc<AtomicBool>) -> Result<()> {
	if operation_cancelled.load(Ordering::SeqCst) {
		println!("{}", "\nOperation cancelled by user.".bright_yellow());
		return Err(anyhow::anyhow!("Operation cancelled"));
	}
	Ok(())
}

// Helper function to add assistant message with tool calls preserved
fn add_assistant_message_with_tool_calls(
	chat_session: &mut ChatSession,
	current_content: &str,
	current_exchange: &ProviderExchange,
	_config: &Config,
	_role: &str,
) -> Result<()> {
	// CRITICAL FIX: We need to add the assistant message with tool_calls PRESERVED
	// The standard add_assistant_message only stores text content, but we need
	// to preserve the tool_calls from the original API response for proper conversation flow

	// Extract the original tool_calls from the exchange response based on provider
	let original_tool_calls = MessageHandler::extract_original_tool_calls(current_exchange);

	// Create the assistant message directly with tool_calls preserved from the exchange
	let assistant_message = crate::session::Message {
		role: "assistant".to_string(),
		content: current_content.to_string(),
		timestamp: std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.unwrap_or_default()
			.as_secs(),
		cached: false,
		tool_call_id: None,
		name: None,
		tool_calls: original_tool_calls, // Store the original tool_calls for proper reconstruction
		images: None,
	};

	// Add the assistant message to the session
	chat_session.session.messages.push(assistant_message);

	// Update last response - no cost tracking here as it will be handled by follow-up processing
	chat_session.last_response = current_content.to_string();

	// Log the assistant response and exchange
	let _ = crate::session::logger::log_assistant_response(
		&chat_session.session.info.name,
		current_content,
	);
	let _ = crate::session::logger::log_raw_exchange(current_exchange);

	Ok(())
}

// Function to process response, handling tool calls recursively
#[allow(clippy::too_many_arguments)]
pub async fn process_response(
	content: String,
	exchange: ProviderExchange,
	tool_calls: Option<Vec<crate::mcp::McpToolCall>>,
	finish_reason: Option<String>,
	chat_session: &mut ChatSession,
	config: &Config,
	role: &str,
	operation_cancelled: Arc<AtomicBool>,
) -> Result<()> {
	// Check if operation has been cancelled at the very start
	check_cancellation(&operation_cancelled)?;

	// Debug logging for finish_reason and tool calls
	log_response_debug(config, &finish_reason, &tool_calls);

	// First, add the user message before processing response
	let last_message = chat_session.session.messages.last();
	if last_message.is_none_or(|msg| msg.role != "user") {
		// This is an edge case - the content variable here is the AI response, not user input
		// We should have added the user message earlier in the main run_interactive_session
		println!(
			"{}",
			"Warning: User message not found in session. This is unexpected.".yellow()
		);
	}

	// Initialize tool processor
	let mut tool_processor = ToolProcessor::new();

	// Process original content first, then any follow-up tool calls
	let mut current_content = content.clone();
	let mut current_exchange = exchange;
	let mut current_tool_calls_param = tool_calls.clone(); // Track the tool_calls parameter

	loop {
		// Check for cancellation at the start of each loop iteration
		check_cancellation(&operation_cancelled)?;

		// Check for tool calls if MCP has any servers configured
		if !config.mcp.servers.is_empty() {
			// Resolve current tool calls for this iteration
			let current_tool_calls =
				resolve_tool_calls(&mut current_tool_calls_param, &current_content);

			if !current_tool_calls.is_empty() {
				// Add assistant message with tool calls preserved
				add_assistant_message_with_tool_calls(
					chat_session,
					&current_content,
					&current_exchange,
					config,
					role,
				)?;

				// Display the clean content (without function calls) to the user FIRST
				let clean_content = remove_function_calls(&current_content);
				print_assistant_response(&clean_content, config, role);

				// Display tool headers and parameters for all modes (after AI response)
				display_tool_headers(config, &current_tool_calls).await;

				// Early exit if cancellation was requested
				if operation_cancelled.load(Ordering::SeqCst) {
					println!("{}", "\nOperation cancelled by user.".bright_yellow());
					// Do NOT add any confusing message to the session
					return Ok(());
				}

				// Execute all tool calls in parallel using the new module
				let (tool_results, total_tool_time_ms) = tool_execution::execute_tools_parallel(
					current_tool_calls,
					chat_session,
					config,
					&mut tool_processor,
					operation_cancelled.clone(),
				)
				.await?;

				// Final cancellation check after all tools processed
				if operation_cancelled.load(Ordering::SeqCst) {
					println!(
						"{}",
						"\nTool execution cancelled - preserving any completed results."
							.bright_yellow()
					);
					// Still continue with processing any completed tool results
				}

				// Process tool results if any exist
				if !tool_results.is_empty() {
					// Process tool results and handle follow-up API calls using the new module
					if let Some((new_content, new_exchange, new_tool_calls)) =
						tool_result_processor::process_tool_results(
							tool_results,
							total_tool_time_ms,
							chat_session,
							config,
							role,
							operation_cancelled.clone(),
						)
						.await?
					{
						// Update current content for next iteration
						current_content = new_content;
						current_exchange = new_exchange;
						current_tool_calls_param = new_tool_calls;

						// Check if there are more tools to process
						if current_tool_calls_param.is_some()
							&& !current_tool_calls_param.as_ref().unwrap().is_empty()
						{
							// Continue processing the new content with tool calls
							continue;
						} else {
							// Check if there are more tool calls in the content itself
							let more_tools = crate::mcp::parse_tool_calls(&current_content);
							if !more_tools.is_empty() {
								// Log if debug mode is enabled
								log_debug!(
									"Found {} more tool calls to process in content",
									more_tools.len()
								);
								continue;
							} else {
								// No more tool calls, break out of the loop
								break;
							}
						}
					} else {
						// No follow-up response (cancelled or error), exit
						return Ok(());
					}
				} else {
					// No tool results - check if there were more tools to execute directly
					let more_tools = crate::mcp::parse_tool_calls(&current_content);
					if !more_tools.is_empty() {
						// Log if debug mode is enabled
						log_debug!(
							"Found {} more tool calls to process (no previous tool results)",
							more_tools.len()
						);
						// If there are more tool calls later in the response, continue processing
						continue;
					} else {
						// No more tool calls, exit the loop
						break;
					}
				}
			} else {
				// No tool calls in this content, break out of the loop
				break;
			}
		} else {
			// MCP not enabled, break out of the loop
			break;
		}
	}

	// Handle final response using helper function
	handle_final_response(
		&content,
		&current_content,
		current_exchange,
		chat_session,
		config,
		role,
	)
}
