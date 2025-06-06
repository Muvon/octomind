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

// Anthropic provider implementation

use super::{AiProvider, ProviderExchange, ProviderResponse, TokenUsage};
use crate::config::Config;
use crate::log_debug;
use crate::session::Message;
use anyhow::Result;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::env;

/// Anthropic pricing constants (per 1M tokens in USD)
/// Source: https://docs.anthropic.com/en/docs/about-claude/models/overview (as of January 2025)
const PRICING: &[(&str, f64, f64)] = &[
	// Model, Input price per 1M tokens, Output price per 1M tokens
	// Claude 4 models
	("claude-opus-4-0", 15.00, 75.00),
	("claude-sonnet-4-0", 3.00, 15.00),
	// Claude 3.7 models
	("claude-3-7-sonnet", 3.00, 15.00),
	// Claude 3.5 models
	("claude-3-5-sonnet", 3.00, 15.00),
	("claude-3-5-haiku", 0.80, 4.00),
	// Claude 3 models
	("claude-3-opus", 15.00, 75.00),
	("claude-3-sonnet", 3.00, 15.00),
	("claude-3-haiku", 0.25, 1.25),
	// Legacy models
	("claude-2.1", 8.00, 24.00),
	("claude-2.0", 8.00, 24.00),
	("claude-instant-1.2", 0.80, 2.40),
];

/// Token usage breakdown for cache-aware pricing
struct CacheTokenUsage {
	regular_input_tokens: u64,
	cache_creation_tokens: u64,
	cache_creation_tokens_1h: u64, // 1h TTL cache creation tokens (2x price)
	cache_read_tokens: u64,
	output_tokens: u64,
}

/// Calculate cost for Anthropic models with cache-aware pricing
/// - cache_creation_tokens: charged at 1.25x normal price (5m cache)
/// - cache_creation_tokens_1h: charged at 2x normal price (1h cache)
/// - cache_read_tokens: charged at 0.1x normal price
/// - regular_input_tokens: charged at normal price
/// - output_tokens: charged at normal price
fn calculate_cost_with_cache(model: &str, usage: CacheTokenUsage) -> Option<f64> {
	for (pricing_model, input_price, output_price) in PRICING {
		if model.contains(pricing_model) {
			// Regular input tokens at normal price
			let regular_input_cost =
				(usage.regular_input_tokens as f64 / 1_000_000.0) * input_price;

			// Cache creation tokens at 1.25x price (25% more expensive) for 5m cache
			let cache_creation_cost =
				(usage.cache_creation_tokens as f64 / 1_000_000.0) * input_price * 1.25;

			// Cache creation tokens at 2x price (100% more expensive) for 1h cache
			let cache_creation_cost_1h =
				(usage.cache_creation_tokens_1h as f64 / 1_000_000.0) * input_price * 2.0;

			// Cache read tokens at 0.1x price (90% cheaper)
			let cache_read_cost =
				(usage.cache_read_tokens as f64 / 1_000_000.0) * input_price * 0.1;

			// Output tokens at normal price (never cached)
			let output_cost = (usage.output_tokens as f64 / 1_000_000.0) * output_price;

			return Some(
				regular_input_cost
					+ cache_creation_cost
					+ cache_creation_cost_1h
					+ cache_read_cost
					+ output_cost,
			);
		}
	}
	None
}

/// Anthropic provider implementation
pub struct AnthropicProvider;

impl Default for AnthropicProvider {
	fn default() -> Self {
		Self::new()
	}
}

impl AnthropicProvider {
	pub fn new() -> Self {
		Self
	}
}

// Constants
const ANTHROPIC_API_KEY_ENV: &str = "ANTHROPIC_API_KEY";
const ANTHROPIC_API_URL: &str = "https://api.anthropic.com/v1/messages";

/// Message format for the Anthropic API
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicMessage {
	pub role: String,
	pub content: serde_json::Value,
}

#[async_trait::async_trait]
impl AiProvider for AnthropicProvider {
	fn name(&self) -> &str {
		"anthropic"
	}

	fn supports_model(&self, model: &str) -> bool {
		// Anthropic Claude models
		model.starts_with("claude-") || model.contains("claude")
	}

