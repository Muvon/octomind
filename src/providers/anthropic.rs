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

use super::{AiProvider, ChatCompletionParams, ProviderExchange, ProviderResponse, TokenUsage};
use crate::config::Config;
use crate::providers::retry;
use crate::session::Message;
use anyhow::Result;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::env;

/// Anthropic pricing constants (per 1M tokens in USD)
/// Source: https://docs.anthropic.com/en/docs/about-claude/pricing (as of June 2025)
const PRICING: &[(&str, f64, f64)] = &[
	// Model, Input price per 1M tokens, Output price per 1M tokens
	("claude-opus-4-0", 15.00, 75.00),
	("claude-sonnet-4-0", 3.00, 15.00),
	("claude-3-7-sonnet", 3.00, 15.00),
	("claude-3-5-sonnet", 3.00, 15.00),
	("claude-3-5-haiku", 0.80, 4.00),
	("claude-3-opus", 15.00, 75.00),
	("claude-3-sonnet", 3.00, 15.00),
	("claude-3-haiku", 0.25, 1.25),
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

			let total_cost = regular_input_cost
				+ cache_creation_cost
				+ cache_creation_cost_1h
				+ cache_read_cost
				+ output_cost;

			// Debug: Log detailed cost calculation breakdown
			crate::log_debug!(
				"Anthropic detailed cost calculation for {}: Regular input: ${:.8} ({} tokens @ ${:.2}/1M), Cache creation 5m: ${:.8} ({} tokens @ ${:.2}/1M), Cache creation 1h: ${:.8} ({} tokens @ ${:.2}/1M), Cache read: ${:.8} ({} tokens @ ${:.2}/1M), Output: ${:.8} ({} tokens @ ${:.2}/1M), Total: ${:.8}",
				model,
				regular_input_cost, usage.regular_input_tokens, input_price,
				cache_creation_cost, usage.cache_creation_tokens, input_price * 1.25,
				cache_creation_cost_1h, usage.cache_creation_tokens_1h, input_price * 2.0,
				cache_read_cost, usage.cache_read_tokens, input_price * 0.1,
				output_cost, usage.output_tokens, output_price,
				total_cost
			);

			return Some(total_cost);
		}
	}
	None
}

