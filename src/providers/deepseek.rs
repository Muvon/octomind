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

// DeepSeek provider implementation
//
// PRICING UPDATE: September 5, 2025 at 16:00 UTC
// New unified pricing for ALL models (no time-based discounts):
// - Cache Hit: $0.07 per 1M tokens
// - Cache Miss (Input): $0.56 per 1M tokens
// - Output: $1.68 per 1M tokens

use super::{AiProvider, ChatCompletionParams, ProviderExchange, ProviderResponse, TokenUsage};
use crate::config::Config;
use crate::log_debug;
use crate::session::Message;
use anyhow::Result;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::env;

// Model pricing (per 1M tokens in USD) - Updated Sept 5, 2025
// New unified pricing for ALL models (no time-based discounts)
lazy_static::lazy_static! {
	/// Input pricing (cache miss): $0.56 per 1M tokens for all models
	static ref INPUT_PRICING: f64 = 0.56;
	/// Output pricing: $1.68 per 1M tokens for all models
	static ref OUTPUT_PRICING: f64 = 1.68;
	/// Cache hit pricing: $0.07 per 1M tokens for all models
	static ref CACHE_HIT_PRICING: f64 = 0.07;
}

// Time-based discount system removed as of Sept 5, 2025
// All models now use unified pricing regardless of time

/// Calculate cost for DeepSeek models with unified pricing (Sept 5, 2025+)
fn calculate_cost_with_cache(
	_model: &str, // Model parameter kept for API compatibility but not used
	regular_input_tokens: u64,
	cache_hit_tokens: u64,
	completion_tokens: u64,
) -> Option<f64> {
	// New unified pricing for all models
	let regular_input_cost = (regular_input_tokens as f64 / 1_000_000.0) * *INPUT_PRICING;
	let cache_hit_cost = (cache_hit_tokens as f64 / 1_000_000.0) * *CACHE_HIT_PRICING;
	let output_cost = (completion_tokens as f64 / 1_000_000.0) * *OUTPUT_PRICING;
	Some(regular_input_cost + cache_hit_cost + output_cost)
}

/// Calculate cost for DeepSeek models with unified pricing (no cache)
fn calculate_cost(_model: &str, prompt_tokens: u64, completion_tokens: u64) -> Option<f64> {
	calculate_cost_with_cache(_model, prompt_tokens, 0, completion_tokens)
}

/// DeepSeek provider implementation
pub struct DeepSeekProvider;

impl Default for DeepSeekProvider {
	fn default() -> Self {
		Self::new()
	}
}

impl DeepSeekProvider {
	pub fn new() -> Self {
		Self
	}
}

// Constants
const DEEPSEEK_API_KEY_ENV: &str = "DEEPSEEK_API_KEY";
const DEEPSEEK_API_URL: &str = "https://api.deepseek.com/v1/chat/completions";

/// Message format for the DeepSeek API (OpenAI-compatible)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeepSeekMessage {
	pub role: String,
	pub content: serde_json::Value, // Can be string or array with content parts
	#[serde(skip_serializing_if = "Option::is_none")]
	pub tool_call_id: Option<String>, // For tool messages: the ID of the tool call
	#[serde(skip_serializing_if = "Option::is_none")]
	pub name: Option<String>, // For tool messages: the name of the tool
	#[serde(skip_serializing_if = "Option::is_none")]
	pub tool_calls: Option<serde_json::Value>, // For assistant messages: array of tool calls
}

#[async_trait::async_trait]
impl AiProvider for DeepSeekProvider {
	fn name(&self) -> &str {
		"deepseek"
	}

	fn supports_model(&self, model: &str) -> bool {
		// DeepSeek models - current lineup
		model == "deepseek-chat" || model == "deepseek-reasoner"
	}

	fn get_api_key(&self, _config: &Config) -> Result<String> {
		// API keys now only from environment variables for security
		match env::var(DEEPSEEK_API_KEY_ENV) {
			Ok(key) => Ok(key),
			Err(_) => Err(anyhow::anyhow!(
				"DeepSeek API key not found in environment variable: {}",
				DEEPSEEK_API_KEY_ENV
			)),
		}
	}

	fn supports_caching(&self, _model: &str) -> bool {
		// DeepSeek supports context caching for all models
		true
	}

	fn supports_vision(&self, _model: &str) -> bool {
		// DeepSeek models don't currently support vision
		false
	}

