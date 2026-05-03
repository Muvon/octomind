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
	} else {
		log_debug!("Animation suspended during tool result processing - not restarting");
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
		// CRITICAL FIX: Extract ONLY the actual tool output, not our custom JSON wrapper
		let raw_content = extract_tool_content(tool_result);

		// Tool result deduplication: if the same (tool_name, content) pair has
		// already been seen in this session, replace the body with a small
		// placeholder. The first occurrence is kept verbatim so the model can
		// still reason about the original output.
		let tool_content =
			if crate::session::dedup::is_duplicate(&tool_result.tool_name, &raw_content) {
				log_debug!(
					"deduplicated tool result for `{}` ({} chars elided)",
					tool_result.tool_name,
					raw_content.len()
				);
				crate::session::dedup::placeholder(&tool_result.tool_name)
			} else {
				crate::session::dedup::record(&tool_result.tool_name, &raw_content);
				raw_content
			};

		// Apply global MCP response token truncation before adding to session
		let (tool_content, was_truncated) = crate::utils::truncation::truncate_mcp_response_global(
			&tool_content,
			config.mcp_response_tokens_threshold,
		);
		if was_truncated {
			let is_terminal = crate::config::with_thread_config(|c| c.output_mode())
				.unwrap_or(crate::session::output::OutputMode::NonInteractive)
				.is_terminal_mode();
			if is_terminal {
				use colored::Colorize;
				eprintln!(
					"{}",
					format!(
						"⚠️  Tool '{}' response truncated to {} tokens (mcp_response_tokens_threshold)",
						tool_result.tool_name, config.mcp_response_tokens_threshold
					)
					.bright_yellow()
				);
			}
		}

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

	// 🗜️ PLAN-DRIVEN COMPRESSION: Process any pending compression requests
	// This happens after tool results are added but before the follow-up API call
	// Compression can significantly reduce context before the next request

	// CRITICAL FIX: Set start_index for next task AFTER plan tool execution
	// This ensures start_index points to the BEGINNING of task work, not the last message
	let mut plan_tool_executed = false;
	for tool_result in &tool_results {
		if tool_result.tool_name == "plan" {
			plan_tool_executed = true;
			break;
		}
	}

	// If plan tool was executed, handle start_index and compression range
	if plan_tool_executed {
		// Check if we need to set start_index for the NEXT task
		// This happens after plan(start) or plan(next) when start_index is None
		if crate::mcp::core::plan::core::get_current_task_start_index().is_none()
			&& crate::mcp::core::plan::core::has_active_plan()
		{
			// CRITICAL: Set start_index to last valid message index (the plan tool result)
			// Compression will remove messages from (start_index + 1) to end_index
			// So start_index should point to the plan command result that we want to KEEP
			// If we have 93 messages (indices 0-92), start_index should be 92 (last message)
			let message_count = chat_session.get_message_count();
			if message_count == 0 {
				crate::log_debug!("Cannot set start_index: no messages in session");
			} else {
				let start_index = message_count - 1; // Last valid index
				crate::mcp::core::plan::set_current_task_start_index(start_index);
				crate::log_debug!(
					"Plan task start_index set to: {} (last message index, total messages: {})",
					start_index,
					message_count
				);
			}
		}

		// If compression is pending (plan(next) was called), set the message range
		if crate::mcp::core::plan::has_pending_compression() {
			// Get the start index that was set when the PREVIOUS task started
			if let Some(start_index) = crate::mcp::core::plan::core::get_and_clear_start_index() {
				// Use last valid index (len - 1) since remove_messages_in_range uses inclusive end_index
				let end_index = chat_session.get_message_count() - 1;

				crate::log_debug!(
					"Setting compression range: start={}, end={} (total messages: {})",
					start_index,
					end_index,
					chat_session.get_message_count()
				);

				// Set the message range on the pending compression task
				if let Err(e) =
					crate::mcp::core::plan::set_pending_compression_range(start_index, end_index)
				{
					log_info!(
						"Compression range could not be set: {}. Compression will be skipped.",
						e
					);
				}
			} else {
				log_info!(
					"Plan compression pending but no start_index found. \
					This may indicate plan(next) was called before any task work was done."
				);
			}
		}
	}

	// Process pending plan compressions (task → phase → project)
	// OPTIMIZATION: When project compression is pending (plan(done)), defer ALL compression
	// to after the assistant's final message. This preserves cache for the follow-up API call
	// that generates the "plan completed" response — compression runs after that response
	// is displayed, so the next user turn gets the compressed context for free.
	if crate::mcp::core::plan::has_pending_project_compression() {
		log_debug!(
			"Deferring plan compression to after assistant response (project compression pending)"
		);
	} else {
		// plan(next) path: compress immediately so the AI has reduced context for next task
		let _task_compression_occurred =
			match crate::mcp::core::plan::process_pending_compression(chat_session).await {
				Ok(Some(metrics)) => {
					chat_session.session.info.compression_stats.add_compression(
						crate::session::CompressionKind::Task,
						metrics.messages_removed,
						metrics.tokens_saved,
					);
					crate::session::chat::cost_tracker::CostTracker::display_compression_result(
						"Task", &metrics,
					);
					true
				}
				Ok(None) => false,
				Err(e) => {
					log_info!(
						"Task compression skipped: {}. Context was not compressed.",
						e
					);
					false
				}
			};

		// Process phase compression (automatic)
		let _phase_compression_occurred =
			match crate::mcp::core::plan::process_pending_phase_compression(chat_session).await {
				Ok(Some(metrics)) => {
					chat_session.session.info.compression_stats.add_compression(
						crate::session::CompressionKind::Phase,
						metrics.messages_removed,
						metrics.tokens_saved,
					);
					crate::session::chat::cost_tracker::CostTracker::display_compression_result(
						"Phase", &metrics,
					);
					true
				}
				Ok(None) => false,
				Err(e) => {
					log_info!(
						"Phase compression skipped: {}. Context was not compressed.",
						e
					);
					false
				}
			};
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
		log_debug!(
			"Adaptive conversation compression failed during tool processing: {}. Continuing session.",
			e
		);
	}

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
		chat_session.session.messages.push(crate::session::Message {
			role: "user".to_string(),
			content: hint_message,
			..Default::default()
		});
	}

	// Make follow-up API call
	let follow_up_result =
		make_follow_up_api_call(chat_session, config, operation_cancelled.clone()).await;

	// NOTE: Don't stop animation here - only stop when we're actually done with tools
	// If there are more tools to call, the animation should continue running
	// Animation will be stopped after checking should_continue_conversation

	// Show cost breakdown for intermediate results (after tool calls, before follow-up AI call)
	// Always show simple cost line, detailed breakdown only at info log level
	use crate::session::chat::cost_tracker::CostTracker;
	CostTracker::display_intermediate_cost_breakdown(chat_session);

	match follow_up_result {
		Ok(response) => {
			// Store direct tool calls for efficient processing if they exist
			let has_more_tools = if let Some(ref calls) = response.tool_calls {
				!calls.is_empty()
			} else {
				// Fall back to parsing if no direct tool calls
				!crate::mcp::parse_tool_calls(&response.content).is_empty()
			};

			// Debug logging for follow-up finish_reason
			if let Some(ref reason) = response.finish_reason {
				log_debug!("Follow-up finish_reason: {}", reason);
			}

			// Check finish_reason to determine if we should continue the conversation
			let should_continue_conversation =
				check_should_continue(&response, config, has_more_tools);

			// Handle cost tracking from follow-up API call
			handle_follow_up_cost_tracking(chat_session, &response.exchange, config);

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
	let model = chat_session.model.clone();
	let temperature = chat_session.temperature;

	// CRITICAL FIX: Pass cancellation token to ensure immediate cancellation
	let validation_params = ChatCompletionWithValidationParams::new(
		&chat_session.session.messages,
		&model,
		temperature,
		chat_session.top_p,
		chat_session.top_k,
		chat_session.max_tokens,
		config,
	)
	.with_max_retries(chat_session.max_retries)
	.with_cancellation_token(cancellation_token);
	crate::session::chat_completion_with_validation(validation_params).await
}

// Check if conversation should continue based on finish_reason
pub fn check_should_continue(
	response: &crate::providers::ProviderResponse,
	_config: &Config,
	has_more_tools: bool,
) -> bool {
	match response.finish_reason.as_deref() {
		Some("tool_calls") | Some("tool_use") => {
			// Model wants to make more tool calls
			log_debug!("finish_reason is 'tool_calls', continuing conversation");
			true
		}
		Some("stop") | Some("length") | Some("end_turn") => {
			// Model finished normally or hit length limit
			log_debug!(
				"finish_reason is '{}', ending conversation",
				response.finish_reason.as_deref().unwrap()
			);
			false
		}
		Some(other) => {
			// Unknown finish_reason, be conservative and continue
			log_info!("Unknown finish_reason '{}', continuing conversation", other);
			true
		}
		None => {
			// No finish_reason, check for tool calls
			log_debug!("Debug: No finish_reason, checking for tool calls");
			has_more_tools
		}
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

		// Update cost
		if let Some(cost) = usage.cost {
			// OpenRouter credits = dollars, use the value directly
			chat_session.session.info.total_cost += cost;
			chat_session.estimated_cost = chat_session.session.info.total_cost;

			log_debug!(
				"Adding ${:.5} to total cost (total now: ${:.5})",
				cost,
				chat_session.session.info.total_cost
			);

			// Enhanced debug for follow-up calls
			log_debug!("Tool response usage detail:");
			if let Ok(usage_str) = serde_json::to_string_pretty(usage) {
				log_debug!("{}", usage_str);
			}

			// Check for cache-related fields
			if let Some(raw_usage) = exchange.response.get("usage") {
				log_debug!("Raw tool response usage object:");
				if let Ok(raw_str) = serde_json::to_string_pretty(raw_usage) {
					log_debug!("{}", raw_str);
				}

				// Look specifically for cache-related fields
				if let Some(cache_cost) = raw_usage.get("cache_cost") {
					log_debug!("Found cache_cost field: {}", cache_cost);
				}

				if let Some(cached_cost) = raw_usage.get("cached_cost") {
					log_debug!("Found cached_cost field: {}", cached_cost);
				}

				if let Some(any_cache) = raw_usage.get("cached") {
					log_debug!("Found cached field: {}", any_cache);
				}
			}
		} else {
			// Try to get cost from the raw response if not in usage struct
			let cost_from_raw = exchange
				.response
				.get("usage")
				.and_then(|u| u.get("cost"))
				.and_then(|c| c.as_f64());

			if let Some(cost) = cost_from_raw {
				// Use the cost value directly
				chat_session.session.info.total_cost += cost;
				chat_session.estimated_cost = chat_session.session.info.total_cost;

				log_debug!(
					"Using cost ${:.5} from raw response (total now: ${:.5})",
					cost,
					chat_session.session.info.total_cost
				);
				// Provider did not provide cost data - this is normal for some providers (e.g., Ollama)
				let provider_name = &exchange.provider;
				log_debug!(
					"{} did not provide cost data for tool response API call",
					provider_name
				);

				// Check if usage tracking was explicitly requested (OpenRouter-specific)
				if provider_name == "openrouter" {
					let has_usage_flag = exchange
						.request
						.get("usage")
						.and_then(|u| u.get("include"))
						.and_then(|i| i.as_bool())
						.unwrap_or(false);

					log_debug!(
						"{} request had usage.include flag: {}",
						provider_name,
						has_usage_flag
					);
					if !has_usage_flag {
						log_debug!(
							"Make sure usage.include=true is set for {} to get cost data",
							provider_name
						);
					}
				}

				// Dump the raw response for debugging
				log_debug!("Raw {} response for debugging:", provider_name);
				if let Ok(resp_str) = serde_json::to_string_pretty(&exchange.response) {
					log_debug!("Partial response JSON:\n{}", resp_str);
				}
			}
		}
	} else {
		println!(
			"{}",
			"ERROR: No usage data for tool response API call".bright_red()
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
