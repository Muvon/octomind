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

// Response processing module

use super::animation::show_loading_animation;
use crate::config::Config;
use crate::session::chat::assistant_output::print_assistant_response;
use crate::session::chat::formatting::{format_duration, remove_function_calls};
use crate::session::chat::tool_error_tracker::ToolErrorTracker;
use crate::session::chat::session::ChatSession;
use crate::session::ProviderExchange;
use crate::{log_debug, log_info};
use anyhow::Result;
use colored::Colorize;
use serde_json;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

// CRITICAL FIX: Provider-agnostic function to extract original tool calls
// This handles different provider formats and ensures proper tool_calls preservation
fn extract_original_tool_calls(exchange: &ProviderExchange) -> Option<serde_json::Value> {
	// First check if there's a stored tool_calls_content (for Anthropic and Google)
	if let Some(content_data) = exchange.response.get("tool_calls_content") {
		return Some(content_data.clone());
	}
	
	// Then check for OpenRouter/OpenAI format
	if let Some(tool_calls) = exchange.response
		.get("choices")
		.and_then(|choices| choices.get(0))
		.and_then(|choice| choice.get("message"))
		.and_then(|message| message.get("tool_calls"))
	{
		return Some(tool_calls.clone());
	}
	
	None
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
	if operation_cancelled.load(Ordering::SeqCst) {
		println!("{}", "\nOperation cancelled by user.".bright_yellow());
		return Ok(());
	}

	// Debug logging for finish_reason and tool calls
	if config.get_log_level().is_debug_enabled() {
		if let Some(ref reason) = finish_reason {
			log_debug!("Processing response with finish_reason: {}", reason);
		}
		if let Some(ref calls) = tool_calls {
			log_debug!("Processing {} tool calls", calls.len());
		}
	}

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
	// Initialize tool error tracker with max of 3 consecutive errors
	let mut error_tracker = ToolErrorTracker::new(3);

	// Process original content first, then any follow-up tool calls
	let mut current_content = content.clone();
	let mut current_exchange = exchange;
	let mut current_tool_calls_param = tool_calls.clone(); // Track the tool_calls parameter

	loop {
		// Check for cancellation at the start of each loop iteration
		if operation_cancelled.load(Ordering::SeqCst) {
			println!("{}", "\nOperation cancelled by user.".bright_yellow());
			return Ok(());
		}

		// Check for tool calls if MCP has any servers configured
		if !config.mcp.servers.is_empty() {
			// CRITICAL FIX: Use current_tool_calls_param for the first iteration only
			// For subsequent iterations, we should NOT reuse the same tool calls
			let current_tool_calls = if let Some(calls) = current_tool_calls_param.take() {
				// Use the tool calls from the API response only once
				if !calls.is_empty() {
					calls
				} else {
					crate::mcp::parse_tool_calls(&current_content) // Fallback
				}
			} else {
				// For follow-up iterations, parse from content if any new tool calls exist
				crate::mcp::parse_tool_calls(&current_content)
			};

			// Add debug logging for tool calls when debug mode is enabled
			if config.get_log_level().is_debug_enabled() && !current_tool_calls.is_empty() {
				log_debug!("Found {} tool calls in response", current_tool_calls.len());
				for (i, call) in current_tool_calls.iter().enumerate() {
					log_debug!(
						"  Tool call {}: {} with params: {}",
						i + 1,
						call.tool_name,
						call.parameters
					);
				}
			}

			if !current_tool_calls.is_empty() {
				// CRITICAL FIX: We need to add the assistant message with tool_calls PRESERVED
				// The standard add_assistant_message only stores text content, but we need
				// to preserve the tool_calls from the original API response for proper conversation flow

				// Extract the original tool_calls from the exchange response based on provider
				let original_tool_calls = extract_original_tool_calls(&current_exchange);

				// Create the assistant message directly with tool_calls preserved from the exchange
				let assistant_message = crate::session::Message {
					role: "assistant".to_string(),
					content: current_content.clone(),
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
				chat_session.last_response = current_content.clone();

				// Handle cost tracking from the exchange (same logic as add_assistant_message)
				if let Some(exchange) = &Some(current_exchange.clone()) {
					if let Some(usage) = &exchange.usage {
						// Calculate regular and cached tokens
						let mut regular_prompt_tokens = usage.prompt_tokens;
						let mut cached_tokens = 0;

						// Check prompt_tokens_details for cached_tokens first
						if let Some(details) = &usage.prompt_tokens_details {
							if let Some(serde_json::Value::Number(num)) =
								details.get("cached_tokens")
							{
								if let Some(num_u64) = num.as_u64() {
									cached_tokens = num_u64;
									regular_prompt_tokens =
										usage.prompt_tokens.saturating_sub(cached_tokens);
								}
							}
						}

						// Fall back to breakdown field
						if cached_tokens == 0 && usage.prompt_tokens > 0 {
							if let Some(breakdown) = &usage.breakdown {
								if let Some(serde_json::Value::Number(num)) =
									breakdown.get("cached")
								{
									if let Some(num_u64) = num.as_u64() {
										cached_tokens = num_u64;
										regular_prompt_tokens =
											usage.prompt_tokens.saturating_sub(cached_tokens);
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
				}

				// Log the assistant response and exchange
				let _ = crate::session::logger::log_assistant_response(
					&chat_session.session.info.name,
					&current_content,
				);
				if let Some(ex) = &Some(current_exchange.clone()) {
					let _ = crate::session::logger::log_raw_exchange(ex);
				}

				// Display the clean content (without function calls) to the user
				let clean_content = remove_function_calls(&current_content);
				print_assistant_response(&clean_content, config, role);

				// Early exit if cancellation was requested
				if operation_cancelled.load(Ordering::SeqCst) {
					println!("{}", "\nOperation cancelled by user.".bright_yellow());
					// Do NOT add any confusing message to the session
					return Ok(());
				}

				// Execute all tool calls in parallel
				let mut tool_tasks = Vec::new();

				for tool_call in current_tool_calls.clone() {
					// Increment tool call counter
					chat_session.session.info.tool_calls += 1;

					// CRITICAL FIX: Use the EXACT tool_id from the original API response
					// Don't generate a new UUID - use the one from the original tool_calls
					let original_tool_id = tool_call.tool_id.clone();

					// Clone tool_name separately for tool task tracking
					let tool_name = tool_call.tool_name.clone();

					// Execute in a tokio task
					let config_clone = config.clone();
					let params_clone = tool_call.parameters.clone();

					// Log the tool request with the session name and ORIGINAL tool_id
					let _ = crate::session::logger::log_tool_call(
						&chat_session.session.info.name,
						&tool_name,
						&original_tool_id,
						&params_clone,
					);

					let tool_id_for_task = original_tool_id.clone();
					let tool_call_clone = tool_call.clone(); // Clone for async move
					let cancel_token_for_task = operation_cancelled.clone(); // Pass cancellation token
					let task = tokio::spawn(async move {
						let mut call_with_id = tool_call_clone.clone();
						// CRITICAL: Use the original tool_id, don't change it
						call_with_id.tool_id = tool_id_for_task.clone();
						crate::mcp::execute_tool_call_with_cancellation(
							&call_with_id,
							&config_clone,
							Some(cancel_token_for_task),
						)
						.await
					});

					tool_tasks.push((tool_name, task, original_tool_id));
				}

				// Collect all results and display them cleanly with real-time cancellation feedback
				let mut tool_results = Vec::new();
				let mut _has_error = false;
				let mut total_tool_time_ms = 0; // Track cumulative tool execution time

				for (tool_name, task, tool_id) in tool_tasks {
					// Enhanced cancellation check with real-time feedback and delay
					if operation_cancelled.load(Ordering::SeqCst) {
						use colored::*;
						println!(
							"{}",
							format!("🛑 Cancelling tool execution: {}", tool_name).bright_yellow()
						);

						// Give the tool a brief moment to finish gracefully (500ms)
						let grace_start = std::time::Instant::now();
						let grace_period = std::time::Duration::from_millis(500);

						loop {
							// Check if tool finished during grace period
							if task.is_finished() {
								println!(
									"{}",
									format!("✓ Tool '{}' completed during grace period", tool_name)
										.bright_green()
								);

								// Process the completed result
								match task.await {
									Ok(result) => match result {
										Ok((res, tool_time_ms)) => {
											tool_results.push(res);
											total_tool_time_ms += tool_time_ms;
										}
										Err(e) => {
											println!(
												"{}",
												format!(
													"⚠ Tool '{}' completed with error: {}",
													tool_name, e
												)
												.bright_yellow()
											);
										}
									},
									Err(_) => {
										println!(
											"{}",
											format!(
												"⚠ Tool '{}' task error during grace period",
												tool_name
											)
											.bright_yellow()
										);
									}
								}
								break;
							}

							// Check if grace period expired
							if grace_start.elapsed() >= grace_period {
								println!(
									"{}",
									format!(
										"🗑️ Force cancelling tool '{}' - grace period expired",
										tool_name
									)
									.bright_red()
								);
								task.abort(); // Force abort the task
								break;
							}

							// Short sleep to avoid busy waiting
							tokio::time::sleep(std::time::Duration::from_millis(50)).await;
						}

						// Skip to next tool or finish cancellation
						continue;
					}

					// Store tool call info for consolidated display after execution
					let tool_call_info = current_tool_calls
						.iter()
						.find(|tc| tc.tool_id == tool_id)
						.or_else(|| {
							current_tool_calls
								.iter()
								.find(|tc| tc.tool_name == tool_name)
						});

					// Store for display after execution
					let stored_tool_call = tool_call_info.cloned();

					match task.await {
						Ok(result) => match result {
							Ok((res, tool_time_ms)) => {
								// Tool succeeded, reset the error counter
								error_tracker.record_success(&tool_name);

								// Display the complete tool execution with consolidated info
								if let Some(tool_call) = &stored_tool_call {
									let category =
										crate::mcp::guess_tool_category(&tool_call.tool_name);
									let title = format!(
										" {} | {} ",
										tool_call.tool_name.bright_cyan(),
										category.bright_blue()
									);
									let separator_length = 70.max(title.len() + 4);
									let dashes = "─".repeat(separator_length - title.len());
									let separator = format!("──{}{}──", title, dashes.dimmed());
									println!("{}", separator);

									// Show parameters based on log level
									if config.get_log_level().is_info_enabled() {
										if let Ok(params_obj) =
											serde_json::from_value::<
												serde_json::Map<String, serde_json::Value>,
											>(tool_call.parameters.clone())
										{
											if !params_obj.is_empty() {
												// Find the longest key for column alignment (max 20 chars to prevent excessive spacing)
												let max_key_length = params_obj
													.keys()
													.map(|k| k.len())
													.max()
													.unwrap_or(0)
													.min(20);

												for (key, value) in params_obj.iter() {
													let formatted_value = match value {
														serde_json::Value::String(s) => {
															if s.is_empty() {
																"\"\"".bright_black().to_string()
															} else if s.chars().count() > 100 {
																format!(
																	"\"{}...\"",
																	s.chars()
																		.take(97)
																		.collect::<String>()
																)
															} else if s.contains('\n') {
																// For multiline strings, show first line + indicator
																let lines: Vec<&str> =
																	s.lines().collect();
																let first_line =
																	lines.first().unwrap_or(&"");
																let first_line_chars: Vec<char> =
																	first_line.chars().collect();
																if first_line_chars.len() > 80 {
																	format!(
																		"\"{}...\" [+{} lines]",
																		first_line_chars
																			.into_iter()
																			.take(77)
																			.collect::<String>(),
																		lines
																			.len()
																			.saturating_sub(1)
																	)
																} else if lines.len() > 1 {
																	format!(
																		"\"{}\" [+{} lines]",
																		first_line,
																		lines
																			.len()
																			.saturating_sub(1)
																	)
																} else {
																	format!("\"{}\"", first_line)
																}
															} else {
																format!("\"{}\"", s)
															}
														}
														serde_json::Value::Bool(b) => b.to_string(),
														serde_json::Value::Number(n) => {
															n.to_string()
														}
														serde_json::Value::Array(arr) => {
															if arr.is_empty() {
																"[]".to_string()
															} else if arr.len() > 3 {
																format!("[{} items]", arr.len())
															} else {
																// Show small arrays inline
																let items: Vec<String> = arr.iter().take(3).map(|item| {
																	match item {
																		serde_json::Value::String(s) => format!("\"{}\"", if s.chars().count() > 20 { format!("{}...", s.chars().take(17).collect::<String>()) } else { s.clone() }),
																		_ => item.to_string()
																	}
																}).collect();
																format!("[{}]", items.join(", "))
															}
														}
														serde_json::Value::Object(obj) => {
															if obj.is_empty() {
																"{}".to_string()
															} else {
																let obj_str =
																	serde_json::to_string(value)
																		.unwrap_or_default();
																if obj_str.chars().count() > 100 {
																	format!(
																		"{{...}} ({} keys)",
																		obj.len()
																	)
																} else {
																	obj_str
																}
															}
														}
														serde_json::Value::Null => {
															"null".bright_black().to_string()
														}
													};

													// Format with proper column alignment and indentation
													println!(
														"{}: {}",
														format!(
															"{:width$}",
															key,
															width = max_key_length
														)
														.bright_blue(),
														formatted_value.white()
													);
												}
											}
										} else {
											// Fallback for non-object parameters (arrays, primitives, etc.)
											let params_str =
												serde_json::to_string(&tool_call.parameters)
													.unwrap_or_default();
											if params_str != "null" {
												if params_str.chars().count() > 100 {
													let truncated: String =
														params_str.chars().take(97).collect();
													println!("params: {}...", truncated);
												} else {
													println!("params: {}", params_str);
												}
											}
										}
									} else {
										// In non-info mode, show just the main parameter compactly
										if let Ok(params_obj) =
											serde_json::from_value::<
												serde_json::Map<String, serde_json::Value>,
											>(tool_call.parameters.clone())
										{
											// Try to find the main parameter (command, path, query, etc.)
											let main_param = params_obj
												.get("command")
												.or_else(|| params_obj.get("path"))
												.or_else(|| params_obj.get("query"))
												.or_else(|| params_obj.get("text"))
												.or_else(|| params_obj.get("content"))
												.or_else(|| params_obj.get("file"))
												.or_else(|| params_obj.get("filename"))
												.or_else(|| {
													params_obj.iter().next().map(|(_, v)| v)
												});

											if let Some(value) = main_param {
												if let Some(str_val) = value.as_str() {
													if !str_val.is_empty() {
														println!(
															"{}: {}",
															params_obj
																.iter()
																.find(|(_, v)| *v == value)
																.map(|(k, _)| k)
																.unwrap_or(&"param".to_string())
																.bright_blue(),
															if str_val.chars().count() > 80 {
																format!(
																	"{}...",
																	str_val
																		.chars()
																		.take(77)
																		.collect::<String>()
																)
															} else {
																str_val.to_string()
															}
														);
													}
												}
											}
										}
									}

									// Show the actual tool output
									if let Some(output) = res.result.get("output") {
										if let Some(output_str) = output.as_str() {
											if !output_str.trim().is_empty() {
												println!("{}", output_str);
											}
										}
									}

									// Show completion status with timing at the end
									println!(
										"✓ Tool '{}' completed in {}ms",
										tool_name, tool_time_ms
									);
								} else {
									// Fallback if tool_call info not found
									println!(
										"✓ Tool '{}' completed in {}ms",
										tool_name, tool_time_ms
									);
								}
								println!("──────────────────");

								// Log the tool response with session name
								let _ = crate::session::logger::log_tool_result(
									&chat_session.session.info.name,
									&tool_id,
									&res.result,
								);
								tool_results.push(res);
								// Accumulate tool execution time
								total_tool_time_ms += tool_time_ms;
							}
							Err(e) => {
								_has_error = true;

								// Check if this is a user-declined large output error
								if e.to_string().contains("LARGE_OUTPUT_DECLINED_BY_USER") {
									println!("⚠ Tool '{}' output declined by user - removing tool call from conversation", tool_name);

									// CRITICAL FIX: Remove the tool_use block from the assistant message
									// to prevent "tool_use ids found without tool_result blocks" error
									if let Some(last_msg) = chat_session.session.messages.last_mut()
									{
										if last_msg.role == "assistant" {
											if let Some(tool_calls_value) = &last_msg.tool_calls {
												// Parse the tool_calls and remove the declined one
												if let Ok(mut tool_calls_array) =
													serde_json::from_value::<Vec<serde_json::Value>>(
														tool_calls_value.clone(),
													) {
													// Remove the tool call with matching ID
													tool_calls_array.retain(|tc| {
														tc.get("id").and_then(|id| id.as_str())
															!= Some(&tool_id)
													});

													// Update the assistant message
													if tool_calls_array.is_empty() {
														// No more tool calls, remove the tool_calls field entirely
														last_msg.tool_calls = None;
														log_debug!("Removed all tool calls from assistant message after user declined large output");
													} else {
														// Update with remaining tool calls
														last_msg.tool_calls = Some(
															serde_json::to_value(tool_calls_array)
																.unwrap_or_default(),
														);
														log_debug!("Removed declined tool call '{}' from assistant message", tool_id);
													}
												}
											}
										}
									}

									// Don't add any tool result - the tool_use block is now properly removed
									continue;
								}

								// Display error in consolidated format for other errors
								if let Some(tool_call) = &stored_tool_call {
									let category =
										crate::mcp::guess_tool_category(&tool_call.tool_name);
									let title = format!(
										" {} | {} ",
										tool_call.tool_name.bright_cyan(),
										category.bright_blue()
									);
									let separator_length = 70.max(title.len() + 4);
									let dashes = "─".repeat(separator_length - title.len());
									let separator = format!("──{}{}──", title, dashes.dimmed());
									println!("{}", separator);

									// Show parameters in non-info mode (compact)
									if let Ok(params_obj) = serde_json::from_value::<
										serde_json::Map<String, serde_json::Value>,
									>(tool_call.parameters.clone())
									{
										// Try to find the main parameter (command, path, query, etc.)
										let main_param = params_obj
											.get("command")
											.or_else(|| params_obj.get("path"))
											.or_else(|| params_obj.get("query"))
											.or_else(|| params_obj.get("text"))
											.or_else(|| params_obj.get("content"))
											.or_else(|| params_obj.get("file"))
											.or_else(|| params_obj.get("filename"))
											.or_else(|| params_obj.iter().next().map(|(_, v)| v));

										if let Some(value) = main_param {
											if let Some(str_val) = value.as_str() {
												if !str_val.is_empty() {
													println!(
														"{}: {}",
														params_obj
															.iter()
															.find(|(_, v)| *v == value)
															.map(|(k, _)| k)
															.unwrap_or(&"param".to_string())
															.bright_blue(),
														if str_val.chars().count() > 80 {
															format!(
																"{}...",
																str_val
																	.chars()
																	.take(77)
																	.collect::<String>()
															)
														} else {
															str_val.to_string()
														}
													);
												}
											}
										}
									}
								}

								// Show error status
								println!("✗ Tool '{}' failed: {}", tool_name, e);

								// Track errors for this tool
								let loop_detected = error_tracker.record_error(&tool_name);

								if loop_detected {
									// Always show loop detection warning since it's critical
									println!("{}", format!("⚠ Warning: {} failed {} times in a row - AI should try a different approach",
										tool_name, error_tracker.max_consecutive_errors()).bright_yellow());

									// Add a detailed error result for loop detection
									let loop_error_result = crate::mcp::McpToolResult {
										tool_name: tool_name.clone(),
										tool_id: tool_id.clone(),
										result: serde_json::json!({
											"error": format!("LOOP DETECTED: Tool '{}' failed {} consecutive times. Last error: {}. Please try a completely different approach or ask the user for guidance.", tool_name, error_tracker.max_consecutive_errors(), e),
											"tool_name": tool_name,
											"consecutive_failures": error_tracker.max_consecutive_errors(),
											"loop_detected": true,
											"suggestion": "Try a different tool or approach, or ask user for clarification"
										}),
									};
									tool_results.push(loop_error_result);
								} else {
									// Regular error - add normal error result
									let error_result = crate::mcp::McpToolResult {
										tool_name: tool_name.clone(),
										tool_id: tool_id.clone(),
										result: serde_json::json!({
											"error": format!("Tool execution failed: {}", e),
											"tool_name": tool_name,
											"attempt": error_tracker.get_error_count(&tool_name),
											"max_attempts": error_tracker.max_consecutive_errors()
										}),
									};
									tool_results.push(error_result);

									if config.get_log_level().is_info_enabled() {
										log_info!("Tool '{}' failed {} of {} times. Adding error to context.",
											tool_name, error_tracker.get_error_count(&tool_name), error_tracker.max_consecutive_errors());
									}
								}
							}
						},
						Err(e) => {
							_has_error = true;

							// Check if this is a user-declined large output error (can occur at task level too)
							if e.to_string().contains("LARGE_OUTPUT_DECLINED_BY_USER") {
								println!("⚠ Tool '{}' task output declined by user - removing tool call from conversation", tool_name);

								// CRITICAL FIX: Remove the tool_use block from the assistant message
								// to prevent "tool_use ids found without tool_result blocks" error
								if let Some(last_msg) = chat_session.session.messages.last_mut() {
									if last_msg.role == "assistant" {
										if let Some(tool_calls_value) = &last_msg.tool_calls {
											// Parse the tool_calls and remove the declined one
											if let Ok(mut tool_calls_array) =
												serde_json::from_value::<Vec<serde_json::Value>>(
													tool_calls_value.clone(),
												) {
												// Remove the tool call with matching ID
												tool_calls_array.retain(|tc| {
													tc.get("id").and_then(|id| id.as_str())
														!= Some(&tool_id)
												});

												// Update the assistant message
												if tool_calls_array.is_empty() {
													// No more tool calls, remove the tool_calls field entirely
													last_msg.tool_calls = None;
													log_debug!("Removed all tool calls from assistant message after user declined large task output");
												} else {
													// Update with remaining tool calls
													last_msg.tool_calls = Some(
														serde_json::to_value(tool_calls_array)
															.unwrap_or_default(),
													);
													log_debug!("Removed declined tool call '{}' from assistant message (task error)", tool_id);
												}
											}
										}
									}
								}

								// Don't add any tool result - the tool_use block is now properly removed
								continue;
							}

							// Display task error in consolidated format for other errors
							if let Some(tool_call) = &stored_tool_call {
								let category =
									crate::mcp::guess_tool_category(&tool_call.tool_name);
								let title = format!(
									" {} | {} ",
									tool_call.tool_name.bright_cyan(),
									category.bright_blue()
								);
								let separator_length = 70.max(title.len() + 4);
								let dashes = "─".repeat(separator_length - title.len());
								let separator = format!("──{}{}──", title, dashes.dimmed());
								println!("{}", separator);

								// Show parameters in non-info mode (compact)
								if let Ok(params_obj) = serde_json::from_value::<
									serde_json::Map<String, serde_json::Value>,
								>(tool_call.parameters.clone())
								{
									// Try to find the main parameter (command, path, query, etc.)
									let main_param = params_obj
										.get("command")
										.or_else(|| params_obj.get("path"))
										.or_else(|| params_obj.get("query"))
										.or_else(|| params_obj.get("text"))
										.or_else(|| params_obj.get("content"))
										.or_else(|| params_obj.get("file"))
										.or_else(|| params_obj.get("filename"))
										.or_else(|| params_obj.iter().next().map(|(_, v)| v));

									if let Some(value) = main_param {
										if let Some(str_val) = value.as_str() {
											if !str_val.is_empty() {
												println!(
													"{}: {}",
													params_obj
														.iter()
														.find(|(_, v)| *v == value)
														.map(|(k, _)| k)
														.unwrap_or(&"param".to_string())
														.bright_blue(),
													if str_val.chars().count() > 80 {
														format!(
															"{}...",
															str_val
																.chars()
																.take(77)
																.collect::<String>()
														)
													} else {
														str_val.to_string()
													}
												);
											}
										}
									}
								}
							}

							// Show task error status
							println!("✗ Task error for '{}': {}", tool_name, e);

							// ALWAYS add error result for task failures too (unless it was a user decline)
							let error_result = crate::mcp::McpToolResult {
								tool_name: tool_name.clone(),
								tool_id: tool_id.clone(),
								result: serde_json::json!({
									"error": format!("Internal task error: {}", e),
									"tool_name": tool_name,
									"error_type": "task_failure"
								}),
							};
							tool_results.push(error_result);
						}
					}
				}

				// Final cancellation check after all tools processed
				if operation_cancelled.load(Ordering::SeqCst) {
					println!(
						"{}",
						"\nTool execution cancelled - preserving any completed results."
							.bright_yellow()
					);
					// Still continue with processing any completed tool results
				}

				// Display results - now handled inline during tool execution for consolidated output
				if !tool_results.is_empty() {
					// Add the accumulated tool execution time to the session total
					chat_session.session.info.total_tool_time_ms += total_tool_time_ms;

					// Check for cancellation before making another request
					if operation_cancelled.load(Ordering::SeqCst) {
						println!("{}", "\nOperation cancelled by user.".bright_yellow());
						// Do NOT add any confusing message to the session
						return Ok(());
					}

					// Create a fresh cancellation flag for the next phase
					let fresh_cancel = Arc::new(AtomicBool::new(false));

					// 🎯 SENIOR FIX: Show "Generating response..." IMMEDIATELY after tools complete
					// This provides instant feedback while tool results are being processed
					let animation_cancel_flag = fresh_cancel.clone();
					let current_cost = chat_session.session.info.total_cost;
					let animation_task = tokio::spawn(async move {
						let _ = show_loading_animation(animation_cancel_flag, current_cost).await;
					});

					// 🔍 PERFORMANCE DEBUG: Track where time is spent during tool result processing
					let processing_start = std::time::Instant::now();

					// IMPROVED APPROACH: Add tool results as proper "tool" role messages
					// This follows the standard OpenAI/Anthropic format and avoids double-serialization
					// CRITICAL FIX: Check cache threshold after EACH tool result, not after all
					let cache_manager = crate::session::cache::CacheManager::new();
					let supports_caching =
						crate::session::model_supports_caching(&chat_session.model);

					let mut cache_check_time = 0u128;
					let mut truncation_time = 0u128;

					// PERFORMANCE OPTIMIZATION: Batch process tool results with smart truncation
					// Instead of checking truncation after EVERY tool result (expensive),
					// we batch process and only truncate when necessary or at the end
					let mut accumulated_content_size = 0;
					let mut needs_truncation_check = false;

					for tool_result in &tool_results {
						// CRITICAL FIX: Extract ONLY the actual tool output, not our custom JSON wrapper
						let tool_content = if let Some(output) = tool_result.result.get("output") {
							// Extract the "output" field which contains the actual tool result
							if let Some(output_str) = output.as_str() {
								output_str.to_string()
							} else {
								// If output is not a string, serialize it
								serde_json::to_string(output).unwrap_or_default()
							}
						} else if tool_result.result.is_string() {
							// If result is already a string, use it directly
							tool_result.result.as_str().unwrap_or("").to_string()
						} else {
							// Fallback: look for common fields or use the whole result
							if let Some(error) = tool_result.result.get("error") {
								format!("Error: {}", error)
							} else {
								// Last resort: serialize the whole result
								serde_json::to_string(&tool_result.result).unwrap_or_default()
							}
						};

						// PERFORMANCE OPTIMIZATION: Check size before moving content
						let content_size = tool_content.len();
						accumulated_content_size += content_size;
						let is_large_output = content_size > 10000; // 10KB+ outputs
						let accumulated_is_large = accumulated_content_size > 50000; // 50KB+ total

						if is_large_output || accumulated_is_large {
							needs_truncation_check = true;
						}

						// Use the new add_tool_message method which handles token tracking properly
						chat_session.add_tool_message(
							&tool_content,
							&tool_result.tool_id,
							&tool_result.tool_name,
							config,
						)?;

						// CRITICAL FIX: Check auto-cache threshold IMMEDIATELY after EACH tool result
						// This ensures proper 2-marker logic and threshold checking after each tool
						let tool_message_index = chat_session.session.messages.len() - 1;
						let cache_start = std::time::Instant::now();
						if let Ok(true) = cache_manager
							.check_and_apply_auto_cache_threshold_on_tool_result(
								&mut chat_session.session,
								config,
								supports_caching,
								tool_message_index,
								role,
							) {
							log_info!("{}", format!("Auto-cache threshold reached after tool result '{}' - cache checkpoint applied before next API request.", tool_result.tool_name));
						}
						cache_check_time += cache_start.elapsed().as_millis();

						// Check truncation only for large individual tool outputs (file contents, search results, etc.)
						if is_large_output {
							let tool_truncate_cancelled = Arc::new(AtomicBool::new(false));
							let truncation_start = std::time::Instant::now();
							if let Err(e) = super::context_truncation::check_and_truncate_context(
								chat_session,
								config,
								role,
								tool_truncate_cancelled.clone(),
							)
							.await
							{
								log_info!(
									"Warning: Error during tool result truncation check: {}",
									e
								);
							}
							truncation_time += truncation_start.elapsed().as_millis();

							// Reset flags after truncation
							needs_truncation_check = false;
							accumulated_content_size = 0;
						}
					}

					// BATCH TRUNCATION: Check once after all small tool results are processed
					if needs_truncation_check {
						let batch_truncate_cancelled = Arc::new(AtomicBool::new(false));
						let truncation_start = std::time::Instant::now();
						if let Err(e) = super::context_truncation::check_and_truncate_context(
							chat_session,
							config,
							role,
							batch_truncate_cancelled.clone(),
						)
						.await
						{
							log_info!("Warning: Error during batch truncation check: {}", e);
						}
						truncation_time += truncation_start.elapsed().as_millis();
					}

					// Call the AI again with the tool results
					// Use session messages directly instead of converting

					// FINAL SAFETY CHECK: Truncate context before making follow-up API call
					// This ensures we don't send an oversized context to the API after processing
					// multiple large tool results
					let final_truncate_cancelled = Arc::new(AtomicBool::new(false));
					let final_truncation_start = std::time::Instant::now();
					if let Err(e) = super::context_truncation::check_and_truncate_context(
						chat_session,
						config,
						role,
						final_truncate_cancelled.clone(),
					)
					.await
					{
						log_info!(
							"Warning: Error during final truncation check before API call: {}",
							e
						);
					}
					truncation_time += final_truncation_start.elapsed().as_millis();

					// 🔍 PERFORMANCE DEBUG: Report processing breakdown and track processing time
					let total_processing_time = processing_start.elapsed().as_millis() as u64;

					// Add the processing time to the session total
					chat_session.session.info.total_layer_time_ms += total_processing_time;

					if config.get_log_level().is_debug_enabled() && total_processing_time > 100 {
						log_debug!(
							"🔍 Tool result processing took {}ms (cache: {}ms, truncation: {}ms)",
							total_processing_time,
							cache_check_time,
							truncation_time
						);
					}

					// Check spending threshold before making follow-up API call
					match chat_session.check_spending_threshold(config) {
						Ok(should_continue) => {
							if !should_continue {
								// User chose not to continue due to spending threshold
								fresh_cancel.store(true, Ordering::SeqCst);
								let _ = animation_task.await;
								println!("{}", "✗ Tool follow-up cancelled due to spending threshold.".bright_red());
								return Ok(());
							}
						}
						Err(e) => {
							// Error checking threshold, log warning and continue
							use colored::*;
							println!("{}: {}", "Warning: Error checking spending threshold".bright_yellow(), e);
						}
					}

					// Call OpenRouter for the follow-up response
					let model = chat_session.model.clone();
					let temperature = chat_session.temperature;

					// Make sure to include the usage parameter for every API call
					// This ensures cost information is always returned
					let follow_up_result = crate::session::chat_completion_with_provider(
						&chat_session.session.messages,
						&model,
						temperature,
						config,
					)
					.await;

					// Stop the animation and wait for completion
					fresh_cancel.store(true, Ordering::SeqCst);
					let _ = animation_task.await;

					match follow_up_result {
						Ok(response) => {
							// Store direct tool calls for efficient processing if they exist
							let has_more_tools = if let Some(ref calls) = response.tool_calls {
								!calls.is_empty()
							} else {
								// Fall back to parsing if no direct tool calls
								!crate::mcp::parse_tool_calls(&response.content).is_empty()
							};

							// Update current content for next iteration
							current_content = response.content;
							current_exchange = response.exchange;
							// CRITICAL FIX: Set the tool calls parameter for the next iteration
							current_tool_calls_param = response.tool_calls;
							let _current_finish_reason = response.finish_reason.clone();

							// Debug logging for follow-up finish_reason
							if config.get_log_level().is_debug_enabled() {
								if let Some(ref reason) = response.finish_reason {
									log_debug!("Debug: Follow-up finish_reason: {}", reason);
								}
							}

							// Check finish_reason to determine if we should continue the conversation
							let should_continue_conversation =
								match response.finish_reason.as_deref() {
									Some("tool_calls") => {
										// Model wants to make more tool calls
										if config.get_log_level().is_debug_enabled() {
											log_debug!("Debug: finish_reason is 'tool_calls', continuing conversation");
										}
										true
									}
									Some("stop") | Some("length") => {
										// Model finished normally or hit length limit
										if config.get_log_level().is_debug_enabled() {
											log_debug!(
												"Debug: finish_reason is '{}', ending conversation",
												response.finish_reason.as_deref().unwrap()
											);
										}
										false
									}
									Some(other) => {
										// Unknown finish_reason, be conservative and continue
										if config.get_log_level().is_debug_enabled() {
											log_debug!("Debug: Unknown finish_reason '{}', continuing conversation", other);
										}
										true
									}
									None => {
										// No finish_reason, check for tool calls
										if config.get_log_level().is_debug_enabled() {
											log_debug!(
												"Debug: No finish_reason, checking for tool calls"
											);
										}
										has_more_tools
									}
								};

							// Make sure the cost from this follow-up API call is properly tracked
							if let Some(usage) = &current_exchange.usage {
								// Calculate regular and cached tokens (same logic as in add_assistant_message)
								let mut regular_prompt_tokens = usage.prompt_tokens;
								let mut cached_tokens = 0;

								// Check prompt_tokens_details for cached_tokens first
								if let Some(details) = &usage.prompt_tokens_details {
									if let Some(serde_json::Value::Number(num)) =
										details.get("cached_tokens")
									{
										if let Some(num_u64) = num.as_u64() {
											cached_tokens = num_u64;
											// Adjust regular tokens to account for cached tokens
											regular_prompt_tokens =
												usage.prompt_tokens.saturating_sub(cached_tokens);
										}
									}
								}

								// Fall back to breakdown field
								if cached_tokens == 0 && usage.prompt_tokens > 0 {
									if let Some(breakdown) = &usage.breakdown {
										if let Some(serde_json::Value::Number(num)) =
											breakdown.get("cached")
										{
											if let Some(num_u64) = num.as_u64() {
												cached_tokens = num_u64;
												regular_prompt_tokens = usage
													.prompt_tokens
													.saturating_sub(cached_tokens);
											}
										}
									}
								}

								// Check for cached tokens in the base API response
								if cached_tokens == 0 && usage.prompt_tokens > 0 {
									if let Some(response) = &current_exchange.response.get("usage")
									{
										if let Some(cached) = response.get("cached_tokens") {
											if let Some(num) = cached.as_u64() {
												cached_tokens = num;
												regular_prompt_tokens = usage
													.prompt_tokens
													.saturating_sub(cached_tokens);
											}
										}
									}
								}

								// Update session token counts using the cache manager
								let cache_manager = crate::session::cache::CacheManager::new();
								cache_manager.update_token_tracking(
									&mut chat_session.session,
									regular_prompt_tokens,
									usage.completion_tokens,
									cached_tokens,
								);

								// Track API time from the follow-up exchange
								if let Some(api_time_ms) = usage.request_time_ms {
									chat_session.session.info.total_api_time_ms += api_time_ms;
								}

								// Note: Auto-cache threshold checking is now done immediately after tool results
								// are processed, not here, to ensure proper timing

								// Update cost
								if let Some(cost) = usage.cost {
									// OpenRouter credits = dollars, use the value directly
									chat_session.session.info.total_cost += cost;
									chat_session.estimated_cost =
										chat_session.session.info.total_cost;

									if config.get_log_level().is_debug_enabled() {
										println!("Debug: Adding ${:.5} from tool response API (total now: ${:.5})",
											cost, chat_session.session.info.total_cost);

										// Enhanced debug for follow-up calls
										println!("Debug: Tool response usage detail:");
										if let Ok(usage_str) = serde_json::to_string_pretty(usage) {
											println!("{}", usage_str);
										}

										// Check for cache-related fields
										if let Some(raw_usage) =
											current_exchange.response.get("usage")
										{
											println!("Debug: Raw tool response usage object:");
											if let Ok(raw_str) =
												serde_json::to_string_pretty(raw_usage)
											{
												println!("{}", raw_str);
											}

											// Look specifically for cache-related fields
											if let Some(cache_cost) = raw_usage.get("cache_cost") {
												println!("Found cache_cost field: {}", cache_cost);
											}

											if let Some(cached_cost) = raw_usage.get("cached_cost")
											{
												println!(
													"Found cached_cost field: {}",
													cached_cost
												);
											}

											if let Some(any_cache) = raw_usage.get("cached") {
												println!("Found cached field: {}", any_cache);
											}
										}
									}
								} else {
									// Try to get cost from the raw response if not in usage struct
									let cost_from_raw = current_exchange
										.response
										.get("usage")
										.and_then(|u| u.get("cost"))
										.and_then(|c| c.as_f64());

									if let Some(cost) = cost_from_raw {
										// Use the cost value directly
										chat_session.session.info.total_cost += cost;
										chat_session.estimated_cost =
											chat_session.session.info.total_cost;

										if config.get_log_level().is_debug_enabled() {
											println!("Debug: Using cost ${:.5} from raw response for tool response (total now: ${:.5})",
												cost, chat_session.session.info.total_cost);
										}
									} else {
										// Only show error if no cost data found
										println!("{}", "ERROR: OpenRouter did not provide cost data for tool response API call".bright_red());
										println!(
											"{}",
											"Make sure usage.include=true is set!".bright_red()
										);

										// Check if usage tracking was explicitly requested
										let has_usage_flag = current_exchange
											.request
											.get("usage")
											.and_then(|u| u.get("include"))
											.and_then(|i| i.as_bool())
											.unwrap_or(false);

										println!(
											"{} {}",
											"Request had usage.include flag:".bright_yellow(),
											has_usage_flag
										);

										// Dump the raw response for debugging
										if config.get_log_level().is_debug_enabled() {
											if let Ok(resp_str) = serde_json::to_string_pretty(
												&current_exchange.response,
											) {
												println!("Partial response JSON:\n{}", resp_str);
											}
										}
									}
								}
							} else {
								println!(
									"{}",
									"ERROR: No usage data for tool response API call".bright_red()
								);
							}

							// Check if there are more tools to process in the new content
							if should_continue_conversation {
								// Log if debug mode is enabled
								if config.get_log_level().is_debug_enabled() {
									println!("{}", "Debug: Continuing conversation due to finish_reason or tool calls".to_string().yellow());
								}
								// Continue processing the new content with tool calls
								continue;
							}

							// If no more tools, break out of the loop and process final content
							break;
						}
						Err(e) => {
							// Extract provider name from the model for better error messaging
							let provider_name = if let Ok((provider, _)) = crate::session::providers::ProviderFactory::parse_model(&chat_session.model) {
								provider
							} else {
								"unknown provider".to_string()
							};
							
							// IMPROVED: Show provider-aware context about the API error
							println!(
								"\n{} {}: {}",
								"✗".bright_red(),
								format!("Error calling {}", provider_name).bright_red(),
								e
							);

							// Additional context if error contains provider information
							if config.get_log_level().is_debug_enabled() {
								println!(
									"{} Model: {}",
									"Debug:".bright_black(),
									chat_session.model
								);
								println!(
									"{} Temperature: {}",
									"Debug:".bright_black(),
									chat_session.temperature
								);
							}

							return Ok(());
						}
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

	// No tool calls (or MCP not enabled), just add the response
	// Remove any function_calls blocks if they exist but weren't processed earlier
	let clean_content = remove_function_calls(&current_content);
	// When adding the final assistant message for a response that involved tool calls,
	// we've already tracked the cost and tokens in the loop above, so we pass None for exchange
	// to avoid double-counting. If this is a direct response with no tool calls, we pass the
	// original exchange to ensure costs are tracked.
	let exchange_for_final = if content == current_content {
		// This is the original content, so use the original exchange for cost tracking
		Some(current_exchange.clone())
	} else {
		// This is a modified content after tool calls, so costs were already tracked
		// in the tool response handling code, so pass None to avoid double counting
		None
	};
	chat_session.add_assistant_message(&clean_content, exchange_for_final, config, role)?;

	// Print assistant response with color
	print_assistant_response(&clean_content, config, role);

	// Display cumulative token usage - minimal output when debug is disabled
	println!();

	// Detailed output in debug mode
	log_info!(
		"{}",
		"── session usage ────────────────────────────────────────"
	);

	// Format token usage with cached tokens
	let cached = chat_session.session.info.cached_tokens;
	let prompt = chat_session.session.info.input_tokens;
	let completion = chat_session.session.info.output_tokens;
	let total = prompt + completion + cached;

	log_info!(
		"tokens: {} prompt ({} cached), {} completion, {} total, ${:.5}",
		prompt,
		cached,
		completion,
		total,
		chat_session.session.info.total_cost
	);

	// If we have cached tokens, show the savings percentage
	if cached > 0 {
		let saving_pct = (cached as f64 / (prompt + cached) as f64) * 100.0;
		log_info!(
			"cached: {:.1}% of prompt tokens ({} tokens saved)",
			saving_pct,
			cached
		);
	}

	// Show time information if available
	let total_time_ms = chat_session.session.info.total_api_time_ms
		+ chat_session.session.info.total_tool_time_ms
		+ chat_session.session.info.total_layer_time_ms;
	if total_time_ms > 0 {
		log_info!(
			"time: {} (API: {}, Tools: {}, Processing: {})",
			format_duration(total_time_ms),
			format_duration(chat_session.session.info.total_api_time_ms),
			format_duration(chat_session.session.info.total_tool_time_ms),
			format_duration(chat_session.session.info.total_layer_time_ms)
		);
	}

	println!();

	Ok(())
}
