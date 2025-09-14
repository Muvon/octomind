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

		// Use the type-safe tool call extraction
		match octolib::ProviderToolCalls::extract_from_exchange(exchange) {
			Ok(Some(provider_calls)) => {
				// Convert to unified GenericToolCall format
				match provider_calls {
					octolib::ProviderToolCalls::Anthropic { content } => {
						let generic_calls: Vec<octolib::GenericToolCall> = content
							.into_iter()
							.map(|tool_use| octolib::GenericToolCall {
								id: tool_use.id,
								name: tool_use.name,
								arguments: tool_use.input,
							})
							.collect();
						Some(serde_json::to_value(&generic_calls).unwrap_or_default())
					}
					octolib::ProviderToolCalls::OpenAI { tool_calls }
					| octolib::ProviderToolCalls::OpenRouter { tool_calls }
					| octolib::ProviderToolCalls::DeepSeek { tool_calls } => {
						let generic_calls: Vec<octolib::GenericToolCall> = tool_calls
							.into_iter()
							.map(|tc| {
								let arguments = if tc.function.arguments.trim().is_empty() {
									serde_json::json!({})
								} else {
									match serde_json::from_str::<serde_json::Value>(
										&tc.function.arguments,
									) {
										Ok(json_args) => json_args,
										Err(_) => {
											serde_json::json!({"raw_arguments": tc.function.arguments})
										}
									}
								};
								octolib::GenericToolCall {
									id: tc.id,
									name: tc.function.name,
									arguments,
								}
							})
							.collect();
						Some(serde_json::to_value(&generic_calls).unwrap_or_default())
					}
					octolib::ProviderToolCalls::Generic { calls } => {
						Some(serde_json::to_value(&calls).unwrap_or_default())
					}
				}
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
			tool_call_id: None,
			name: None,
			tool_calls: original_tool_calls, // Store the original tool_calls for proper reconstruction
			images: None,
		};

		// Add the assistant message to the session
		chat_session.session.messages.push(assistant_message);

		// Update last response
		chat_session.last_response = content.to_string();

		Ok(())
	}

	/// Log assistant response and exchange data
	pub fn log_response_data(
		session_name: &str,
		content: &str,
		exchange: &ProviderExchange,
	) -> Result<()> {
		let _ = crate::session::logger::log_assistant_response(session_name, content);
		let _ = crate::session::logger::log_raw_exchange(exchange);
		Ok(())
	}
}
