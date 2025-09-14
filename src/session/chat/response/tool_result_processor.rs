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

// Tool result processor module - handles tool result processing, caching, and follow-up API calls

use crate::config::Config;
use crate::session::chat::animation::show_smart_animation;
use crate::session::chat::session::ChatSession;
use crate::session::ChatCompletionWithValidationParams;
use crate::{log_debug, log_info};
use anyhow::Result;
use colored::Colorize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

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
	)>,
> {
	// Add the accumulated tool execution time to the session total
	chat_session.session.info.total_tool_time_ms += total_tool_time_ms;

	// Check for cancellation before making another request
	if *operation_cancelled.borrow() {
		println!("{}", "\nOperation cancelled by user.".bright_yellow());
		// Do NOT add any confusing message to the session
		return Ok(None);
	}

	// Create separate animation flag but monitor global cancellation
	let animation_cancel = Arc::new(AtomicBool::new(false));

	// Set up monitor task to propagate global cancellation to animation
	let animation_cancel_monitor = animation_cancel.clone();
	let operation_cancelled_monitor = operation_cancelled.clone();
	let _cancel_monitor = tokio::spawn(async move {
		while !animation_cancel_monitor.load(Ordering::SeqCst) {
			if *operation_cancelled_monitor.borrow() {
				animation_cancel_monitor.store(true, Ordering::SeqCst);
				break;
			}
			tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;
		}
	});

	// 🎯 SENIOR FIX: Show "Generating response..." IMMEDIATELY after tools complete
	// This provides instant feedback while tool results are being processed
	let animation_cancel_flag = animation_cancel.clone();
	let current_cost = chat_session.session.info.total_cost;
	let animation_task = tokio::spawn(async move {
		let _ = show_smart_animation(animation_cancel_flag, current_cost).await;
	});

	// 🔍 PERFORMANCE DEBUG: Track where time is spent during tool result processing
	let processing_start = std::time::Instant::now();

	// IMPROVED APPROACH: Add tool results as proper "tool" role messages
	// This follows the standard OpenAI/Anthropic format and avoids double-serialization
	// CRITICAL FIX: Check cache threshold after EACH tool result, not after all
	let cache_manager = crate::session::cache::CacheManager::new();
	let supports_caching = crate::session::model_supports_caching(&chat_session.model);

	let mut cache_check_time = 0u128;
	let mut truncation_time = 0u128;

	// PERFORMANCE OPTIMIZATION: Batch process tool results with smart truncation
	// Instead of checking truncation after EVERY tool result (expensive),
	// we batch process and only truncate when necessary or at the end
	let mut accumulated_content_size = 0;
	let mut needs_truncation_check = false;

	for tool_result in &tool_results {
		// CRITICAL FIX: Extract ONLY the actual tool output, not our custom JSON wrapper
		let tool_content = extract_tool_content(tool_result);

		// PERFORMANCE OPTIMIZATION: Check size before moving content
		let content_size = tool_content.len();
		accumulated_content_size += content_size;
		let is_large_output = content_size > 10000; // 10KB+ outputs
		let accumulated_is_large = accumulated_content_size > 50000; // 50KB+ total

		if is_large_output || accumulated_is_large {
			needs_truncation_check = true;
		}

		// Use the new add_tool_message method which handles token tracking properly
		chat_session.add_tool_message(
			&tool_content,
			&tool_result.tool_id,
			&tool_result.tool_name,
			config,
		)?;

		// Check truncation only for large individual tool outputs (file contents, search results, etc.)
		if is_large_output {
			let truncation_start = std::time::Instant::now();
			if let Err(e) = crate::session::chat::context_truncation::check_and_truncate_context(
				chat_session,
				config,
				crate::session::chat::TruncationOptions {
					defer_continuation: true, // Defer during tool processing
				},
			)
			.await
			{
				log_info!("Warning: Error during tool result truncation check: {}", e);
			}
			truncation_time += truncation_start.elapsed().as_millis();

			// Reset flags after truncation
			needs_truncation_check = false;
			accumulated_content_size = 0;
		}
	}

	// BATCH TRUNCATION: Check once after all small tool results are processed
	if needs_truncation_check {
		let truncation_start = std::time::Instant::now();
		if let Err(e) = crate::session::chat::context_truncation::check_and_truncate_context(
			chat_session,
			config,
			crate::session::chat::TruncationOptions {
				defer_continuation: true, // Defer during tool processing
			},
		)
		.await
		{
			log_info!("Warning: Error during batch truncation check: {}", e);
		}
		truncation_time += truncation_start.elapsed().as_millis();
	}

	// NEW FLOW: Check for continuation AFTER all tool results are gathered
	// This is one of the two correct moments to trigger continuation:
	// 1) On new user request (handled in session runner)
	// 2) After all tool results gathered, before sending to AI (HERE)
	use crate::session::chat::session_continuation;
	if session_continuation::check_and_handle_continuation(chat_session, config).await? {
		// Stop animation before returning
		animation_cancel.store(true, Ordering::SeqCst);
		let _ = animation_task.await;

		log_info!("Token limit reached after processing all tool results - continuation triggered");
		return Ok(None); // Return None to stop tool processing and let continuation be handled by main loop
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
			"🔍 Tool result processing took {}ms (cache: {}ms, truncation: {}ms)",
			total_processing_time,
			cache_check_time,
			truncation_time
		);
	}

	// Check spending threshold before making follow-up API call
	match chat_session.check_spending_threshold(config) {
		Ok(should_continue) => {
			if !should_continue {
				// User chose not to continue due to spending threshold
				animation_cancel.store(true, Ordering::SeqCst);
				let _ = animation_task.await;
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
				animation_cancel.store(true, Ordering::SeqCst);
				let _ = animation_task.await;
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
		// Stop animation before returning
		animation_cancel.store(true, Ordering::SeqCst);
		let _ = animation_task.await;
		println!("{}", "\nOperation cancelled by user.".bright_yellow());
		return Ok(None);
	}

	// Make follow-up API call
	let follow_up_result =
		make_follow_up_api_call(chat_session, config, operation_cancelled.clone()).await;

	// Stop the animation and wait for completion
	animation_cancel.store(true, Ordering::SeqCst);
	let _ = animation_task.await;

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

			// Display rate limit information if available
			display_rate_limit_info(&response.exchange);

			if should_continue_conversation {
				Ok(Some((
					response.content,
					response.exchange,
					response.tool_calls,
				)))
			} else {
				// If no more tools, return None to break out of the loop
				Ok(Some((response.content, response.exchange, None)))
			}
		}
		Err(e) => {
			// Extract provider name from the model for better error messaging
			let provider_name = if let Ok((provider, _)) =
				crate::providers::ProviderFactory::parse_model(&chat_session.model)
			{
				provider
			} else {
				"unknown provider".to_string()
			};

			// IMPROVED: Show provider-aware context about the API error
			let error_message =
				crate::session::chat::session::format_provider_error(&provider_name, &e);
			println!(
				"\n{} {}: {}",
				"✗".bright_red(),
				format!("Error calling {}", provider_name).bright_red(),
				error_message
			);

			// Additional context if error contains provider information
			log_debug!("Model: {}", chat_session.model);
			log_debug!("Temperature: {}", chat_session.temperature);

			Err(e)
		}
	}
}