	fn get_api_key(&self, _config: &Config) -> Result<String> {
		// API keys now only from environment variables for security
		match env::var(ANTHROPIC_API_KEY_ENV) {
			Ok(key) => Ok(key),
			Err(_) => Err(anyhow::anyhow!(
				"Anthropic API key not found in environment variable: {}",
				ANTHROPIC_API_KEY_ENV
			)),
		}
	}

	fn supports_caching(&self, _model: &str) -> bool {
		true
	}

	fn get_max_input_tokens(&self, model: &str) -> usize {
		// Anthropic model context window limits (what we can send as input)
		// These are the actual context windows - no output reservation needed
		// The API will handle output limits internally

		// Claude 4 models: 200K context window
		if model.contains("claude-opus-4") || model.contains("claude-sonnet-4") {
			return 200_000;
		}
		// Claude 3.7 models: 200K context window
		if model.contains("claude-3-7-sonnet") {
			return 200_000;
		}
		// Claude 3.5 models: 200K context window
		if model.contains("claude-3-5-sonnet") || model.contains("claude-3-5-haiku") {
			return 200_000;
		}
		// Claude 3 models: 200K context window
		if model.contains("claude-3-opus")
			|| model.contains("claude-3-sonnet")
			|| model.contains("claude-3-haiku")
		{
			return 200_000;
		}
		// Legacy models: 100K context window
		if model.contains("claude-2") || model.contains("claude-instant") {
			return 100_000;
		}
		// Default conservative limit
		100_000
	}

	async fn chat_completion(
		&self,
		messages: &[Message],
		model: &str,
		temperature: f32,
		config: &Config,
	) -> Result<ProviderResponse> {
		// Get API key
		let api_key = self.get_api_key(config)?;

		// Convert messages to Anthropic format with automatic cache markers
		let anthropic_messages = convert_messages(messages);

		// Extract system message if present and handle caching
		let system_message = messages
			.iter()
			.find(|m| m.role == "system")
			.map(|m| m.content.clone())
			.unwrap_or_else(|| "You are a helpful assistant.".to_string());

		let system_cached = messages.iter().any(|m| m.role == "system" && m.cached);

		// Create the request body
		let mut request_body = serde_json::json!({
			"model": model,
			"max_tokens": 32768,
			"messages": anthropic_messages,
			"temperature": temperature,
		});

		// Add system message with cache control if needed
		if system_cached {
			let ttl = if config.use_long_system_cache {
				"1h"
			} else {
				"5m"
			};
			request_body["system"] = serde_json::json!([{
				"type": "text",
				"text": system_message,
				"cache_control": {
					"type": "ephemeral",
					"ttl": ttl
				}
			}]);
		} else {
			request_body["system"] = serde_json::json!(system_message);
		}

		// Add tool definitions if MCP has any servers configured
		if !config.mcp.servers.is_empty() {
			let functions = crate::mcp::get_available_functions(config).await;
			if !functions.is_empty() {
				// CRITICAL FIX: Ensure tool definitions are ALWAYS in the same order
				// Sort functions by name to guarantee consistent ordering across API calls
				let mut sorted_functions = functions;
				sorted_functions.sort_by(|a, b| a.name.cmp(&b.name));

				let mut tools = sorted_functions
					.iter()
					.map(|f| {
						serde_json::json!({
							"name": f.name,
							"description": f.description,
							"input_schema": f.parameters
						})
					})
					.collect::<Vec<_>>();

				// CRITICAL FIX: Cache control should be handled consistently
				// Add cache control to the LAST tool definition ONLY if the model supports caching
				// and we actually want to cache tool definitions (check session state)
				if self.supports_caching(model) && !tools.is_empty() {
					// Check if any system message is cached - if so, we should cache tool definitions too
					let system_cached = messages
						.iter()
						.any(|msg| msg.role == "system" && msg.cached);

					if system_cached {
						if let Some(last_tool) = tools.last_mut() {
							last_tool["cache_control"] = serde_json::json!({
								"type": "ephemeral",
								"ttl": "1h"
							});
						}
					}
				}

				request_body["tools"] = serde_json::json!(tools);
			}
		}

		// Create HTTP client
		let client = Client::new();

		// Make the actual API request
		let response = client
			.post(ANTHROPIC_API_URL)
			.header("x-api-key", api_key)
			.header("Content-Type", "application/json")
			.header("anthropic-version", "2023-06-01")
			.header("anthropic-beta", "extended-cache-ttl-2025-04-11")
			.header("anthropic-beta", "token-efficient-tools-2025-02-19")
			.json(&request_body)
			.send()
			.await?;

		// Get response status
		let status = response.status();

		// Get response body as text first for debugging
		let response_text = response.text().await?;

		// Parse the text to JSON
		let response_json: serde_json::Value = match serde_json::from_str(&response_text) {
			Ok(json) => json,
			Err(e) => {
				return Err(anyhow::anyhow!(
					"Failed to parse response JSON: {}. Response: {}",
					e,
					response_text
				));
			}
		};

		// Handle error responses
		if !status.is_success() {
			let mut error_details = Vec::new();
			error_details.push(format!("HTTP {}", status));

			if let Some(error_obj) = response_json.get("error") {
				if let Some(msg) = error_obj.get("message").and_then(|m| m.as_str()) {
					error_details.push(format!("Message: {}", msg));
				}
				if let Some(error_type) = error_obj.get("type").and_then(|t| t.as_str()) {
					error_details.push(format!("Type: {}", error_type));
				}
			}

			if error_details.len() == 1 {
				error_details.push(format!("Raw response: {}", response_text));
			}

			let full_error = error_details.join(" | ");
			return Err(anyhow::anyhow!("Anthropic API error: {}", full_error));
		}

		// Extract content from response
		let mut content = String::new();
		let mut tool_calls = None;

		if let Some(content_array) = response_json.get("content").and_then(|c| c.as_array()) {
			for content_block in content_array {
				if let Some(text) = content_block.get("text").and_then(|t| t.as_str()) {
					content.push_str(text);
				} else if content_block.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
					// Handle tool calls
					if tool_calls.is_none() {
						tool_calls = Some(Vec::new());
					}

					if let (Some(name), Some(input), Some(id)) = (
						content_block.get("name").and_then(|n| n.as_str()),
						content_block.get("input"),
						content_block.get("id").and_then(|i| i.as_str()),
					) {
						let mcp_call = crate::mcp::McpToolCall {
							tool_name: name.to_string(),
							parameters: input.clone(),
							tool_id: id.to_string(),
						};

						if let Some(ref mut calls) = tool_calls {
							calls.push(mcp_call);
						}
					}
				}
			}
		}

