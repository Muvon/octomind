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

use std::time::Duration;

use super::super::animation::{show_loading_animation, show_no_animation};
use super::super::response::{process_response, ResponseProcessingParams};
use super::super::CostTracker;
use super::core::ChatSession;
use super::error_utils::display_rate_limit_info;
use crate::config::Config;
use crate::mcp::get_available_functions;
use crate::session::chat_completion_with_validation;
use crate::session::estimate_full_context_tokens;
use crate::session::ChatCompletionWithValidationParams;
use anyhow::Result;
use colored::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::watch;

use crate::session::output::{OutputMode, OutputSink};

// Helper function to execute API call and process response
pub async fn execute_api_call_and_process_response<S: OutputSink>(
	chat_session: &mut ChatSession,
	config: &Config,
	role: &str,
	operation_rx: watch::Receiver<bool>,
	mode: OutputMode,
	sink: S,
) -> Result<()> {
	let model = chat_session.model.clone();
	let temperature = chat_session.temperature;
	let config_clone = config.clone();

	// Create animation task
	let animation_cancel = Arc::new(AtomicBool::new(false));
	let animation_cancel_clone = animation_cancel.clone();
	let current_cost = chat_session.session.info.total_cost;
	let max_threshold = config.max_session_tokens_threshold;

	// Calculate actual current context tokens for percentage display
	let (_, _, _, _, system_prompt) = config.get_role_config(role);
	let tools = get_available_functions(config).await;
	let current_context_tokens = estimate_full_context_tokens(
		&chat_session.session.messages,
		Some(system_prompt),
		Some(&tools),
	) as u64;

	// Set up monitor task to propagate global cancellation to animation
	let animation_cancel_monitor = animation_cancel.clone();
	let operation_cancelled_monitor = operation_rx.clone();
	let operation_rx_for_response = operation_rx.clone();
	let _cancel_monitor = tokio::spawn(async move {
		while !animation_cancel_monitor.load(Ordering::SeqCst) {
			if *operation_cancelled_monitor.borrow() {
				animation_cancel_monitor.store(true, Ordering::SeqCst);
				break;
			}
			tokio::time::sleep(tokio::time::Duration::from_millis(1)).await;
		}
	});

	// Check if we're in JSONL mode before spawning animation
	let suppress_animation = mode.should_suppress_cli_output();

	let animation_task = tokio::spawn(async move {
		// Skip animation entirely when in JSONL mode
		if suppress_animation {
			// Just wait for cancellation without any output
			while !animation_cancel_clone.load(Ordering::SeqCst) {
				tokio::time::sleep(Duration::from_millis(10)).await;
			}
			return;
		}

		if mode.is_interactive() {
			let _ = show_loading_animation(
				animation_cancel_clone,
				current_cost,
				current_context_tokens,
				max_threshold,
			)
			.await;
		} else {
			let _ = show_no_animation(animation_cancel_clone, current_cost).await;
		}
	});

	// Check spending threshold for interactive mode
	if mode.is_interactive() {
		match chat_session.check_spending_threshold(config) {
			Ok(should_continue) => {
				if !should_continue {
					// User chose not to continue due to spending threshold
					let _ = animation_task.await;
					return Ok(());
				}
			}
			Err(e) => {
				// Error checking threshold, log and continue
				println!(
					"{}: {}",
					"Warning: Error checking spending threshold".bright_yellow(),
					e
				);
			}
		}

		// Check request spending threshold
		match chat_session.check_request_spending_threshold(config) {
			Ok(should_continue) => {
				if !should_continue {
					// Request spending threshold exceeded - stop execution
					let _ = animation_task.await;
					return Ok(());
				}
			}
			Err(e) => {
				// Error checking request threshold, log and continue
				println!(
					"{}: {}",
					"Warning: Error checking request spending threshold".bright_yellow(),
					e
				);
			}
		}
	}

	// Make API call
	let messages = chat_session.session.messages.clone();
	let max_retries = chat_session.max_retries;
	let validation_params = ChatCompletionWithValidationParams::new(
		&messages,
		&model,
		temperature,
		chat_session.top_p,
		chat_session.top_k,
		chat_session.max_tokens,
		&config_clone,
	)
	.with_max_retries(max_retries)
	.with_chat_session(chat_session)
	.with_cancellation_token(operation_rx);
	let api_result = chat_completion_with_validation(validation_params).await;

	// Stop animation
	animation_cancel.store(true, Ordering::SeqCst);
	let _ = animation_task.await;

	// CRITICAL FIX: Check for cancellation after API call completion
	// This prevents the race condition where Ctrl+C is pressed after API completes
	// but before response processing begins
	if *operation_rx_for_response.borrow() {
		crate::log_debug!("Operation cancelled by user.");
		return Ok(()); // Return gracefully to main loop instead of force exit
	}

	// Process response
	match api_result {
		Ok(response) => {
			// CRITICAL FIX: Track exchange cost immediately after successful API call
			// This ensures all API calls (with or without tool calls) have their costs tracked
			if let Err(e) =
				CostTracker::track_exchange_cost(chat_session, &response.exchange, config)
			{
				if mode.is_terminal_mode() {
					println!(
						"{}: Failed to track exchange cost: {}",
						"Warning".bright_yellow(),
						e
					);
				}
			}

			// Display rate limit information if available
			display_rate_limit_info(&response.exchange);

			// Process the response with tool calls
			// CRITICAL FIX: Use operation_cancelled instead of creating a new token
			// This ensures Ctrl+C cancellation works properly during tool execution
			let process_result = process_response(
				ResponseProcessingParams::new(
					response.content,
					response.exchange,
					response.tool_calls,
					response.finish_reason,
					response.response_id,
					chat_session,
					config,
					role,
					operation_rx_for_response.clone(),
					sink,
				)
				.with_thinking(response.thinking)
				.with_mode(mode),
			) // Pass through mode, thinking, and sink
			.await;

			if let Err(e) = process_result {
				if mode.is_terminal_mode() {
					println!("\n{}: {}", "Error processing response".bright_red(), e);
				}
			}
		}
		Err(e) => {
			return Err(e);
		}
	}

	Ok(())
}
