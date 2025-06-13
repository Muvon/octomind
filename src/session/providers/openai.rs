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

// OpenAI provider implementation

use super::{AiProvider, ProviderExchange, ProviderResponse, TokenUsage};
use crate::config::Config;
use crate::log_debug;
use crate::session::Message;
use anyhow::Result;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::env;

/// OpenAI pricing constants (per 1M tokens in USD)
/// Source: https://platform.openai.com/docs/pricing (as of January 2025)
const PRICING: &[(&str, f64, f64)] = &[
	// Model, Input price per 1M tokens, Output price per 1M tokens
	// GPT-4o models
	("gpt-4o", 2.50, 10.00),
	("gpt-4o-mini", 0.15, 0.60),
	("gpt-4o-2024-11-20", 2.50, 10.00),
	("gpt-4o-2024-08-06", 2.50, 10.00),
	("gpt-4o-2024-05-13", 5.00, 15.00),
	("chatgpt-4o-latest", 2.50, 10.00),
	// O-series reasoning models
	("o4", 25.00, 100.00),    // Latest O4 model
	("o3", 20.00, 80.00),     // O3 model
	("o3-mini", 5.00, 20.00), // O3 mini variant
	("o1", 15.00, 60.00),
	("o1-preview", 15.00, 60.00),
	("o1-mini", 3.00, 12.00),
	// GPT-4.5 models (newest series)
	("gpt-4.5-turbo", 6.00, 20.00),
	("gpt-4.5", 20.00, 40.00),
	("gpt-4.5-preview", 6.00, 20.00),
	// GPT-4.1 models (newer series)
	("gpt-4.1-turbo", 8.00, 25.00),
	("gpt-4.1", 25.00, 50.00),
	("gpt-4.1-preview", 8.00, 25.00),
	// GPT-4 Turbo models
	("gpt-4-turbo", 10.00, 30.00),
	("gpt-4-turbo-2024-04-09", 10.00, 30.00),
	("gpt-4-0125-preview", 10.00, 30.00),
	("gpt-4-1106-preview", 10.00, 30.00),
	// GPT-4 models
	("gpt-4", 30.00, 60.00),
	("gpt-4-0613", 30.00, 60.00),
	("gpt-4-0314", 30.00, 60.00),
	// GPT-3.5 Turbo models
	("gpt-3.5-turbo", 0.50, 1.50),
	("gpt-3.5-turbo-0125", 0.50, 1.50),
	("gpt-3.5-turbo-1106", 1.00, 2.00),
];

/// Calculate cost for OpenAI models with basic pricing
fn calculate_cost(model: &str, prompt_tokens: u64, completion_tokens: u64) -> Option<f64> {
	for (pricing_model, input_price, output_price) in PRICING {
		if model.contains(pricing_model) {
			let input_cost = (prompt_tokens as f64 / 1_000_000.0) * input_price;
			let output_cost = (completion_tokens as f64 / 1_000_000.0) * output_price;
			return Some(input_cost + output_cost);
		}
	}
	None
}

/// Calculate cost for OpenAI models with cache-aware pricing
/// - cache_read_tokens: charged at 0.25x normal price (75% cheaper)
/// - regular_input_tokens: charged at normal price (includes cache write tokens)
/// - output_tokens: charged at normal price
fn calculate_cost_with_cache(
	model: &str,
	regular_input_tokens: u64,
	cache_read_tokens: u64,
	completion_tokens: u64,
) -> Option<f64> {
	for (pricing_model, input_price, output_price) in PRICING {
		if model.contains(pricing_model) {
			// Regular input tokens at normal price (includes cache write - no additional cost)
			let regular_input_cost = (regular_input_tokens as f64 / 1_000_000.0) * input_price;

			// Cache read tokens at 0.25x price (75% cheaper)
			let cache_read_cost = (cache_read_tokens as f64 / 1_000_000.0) * input_price * 0.25;

			// Output tokens at normal price (never cached)
			let output_cost = (completion_tokens as f64 / 1_000_000.0) * output_price;

			return Some(regular_input_cost + cache_read_cost + output_cost);
		}
	}
	None
}

/// Check if a model supports the temperature parameter
/// O1 and O2 series models don't support temperature
fn supports_temperature(model: &str) -> bool {
	!model.starts_with("o1")
		&& !model.starts_with("o2")
		&& !model.starts_with("o3")
		&& !model.starts_with("o4")
}

/// OpenAI provider implementation
pub struct OpenAiProvider;

impl Default for OpenAiProvider {
	fn default() -> Self {
		Self::new()
	}
}

impl OpenAiProvider {
	pub fn new() -> Self {
		Self
	}
}

// Constants
const OPENAI_API_KEY_ENV: &str = "OPENAI_API_KEY";
const OPENAI_API_URL: &str = "https://api.openai.com/v1/chat/completions";

