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

// Message handling module - extracted from response.rs for better modularity

use crate::session::chat::session::ChatSession;
use crate::session::ProviderExchange;
use anyhow::Result;

pub struct MessageHandler;

impl MessageHandler {
	/// Extract original tool calls from provider exchange with type safety
	pub fn extract_original_tool_calls(exchange: &ProviderExchange) -> Option<serde_json::Value> {
		// Check for unified format first (new clean approach)
		if let Some(tool_calls) = exchange.response.get("tool_calls") {
			return Some(tool_calls.clone());
		}

		// Use octolib's conversion method for provider-specific formats
		match octolib::ProviderToolCalls::extract_from_exchange(exchange) {
			Ok(Some(provider_calls)) => {
				// Convert to unified GenericToolCall format using octolib
				let generic_calls = provider_calls.to_generic_tool_calls();
				Some(serde_json::to_value(&generic_calls).unwrap_or_default())
			}
			Ok(None) => None,
			Err(_) => None, // No fallback - unified format is mandatory
		}
	}

	/// Add assistant message with tool calls preserved
	pub fn add_assistant_message_with_tool_calls(
		chat_session: &mut ChatSession,
		content: &str,
		exchange: &ProviderExchange,
		response_id: Option<String>,
	) -> Result<()> {
		// Extract the original tool_calls from the exchange response based on provider
		let original_tool_calls = Self::extract_original_tool_calls(exchange);

		// Create the assistant message directly with tool_calls preserved from the exchange
		let assistant_message = crate::session::Message {
			role: "assistant".to_string(),
			content: content.to_string(),
			timestamp: std::time::SystemTime::now()
				.duration_since(std::time::UNIX_EPOCH)
				.unwrap_or_default()
				.as_secs(),
			cached: false,
			cache_ttl: None,
			tool_call_id: None,
			name: None,
			tool_calls: original_tool_calls,
			images: None,
			videos: None,
			thinking: None,
			id: response_id,
		};

		// ATOMIC ADD: persist BEFORE pushing to in-memory Vec.
		// If persist fails, `?` propagates with clean memory state — prevents orphaned
		// assistant(tool_calls=...) without matching tool_results. Token/cost
		// bookkeeping only runs after the message is durably recorded.
		if let Some(session_file) = &chat_session.session.session_file {
			let message_json = serde_json::to_string(&assistant_message)?;
			crate::session::append_to_session_file(session_file, &message_json)?;
		}
		chat_session.session.messages.push(assistant_message);

		// Update last response
		chat_session.last_response = content.to_string();

		// CRITICAL FIX: Track API call and tokens (same logic as add_assistant_message)
		// This was missing, causing api_calls=0 in compression analysis
		if let Some(usage) = &exchange.usage {
			// Track API time if available
			if let Some(api_time_ms) = usage.request_time_ms {
				chat_session.session.info.total_api_time_ms += api_time_ms;
			}

			// CACHE-AWARE COMPRESSION: Track API calls for amortized cost analysis
			chat_session.session.info.total_api_calls += 1;

			// Update token counts using cache manager with octolib data directly
			let cache_manager = crate::session::cache::CacheManager::new();
			cache_manager.update_token_tracking(
				&mut chat_session.session,
				usage.input_tokens, // Non-cached input tokens from API
				usage.output_tokens,
				usage.cache_read_tokens,
				usage.cache_write_tokens,
				usage.reasoning_tokens,
			);

			// Track cost if available
			if let Some(cost) = usage.cost {
				chat_session.session.info.total_cost += cost;
			}
		}

		Ok(())
	}
}
