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
use crate::session::chat::session::ChatSession;
use crate::session::ProviderExchange;
use crate::log_debug;
use anyhow::Result;

pub struct CostTracker;

impl CostTracker {
    /// Handle cost and token tracking from a provider exchange
    pub fn track_exchange_cost(
        chat_session: &mut ChatSession,
        exchange: &ProviderExchange,
        config: &Config,
    ) -> Result<()> {
        if let Some(usage) = &exchange.usage {
            // Calculate regular and cached tokens
            let mut regular_prompt_tokens = usage.prompt_tokens;
            let mut cached_tokens = 0;

            // Check prompt_tokens_details for cached_tokens first
            if let Some(details) = &usage.prompt_tokens_details {
                if let Some(serde_json::Value::Number(num)) = details.get("cached_tokens") {
                    if let Some(num_u64) = num.as_u64() {
                        cached_tokens = num_u64;
                        regular_prompt_tokens = usage.prompt_tokens.saturating_sub(cached_tokens);
                    }
                }
            }

            // Fall back to breakdown field
            if cached_tokens == 0 && usage.prompt_tokens > 0 {
                if let Some(breakdown) = &usage.breakdown {
                    if let Some(serde_json::Value::Number(num)) = breakdown.get("cached") {
                        if let Some(num_u64) = num.as_u64() {
                            cached_tokens = num_u64;
                            regular_prompt_tokens = usage.prompt_tokens.saturating_sub(cached_tokens);
                        }
                    }
                }
            }

            // Track API time if available
            if let Some(api_time_ms) = usage.request_time_ms {
                chat_session.session.info.total_api_time_ms += api_time_ms;
            }

            // Update session token counts using cache manager
            let cache_manager = crate::session::cache::CacheManager::new();
            cache_manager.update_token_tracking(
                &mut chat_session.session,
                regular_prompt_tokens,
                usage.completion_tokens,
                cached_tokens,
            );

            // Update cost
            if let Some(cost) = usage.cost {
                chat_session.session.info.total_cost += cost;
                chat_session.estimated_cost = chat_session.session.info.total_cost;

                if config.get_log_level().is_debug_enabled() {
                    log_debug!(
                        "Adding ${:.5} from API (total now: ${:.5})",
                        cost,
                        chat_session.session.info.total_cost
                    );
                }
            }
        }

        Ok(())
    }

    /// Display session usage statistics
    pub fn display_session_usage(chat_session: &ChatSession) {
        use crate::session::chat::formatting::format_duration;
        use crate::log_info;

        println!();

        log_info!(
            "{}",
            "── session usage ────────────────────────────────────────"
        );

        // Format token usage with cached tokens
        let cached = chat_session.session.info.cached_tokens;
        let prompt = chat_session.session.info.input_tokens;
        let completion = chat_session.session.info.output_tokens;
        let total = prompt + completion + cached;

        log_info!(
            "tokens: {} prompt ({} cached), {} completion, {} total, ${:.5}",
            prompt,
            cached,
            completion,
            total,
            chat_session.session.info.total_cost
        );

        // If we have cached tokens, show the savings percentage
        if cached > 0 {
            let saving_pct = (cached as f64 / (prompt + cached) as f64) * 100.0;
            log_info!(
                "cached: {:.1}% of prompt tokens ({} tokens saved)",
                saving_pct,
                cached
            );
        }

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
}