// Extract tool content from tool result
fn extract_tool_content(tool_result: &crate::mcp::McpToolResult) -> String {
	if let Some(output) = tool_result.result.get("output") {
		// Extract the "output" field which contains the actual tool result
		if let Some(output_str) = output.as_str() {
			output_str.to_string()
		} else {
			// If output is not a string, serialize it
			serde_json::to_string(output).unwrap_or_default()
		}
	} else if tool_result.result.is_string() {
		// If result is already a string, use it directly
		tool_result.result.as_str().unwrap_or("").to_string()
	} else {
		// Fallback: look for common fields or use the whole result
		if let Some(error) = tool_result.result.get("error") {
			format!("Error: {}", error)
		} else {
			// Last resort: serialize the whole result
			serde_json::to_string(&tool_result.result).unwrap_or_default()
		}
	}
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
		// Simple token extraction with clean provider interface
		let cached_tokens = usage.cached_tokens;
		let regular_prompt_tokens = usage.prompt_tokens.saturating_sub(cached_tokens);

		// Update session token counts using the cache manager
		let cache_manager = crate::session::cache::CacheManager::new();
		cache_manager.update_token_tracking(
			&mut chat_session.session,
			regular_prompt_tokens,
			usage.output_tokens,
			cached_tokens,
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

			// CRITICAL: Log session stats immediately after cost update
			let _ = crate::session::logger::log_session_stats(
				&chat_session.session.info.name,
				&chat_session.session.info,
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

				// CRITICAL: Log session stats immediately after cost update
				let _ = crate::session::logger::log_session_stats(
					&chat_session.session.info.name,
					&chat_session.session.info,
				);
			} else {
				// Only show error if no cost data found
				println!(
					"{}",
					"ERROR: OpenRouter did not provide cost data for tool response API call"
						.bright_red()
				);
				println!("{}", "Make sure usage.include=true is set!".bright_red());

				// Check if usage tracking was explicitly requested
				let has_usage_flag = exchange
					.request
					.get("usage")
					.and_then(|u| u.get("include"))
					.and_then(|i| i.as_bool())
					.unwrap_or(false);

				println!(
					"{} {}",
					"Request had usage.include flag:".bright_yellow(),
					has_usage_flag
				);

				// Dump the raw response for debugging
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
