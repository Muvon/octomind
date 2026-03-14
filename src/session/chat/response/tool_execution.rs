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
// Unified interface for both main sessions and layers

use crate::config::Config;
use crate::session::chat::session::ChatSession;
use crate::session::chat::ToolProcessor;
use crate::session::output::OutputMode;
use crate::{log_debug, log_info};
use anyhow::Result;
use colored::Colorize;
use std::io::IsTerminal;

/// Context for tool execution - can be either main session or layer context
pub enum ToolExecutionContext<'a> {
	/// Main session context with full session access
	MainSession {
		chat_session: &'a mut ChatSession,
		tool_processor: &'a mut ToolProcessor,
	},
	/// Layer context with layer-specific configuration
	Layer {
		session_name: String,
		layer_config: &'a crate::session::layers::LayerConfig,
		layer_name: String,
	},
}

impl ToolExecutionContext<'_> {
	/// Get session name for logging
	pub fn session_name(&self) -> &str {
		match self {
			ToolExecutionContext::MainSession { chat_session, .. } => {
				&chat_session.session.info.name
			}
			ToolExecutionContext::Layer { session_name, .. } => session_name,
		}
	}

	/// Get execution context for display (None for main session, Some(context) for layers/agents)
	pub fn execution_context(&self) -> Option<String> {
		match self {
			ToolExecutionContext::MainSession { .. } => None, // No suffix for main session
			ToolExecutionContext::Layer { layer_name, .. } => Some(layer_name.clone()),
		}
	}

	/// Check if tool is allowed in this context
	pub fn is_tool_allowed(&self, tool_name: &str) -> bool {
		match self {
			ToolExecutionContext::MainSession { .. } => true, // Main session allows all tools
			ToolExecutionContext::Layer { layer_config, .. } => {
				// Get the server name for this tool to support server patterns
				let server_name = crate::mcp::tool_map::get_tool_server_name(tool_name)
					.unwrap_or_else(|| "unknown".to_string());

				// Use the sophisticated pattern-based validation from LayerMcpConfig
				layer_config.mcp.is_tool_allowed(tool_name, &server_name)
			}
		}
	}

	/// Get error tracker (if available)
	pub fn error_tracker(
		&mut self,
	) -> Option<&mut crate::session::chat::tool_error_tracker::ToolErrorTracker> {
		match self {
			ToolExecutionContext::MainSession { tool_processor, .. } => {
				Some(&mut tool_processor.error_tracker)
			}
			ToolExecutionContext::Layer { .. } => None, // Layers don't have error tracking yet
		}
	}

	/// Increment tool call counter
	pub fn increment_tool_calls(&mut self) {
		if let ToolExecutionContext::MainSession { chat_session, .. } = self {
			chat_session.session.info.tool_calls += 1;
		}
	}

	/// Handle declined output by removing tool call from conversation
	pub fn handle_declined_output(&mut self, tool_id: &str) {
		if let ToolExecutionContext::MainSession { chat_session, .. } = self {
			handle_declined_in_session(tool_id, chat_session);
		}
		// For layers, we don't need to modify conversation history
	}
}

/// Execute all tool calls in parallel for given execution context
pub async fn execute_tools_in_context(
	current_tool_calls: Vec<crate::mcp::McpToolCall>,
	context: &mut ToolExecutionContext<'_>,
	config: &Config,
	operation_cancelled: Option<tokio::sync::watch::Receiver<bool>>,
	mode: OutputMode,
) -> Result<(Vec<crate::mcp::McpToolResult>, u64)> {
	// Filter tools based on context permissions
	let allowed_tool_calls: Vec<_> = current_tool_calls
		.into_iter()
		.filter(|tool_call| {
			if context.is_tool_allowed(&tool_call.tool_name) {
				true
			} else {
				if !mode.should_suppress_cli_output() {
					println!(
						"{} {} {}",
						"Tool".red(),
						tool_call.tool_name,
						"not allowed in this context".red()
					);
				}
				false
			}
		})
		.collect();

	if allowed_tool_calls.is_empty() {
		return Ok((Vec::new(), 0));
	}

	execute_tools_with_context(
		allowed_tool_calls,
		context,
		config,
		operation_cancelled,
		mode,
	)
	.await
}

