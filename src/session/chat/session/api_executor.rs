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
use super::core::{ChatSession, MAX_PLUGIN_CONTINUATIONS};
use super::error_utils::display_rate_limit_info;
use crate::config::Config;
use crate::plugins::ground_truth;
use crate::plugins::payload::ContextParts;
use crate::plugins::protocol::{event, BackgroundOutput, RequestPhase, TurnEnd, TurnKind};
use crate::session::chat_completion_with_validation;
use crate::session::ChatCompletionWithValidationParams;
use anyhow::Result;
use colored::*;
use tokio::sync::watch;

use crate::session::output::{OutputMode, OutputSink};

/// `TurnEnd::stopped` after octomind ended the turn on repeated tool rounds.
const LOOP_STOP: &str = "loop";

/// `turn.end`: tell the plugins the agent stopped with no tool call left and
/// apply their answer. Returns true when a plugin kept the turn going: its text
/// is written as the next input and the caller runs the request again.
async fn end_turn_with_plugins(chat_session: &mut ChatSession, config: &Config) -> Result<bool> {
	let session_id = chat_session.session.info.name.clone();
	if !crate::plugins::subscribed(&session_id, event::TURN_END) {
		return Ok(false);
	}
	let parts = ContextParts::of(chat_session);
	// Without a size limit from a plugin the host sends only which files changed.
	let diff_chars = crate::plugins::options(&session_id)
		.iter()
		.filter_map(|(_, options)| options.diff_chars)
		.max();
	let paths: Vec<String> = chat_session.turn_mutated_paths.iter().cloned().collect();
	let max_identical_rounds = config.max_identical_rounds;
	let info = &chat_session.session.info;
	let payload = TurnEnd {
		turn: if chat_session.completion_gate_eligible {
			TurnKind::User
		} else {
			TurnKind::System
		},
		answers: if chat_session.turn_answers.is_empty() {
			vec![chat_session.last_response.clone()]
		} else {
			chat_session.turn_answers.clone()
		},
		pending_async: crate::session::has_pending_async_work(),
		changes: ground_truth::file_changes(&parts.workdir, &paths, diff_chars).await,
		tree_status: if diff_chars.is_some() {
			ground_truth::tree_status(&parts.workdir).await
		} else {
			Vec::new()
		},
		tree: ground_truth::tree_fingerprint(),
		requests: info.total_api_calls.saturating_sub(info.api_calls_at_turn_start) as u64,
		stopped: (max_identical_rounds > 0 && chat_session.repeat_guard.identical_rounds() >= max_identical_rounds)
			.then(|| LOOP_STOP.to_string()),
		// A finished job is not linked to the call that started it here, so no call id is known.
		background: crate::session::shell_jobs::take_completed_for_session(&session_id)
			.into_iter()
			.map(|job| BackgroundOutput {
				call_id: String::new(),
				label: job.label,
				output: job.body,
			})
			.collect(),
	};
	let outcome = crate::plugins::turn_end(&parts.context(&chat_session.session.messages, None), payload).await;
	crate::plugins::apply::apply(chat_session, outcome.effects).await?;
	// A note at the end of a turn must be written now: an inbox item at a turn
	// boundary would start a new turn.
	chat_session.deliver_plugin_notes()?;
	chat_session.plugin_verified = outcome.verified;
	let Some(text) = outcome.continue_with else {
		return Ok(false);
	};
	if chat_session.plugin_continuations >= MAX_PLUGIN_CONTINUATIONS {
		crate::log_debug!(
			"Plugin asked to continue the turn after {} continuations; ending it",
			MAX_PLUGIN_CONTINUATIONS
		);
		return Ok(false);
	}
	chat_session.add_system_managed_user_message(&text)?;
	chat_session.last_self_report = None;
	chat_session.plugin_continuations += 1;
	Ok(true)
}

// Helper function to execute API call and process response
pub async fn execute_api_call_and_process_response<S: OutputSink>(
	chat_session: &mut ChatSession,
	config: &Config,
	role: &str,
	operation_rx: watch::Receiver<bool>,
	mode: OutputMode,
	sink: S,
) -> Result<()> {
	run_request_round(
		chat_session,
		config,
		role,
		operation_rx,
		mode,
		sink,
		RequestPhase::TurnStart,
	)
	.await
}