	fn get_max_input_tokens(&self, model: &str) -> usize {
		// DeepSeek model context window limits
		match model {
			"deepseek-chat" => 64_000,     // 64K context window
			"deepseek-reasoner" => 64_000, // 64K context window (output doesn't count toward limit)
			_ => 64_000,                   // Default to 64K for all DeepSeek models
		}
	}

	async fn chat_completion(&self, params: ChatCompletionParams<'_>) -> Result<ProviderResponse> {
		// Check for cancellation before starting
		if let Some(ref token) = params.cancellation_token {
			if *token.borrow() {
				return Err(anyhow::anyhow!("Request cancelled before starting"));
			}
		}

		// Get API key
		let api_key = self.get_api_key(params.config)?;

		// Convert messages to DeepSeek format (OpenAI-compatible)
		let deepseek_messages = convert_messages(params.messages);

		// Create the request body
		let mut request_body = serde_json::json!({
			"model": params.model,
			"messages": deepseek_messages,
			"temperature": params.temperature,
			"top_p": params.top_p,
			"top_k": params.top_k,
		});

		// Add max_tokens if specified (0 means don't include it in request)
		if params.max_tokens > 0 {
			request_body["max_tokens"] = serde_json::json!(params.max_tokens);
		}

		// Add tool definitions if MCP has any servers configured
		if !params.config.mcp.servers.is_empty() {
			let functions = crate::mcp::get_available_functions(params.config).await;
			if !functions.is_empty() {
				// CRITICAL FIX: Ensure tool definitions are ALWAYS in the same order
				// Sort functions by name to guarantee consistent ordering across API calls
				let mut sorted_functions = functions;
				sorted_functions.sort_by(|a, b| a.name.cmp(&b.name));

				let tools = sorted_functions
					.iter()
					.map(|f| {
						serde_json::json!({
							"type": "function",
							"function": {
								"name": f.name,
								"description": f.description,
								"parameters": f.parameters
							}
						})
					})
					.collect::<Vec<_>>();

				request_body["tools"] = serde_json::json!(tools);
				request_body["tool_choice"] = serde_json::json!("auto");
			}
		}

		// Implement retry logic with exponential backoff
		if params.max_retries > 0 {
			crate::log_debug!(
				"🔄 DeepSeek provider configured with {} max retries",
				params.max_retries
			);
		}

		// Track API request time
		let api_start = std::time::Instant::now();

		// Make the actual API request with retry logic
		let response = crate::providers::retry::retry_with_exponential_backoff(
			|| {
				let client = Client::new();
				let request_body = request_body.clone();
				let api_key = api_key.clone();

				Box::pin(async move {
					client
						.post(DEEPSEEK_API_URL)
						.header("Authorization", format!("Bearer {}", api_key))
						.header("Content-Type", "application/json")
						.json(&request_body)
						.send()
						.await
						.map_err(|e| anyhow::anyhow!("HTTP request failed: {}", e))
				})
			},
			params.max_retries,
			params.retry_timeout,
			params.cancellation_token.as_ref(),
		)
		.await?;

		// Calculate API request time
		let api_duration = api_start.elapsed();
		let api_time_ms = api_duration.as_millis() as u64;

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
				if let Some(code) = error_obj.get("code").and_then(|c| c.as_str()) {
					error_details.push(format!("Code: {}", code));
				}
				if let Some(type_) = error_obj.get("type").and_then(|t| t.as_str()) {
					error_details.push(format!("Type: {}", type_));
				}
			}

			if error_details.len() == 1 {
				error_details.push(format!("Raw response: {}", response_text));
			}

