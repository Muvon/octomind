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

// Tool execution module - handles parallel tool execution, display, and error handling

use crate::config::Config;
use crate::session::chat::session::ChatSession;
use crate::session::chat::ToolProcessor;
use crate::{log_debug, log_info};
use anyhow::Result;
use colored::Colorize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

// Execute all tool calls in parallel and collect results
pub async fn execute_tools_parallel(
	current_tool_calls: Vec<crate::mcp::McpToolCall>,
	chat_session: &mut ChatSession,
	config: &Config,
	tool_processor: &mut ToolProcessor,
	operation_cancelled: Arc<AtomicBool>,
) -> Result<(Vec<crate::mcp::McpToolResult>, u64)> {
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
									format!("⚠ Tool '{}' completed with error: {}", tool_name, e)
										.bright_yellow()
								);
							}
						},
						Err(_) => {
							println!(
								"{}",
								format!("⚠ Tool '{}' task error during grace period", tool_name)
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
					tool_processor.error_tracker.record_success(&tool_name);

					// Display the complete tool execution with consolidated info
					display_tool_success(
						&stored_tool_call,
						&res,
						&tool_name,
						tool_time_ms,
						config,
						&chat_session.session.info.name,
						&tool_id,
					);

					tool_results.push(res);
					// Accumulate tool execution time
					total_tool_time_ms += tool_time_ms;
				}
				Err(e) => {
					_has_error = true;

					// Check if this is a user-declined large output error
					if e.to_string().contains("LARGE_OUTPUT_DECLINED_BY_USER") {
						handle_declined_output(&tool_name, &tool_id, chat_session);
						continue;
					}

					// Display error in consolidated format for other errors
					display_tool_error(&stored_tool_call, &tool_name, &e);

					// Track errors for this tool
					let loop_detected = tool_processor.error_tracker.record_error(&tool_name);

					if loop_detected {
						// Always show loop detection warning since it's critical
						println!("{}", format!("⚠ Warning: {} failed {} times in a row - AI should try a different approach",
							tool_name, tool_processor.error_tracker.max_consecutive_errors()).bright_yellow());

						// Add a detailed error result for loop detection
						let loop_error_result = crate::mcp::McpToolResult {
							tool_name: tool_name.clone(),
							tool_id: tool_id.clone(),
							result: serde_json::json!({
								"error": format!("LOOP DETECTED: Tool '{}' failed {} consecutive times. Last error: {}. Please try a completely different approach or ask the user for guidance.", tool_name, tool_processor.error_tracker.max_consecutive_errors(), e),
								"tool_name": tool_name,
								"consecutive_failures": tool_processor.error_tracker.max_consecutive_errors(),
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
								"attempt": tool_processor.error_tracker.get_error_count(&tool_name),
								"max_attempts": tool_processor.error_tracker.max_consecutive_errors()
							}),
						};
						tool_results.push(error_result);

						if config.get_log_level().is_info_enabled() {
							log_info!(
								"Tool '{}' failed {} of {} times. Adding error to context.",
								tool_name,
								tool_processor.error_tracker.get_error_count(&tool_name),
								tool_processor.error_tracker.max_consecutive_errors()
							);
						}
					}
				}
			},
			Err(e) => {
				_has_error = true;

				// Check if this is a user-declined large output error (can occur at task level too)
				if e.to_string().contains("LARGE_OUTPUT_DECLINED_BY_USER") {
					handle_declined_output_task(&tool_name, &tool_id, chat_session);
					continue;
				}

				// Display task error in consolidated format for other errors
				display_tool_error(&stored_tool_call, &tool_name, &anyhow::anyhow!("{}", e));

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

	Ok((tool_results, total_tool_time_ms))
}

// Display successful tool execution (after execution - no header, output based on log level)
fn display_tool_success(
	_stored_tool_call: &Option<crate::mcp::McpToolCall>,
	res: &crate::mcp::McpToolResult,
	tool_name: &str,
	tool_time_ms: u64,
	config: &Config,
	session_name: &str,
	tool_id: &str,
) {
	// Show the actual tool output based on log level
	if config.get_log_level().is_info_enabled() || config.get_log_level().is_debug_enabled() {
		if let Some(output) = res.result.get("output") {
			if let Some(output_str) = output.as_str() {
				if !output_str.trim().is_empty() {
					if config.get_log_level().is_debug_enabled() {
						// Debug mode: Show full output
						println!("{}", output_str);
					} else {
						// Info mode: Show smart output (with some reasonable limits)
						display_tool_output_smart(output_str);
					}
				}
			}
		}
	}
	// None mode: No output shown (as requested)

	// Always show completion status with timing
	println!("✓ Tool '{}' completed in {}ms", tool_name, tool_time_ms);
	println!("──────────────────");

	// Log the tool response with session name
	let _ = crate::session::logger::log_tool_result(session_name, tool_id, &res.result);
}

