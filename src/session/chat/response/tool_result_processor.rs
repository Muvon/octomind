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

// Tool result processor module - handles tool result processing, caching, and follow-up API calls

use crate::config::Config;
use crate::session::chat::session::ChatSession;
use crate::session::ChatCompletionWithValidationParams;
use crate::{log_debug, log_info};
use anyhow::Result;
use colored::Colorize;

// Process tool results and handle follow-up API calls
pub async fn process_tool_results(
	tool_results: Vec<crate::mcp::McpToolResult>,
	total_tool_time_ms: u64,
	chat_session: &mut ChatSession,
	config: &Config,
	role: &str,
	operation_cancelled: tokio::sync::watch::Receiver<bool>,
) -> Result<
	Option<(
		String,
		crate::session::ProviderExchange,
		Option<Vec<crate::mcp::McpToolCall>>,
		Option<String>,                          // response_id from follow-up API call
		Option<crate::providers::ThinkingBlock>, // thinking from follow-up API call
	)>,
> {
	// Add the accumulated tool execution time to the session total
	chat_session.session.info.total_tool_time_ms += total_tool_time_ms;

	// Check for cancellation before making another request
	if *operation_cancelled.borrow() {
		crate::log_debug!("Operation cancelled by user.");
		// Do NOT add any confusing message to the session
		return Ok(None);
	}

	// Start animation (uses state already set by api_executor.rs)
	// CRITICAL FIX: Don't recalculate animation parameters here to avoid flickering
	// Animation state is set once at request start in api_executor.rs and remains stable
	use crate::session::chat::get_animation_manager;
	let animation_manager = get_animation_manager();

	// DEFENSE: Don't start animation if suspended (e.g., during user prompt)
	// This prevents animation from covering prompts in race conditions
	if !animation_manager.is_suspended() {
		animation_manager
			.start_animation(
				&crate::config::with_thread_config(|c| c.output_mode())
					.unwrap_or(crate::session::output::OutputMode::NonInteractive),
			)
			.await;
	}

	// 🔍 PERFORMANCE DEBUG: Track where time is spent during tool result processing
	let processing_start = std::time::Instant::now();

	// IMPROVED APPROACH: Add tool results as proper "tool" role messages
	// This follows the standard OpenAI/Anthropic format and avoids double-serialization
	// CRITICAL FIX: Check cache threshold after EACH tool result, not after all
	let cache_manager = crate::session::cache::CacheManager::new();
	let supports_caching = crate::session::model_supports_caching(&chat_session.model);

	let mut cache_check_time = 0u128;

	for tool_result in &tool_results {
		// CRITICAL FIX: Extract ONLY the actual tool output, not our custom JSON wrapper.
		// Deduplication already ran upstream in execute_tools_parallel (before display),
		// where a duplicate was converted into an error result carrying the placeholder.
		// Here we just extract and pass the body through.
		let tool_content = extract_tool_content(tool_result);

		// (Resource links a tool advertised — detached background jobs — are
		// registered as pending in the MCP client the moment the tool returns,
		// before the job can complete; see `mcp::client::call_tool`.)

		// Apply global MCP response token truncation before adding to session
		let (tool_content, was_truncated) = crate::utils::truncation::truncate_mcp_response_global(
			&tool_content,
			config.mcp_response_tokens_threshold,
			&tool_result.tool_name,
		);
		// Truncation marker is rendered inline on each tool's close line in
		// `display_tool_success` (e.g. `╰ ✓ view 55ms · 6.9K tokens · truncated
		// to 4K tokens`). Re-printing a separate `⚠️ … truncated …` line here
		// would duplicate the warning and break the framed block.
		let _ = was_truncated;

		// Use the new add_tool_message method which handles token tracking properly
		// NOTE: Compression intentionally does NOT run mid-loop here. Compressing while
		// we are still adding tool_results for the current assistant's tool_calls would
		// orphan the tool_use blocks (drain the parent assistant message) and leave
		// subsequent tool_results with no matching tool_use — the API would then reject
		// the follow-up request. Compression is safely handled after the full batch is
		// added (see check_and_compress_conversation call below) and before the next
		// API request (see api_prep.rs).
		chat_session.add_tool_message(
			&tool_content,
			&tool_result.tool_id,
			&tool_result.tool_name,
			config,
		)?;
	}

	// External plan manager: a sparse hidden signal rides with the specialist's
	// normal action batch. Reconcile only after results exist, then inject the
	// manager-owned state before the already-needed follow-up request. With no
	// signal this is a free no-op.
	if config.supervisor.enabled && config.supervisor.plan.enabled {
		animation_manager.set_phase("Reconciling plan …").await;
		if let Err(error) = crate::supervisor::plan::reconcile_after_actions(
			chat_session,
			config,
			operation_cancelled.clone(),
		)
		.await
		{
			crate::log_debug!("External plan reconciliation failed: {}", error);
		}
		animation_manager.clear_phase();
	}

	// 🗜️ ADAPTIVE CONVERSATION COMPRESSION: Check if context should be compressed
	if let Err(e) = crate::session::chat::conversation_compression::check_and_compress_conversation(
		chat_session,
		config,
		operation_cancelled.clone(),
		crate::session::chat::conversation_compression::CompressionTrigger::Automatic,
	)
	.await
	{
		if crate::session::chat::conversation_compression::within_ceiling_margin(
			chat_session,
			config,
		)
		.await
		{
			return Err(e.context("forced compression inside the context ceiling margin failed"));
		}
		log_debug!(
			"Adaptive conversation compression failed during tool processing: {}.",
			e
		);
	}
	crate::session::chat::conversation_compression::ensure_context_within_ceiling(
		chat_session,
		config,
	)
	.await?;

	// CRITICAL FIX: Check cache threshold AFTER all tool results are processed
	// This ensures cache markers are set at the correct boundary - after all parallel
	// tool results are added to session, but before sending the complete batch to server
	let cache_start = std::time::Instant::now();
	if let Ok(true) = cache_manager.check_and_apply_auto_cache_threshold(
		&mut chat_session.session,
		config,
		supports_caching,
		role,
	) {
		log_debug!("Auto-cache threshold reached after processing all tool results - cache checkpoint applied before follow-up API request.");
	}
	cache_check_time += cache_start.elapsed().as_millis();

	// 🔍 PERFORMANCE DEBUG: Report processing breakdown and track processing time
	let total_processing_time = processing_start.elapsed().as_millis() as u64;

	// Add the processing time to the session total
	chat_session.session.info.total_layer_time_ms += total_processing_time;

	if total_processing_time > 100 {
		log_debug!(
			"🔍 Tool result processing took {}ms (cache: {}ms)",
			total_processing_time,
			cache_check_time
		);
	}

	// Check spending threshold before making follow-up API call
	match chat_session.check_spending_threshold(config) {
		Ok(should_continue) => {
			if !should_continue {
				// User chose not to continue due to spending threshold
				// Stop global animation before returning
				animation_manager.stop_current().await;
				println!(
					"{}",
					"✗ Tool follow-up cancelled due to spending threshold.".bright_red()
				);
				return Ok(None);
			}
		}
		Err(e) => {
			// Error checking threshold, log warning and continue
			use colored::*;
			println!(
				"{}: {}",
				"Warning: Error checking spending threshold".bright_yellow(),
				e
			);
		}
	}

	// Check request spending threshold before making follow-up API call
	match chat_session.check_request_spending_threshold(config) {
		Ok(should_continue) => {
			if !should_continue {
				// Request spending threshold exceeded - stop execution
				// Stop global animation before returning
				animation_manager.stop_current().await;
				println!(
					"{}",
					"✗ Tool follow-up cancelled due to request spending threshold.".bright_red()
				);
				return Ok(None);
			}
		}
		Err(e) => {
			// Error checking request threshold, log warning and continue
			use colored::*;
			println!(
				"{}: {}",
				"Warning: Error checking request spending threshold".bright_yellow(),
				e
			);
		}
	}

	// CRITICAL FIX: Check for cancellation before making follow-up API call
	if *operation_cancelled.borrow() {
		// Stop global animation before returning
		animation_manager.stop_current().await;
		crate::log_debug!("Operation cancelled by user.");
		return Ok(None);
	}

	// Inject accumulated tool-misuse hints as a user message so the AI sees guidance
	// without polluting individual tool result strings. Hints are deduplicated across
	// all parallel tool calls in this round and cleared after injection.
	let hints = crate::mcp::hint_accumulator::drain_hints();
	if !hints.is_empty() {
		let bullet_list = hints
			.iter()
			.map(|h| format!("• {h}"))
			.collect::<Vec<_>>()
			.join("\n");
		let hint_message = format!(
			"⚠️ Tool usage notice:\n{bullet_list}\n\nPlease prefer the recommended tools going forward."
		);
		chat_session.add_system_managed_user_message(&hint_message)?;
	}

	// Supervisor: deliver any queued steer note HERE — during the tool loop, before
	// the follow-up call — not only at the next user turn. The detector sets
	// `steer_pending` mid-loop, but it was previously consumed only at the turn-start
	// injection point (api_executor), so a runaway identical-call loop never actually
	// saw the steer (it only landed when a real user message began a fresh turn). This
	// is the round-by-round delivery that makes the steer reach the model in the loop.
	if let Some(note) = chat_session.steer_pending.take() {
		chat_session.add_system_managed_user_message(&note)?;
		crate::log_debug!("Supervisor steer injected (tool loop)");
	}

	// Deliver everything that landed in the inbox WHILE this turn was running — a
	// finished background job, a monitor batch, a tap reply. The inbox was drained
	// only between turns, so a result that arrived mid-loop stayed invisible until
	// the model happened to stop; waiting for it was therefore something the model
	// had to arrange itself, and one that polls instead of yielding pays a
	// full-context request per poll. Delivered here, after this round's tool
	// results and before the next call, the result reaches the model on the very
	// next round. Human-shaped injections stay queued for the turn boundary, where
	// they get real user-turn semantics.
	while let Some(msg) = crate::session::inbox::try_pop_system_managed_message() {
		crate::log_debug!("Inbox delivered mid-turn from {:?}", msg.source);
		if crate::logging::tracing_setup::is_structured_output_mode() {
			let injected = crate::websocket::ServerMessage::Injected(
				crate::websocket::protocol::InjectedPayload {
					source_kind: msg.source.display_kind().to_string(),
					source_label: msg.source.display_label(),
					content: msg.content.clone(),
					session_id: chat_session.session.info.name.clone(),
				},
			);
			crate::mcp::process::send_notification_message(injected);
		} else {
			crate::session::inbox::display_injected_input(&msg);
		}
		chat_session.add_system_managed_user_message(&msg.content)?;
	}

	// Make follow-up API call
	let follow_up_result =
		make_follow_up_api_call(chat_session, config, operation_cancelled.clone()).await;

	// NOTE: Don't stop animation here - only stop when we're actually done with tools
	// If there are more tools to call, the animation should continue running
	// Animation will be stopped after checking should_continue_conversation

	match follow_up_result {
		Ok(response) => {
			// Use structured tool_calls from the API response; the legacy
			// text-parse fallback never returned anything.
			let has_more_tools = response
				.tool_calls
				.as_ref()
				.is_some_and(|calls| !calls.is_empty());

			// Check finish_reason to determine if we should continue the conversation
			let should_continue_conversation =
				check_should_continue(&response, config, has_more_tools);
			log_debug!(
				"Provider response [follow-up]: finish={}, tool_calls={}, continue={}",
				response.finish_reason.as_deref().unwrap_or("none"),
				response.tool_calls.as_ref().map_or(0, Vec::len),
				should_continue_conversation
			);

			// Handle cost tracking from follow-up API call
			handle_follow_up_cost_tracking(chat_session, &response.exchange, config);

			// Show the cost line for this follow-up round. Printed only on the
			// success path and only after cost tracking: the Err path must NOT
			// print (on Ctrl+C the main loop prints the cancellation snapshot —
			// printing here too produced a doubled identical line), and printing
			// before tracking showed the previous round's stale total.
			use crate::session::chat::cost_tracker::CostTracker;
			CostTracker::display_intermediate_cost_breakdown(chat_session);

			// CRITICAL FIX: Update animation state after cost tracking
			// This ensures the animation shows updated cost/tokens during multi-hop tool loops
			// The animation loop reads from shared state every 100ms, so this keeps it current
			let current_cost = chat_session.session.info.total_cost;
			let current_context_tokens = chat_session.get_full_context_tokens(config).await as u64;
			animation_manager.get_state().update_cost(current_cost);
			animation_manager
				.get_state()
				.update_context_tokens(current_context_tokens);

			// Display rate limit information if available
			display_rate_limit_info(&response.exchange);

			if should_continue_conversation {
				Ok(Some((
					response.content,
					response.exchange,
					response.tool_calls,
					response.response_id, // Include response_id from follow-up response
					response.thinking,    // CRITICAL FIX: Include thinking from follow-up response for Moonshot
				)))
			} else {
				// If no more tools, stop animation and return
				animation_manager.stop_current().await;
				Ok(Some((
					response.content,
					response.exchange,
					None,
					response.response_id,
					response.thinking, // CRITICAL FIX: Include thinking even when stopping
				)))
			}
		}
		Err(e) => {
			// Centralized error printing happens in the main loop's Err branch
			// (handle_followup_api_error). Just log diagnostics, stop animation,
			// and propagate so the caller can offer a Ctrl+G retry.
			log_debug!(
				"Follow-up API call failed for model {}: {}",
				chat_session.model,
				e
			);
			log_debug!("Temperature: {}", chat_session.temperature);

			// Stop animation on error before returning
			animation_manager.stop_current().await;

			Err(e)
		}
	}
}

