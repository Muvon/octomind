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
			let cache_read_tokens = usage.cache_read_tokens;
			let cache_write_tokens = usage.cache_write_tokens;

			// Track API time if available
			if let Some(api_time_ms) = usage.request_time_ms {
				chat_session.session.info.total_api_time_ms += api_time_ms;
			}

			// Every exchange with usage data = one completed API call.
			// This is the single authoritative increment point for the initial call path
			// (api_executor → CostTracker). Follow-up calls go through messages.rs which
			// also increments here via add_assistant_message, so both paths are covered.
			chat_session.session.info.total_api_calls += 1;

			// Update session token counts using cache manager
			let cache_manager = crate::session::cache::CacheManager::new();
			cache_manager.update_token_tracking(
				&mut chat_session.session,
				usage.input_tokens, // Non-cached input tokens
				usage.output_tokens,
				cache_read_tokens,
				cache_write_tokens,
				usage.reasoning_tokens,
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
			}
		}

		Ok(())
	}

	/// Compact cost snapshot at a turn boundary (end of response, Ctrl+C
	/// cancellation). Delegates to the full breakdown so the user sees the
	/// same `· $X · input $Y · output $Z` (or `· $X · N in · N out` for
	/// unknown-pricing models) on every cost-emit point.
	pub fn display_cost_line(chat_session: &ChatSession) {
		let total_cost = chat_session.session.info.total_cost;
		if total_cost > 0.0 {
			Self::display_cost_breakdown(chat_session);
		}
	}

	/// Show cost + breakdown on a single line after tool execution. Replaces
	/// the previous two-line banner+breakdown (`── cost: $X ──` + `cost: $X
	/// total (…)`) which was noisy and redundant.
	pub fn display_intermediate_cost_breakdown(chat_session: &ChatSession) {
		let total_cost = chat_session.session.info.total_cost;
		if total_cost <= 0.0 {
			return;
		}
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
		metrics: &crate::mcp::core::plan::compression::CompressionMetrics,
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
		let cache_read = chat_session.session.info.cache_read_tokens;
		let cache_write = chat_session.session.info.cache_write_tokens;
		let non_cached_prompt = chat_session.session.info.input_tokens;
		let completion = chat_session.session.info.output_tokens;
		let reasoning = chat_session.session.info.reasoning_tokens;

		// FIXED: Show total prompt tokens (cached + non-cached) as "prompt"
		// This matches user expectation that prompt tokens should show the actual tokens processed
		let total_prompt = non_cached_prompt + cache_read;
		let total = total_prompt + completion + reasoning;

		// Build token display string
		let mut token_parts = vec![
			format!("{} prompt ({} cache read)", total_prompt, cache_read),
			format!("{} completion", completion),
		];

		// Add cache write tokens if present (Anthropic-style cache creation)
		if cache_write > 0 {
			token_parts.push(format!("{} cache write", cache_write));
		}

		// Add reasoning tokens if present
		if reasoning > 0 {
			token_parts.push(format!("{} reasoning", reasoning));
		}

		log_info!(
			"tokens: {}, {} total, ${:.5}",
			token_parts.join(", "),
			total,
			chat_session.session.info.total_cost
		);

		// If we have cache read tokens, show the savings percentage
		if cache_read > 0 {
			let saving_pct = (cache_read as f64 / total_prompt as f64) * 100.0;
			log_info!(
				"cache read: {:.1}% of prompt tokens ({} tokens saved)",
				saving_pct,
				cache_read
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

	/// Display detailed cost breakdown using real per-token pricing.
	///
	/// Looks up the model's pricing tuple (input / output / cache_write / cache_read
	/// per 1M tokens) from octolib's reference pricing table and computes each line
	/// item from the actual tracked token counts. The "saved" figure is the genuine
	/// difference between what cache reads would have cost at the full input rate
	/// vs. what they actually cost at the cache-read rate -- not an estimate.
	///
	/// If the model isn't in the pricing table (custom / local / unknown provider),
	/// only the authoritative `total_cost` (set by the provider via `usage.cost`) is
	/// shown -- no fabricated breakdown.
	fn display_cost_breakdown(chat_session: &ChatSession) {
		use crate::log_info;

		let total_cost = chat_session.session.info.total_cost;
		let info = &chat_session.session.info;
		let non_cached_input = info.input_tokens;
		let cache_read = info.cache_read_tokens;
		let cache_write = info.cache_write_tokens;
		let output = info.output_tokens;

		if non_cached_input + cache_read + cache_write + output == 0 {
			return;
		}

		// Strip provider prefix ("anthropic:claude-opus-4-7" -> "claude-opus-4-7").
		// get_reference_pricing tolerates either form, but stripping makes the match
		// path predictable across providers.
		let model_for_lookup = info
			.model
			.split_once(':')
			.map(|(_, m)| m)
			.unwrap_or(&info.model);

		let pricing = octolib::llm::reference_pricing::get_reference_pricing(model_for_lookup);

		let Some(pricing) = pricing else {
			use colored::Colorize;
			// Unknown model — no pricing table, so we can't split cost by
			// component. But token counts are still a useful state snapshot,
			// so we surface them: `· $X · N in · N out [· N cache]`.
			let dot = "·".bright_black();
			let in_total = non_cached_input + cache_read;
			let in_label = crate::session::chat::format_number(in_total);
			let out_label = crate::session::chat::format_number(output);
			if cache_read > 0 || cache_write > 0 {
				let cache_label = crate::session::chat::format_number(cache_read + cache_write);
				log_info!(
					"{} ${:.5} {} {} in {} {} out {} {} cache",
					dot,
					total_cost,
					dot,
					in_label,
					dot,
					out_label,
					dot,
					cache_label
				);
			} else {
				log_info!(
					"{} ${:.5} {} {} in {} {} out",
					dot,
					total_cost,
					dot,
					in_label,
					dot,
					out_label
				);
			}
			return;
		};

		// Real per-component cost from the pricing table.
		let per_million = |tokens: u64, rate: f64| (tokens as f64 / 1_000_000.0) * rate;
		let input_cost = per_million(non_cached_input, pricing.input_price_per_1m);
		let output_cost = per_million(output, pricing.output_price_per_1m);
		let cache_write_cost = per_million(cache_write, pricing.cache_write_price_per_1m);
		let cache_read_cost = per_million(cache_read, pricing.cache_read_price_per_1m);

		// Genuine savings: what cache reads would have cost at the full input rate
		// minus what they actually cost at the cache-read rate.
		let cache_savings = per_million(
			cache_read,
			pricing.input_price_per_1m - pricing.cache_read_price_per_1m,
		)
		.max(0.0);

		// Compact breakdown: `· $0.48 total · input $0.05 · output $0.22 …`
		// Each part separated by middle dots; no parens, no commas.
		use colored::Colorize;
		let dot = "·".bright_black();
		let mut parts: Vec<String> = Vec::with_capacity(5);
		if non_cached_input > 0 {
			parts.push(format!("input ${:.5}", input_cost));
		}
		if output > 0 {
			parts.push(format!("output ${:.5}", output_cost));
		}
		if cache_write > 0 {
			parts.push(format!("cache write ${:.5}", cache_write_cost));
		}
		if cache_read > 0 {
			parts.push(format!("cache read ${:.5}", cache_read_cost));
		}
		if cache_savings > 0.0 {
			parts.push(format!("saved ${:.5}", cache_savings));
		}

		let joiner = format!(" {} ", dot);
		if parts.is_empty() {
			log_info!("{} ${:.5}", dot, total_cost);
		} else {
			log_info!("{} ${:.5} {} {}", dot, total_cost, dot, parts.join(&joiner));
		}
	}
}
