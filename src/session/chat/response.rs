// Response processing module

use crate::config::Config;
use crate::{log_debug, log_info};
use crate::session::openrouter;
use crate::session::chat::session::ChatSession;
use crate::session::chat::markdown::{MarkdownRenderer, is_markdown_content};
use colored::Colorize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use anyhow::Result;
use std::collections::HashMap;
use serde_json;
use super::animation::show_loading_animation;
use regex::Regex;

// Function to remove function_calls blocks from content
fn remove_function_calls(content: &str) -> String {
	// Use multiple regex patterns to catch different function call formats
	let patterns = [
		r#"<(antml:)?function_calls>\s*(.+?)\s*</(antml:)?function_calls>"#,
		r#"```(json)?\s*\[?\s*\{\s*"tool_name":.+?\}\s*\]?\s*```"#,
		r#"^\s*\{\s*"tool_name":.+?\}\s*$"#
	];

	let mut result = content.to_string();

	for pattern in patterns {
		if let Ok(re) = Regex::new(pattern) {
			result = re.replace_all(&result, "").to_string();
		}
	}

	// Also remove "I'll use the X tool" phrases that often accompany function calls
	if let Ok(re) = Regex::new(r#"(?i)I'?ll use the \w+ tool[^\n]*"#) {
		result = re.replace_all(&result, "").to_string();
	}

	result.trim().to_string()
}

// Helper function to print content with optional markdown rendering
fn print_assistant_response(content: &str, config: &Config) {
	if config.openrouter.enable_markdown_rendering && is_markdown_content(content) {
		// Use markdown rendering
		let renderer = MarkdownRenderer::new();
		match renderer.render_and_print(content) {
			Ok(_) => {
				// Successfully rendered as markdown
			}
			Err(e) => {
				// Fallback to plain text if markdown rendering fails
				if config.openrouter.log_level.is_debug_enabled() {
					println!("{}: {}", "Warning: Markdown rendering failed".yellow(), e);
				}
				println!("{}", content.bright_green());
			}
		}
	} else {
		// Use plain text with color
		println!("{}", content.bright_green());
	}
}

// Structure to track tool call errors to detect loops
#[derive(Default)]
pub(crate) struct ToolErrorTracker {
	tool_errors: HashMap<String, HashMap<String, usize>>,
	max_consecutive_errors: usize,
}

impl ToolErrorTracker {
	fn new(max_errors: usize) -> Self {
		Self {
			tool_errors: HashMap::new(),
			max_consecutive_errors: max_errors,
		}
	}

	// Record an error for a tool and return true if we've hit the error threshold
	fn record_error(&mut self, tool_name: &str) -> bool {
		// Get the nested hash map for this tool, creating it if it doesn't exist
		let server_map = self.tool_errors.entry(tool_name.to_string()).or_insert_with(HashMap::new);

		// For now, we use a special key to track errors. In the future this could be server-specific
		let curr_server = "current_server".to_string();

		// Increment the error count for this tool on this server
		let count = server_map.entry(curr_server).or_insert(0);
		*count += 1;

		*count >= self.max_consecutive_errors
	}

	// Record a successful tool call, resetting the error counter for this tool from any server
	fn record_success(&mut self, tool_name: &str) {
		if let Some(server_map) = self.tool_errors.get_mut(tool_name) {
			server_map.clear(); // Clear all server counts for this tool
		}
	}

	// Get the current error count for a specific tool
	fn get_error_count(&self, tool_name: &str) -> usize {
		if let Some(server_map) = self.tool_errors.get(tool_name) {
			if let Some(count) = server_map.get("current_server") {
				return *count;
			}
		}
		0
	}

	// Not used for now, but kept for future extensibility
	#[allow(dead_code)]
	fn reset(&mut self) {
		self.tool_errors.clear();
	}
}