// Extract tool content from tool result
fn extract_tool_content(tool_result: &crate::mcp::McpToolResult) -> String {
	tool_result.extract_content()
}

// Make follow-up API call with cancellation support
async fn make_follow_up_api_call(
	chat_session: &ChatSession,
	config: &Config,
	cancellation_token: tokio::sync::watch::Receiver<bool>,
) -> Result<crate::providers::ProviderResponse> {
	let profile = chat_session.model_profile(config);

	// CRITICAL FIX: Pass cancellation token to ensure immediate cancellation
	let validation_params = ChatCompletionWithValidationParams::from_profile(
		&chat_session.session.messages,
		&profile,
		config,
	)
	.with_cancellation_token(cancellation_token);

	// Carry the structured-output schema onto every follow-up turn too. Without
	// this the schema only applies to the first call (which returns a tool call,
	// not the answer), so the final post-tool reply is unconstrained. The schema
	// is native `response_format` and coexists with tool calling.
	let validation_params = if let Some(schema) = chat_session.schema.clone() {
		validation_params.with_schema(schema)
	} else {
		validation_params
	};
	crate::session::chat_completion_with_validation(validation_params).await
}

// Check if conversation should continue based on finish_reason
pub fn check_should_continue(
	response: &crate::providers::ProviderResponse,
	_config: &Config,
	has_more_tools: bool,
) -> bool {
	match response.finish_reason.as_deref() {
		Some("tool_calls") | Some("tool_use") => true,
		Some("stop") | Some("length") | Some("end_turn") => false,
		Some(other) => {
			// Unknown finish_reason, be conservative and continue
			log_info!("Unknown finish_reason '{}', continuing conversation", other);
			true
		}
		None => has_more_tools,
	}
}

