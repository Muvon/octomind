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
pub mod tool_result_processor;

use super::{CostTracker, MessageHandler, ToolProcessor};
use crate::config::Config;
use crate::log_debug;
use crate::session::chat::assistant_output::print_assistant_response;
use crate::session::chat::session::ChatSession;
use crate::session::chat::session_continuation;
use crate::session::ProviderExchange;
use anyhow::Result;
use colored::Colorize;

// Response processing parameters struct
pub struct ResponseProcessingParams<'a> {
	pub content: String,
	pub exchange: ProviderExchange,
	pub tool_calls: Option<Vec<crate::mcp::McpToolCall>>,
	pub finish_reason: Option<String>,
	pub chat_session: &'a mut ChatSession,
	pub config: &'a Config,
	pub role: &'a str,
	pub operation_cancelled: tokio::sync::watch::Receiver<bool>,
}

impl<'a> ResponseProcessingParams<'a> {
	#[allow(clippy::too_many_arguments)]
	pub fn new(
		content: String,
		exchange: ProviderExchange,
		tool_calls: Option<Vec<crate::mcp::McpToolCall>>,
		finish_reason: Option<String>,
		chat_session: &'a mut ChatSession,
		config: &'a Config,
		role: &'a str,
		operation_cancelled: tokio::sync::watch::Receiver<bool>,
	) -> Self {
		Self {
			content,
			exchange,
			tool_calls,
			finish_reason,
			chat_session,
			config,
			role,
			operation_cancelled,
		}
	}
}

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
	chat_session: &mut ChatSession,
	config: &Config,
	role: &str,
) -> Result<()> {
	// CRITICAL: Add the assistant message to the session for continuation logic to work
	// This was removed in the recent commit but is needed for proper session state
	chat_session.add_assistant_message(content, None, config, role)?;

	// Print assistant response with color
	print_assistant_response(content, config, role);

	// Display cost line only for non-interactive mode or specific scenarios
	// Skip for interactive mode to avoid duplication before user input prompt
	use std::io::IsTerminal;
	if !std::io::stdin().is_terminal() {
		// Non-interactive mode - always show cost line
		CostTracker::display_cost_line(chat_session);
	}
	// Interactive mode: Skip cost line here to avoid duplication before user input

	Ok(())
}

// Get the actual server name for a tool (async version that matches execution)
pub async fn get_tool_server_name_async(tool_name: &str, _config: &Config) -> String {
	// STATIC ONLY: Use pre-built static tool map
	crate::mcp::tool_map::get_tool_server_name(tool_name).unwrap_or_else(|| "unknown".to_string())
}

// Display execution intent with headers upfront (before execution)
async fn display_tool_parameters_only(config: &Config, tool_calls: &[crate::mcp::McpToolCall]) {
	if !tool_calls.is_empty() {
		// Always log debug info if debug enabled
		log_debug!("Found {} tool calls in response", tool_calls.len());

		let is_single_tool = tool_calls.len() == 1;

		// Show headers upfront - with indices for multiple tools, without for single tool
		for (index, call) in tool_calls.iter().enumerate() {
			let tool_index = index + 1;

			// Get server name using same logic as execution
			let server_name = get_tool_server_name_async(&call.tool_name, config).await;

			// Create formatted header - with or without index based on tool count
			let title = if is_single_tool {
				format!(
					" {} | {} ",
					call.tool_name.bright_cyan(),
					server_name.bright_blue()
				)
			} else {
				format!(
					" [{}] {} | {} ",
					tool_index,
					call.tool_name.bright_cyan(),
					server_name.bright_blue()
				)
			};
			let separator_length = 70.max(title.len() + 4);
			let dashes = "─".repeat(separator_length - title.len());
			let separator = format!("──{}{}──", title, dashes.dimmed());
			println!("{}", separator);

			// Show parameters based on log level
			if config.get_log_level().is_debug_enabled() || config.get_log_level().is_info_enabled()
			{
				display_tool_parameters_full(call, config);
			}

			// Add spacing between tools (except for the last one)
			if index < tool_calls.len() - 1 {
				println!();
			}
		}

		// Add final spacing before execution starts
		println!();
	}
}

