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

use super::{AiProvider, ChatCompletionParams, ProviderExchange, ProviderResponse, TokenUsage};
use crate::config::Config;
use crate::providers::retry;
use crate::session::Message;
use anyhow::Result;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::env;

/// OpenAI pricing constants (per 1M tokens in USD)
/// Source: https://platform.openai.com/docs/pricing (as of January 2025)
const PRICING: &[(&str, f64, f64)] = &[
	// Model, Input price per 1M tokens, Output price per 1M tokens
	// Latest models (2025)
	// GPT-4.1 and variants
	("gpt-4.1", 2.00, 8.00),
	("gpt-4.1-2025-04-14", 2.00, 8.00),
	("gpt-4.1-mini", 0.40, 1.60),
	("gpt-4.1-mini-2025-04-14", 0.40, 1.60),
	("gpt-4.1-nano", 0.10, 0.40),
	("gpt-4.1-nano-2025-04-14", 0.10, 0.40),
	// GPT-4.5
	("gpt-4.5-preview", 75.00, 150.00),
	("gpt-4.5-preview-2025-02-27", 75.00, 150.00),
	// GPT-4o series
	("gpt-4o", 2.50, 10.00),
	("gpt-4o-2024-08-06", 2.50, 10.00),
	("gpt-4o-realtime-preview", 5.00, 20.00),
	("gpt-4o-realtime-preview-2025-06-03", 5.00, 20.00),
	("gpt-4o-mini", 0.15, 0.60),
	("gpt-4o-mini-2024-07-18", 0.15, 0.60),
	("gpt-4o-mini-realtime-preview", 0.60, 2.40),
	("gpt-4o-mini-realtime-preview-2024-12-17", 0.60, 2.40),
	("gpt-4o-mini-search-preview", 0.15, 0.60),
	("gpt-4o-mini-search-preview-2025-03-11", 0.15, 0.60),
	("gpt-4o-search-preview", 2.50, 10.00),
	("gpt-4o-search-preview-2025-03-11", 2.50, 10.00),
	// O-series and variants
	("o1", 15.00, 60.00),
	("o1-2024-12-17", 15.00, 60.00),
	("o1-pro", 150.00, 600.00),
	("o1-pro-2025-03-19", 150.00, 600.00),
	("o1-mini", 1.10, 4.40),
	("o1-mini-2024-09-12", 1.10, 4.40),
	("o3", 2.00, 8.00),
	("o3-2025-04-16", 2.00, 8.00),
	("o3-pro", 20.00, 80.00),
	("o3-pro-2025-06-10", 20.00, 80.00),
	("o3-mini", 1.10, 4.40),
	("o3-mini-2025-01-31", 1.10, 4.40),
	("o3-deep-research", 10.00, 40.00),
	("o3-deep-research-2025-06-26", 10.00, 40.00),
	("o4-mini", 1.10, 4.40),
	("o4-mini-2025-04-16", 1.10, 4.40),
	("o4-mini-deep-research", 2.00, 8.00),
	("o4-mini-deep-research-2025-06-26", 2.00, 8.00),
	// GPT-4 Turbo
	("gpt-4-turbo", 10.00, 30.00),
	("gpt-4-turbo-2024-04-09", 10.00, 30.00),
	// GPT-4
	("gpt-4", 30.00, 60.00),
	("gpt-4-0613", 30.00, 60.00),
	("gpt-4-32k", 60.00, 120.00),
	// GPT-3.5 Turbo
	("gpt-3.5-turbo", 0.50, 1.50),
	("gpt-3.5-turbo-0125", 0.50, 1.50),
	("gpt-3.5-turbo-instruct", 1.50, 2.00),
	("gpt-3.5-turbo-16k-0613", 3.00, 4.00),
	// End of models
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
/// Calculate cost with cache-aware pricing
/// This function handles the different pricing tiers for cached vs non-cached tokens:
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

/// OpenAI provider implementation with intelligent rate limiting
///
/// This provider includes sophisticated retry logic that:
/// - Uses exponential backoff for network failures
/// - Parses OpenAI rate limit headers for optimal retry timing
/// - Respects retry-after headers for 429 responses
/// - Logs rate limit status for debugging
/// - Only retries when max_retries > 0 is specified
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

	async fn chat_completion(&self, params: ChatCompletionParams<'_>) -> Result<ProviderResponse> {
		// Don't check cancellation at start - only during the actual request
		// This prevents stale cancellation state from blocking new requests
		// Get API key
		let api_key = self.get_api_key(params.config)?;

		// Convert messages to OpenAI format
		let openai_messages = convert_messages(params.messages);

		// Create the request body
		let mut request_body = serde_json::json!({
			"model": params.model,
			"messages": openai_messages,
		});

		// Only add temperature for models that support it
		// O1/O2 series models don't support temperature parameter
		if supports_temperature(params.model) {
			request_body["temperature"] = serde_json::json!(params.temperature);
			// OpenAI supports top_p and top_k for most models
			request_body["top_p"] = serde_json::json!(params.top_p);
			// Note: OpenAI doesn't have top_k parameter, but has similar "top_logprobs"
			// We'll skip top_k for OpenAI as it's not directly supported
		}

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

		// Check for cancellation before making HTTP request
		if let Some(ref token) = params.cancellation_token {
			if *token.borrow() {
				return Err(anyhow::anyhow!("Request cancelled before HTTP call"));
			}
		}

		// Use retry.rs with smart header-based delay calculation
		if params.max_retries > 0 {
			crate::log_debug!(
				"🔄 OpenAI provider configured with {} max retries",
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
					execute_openai_request(api_key, request_body, cancellation_token).await
				})
			},
			params.max_retries,
			std::time::Duration::from_millis(1000), // Base timeout for fallback
			params.cancellation_token.as_ref(),
		)
		.await;

		match result {
			Ok(response_data) => {
				// Create exchange record
				let exchange = ProviderExchange::new(
					request_body,
					response_data.response_json.clone(),
					response_data.usage,
					self.name(),
				);

				Ok(ProviderResponse {
					content: response_data.content,
					exchange,
					tool_calls: response_data.tool_calls,
					finish_reason: response_data.finish_reason,
				})
			}
			Err(e) => Err(anyhow::anyhow!("OpenAI API request failed: {}", e)),
		}
	}
}