// Handle cost tracking from follow-up API call
fn handle_follow_up_cost_tracking(
	chat_session: &mut ChatSession,
	exchange: &crate::session::ProviderExchange,
	_config: &Config,
) {
	if let Some(usage) = &exchange.usage {
		// Every follow-up exchange = one completed API call (mirrors CostTracker::track_exchange_cost)
		chat_session.session.info.total_api_calls += 1;

		// Update session token counts using cache manager with octolib data directly
		let cache_manager = crate::session::cache::CacheManager::new();
		cache_manager.update_token_tracking(
			&mut chat_session.session,
			usage.input_tokens, // Non-cached input tokens from API
			usage.output_tokens,
			usage.cache_read_tokens,
			usage.cache_write_tokens,
			usage.reasoning_tokens,
		);

		// Track API time from the follow-up exchange
		if let Some(api_time_ms) = usage.request_time_ms {
			chat_session.session.info.total_api_time_ms += api_time_ms;
		}

		let raw_cost = exchange
			.response
			.get("usage")
			.and_then(|value| value.get("cost"))
			.and_then(|value| value.as_f64());
		let (cost, cost_source) = match (usage.cost, raw_cost) {
			(Some(cost), _) => (Some(cost), "normalized"),
			(None, Some(cost)) => (Some(cost), "raw"),
			(None, None) => (None, "unreported"),
		};
		if let Some(cost) = cost {
			chat_session.session.info.total_cost += cost;
			chat_session.estimated_cost = chat_session.session.info.total_cost;
		}
		let cost_summary = cost
			.map(|value| format!("${value:.5} ({cost_source})"))
			.unwrap_or_else(|| cost_source.to_string());
		let latency = usage
			.request_time_ms
			.map(|value| format!("{value}ms"))
			.unwrap_or_else(|| "unreported".to_string());
		log_debug!(
			"Provider usage [follow-up]: provider={}, input={}, output={}, cache_read={}, cache_write={}, reasoning={}, cost={}, session_total=${:.5}, latency={}",
			exchange.provider,
			usage.input_tokens,
			usage.output_tokens,
			usage.cache_read_tokens,
			usage.cache_write_tokens,
			usage.reasoning_tokens,
			cost_summary,
			chat_session.session.info.total_cost,
			latency
		);
		if cost.is_none()
			&& exchange.provider == "openrouter"
			&& !exchange
				.request
				.get("usage")
				.and_then(|value| value.get("include"))
				.and_then(|value| value.as_bool())
				.unwrap_or(false)
		{
			log_debug!("OpenRouter cost unavailable: request usage.include was false");
		}
	} else {
		log_debug!(
			"Provider usage [follow-up]: provider={}, usage unavailable",
			exchange.provider
		);
	}
}

