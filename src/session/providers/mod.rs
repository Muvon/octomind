// Provider abstraction layer for different AI providers

use anyhow::Result;
use serde::{Serialize, Deserialize};
use std::time::{SystemTime, UNIX_EPOCH};
use crate::config::Config;
use crate::session::Message;

pub mod openrouter;
pub mod openai;

// Re-export provider implementations
pub use openrouter::OpenRouterProvider;
pub use openai::OpenAiProvider;

/// Common token usage structure across all providers
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TokenUsage {
	pub prompt_tokens: u64,
	pub completion_tokens: u64,
	pub total_tokens: u64,
	#[serde(default)]
	pub cost: Option<f64>,
	pub completion_tokens_details: Option<serde_json::Value>,
	pub prompt_tokens_details: Option<serde_json::Value>,
	pub breakdown: Option<std::collections::HashMap<String, serde_json::Value>>,
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
	pub fn new(request: serde_json::Value, response: serde_json::Value, usage: Option<TokenUsage>, provider: &str) -> Self {
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
	pub tool_calls: Option<Vec<crate::session::mcp::McpToolCall>>,
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
	async fn chat_completion(
		&self,
		messages: &[Message],
		model: &str,
		temperature: f32,
		config: &Config,
	) -> Result<ProviderResponse>;

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
}

/// Provider factory to create the appropriate provider based on model string
pub struct ProviderFactory;

impl ProviderFactory {
	/// Parse a model string in format "provider:model" and return (provider_name, model_name)
	/// If no provider prefix is found, defaults to "openrouter"
	pub fn parse_model(model: &str) -> (String, String) {
		if let Some(pos) = model.find(':') {
			let provider = model[..pos].to_string();
			let model_name = model[pos + 1..].to_string();
			(provider, model_name)
		} else {
			// Default to openrouter if no provider specified
			("openrouter".to_string(), model.to_string())
		}
	}

	/// Create a provider instance based on the provider name
	pub fn create_provider(provider_name: &str) -> Result<Box<dyn AiProvider>> {
		match provider_name.to_lowercase().as_str() {
			"openrouter" => Ok(Box::new(OpenRouterProvider::new())),
			"openai" => Ok(Box::new(OpenAiProvider::new())),
			// Future providers will be added here:
			// "anthropic" => Ok(Box::new(AnthropicProvider::new())),
			_ => Err(anyhow::anyhow!("Unsupported provider: {}", provider_name)),
		}
	}

	/// Get the appropriate provider for a given model string
	pub fn get_provider_for_model(model: &str) -> Result<(Box<dyn AiProvider>, String)> {
		let (provider_name, model_name) = Self::parse_model(model);
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
		let (provider, model) = ProviderFactory::parse_model("openrouter:anthropic/claude-3.5-sonnet");
		assert_eq!(provider, "openrouter");
		assert_eq!(model, "anthropic/claude-3.5-sonnet");

		// Test with different provider
		let (provider, model) = ProviderFactory::parse_model("openai:gpt-4o");
		assert_eq!(provider, "openai");
		assert_eq!(model, "gpt-4o");

		// Test without provider prefix (should default to openrouter)
		let (provider, model) = ProviderFactory::parse_model("anthropic/claude-3.5-sonnet");
		assert_eq!(provider, "openrouter");
		assert_eq!(model, "anthropic/claude-3.5-sonnet");
	}

	#[test]
	fn test_create_provider() {
		// Test valid providers
		let provider = ProviderFactory::create_provider("openrouter");
		assert!(provider.is_ok());

		let provider = ProviderFactory::create_provider("openai");
		assert!(provider.is_ok());

		// Test invalid provider
		let provider = ProviderFactory::create_provider("invalid");
		assert!(provider.is_err());
	}
}
