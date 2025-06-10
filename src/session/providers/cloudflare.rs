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

// Cloudflare Workers AI provider implementation

use super::{AiProvider, ProviderExchange, ProviderResponse, TokenUsage};
use crate::config::Config;
use crate::log_debug;
use crate::session::Message;
use anyhow::Result;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::env;

/// Cloudflare Workers AI pricing constants (per 1M tokens in USD)
/// Source: https://developers.cloudflare.com/workers-ai/platform/pricing/ (as of January 2025)
const PRICING: &[(&str, f64, f64)] = &[
	// Model, Input price per 1M tokens, Output price per 1M tokens
	// Meta Llama models
	("llama-3.1-8b-instruct", 0.125, 0.125),
	("llama-3.1-70b-instruct", 0.59, 0.80),
	("llama-3.2-1b-instruct", 0.04, 0.04),
	("llama-3.2-3b-instruct", 0.06, 0.06),
	("llama-2-7b-chat", 0.125, 0.125),
	("llama-2-13b-chat", 0.25, 0.25),
	// Mistral models
	("mistral-7b-instruct", 0.125, 0.125),
	// Microsoft models
	("phi-2", 0.125, 0.125),
	// Qwen models
	("qwen1.5-0.5b-chat", 0.04, 0.04),
	("qwen1.5-1.8b-chat", 0.04, 0.04),
	("qwen1.5-7b-chat", 0.125, 0.125),
	("qwen1.5-14b-chat", 0.25, 0.25),
	// TinyLlama models
	("tinyllama-1.1b-chat", 0.04, 0.04),
	// Neural Chat models
	("neural-chat-7b", 0.125, 0.125),
	// Gemma models
	("gemma-2b-it", 0.04, 0.04),
	("gemma-7b-it", 0.125, 0.125),
	// Code Llama models
	("codellama-7b-instruct", 0.125, 0.125),
	// Hermes models
	("hermes-2-pro-mistral-7b", 0.125, 0.125),
];

/// Calculate cost for Cloudflare Workers AI models
fn calculate_cost(model: &str, prompt_tokens: u64, completion_tokens: u64) -> Option<f64> {
	for (pricing_model, input_price, output_price) in PRICING {
		if model.contains(pricing_model) {
			let input_cost = (prompt_tokens as f64 / 1_000_000.0) * input_price;
			let output_cost = (completion_tokens as f64 / 1_000_000.0) * output_price;
			return Some(input_cost + output_cost);
		}
	}
	// Default pricing for unknown models (roughly similar to small models)
	let input_cost = (prompt_tokens as f64 / 1_000_000.0) * 0.125;
	let output_cost = (completion_tokens as f64 / 1_000_000.0) * 0.125;
	Some(input_cost + output_cost)
}

/// Cloudflare Workers AI provider implementation
pub struct CloudflareWorkersAiProvider;

impl Default for CloudflareWorkersAiProvider {
	fn default() -> Self {
		Self::new()
	}
}

impl CloudflareWorkersAiProvider {
	pub fn new() -> Self {
		Self
	}

	/// Get Cloudflare API token
	fn get_api_token(&self) -> Result<String> {
		env::var("CLOUDFLARE_API_TOKEN")
			.map_err(|_| anyhow::anyhow!("CLOUDFLARE_API_TOKEN not found in environment"))
	}

	/// Get Cloudflare Account ID
	fn get_account_id(&self) -> Result<String> {
		env::var("CLOUDFLARE_ACCOUNT_ID")
			.map_err(|_| anyhow::anyhow!("CLOUDFLARE_ACCOUNT_ID not found in environment"))
	}