/// Response data structure for OpenAI HTTP requests
#[derive(Debug)]
struct OpenAiResponseData {
	content: String,
	response_json: serde_json::Value,
	usage: Option<TokenUsage>,
	tool_calls: Option<Vec<crate::mcp::McpToolCall>>,
	finish_reason: Option<String>,
}

/// Execute a single OpenAI HTTP request with smart retry delay calculation
async fn execute_openai_request(
	api_key: String,
	request_body: serde_json::Value,
	cancellation_token: Option<tokio::sync::watch::Receiver<bool>>,
) -> Result<OpenAiResponseData, String> {
	// Don't check cancellation at start - only during the actual request
	// This prevents stale cancellation state from blocking new requests

	// Create HTTP client
	let client = Client::new();

	// Track API request time
	let api_start = std::time::Instant::now();

	// Create the HTTP request
	let request = client
		.post(OPENAI_API_URL)
		.header("Authorization", format!("Bearer {}", api_key))
		.header("Content-Type", "application/json")
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

		// Determine retry delay using OpenAI rate limit headers
		if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
			crate::log_info!("🚦 OpenAI rate limit exceeded (429) - analyzing headers for optimal retry timing...");

			// Debug: Log all headers received in response for troubleshooting
			crate::log_debug!("📋 OpenAI 429 response headers: {:?}", headers);

			// Check for OpenAI's retry-after header first
			if let Some(retry_after) = headers.get("retry-after") {
				if let Ok(retry_after_str) = retry_after.to_str() {
					if let Ok(retry_seconds) = retry_after_str.parse::<u64>() {
						let delay_ms = retry_seconds * 1000;
						crate::log_info!(
							"📋 Using retry-after header: waiting {:.1}s as specified by OpenAI",
							delay_ms as f64 / 1000.0
						);
						// Sleep here with smart delay, then return error to trigger retry
						tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
						return Err(format!("Rate limit exceeded (429): {}", response_text));
					}
				}
			}

			// Check for OpenAI rate limit reset headers to calculate smart delay
			if let Some(delay_ms) = calculate_smart_retry_delay(&headers, 0) {
				crate::log_info!(
					"🎯 Using OpenAI rate limit reset time: waiting {:.1}s",
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
		return Err(format!("OpenAI API error: {}", full_error));
	}

	// Check for errors in response body even with HTTP 200
	if let Some(error_obj) = response_json.get("error") {
		let mut error_details = Vec::new();
		error_details.push("HTTP 200 but error in response".to_string());

		if let Some(msg) = error_obj.get("message").and_then(|m| m.as_str()) {
			error_details.push(format!("Message: {}", msg));
		}
		if let Some(code) = error_obj.get("code").and_then(|c| c.as_str()) {
			error_details.push(format!("Code: {}", code));
		}
		if let Some(type_) = error_obj.get("type").and_then(|t| t.as_str()) {
			error_details.push(format!("Type: {}", type_));
		}

		let full_error = error_details.join(" | ");
		return Err(format!("OpenAI API error: {}", full_error));
	}

	// Extract content from the response
	let content = response_json
		.get("choices")
		.and_then(|choices| choices.as_array())
		.and_then(|choices| choices.first())
		.and_then(|choice| choice.get("message"))
		.and_then(|message| message.get("content"))
		.and_then(|content| content.as_str())
		.map(|s| s.to_string())
		.unwrap_or_default();

	// Check for cache hit headers first
	let cache_creation_input_tokens = headers
		.get("x-cache-creation-input-tokens")
		.and_then(|h| h.to_str().ok())
		.and_then(|s| s.parse::<u32>().ok())
		.unwrap_or(0);

	let cache_read_input_tokens = headers
		.get("x-cache-read-input-tokens")
		.and_then(|h| h.to_str().ok())
		.and_then(|s| s.parse::<u32>().ok())
		.unwrap_or(0);

	// Extract token usage
	let usage = if let Some(usage_obj) = response_json.get("usage") {
		let prompt_tokens = usage_obj
			.get("prompt_tokens")
			.and_then(|v| v.as_u64())
			.unwrap_or(0) as u32;
		let completion_tokens = usage_obj
			.get("completion_tokens")
			.and_then(|v| v.as_u64())
			.unwrap_or(0) as u32;

		// Calculate cost using local pricing tables if model is available
		let cost = request_body
			.get("model")
			.and_then(|m| m.as_str())
			.and_then(|model| {
				if cache_creation_input_tokens > 0 || cache_read_input_tokens > 0 {
					// Use cache-aware pricing when cache tokens are present
					let regular_input_tokens =
						prompt_tokens.saturating_sub(cache_read_input_tokens) as u64;
					calculate_cost_with_cache(
						model,
						regular_input_tokens,
						cache_read_input_tokens as u64,
						completion_tokens as u64,
					)
				} else {
					// Use basic pricing when no cache tokens
					calculate_cost(model, prompt_tokens as u64, completion_tokens as u64)
				}
			});

		Some(TokenUsage {
			prompt_tokens: prompt_tokens as u64,
			output_tokens: completion_tokens as u64,
			total_tokens: (prompt_tokens + completion_tokens) as u64,
			cached_tokens: cache_read_input_tokens as u64,
			cost,
			request_time_ms: Some(api_time_ms),
		})
	} else {
		None
	};

	// Extract tool calls if present
	let tool_calls = response_json
		.get("choices")
		.and_then(|choices| choices.as_array())
		.and_then(|choices| choices.first())
		.and_then(|choice| choice.get("message"))
		.and_then(|message| message.get("tool_calls"))
		.and_then(|tool_calls| tool_calls.as_array())
		.map(|tool_calls| {
			tool_calls
				.iter()
				.filter_map(|tool_call| {
					let function = tool_call.get("function")?;
					let name = function.get("name")?.as_str()?.to_string();
					let arguments = function.get("arguments")?.as_str()?.to_string();

					// ✅ CRITICAL FIX: Extract the tool_call_id from OpenAI response (same as OpenRouter)
					let tool_id = tool_call.get("id").and_then(|i| i.as_str()).unwrap_or("");

					// Parse arguments as JSON
					match serde_json::from_str::<serde_json::Value>(&arguments) {
						Ok(args) => Some(crate::mcp::McpToolCall {
							tool_name: name,
							parameters: args,
							tool_id: tool_id.to_string(), // ✅ FIXED: Use extracted tool_call_id
						}),
						Err(_) => None,
					}
				})
				.collect::<Vec<_>>()
		})
		.filter(|calls| !calls.is_empty());

	// Extract finish reason
	let finish_reason = response_json
		.get("choices")
		.and_then(|choices| choices.as_array())
		.and_then(|choices| choices.first())
		.and_then(|choice| choice.get("finish_reason"))
		.and_then(|reason| reason.as_str())
		.map(|s| s.to_string());

	// Log rate limit information for successful requests
	log_rate_limit_info(&headers);

	// Log API timing
	crate::log_debug!("⏱️  OpenAI API request completed in {}ms", api_time_ms);

	// Log cache information if available
	if cache_creation_input_tokens > 0 || cache_read_input_tokens > 0 {
		crate::log_debug!(
			"💾 OpenAI cache info: {} creation tokens, {} read tokens",
			cache_creation_input_tokens,
			cache_read_input_tokens
		);
	}

	Ok(OpenAiResponseData {
		content,
		response_json,
		usage,
		tool_calls,
		finish_reason,
	})
}

/// Calculate smart retry delay based on OpenAI rate limit headers
/// Returns None if no suitable headers are found, falling back to exponential backoff
fn calculate_smart_retry_delay(headers: &reqwest::header::HeaderMap, attempt: u32) -> Option<u64> {
	// Check for token-based rate limit reset times
	let token_reset = headers
		.get("x-ratelimit-reset-tokens")
		.and_then(|h| h.to_str().ok())
		.and_then(parse_openai_duration);

	let request_reset = headers
		.get("x-ratelimit-reset-requests")
		.and_then(|h| h.to_str().ok())
		.and_then(parse_openai_duration);

	// Find the earliest reset time (most restrictive limit)
	let earliest_reset_seconds = [token_reset, request_reset]
		.iter()
		.filter_map(|&reset| reset)
		.min();

	if let Some(reset_seconds) = earliest_reset_seconds {
		if reset_seconds > 0 {
			// Add a small buffer (1-2 seconds) to ensure we're past the reset time
			let wait_ms = (reset_seconds * 1000) + 1000 + (attempt as u64 * 500);

			// Cap the wait time to reasonable limits (max 5 minutes)
			let max_wait_ms = 5 * 60 * 1000; // 5 minutes
			let final_wait_ms = wait_ms.min(max_wait_ms);

			crate::log_info!(
				"🎯 Using OpenAI rate limit reset time: waiting {:.1}s (limits reset in {}s)",
				final_wait_ms as f64 / 1000.0,
				reset_seconds
			);

			return Some(final_wait_ms);
		}
	}

	None
}

/// Parse OpenAI duration format (e.g., "1s", "6m0s") to seconds
fn parse_openai_duration(duration_str: &str) -> Option<u64> {
	let mut total_seconds = 0u64;
	let mut current_number = String::new();

	for ch in duration_str.chars() {
		if ch.is_ascii_digit() {
			current_number.push(ch);
		} else if !current_number.is_empty() {
			if let Ok(num) = current_number.parse::<u64>() {
				match ch {
					's' => total_seconds += num,
					'm' => total_seconds += num * 60,
					'h' => total_seconds += num * 3600,
					_ => {} // Ignore unknown units
				}
			}
			current_number.clear();
		}
	}

	if total_seconds > 0 {
		Some(total_seconds)
	} else {
		None
	}
}

/// Log rate limit information from OpenAI headers for debugging
fn log_rate_limit_info(headers: &reqwest::header::HeaderMap) {
	let mut rate_limit_info = Vec::new();

	// Request limits
	if let (Some(limit), Some(remaining)) = (
		headers
			.get("x-ratelimit-limit-requests")
			.and_then(|h| h.to_str().ok()),
		headers
			.get("x-ratelimit-remaining-requests")
			.and_then(|h| h.to_str().ok()),
	) {
		rate_limit_info.push(format!("Requests: {}/{}", remaining, limit));
	}

	// Token limits
	if let (Some(limit), Some(remaining)) = (
		headers
			.get("x-ratelimit-limit-tokens")
			.and_then(|h| h.to_str().ok()),
		headers
			.get("x-ratelimit-remaining-tokens")
			.and_then(|h| h.to_str().ok()),
	) {
		rate_limit_info.push(format!("Tokens: {}/{}", remaining, limit));
	}

	// Reset times
	if let Some(token_reset) = headers
		.get("x-ratelimit-reset-tokens")
		.and_then(|h| h.to_str().ok())
	{
		rate_limit_info.push(format!("Token reset: {}", token_reset));
	}

	if let Some(request_reset) = headers
		.get("x-ratelimit-reset-requests")
		.and_then(|h| h.to_str().ok())
	{
		rate_limit_info.push(format!("Request reset: {}", request_reset));
	}

	if !rate_limit_info.is_empty() {
		crate::log_info!("📊 OpenAI rate limits: {}", rate_limit_info.join(" | "));
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

	#[test]
	fn test_parse_openai_duration() {
		// Test various OpenAI duration formats
		assert_eq!(parse_openai_duration("1s"), Some(1));
		assert_eq!(parse_openai_duration("30s"), Some(30));
		assert_eq!(parse_openai_duration("1m"), Some(60));
		assert_eq!(parse_openai_duration("6m0s"), Some(360));
		assert_eq!(parse_openai_duration("1h30m"), Some(5400));
		assert_eq!(parse_openai_duration("2h15m30s"), Some(8130));

		// Test invalid formats
		assert_eq!(parse_openai_duration(""), None);
		assert_eq!(parse_openai_duration("invalid"), None);
		assert_eq!(parse_openai_duration("0s"), None);
	}
}