/// Message format for the OpenAI API
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAiMessage {
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
impl AiProvider for OpenAiProvider {
	fn name(&self) -> &str {
		"openai"
	}

	fn supports_model(&self, model: &str) -> bool {
		// OpenAI models - current lineup
		model.starts_with("gpt-4o")
			|| model.starts_with("gpt-4.5")
			|| model.starts_with("gpt-4.1")
			|| model.starts_with("gpt-4")
			|| model.starts_with("gpt-3.5")
			|| model.starts_with("o1")
			|| model.starts_with("o3")
			|| model.starts_with("o4")
			|| model == "chatgpt-4o-latest"
	}

	fn get_api_key(&self, _config: &Config) -> Result<String> {
		// API keys now only from environment variables for security
		match env::var(OPENAI_API_KEY_ENV) {
			Ok(key) => Ok(key),
			Err(_) => Err(anyhow::anyhow!(
				"OpenAI API key not found in environment variable: {}",
				OPENAI_API_KEY_ENV
			)),
		}
	}

	fn supports_caching(&self, model: &str) -> bool {
		// OpenAI doesn't currently support caching in the same way as Anthropic
		// But some models support better context handling
		model.contains("gpt-4") || model.contains("o1")
	}

	fn supports_vision(&self, model: &str) -> bool {
		// OpenAI vision-capable models
		model.contains("gpt-4o")
			|| model.contains("gpt-4.1")
			|| model.contains("gpt-4-turbo")
			|| model.contains("gpt-4-vision-preview")
			|| model.starts_with("gpt-4o-")
	}

	fn get_max_input_tokens(&self, model: &str) -> usize {
		// OpenAI model context window limits (what we can send as input)
		// These are the actual context windows - API handles output limits

		// GPT-4o models: 128K context window
		if model.contains("gpt-4o") {
			return 128_000;
		}
		// GPT-4 models: varies by version
		if model.contains("gpt-4-turbo") || model.contains("gpt-4.5") || model.contains("gpt-4.1") {
			return 128_000;
		}
		if model.contains("gpt-4") && !model.contains("gpt-4o") {
			return 8_192; // Old GPT-4: 8K context window
		}
		// O-series models: 128K context window
		if model.starts_with("o1") || model.starts_with("o2") || model.starts_with("o3") {
			return 128_000;
		}
		// GPT-3.5: 16K context window
		if model.contains("gpt-3.5") {
			return 16_384;
		}
		// Default conservative limit
		8_192
	}

	async fn chat_completion(
		&self,
		messages: &[Message],
		model: &str,
		temperature: f32,
		config: &Config,
		cancellation_token: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
	) -> Result<ProviderResponse> {
		// Check for cancellation before starting
		if let Some(ref token) = cancellation_token {
			if token.load(std::sync::atomic::Ordering::SeqCst) {
				return Err(anyhow::anyhow!("Request cancelled before starting"));
			}
		}
		// Get API key
		let api_key = self.get_api_key(config)?;

		// Convert messages to OpenAI format
		let openai_messages = convert_messages(messages);

		// Create the request body
		let mut request_body = serde_json::json!({
			"model": model,
			"messages": openai_messages,
		});

		// Only add temperature for models that support it
		// O1/O2 series models don't support temperature parameter
		if supports_temperature(model) {
			request_body["temperature"] = serde_json::json!(temperature);
		}

		// Add tool definitions if MCP has any servers configured
		if !config.mcp.servers.is_empty() {
			let functions = crate::mcp::get_available_functions(config).await;
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

				// Note: OpenAI doesn't support caching yet, but we prepare for future support
				// if self.supports_caching(model) && !tools.is_empty() {
				//     if let Some(last_tool) = tools.last_mut() {
				//         last_tool["cache_control"] = serde_json::json!({
				//             "type": "ephemeral"
				//         });
				//     }
				// }

				request_body["tools"] = serde_json::json!(tools);
				request_body["tool_choice"] = serde_json::json!("auto");
			}
		}

		// Create HTTP client
		let client = Client::new();

		// Track API request time
		let api_start = std::time::Instant::now();

		// Make the actual API request
		let response = client
			.post(OPENAI_API_URL)
			.header("Authorization", format!("Bearer {}", api_key))
			.header("Content-Type", "application/json")
			.json(&request_body)
			.send()
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
			return Err(anyhow::anyhow!("OpenAI API error: {}", full_error));
		}

		// Check for errors in response body even with HTTP 200
		if let Some(error_obj) = response_json.get("error") {
			let mut error_details = Vec::new();
			error_details.push("HTTP 200 but error in response".to_string());

			if let Some(msg) = error_obj.get("message").and_then(|m| m.as_str()) {
				error_details.push(format!("Message: {}", msg));
			}

			let full_error = error_details.join(" | ");
			return Err(anyhow::anyhow!("OpenAI API error: {}", full_error));
		}

		// Extract content and tool calls from response
		let message = response_json
			.get("choices")
			.and_then(|choices| choices.get(0))
			.and_then(|choice| choice.get("message"))
			.ok_or_else(|| {
				anyhow::anyhow!("Invalid response format from OpenAI: {}", response_text)
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

			// Parse cache-specific token fields from OpenAI API
			// OpenAI returns cache read tokens in prompt_tokens_details.cached_tokens
			let cache_read_tokens = usage_obj
				.get("prompt_tokens_details")
				.and_then(|details| details.get("cached_tokens"))
				.and_then(|v| v.as_u64())
				.unwrap_or(0);

			// For OpenAI: Cache write tokens are NOT charged extra (1x normal price)
			// Regular input tokens include both new tokens and cache write tokens
			// Only cache READ tokens get the discount (0.25x price)
			let regular_input_tokens = prompt_tokens.saturating_sub(cache_read_tokens);

			// Calculate cost with cache-aware pricing
			let cost = if cache_read_tokens > 0 {
				calculate_cost_with_cache(
					model,
					regular_input_tokens,
					cache_read_tokens,
					completion_tokens,
				)
			} else {
				// Fallback to regular pricing if no cache reads
				calculate_cost(model, prompt_tokens, completion_tokens)
			};

			// Simple interface: only expose cached tokens (OpenAI only has cache reads, no extra cost for writes)
			let cached_tokens = cache_read_tokens;

			Some(TokenUsage {
				prompt_tokens,
				output_tokens: completion_tokens,
				total_tokens,
				cached_tokens,                      // Simple: total tokens that came from cache
				cost,                               // Pre-calculated with proper cache pricing
				request_time_ms: Some(api_time_ms), // Track API timing for OpenAI
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

// Convert our session messages to OpenAI format
fn convert_messages(messages: &[Message]) -> Vec<OpenAiMessage> {
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

						result.push(OpenAiMessage {
							role: "tool".to_string(),
							content: serde_json::json!(content),
							tool_call_id: Some(tool_call_id.to_string()),
							name: Some(name.to_string()),
							tool_calls: None,
						});
					}
					continue;
				} else {
					result.push(OpenAiMessage {
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

			result.push(OpenAiMessage {
				role: "tool".to_string(),
				content: serde_json::json!(msg.content),
				tool_call_id: Some(tool_call_id),
				name: Some(name),
				tool_calls: None,
			});
			continue;
		} else if msg.role == "assistant" {
			let mut assistant_message = OpenAiMessage {
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

		// Regular messages - handle both text and images
		if msg.role == "user" && msg.images.is_some() {
			// User message with images - use multimodal format
			let mut content_parts = Vec::new();

			// Add text content if not empty
			if !msg.content.trim().is_empty() {
				content_parts.push(serde_json::json!({
					"type": "text",
					"text": msg.content
				}));
			}

			// Add image attachments
			if let Some(ref images) = msg.images {
				for img in images {
					if let crate::session::image::ImageData::Base64(ref data) = img.data {
						content_parts.push(serde_json::json!({
							"type": "image_url",
							"image_url": {
								"url": format!("data:{};base64,{}", img.media_type, data)
							}
						}));
					}
				}
			}

			result.push(OpenAiMessage {
				role: msg.role.clone(),
				content: serde_json::json!(content_parts),
				tool_call_id: None,
				name: None,
				tool_calls: None,
			});
		} else {
			// Regular text-only messages
			result.push(OpenAiMessage {
				role: msg.role.clone(),
				content: serde_json::json!(msg.content),
				tool_call_id: None,
				name: None,
				tool_calls: None,
			});
		}
	}

	result
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_supports_temperature() {
		// Models that should support temperature
		assert!(supports_temperature("gpt-4"));
		assert!(supports_temperature("gpt-4o"));
		assert!(supports_temperature("gpt-4o-mini"));
		assert!(supports_temperature("gpt-3.5-turbo"));
		assert!(supports_temperature("chatgpt-4o-latest"));

		// Models that should NOT support temperature (o1/o2 series)
		assert!(!supports_temperature("o1"));
		assert!(!supports_temperature("o1-preview"));
		assert!(!supports_temperature("o1-mini"));
		assert!(!supports_temperature("o3"));
		assert!(!supports_temperature("o3-mini"));
		assert!(!supports_temperature("o4"));
	}

	#[test]
	fn test_supports_vision() {
		let provider = OpenAiProvider::new();

		// Models that should support vision
		assert!(provider.supports_vision("gpt-4o"));
		assert!(provider.supports_vision("gpt-4o-mini"));
		assert!(provider.supports_vision("gpt-4o-2024-05-13"));
		assert!(provider.supports_vision("gpt-4-turbo"));
		assert!(provider.supports_vision("gpt-4-vision-preview"));

		// Models that should NOT support vision
		assert!(!provider.supports_vision("gpt-3.5-turbo"));
		assert!(!provider.supports_vision("gpt-4"));
		assert!(!provider.supports_vision("o1-preview"));
		assert!(!provider.supports_vision("o1-mini"));
		assert!(!provider.supports_vision("text-davinci-003"));
	}
}
