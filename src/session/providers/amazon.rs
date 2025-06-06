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

// Amazon Bedrock provider implementation

use super::{AiProvider, ProviderExchange, ProviderResponse, TokenUsage};
use crate::config::Config;
use crate::log_debug;
use crate::session::Message;
use anyhow::Result;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::env;

/// Amazon Bedrock pricing constants (per 1M tokens in USD)
/// Source: https://aws.amazon.com/bedrock/pricing/ (as of January 2025)
const PRICING: &[(&str, f64, f64)] = &[
	// Model, Input price per 1M tokens, Output price per 1M tokens
	// Anthropic Claude models on Bedrock
	("claude-3-5-sonnet", 3.00, 15.00),
	("claude-3-5-haiku", 0.80, 4.00),
	("claude-3-opus", 15.00, 75.00),
	("claude-3-sonnet", 3.00, 15.00),
	("claude-3-haiku", 0.25, 1.25),
	// Meta Llama models on Bedrock
	("llama3-2-90b", 2.00, 2.00),
	("llama3-2-11b", 0.35, 0.35),
	("llama3-2-3b", 0.06, 0.06),
	("llama3-2-1b", 0.035, 0.035),
	("llama3-1-405b", 5.32, 16.00),
	("llama3-1-70b", 0.99, 0.99),
	("llama3-1-8b", 0.22, 0.22),
	// Cohere Command models on Bedrock
	("command-r-plus", 3.00, 15.00),
	("command-r", 0.50, 1.50),
	("command-light", 0.30, 0.60),
	// AI21 Jamba models on Bedrock
	("jamba-1-5-large", 2.00, 8.00),
	("jamba-1-5-mini", 0.20, 0.40),
];

/// Calculate cost for Amazon Bedrock models
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

/// Amazon Bedrock provider implementation
pub struct AmazonBedrockProvider;

impl Default for AmazonBedrockProvider {
	fn default() -> Self {
		Self::new()
	}
}

impl AmazonBedrockProvider {
	pub fn new() -> Self {
		Self
	}

	/// Get AWS region from environment or default
	fn get_aws_region(&self) -> String {
		env::var("AWS_REGION")
			.or_else(|_| env::var("AWS_DEFAULT_REGION"))
			.unwrap_or_else(|_| "us-east-1".to_string())
	}

	/// Get AWS access key ID
	fn get_aws_access_key_id(&self) -> Result<String> {
		env::var("AWS_ACCESS_KEY_ID")
			.map_err(|_| anyhow::anyhow!("AWS_ACCESS_KEY_ID not found in environment"))
	}

	/// Get AWS secret access key
	fn get_aws_secret_access_key(&self) -> Result<String> {
		env::var("AWS_SECRET_ACCESS_KEY")
			.map_err(|_| anyhow::anyhow!("AWS_SECRET_ACCESS_KEY not found in environment"))
	}

	/// Convert Bedrock model ID to full model name for API calls
	fn get_full_model_id(&self, model: &str) -> String {
		// If the model already contains dots (like anthropic.claude-3-5-sonnet-20241022-v2:0)
		// return as-is, otherwise construct the full ID
		if model.contains('.') {
			model.to_string()
		} else {
			// Map common model names to full Bedrock IDs
			match model {
				"claude-3-5-sonnet" => "anthropic.claude-3-5-sonnet-20241022-v2:0".to_string(),
				"claude-3-5-haiku" => "anthropic.claude-3-5-haiku-20241022-v1:0".to_string(),
				"claude-3-opus" => "anthropic.claude-3-opus-20240229-v1:0".to_string(),
				"claude-3-sonnet" => "anthropic.claude-3-sonnet-20240229-v1:0".to_string(),
				"claude-3-haiku" => "anthropic.claude-3-haiku-20240307-v1:0".to_string(),
				"llama3-2-90b" => "meta.llama3-2-90b-instruct-v1:0".to_string(),
				"llama3-2-11b" => "meta.llama3-2-11b-instruct-v1:0".to_string(),
				"llama3-2-3b" => "meta.llama3-2-3b-instruct-v1:0".to_string(),
				"llama3-2-1b" => "meta.llama3-2-1b-instruct-v1:0".to_string(),
				_ => model.to_string(), // Return as-is if no mapping found
			}
		}
	}

