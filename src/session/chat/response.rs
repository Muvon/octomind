// Response processing module

use crate::config::Config;
use crate::session::openrouter;
use crate::session::mcp;
use crate::session::chat::session::ChatSession;
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
	let re = Regex::new(r"<(antml:)?function_calls>\s*(.+?)\s*</(antml:)?function_calls>").unwrap_or_else(|_| Regex::new(r"<function_calls>.+</function_calls>").unwrap());
	re.replace_all(content, "").trim().to_string()
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

// Process a response, handling tool calls recursively
pub async fn process_response(
	content: String,
	exchange: openrouter::OpenRouterExchange,
	chat_session: &mut ChatSession,
	config: &Config,
	operation_cancelled: Arc<AtomicBool>
) -> Result<()> {
	// Check if operation has been cancelled at the very start
	if operation_cancelled.load(Ordering::SeqCst) {
		println!("{}", "\nOperation cancelled by user.".bright_yellow());
		return Ok(());
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

	loop {
		// Check for cancellation at the start of each loop iteration
		if operation_cancelled.load(Ordering::SeqCst) {
			println!("{}", "\nOperation cancelled by user.".bright_yellow());
			return Ok(());
		}

		// Check for tool calls if MCP is enabled
		if config.mcp.enabled {
			let tool_calls = mcp::parse_tool_calls(&current_content);

			if !tool_calls.is_empty() {
				// Add assistant message with the response but strip the function_calls block
				let clean_content = remove_function_calls(&current_content);
				chat_session.add_assistant_message(&clean_content, Some(current_exchange.clone()), config)?;

				// Display assistant response with tool calls removed from display
				println!("\n{}", clean_content.bright_green());

				// Early exit if cancellation was requested
				if operation_cancelled.load(Ordering::SeqCst) {
					println!("{}", "\nOperation cancelled by user.".bright_yellow());
					// Do NOT add any confusing message to the session
					return Ok(());
				}

				// Execute all tool calls in parallel
				let mut tool_tasks = Vec::new();

				for tool_call in tool_calls.clone() {
					// Print colorful tool execution message if debug is enabled
					if config.openrouter.debug {
						println!("  - Executing: {}", tool_call.tool_name.yellow());
					}

					// Clone tool_name separately for tool task tracking
					let tool_name = tool_call.tool_name.clone();

					// Execute in a tokio task
					let config_clone = config.clone();
					let task = tokio::spawn(async move {
						mcp::execute_tool_call(&tool_call, &config_clone).await
					});

					tool_tasks.push((tool_name, task));
				}

				// Collect all results
				let mut tool_results = Vec::new();
				let mut _has_error = false;

				for (tool_name, task) in tool_tasks {
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
								tool_results.push(res);
							},
							Err(e) => {
								_has_error = true;
								if config.openrouter.debug {
									println!("  - {}: {}", "Error executing tool".bright_red(), e);
								}

								// Track errors for this tool
								let loop_detected = error_tracker.record_error(&tool_name);
								if loop_detected {
									// Always show loop detection errors as they're critical
									println!("{}", format!("  - Loop detected: {} failed {} times in a row. Breaking out of tool call loop.",
										tool_name, error_tracker.max_consecutive_errors).bright_red());

									// Add a synthetic result with error message for the AI to see
									let error_result = mcp::McpToolResult {
										tool_name: tool_name.clone(),
										result: serde_json::json!({
											"error": "Tool execution failed multiple times. Please check parameters and try a different approach."
										}),
									};
									tool_results.push(error_result);
								} else {
									// Don't break the loop yet - we need 3 consecutive errors for the same tool
									if config.openrouter.debug {
										println!("{}", format!("  - Tool '{}' failed {} of {} times. Continuing execution.",
											tool_name, error_tracker.get_error_count(&tool_name), error_tracker.max_consecutive_errors).yellow());
									}
								}
							},
						},
						Err(e) => {
							_has_error = true;
							if config.openrouter.debug {
								println!("  - {}: {}", "Task error".bright_red(), e);
							}
						},
					}
				}

				// Modify process_response to check for the operation_cancelled flag immediately after extracting tool results
				// Display results
				if !tool_results.is_empty() {
					let formatted = mcp::format_tool_results(&tool_results);
					println!("{}", formatted);

					// Check for cancellation before making another request
					if operation_cancelled.load(Ordering::SeqCst) {
						println!("{}", "\nOperation cancelled by user.".bright_yellow());
						// Do NOT add any confusing message to the session
						return Ok(());
					}

					// Create a fresh cancellation flag for the next phase
					let fresh_cancel = Arc::new(AtomicBool::new(false));

					// Create user message with tool results
					let tool_results_message = serde_json::to_string(&tool_results)
						.unwrap_or_else(|_| "[]".to_string());

					let tool_message = format!("<fnr>\n{}\n</fnr>",
						tool_results_message);

					chat_session.add_user_message(&tool_message)?;

					// Call the AI again with the tool results
					let or_messages = openrouter::convert_messages(&chat_session.session.messages);

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
						or_messages,
						&model,
						temperature,
						config
					).await;

					// Stop the animation
					fresh_cancel.store(true, Ordering::SeqCst);
					let _ = animation_task.await;

					match follow_up_result {
						Ok((next_content, next_exchange)) => {
							// Update current content for next iteration
							current_content = next_content;
							current_exchange = next_exchange;

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

								// Update session token counts
								chat_session.session.info.input_tokens += regular_prompt_tokens;
								chat_session.session.info.output_tokens += usage.completion_tokens;
								chat_session.session.info.cached_tokens += cached_tokens;

								// Update cost
								if let Some(cost) = usage.cost {
									// OpenRouter credits = dollars, use the value directly
									chat_session.session.info.total_cost += cost;
									chat_session.estimated_cost = chat_session.session.info.total_cost;

									if config.openrouter.debug {
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

										if config.openrouter.debug {
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
										if config.openrouter.debug {
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
							let more_tools = mcp::parse_tool_calls(&current_content);
							if !more_tools.is_empty() {
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
					// No tool results - check if there were more tools to execute
					let more_tools = mcp::parse_tool_calls(&current_content);
					if !more_tools.is_empty() {
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
	println!("\n{}", clean_content.bright_green());

	// Display cumulative token usage
	println!();
	println!("{}", "── session usage ────────────────────────────────────────".bright_cyan());

	// Format token usage with cached tokens
	let cached = chat_session.session.info.cached_tokens;
	let prompt = chat_session.session.info.input_tokens;
	let completion = chat_session.session.info.output_tokens;
	let total = prompt + completion + cached;

	println!("{} {} prompt ({} cached), {} completion, {} total, ${:.5}",
		"tokens:".bright_blue(),
		prompt,
		cached,
		completion,
		total,
		chat_session.session.info.total_cost);

	// If we have cached tokens, show the savings percentage
	if cached > 0 {
		let saving_pct = (cached as f64 / (prompt + cached) as f64) * 100.0;
		println!("{} {:.1}% of prompt tokens ({} tokens saved)",
			"cached:".bright_green(),
			saving_pct,
			cached);
	}

	println!();

	Ok(())
}