// Helper function to display rate limit information from provider response
fn display_rate_limit_info(exchange: &crate::session::ProviderExchange) {
	if let Some(ref rate_limit_headers) = exchange.rate_limit_headers {
		let mut rate_limit_info = Vec::new();

		match exchange.provider.as_str() {
			"anthropic" => {
				// Anthropic rate limit format
				if let (Some(tokens_remaining), Some(tokens_limit)) = (
					rate_limit_headers.get("tokens_remaining"),
					rate_limit_headers.get("tokens_limit"),
				) {
					rate_limit_info.push(format!("Tokens: {}/{}", tokens_remaining, tokens_limit));
				}

				if let (Some(input_remaining), Some(input_limit)) = (
					rate_limit_headers.get("input_tokens_remaining"),
					rate_limit_headers.get("input_tokens_limit"),
				) {
					rate_limit_info
						.push(format!("Input tokens: {}/{}", input_remaining, input_limit));
				}

				if let (Some(output_remaining), Some(output_limit)) = (
					rate_limit_headers.get("output_tokens_remaining"),
					rate_limit_headers.get("output_tokens_limit"),
				) {
					rate_limit_info.push(format!(
						"Output tokens: {}/{}",
						output_remaining, output_limit
					));
				}

				if !rate_limit_info.is_empty() {
					crate::log_info!("📊 Anthropic rate limits: {}", rate_limit_info.join(" | "));
				}
			}
			"openai" => {
				// OpenAI rate limit format
				if let (Some(requests_remaining), Some(requests_limit)) = (
					rate_limit_headers.get("requests_remaining"),
					rate_limit_headers.get("requests_limit"),
				) {
					rate_limit_info.push(format!(
						"Requests: {}/{}",
						requests_remaining, requests_limit
					));
				}

				if let (Some(tokens_remaining), Some(tokens_limit)) = (
					rate_limit_headers.get("tokens_remaining"),
					rate_limit_headers.get("tokens_limit"),
				) {
					rate_limit_info.push(format!("Tokens: {}/{}", tokens_remaining, tokens_limit));
				}

				if let Some(request_reset) = rate_limit_headers.get("request_reset") {
					rate_limit_info.push(format!("Request reset: {}", request_reset));
				}

				if !rate_limit_info.is_empty() {
					crate::log_info!("📊 OpenAI rate limits: {}", rate_limit_info.join(" | "));
				}
			}
			_ => {
				// Generic rate limit display for other providers
				if !rate_limit_headers.is_empty() {
					let info: Vec<String> = rate_limit_headers
						.iter()
						.map(|(k, v)| format!("{}: {}", k, v))
						.collect();
					crate::log_info!("📊 {} rate limits: {}", exchange.provider, info.join(" | "));
				}
			}
		}
	}
}

#[cfg(test)]
#[path = "tool_result_processor_tests.rs"]
mod tests;
