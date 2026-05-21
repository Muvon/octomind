// Copyright 2026 Muvon Un Limited
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
use crate::log_info;
use crate::session::chat::session::ChatSession;
use crate::session::chat::ToolProcessor;
use crate::session::output::OutputMode;
use anyhow::Result;
use colored::Colorize;

/// Context for tool execution - can be either main session or layer context
pub enum ToolExecutionContext<'a> {
	/// Main session context with full session access
	MainSession {
		chat_session: &'a mut ChatSession,
		tool_processor: &'a mut ToolProcessor,
	},
	/// Layer/agent context — tool access controlled by the ACP session's role config
	Layer {
		session_name: String,
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

	/// Check if tool is allowed in this context.
	/// Tool access for layers/agents is controlled by the ACP session's role config.
	pub fn is_tool_allowed(&self, _tool_name: &str) -> bool {
		true
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
	if current_tool_calls.len() == 1 && is_tap_capability_call(&current_tool_calls[0]) {
		chat_session.session.info.tool_calls += 1;
		let (result, elapsed_ms) =
			execute_tap_capability_inline(&current_tool_calls[0], chat_session, config).await;
		let processed = handle_large_tool_results(vec![result], config, mode).await?;
		return Ok((processed, elapsed_ms));
	}

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

fn is_tap_capability_call(call: &crate::mcp::McpToolCall) -> bool {
	call.tool_name == "tap"
		&& call
			.parameters
			.get("action")
			.and_then(|v| v.as_str())
			.is_some_and(|action| action == "capability")
}

async fn execute_tap_capability_inline(
	call: &crate::mcp::McpToolCall,
	chat_session: &mut ChatSession,
	config: &Config,
) -> (crate::mcp::McpToolResult, u64) {
	let started = std::time::Instant::now();
	let prompt = match call.parameters.get("prompt").and_then(|v| v.as_str()) {
		Some(p) if !p.trim().is_empty() => p.trim().to_string(),
		_ => {
			return (
				crate::mcp::McpToolResult::error(
					call.tool_name.clone(),
					call.tool_id.clone(),
					"Missing required parameter 'prompt'.".to_string(),
				),
				started.elapsed().as_millis() as u64,
			);
		}
	};

	let session_id = crate::session::context::current_session_id();
	let skills_before = session_id
		.as_ref()
		.map(crate::session::context::get_active_skills)
		.unwrap_or_default();
	let capabilities_before = crate::mcp::core::capability::list_active_names();

	let workdir = crate::mcp::workdir::get_thread_working_directory();
	crate::mcp::core::skill_auto::run_activation(&prompt, &workdir, chat_session).await;
	let mut activated_capabilities =
		crate::mcp::core::capability::auto_activate_capabilities_for_intent(&prompt, config).await;

	let activated_skills = session_id
		.as_ref()
		.map(crate::session::context::get_active_skills)
		.unwrap_or_default()
		.into_iter()
		.filter(|name| !skills_before.contains(name))
		.collect::<Vec<_>>();

	for name in crate::mcp::core::capability::list_active_names() {
		if !capabilities_before.contains(&name) && !activated_capabilities.contains(&name) {
			activated_capabilities.push(name);
		}
	}

	let message = if activated_skills.is_empty() && activated_capabilities.is_empty() {
		"No skill or capability matched the prompt."
	} else {
		"Skill/capability auto-activation completed."
	};

	let result = crate::mcp::McpToolResult::success(
		call.tool_name.clone(),
		call.tool_id.clone(),
		serde_json::json!({
			"activated_skills": activated_skills,
			"activated_capabilities": activated_capabilities,
			"message": message,
		})
		.to_string(),
	);

	(result, started.elapsed().as_millis() as u64)
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

	// Guardrails run sequentially over the ordered batch before any tool is
	// spawned. Tools arrive in the order the model emitted them, so `+/-`
	// history conditions evaluate against the intent log — earlier allowed
	// calls in the batch are visible to later ones. Blocked tools are NEVER
	// spawned; they return an immediate error result, saving the execution
	// roundtrip entirely.
	let session_id_for_guardrails = context.session_name().to_string();
	let block_messages: Vec<Option<String>> = crate::session::guardrails::check_batch(
		&session_id_for_guardrails,
		config,
		&current_tool_calls,
	);

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

		let tool_id_for_task = original_tool_id.clone();
		let tool_call_clone = tool_call.clone(); // Clone for async move
		let cancel_token_for_task = operation_cancelled.clone(); // Pass cancellation token

		// Get session ID for task-local propagation
		let session_id = context.session_name().to_string();

		// Guardrail decided this tool is denied — return the error result
		// directly without spawning the underlying executor.
		let block_msg = block_messages.get(index).cloned().unwrap_or(None);
		let task = if let Some(msg) = block_msg {
			let tool_name_for_err = tool_name.clone();
			let tool_id_for_err = original_tool_id.clone();
			tokio::spawn(async move {
				Ok::<_, anyhow::Error>((
					crate::mcp::McpToolResult::error(
						tool_name_for_err,
						tool_id_for_err,
						format!("[guardrail] {msg}"),
					),
					0u64,
				))
			})
		} else {
			match context {
				ToolExecutionContext::MainSession { .. } => tokio::spawn(async move {
					// Propagate session ID to spawned task for session-scoped state
					crate::session::context::with_session_id(session_id, async move {
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
					.await
				}),
				ToolExecutionContext::Layer { .. } => tokio::spawn(async move {
					crate::session::context::with_session_id(session_id, async move {
						let mut call_with_id = tool_call_clone.clone();
						call_with_id.tool_id = tool_id_for_task.clone();
						crate::mcp::execute_layer_tool_call(
							&call_with_id,
							&config_clone,
							cancel_token_for_task,
						)
						.await
					})
					.await
				}),
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

	// Animation lifecycle during tool execution:
	// - response.rs starts animation AFTER tool header is printed (so header is visible)
	// - This function stops animation BEFORE displaying results (no ghost spinners)
	// Do NOT start animation here — it's already running from response.rs.

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
			// Animation is stopped by main_loop.rs when Ctrl+C fires - no action needed here.

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

	// Note: No explicit stop needed before printing tool results.
	// The spinner-aware print macros use pb.suspend() to safely interleave
	// output with the live spinner (see src/lib.rs + animation_manager).

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
						let error_content = res.extract_content();
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
							tool_index,
						};
						display_tool_success(
							display_params,
							&res,
							tool_time_ms,
							config,
							mode,
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
							let loop_error_result = crate::mcp::McpToolResult::error(
								tool_name.clone(),
								tool_id.clone(),
								format!("LOOP DETECTED: Tool '{}' failed {} consecutive times. Last error: {}. Please try a completely different approach or ask the user for guidance.", tool_name, error_tracker.max_consecutive_errors(), e),
							);
							tool_results.push(loop_error_result);
						}
					} else {
						// Regular error - add normal error result
						let error_result = if let Some(error_tracker) = context.error_tracker() {
							crate::mcp::McpToolResult::error(
								tool_name.clone(),
								tool_id.clone(),
								format!(
									"Tool execution failed (attempt {}/{}): {}",
									error_tracker.get_error_count(&tool_name),
									error_tracker.max_consecutive_errors(),
									e
								),
							)
						} else {
							crate::mcp::McpToolResult::error(
								tool_name.clone(),
								tool_id.clone(),
								format!("Tool execution failed: {}", e),
							)
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

				// Display task error in consolidated format for other errors —
				// `display_tool_error` already prints a framed `╭ … ╰ ✗` block
				// with the error summary on the close line, so no extra raw
				// `println!` is needed below (that would just spill outside
				// the visual block as an unframed line).
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

				// ALWAYS add error result for task failures too (unless it was a user decline)
				let error_result = crate::mcp::McpToolResult::error(
					tool_name.clone(),
					tool_id.clone(),
					format!("Internal task error: {}", e),
				);
				tool_results.push(error_result);
			}
		}
	}

	// Post-result hooks: run AFTER all tool results are collected but BEFORE
	// truncation, so hook scripts see full untruncated output. Hooks for
	// guardrail-blocked tools are skipped (synthetic results aren't real).
	let blocked_flags: Vec<bool> = block_messages.iter().map(|m| m.is_some()).collect();
	crate::session::hooks::run_hooks(
		&session_id_for_guardrails,
		config,
		&current_tool_calls,
		&tool_results,
		&blocked_flags,
	)
	.await;

	// Handle large outputs with batched confirmation
	let processed_results = handle_large_tool_results(tool_results, config, mode).await?;
	Ok((processed_results, total_tool_time_ms))
}

// Handle large tool results: apply token-cap truncation (warnings removed).
async fn handle_large_tool_results(
	results: Vec<crate::mcp::McpToolResult>,
	config: &Config,
	_mode: OutputMode,
) -> Result<Vec<crate::mcp::McpToolResult>> {
	// Apply token truncation — hard cap on per-tool output size.
	let results: Vec<crate::mcp::McpToolResult> = results
		.into_iter()
		.map(|mut result| {
			let content_str = result.extract_content();
			let (truncated, was_truncated) = crate::utils::truncation::truncate_mcp_response_global(
				&content_str,
				config.mcp_response_tokens_threshold,
			);
			if was_truncated {
				result.result =
					rmcp::model::CallToolResult::success(vec![rmcp::model::Content::text(
						truncated,
					)]);
			}
			result
		})
		.collect();

	Ok(results)
}

// Parameters for tool display functions
struct ToolDisplayParams<'a> {
	stored_tool_call: &'a Option<crate::mcp::McpToolCall>,
	tool_name: &'a str,
	tool_index: usize,
}

// Display successful tool execution. Each tool gets a self-contained block:
// `╭ tool · server` + railed params + railed output + `╰ ✓ tool Xms · N tokens`.
// The header is re-rendered here regardless of upfront preview — the upfront
// preview is only a compact `▸ tool · server` queue listing for parallel
// tools, not a substitute for the full result block.
async fn display_tool_success(
	params: ToolDisplayParams<'_>,
	res: &crate::mcp::McpToolResult,
	tool_time_ms: u64,
	config: &Config,
	mode: OutputMode,
	execution_context: Option<String>,
) {
	if !mode.should_suppress_cli_output() {
		crate::session::chat::tool_display::display_individual_tool_header_with_context(
			params.tool_name,
			params.stored_tool_call,
			config,
			params.tool_index,
			execution_context.as_deref(),
		)
		.await;
	}

	// Output content (info/debug levels only; respect JSONL suppression).
	// `display_tool_output_smart` adds the `│ ` rail per line.
	if !mode.should_suppress_cli_output()
		&& (config.get_log_level().is_info_enabled() || config.get_log_level().is_debug_enabled())
	{
		let content = res.extract_content();
		if !content.trim().is_empty() {
			if config.get_log_level().is_debug_enabled() {
				use colored::Colorize;
				let rail = "│".bright_black();
				for line in content.lines() {
					println!("{} {}", rail, line);
				}
			} else {
				crate::session::chat::tool_display::display_tool_output_smart(&content);
			}
		}
	}

	// Close line: `╰ ✓ tool Xms · N tokens [· truncated to MK]` — tool name
	// identifies which tool finished when results arrive out of order. When
	// the response will be truncated by `mcp_response_tokens_threshold`, the
	// indicator is appended inline (yellow) instead of dumping a separate
	// `⚠️ … response truncated …` line below the block.
	if !mode.should_suppress_cli_output() {
		let content = res.extract_content();
		let token_count = crate::session::token_counter::estimate_tokens(&content);
		let formatted_tokens = crate::session::chat::format_number(token_count as u64);

		use colored::Colorize;
		let threshold = config.mcp_response_tokens_threshold;
		let truncated_suffix = if threshold > 0 && token_count > threshold {
			let formatted_threshold = crate::session::chat::format_number(threshold as u64);
			format!(
				" {} {}",
				"·".bright_black(),
				format!("truncated to {} tokens", formatted_threshold).bright_yellow()
			)
		} else {
			String::new()
		};
		println!(
			"{} {} {} {}ms {} {} tokens{}",
			"╰".bright_cyan(),
			"✓".bright_green(),
			params.tool_name.bright_cyan(),
			tool_time_ms,
			"·".bright_black(),
			formatted_tokens,
			truncated_suffix,
		);
	}
}

// Display tool error in consolidated format. Matches the success layout:
// re-rendered header + optional body lines under a `│ ` rail + `╰ ✗ tool:
// summary` close. The body rail is essential when the error message spans
// multiple lines (e.g. a `view` on a directory returns a one-line summary
// plus the directory listing as a hint) — without it, every line after the
// first prints raw and breaks the visual block.
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

	crate::session::chat::tool_display::display_individual_tool_header_with_context(
		tool_name,
		stored_tool_call,
		config,
		tool_index,
		execution_context.as_deref(),
	)
	.await;

	use colored::Colorize;
	let error_text = error.to_string();
	let mut lines = error_text.lines();
	let summary = lines.next().unwrap_or("").to_string();
	let body: String = lines.collect::<Vec<&str>>().join("\n");
	if !body.is_empty() {
		crate::session::chat::tool_display::display_tool_output_smart(&body);
	}
	println!(
		"{} {} {}: {}",
		"╰".bright_cyan(),
		"✗".bright_red(),
		tool_name.bright_red(),
		summary,
	);
}

/// Parameters for layer tool execution using the unified parallel logic.
pub struct LayerToolExecutionParams {
	pub tool_calls: Vec<crate::mcp::McpToolCall>,
	pub session_name: String,
	pub layer_name: String,
	pub operation_cancelled: Option<tokio::sync::watch::Receiver<bool>>,
	pub mode: OutputMode,
}

/// Execute tool calls for layers using the unified parallel execution logic
pub async fn execute_layer_tool_calls_parallel(
	config: &Config,
	params: LayerToolExecutionParams,
) -> Result<(Vec<crate::mcp::McpToolResult>, u64)> {
	let mut context = ToolExecutionContext::Layer {
		session_name: params.session_name,
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
