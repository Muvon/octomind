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

mod tool_execution;
mod tool_result_processor;

use super::{CostTracker, MessageHandler, ToolProcessor};
use crate::config::Config;
use crate::session::chat::assistant_output::print_assistant_response;
use crate::session::chat::formatting::remove_function_calls;
use crate::session::chat::session::ChatSession;
use crate::session::ProviderExchange;
use crate::{log_debug};
use anyhow::Result;
use colored::Colorize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

// Helper function to log debug information about the response
fn log_response_debug(config: &Config, finish_reason: &Option<String>, tool_calls: &Option<Vec<crate::mcp::McpToolCall>>) {
	if config.get_log_level().is_debug_enabled() {
		if let Some(ref reason) = finish_reason {
			log_debug!("Processing response with finish_reason: {}", reason);
		}
		if let Some(ref calls) = tool_calls {
			log_debug!("Processing {} tool calls", calls.len());
		}
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

// Helper function to log tool calls in debug mode
fn log_tool_calls_debug(config: &Config, tool_calls: &[crate::mcp::McpToolCall]) {
	if config.get_log_level().is_debug_enabled() && !tool_calls.is_empty() {
		log_debug!("Found {} tool calls in response", tool_calls.len());
		for (i, call) in tool_calls.iter().enumerate() {
			log_debug!(
				"  Tool call {}: {} with params: {}",
				i + 1,
				call.tool_name,
				call.parameters
			);
		}
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
	config: &Config,
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
	};

	// Add the assistant message to the session
	chat_session.session.messages.push(assistant_message);

	// Update last response and handle exchange/cost tracking if provided
	chat_session.last_response = current_content.to_string();

	// Handle cost tracking from the exchange (same logic as add_assistant_message)
	if let Some(usage) = &current_exchange.usage {
		// Calculate regular and cached tokens
		let mut regular_prompt_tokens = usage.prompt_tokens;
		let mut cached_tokens = 0;

		// Check prompt_tokens_details for cached_tokens first
		if let Some(details) = &usage.prompt_tokens_details {
			if let Some(serde_json::Value::Number(num)) = details.get("cached_tokens") {
				if let Some(num_u64) = num.as_u64() {
					cached_tokens = num_u64;
					regular_prompt_tokens = usage.prompt_tokens.saturating_sub(cached_tokens);
				}
			}
		}

		// Fall back to breakdown field
		if cached_tokens == 0 && usage.prompt_tokens > 0 {
			if let Some(breakdown) = &usage.breakdown {
				if let Some(serde_json::Value::Number(num)) = breakdown.get("cached") {
					if let Some(num_u64) = num.as_u64() {
						cached_tokens = num_u64;
						regular_prompt_tokens = usage.prompt_tokens.saturating_sub(cached_tokens);
					}
				}
			}
		}

		// Track API time if available
		if let Some(api_time_ms) = usage.request_time_ms {
			chat_session.session.info.total_api_time_ms += api_time_ms;
		}

		// Update session token counts using cache manager
		let cache_manager = crate::session::cache::CacheManager::new();
		cache_manager.update_token_tracking(
			&mut chat_session.session,
			regular_prompt_tokens,
			usage.completion_tokens,
			cached_tokens,
		);

		// Update cost
		if let Some(cost) = usage.cost {
			chat_session.session.info.total_cost += cost;
			chat_session.estimated_cost = chat_session.session.info.total_cost;

			if config.get_log_level().is_debug_enabled() {
				log_debug!(
					"Adding ${:.5} from initial API (total now: ${:.5})",
					cost,
					chat_session.session.info.total_cost
				);
			}
		}
	}

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
			let current_tool_calls = resolve_tool_calls(&mut current_tool_calls_param, &current_content);

			// Log tool calls in debug mode
			log_tool_calls_debug(config, &current_tool_calls);

			if !current_tool_calls.is_empty() {
				// Add assistant message with tool calls preserved
				add_assistant_message_with_tool_calls(
					chat_session,
					&current_content,
					&current_exchange,
					config,
					role,
				)?;

				// Display the clean content (without function calls) to the user
				let clean_content = remove_function_calls(&current_content);
				print_assistant_response(&clean_content, config, role);

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
				).await?;

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
					if let Some((new_content, new_exchange, new_tool_calls)) = tool_result_processor::process_tool_results(
						tool_results,
						total_tool_time_ms,
						chat_session,
						config,
						role,
						operation_cancelled.clone(),
					).await? {
						// Update current content for next iteration
						current_content = new_content;
						current_exchange = new_exchange;
						current_tool_calls_param = new_tool_calls;

						// Check if there are more tools to process
						if current_tool_calls_param.is_some() && !current_tool_calls_param.as_ref().unwrap().is_empty() {
							// Continue processing the new content with tool calls
							continue;
						} else {
							// Check if there are more tool calls in the content itself
							let more_tools = crate::mcp::parse_tool_calls(&current_content);
							if !more_tools.is_empty() {
								// Log if debug mode is enabled
								if config.get_log_level().is_debug_enabled() {
									println!("{}", format!("Debug: Found {} more tool calls to process in content", more_tools.len()).yellow());
								}
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
						if config.get_log_level().is_debug_enabled() {
							println!("{}", format!("Debug: Found {} more tool calls to process (no previous tool results)", more_tools.len()).yellow());
						}
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