			let full_error = error_details.join(" | ");
			return Err(anyhow::anyhow!("DeepSeek API error: {}", full_error));
		}

		// Check for errors in response body even with HTTP 200
		if let Some(error_obj) = response_json.get("error") {
			let mut error_details = Vec::new();
			error_details.push("HTTP 200 but error in response".to_string());

			if let Some(msg) = error_obj.get("message").and_then(|m| m.as_str()) {
				error_details.push(format!("Message: {}", msg));
			}

			let full_error = error_details.join(" | ");
			return Err(anyhow::anyhow!("DeepSeek API error: {}", full_error));
		}

		// Extract content and tool calls from response
		let message = response_json
			.get("choices")
			.and_then(|choices| choices.get(0))
			.and_then(|choice| choice.get("message"))
			.ok_or_else(|| {
				anyhow::anyhow!("Invalid response format from DeepSeek: {}", response_text)
			})?;

		// Extract finish_reason
		let finish_reason = response_json
			.get("choices")
			.and_then(|choices| choices.get(0))
			.and_then(|choice| choice.get("finish_reason"))
			.and_then(|fr| fr.as_str())
			.map(|s| s.to_string());

		if let Some(ref reason) = finish_reason {
			log_debug!("Finish reason: {}", reason);
		}

		// Extract content
		let mut content = String::new();
		if let Some(text) = message.get("content").and_then(|c| c.as_str()) {
			content = text.to_string();
		}

		// Extract tool calls
		let tool_calls = if let Some(tool_calls_val) = message.get("tool_calls") {
			if tool_calls_val.is_array() && !tool_calls_val.as_array().unwrap().is_empty() {
				let mut extracted_tool_calls = Vec::new();

				for tool_call in tool_calls_val.as_array().unwrap() {
					if let Some(function) = tool_call.get("function") {
						if let (Some(name), Some(args)) = (
							function.get("name").and_then(|n| n.as_str()),
							function.get("arguments").and_then(|a| a.as_str()),
						) {
							let params = if args.trim().is_empty() {
								serde_json::json!({})
							} else {
								match serde_json::from_str::<serde_json::Value>(args) {
									Ok(json_params) => json_params,
									Err(_) => serde_json::Value::String(args.to_string()),
								}
							};

							let tool_id =
								tool_call.get("id").and_then(|i| i.as_str()).unwrap_or("");
							let mcp_call = crate::mcp::McpToolCall {
								tool_name: name.to_string(),
								parameters: params,
								tool_id: tool_id.to_string(),
							};

							extracted_tool_calls.push(mcp_call);
						}
					}
				}

				crate::mcp::ensure_tool_call_ids(&mut extracted_tool_calls);
				Some(extracted_tool_calls)
			} else {
				None
			}
		} else {
			None
		};

		// Extract token usage with cache-aware pricing
		let usage: Option<TokenUsage> = if let Some(usage_obj) = response_json.get("usage") {
			let prompt_tokens = usage_obj
				.get("prompt_tokens")
				.and_then(|v| v.as_u64())
				.unwrap_or(0);
			let completion_tokens = usage_obj
				.get("completion_tokens")
				.and_then(|v| v.as_u64())
				.unwrap_or(0);
			let total_tokens = usage_obj
				.get("total_tokens")
				.and_then(|v| v.as_u64())
				.unwrap_or(0);

			// Parse cache-specific token fields from DeepSeek API
			// DeepSeek returns cache hit tokens in prompt_tokens_details.cached_tokens
			let cache_hit_tokens = usage_obj
				.get("prompt_tokens_details")
				.and_then(|details| details.get("cached_tokens"))
				.and_then(|v| v.as_u64())
				.unwrap_or(0);

			// For DeepSeek: Cache hit tokens get special pricing ($0.07/1M)
			// Regular input tokens are charged at cache miss rate ($0.56/1M)
			let regular_input_tokens = prompt_tokens.saturating_sub(cache_hit_tokens);

			// Calculate cost with unified pricing (Sept 5, 2025+)
			let cost = if cache_hit_tokens > 0 {
				calculate_cost_with_cache(
					params.model,
					regular_input_tokens,
					cache_hit_tokens,
					completion_tokens,
				)
			} else {
				// Fallback to regular pricing if no cache hits
				calculate_cost(params.model, prompt_tokens, completion_tokens)
			};

			// Simple interface: only expose cached tokens
			let cached_tokens = cache_hit_tokens;

			Some(TokenUsage {
				prompt_tokens,
				output_tokens: completion_tokens,
				total_tokens,
				cached_tokens,                      // Simple: total tokens that came from cache
				cost,                               // Pre-calculated with unified pricing (Sept 5, 2025+)
				request_time_ms: Some(api_time_ms), // Track API timing for DeepSeek
			})
		} else {
			None
		};

		// Create exchange record
		let exchange = ProviderExchange::new(request_body, response_json, usage, self.name());

		Ok(ProviderResponse {
			content,
			exchange,
			tool_calls,
			finish_reason,
		})
	}
}

