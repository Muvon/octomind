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

// Provider abstraction layer for different AI providers

use crate::config::Config;
use crate::session::Message;
use anyhow::Result;
use serde::{Deserialize, Serialize};

use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::watch;

/// Parameters for chat completion requests
///
/// This struct groups all parameters needed for AI provider chat completion calls,
/// following best practices for parameter passing and future extensibility.
#[derive(Clone)]
pub struct ChatCompletionParams<'a> {
	/// Array of conversation messages
	pub messages: &'a [Message],
	/// Model identifier (e.g., "claude-3-5-sonnet", "gpt-4")
	pub model: &'a str,
	/// Sampling temperature (0.0 to 2.0)
	pub temperature: f32,
	/// Top-p nucleus sampling (0.0 to 1.0)
	pub top_p: f32,
	/// Top-k sampling (1 to infinity)
	pub top_k: u32,
	/// Maximum tokens to generate (0 = no limit)
	pub max_tokens: u32,
	/// Maximum retry attempts on failure
	pub max_retries: u32,
	/// Base timeout for exponential backoff retry logic
	pub retry_timeout: std::time::Duration,
	/// Configuration object
	pub config: &'a Config,
	/// Cancellation token for request abortion
	pub cancellation_token: Option<watch::Receiver<bool>>,
}

impl<'a> ChatCompletionParams<'a> {
	/// Create new chat completion parameters
	pub fn new(
		messages: &'a [Message],
		model: &'a str,
		temperature: f32,
		top_p: f32,
		top_k: u32,
		max_tokens: u32,
		config: &'a Config,
	) -> Self {
		Self {
			messages,
			model,
			temperature,
			top_p,
			top_k,
			max_tokens,
			max_retries: config.max_retries,
			retry_timeout: std::time::Duration::from_secs(config.retry_timeout as u64),
			config,
			cancellation_token: None,
		}
	}

	/// Set maximum retry attempts
	pub fn with_max_retries(mut self, max_retries: u32) -> Self {
		self.max_retries = max_retries;
		self
	}

	/// Set cancellation token
	pub fn with_cancellation_token(mut self, token: watch::Receiver<bool>) -> Self {
		self.cancellation_token = Some(token);
		self
	}
}

pub mod amazon;
pub mod anthropic;
pub mod cloudflare;
pub mod deepseek;
pub mod google;
pub mod openai;
pub mod openrouter;
pub mod retry;

// Re-export provider implementations
pub use amazon::AmazonBedrockProvider;
pub use anthropic::AnthropicProvider;
pub use cloudflare::CloudflareWorkersAiProvider;
pub use deepseek::DeepSeekProvider;
pub use google::GoogleVertexProvider;
pub use openai::OpenAiProvider;
pub use openrouter::OpenRouterProvider;

/// Common token usage structure across all providers
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TokenUsage {
	pub prompt_tokens: u64, // ALL input tokens (user messages, system prompts, tool definitions, tool responses)
	pub output_tokens: u64, // AI-generated response tokens only
	pub total_tokens: u64,  // prompt_tokens + output_tokens
	pub cached_tokens: u64, // Subset of prompt_tokens that came from cache (discounted)
	#[serde(default)]
	pub cost: Option<f64>, // Pre-calculated total cost (provider handles cache pricing)
	// Time tracking
	#[serde(default)]
	pub request_time_ms: Option<u64>, // Time spent on this API request
}

/// Common exchange record for logging across all providers
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ProviderExchange {
	pub request: serde_json::Value,
	pub response: serde_json::Value,
	pub timestamp: u64,
	pub usage: Option<TokenUsage>,
	pub provider: String, // Which provider was used
}

impl ProviderExchange {
	pub fn new(
		request: serde_json::Value,
		response: serde_json::Value,
		usage: Option<TokenUsage>,
		provider: &str,
	) -> Self {
		Self {
			request,
			response,
			timestamp: SystemTime::now()
				.duration_since(UNIX_EPOCH)
				.unwrap_or_default()
				.as_secs(),
			usage,
			provider: provider.to_string(),
		}
	}
}

/// Provider response containing the AI completion
#[derive(Debug, Clone)]
pub struct ProviderResponse {
	pub content: String,
	pub exchange: ProviderExchange,
	pub tool_calls: Option<Vec<crate::mcp::McpToolCall>>,
	pub finish_reason: Option<String>,
}

/// Trait that all AI providers must implement
#[async_trait::async_trait]
pub trait AiProvider: Send + Sync {
	/// Get the provider name (e.g., "openrouter", "openai", "anthropic")
	fn name(&self) -> &str;

	/// Check if the provider supports the given model
	fn supports_model(&self, model: &str) -> bool;

	/// Send a chat completion request
	async fn chat_completion(&self, params: ChatCompletionParams<'_>) -> Result<ProviderResponse>;

	/// Get API key for this provider from config or environment
	fn get_api_key(&self, config: &Config) -> Result<String>;

	/// Check if the provider/model supports caching
	fn supports_caching(&self, _model: &str) -> bool {
		// Default implementation - providers can override
		false
	}

	/// Get provider-specific configuration from the config
	fn get_provider_config<'a>(&self, _config: &'a Config) -> Option<&'a serde_json::Value> {
		// Default implementation - providers can override if they have specific config sections
		None
	}

	/// Get maximum input tokens for a model (actual context window size)
	/// This is what we can send to the API - the provider handles output limits internally
	fn get_max_input_tokens(&self, model: &str) -> usize;

	/// Check if the provider/model supports vision capabilities
	fn supports_vision(&self, _model: &str) -> bool {
		// Default implementation - providers can override
		false
	}
}