	/// Sign AWS request (simplified version for Bedrock)
	async fn sign_request(
		&self,
		_method: &str,
		_uri: &str,
		headers: &mut std::collections::HashMap<String, String>,
		_body: &str,
	) -> Result<()> {
		// This is a simplified AWS signature implementation
		// In production, you'd want to use aws-sigv4 crate or AWS SDK
		let access_key = self.get_aws_access_key_id()?;
		let _secret_key = self.get_aws_secret_access_key()?;
		let _region = self.get_aws_region();

		// Add required headers
		headers.insert(
			"Authorization".to_string(),
			format!("AWS4-HMAC-SHA256 Credential={}/...", access_key),
		);
		headers.insert(
			"X-Amz-Date".to_string(),
			chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string(),
		);
		headers.insert(
			"X-Amz-Target".to_string(),
			"BedrockRuntime.InvokeModel".to_string(),
		);

		// Note: This is a placeholder - actual AWS signing is complex
		// In a real implementation, you should use the aws-sigv4 crate
		Ok(())
	}
}

/// Message format for Amazon Bedrock API (varies by model family)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BedrockMessage {
	pub role: String,
	pub content: serde_json::Value,
}

#[async_trait::async_trait]
impl AiProvider for AmazonBedrockProvider {
	fn name(&self) -> &str {
		"amazon"
	}

	fn supports_model(&self, model: &str) -> bool {
		// Amazon Bedrock supported models
		model.contains("anthropic.claude")
			|| model.contains("meta.llama")
			|| model.contains("cohere.command")
			|| model.contains("ai21.jamba")
			|| model.contains("claude-3")
			|| model.contains("llama3")
			|| model.contains("command-")
			|| model.contains("jamba-")
	}

	fn get_api_key(&self, _config: &Config) -> Result<String> {
		// API keys now only from environment variables for security
		self.get_aws_access_key_id()
	}

	fn supports_caching(&self, model: &str) -> bool {
		// Bedrock supports some caching for Claude models
		model.contains("claude")
	}

	fn get_max_input_tokens(&self, model: &str) -> usize {
		// Amazon Bedrock model input limits (total context minus reserved output tokens)
		// Claude models on Bedrock: 200K total context
		if model.contains("claude") {
			return 200_000 - 32_768; // Reserve 32K for output = ~167K input max
		}
		// Llama models on Bedrock: varies by version
		if model.contains("llama-3.1") || model.contains("llama-3.2") {
			return 128_000 - 4_096; // Reserve 4K for output = ~124K input max
		}
		if model.contains("llama") {
			return 32_768 - 2_048; // Older Llama models
		}
		// Cohere models on Bedrock: typically 128K
		if model.contains("cohere") {
			return 128_000 - 4_096;
		}
		// Titan models on Bedrock: 32K
		if model.contains("titan") {
			return 32_768 - 2_048;
		}
		// Default conservative limit
		32_768 - 2_048
	}

	async fn chat_completion(
		&self,
		messages: &[Message],
		model: &str,
		temperature: f32,
		_config: &Config,
	) -> Result<ProviderResponse> {
		// Validate AWS credentials
		let _access_key = self.get_aws_access_key_id()?;
		let _secret_key = self.get_aws_secret_access_key()?;
		let region = self.get_aws_region();

		// Get full model ID
		let full_model_id = self.get_full_model_id(model);
		log_debug!("Using Bedrock model: {}", full_model_id);

		// Convert messages to Bedrock format
		let bedrock_messages = convert_messages(messages);

		// Create request body (format varies by model family)
		let request_body = if full_model_id.contains("anthropic.claude") {
			// Anthropic Claude format on Bedrock
			serde_json::json!({
				"anthropic_version": "bedrock-2023-05-31",
				"max_tokens": 16384,
				"temperature": temperature,
				"messages": bedrock_messages,
			})
		} else if full_model_id.contains("meta.llama") {
			// Meta Llama format on Bedrock
			serde_json::json!({
				"prompt": convert_messages_to_prompt(messages),
				"max_gen_len": 4096,
				"temperature": temperature,
			})
		} else {
			// Generic format
			serde_json::json!({
				"messages": bedrock_messages,
				"temperature": temperature,
				// "max_tokens": 4096,
			})
		};

		// Build Bedrock API URL
		let api_url = format!(
			"https://bedrock-runtime.{}.amazonaws.com/model/{}/invoke",
			region, full_model_id
		);

		// Create HTTP client
		let client = Client::new();

		// Prepare headers
		let mut headers = std::collections::HashMap::new();
		headers.insert("Content-Type".to_string(), "application/json".to_string());

		// Sign the request (simplified - in production use AWS SDK)
		self.sign_request("POST", &api_url, &mut headers, &request_body.to_string())
			.await?;

		// Make the API request
		let mut request_builder = client
			.post(&api_url)
			.header("Content-Type", "application/json")
			.json(&request_body);

		// Add signed headers
		for (key, value) in headers {
			request_builder = request_builder.header(&key, &value);
		}

		// Track API request time
		let api_start = std::time::Instant::now();

		let response = request_builder.send().await?;

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
			let error_message = response_json
				.get("message")
				.and_then(|m| m.as_str())
				.unwrap_or(&response_text);
			return Err(anyhow::anyhow!(
				"Amazon Bedrock API error ({}): {}",
				status,
				error_message
			));
		}