// Convert our session messages to DeepSeek format (OpenAI-compatible)
fn convert_messages(messages: &[Message]) -> Vec<DeepSeekMessage> {
	let mut result = Vec::new();

	for msg in messages {
		// Handle tool response messages (has <fnr> tags)
		if msg.role == "user" && msg.content.starts_with("<fnr>") && msg.content.ends_with("</fnr>")
		{
			let content = msg
				.content
				.trim_start_matches("<fnr>")
				.trim_end_matches("</fnr>")
				.trim();

			if let Ok(tool_responses) = serde_json::from_str::<Vec<serde_json::Value>>(content) {
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

						let name = tool_response
							.get("name")
							.and_then(|n| n.as_str())
							.unwrap_or("");

						let content = tool_response
							.get("content")
							.and_then(|c| c.as_str())
							.unwrap_or("");

						result.push(DeepSeekMessage {
							role: "tool".to_string(),
							content: serde_json::json!(content),
							tool_call_id: Some(tool_call_id.to_string()),
							name: Some(name.to_string()),
							tool_calls: None,
						});
					}
					continue;
				} else {
					result.push(DeepSeekMessage {
						role: "tool".to_string(),
						content: serde_json::json!(content),
						tool_call_id: Some("legacy_tool_call".to_string()),
						name: Some("legacy_tool".to_string()),
						tool_calls: None,
					});
					continue;
				}
			}
		} else if msg.role == "tool" {
			let tool_call_id = msg.tool_call_id.clone().unwrap_or_default();
			let name = msg.name.clone().unwrap_or_default();

			result.push(DeepSeekMessage {
				role: "tool".to_string(),
				content: serde_json::json!(msg.content),
				tool_call_id: Some(tool_call_id),
				name: Some(name),
				tool_calls: None,
			});
			continue;
		} else if msg.role == "assistant" {
			let mut assistant_message = DeepSeekMessage {
				role: msg.role.clone(),
				content: serde_json::json!(msg.content),
				tool_call_id: None,
				name: None,
				tool_calls: None,
			};

			// Include stored tool_calls if present
			if let Some(ref tool_calls_data) = msg.tool_calls {
				assistant_message.tool_calls = Some(tool_calls_data.clone());
			}

			result.push(assistant_message);
			continue;
		}

		// Regular text-only messages (DeepSeek doesn't support vision yet)
		result.push(DeepSeekMessage {
			role: msg.role.clone(),
			content: serde_json::json!(msg.content),
			tool_call_id: None,
			name: None,
			tool_calls: None,
		});
	}

	result
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_supports_model() {
		let provider = DeepSeekProvider::new();

		// Models that should be supported
		assert!(provider.supports_model("deepseek-chat"));
		assert!(provider.supports_model("deepseek-reasoner"));

		// Models that should NOT be supported
		assert!(!provider.supports_model("gpt-4"));
		assert!(!provider.supports_model("claude-3.5-sonnet"));
		assert!(!provider.supports_model("deepseek-coder")); // Not in current API
	}

	#[test]
	fn test_calculate_cost() {
		// Test basic cost calculation with new unified pricing (Sept 5, 2025+)
		// Input: $0.56/1M, Output: $1.68/1M
		let cost = calculate_cost("deepseek-chat", 1_000_000, 500_000);
		assert!(cost.is_some());
		let cost_value = cost.unwrap();

		// Expected: (1M * $0.56) + (0.5M * $1.68) = $0.56 + $0.84 = $1.40
		let expected = 0.56 + (0.5 * 1.68);
		assert!((cost_value - expected).abs() < 0.01); // Allow small floating point differences

		// Test with different model - should be same price now
		let cost2 = calculate_cost("deepseek-reasoner", 1_000_000, 500_000);
		assert!(cost2.is_some());
		assert!((cost2.unwrap() - expected).abs() < 0.01); // Same pricing for all models
	}

	#[test]
	fn test_calculate_cost_with_cache() {
		// Test cache-aware cost calculation with new unified pricing
		// Cache hit: $0.07/1M, Cache miss: $0.56/1M, Output: $1.68/1M
		let cost = calculate_cost_with_cache("deepseek-chat", 500_000, 500_000, 250_000);
		assert!(cost.is_some());
		let cost_value = cost.unwrap();

		// Expected: (0.5M * $0.56) + (0.5M * $0.07) + (0.25M * $1.68)
		//         = $0.28 + $0.035 + $0.42 = $0.735
		let expected = (0.5 * 0.56) + (0.5 * 0.07) + (0.25 * 1.68);
		assert!((cost_value - expected).abs() < 0.01);

		// Cost with cache should be less than without cache for same total input
		let cost_no_cache = calculate_cost("deepseek-chat", 1_000_000, 250_000);
		assert!(cost_no_cache.is_some());
		assert!(cost_value < cost_no_cache.unwrap());
	}
}