// Function to process response, handling tool calls recursively
pub async fn process_response(
	content: String,
	exchange: openrouter::OpenRouterExchange,
	tool_calls: Option<Vec<crate::mcp::McpToolCall>>,
	finish_reason: Option<String>,
	chat_session: &mut ChatSession,
	config: &Config,
	operation_cancelled: Arc<AtomicBool>
) -> Result<()> {
	// Check if operation has been cancelled at the very start
	if operation_cancelled.load(Ordering::SeqCst) {
		println!("{}", "\nOperation cancelled by user.".bright_yellow());
		return Ok(());
	}

	// Debug logging for finish_reason and tool calls
	if config.openrouter.log_level.is_debug_enabled() {
		if let Some(ref reason) = finish_reason {
			log_debug!("Processing response with finish_reason: {}", reason);
		}
		if let Some(ref calls) = tool_calls {
			log_debug!("Processing {} tool calls", calls.len());
		}
	}

	// First, add the user message before processing response
	let last_message = chat_session.session.messages.last();
	if last_message.map_or(true, |msg| msg.role != "user") {
		// This is an edge case - the content variable here is the AI response, not user input
		// We should have added the user message earlier in the main run_interactive_session
		println!("{}", "Warning: User message not found in session. This is unexpected.".yellow());
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

		// Check for tool calls if MCP is enabled
		if config.mcp.enabled {
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
			if config.openrouter.log_level.is_debug_enabled() && !current_tool_calls.is_empty() {
				log_debug!("Found {} tool calls in response", current_tool_calls.len());
				for (i, call) in current_tool_calls.iter().enumerate() {
					log_debug!("  Tool call {}: {} with params: {}", i+1, call.tool_name, call.parameters);
				}
			}

			if !current_tool_calls.is_empty() {
				// CRITICAL FIX: We need to add the assistant message with tool_calls PRESERVED
				// The standard add_assistant_message only stores text content, but we need
				// to preserve the tool_calls from the original API response for proper conversation flow

				// Extract the original tool_calls from the exchange response if they exist
				let original_tool_calls = current_exchange.response
					.get("choices")
					.and_then(|choices| choices.get(0))
					.and_then(|choice| choice.get("message"))
					.and_then(|message| message.get("tool_calls"))
					.cloned();

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
							if let Some(cached) = details.get("cached_tokens") {
								if let serde_json::Value::Number(num) = cached {
									if let Some(num_u64) = num.as_u64() {
										cached_tokens = num_u64;
										regular_prompt_tokens = usage.prompt_tokens.saturating_sub(cached_tokens);
									}
								}
							}
						}

						// Fall back to breakdown field
						if cached_tokens == 0 && usage.prompt_tokens > 0 {
							if let Some(breakdown) = &usage.breakdown {
								if let Some(cached) = breakdown.get("cached") {
									if let serde_json::Value::Number(num) = cached {
										if let Some(num_u64) = num.as_u64() {
											cached_tokens = num_u64;
											regular_prompt_tokens = usage.prompt_tokens.saturating_sub(cached_tokens);
										}
									}
								}
							}
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

							if config.openrouter.log_level.is_debug_enabled() {
								log_debug!("Adding ${:.5} from initial API (total now: ${:.5})",
									cost, chat_session.session.info.total_cost);
							}
						}
					}
				}

				// Log the assistant response and exchange
				let _ = crate::session::logger::log_assistant_response(&current_content);
				if let Some(ex) = &Some(current_exchange.clone()) {
					let _ = crate::session::logger::log_raw_exchange(ex);
				}

				// Display the clean content (without function calls) to the user
				let clean_content = remove_function_calls(&current_content);
				print_assistant_response(&clean_content, config);

				// Early exit if cancellation was requested
				if operation_cancelled.load(Ordering::SeqCst) {
					println!("{}", "\nOperation cancelled by user.".bright_yellow());
					// Do NOT add any confusing message to the session
					return Ok(());
				}

				// Execute all tool calls in parallel
				let mut tool_tasks = Vec::new();

				for tool_call in current_tool_calls.clone() {
					// Always print tool call execution, but with different level of detail based on debug mode
					log_info!("Executing: {} with params: {}", tool_call.tool_name, serde_json::to_string_pretty(&tool_call.parameters).unwrap_or_default());

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

					// Log the tool request with the ORIGINAL tool_id
					let _ = crate::session::logger::log_tool_request(&tool_name, &params_clone, &original_tool_id);

					let tool_id_for_task = original_tool_id.clone();
					let task = tokio::spawn(async move {
						let mut call_with_id = tool_call.clone();
						// CRITICAL: Use the original tool_id, don't change it
						call_with_id.tool_id = tool_id_for_task.clone();
						crate::mcp::execute_tool_call(&call_with_id, &config_clone).await
					});

					tool_tasks.push((tool_name, task, original_tool_id));
				}

				// Collect all results
				let mut tool_results = Vec::new();
				let mut _has_error = false;

				for (tool_name, task, tool_id) in tool_tasks {
					// Check for cancellation between tool result processing
					if operation_cancelled.load(Ordering::SeqCst) {
						println!("{}", "\nOperation cancelled by user.".bright_yellow());
						// Do NOT add any confusing message to the session
						return Ok(());
					}

					match task.await {
						Ok(result) => match result {
							Ok(res) => {
								// Tool succeeded, reset the error counter
								error_tracker.record_success(&tool_name);
								// Log the tool response
								let _ = crate::session::logger::log_tool_response(&res.result, &tool_id);
								tool_results.push(res);
							},
							Err(e) => {
								_has_error = true;
								if config.openrouter.log_level.is_debug_enabled() {
									println!("  - {}: {}", "Error executing tool".bright_red(), e);
								} else {
									// Show minimal error info without debug mode
									println!("  - {}", "Tool execution failed".bright_red());
								}

								// Track errors for this tool
								let loop_detected = error_tracker.record_error(&tool_name);
								if loop_detected {
									// Always show loop detection errors as they're critical
									println!("{}", format!("  - Loop detected: {} failed {} times in a row",
										tool_name, error_tracker.max_consecutive_errors).bright_red());

									// Add a synthetic result with error message for the AI to see
									let error_result = crate::mcp::McpToolResult {
										tool_name: tool_name.clone(),
										tool_id: tool_id.clone(),
										result: serde_json::json!({
											"error": "Tool execution failed multiple times. Please check parameters and try a different approach."
										}),
									};
									tool_results.push(error_result);
								} else {
									// Don't break the loop yet - we need 3 consecutive errors for the same tool
									if config.openrouter.log_level.is_debug_enabled() {
										println!("{}", format!("  - Tool '{}' failed {} of {} times. Continuing execution.",
											tool_name, error_tracker.get_error_count(&tool_name), error_tracker.max_consecutive_errors).yellow());
									}
								}
							},
						},
						Err(e) => {
							_has_error = true;
							if config.openrouter.log_level.is_debug_enabled() {
								println!("  - {}: {}", "Task error".bright_red(), e);
							} else {
								// Show minimal error info without debug mode
								println!("  - {}", "Internal task error".bright_red());
							}
						},
					}
				}

				// Modify process_response to check for the operation_cancelled flag immediately after extracting tool results
				// Display results
				if !tool_results.is_empty() {
					let formatted = crate::mcp::format_tool_results(&tool_results);
					println!("{}", formatted);

					// Check for cancellation before making another request
					if operation_cancelled.load(Ordering::SeqCst) {
						println!("{}", "\nOperation cancelled by user.".bright_yellow());
						// Do NOT add any confusing message to the session
						return Ok(());
					}

					// Create a fresh cancellation flag for the next phase
					let fresh_cancel = Arc::new(AtomicBool::new(false));

					// IMPROVED APPROACH: Add tool results as proper "tool" role messages
					// This follows the standard OpenAI/Anthropic format and avoids double-serialization
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

						// Create a proper tool message with tool_call_id and name
						let tool_message = crate::session::Message {
							role: "tool".to_string(),
							content: tool_content,
							timestamp: std::time::SystemTime::now()
								.duration_since(std::time::UNIX_EPOCH)
								.unwrap_or_default()
								.as_secs(),
							cached: false,
							tool_call_id: Some(tool_result.tool_id.clone()),
							name: Some(tool_result.tool_name.clone()),
							tool_calls: None,
						};

						// Add the tool message directly to the session
						chat_session.session.messages.push(tool_message);
					}

					// Call the AI again with the tool results
					// Use session messages directly instead of converting

					// Create a task to show loading animation
					let animation_cancel_flag = fresh_cancel.clone();
					let current_cost = chat_session.session.info.total_cost;
					let animation_task = tokio::spawn(async move {
						let _ = show_loading_animation(animation_cancel_flag, current_cost).await;
					});

					// Call OpenRouter for the follow-up response
					let model = chat_session.model.clone();
					let temperature = chat_session.temperature;

					// Make sure to include the usage parameter for every API call
					// This ensures cost information is always returned
					let follow_up_result = openrouter::chat_completion(
						chat_session.session.messages.clone(),
						&model,
						temperature,
						config
					).await;

					// Stop the animation
					fresh_cancel.store(true, Ordering::SeqCst);
					let _ = animation_task.await;

					match follow_up_result {
						Ok((next_content, next_exchange, next_tool_calls, next_finish_reason)) => {
							// Store direct tool calls for efficient processing if they exist
							let has_more_tools = if let Some(ref calls) = next_tool_calls {
								!calls.is_empty()
							} else {
								// Fall back to parsing if no direct tool calls
								!crate::mcp::parse_tool_calls(&next_content).is_empty()
							};

							// Update current content for next iteration
							current_content = next_content;
							current_exchange = next_exchange;
							// CRITICAL FIX: Set the tool calls parameter for the next iteration
							current_tool_calls_param = next_tool_calls;

							// Debug logging for follow-up finish_reason
							if config.openrouter.log_level.is_debug_enabled() {
								if let Some(ref reason) = next_finish_reason {
									log_debug!("Debug: Follow-up finish_reason: {}", reason);
								}
							}

							// Check finish_reason to determine if we should continue the conversation
							let should_continue_conversation = match next_finish_reason.as_deref() {
								Some("tool_calls") => {
									// Model wants to make more tool calls
									if config.openrouter.log_level.is_debug_enabled() {
										log_debug!("Debug: finish_reason is 'tool_calls', continuing conversation");
									}
									true
								}
								Some("stop") | Some("length") => {
									// Model finished normally or hit length limit
									if config.openrouter.log_level.is_debug_enabled() {
										log_debug!("Debug: finish_reason is '{}', ending conversation", next_finish_reason.as_deref().unwrap());
									}
									false
								}
								Some(other) => {
									// Unknown finish_reason, be conservative and continue
									if config.openrouter.log_level.is_debug_enabled() {
										log_debug!("Debug: Unknown finish_reason '{}', continuing conversation", other);
									}
									true
								}
								None => {
									// No finish_reason, check for tool calls
									if config.openrouter.log_level.is_debug_enabled() {
										log_debug!("Debug: No finish_reason, checking for tool calls");
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
									if let Some(cached) = details.get("cached_tokens") {
										if let serde_json::Value::Number(num) = cached {
											if let Some(num_u64) = num.as_u64() {
												cached_tokens = num_u64;
												// Adjust regular tokens to account for cached tokens
												regular_prompt_tokens = usage.prompt_tokens.saturating_sub(cached_tokens);
											}
										}
									}
								}

								// Fall back to breakdown field
								if cached_tokens == 0 && usage.prompt_tokens > 0 {
									if let Some(breakdown) = &usage.breakdown {
										if let Some(cached) = breakdown.get("cached") {
											if let serde_json::Value::Number(num) = cached {
												if let Some(num_u64) = num.as_u64() {
													cached_tokens = num_u64;
													regular_prompt_tokens = usage.prompt_tokens.saturating_sub(cached_tokens);
												}
											}
										}
									}
								}

								// Check for cached tokens in the base API response
								if cached_tokens == 0 && usage.prompt_tokens > 0 {
									if let Some(response) = &current_exchange.response.get("usage") {
										if let Some(cached) = response.get("cached_tokens") {
											if let Some(num) = cached.as_u64() {
												cached_tokens = num;
												regular_prompt_tokens = usage.prompt_tokens.saturating_sub(cached_tokens);
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

								// Check if we should automatically move the cache marker
								if let Ok(true) = chat_session.session.check_auto_cache_threshold(config) {
									log_info!("{}", "Auto-cache threshold reached during tool calls - adding cache checkpoint at last user message.");
								}

								// Update cost
								if let Some(cost) = usage.cost {
									// OpenRouter credits = dollars, use the value directly
									chat_session.session.info.total_cost += cost;
									chat_session.estimated_cost = chat_session.session.info.total_cost;

									if config.openrouter.log_level.is_debug_enabled() {
										println!("Debug: Adding ${:.5} from tool response API (total now: ${:.5})",
											cost, chat_session.session.info.total_cost);

										// Enhanced debug for follow-up calls
										println!("Debug: Tool response usage detail:");
										if let Ok(usage_str) = serde_json::to_string_pretty(usage) {
											println!("{}", usage_str);
										}

										// Check for cache-related fields
										if let Some(raw_usage) = current_exchange.response.get("usage") {
											println!("Debug: Raw tool response usage object:");
											if let Ok(raw_str) = serde_json::to_string_pretty(raw_usage) {
												println!("{}", raw_str);
											}

											// Look specifically for cache-related fields
											if let Some(cache_cost) = raw_usage.get("cache_cost") {
												println!("Found cache_cost field: {}", cache_cost);
											}

											if let Some(cached_cost) = raw_usage.get("cached_cost") {
												println!("Found cached_cost field: {}", cached_cost);
											}

											if let Some(any_cache) = raw_usage.get("cached") {
												println!("Found cached field: {}", any_cache);
											}
										}
									}
								} else {
									// Try to get cost from the raw response if not in usage struct
									let cost_from_raw = current_exchange.response.get("usage")
										.and_then(|u| u.get("cost"))
										.and_then(|c| c.as_f64());

									if let Some(cost) = cost_from_raw {
										// Use the cost value directly
										chat_session.session.info.total_cost += cost;
										chat_session.estimated_cost = chat_session.session.info.total_cost;

										if config.openrouter.log_level.is_debug_enabled() {
											println!("Debug: Using cost ${:.5} from raw response for tool response (total now: ${:.5})",
												cost, chat_session.session.info.total_cost);
										}
									} else {
										// Only show error if no cost data found
										println!("{}", "ERROR: OpenRouter did not provide cost data for tool response API call".bright_red());
										println!("{}", "Make sure usage.include=true is set!".bright_red());

										// Check if usage tracking was explicitly requested
										let has_usage_flag = current_exchange.request.get("usage")
											.and_then(|u| u.get("include"))
											.and_then(|i| i.as_bool())
											.unwrap_or(false);

										println!("{} {}", "Request had usage.include flag:".bright_yellow(), has_usage_flag);

										// Dump the raw response for debugging
										if config.openrouter.log_level.is_debug_enabled() {
											if let Ok(resp_str) = serde_json::to_string_pretty(&current_exchange.response) {
												println!("Partial response JSON:\n{}", resp_str);
											}
										}
									}
								}
							} else {
								println!("{}", "ERROR: No usage data for tool response API call".bright_red());
							}

							// Check if there are more tools to process in the new content
							if should_continue_conversation {
								// Log if debug mode is enabled
								if config.openrouter.log_level.is_debug_enabled() {
									println!("{}", format!("Debug: Continuing conversation due to finish_reason or tool calls").yellow());
								}
								// Continue processing the new content with tool calls
								continue;
							}

							// If no more tools, break out of the loop and process final content
							break;
						},
						Err(e) => {
							// Critical errors should always be shown
							println!("\n{}: {}", "Error calling OpenRouter".bright_red(), e);
							return Ok(());
						}
					}
				} else {
					// No tool results - check if there were more tools to execute directly
					let more_tools = crate::mcp::parse_tool_calls(&current_content);
					if !more_tools.is_empty() {
						// Log if debug mode is enabled
						if config.openrouter.log_level.is_debug_enabled() {
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
	chat_session.add_assistant_message(&clean_content, exchange_for_final, config)?;

	// Print assistant response with color
	print_assistant_response(&clean_content, config);

	// Display cumulative token usage - minimal output when debug is disabled
	println!();

	// Detailed output in debug mode
	log_info!("{}", "── session usage ────────────────────────────────────────");

	// Format token usage with cached tokens
	let cached = chat_session.session.info.cached_tokens;
	let prompt = chat_session.session.info.input_tokens;
	let completion = chat_session.session.info.output_tokens;
	let total = prompt + completion + cached;

	log_info!("tokens: {} prompt ({} cached), {} completion, {} total, ${:.5}",
		prompt,
		cached,
		completion,
		total,
		chat_session.session.info.total_cost);

	// If we have cached tokens, show the savings percentage
	if cached > 0 {
		let saving_pct = (cached as f64 / (prompt + cached) as f64) * 100.0;
		log_info!("cached: {:.1}% of prompt tokens ({} tokens saved)",
			saving_pct,
			cached);
	}
	println!();

	Ok(())
}