/// Simplified cost calculation for Anthropic models with cache support
/// This is used by the helper function for individual token counts
fn calculate_anthropic_cost(
	model: &str,
	input_tokens: u32,
	output_tokens: u32,
	cache_creation_input_tokens: u32,
	cache_read_input_tokens: u32,
) -> Option<f64> {
	// Calculate cache creation tokens for 1h (these are charged at 2x)
	let cache_creation_1h_tokens = if cache_creation_input_tokens > 0 {
		// Assume all cache creation is for 1h TTL (more expensive)
		cache_creation_input_tokens
	} else {
		0
	};

	// Calculate regular input tokens (total input minus cache tokens)
	let regular_input_tokens =
		input_tokens.saturating_sub(cache_creation_input_tokens + cache_read_input_tokens);

	let usage = CacheTokenUsage {
		regular_input_tokens: regular_input_tokens as u64,
		cache_creation_tokens: 0, // Using 1h cache creation instead
		cache_creation_tokens_1h: cache_creation_1h_tokens as u64,
		cache_read_tokens: cache_read_input_tokens as u64,
		output_tokens: output_tokens as u64,
	};

	calculate_cost_with_cache(model, usage)
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

	fn supports_vision(&self, model: &str) -> bool {
		// Claude 3+ models support vision
		model.contains("claude-3")
			|| model.contains("claude-4")
			|| model.contains("claude-3.5")
			|| model.contains("claude-3.7")
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

	async fn chat_completion(&self, params: ChatCompletionParams<'_>) -> Result<ProviderResponse> {
		// Don't check cancellation at start - only during the actual request
		// This prevents stale cancellation state from blocking new requests
		// Get API key
		let api_key = self.get_api_key(params.config)?;

		// Convert messages to Anthropic format with automatic cache markers
		let anthropic_messages = convert_messages(params.messages);

		// Extract system message if present and handle caching
		let system_message = params
			.messages
			.iter()
			.find(|m| m.role == "system")
			.map(|m| m.content.clone())
			.unwrap_or_else(|| "You are a helpful assistant.".to_string());

		let system_cached = params
			.messages
			.iter()
			.any(|m| m.role == "system" && m.cached);

		// Create the request body
		let mut request_body = serde_json::json!({
			"model": params.model,
			"messages": anthropic_messages,
			"temperature": params.temperature,
			"top_p": params.top_p,
			"top_k": params.top_k,
		});

		// Add max_tokens if specified (0 means don't include it in request)
		if params.max_tokens > 0 {
			request_body["max_tokens"] = serde_json::json!(params.max_tokens);
		}

		// Add system message with cache control if needed
		if system_cached {
			let ttl = if params.config.use_long_system_cache {
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
		if !params.config.mcp.servers.is_empty() {
			let functions = crate::mcp::get_available_functions(params.config).await;
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
				if self.supports_caching(params.model) && !tools.is_empty() {
					// Check if any system message is cached - if so, we should cache tool definitions too
					let system_cached = params
						.messages
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

		// Check for cancellation before making HTTP request
		if let Some(ref token) = params.cancellation_token {
			if *token.borrow() {
				return Err(anyhow::anyhow!("Request cancelled before HTTP call"));
			}
		}

		// Use retry.rs with smart header-based delay calculation
		if params.max_retries > 0 {
			crate::log_debug!(
				"🔄 Anthropic provider configured with {} max retries",
				params.max_retries
			);
		}

		// Create the HTTP request operation that can be retried
		let api_key_clone = api_key.clone();
		let request_body_clone = request_body.clone();
		let cancellation_token_clone = params.cancellation_token.clone();

		let result = retry::retry_with_exponential_backoff(
			|| {
				let api_key = api_key_clone.clone();
				let request_body = request_body_clone.clone();
				let cancellation_token = cancellation_token_clone.clone();

				Box::pin(async move {
					execute_anthropic_request(api_key, request_body, cancellation_token).await
				})
			},
			params.max_retries,
			std::time::Duration::from_millis(1000), // Base timeout for fallback
			params.cancellation_token.as_ref(),
		)
		.await;

		match result {
			Ok(response_data) => {
				// CRITICAL FIX: Store the original content array for proper tool_use reconstruction
				// This ensures tool_result messages can reference the correct tool_use_id
				let stored_tool_calls = if response_data.tool_calls.is_some() {
					// If we found tool_use blocks, store the complete content array
					// This preserves both text content and tool_use blocks for conversation history
					response_data.response_json.get("content").cloned()
				} else {
					None
				};

				// Create exchange record
				let mut exchange = ProviderExchange::new(
					request_body,
					response_data.response_json.clone(),
					response_data.usage,
					self.name(),
				);

				// CRITICAL FIX: Store the original tool calls in the exchange for later reconstruction
				if let Some(ref content_array) = stored_tool_calls {
					exchange.response["tool_calls_content"] = content_array.clone();
				}

				Ok(ProviderResponse {
					content: response_data.content,
					exchange,
					tool_calls: response_data.tool_calls,
					finish_reason: response_data.finish_reason,
				})
			}
			Err(e) => Err(anyhow::anyhow!("Anthropic API request failed: {}", e)),
		}
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
				// Handle both text and image content
				let mut content_blocks = Vec::new();

				// Add text content if not empty
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

					content_blocks.push(text_content);
				}

				// Add image attachments if present
				if let Some(ref images) = msg.images {
					for img in images {
						if let crate::session::image::ImageData::Base64(ref data) = img.data {
							content_blocks.push(serde_json::json!({
								"type": "image",
								"source": {
									"type": "base64",
									"media_type": img.media_type,
									"data": data
								}
							}));
						}
					}
				}

				// Only create message if we have content
				if !content_blocks.is_empty() {
					result.push(AnthropicMessage {
						role: msg.role.clone(),
						content: serde_json::json!(content_blocks),
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

/// Response data structure for Anthropic HTTP requests
#[derive(Debug)]
struct AnthropicResponseData {
	content: String,
	response_json: serde_json::Value,
	usage: Option<TokenUsage>,
	tool_calls: Option<Vec<crate::mcp::McpToolCall>>,
	finish_reason: Option<String>,
}

/// Execute a single Anthropic HTTP request with smart retry delay calculation
async fn execute_anthropic_request(
	api_key: String,
	request_body: serde_json::Value,
	cancellation_token: Option<tokio::sync::watch::Receiver<bool>>,
) -> Result<AnthropicResponseData, String> {
	// Check for cancellation before starting
	if let Some(ref token) = cancellation_token {
		if *token.borrow() {
			return Err("Request cancelled before starting".to_string());
		}
	}

	// Create HTTP client
	let client = Client::new();

	// Track API request time
	let api_start = std::time::Instant::now();

	// Create the HTTP request
	let request = client
		.post(ANTHROPIC_API_URL)
		.header("x-api-key", api_key.clone())
		.header("Content-Type", "application/json")
		.header("anthropic-version", "2023-06-01")
		.header("anthropic-beta", "extended-cache-ttl-2025-04-11")
		.header("anthropic-beta", "token-efficient-tools-2025-02-19")
		.json(&request_body);

	// Race the HTTP request against cancellation
	let response = if let Some(token) = cancellation_token {
		let token_clone = token.clone();

		// Check current cancellation state immediately
		if *token_clone.borrow() {
			return Err("Request cancelled before HTTP call".to_string());
		}

		// Create an abortable wrapper using tokio's JoinHandle
		let mut request_handle = tokio::spawn(async move { request.send().await });

		// Create cancellation future
		let mut cancel_receiver = token_clone;
		let cancellation_future = async move {
			loop {
				if *cancel_receiver.borrow() {
					break;
				}
				cancel_receiver.changed().await.ok();
			}
		};

		// Use biased select to prioritize cancellation checking
		tokio::select! {
			biased;

			_ = cancellation_future => {
				// Abort the spawned task - this will actually cancel the HTTP request
				request_handle.abort();
				Err("Request cancelled during HTTP call".to_string())
			}
			result = &mut request_handle => {
				// Handle the JoinHandle result
				match result {
					Ok(Ok(response)) => Ok(response),
					Ok(Err(e)) => Err(e.to_string()),
					Err(_) => {
						// JoinError means the task was aborted
						Err("Request aborted".to_string())
					}
				}
			}
		}
	} else {
		request.send().await.map_err(|e| e.to_string())
	}?;

	// response is already a Response, not a Result

	// Calculate API request time
	let api_duration = api_start.elapsed();
	let api_time_ms = api_duration.as_millis() as u64;

	// Get response status and headers
	let status = response.status();
	let headers = response.headers().clone();

	// Check if we should retry based on status code
	if status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS {
		// Get response body for error details
		let response_text = response
			.text()
			.await
			.unwrap_or_else(|_| "Failed to read response".to_string());

		// Determine retry delay using Anthropic rate limit headers
		if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
			crate::log_info!("🚦 Anthropic rate limit exceeded (429) - analyzing headers for optimal retry timing...");

			// Debug: Log all headers received in response for troubleshooting
			crate::log_debug!("📋 Anthropic 429 response headers: {:?}", headers);

			// Check for Anthropic's retry-after header first
			if let Some(retry_after) = headers.get("retry-after") {
				if let Ok(retry_after_str) = retry_after.to_str() {
					if let Ok(retry_seconds) = retry_after_str.parse::<u64>() {
						let delay_ms = retry_seconds * 1000;
						crate::log_info!(
							"📋 Using retry-after header: waiting {:.1}s as specified by Anthropic",
							delay_ms as f64 / 1000.0
						);
						// Sleep here with smart delay, then return error to trigger retry
						tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
						return Err(format!("Rate limit exceeded (429): {}", response_text));
					}
				}
			}

			// Check for Anthropic rate limit reset headers to calculate smart delay
			if let Some(delay_ms) = calculate_smart_retry_delay(&headers, 0) {
				crate::log_info!(
					"🎯 Using Anthropic rate limit reset time: waiting {:.1}s",
					delay_ms as f64 / 1000.0
				);
				tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
				return Err(format!("Rate limit exceeded (429): {}", response_text));
			}

			// Log rate limit information for debugging
			log_rate_limit_info(&headers);

			// No smart delay available, let retry.rs handle exponential backoff
			crate::log_info!("📈 No rate limit headers found, using fallback exponential backoff");
		}

		return Err(format!("HTTP {} - {}", status, response_text));
	}

	// Success path - get response body
	let response_text = response
		.text()
		.await
		.map_err(|e| format!("Failed to read response: {}", e))?;

	// Parse JSON response
	let response_json: serde_json::Value = serde_json::from_str(&response_text).map_err(|e| {
		format!(
			"Failed to parse response JSON: {}. Response: {}",
			e, response_text
		)
	})?;

	// Handle error responses
	if !status.is_success() {
		let mut error_details = Vec::new();
		error_details.push(format!("HTTP {}", status));

		if let Some(error_obj) = response_json.get("error") {
			if let Some(msg) = error_obj.get("message").and_then(|m| m.as_str()) {
				error_details.push(format!("Message: {}", msg));
			}
			if let Some(type_) = error_obj.get("type").and_then(|t| t.as_str()) {
				error_details.push(format!("Type: {}", type_));
			}
		}

		if error_details.len() == 1 {
			error_details.push(format!("Raw response: {}", response_text));
		}

		let full_error = error_details.join(" | ");
		return Err(format!("Anthropic API error: {}", full_error));
	}

	// Extract content from the response
	let content = response_json
		.get("content")
		.and_then(|content| content.as_array())
		.map(|content_array| {
			content_array
				.iter()
				.filter_map(|item| {
					if item.get("type")?.as_str()? == "text" {
						item.get("text")?.as_str()
					} else {
						None
					}
				})
				.collect::<Vec<_>>()
				.join("")
		})
		.unwrap_or_default();

	// Extract tool calls if present
	let tool_calls = response_json
		.get("content")
		.and_then(|content| content.as_array())
		.map(|content_array| {
			content_array
				.iter()
				.filter_map(|item| {
					if item.get("type")?.as_str()? == "tool_use" {
						let name = item.get("name")?.as_str()?.to_string();
						let input = item.get("input")?.clone();
						Some(crate::mcp::McpToolCall {
							tool_name: name,
							parameters: input,
							tool_id: item.get("id")?.as_str()?.to_string(), // Anthropic provides tool_use_id
						})
					} else {
						None
					}
				})
				.collect::<Vec<_>>()
		})
		.filter(|calls| !calls.is_empty());

	// Extract finish reason
	let finish_reason = response_json
		.get("stop_reason")
		.and_then(|reason| reason.as_str())
		.map(|s| s.to_string());

	// Extract token usage with cache information
	let usage = if let Some(usage_obj) = response_json.get("usage") {
		let input_tokens = usage_obj
			.get("input_tokens")
			.and_then(|v| v.as_u64())
			.unwrap_or(0) as u32;
		let output_tokens = usage_obj
			.get("output_tokens")
			.and_then(|v| v.as_u64())
			.unwrap_or(0) as u32;

		// Extract cache information
		let cache_creation_input_tokens = usage_obj
			.get("cache_creation_input_tokens")
			.and_then(|v| v.as_u64())
			.unwrap_or(0) as u32;
		let cache_read_input_tokens = usage_obj
			.get("cache_read_input_tokens")
			.and_then(|v| v.as_u64())
			.unwrap_or(0) as u32;

		// Calculate cached tokens and cost
		let cached_tokens = cache_read_input_tokens;

		// Calculate cost with proper cache pricing
		let cost = calculate_anthropic_cost(
			request_body
				.get("model")
				.and_then(|m| m.as_str())
				.unwrap_or("claude-3-5-sonnet-20241022"),
			input_tokens,
			output_tokens,
			cache_creation_input_tokens,
			cache_read_input_tokens,
		);

		if cache_creation_input_tokens > 0 || cache_read_input_tokens > 0 {
			crate::log_debug!(
				"💾 Anthropic cache info: {} creation tokens, {} read tokens",
				cache_creation_input_tokens,
				cache_read_input_tokens
			);
		}

		Some(TokenUsage {
			prompt_tokens: (input_tokens + cache_read_input_tokens) as u64,
			output_tokens: output_tokens as u64,
			total_tokens: (input_tokens + cache_read_input_tokens + output_tokens) as u64,
			cached_tokens: cached_tokens as u64,
			cost,
			request_time_ms: Some(api_time_ms),
		})
	} else {
		None
	};

	// Log rate limit information for successful requests
	log_rate_limit_info(&headers);

	// Log API timing
	crate::log_debug!("⏱️  Anthropic API request completed in {}ms", api_time_ms);

	Ok(AnthropicResponseData {
		content,
		response_json,
		usage,
		tool_calls,
		finish_reason,
	})
}

/// Calculate smart retry delay based on Anthropic rate limit headers
/// Returns None if no suitable headers are found, falling back to exponential backoff
fn calculate_smart_retry_delay(headers: &reqwest::header::HeaderMap, attempt: u32) -> Option<u64> {
	// Check for token-based rate limit reset times
	let token_reset = headers
		.get("anthropic-ratelimit-tokens-reset")
		.and_then(|h| h.to_str().ok())
		.and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok());

	let input_token_reset = headers
		.get("anthropic-ratelimit-input-tokens-reset")
		.and_then(|h| h.to_str().ok())
		.and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok());

	let output_token_reset = headers
		.get("anthropic-ratelimit-output-tokens-reset")
		.and_then(|h| h.to_str().ok())
		.and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok());

	let request_reset = headers
		.get("anthropic-ratelimit-requests-reset")
		.and_then(|h| h.to_str().ok())
		.and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok());

	// Find the earliest reset time (most restrictive limit)
	let earliest_reset = [
		token_reset,
		input_token_reset,
		output_token_reset,
		request_reset,
	]
	.iter()
	.filter_map(|&reset| reset)
	.min();

	if let Some(reset_time) = earliest_reset {
		let now = chrono::Utc::now();
		let wait_duration = reset_time.signed_duration_since(now);

		if wait_duration.num_seconds() > 0 {
			// Add a small buffer (1-2 seconds) to ensure we're past the reset time
			let wait_ms = (wait_duration.num_milliseconds() + 1000 + (attempt as i64 * 500)) as u64;

			// Cap the wait time to reasonable limits (max 5 minutes)
			let max_wait_ms = 5 * 60 * 1000; // 5 minutes
			let final_wait_ms = wait_ms.min(max_wait_ms);

			crate::log_info!(
				"🎯 Using Anthropic rate limit reset time: waiting {:.1}s until {} UTC",
				final_wait_ms as f64 / 1000.0,
				reset_time.format("%H:%M:%S")
			);

			return Some(final_wait_ms);
		}
	}

	None
}

/// Log rate limit information from Anthropic headers for debugging
fn log_rate_limit_info(headers: &reqwest::header::HeaderMap) {
	let mut rate_limit_info = Vec::new();

	// Request limits
	if let (Some(limit), Some(remaining)) = (
		headers
			.get("anthropic-ratelimit-requests-limit")
			.and_then(|h| h.to_str().ok()),
		headers
			.get("anthropic-ratelimit-requests-remaining")
			.and_then(|h| h.to_str().ok()),
	) {
		rate_limit_info.push(format!("Requests: {}/{}", remaining, limit));
	}

	// Token limits
	if let (Some(limit), Some(remaining)) = (
		headers
			.get("anthropic-ratelimit-tokens-limit")
			.and_then(|h| h.to_str().ok()),
		headers
			.get("anthropic-ratelimit-tokens-remaining")
			.and_then(|h| h.to_str().ok()),
	) {
		rate_limit_info.push(format!("Tokens: {}/{}", remaining, limit));
	}

	// Input token limits
	if let (Some(limit), Some(remaining)) = (
		headers
			.get("anthropic-ratelimit-input-tokens-limit")
			.and_then(|h| h.to_str().ok()),
		headers
			.get("anthropic-ratelimit-input-tokens-remaining")
			.and_then(|h| h.to_str().ok()),
	) {
		rate_limit_info.push(format!("Input tokens: {}/{}", remaining, limit));
	}

	// Output token limits
	if let (Some(limit), Some(remaining)) = (
		headers
			.get("anthropic-ratelimit-output-tokens-limit")
			.and_then(|h| h.to_str().ok()),
		headers
			.get("anthropic-ratelimit-output-tokens-remaining")
			.and_then(|h| h.to_str().ok()),
	) {
		rate_limit_info.push(format!("Output tokens: {}/{}", remaining, limit));
	}

	if !rate_limit_info.is_empty() {
		crate::log_info!("📊 Anthropic rate limits: {}", rate_limit_info.join(" | "));
	}
}