	/// Convert model name to full Cloudflare model identifier
	fn get_full_model_id(&self, model: &str) -> String {
		// If the model already has the @cf/ or @hf/ prefix, return as-is
		if model.starts_with("@cf/") || model.starts_with("@hf/") {
			model.to_string()
		} else {
			// Map common model names to full Cloudflare IDs
			match model {
				"llama-3.1-8b-instruct" => "@cf/meta/llama-3.1-8b-instruct".to_string(),
				"llama-3.1-70b-instruct" => "@cf/meta/llama-3.1-70b-instruct".to_string(),
				"llama-3.2-1b-instruct" => "@cf/meta/llama-3.2-1b-instruct".to_string(),
				"llama-3.2-3b-instruct" => "@cf/meta/llama-3.2-3b-instruct".to_string(),
				"llama-2-7b-chat" => "@cf/meta/llama-2-7b-chat-fp16".to_string(),
				"llama-2-13b-chat" => "@hf/thebloke/llama-2-13b-chat-awq".to_string(),
				"mistral-7b-instruct" => "@cf/mistral/mistral-7b-instruct-v0.1".to_string(),
				"phi-2" => "@cf/microsoft/phi-2".to_string(),
				"qwen1.5-0.5b-chat" => "@cf/qwen/qwen1.5-0.5b-chat".to_string(),
				"qwen1.5-1.8b-chat" => "@cf/qwen/qwen1.5-1.8b-chat".to_string(),
				"qwen1.5-7b-chat" => "@cf/qwen/qwen1.5-7b-chat".to_string(),
				"qwen1.5-14b-chat" => "@cf/qwen/qwen1.5-14b-chat".to_string(),
				"tinyllama-1.1b-chat" => "@cf/tinyllama/tinyllama-1.1b-chat-v1.0".to_string(),
				"neural-chat-7b" => "@cf/intel/neural-chat-7b-v3-1".to_string(),
				"gemma-2b-it" => "@cf/google/gemma-2b-it".to_string(),
				"gemma-7b-it" => "@cf/google/gemma-7b-it".to_string(),
				"codellama-7b-instruct" => "@cf/meta/codellama-7b-instruct-awq".to_string(),
				"hermes-2-pro-mistral-7b" => "@hf/nousresearch/hermes-2-pro-mistral-7b".to_string(),
				_ => model.to_string(), // Return as-is if no mapping found
			}
		}
	}
}

