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

use super::super::response::{process_response, ResponseProcessingParams};
use super::super::CostTracker;
use super::core::ChatSession;
use super::error_utils::display_rate_limit_info;
use crate::config::Config;
use crate::session::chat_completion_with_validation;
use crate::session::ChatCompletionWithValidationParams;
use anyhow::Result;
use colored::*;
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

	// Calculate animation parameters
	let current_cost = chat_session.session.info.total_cost;
	let max_threshold = config.max_session_tokens_threshold;
	let current_context_tokens = chat_session.get_full_context_tokens(config).await as u64;

	// Clone operation_rx for response processing
	let operation_rx_for_response = operation_rx.clone();

	// CRITICAL FIX: Check spending threshold BEFORE starting animation
	// This prevents animation from covering the Y/N prompt
	if mode.is_interactive() {
		match chat_session.check_spending_threshold(config) {
			Ok(should_continue) => {
				if !should_continue {
					// User chose not to continue due to spending threshold
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

	// Update animation state with current cost/context values
	// Animation was already started early in main_loop to cover pre-processing
	use crate::session::chat::get_animation_manager;
	let animation_manager = get_animation_manager();
	let anim_state = animation_manager.get_state();
	anim_state.update_cost(current_cost);
	anim_state.update_context_tokens(current_context_tokens);
	anim_state.update_max_threshold(max_threshold);

	// CRITICAL: Connect session cancellation to animation for INSTANT Ctrl+C response
	animation_manager.set_cancel_receiver(operation_rx.clone());

	// Inject learned lessons as user message on first API call (once per session, resets on /done)
	if !chat_session.learning_injected && config.learning.enabled {
		chat_session.learning_injected = true;
		crate::log_debug!("Learning injection triggered for this task");
		let current_dir = crate::mcp::get_thread_working_directory();
		let project = current_dir
			.file_name()
			.and_then(|n| n.to_str())
			.unwrap_or("unknown")
			.to_string();
		// Extract user's input from the last user message for query-based retrieval
		let user_input = chat_session
			.session
			.messages
			.iter()
			.rev()
			.find(|m| m.role == "user")
			.map(|m| m.content.clone())
			.unwrap_or_default();
		let learned_context = crate::learning::inject::retrieve_and_format(
			chat_session,
			config,
			&user_input,
			role,
			&project,
			operation_rx.clone(),
		)
		.await;
		if !learned_context.is_empty() {
			chat_session.add_user_message(&learned_context)?;
			crate::log_debug!("Injected learning context as user message");
		}
	}

	// Make API call
	let messages = chat_session.session.messages.clone();
	let max_retries = chat_session.max_retries;
	let schema = chat_session.schema.clone();
	let reasoning_effort = chat_session.reasoning_effort;
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
	let validation_params = if let Some(schema) = schema {
		validation_params.with_schema(schema)
	} else {
		validation_params
	};
	let validation_params = if let Some(effort) = reasoning_effort {
		validation_params.with_reasoning_effort(effort)
	} else {
		validation_params
	};
	let api_result = chat_completion_with_validation(validation_params).await;

	// DON'T stop animation here - process_response stops it before tool output.
	// After the tool header is printed, response.rs restarts the animation so it
	// runs during tool execution, giving the user progress feedback.

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

			// Update animation cost BEFORE process_response stops it.
			// track_exchange_cost() just updated total_cost; push it now so the
			// animation (and next turn's start) shows the correct post-call value.
			anim_state.update_cost(chat_session.session.info.total_cost);

			// Display rate limit information if available
			display_rate_limit_info(&response.exchange);

			// Process the response with tool calls
			// CRITICAL FIX: Use operation_cancelled instead of creating a new token
			// This ensures Ctrl+C cancellation works properly during tool execution
			let process_result = process_response(ResponseProcessingParams {
				content: response.content,
				exchange: response.exchange,
				tool_calls: response.tool_calls,
				thinking: response.thinking,
				finish_reason: response.finish_reason,
				response_id: response.response_id,
				chat_session,
				config,
				role,
				operation_cancelled: operation_rx_for_response.clone(),
				sink,
				mode,
			})
			.await;

			// Propagate response-processing errors (e.g. follow-up API call failures
			// after tool execution) so the main loop can offer a Ctrl+G retry.
			// Previously this was printed-and-swallowed, hiding the failure from
			// the retry mechanism.
			process_result?;
		}
		Err(e) => {
			// Stop animation on error before returning
			animation_manager.stop_current().await;
			return Err(e);
		}
	}

	Ok(())
}
