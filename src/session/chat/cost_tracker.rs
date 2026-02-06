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

// Cost tracking module - extracted from response.rs for better modularity

use crate::config::Config;
use crate::log_debug;
use crate::session::chat::session::ChatSession;
use crate::session::ProviderExchange;
use anyhow::Result;

pub struct CostTracker;

impl CostTracker {
	/// Handle cost and token tracking from a provider exchange
	pub fn track_exchange_cost(
		chat_session: &mut ChatSession,
		exchange: &ProviderExchange,
		_config: &Config,
	) -> Result<()> {
		if let Some(usage) = &exchange.usage {
			// Simple token extraction with clean provider interface
			let cached_tokens = usage.cached_tokens;
			let regular_prompt_tokens = usage.prompt_tokens.saturating_sub(cached_tokens);

			// Track API time if available
			if let Some(api_time_ms) = usage.request_time_ms {
				chat_session.session.info.total_api_time_ms += api_time_ms;
			}

			// Update session token counts using cache manager
			let cache_manager = crate::session::cache::CacheManager::new();
			cache_manager.update_token_tracking(
				&mut chat_session.session,
				regular_prompt_tokens,
				usage.output_tokens,
				cached_tokens,
			);

			// Update cost
			if let Some(cost) = usage.cost {
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
			}
		}

		Ok(())
	}

	/// Display short, single-line cost information
	pub fn display_cost_line(chat_session: &ChatSession) {
		use crate::log_info;

		let total_cost = chat_session.session.info.total_cost;
		if total_cost > 0.0 {
			log_info!(
				" ── cost: ${:.5} ────────────────────────────────────────",
				total_cost
			);
		}
	}

	/// Display detailed cost breakdown for intermediate results (after tool calls)
	pub fn display_intermediate_cost_breakdown(chat_session: &ChatSession) {
		use crate::log_info;

		let total_cost = chat_session.session.info.total_cost;
		if total_cost <= 0.0 {
			return; // No cost to show
		}

		// Show cost breakdown after tool execution, before next AI call
		log_info!(
			" ── cost: ${:.5} ────────────────────────────────────────",
			total_cost
		);

		// Show detailed breakdown
		Self::display_cost_breakdown(chat_session);
	}

	/// Display compression result with consistent formatting
	///
	/// Formats compression messages to match cost display style with separator lines.
	/// Uses type-specific colors for visual distinction.
	///
	/// # Arguments
	/// * `compression_type` - Type of compression: "Task", "Phase", "Project", "Conversation"
	/// * `metrics` - Compression metrics (messages removed, tokens saved, ratio)
	pub fn display_compression_result(
		compression_type: &str,
		metrics: &crate::mcp::dev::plan::compression::CompressionMetrics,
	) {
		use crate::log_info;
		use colored::Colorize;

		// Type-specific colors for visual distinction
		let type_label = match compression_type {
			"Task" => "task".bright_cyan(),
			"Phase" => "phase".bright_magenta(),
			"Project" => "project".bright_yellow(),
			"Conversation" => "conversation".bright_green(),
			_ => compression_type.bright_white(),
		};

		// Format numbers with proper styling
		let msgs_removed = format!("{}", metrics.messages_removed).bright_white();
		let tokens_saved = format!("{}", metrics.tokens_saved).bright_green();
		let ratio_pct = format!("{:.1}%", metrics.compression_ratio * 100.0).bright_yellow();

		// Display with consistent separator style matching cost display
		log_info!(
			" ── {} compression: {} msgs → {} tokens saved ({} reduction) ──",
			type_label,
			msgs_removed,
			tokens_saved,
			ratio_pct
		);
	}

