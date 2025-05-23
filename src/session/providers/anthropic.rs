// Anthropic provider implementation

use anyhow::Result;
use reqwest::Client;
use serde::{Serialize, Deserialize};
use std::env;
use crate::config::Config;
use crate::session::Message;
use super::{AiProvider, ProviderResponse, ProviderExchange, TokenUsage};
use crate::log_debug;

/// Anthropic pricing constants (per 1M tokens in USD)
/// Source: https://www.anthropic.com/pricing (as of January 2025)
const PRICING: &[(&str, f64, f64)] = &[
	// Model, Input price per 1M tokens, Output price per 1M tokens
	("claude-3-5-sonnet", 3.00, 15.00),
	("claude-3-5-haiku", 0.25, 1.25),
	("claude-3-opus", 15.00, 75.00),
	("claude-3-sonnet", 3.00, 15.00),
	("claude-3-haiku", 0.25, 1.25),
	("claude-2.1", 8.00, 24.00),
	("claude-2.0", 8.00, 24.00),
	("claude-instant-1.2", 0.80, 2.40),
];

/// Calculate cost for Anthropic models
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

/// Anthropic provider implementation
pub struct AnthropicProvider;

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
		model.starts_with("claude-") ||
		model.contains("claude")
	}

	fn get_api_key(&self, _config: &Config) -> Result<String> {
		match env::var(ANTHROPIC_API_KEY_ENV) {
			Ok(key) => Ok(key),
			Err(_) => Err(anyhow::anyhow!("Anthropic API key not found in environment variable {}", ANTHROPIC_API_KEY_ENV))
		}
	}

	fn supports_caching(&self, model: &str) -> bool {
		// Anthropic supports caching for Claude 3.5 models
		model.contains("claude-3-5") || model.contains("claude-3.5")
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

		// Convert messages to Anthropic format
		let anthropic_messages = convert_messages(messages);

		// Extract system message if present
		let system_message = messages.iter()
			.find(|m| m.role == "system")
			.map(|m| m.content.clone())
			.unwrap_or_else(|| "You are a helpful assistant.".to_string());

		// Create the request body
		let mut request_body = serde_json::json!({
			"model": model,
			"max_tokens": 4096,
			"messages": anthropic_messages,
			"system": system_message,
			"temperature": temperature,
		});

		// Add tool definitions if MCP is enabled
		if config.mcp.enabled {
			let functions = crate::session::mcp::get_available_functions(config).await;
			if !functions.is_empty() {
				let tools = functions.iter().map(|f| {
					serde_json::json!({
						"name": f.name,
						"description": f.description,
						"input_schema": f.parameters
					})
				}).collect::<Vec<_>>();

				request_body["tools"] = serde_json::json!(tools);
			}
		}

		// Create HTTP client
		let client = Client::new();

		// Make the actual API request
		let response = client.post(ANTHROPIC_API_URL)
			.header("Authorization", format!("Bearer {}", api_key))
			.header("Content-Type", "application/json")
			.header("anthropic-version", "2023-06-01")
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
				return Err(anyhow::anyhow!("Failed to parse response JSON: {}. Response: {}", e, response_text));
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
						content_block.get("id").and_then(|i| i.as_str())
					) {
						let mcp_call = crate::session::mcp::McpToolCall {
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
		let finish_reason = response_json.get("stop_reason")
			.and_then(|fr| fr.as_str())
			.map(|s| s.to_string());

		if let Some(ref reason) = finish_reason {
			log_debug!("Stop reason: {}", reason);
		}

		// Extract token usage
		let usage: Option<TokenUsage> = if let Some(usage_obj) = response_json.get("usage") {
			let input_tokens = usage_obj.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
			let output_tokens = usage_obj.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
			let total_tokens = input_tokens + output_tokens;

			// Calculate cost using our pricing constants
			let cost = calculate_cost(model, input_tokens, output_tokens);

			Some(TokenUsage {
				prompt_tokens: input_tokens,
				completion_tokens: output_tokens,
				total_tokens,
				cost,
				completion_tokens_details: None,
				prompt_tokens_details: None,
				breakdown: None,
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

// Convert our session messages to Anthropic format
fn convert_messages(messages: &[Message]) -> Vec<AnthropicMessage> {
	let mut result = Vec::new();

	for msg in messages {
		// Skip system messages as they're handled separately
		if msg.role == "system" {
			continue;
		}

		// Handle tool response messages (has <fnr> tags)
		if msg.role == "user" && msg.content.starts_with("<fnr>") && msg.content.ends_with("</fnr>") {
			let content = msg.content.trim_start_matches("<fnr>").trim_end_matches("</fnr>").trim();

			if let Ok(tool_responses) = serde_json::from_str::<Vec<serde_json::Value>>(content) {
				if !tool_responses.is_empty() && tool_responses[0].get("role").map_or(false, |r| r.as_str().unwrap_or("") == "tool") {
					for tool_response in tool_responses {
						let tool_call_id = tool_response.get("tool_call_id")
							.and_then(|id| id.as_str())
							.unwrap_or("");

						let content_text = tool_response.get("content")
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
		} else if msg.role == "tool" {
			let tool_call_id = msg.tool_call_id.clone().unwrap_or_default();

			result.push(AnthropicMessage {
				role: "user".to_string(),
				content: serde_json::json!([{
					"type": "tool_result",
					"tool_use_id": tool_call_id,
					"content": msg.content
				}]),
			});
			continue;
		}

		// Regular messages
		result.push(AnthropicMessage {
			role: msg.role.clone(),
			content: serde_json::json!(msg.content),
		});
	}

	result
}