/// Message format for Cloudflare Workers AI API
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudflareMessage {
	pub role: String,
	pub content: CloudflareContent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CloudflareContent {
	Text(String),
	Multimodal(Vec<CloudflareContentPart>),
}

impl CloudflareContent {
	pub fn as_str(&self) -> &str {
		match self {
			CloudflareContent::Text(text) => text,
			CloudflareContent::Multimodal(parts) => {
				// For multimodal content, return the first text part or empty string
				for part in parts {
					if let CloudflareContentPart::Text { text } = part {
						return text;
					}
				}
				""
			}
		}
	}
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum CloudflareContentPart {
	#[serde(rename = "text")]
	Text { text: String },
	#[serde(rename = "image_url")]
	ImageUrl { image_url: CloudflareImageUrl },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudflareImageUrl {
	pub url: String,
}

#[async_trait::async_trait]
impl AiProvider for CloudflareWorkersAiProvider {
	fn name(&self) -> &str {
		"cloudflare"
	}

	fn supports_model(&self, model: &str) -> bool {
		// Cloudflare Workers AI supported models
		model.starts_with("@cf/")
			|| model.starts_with("@hf/")
			|| model.contains("llama")
			|| model.contains("mistral")
			|| model.contains("phi-")
			|| model.contains("qwen")
			|| model.contains("tinyllama")
			|| model.contains("neural-chat")
			|| model.contains("gemma")
			|| model.contains("codellama")
			|| model.contains("hermes")
	}

	fn get_api_key(&self, _config: &Config) -> Result<String> {
		// API keys now only from environment variables for security
		self.get_api_token()
	}

	fn supports_caching(&self, _model: &str) -> bool {
		// Cloudflare Workers AI doesn't currently support caching
		false
	}

	fn supports_vision(&self, model: &str) -> bool {
		// Cloudflare Workers AI vision-capable models
		// Llama 3.2 vision models support multimodal input
		// Source: https://developers.cloudflare.com/workers-ai/models/
		model.contains("llama-3.2") && model.contains("vision")
	}

	fn get_max_input_tokens(&self, model: &str) -> usize {
		// Cloudflare Workers AI model input limits (total context minus reserved output tokens)
		// Llama models: varies by version
		if model.contains("llama-3.1") || model.contains("llama-3.2") {
			return 128_000 - 4_096; // Reserve 4K for output = ~124K input max
		}
		if model.contains("llama") {
			return 32_768 - 2_048; // Older Llama models
		}
		// Qwen models: typically 32K
		if model.contains("qwen") {
			return 32_768 - 2_048;
		}
		// Mistral models: varies
		if model.contains("mistral") {
			return 32_768 - 2_048;
		}
		// Default conservative limit for Workers AI
		16_384 - 1_024
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
		// Get API credentials
		let api_token = self.get_api_key(config)?;
		let account_id = self.get_account_id()?;

		// Get full model ID
		let full_model_id = self.get_full_model_id(model);
		log_debug!("Using Cloudflare Workers AI model: {}", full_model_id);

		// Convert messages to Cloudflare format
		let cloudflare_messages = convert_messages(messages);

		// Create request body
		let request_body = serde_json::json!({
			"messages": cloudflare_messages,
			"temperature": temperature,
			"max_tokens": 16384,
		});

		// Build Cloudflare Workers AI API URL
		let api_url = format!(
			"https://api.cloudflare.com/client/v4/accounts/{}/ai/run/{}",
			account_id, full_model_id
		);

		// Create HTTP client
		let client = Client::new();

		// Track API request time
		let api_start = std::time::Instant::now();

		// Make the API request
		let response = client
			.post(&api_url)
			.header("Authorization", format!("Bearer {}", api_token))
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

			if let Some(errors) = response_json.get("errors").and_then(|e| e.as_array()) {
				for error in errors {
					if let Some(message) = error.get("message").and_then(|m| m.as_str()) {
						error_details.push(format!("Error: {}", message));
					}
				}
			}

			if error_details.len() == 1 {
				error_details.push(format!("Raw response: {}", response_text));
			}

			let full_error = error_details.join(" | ");
			return Err(anyhow::anyhow!(
				"Cloudflare Workers AI API error: {}",
				full_error
			));
		}

		// Check for success in response
		let success = response_json
			.get("success")
			.and_then(|s| s.as_bool())
			.unwrap_or(false);
		if !success {
			let error_message = response_json
				.get("errors")
				.and_then(|errors| errors.as_array())
				.and_then(|arr| arr.first())
				.and_then(|err| err.get("message"))
				.and_then(|msg| msg.as_str())
				.unwrap_or("Unknown error");
			return Err(anyhow::anyhow!(
				"Cloudflare Workers AI API error: {}",
				error_message
			));
		}

		// Extract content from response
		let content = response_json
			.get("result")
			.and_then(|result| result.get("response"))
			.and_then(|resp| resp.as_str())
			.unwrap_or("")
			.to_string();

		// Cloudflare Workers AI doesn't provide detailed token usage, so we estimate
		let prompt_text = cloudflare_messages
			.iter()
			.map(|m| m.content.as_str())
			.collect::<Vec<_>>()
			.join(" ");
		let estimated_prompt_tokens = (prompt_text.len() / 4) as u64; // Rough estimate: 4 chars per token
		let estimated_completion_tokens = (content.len() / 4) as u64;
		let total_tokens = estimated_prompt_tokens + estimated_completion_tokens;

		// Calculate estimated cost
		let cost = calculate_cost(
			&full_model_id,
			estimated_prompt_tokens,
			estimated_completion_tokens,
		);

		let usage = Some(TokenUsage {
			prompt_tokens: estimated_prompt_tokens,
			output_tokens: estimated_completion_tokens,
			total_tokens,
			cached_tokens: 0, // Cloudflare Workers AI doesn't support caching yet
			cost,
			request_time_ms: Some(api_time_ms), // Track API timing for Cloudflare
		});

		// Cloudflare Workers AI doesn't support tool calls in this basic implementation
		let tool_calls = None;

		// Cloudflare doesn't provide finish_reason in the same way
		let finish_reason = Some("stop".to_string());

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

// Convert our session messages to Cloudflare format
fn convert_messages(messages: &[Message]) -> Vec<CloudflareMessage> {
	let mut result = Vec::new();

	for msg in messages {
		// Skip tool messages for now - Cloudflare Workers AI has limited tool support
		if msg.role == "tool" {
			continue;
		}

		// Convert regular messages - handle multimodal content
		let content = if let Some(ref images) = msg.images {
			// Create multimodal content
			let mut parts = Vec::new();

			// Add text content if not empty
			if !msg.content.is_empty() {
				parts.push(CloudflareContentPart::Text {
					text: msg.content.clone(),
				});
			}

			// Add image attachments
			for image in images {
				if let crate::session::image::ImageData::Base64(ref base64_data) = image.data {
					let data_url = format!("data:{};base64,{}", image.media_type, base64_data);
					parts.push(CloudflareContentPart::ImageUrl {
						image_url: CloudflareImageUrl { url: data_url },
					});
				}
			}

			CloudflareContent::Multimodal(parts)
		} else {
			// Simple text content
			CloudflareContent::Text(msg.content.clone())
		};

		result.push(CloudflareMessage {
			role: match msg.role.as_str() {
				"assistant" => "assistant".to_string(),
				"user" => "user".to_string(),
				"system" => "system".to_string(),
				_ => "user".to_string(), // Default to user for unknown roles
			},
			content,
		});
	}

	result
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_supports_vision() {
		let provider = CloudflareWorkersAiProvider::new();

		// Models that should support vision
		assert!(provider.supports_vision("llama-3.2-11b-vision-instruct"));
		assert!(provider.supports_vision("llama-3.2-90b-vision-instruct"));

		// Models that should NOT support vision
		assert!(!provider.supports_vision("llama-3.1-8b-instruct"));
		assert!(!provider.supports_vision("llama-3.1-70b-instruct"));
		assert!(!provider.supports_vision("mistral-7b-instruct"));
		assert!(!provider.supports_vision("llama-3.2-1b-instruct")); // No vision
	}

	#[test]
	fn test_supports_caching() {
		let provider = CloudflareWorkersAiProvider::new();

		// Cloudflare doesn't support caching currently
		assert!(!provider.supports_caching("llama-3.2-11b-vision-instruct"));
		assert!(!provider.supports_caching("llama-3.1-8b-instruct"));
	}

	#[test]
	fn test_cloudflare_content_as_str() {
		// Test text content
		let text_content = CloudflareContent::Text("Hello world".to_string());
		assert_eq!(text_content.as_str(), "Hello world");

		// Test multimodal content with text
		let multimodal_content = CloudflareContent::Multimodal(vec![
			CloudflareContentPart::Text {
				text: "Describe this image".to_string(),
			},
			CloudflareContentPart::ImageUrl {
				image_url: CloudflareImageUrl {
					url: "data:image/jpeg;base64,abc123".to_string(),
				},
			},
		]);
		assert_eq!(multimodal_content.as_str(), "Describe this image");

		// Test multimodal content without text
		let image_only_content =
			CloudflareContent::Multimodal(vec![CloudflareContentPart::ImageUrl {
				image_url: CloudflareImageUrl {
					url: "data:image/jpeg;base64,abc123".to_string(),
				},
			}]);
		assert_eq!(image_only_content.as_str(), "");
	}
}