	/// Display session usage statistics
	pub fn display_session_usage(chat_session: &ChatSession) {
		use crate::log_info;
		use crate::session::chat::formatting::format_duration;

		println!();

		log_info!(
			"{}",
			"── session usage ────────────────────────────────────────"
		);

		// Format token usage with cached tokens
		let cached = chat_session.session.info.cached_tokens;
		let non_cached_prompt = chat_session.session.info.input_tokens;
		let completion = chat_session.session.info.output_tokens;

		// FIXED: Show total prompt tokens (cached + non-cached) as "prompt"
		// This matches user expectation that prompt tokens should show the actual tokens processed
		let total_prompt = non_cached_prompt + cached;
		let total = total_prompt + completion;

		log_info!(
			"tokens: {} prompt ({} cached), {} completion, {} total, ${:.5}",
			total_prompt,
			cached,
			completion,
			total,
			chat_session.session.info.total_cost
		);

		// If we have cached tokens, show the savings percentage
		if cached > 0 {
			let saving_pct = (cached as f64 / total_prompt as f64) * 100.0;
			log_info!(
				"cached: {:.1}% of prompt tokens ({} tokens saved)",
				saving_pct,
				cached
			);
		}

		// Show cost breakdown
		Self::display_cost_breakdown(chat_session);

		// Show time information if available
		let total_time_ms = chat_session.session.info.total_api_time_ms
			+ chat_session.session.info.total_tool_time_ms
			+ chat_session.session.info.total_layer_time_ms;
		if total_time_ms > 0 {
			log_info!(
				"time: {} (API: {}, Tools: {}, Processing: {})",
				format_duration(total_time_ms),
				format_duration(chat_session.session.info.total_api_time_ms),
				format_duration(chat_session.session.info.total_tool_time_ms),
				format_duration(chat_session.session.info.total_layer_time_ms)
			);
		}

		println!();
	}

	/// Display detailed cost breakdown
	fn display_cost_breakdown(chat_session: &ChatSession) {
		use crate::log_info;

		let total_cost = chat_session.session.info.total_cost;
		if total_cost <= 0.0 {
			return; // No cost to break down
		}

		let cached = chat_session.session.info.cached_tokens;
		let non_cached_prompt = chat_session.session.info.input_tokens;
		let completion = chat_session.session.info.output_tokens;
		let total_tokens = non_cached_prompt + cached + completion;

		if total_tokens == 0 {
			return; // Avoid division by zero
		}

		// Estimate cost breakdown based on typical pricing patterns
		// Most providers charge more for output tokens than input tokens
		// Cached tokens are typically free or heavily discounted
		let estimated_input_cost = if non_cached_prompt > 0 {
			// Estimate input cost as proportional to tokens, assuming typical 1:3 input:output ratio
			let input_weight = 1.0;
			let output_weight = 3.0; // Output tokens typically cost 3x more
			let total_weighted =
				(non_cached_prompt as f64 * input_weight) + (completion as f64 * output_weight);
			if total_weighted > 0.0 {
				total_cost * (non_cached_prompt as f64 * input_weight) / total_weighted
			} else {
				0.0
			}
		} else {
			0.0
		};

		let estimated_output_cost = total_cost - estimated_input_cost;
		let cached_savings = if cached > 0 {
			// Estimate savings from cached tokens (assuming they would cost same as input tokens)
			let input_weight = 1.0;
			let output_weight = 3.0;
			let total_weighted =
				(non_cached_prompt as f64 * input_weight) + (completion as f64 * output_weight);
			if total_weighted > 0.0 && non_cached_prompt > 0 {
				let estimated_input_rate = estimated_input_cost / non_cached_prompt as f64;
				cached as f64 * estimated_input_rate
			} else {
				0.0
			}
		} else {
			0.0
		};

		// Display cost breakdown
		if non_cached_prompt > 0 && completion > 0 {
			log_info!(
				"cost: ${:.5} total (input: ${:.5}, output: ${:.5}{})",
				total_cost,
				estimated_input_cost,
				estimated_output_cost,
				if cached_savings > 0.0 {
					format!(", saved: ${:.5}", cached_savings)
				} else {
					String::new()
				}
			);
		} else if non_cached_prompt > 0 {
			log_info!(
				"cost: ${:.5} total (input: ${:.5}{})",
				total_cost,
				total_cost,
				if cached_savings > 0.0 {
					format!(", saved: ${:.5}", cached_savings)
				} else {
					String::new()
				}
			);
		} else if completion > 0 {
			log_info!(
				"cost: ${:.5} total (output: ${:.5})",
				total_cost,
				total_cost
			);
		} else {
			log_info!("cost: ${:.5}", total_cost);
		}
	}
}
