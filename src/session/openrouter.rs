// Legacy OpenRouter API client for backward compatibility
// This file now acts as a wrapper around the new provider system

use anyhow::Result;
use serde::{Serialize, Deserialize};
use std::env;
use crate::config::Config;

// Legacy OpenRouter response with token usage (for backward compatibility)
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

// Legacy exchange record for logging (for backward compatibility)
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct OpenRouterExchange {
	pub request: serde_json::Value,
	pub response: serde_json::Value,
	pub timestamp: u64,
	pub usage: Option<TokenUsage>,
}

// Legacy constants
const OPENROUTER_API_KEY_ENV: &str = "OPENROUTER_API_KEY";

// Legacy API key getter (for backward compatibility)
pub fn get_api_key(config: &Config) -> Result<String, anyhow::Error> {
	// First check the config file
	if let Some(key) = &config.openrouter.api_key {
		return Ok(key.clone());
	}

	// Then fall back to environment variable
	match env::var(OPENROUTER_API_KEY_ENV) {
		Ok(key) => Ok(key),
		Err(_) => Err(anyhow::anyhow!("OpenRouter API key not found in config or environment"))
	}
}

/// Legacy message format for the OpenRouter API (for backward compatibility)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
	pub role: String,
	pub content: serde_json::Value,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub tool_call_id: Option<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub name: Option<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub tool_calls: Option<serde_json::Value>,
}

// Legacy message conversion function (for backward compatibility)
pub fn convert_messages(messages: &[super::Message]) -> Vec<Message> {
	// This is kept for backward compatibility but could be removed if not used elsewhere
	messages.iter().map(|msg| Message {
		role: msg.role.clone(),
		content: serde_json::json!(msg.content),
		tool_call_id: msg.tool_call_id.clone(),
		name: msg.name.clone(),
		tool_calls: msg.tool_calls.clone(),
	}).collect()
}

// Legacy wrapper that maintains backward compatibility
// This calls the new provider system internally
pub async fn chat_completion(
	messages: Vec<super::Message>,
	model: &str,
	temperature: f32,
	config: &Config,
) -> Result<(String, OpenRouterExchange, Option<Vec<crate::mcp::McpToolCall>>, Option<String>)> {
	// Use the new provider system
	let response = crate::session::chat_completion_with_provider(&messages, model, temperature, config).await?;

	// Convert ProviderExchange back to OpenRouterExchange for backward compatibility
	let openrouter_exchange = OpenRouterExchange {
		request: response.exchange.request,
		response: response.exchange.response,
		timestamp: response.exchange.timestamp,
		usage: response.exchange.usage.map(|usage| TokenUsage {
			prompt_tokens: usage.prompt_tokens,
			completion_tokens: usage.completion_tokens,
			total_tokens: usage.total_tokens,
			cost: usage.cost,
			completion_tokens_details: usage.completion_tokens_details,
			prompt_tokens_details: usage.prompt_tokens_details,
			breakdown: usage.breakdown,
		}),
	};

	Ok((response.content, openrouter_exchange, response.tool_calls, response.finish_reason))
}