// Display tool output in smart format (for info mode)
fn display_tool_output_smart(output_str: &str) {
	let lines: Vec<&str> = output_str.lines().collect();

	if lines.len() <= 20 && output_str.chars().count() <= 2000 {
		// Small output: show as-is
		println!("{}", output_str);
	} else if lines.len() > 20 {
		// Many lines: show first 15 lines + summary
		for line in lines.iter().take(15) {
			println!("{}", line);
		}
		println!("... [{} more lines]", lines.len().saturating_sub(15));
	} else {
		// Long single line or few long lines: truncate
		let truncated: String = output_str.chars().take(1997).collect();
		println!("{}...", truncated);
	}
}

// Display tool error in consolidated format
fn display_tool_error(
	stored_tool_call: &Option<crate::mcp::McpToolCall>,
	tool_name: &str,
	error: &anyhow::Error,
) {
	if let Some(tool_call) = stored_tool_call {
		let category = crate::mcp::guess_tool_category(&tool_call.tool_name);
		let title = format!(
			" {} | {} ",
			tool_call.tool_name.bright_cyan(),
			category.bright_blue()
		);
		let separator_length = 70.max(title.len() + 4);
		let dashes = "─".repeat(separator_length - title.len());
		let separator = format!("──{}{}──", title, dashes.dimmed());
		println!("{}", separator);

		// Show error without parameters (since header already shown before execution)
	}

	// Show error status
	println!("✗ Tool '{}' failed: {}", tool_name, error);
}

// Handle user-declined large output
fn handle_declined_output(tool_name: &str, tool_id: &str, chat_session: &mut ChatSession) {
	println!(
		"⚠ Tool '{}' output declined by user - removing tool call from conversation",
		tool_name
	);

	// CRITICAL FIX: Remove the tool_use block from the assistant message
	// to prevent "tool_use ids found without tool_result blocks" error
	if let Some(last_msg) = chat_session.session.messages.last_mut() {
		if last_msg.role == "assistant" {
			if let Some(tool_calls_value) = &last_msg.tool_calls {
				// Parse the tool_calls and remove the declined one
				if let Ok(mut tool_calls_array) =
					serde_json::from_value::<Vec<serde_json::Value>>(tool_calls_value.clone())
				{
					// Remove the tool call with matching ID
					tool_calls_array
						.retain(|tc| tc.get("id").and_then(|id| id.as_str()) != Some(tool_id));

					// Update the assistant message
					if tool_calls_array.is_empty() {
						// No more tool calls, remove the tool_calls field entirely
						last_msg.tool_calls = None;
						log_debug!("Removed all tool calls from assistant message after user declined large output");
					} else {
						// Update with remaining tool calls
						last_msg.tool_calls =
							Some(serde_json::to_value(tool_calls_array).unwrap_or_default());
						log_debug!(
							"Removed declined tool call '{}' from assistant message",
							tool_id
						);
					}
				}
			}
		}
	}
}

// Handle user-declined large output at task level
fn handle_declined_output_task(tool_name: &str, tool_id: &str, chat_session: &mut ChatSession) {
	println!(
		"⚠ Tool '{}' task output declined by user - removing tool call from conversation",
		tool_name
	);

	// CRITICAL FIX: Remove the tool_use block from the assistant message
	// to prevent "tool_use ids found without tool_result blocks" error
	if let Some(last_msg) = chat_session.session.messages.last_mut() {
		if last_msg.role == "assistant" {
			if let Some(tool_calls_value) = &last_msg.tool_calls {
				// Parse the tool_calls and remove the declined one
				if let Ok(mut tool_calls_array) =
					serde_json::from_value::<Vec<serde_json::Value>>(tool_calls_value.clone())
				{
					// Remove the tool call with matching ID
					tool_calls_array
						.retain(|tc| tc.get("id").and_then(|id| id.as_str()) != Some(tool_id));

					// Update the assistant message
					if tool_calls_array.is_empty() {
						// No more tool calls, remove the tool_calls field entirely
						last_msg.tool_calls = None;
						log_debug!("Removed all tool calls from assistant message after user declined large task output");
					} else {
						// Update with remaining tool calls
						last_msg.tool_calls =
							Some(serde_json::to_value(tool_calls_array).unwrap_or_default());
						log_debug!(
							"Removed declined tool call '{}' from assistant message (task error)",
							tool_id
						);
					}
				}
			}
		}
	}
}
