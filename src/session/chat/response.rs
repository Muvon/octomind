// Response processing module

use crate::config::Config;
use crate::session::openrouter;
use crate::session::mcp;
use crate::session::chat::session::ChatSession;
use colored::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use anyhow::Result;
use std::collections::HashMap;
use serde_json;
use super::animation::show_loading_animation;

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
	let mut current_content = content;
	let mut current_exchange = exchange;
	
	loop {
		// Check for tool calls if MCP is enabled
		if config.mcp.enabled {
			let tool_calls = mcp::parse_tool_calls(&current_content);

			if !tool_calls.is_empty() {
				// Add assistant message with the initial response containing tool calls
				chat_session.add_assistant_message(&current_content, Some(current_exchange.clone()), config)?;

				// Display assistant response with tool calls
				println!("\n{}", current_content.bright_green());
				
				// Early exit if cancellation was requested
				if operation_cancelled.load(Ordering::SeqCst) {
					println!("{}", "\nOperation cancelled by user.".bright_yellow());
					// Do NOT add any confusing message to the session
					return Ok(());
				}

				// Execute all tool calls in parallel
				let mut tool_tasks = Vec::new();

				for tool_call in tool_calls.clone() {
					// Print colorful tool execution message
					println!("  - Executing: {}", tool_call.tool_name.yellow());

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
								println!("  - {}: {}", "Error executing tool".bright_red(), e);
								
								// Track errors for this tool
								let loop_detected = error_tracker.record_error(&tool_name);
								if loop_detected {
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
									println!("{}", format!("  - Tool '{}' failed {} of {} times. Continuing execution.", 
										tool_name, error_tracker.get_error_count(&tool_name), error_tracker.max_consecutive_errors).yellow());
								}
							},
						},
						Err(e) => {
							_has_error = true;
							println!("  - {}: {}", "Task error".bright_red(), e);
						},
					}
				}

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
					let animation_task = tokio::spawn(async move {
						let _ = show_loading_animation(animation_cancel_flag).await;
					});

					// Call OpenRouter for the follow-up response
					let model = chat_session.model.clone();
					let temperature = chat_session.temperature;
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
	chat_session.add_assistant_message(&current_content, Some(current_exchange.clone()), config)?;

	// Print assistant response with color
	println!("\n{}", current_content.bright_green());

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