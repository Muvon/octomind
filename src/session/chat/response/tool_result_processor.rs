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
use crate::session::chat::animation::show_loading_animation;
use crate::session::chat::session::ChatSession;
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
	operation_cancelled: Arc<AtomicBool>,
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
	if operation_cancelled.load(Ordering::SeqCst) {
		println!("{}", "\nOperation cancelled by user.".bright_yellow());
		// Do NOT add any confusing message to the session
		return Ok(None);
	}

	// Create a fresh cancellation flag for the next phase
	let fresh_cancel = Arc::new(AtomicBool::new(false));

	// 🎯 SENIOR FIX: Show "Generating response..." IMMEDIATELY after tools complete
	// This provides instant feedback while tool results are being processed
	let animation_cancel_flag = fresh_cancel.clone();
	let current_cost = chat_session.session.info.total_cost;
	let animation_task = tokio::spawn(async move {
		let _ = show_loading_animation(animation_cancel_flag, current_cost).await;
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

		// CRITICAL FIX: Check auto-cache threshold IMMEDIATELY after EACH tool result
		// This ensures proper 2-marker logic and threshold checking after each tool
		let tool_message_index = chat_session.session.messages.len() - 1;
		let cache_start = std::time::Instant::now();
		if let Ok(true) = cache_manager.check_and_apply_auto_cache_threshold_on_tool_result(
			&mut chat_session.session,
			config,
			supports_caching,
			tool_message_index,
			role,
		) {
			log_info!("{}", format!("Auto-cache threshold reached after tool result '{}' - cache checkpoint applied before next API request.", tool_result.tool_name));
		}
		cache_check_time += cache_start.elapsed().as_millis();

		// Check truncation only for large individual tool outputs (file contents, search results, etc.)
		if is_large_output {
			let tool_truncate_cancelled = Arc::new(AtomicBool::new(false));
			let truncation_start = std::time::Instant::now();
			if let Err(e) = crate::session::chat::context_truncation::check_and_truncate_context(
				chat_session,
				config,
				role,
				tool_truncate_cancelled.clone(),
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
		let batch_truncate_cancelled = Arc::new(AtomicBool::new(false));
		let truncation_start = std::time::Instant::now();
		if let Err(e) = crate::session::chat::context_truncation::check_and_truncate_context(
			chat_session,
			config,
			role,
			batch_truncate_cancelled.clone(),
		)
		.await
		{
			log_info!("Warning: Error during batch truncation check: {}", e);
		}
		truncation_time += truncation_start.elapsed().as_millis();
	}

	// FINAL SAFETY CHECK: Truncate context before making follow-up API call
	// This ensures we don't send an oversized context to the API after processing
	// multiple large tool results
	let final_truncate_cancelled = Arc::new(AtomicBool::new(false));
	let final_truncation_start = std::time::Instant::now();
	if let Err(e) = crate::session::chat::context_truncation::check_and_truncate_context(
		chat_session,
		config,
		role,
		final_truncate_cancelled.clone(),
	)
	.await
	{
		log_info!(
			"Warning: Error during final truncation check before API call: {}",
			e
		);
	}
	truncation_time += final_truncation_start.elapsed().as_millis();

	// 🔍 PERFORMANCE DEBUG: Report processing breakdown and track processing time
	let total_processing_time = processing_start.elapsed().as_millis() as u64;

	// Add the processing time to the session total
	chat_session.session.info.total_layer_time_ms += total_processing_time;

	if config.get_log_level().is_debug_enabled() && total_processing_time > 100 {
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
				fresh_cancel.store(true, Ordering::SeqCst);
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

	// Make follow-up API call
	let follow_up_result = make_follow_up_api_call(chat_session, config).await;

	// Stop the animation and wait for completion
	fresh_cancel.store(true, Ordering::SeqCst);
	let _ = animation_task.await;

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
			if config.get_log_level().is_debug_enabled() {
				if let Some(ref reason) = response.finish_reason {
					log_debug!("Follow-up finish_reason: {}", reason);
				}
			}

			// Check finish_reason to determine if we should continue the conversation
			let should_continue_conversation =
				check_should_continue(&response, config, has_more_tools);

			// Handle cost tracking from follow-up API call
			handle_follow_up_cost_tracking(chat_session, &response.exchange, config);

			if should_continue_conversation {
				// Log if debug mode is enabled
				if config.get_log_level().is_debug_enabled() {
					println!(
						"{}",
						"Debug: Continuing conversation due to finish_reason or tool calls"
							.to_string()
							.yellow()
					);
				}
				// Continue processing the new content with tool calls
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
				crate::session::providers::ProviderFactory::parse_model(&chat_session.model)
			{
				provider
			} else {
				"unknown provider".to_string()
			};

			// IMPROVED: Show provider-aware context about the API error
			println!(
				"\n{} {}: {}",
				"✗".bright_red(),
				format!("Error calling {}", provider_name).bright_red(),
				e
			);

			// Additional context if error contains provider information
			if config.get_log_level().is_debug_enabled() {
				println!("{} Model: {}", "Debug:".bright_black(), chat_session.model);
				println!(
					"{} Temperature: {}",
					"Debug:".bright_black(),
					chat_session.temperature
				);
			}

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

// Make follow-up API call
async fn make_follow_up_api_call(
	chat_session: &ChatSession,
	config: &Config,
) -> Result<crate::session::providers::ProviderResponse> {
	let model = chat_session.model.clone();
	let temperature = chat_session.temperature;

	// Make sure to include the usage parameter for every API call
	// This ensures cost information is always returned
	crate::session::chat_completion_with_provider(
		&chat_session.session.messages,
		&model,
		temperature,
		config,
	)
	.await
}

// Check if conversation should continue based on finish_reason
fn check_should_continue(
	response: &crate::session::providers::ProviderResponse,
	config: &Config,
	has_more_tools: bool,
) -> bool {
	match response.finish_reason.as_deref() {
		Some("tool_calls") | Some("tool_use") => {
			// Model wants to make more tool calls
			if config.get_log_level().is_debug_enabled() {
				log_debug!("finish_reason is 'tool_calls', continuing conversation");
			}
			true
		}
		Some("stop") | Some("length") | Some("end_turn") => {
			// Model finished normally or hit length limit
			if config.get_log_level().is_debug_enabled() {
				log_debug!(
					"finish_reason is '{}', ending conversation",
					response.finish_reason.as_deref().unwrap()
				);
			}
			false
		}
		Some(other) => {
			// Unknown finish_reason, be conservative and continue
			if config.get_log_level().is_debug_enabled() {
				log_info!("Unknown finish_reason '{}', continuing conversation", other);
			}
			true
		}
		None => {
			// No finish_reason, check for tool calls
			if config.get_log_level().is_debug_enabled() {
				log_debug!("Debug: No finish_reason, checking for tool calls");
			}
			has_more_tools
		}
	}
}

// Handle cost tracking from follow-up API call
fn handle_follow_up_cost_tracking(
	chat_session: &mut ChatSession,
	exchange: &crate::session::ProviderExchange,
	config: &Config,
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

			if config.get_log_level().is_debug_enabled() {
				println!(
					"Debug: Adding ${:.5} from tool response API (total now: ${:.5})",
					cost, chat_session.session.info.total_cost
				);

				// Enhanced debug for follow-up calls
				println!("Debug: Tool response usage detail:");
				if let Ok(usage_str) = serde_json::to_string_pretty(usage) {
					println!("{}", usage_str);
				}

				// Check for cache-related fields
				if let Some(raw_usage) = exchange.response.get("usage") {
					println!("Debug: Raw tool response usage object:");
					if let Ok(raw_str) = serde_json::to_string_pretty(raw_usage) {
						println!("{}", raw_str);
					}

					// Look specifically for cache-related fields
					if let Some(cache_cost) = raw_usage.get("cache_cost") {
						println!("Found cache_cost field: {}", cache_cost);
					}

					if let Some(cached_cost) = raw_usage.get("cached_cost") {
						println!("Found cached_cost field: {}", cached_cost);
					}

					if let Some(any_cache) = raw_usage.get("cached") {
						println!("Found cached field: {}", any_cache);
					}
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

				if config.get_log_level().is_debug_enabled() {
					println!("Debug: Using cost ${:.5} from raw response for tool response (total now: ${:.5})",
						cost, chat_session.session.info.total_cost);
				}
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
				if config.get_log_level().is_debug_enabled() {
					if let Ok(resp_str) = serde_json::to_string_pretty(&exchange.response) {
						println!("Partial response JSON:\n{}", resp_str);
					}
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