/// Provider factory to create the appropriate provider based on model string
pub struct ProviderFactory;

impl ProviderFactory {
	/// Parse a model string in format "provider:model" and return (provider_name, model_name)
	/// Provider prefix is now REQUIRED
	pub fn parse_model(model: &str) -> Result<(String, String)> {
		if let Some(pos) = model.find(':') {
			let provider = model[..pos].to_string();
			let model_name = model[pos + 1..].to_string();

			if provider.is_empty() || model_name.is_empty() {
				return Err(anyhow::anyhow!(
					"Invalid model format. Use 'provider:model' (e.g., 'openai:gpt-4o')"
				));
			}

			Ok((provider, model_name))
		} else {
			Err(anyhow::anyhow!("Invalid model format '{}'. Must specify provider like 'openai:gpt-4o' or 'openrouter:anthropic/claude-3.5-sonnet'", model))
		}
	}

	/// Create a provider instance based on the provider name
	pub fn create_provider(provider_name: &str) -> Result<Box<dyn AiProvider>> {
		match provider_name.to_lowercase().as_str() {
			"openrouter" => Ok(Box::new(OpenRouterProvider::new())),
			"openai" => Ok(Box::new(OpenAiProvider::new())),
			"anthropic" => Ok(Box::new(AnthropicProvider::new())),
			"google" => Ok(Box::new(GoogleVertexProvider::new())),
			"amazon" => Ok(Box::new(AmazonBedrockProvider::new())),
			"cloudflare" => Ok(Box::new(CloudflareWorkersAiProvider::new())),
			"deepseek" => Ok(Box::new(DeepSeekProvider::new())),
			_ => Err(anyhow::anyhow!("Unsupported provider: {}. Supported providers: openrouter, openai, anthropic, google, amazon, cloudflare, deepseek", provider_name)),
		}
	}

	/// Get the appropriate provider for a given model string
	pub fn get_provider_for_model(model: &str) -> Result<(Box<dyn AiProvider>, String)> {
		let (provider_name, model_name) = Self::parse_model(model)?;
		let provider = Self::create_provider(&provider_name)?;

		// Verify the provider supports this model
		if !provider.supports_model(&model_name) {
			return Err(anyhow::anyhow!(
				"Provider '{}' does not support model '{}'",
				provider_name,
				model_name
			));
		}

		Ok((provider, model_name))
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_parse_model() {
		// Test with provider prefix
		let result = ProviderFactory::parse_model("openrouter:anthropic/claude-3.5-sonnet");
		assert!(result.is_ok());
		let (provider, model) = result.unwrap();
		assert_eq!(provider, "openrouter");
		assert_eq!(model, "anthropic/claude-3.5-sonnet");

		// Test with different provider
		let result = ProviderFactory::parse_model("openai:gpt-4o");
		assert!(result.is_ok());
		let (provider, model) = result.unwrap();
		assert_eq!(provider, "openai");
		assert_eq!(model, "gpt-4o");

		// Test DeepSeek provider
		let result = ProviderFactory::parse_model("deepseek:deepseek-chat");
		assert!(result.is_ok());
		let (provider, model) = result.unwrap();
		assert_eq!(provider, "deepseek");
		assert_eq!(model, "deepseek-chat");

		let result = ProviderFactory::parse_model("deepseek:deepseek-reasoner");
		assert!(result.is_ok());
		let (provider, model) = result.unwrap();
		assert_eq!(provider, "deepseek");
		assert_eq!(model, "deepseek-reasoner");

		// Test without provider prefix (should fail now)
		let result = ProviderFactory::parse_model("anthropic/claude-3.5-sonnet");
		assert!(result.is_err());

		// Test empty provider
		let result = ProviderFactory::parse_model(":gpt-4o");
		assert!(result.is_err());

		// Test empty model
		let result = ProviderFactory::parse_model("openai:");
		assert!(result.is_err());
	}

	#[test]
	fn test_create_provider() {
		// Test valid providers
		let provider = ProviderFactory::create_provider("openrouter");
		assert!(provider.is_ok());

		let provider = ProviderFactory::create_provider("openai");
		assert!(provider.is_ok());

		let provider = ProviderFactory::create_provider("anthropic");
		assert!(provider.is_ok());

		let provider = ProviderFactory::create_provider("google");
		assert!(provider.is_ok());

		let provider = ProviderFactory::create_provider("amazon");
		assert!(provider.is_ok());

		let provider = ProviderFactory::create_provider("cloudflare");
		assert!(provider.is_ok());

		let provider = ProviderFactory::create_provider("deepseek");
		assert!(provider.is_ok());

		// Test invalid provider
		let provider = ProviderFactory::create_provider("invalid");
		assert!(provider.is_err());
	}
}