/// One model request and the end-of-turn work after it. A re-run of the turn
/// calls this again with `RequestPhase::Continuation`.
async fn run_request_round<S: OutputSink>(
	chat_session: &mut ChatSession,
	config: &Config,
	role: &str,
	operation_rx: watch::Receiver<bool>,
	mode: OutputMode,
	sink: S,
	phase: RequestPhase,
) -> Result<()> {
	let model = chat_session.model.clone();
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

	chat_session.send_request_before(config, phase).await?;

	// Context blocks and notes plugins set for this request land here, before
	// the request is built.
	chat_session.deliver_plugin_context()?;
	chat_session.deliver_plugin_notes()?;

	// Supervisor: goal and execution-boundary recitation (see
	// `inject_before_request`); before the cache-marker advance below.
	crate::session::recite::inject_before_request(
		chat_session,
		config,
		crate::session::recite::RecitalRefresh::ByVolume,
	)?;

	crate::session::chat::conversation_compression::ensure_context_within_ceiling(chat_session, config).await?;

	// Advance Anthropic-style content cache markers after persistent pre-call
	// injections (plugin context and notes, inbox hints, steers, etc.). This
	// preserves the previous marker while moving the oldest marker to the latest
	// user/tool boundary for this new request.
	let cache_manager = crate::session::cache::CacheManager::new();
	let supports_caching = crate::session::model_supports_caching(&model);
	if let Err(e) =
		cache_manager.check_and_apply_auto_cache_threshold(&mut chat_session.session, config, supports_caching, role)
	{
		crate::log_debug!("pre-request cache marker advance failed: {}", e);
	}
	// Project the bounded summary views only for the provider request; they may
	// not enter persistence, compression, extraction, or later history.
	let full_summaries = chat_session.prepare_request_messages();

	// Make API call. `session.messages` is borrowed directly — no clone — and
	// the validation params hold that shared borrow only until they're consumed
	// by `chat_completion_with_validation` below.
	let schema = chat_session.schema.clone();
	let model_profile = chat_session.model_profile(&config_clone);
	let validation_params =
		ChatCompletionWithValidationParams::from_profile(&chat_session.session.messages, &model_profile, &config_clone)
			.with_full_context_tokens(true)
			.with_cancellation_token(operation_rx.clone());
	let validation_params = if let Some(schema) = schema {
		validation_params.with_schema(schema)
	} else {
		validation_params
	};
	let api_result = chat_completion_with_validation(validation_params).await;
	chat_session.restore_summaries_after_request(full_summaries);

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
			if let Err(e) = CostTracker::track_exchange_cost(chat_session, &response.exchange, config) {
				if mode.is_terminal_mode() {
					println!("{}: Failed to track exchange cost: {}", "Warning".bright_yellow(), e);
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
				chat_session: &mut *chat_session,
				config,
				role,
				operation_cancelled: operation_rx_for_response.clone(),
				sink: sink.clone(),
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

	// Plugins judge the turn once no tool call is left.
	if end_turn_with_plugins(chat_session, config).await? {
		return Box::pin(run_request_round(
			chat_session,
			config,
			role,
			operation_rx,
			mode,
			sink,
			RequestPhase::Continuation,
		))
		.await;
	}

	let pending_async = crate::session::has_pending_async_work();
	// A session-owned background job (or an unread inbox result) means the turn is a
	// wait, not a stop: the inbox monitor resumes the agent when the result lands,
	// and that later turn inherits this turn's eligibility.
	chat_session.gate_deferred = chat_session.completion_gate_eligible && pending_async;

	// A background fold still in flight at turn end: collect and apply it so
	// the session persists compacted — replace only, never auto-continue (no
	// agent call is made here; the summary was already paid for).
	match crate::session::chat::conversation_compression::settle_pending_fold(chat_session, &config_clone).await {
		Ok(true) => {
			if let Err(error) = chat_session.save() {
				crate::log_debug!("Session save after settled fold failed: {}", error);
			}
		}
		Ok(false) => {}
		Err(error) => crate::log_debug!("Settling background fold failed: {}", error),
	}

	chat_session.finish_turn_timing();

	Ok(())
}

#[cfg(test)]
#[path = "api_executor_tests.rs"]
mod e2e_tests;