		// Extract finish_reason
		let finish_reason = response_json
			.get("stop_reason")
			.and_then(|fr| fr.as_str())
			.map(|s| s.to_string());

		if let Some(ref reason) = finish_reason {
			log_debug!("Stop reason: {}", reason);
		}

		// Extract token usage with cache-aware pricing
		let usage: Option<TokenUsage> = if let Some(usage_obj) = response_json.get("usage") {
			let input_tokens = usage_obj
				.get("input_tokens")
				.and_then(|v| v.as_u64())
				.unwrap_or(0);
			let output_tokens = usage_obj
				.get("output_tokens")
				.and_then(|v| v.as_u64())
				.unwrap_or(0);

			// Parse cache-specific token fields from Anthropic API
			let cache_creation_input_tokens = usage_obj
				.get("cache_creation_input_tokens")
				.and_then(|v| v.as_u64())
				.unwrap_or(0);
			let cache_read_input_tokens = usage_obj
				.get("cache_read_input_tokens")
				.and_then(|v| v.as_u64())
				.unwrap_or(0);

			// CORRECTED: According to Anthropic API docs:
			// - input_tokens: Regular input tokens that were NOT cached (normal price)
			// - cache_creation_input_tokens: Tokens used to CREATE cache (1.25x price) - NOT cached tokens
			// - cache_read_input_tokens: Tokens READ from cache (0.1x price) - these ARE cached tokens
			//
			// For display:
			// - prompt_tokens should be: input_tokens + cache_creation_input_tokens + cache_read_input_tokens
			// - cached_tokens should ONLY be: cache_read_input_tokens
			// - This way we show the total prompt tokens processed, with breakdown of what was cached

			let cached_tokens = cache_read_input_tokens; // Only cache reads are "cached"

			// For cost calculation, we need to separate the different token types
			let regular_input_tokens = input_tokens; // These are already regular tokens from API

			// Calculate cost with cache-aware pricing
			let cost = calculate_cost_with_cache(
				model,
				CacheTokenUsage {
					regular_input_tokens,
					cache_creation_tokens: cache_creation_input_tokens,
					cache_creation_tokens_1h: 0, // TODO: Distinguish 1h vs 5m cache tokens from API
					cache_read_tokens: cache_read_input_tokens,
					output_tokens,
				},
			);

			Some(TokenUsage {
				prompt_tokens: input_tokens + cache_creation_input_tokens + cache_read_input_tokens, // Total prompt tokens processed
				output_tokens,
				total_tokens: input_tokens
					+ cache_creation_input_tokens
					+ cache_read_input_tokens
					+ output_tokens,
				cached_tokens,         // Only cache_read_input_tokens are truly "cached"
				cost,                  // Pre-calculated with proper cache pricing
				request_time_ms: None, // TODO: Add API timing for Anthropic
			})
		} else {
			None
		};