// Display tool parameters in full detail (for info/debug modes)
pub fn display_tool_parameters_full(tool_call: &crate::mcp::McpToolCall, config: &Config) {
	// Delegate to shared implementation
	crate::session::chat::tool_display::display_tool_parameters_full(tool_call, config);
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
fn check_cancellation(operation_cancelled: &tokio::sync::watch::Receiver<bool>) -> Result<()> {
	if *operation_cancelled.borrow() {
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
pub async fn process_response(params: ResponseProcessingParams<'_>) -> Result<()> {
	// Check if operation has been cancelled at the very start
	check_cancellation(&params.operation_cancelled)?;

	// Debug logging for finish_reason and tool calls
	log_response_debug(params.config, &params.finish_reason, &params.tool_calls);

	// CONTINUATION FIX: Removed early continuation check that was causing tool_calls/tool_result mismatch
	// Continuation is now handled AFTER tool processing completes to ensure conversation integrity

	// CRITICAL FIX: Check for continuation BEFORE any tool processing
	// LOGIC: If continuation_pending=true, skip ALL tool processing and handle continuation immediately
	// FLOW: Token limit → inject_summary_request() → continuation_pending=true → AI responds with summary
	//       → Check continuation_pending FIRST → If true: skip tools, process continuation immediately
	//       → If false: continue normal tool processing
	// IMPORTANT: This prevents tool calls in summary responses from interfering with continuation
	let has_tool_calls = params
		.tool_calls
		.as_ref()
		.is_some_and(|calls| !calls.is_empty());

	// Check if continuation is pending - if so, handle it immediately and skip tools
	if params.chat_session.continuation_pending
		&& session_continuation::process_continuation_response(
			params.chat_session,
			&params.content,
			has_tool_calls,
			params.config,
			params.role,
		)
		.await?
	{
		// Continuation was processed - handle it immediately with the summary response
		return process_continuation_message_immediately(params).await;
	}

	// First, add the user message before processing response
	let last_message = params.chat_session.session.messages.last();
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
	let mut current_content = params.content.clone();
	let mut current_exchange = params.exchange;
	let mut current_tool_calls_param = params.tool_calls.clone(); // Track the tool_calls parameter
	let operation_cancelled_ref = &params.operation_cancelled; // Create a reference to avoid moves

	loop {
		// Check for cancellation at the start of each loop iteration
		check_cancellation(operation_cancelled_ref)?;

		// Check for tool calls if MCP has any servers configured
		if !params.config.mcp.servers.is_empty() {
			// Resolve current tool calls for this iteration
			let current_tool_calls =
				resolve_tool_calls(&mut current_tool_calls_param, &current_content);

			if !current_tool_calls.is_empty() {
				// Add assistant message with tool calls preserved
				add_assistant_message_with_tool_calls(
					params.chat_session,
					&current_content,
					&current_exchange,
					params.config,
					params.role,
				)?;

				// Display the content to the user FIRST
				print_assistant_response(&current_content, params.config, params.role);

				// Display tool parameters upfront (headers will be shown per-tool during execution)
				display_tool_parameters_only(params.config, &current_tool_calls).await;

				// Clone operation_cancelled to avoid borrow issues
				let operation_cancelled_clone = params.operation_cancelled.clone();

				// Early exit if cancellation was requested
				if *operation_cancelled_clone.borrow() {
					println!("{}", "\nOperation cancelled by user.".bright_yellow());
					// Do NOT add any confusing message to the session
					return Ok(());
				}

				// Execute all tool calls in parallel using the new module
				let (tool_results, total_tool_time_ms) = tool_execution::execute_tools_parallel(
					current_tool_calls,
					params.chat_session,
					params.config,
					&mut tool_processor,
					operation_cancelled_clone.clone(),
				)
				.await?;

				// Final cancellation check after all tools processed
				if *operation_cancelled_clone.borrow() {
					println!(
						"{}",
						"\nTool execution cancelled - cleaning up conversation state."
							.bright_yellow()
					);

					// BOSS FIX: Remove the assistant message with tool_calls when cancelled
					// The problem: we added assistant message with tool_calls but cancellation prevents proper tool_result processing
					if let Some(last_msg) = params.chat_session.session.messages.last() {
						if last_msg.role == "assistant" && last_msg.tool_calls.is_some() {
							// Last message is broken assistant with tool_calls - remove it on cancellation
							params.chat_session.session.messages.pop();
							log_debug!("Removed last assistant message with tool_calls due to cancellation");
						}
					}

					// DO NOT process tool results when cancelled - this prevents conversation corruption
					return Ok(());
				}

				// Process tool results if any exist
				if !tool_results.is_empty() {
					// Process tool results and handle follow-up API calls using the new module
					if let Some((new_content, new_exchange, new_tool_calls)) =
						tool_result_processor::process_tool_results(
							tool_results,
							total_tool_time_ms,
							params.chat_session,
							params.config,
							params.role,
							operation_cancelled_clone.clone(),
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
						// No follow-up response - check if this was due to continuation being triggered
						if params.chat_session.continuation_pending {
							log_debug!("Tool processing stopped due to continuation trigger - breaking out of tool loop to handle continuation");
							// Break out of tool processing loop to let the main continuation check handle it
							break;
						}
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

	// CRITICAL FIX: Check for continuation after tool processing loop
	// When continuation is triggered during tool execution, we broke out of the tool loop (line 430)
	// The main session runner will detect continuation_pending and handle it automatically (runner.rs:984-988)
	// We just need to skip final response processing and return cleanly
	if params.chat_session.continuation_pending {
		log_debug!("Continuation pending after tool processing - skipping final response, main session loop will handle continuation");
		// DO NOT process final response when continuation is pending
		// The main session loop will detect continuation_pending and continue to process the summary request
		return Ok(());
	}

	// Handle final response using helper function (only when no continuation is pending)
	handle_final_response(
		&current_content,
		params.chat_session,
		params.config,
		params.role,
	)?;

	Ok(())
}

/// Process continuation message immediately after session reset
/// This makes the continuation completely invisible to the user
pub async fn process_continuation_message_immediately(
	params: ResponseProcessingParams<'_>,
) -> Result<()> {
	use crate::session::ChatCompletionWithValidationParams;
	use crate::{log_debug, log_info};

	log_info!("Processing continuation message automatically...");

	// Get the last message which should be our continuation message
	let continuation_message = params
		.chat_session
		.session
		.messages
		.last()
		.ok_or_else(|| anyhow::anyhow!("No continuation message found"))?;

	if continuation_message.role != "user" {
		return Err(anyhow::anyhow!(
			"Expected user continuation message, found: {}",
			continuation_message.role
		));
	}

	// Clone messages to avoid borrowing conflicts
	let messages = params.chat_session.session.messages.clone();

	// Prepare API call parameters for continuation using the session's current settings
	let chat_params = ChatCompletionWithValidationParams::new(
		&messages,
		&params.chat_session.model,
		params.chat_session.temperature,
		params.chat_session.top_p,
		params.chat_session.top_k,
		params.chat_session.max_tokens,
		params.config,
	)
	.with_max_retries(params.chat_session.max_retries)
	.with_cancellation_token(params.operation_cancelled.clone());

	// Make API call with continuation message
	match crate::session::chat_completion_with_validation(chat_params).await {
		Ok(response) => {
			log_debug!("Continuation API call successful");

			// Process the continuation response recursively using Box::pin to avoid stack overflow
			let continuation_params = ResponseProcessingParams::new(
				response.content,
				response.exchange,
				response.tool_calls,
				response.finish_reason,
				params.chat_session,
				params.config,
				params.role,
				params.operation_cancelled.clone(),
			);

			// Use Box::pin to avoid recursion compilation issues
			Box::pin(process_response(continuation_params)).await
		}
		Err(e) => {
			// If continuation fails, log the error but don't crash the session
			log_info!("Continuation API call failed: {}", e);
			log_info!("Falling back to normal session flow");
			Ok(())
		}
	}
}