// Execute all tool calls in parallel and collect results (legacy interface for main session)
pub async fn execute_tools_parallel(
	current_tool_calls: Vec<crate::mcp::McpToolCall>,
	chat_session: &mut ChatSession,
	config: &Config,
	tool_processor: &mut ToolProcessor,
	operation_cancelled: tokio::sync::watch::Receiver<bool>,
	mode: OutputMode,
) -> Result<(Vec<crate::mcp::McpToolResult>, u64)> {
	let mut context = ToolExecutionContext::MainSession {
		chat_session,
		tool_processor,
	};

	let result = execute_tools_in_context(
		current_tool_calls.clone(),
		&mut context,
		config,
		Some(operation_cancelled),
		mode,
	)
	.await;

	result
}

// Implementation that works with execution context
async fn execute_tools_with_context(
	current_tool_calls: Vec<crate::mcp::McpToolCall>,
	context: &mut ToolExecutionContext<'_>,
	config: &Config,
	operation_cancelled: Option<tokio::sync::watch::Receiver<bool>>,
	mode: OutputMode,
) -> Result<(Vec<crate::mcp::McpToolResult>, u64)> {
	let mut tool_tasks = Vec::new();
	let is_single_tool = current_tool_calls.len() == 1;

	for (index, tool_call) in current_tool_calls.clone().iter().enumerate() {
		// Increment tool call counter
		context.increment_tool_calls();

		// CRITICAL FIX: Use the EXACT tool_id from the original API response
		// Don't generate a new UUID - use the one from the original tool_calls
		let original_tool_id = tool_call.tool_id.clone();

		// Clone tool_name separately for tool task tracking
		let tool_name = tool_call.tool_name.clone();
		let tool_index = index + 1; // 1-based index for display

		// Execute in a tokio task
		let config_clone = config.clone();
		let params_clone = tool_call.parameters.clone();

		// Log the tool request with the session name and ORIGINAL tool_id
		let _ = crate::session::logger::log_tool_call(
			context.session_name(),
			&tool_name,
			&original_tool_id,
			&params_clone,
		);

		let tool_id_for_task = original_tool_id.clone();
		let tool_call_clone = tool_call.clone(); // Clone for async move
		let cancel_token_for_task = operation_cancelled.clone(); // Pass cancellation token

		// Create the appropriate execution task based on context
		let task = match context {
			ToolExecutionContext::MainSession { .. } => {
				tokio::spawn(async move {
					let mut call_with_id = tool_call_clone.clone();
					// CRITICAL: Use the original tool_id, don't change it
					call_with_id.tool_id = tool_id_for_task.clone();
					crate::mcp::execute_tool_call(
						&call_with_id,
						&config_clone,
						cancel_token_for_task,
					)
					.await
				})
			}
			ToolExecutionContext::Layer { layer_config, .. } => {
				let layer_config_clone = layer_config.clone();
				tokio::spawn(async move {
					let mut call_with_id = tool_call_clone.clone();
					// CRITICAL: Use the original tool_id, don't change it
					call_with_id.tool_id = tool_id_for_task.clone();
					crate::mcp::execute_layer_tool_call(
						&call_with_id,
						&config_clone,
						&layer_config_clone,
						cancel_token_for_task, // FIXED: Pass cancellation token
					)
					.await
				})
			}
		};

		tool_tasks.push((tool_name, task, original_tool_id, tool_index));
	}

	// FIXED: Proper parallel awaiting with immediate cancellation support
	let mut tool_results = Vec::new();
	let mut _has_error = false;
	let mut total_tool_time_ms = 0; // Track cumulative tool execution time

	// Extract task info for later use
	let task_info: Vec<(String, String, usize)> = tool_tasks
		.iter()
		.map(|(tool_name, _, tool_id, tool_index)| {
			(tool_name.clone(), tool_id.clone(), *tool_index)
		})
		.collect();

	// Extract just the tasks for parallel execution
	let tasks: Vec<_> = tool_tasks.into_iter().map(|(_, task, _, _)| task).collect();

	// CRITICAL: Do NOT restart animation here!
	// Animation lifecycle is managed by api_executor.rs:
	// - api_executor starts animation before API call
	// - process_response stops animation before tool output
	// - Tool execution should NOT restart animation (output already displaying)
	// Restarting here causes ghost spinner that continues after output

	// Use tokio::select! to run tasks with cancellation support.
	let all_tasks = futures::future::join_all(tasks);
	tokio::pin!(all_tasks);
	let task_results: Vec<_> = tokio::select! {
		results = &mut all_tasks => results,
		_ = async {
			if let Some(ref cancel_rx) = operation_cancelled {
				let mut cancel_rx_clone = cancel_rx.clone();
				while !*cancel_rx_clone.borrow() {
					if cancel_rx_clone.changed().await.is_err() {
						break;
					}
				}
			} else {
				std::future::pending::<()>().await;
			}
		} => {
			// Animation already stopped by process_response - no action needed

			// Cancellation occurred - provide immediate feedback
			use colored::*;
			if !mode.should_suppress_cli_output() {
				println!(
					"{}",
					"🛑 All tool execution cancelled - returning to input".bright_yellow()
				);
			}

			// Show cancellation message for each tool
			if !mode.should_suppress_cli_output() {
				for (tool_name, _, _) in task_info {
					println!(
						"{}",
						format!("🛑 Tool '{}' cancelled - server preserved", tool_name).bright_yellow()
					);
				}
			}

			return Ok((Vec::new(), total_tool_time_ms));
		}
	};

	// Animation already stopped by process_response - no action needed

	// All tasks completed before cancellation
	for ((tool_name, tool_id, tool_index), task_result) in task_info.into_iter().zip(task_results) {
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

		match task_result {
			Ok(result) => match result {
				Ok((res, tool_time_ms)) => {
					// CRITICAL MCP PROTOCOL FIX: Check if result is actually an error
					if res.is_error() {
						// This is an MCP error result (isError: true) - treat as error
						_has_error = true;

						// Record error in error tracker
						if let Some(error_tracker) = context.error_tracker() {
							let has_hit_threshold = error_tracker.record_error(&tool_name);
							if has_hit_threshold {
								crate::log_debug!("Tool '{}' has hit error threshold", tool_name);
							}
						}

						// Extract error message from MCP result
						let error_content = crate::mcp::extract_mcp_content(&res.result);
						let error = anyhow::anyhow!("{}", error_content);

						// Display as error, not success
						display_tool_error(
							&stored_tool_call,
							&tool_name,
							&error,
							tool_index,
							config,
							mode,
							context.execution_context(),
						)
						.await;

						// Still push the result for conversation continuity (AI needs to see the error)
						tool_results.push(res.clone());

						// Accumulate tool execution time even for errors
						total_tool_time_ms += tool_time_ms;
					} else {
						// This is a genuine success (isError: false or missing)
						// Tool succeeded, reset the error counter (if available)
						if let Some(error_tracker) = context.error_tracker() {
							error_tracker.record_success(&tool_name);
						}

						// Display the complete tool execution with consolidated info
						let display_params = ToolDisplayParams {
							stored_tool_call: &stored_tool_call,
							tool_name: &tool_name,
							tool_id: &tool_id,
							tool_index,
							is_single_tool,
						};
						display_tool_success(
							display_params,
							&res,
							tool_time_ms,
							config,
							mode,
							context.session_name(),
							context.execution_context(),
						)
						.await;

						tool_results.push(res.clone());

						// Accumulate tool execution time
						total_tool_time_ms += tool_time_ms;
					}
				}
				Err(e) => {
					_has_error = true;

					// Check if this is a user-declined large output error
					if e.to_string().contains("LARGE_OUTPUT_DECLINED_BY_USER") {
						context.handle_declined_output(&tool_id);
						continue;
					}

					// Display error in consolidated format for other errors
					display_tool_error(
						&stored_tool_call,
						&tool_name,
						&e,
						tool_index,
						config,
						mode,
						context.execution_context(),
					)
					.await;

					// Track errors for this tool (if error tracking is available)
					let loop_detected = if let Some(error_tracker) = context.error_tracker() {
						error_tracker.record_error(&tool_name)
					} else {
						false
					};

					if loop_detected {
						// Always show loop detection warning since it's critical
						if let Some(error_tracker) = context.error_tracker() {
							if !mode.should_suppress_cli_output() {
								println!("{}", format!("⚠ Warning: {} failed {} times in a row - AI should try a different approach",
								tool_name, error_tracker.max_consecutive_errors()).bright_yellow());
							}

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
						}
					} else {
						// Regular error - add normal error result
						let error_result = if let Some(error_tracker) = context.error_tracker() {
							crate::mcp::McpToolResult {
								tool_name: tool_name.clone(),
								tool_id: tool_id.clone(),
								result: serde_json::json!({
									"error": format!("Tool execution failed: {}", e),
									"tool_name": tool_name,
									"attempt": error_tracker.get_error_count(&tool_name),
									"max_attempts": error_tracker.max_consecutive_errors()
								}),
							}
						} else {
							// For layers without error tracking
							crate::mcp::McpToolResult {
								tool_name: tool_name.clone(),
								tool_id: tool_id.clone(),
								result: serde_json::json!({
									"error": format!("Tool execution failed: {}", e),
									"tool_name": tool_name,
								}),
							}
						};
						tool_results.push(error_result);

						if let Some(error_tracker) = context.error_tracker() {
							log_info!(
								"Tool '{}' failed {} of {} times. Adding error to context.",
								tool_name,
								error_tracker.get_error_count(&tool_name),
								error_tracker.max_consecutive_errors()
							);
						}
					}
				}
			},
			Err(e) => {
				_has_error = true;

				// Check if this is a user-declined large output error (can occur at task level too)
				if e.to_string().contains("LARGE_OUTPUT_DECLINED_BY_USER") {
					context.handle_declined_output(&tool_id);
					continue;
				}

				// Display task error in consolidated format for other errors
				display_tool_error(
					&stored_tool_call,
					&tool_name,
					&anyhow::anyhow!("{}", e),
					tool_index,
					config,
					mode,
					context.execution_context(),
				)
				.await;

				if !mode.should_suppress_cli_output() {
					println!("✗ Task error for '{}': {}", tool_name, e);
				}

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

	// Handle large outputs with batched confirmation
	let processed_results = handle_large_tool_results(tool_results, config, mode).await?;
	Ok((processed_results, total_tool_time_ms))
}

// Handle large tool results with batched confirmation
async fn handle_large_tool_results(
	results: Vec<crate::mcp::McpToolResult>,
	config: &Config,
	mode: OutputMode,
) -> Result<Vec<crate::mcp::McpToolResult>> {
	use colored::Colorize;
	use std::io::{stdin, stdout, Write};

	// Apply token truncation first so the warning threshold sees the already-truncated size.
	// This prevents the user prompt from firing when truncation would bring the output under
	// the warning threshold anyway.
	let results: Vec<crate::mcp::McpToolResult> = results
		.into_iter()
		.map(|mut result| {
			let content_str = format!("{}", result.result);
			let (truncated, was_truncated) = crate::utils::truncation::truncate_mcp_response_global(
				&content_str,
				config.mcp_response_tokens_threshold,
			);
			if was_truncated {
				result.result = serde_json::Value::String(truncated);
			}
			result
		})
		.collect();

	// Find large results (checked AFTER truncation)
	let mut large_indices = Vec::new();
	let mut total_tokens = 0;

	for (index, result) in results.iter().enumerate() {
		let estimated_tokens = crate::session::estimate_tokens(&format!("{}", result.result));
		if config.mcp_response_warning_threshold > 0
			&& estimated_tokens > config.mcp_response_warning_threshold
		{
			large_indices.push((index, estimated_tokens));
			total_tokens += estimated_tokens;
		}
	}

	// No large results - return as is
	if large_indices.is_empty() {
		return Ok(results);
	}

	// Single large result - use existing individual behavior
	if large_indices.len() == 1 {
		let (index, _) = large_indices[0];
		let result = &results[index];
		let processed = crate::mcp::handle_large_response(result.clone(), config, mode).await?;
		let mut new_results = results;
		new_results[index] = processed;
		return Ok(new_results);
	}

	// Multiple large results - batch handling
	// Auto-decline in non-interactive mode
	if mode.should_suppress_cli_output() || !std::io::stdin().is_terminal() {
		if !mode.should_suppress_cli_output() {
			println!(
				"{}",
				format!(
					"Large outputs from {} tools ({} total tokens) automatically declined in non-interactive mode.",
					large_indices.len(), total_tokens
				)
				.bright_red()
			);
		}
		let mut processed_results = results;
		for (index, tokens) in large_indices {
			processed_results[index] = crate::mcp::McpToolResult::error(
				processed_results[index].tool_name.clone(),
				processed_results[index].tool_id.clone(),
				format!("Large output from tool '{}' ({} tokens) was automatically declined in non-interactive mode.", processed_results[index].tool_name, tokens)
			);
		}
		return Ok(processed_results);
	}

	// CRITICAL: Suspend animation before prompting user
	// This prevents animation from covering the prompt and from being restarted
	// by other code paths while waiting for user input
	use crate::session::chat::get_animation_manager;
	let animation_manager = get_animation_manager();
	animation_manager.suspend().await;

	// Interactive mode - show batched warning
	println!(
		"{}",
		format!(
			"⚠️  WARNING: {} tools produced large outputs (total: {} tokens)",
			large_indices.len(),
			total_tokens
		)
		.bright_yellow()
	);

	// Show each large tool
	for (i, (index, tokens)) in large_indices.iter().enumerate() {
		let result = &results[*index];
		let server_name =
			crate::session::chat::response::get_tool_server_name_async(&result.tool_name, config)
				.await;
		println!(
			"{}",
			format!(
				"[{}] {} ({}) - {} tokens{}",
				i + 1,
				result.tool_name,
				server_name,
				tokens,
				if !result.tool_id.is_empty() {
					format!(" [ID: {}]", result.tool_id)
				} else {
					String::new()
				}
			)
			.bright_yellow()
		);
	}

	println!(
		"{}",
		"This may consume significant tokens and impact your usage limits.".bright_yellow()
	);

	// Single prompt for all
	print!("{}", "Do you want to continue? [y/N/1,2,3]: ".bright_cyan());
	stdout().flush().unwrap();

	let mut input = String::new();
	stdin().read_line(&mut input).unwrap_or_default();
	let input = input.trim();

	// Resume animation now that user input is complete
	animation_manager.resume();

	let mut processed_results = results;

	if input.is_empty() || input.to_lowercase().starts_with('n') {
		// Decline all
		println!("{}", "All large outputs declined by user.".bright_red());
		for (index, tokens) in large_indices {
			processed_results[index] = crate::mcp::McpToolResult::error(
				processed_results[index].tool_name.clone(),
				processed_results[index].tool_id.clone(),
				format!(
					"User declined large output from tool '{}' ({} tokens).",
					processed_results[index].tool_name, tokens
				),
			);
		}
	} else if input.to_lowercase().starts_with('y') {
		// Accept all
		println!("{}", "Proceeding with all outputs...".bright_green());
	} else {
		// Parse selective numbers
		let selected: std::collections::HashSet<usize> = input
			.split(',')
			.filter_map(|s| s.trim().parse::<usize>().ok())
			.filter(|&i| i > 0 && i <= large_indices.len())
			.collect();

		if selected.is_empty() {
			println!("{}", "Invalid selection. Declining all...".bright_red());
			for (index, tokens) in large_indices {
				processed_results[index] = crate::mcp::McpToolResult::error(
					processed_results[index].tool_name.clone(),
					processed_results[index].tool_id.clone(),
					format!(
						"User declined large output from tool '{}' ({} tokens).",
						processed_results[index].tool_name, tokens
					),
				);
			}
		} else {
			let kept_count = selected.len();
			let declined_count = large_indices.len() - kept_count;
			println!(
				"{}",
				format!(
					"✓ Keeping {} tools, declining {} tools",
					kept_count, declined_count
				)
				.bright_green()
			);

			for (i, (index, tokens)) in large_indices.iter().enumerate() {
				if !selected.contains(&(i + 1)) {
					processed_results[*index] = crate::mcp::McpToolResult::error(
						processed_results[*index].tool_name.clone(),
						processed_results[*index].tool_id.clone(),
						format!(
							"User selectively declined large output from tool '{}' ({} tokens).",
							processed_results[*index].tool_name, tokens
						),
					);
				}
			}
		}
	}

	Ok(processed_results)
}

// Parameters for tool display functions
struct ToolDisplayParams<'a> {
	stored_tool_call: &'a Option<crate::mcp::McpToolCall>,
	tool_name: &'a str,
	tool_id: &'a str,
	tool_index: usize,
	is_single_tool: bool,
}

// Display successful tool execution (after execution - header + results)
async fn display_tool_success(
	params: ToolDisplayParams<'_>,
	res: &crate::mcp::McpToolResult,
	tool_time_ms: u64,
	config: &Config,
	mode: OutputMode,
	session_name: &str,
	execution_context: Option<String>, // New parameter for context display
) {
	// For multiple tools: show header again with index
	// For single tool in main session: skip header (already shown upfront)
	// For layer/agent contexts: always show header (no upfront display in isolated contexts)
	if !mode.should_suppress_cli_output() && (!params.is_single_tool || execution_context.is_some())
	{
		crate::session::chat::tool_display::display_individual_tool_header_with_context(
			params.tool_name,
			params.stored_tool_call,
			config,
			params.tool_index,
			execution_context.as_deref(), // Pass the execution context
		)
		.await;
	}

	// Show the actual tool output based on log level using MCP protocol
	// Skip in JSONL mode (output goes through callback instead)
	if !mode.should_suppress_cli_output()
		&& (config.get_log_level().is_info_enabled() || config.get_log_level().is_debug_enabled())
	{
		// Extract content using MCP protocol
		let content = crate::mcp::extract_mcp_content(&res.result);

		if !content.trim().is_empty() {
			if config.get_log_level().is_debug_enabled() {
				// Debug mode: Show full content
				println!("{}", content);
			} else {
				// Info mode: Show smart output (with some reasonable limits)
				crate::session::chat::tool_display::display_tool_output_smart(&content);
			}
		}
	}

	// None mode: No output shown (as requested)

	// Always show completion status with timing and token count
	// Skip in JSONL mode (output goes through callback instead)
	if !mode.should_suppress_cli_output() {
		let content = crate::mcp::extract_mcp_content(&res.result);
		let token_count = crate::session::token_counter::estimate_tokens(&content);
		let formatted_tokens = crate::session::chat::format_number(token_count as u64);

		println!(
			"✓ Tool '{}' completed in {}ms [{}]",
			params.tool_name, tool_time_ms, formatted_tokens
		);
		println!("──────────────────");
	}

	// Log the tool response with session name and timing
	let _ = crate::session::logger::log_tool_result(
		session_name,
		params.tool_id,
		&res.result,
		tool_time_ms,
	);
}

// Display tool error in consolidated format
async fn display_tool_error(
	stored_tool_call: &Option<crate::mcp::McpToolCall>,
	tool_name: &str,
	error: &anyhow::Error,
	tool_index: usize,
	config: &Config,
	mode: OutputMode,
	execution_context: Option<String>,
) {
	if mode.should_suppress_cli_output() {
		return;
	}

	// Always show header with parameters — same as success path.
	// On error the upfront header was shown without parameters, so we must re-show it here.
	crate::session::chat::tool_display::display_individual_tool_header_with_context(
		tool_name,
		stored_tool_call,
		config,
		tool_index,
		execution_context.as_deref(),
	)
	.await;

	// Show error status
	println!("✗ Tool '{}' failed: {}", tool_name, error);
}

// Handle user-declined large output in chat session
fn handle_declined_in_session(tool_id: &str, chat_session: &mut ChatSession) {
	println!("⚠ Tool output declined by user - removing tool call from conversation");

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

/// Parameters for layer tool execution using the unified parallel logic.
pub struct LayerToolExecutionParams<'a> {
	pub tool_calls: Vec<crate::mcp::McpToolCall>,
	pub session_name: String,
	pub layer_config: &'a crate::session::layers::LayerConfig,
	pub layer_name: String,
	pub operation_cancelled: Option<tokio::sync::watch::Receiver<bool>>,
	pub mode: OutputMode,
}

/// Execute tool calls for layers using the unified parallel execution logic
pub async fn execute_layer_tool_calls_parallel(
	config: &Config,
	params: LayerToolExecutionParams<'_>,
) -> Result<(Vec<crate::mcp::McpToolResult>, u64)> {
	let mut context = ToolExecutionContext::Layer {
		session_name: params.session_name,
		layer_config: params.layer_config,
		layer_name: params.layer_name,
	};

	execute_tools_in_context(
		params.tool_calls,
		&mut context,
		config,
		params.operation_cancelled,
		params.mode,
	)
	.await
}