		// Extract content based on model family
		let content = if full_model_id.contains("anthropic.claude") {
			// Anthropic Claude response format
			response_json
				.get("content")
				.and_then(|content_arr| content_arr.as_array())
				.and_then(|arr| arr.first())
				.and_then(|first| first.get("text"))
				.and_then(|text| text.as_str())
				.unwrap_or("")
				.to_string()
		} else if full_model_id.contains("meta.llama") {
			// Meta Llama response format
			response_json
				.get("generation")
				.and_then(|gen| gen.as_str())
				.unwrap_or("")
				.to_string()
		} else {
			// Generic response format
			response_json
				.get("content")
				.and_then(|c| c.as_str())
				.unwrap_or("")
				.to_string()
		};

		// Extract token usage (format varies by model)
		let usage: Option<TokenUsage> = if let Some(usage_obj) = response_json.get("usage") {
			let prompt_tokens = usage_obj
				.get("input_tokens")
				.and_then(|v| v.as_u64())
				.unwrap_or(0);
			let completion_tokens = usage_obj
				.get("output_tokens")
				.and_then(|v| v.as_u64())
				.unwrap_or(0);
			let total_tokens = prompt_tokens + completion_tokens;

			// Calculate cost using our pricing constants
			let cost = calculate_cost(&full_model_id, prompt_tokens, completion_tokens);

			Some(TokenUsage {
				prompt_tokens,
				output_tokens: completion_tokens,
				total_tokens,
				cached_tokens: 0, // Amazon Bedrock doesn't support caching yet
				cost,
				request_time_ms: Some(api_time_ms), // Track API timing for Amazon
			})
		} else {
			None
		};

		// For now, Bedrock tool calls are not implemented in this basic version
		let tool_calls = None;

		// Extract finish reason
		let finish_reason = response_json
			.get("stop_reason")
			.and_then(|fr| fr.as_str())
			.map(|s| s.to_string());

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

// Convert our session messages to Bedrock format
fn convert_messages(messages: &[Message]) -> Vec<BedrockMessage> {
	let mut result = Vec::new();

	for msg in messages {
		// Skip system messages - they're handled differently in Bedrock
		if msg.role == "system" {
			continue;
		}

		// Convert regular messages
		result.push(BedrockMessage {
			role: match msg.role.as_str() {
				"assistant" => "assistant".to_string(),
				"user" => "user".to_string(),
				_ => "user".to_string(), // Default to user for unknown roles
			},
			content: serde_json::json!(msg.content),
		});
	}

	result
}

// Convert messages to a single prompt string (for models that expect prompt format)
fn convert_messages_to_prompt(messages: &[Message]) -> String {
	let mut prompt = String::new();

	for msg in messages {
		match msg.role.as_str() {
			"system" => {
				prompt.push_str(&format!("System: {}\n\n", msg.content));
			}
			"user" => {
				prompt.push_str(&format!("Human: {}\n\n", msg.content));
			}
			"assistant" => {
				prompt.push_str(&format!("Assistant: {}\n\n", msg.content));
			}
			_ => {
				prompt.push_str(&format!("{}: {}\n\n", msg.role, msg.content));
			}
		}
	}

	// Add final prompt for assistant response
	prompt.push_str("Assistant: ");

	prompt
}