		// CRITICAL FIX: Store the original content array for proper tool_use reconstruction
		// This ensures tool_result messages can reference the correct tool_use_id
		let stored_tool_calls = if tool_calls.is_some() {
			// If we found tool_use blocks, store the complete content array
			// This preserves both text content and tool_use blocks for conversation history
			response_json.get("content").cloned()
		} else {
			None
		};

		// Create exchange record
		let mut exchange = ProviderExchange::new(request_body, response_json, usage, self.name());

		// CRITICAL FIX: Store the original tool calls in the exchange for later reconstruction
		if let Some(ref content_array) = stored_tool_calls {
			exchange.response["tool_calls_content"] = content_array.clone();
		}

		Ok(ProviderResponse {
			content,
			exchange,
			tool_calls,
			finish_reason,
		})
	}
}

// Convert our session messages to Anthropic format
fn convert_messages(messages: &[Message]) -> Vec<AnthropicMessage> {
	// Cache markers should already be properly set by session logic
	// We just need to respect them when converting to API format
	let mut result = Vec::new();

	for msg in messages {
		// Skip system messages as they're handled separately in Anthropic API
		if msg.role == "system" {
			continue;
		}

		// Handle all message types with simplified structure
		match msg.role.as_str() {
			"tool" => {
				// Tool messages in Anthropic format
				let tool_call_id = msg.tool_call_id.clone().unwrap_or_default();

				let mut tool_content = serde_json::json!({
					"type": "tool_result",
					"tool_use_id": tool_call_id,
					"content": msg.content
				});

				// Add cache_control if needed
				if msg.cached {
					tool_content["cache_control"] = serde_json::json!({
						"type": "ephemeral"
					});
				}

				result.push(AnthropicMessage {
					role: "user".to_string(),
					content: serde_json::json!([tool_content]),
				});
			}
			"user" => {
				// Handle legacy <fnr> format for backwards compatibility
				if msg.content.starts_with("<fnr>") && msg.content.ends_with("</fnr>") {
					let content = msg
						.content
						.trim_start_matches("<fnr>")
						.trim_end_matches("</fnr>")
						.trim();

					if let Ok(tool_responses) =
						serde_json::from_str::<Vec<serde_json::Value>>(content)
					{
						if !tool_responses.is_empty()
							&& tool_responses[0]
								.get("role")
								.is_some_and(|r| r.as_str().unwrap_or("") == "tool")
						{
							for tool_response in tool_responses {
								let tool_call_id = tool_response
									.get("tool_call_id")
									.and_then(|id| id.as_str())
									.unwrap_or("");

								let content_text = tool_response
									.get("content")
									.and_then(|c| c.as_str())
									.unwrap_or("");

								result.push(AnthropicMessage {
									role: "user".to_string(),
									content: serde_json::json!([{
										"type": "tool_result",
										"tool_use_id": tool_call_id,
										"content": content_text
									}]),
								});
							}
							continue;
						}
					}
				}

				// Regular user messages with proper structure
				// CRITICAL FIX: Only create message if content is not empty
				if !msg.content.trim().is_empty() {
					let mut text_content = serde_json::json!({
						"type": "text",
						"text": msg.content
					});

					// Add cache_control if needed
					if msg.cached {
						text_content["cache_control"] = serde_json::json!({
							"type": "ephemeral"
						});
					}

					result.push(AnthropicMessage {
						role: msg.role.clone(),
						content: serde_json::json!([text_content]),
					});
				}
			}
			"assistant" => {
				// Assistant messages with proper structure
				let mut content_blocks = Vec::new();

				// Add text content if not empty
				if !msg.content.is_empty() {
					let mut text_content = serde_json::json!({
						"type": "text",
						"text": msg.content
					});

					// Add cache_control if needed
					if msg.cached {
						text_content["cache_control"] = serde_json::json!({
							"type": "ephemeral"
						});
					}

					content_blocks.push(text_content);
				}

				// CRITICAL FIX: Preserve tool_use blocks from original API response
				// This ensures tool_result messages can reference the correct tool_use_id
				if let Some(ref tool_calls_data) = msg.tool_calls {
					// Handle tool calls from Anthropic API format
					if let Some(content_array) =
						tool_calls_data.get("content").and_then(|c| c.as_array())
					{
						// If tool_calls contains Anthropic format content blocks, extract tool_use blocks
						for content_block in content_array {
							if content_block.get("type").and_then(|t| t.as_str())
								== Some("tool_use")
							{
								content_blocks.push(content_block.clone());
							}
						}
					} else if tool_calls_data.is_array() {
						// Handle OpenRouter/OpenAI format tool calls - convert to Anthropic format
						if let Some(calls_array) = tool_calls_data.as_array() {
							for tool_call in calls_array {
								if let Some(function) = tool_call.get("function") {
									if let (Some(name), Some(args_str), Some(id)) = (
										function.get("name").and_then(|n| n.as_str()),
										function.get("arguments").and_then(|a| a.as_str()),
										tool_call.get("id").and_then(|i| i.as_str()),
									) {
										// Parse arguments string to JSON
										let input = if args_str.trim().is_empty() {
											serde_json::json!({})
										} else {
											match serde_json::from_str::<serde_json::Value>(
												args_str,
											) {
												Ok(json_args) => json_args,
												Err(_) => {
													serde_json::json!({"arguments": args_str})
												}
											}
										};

										// Create Anthropic format tool_use block
										let tool_use_block = serde_json::json!({
											"type": "tool_use",
											"id": id,
											"name": name,
											"input": input
										});

										content_blocks.push(tool_use_block);
									}
								} else if let (Some(id), Some(name)) = (
									tool_call.get("id").and_then(|i| i.as_str()),
									tool_call.get("name").and_then(|n| n.as_str()),
								) {
									// Direct Anthropic format
									let input = tool_call
										.get("input")
										.cloned()
										.unwrap_or_else(|| serde_json::json!({}));

									let tool_use_block = serde_json::json!({
										"type": "tool_use",
										"id": id,
										"name": name,
										"input": input
									});

									content_blocks.push(tool_use_block);
								}
							}
						}
					}
				}

				// CRITICAL FIX: Only push the message if it has content
				// This prevents empty assistant messages from being sent to the API
				if !content_blocks.is_empty() {
					result.push(AnthropicMessage {
						role: msg.role.clone(),
						content: serde_json::json!(content_blocks),
					});
				}
			}
			_ => {
				// All other message types with proper structure
				// CRITICAL FIX: Only create message if content is not empty
				if !msg.content.trim().is_empty() {
					let mut text_content = serde_json::json!({
						"type": "text",
						"text": msg.content
					});

					// Add cache_control if needed
					if msg.cached {
						text_content["cache_control"] = serde_json::json!({
							"type": "ephemeral"
						});
					}

					result.push(AnthropicMessage {
						role: msg.role.clone(),
						content: serde_json::json!([text_content]),
					});
				}
			}
		}
	}

	result
}
