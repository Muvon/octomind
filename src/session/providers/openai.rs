// OpenAI provider implementation

use anyhow::Result;
use reqwest::Client;
use serde::{Serialize, Deserialize};
use std::env;
use crate::config::Config;
use crate::session::Message;
use super::{AiProvider, ProviderResponse, ProviderExchange, TokenUsage};
use crate::log_debug;

/// OpenAI pricing constants (per 1M tokens in USD)
/// Source: https://openai.com/pricing (as of January 2025)
const PRICING: &[(&str, f64, f64)] = &[
	// Model, Input price per 1M tokens, Output price per 1M tokens
	("gpt-4o", 2.50, 10.00),
	("gpt-4o-mini", 0.15, 0.60),
	("gpt-4-turbo", 10.00, 30.00),
	("gpt-4", 30.00, 60.00),
	("gpt-3.5-turbo", 0.50, 1.50),
	("o1-preview", 15.00, 60.00),
	("o1-mini", 3.00, 12.00),
	("chatgpt-4o-latest", 2.50, 10.00), // Same as gpt-4o
];

/// Calculate cost for OpenAI models
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

/// OpenAI provider implementation
pub struct OpenAiProvider;

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
	pub content: serde_json::Value,  // Can be string or array with content parts
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
		// OpenAI models - common ones
		model.starts_with("gpt-") ||
		model.starts_with("o1-") ||
		model == "chatgpt-4o-latest" ||
		model.contains("gpt-4") ||
		model.contains("gpt-3.5")
	}

	fn get_api_key(&self, _config: &Config) -> Result<String> {
		// For now, only check environment variable
		// In the future, we could add openai-specific config section
		match env::var(OPENAI_API_KEY_ENV) {
			Ok(key) => Ok(key),
			Err(_) => Err(anyhow::anyhow!("OpenAI API key not found in environment variable {}", OPENAI_API_KEY_ENV))
		}
	}

	fn supports_caching(&self, model: &str) -> bool {
		// OpenAI doesn't currently support caching in the same way as Anthropic
		// But some models support better context handling
		model.contains("gpt-4") || model.contains("o1")
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

		// Convert messages to OpenAI format
		let openai_messages = convert_messages(messages);

		// Create the request body
		let mut request_body = serde_json::json!({
			"model": model,
			"messages": openai_messages,
			"temperature": temperature,
		});

		// Add tool definitions if MCP is enabled
		if config.mcp.enabled {
			let functions = crate::mcp::get_available_functions(config).await;
			if !functions.is_empty() {
				let tools = functions.iter().map(|f| {
					serde_json::json!({
						"type": "function",
						"function": {
							"name": f.name,
							"description": f.description,
							"parameters": f.parameters
						}
					})
				}).collect::<Vec<_>>();

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

		// Make the actual API request
		let response = client.post(OPENAI_API_URL)
			.header("Authorization", format!("Bearer {}", api_key))
			.header("Content-Type", "application/json")
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
			.ok_or_else(|| anyhow::anyhow!("Invalid response format from OpenAI: {}", response_text))?;

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
							function.get("arguments").and_then(|a| a.as_str())
						) {
							let params = if args.trim().is_empty() {
								serde_json::json!({})
							} else {
								match serde_json::from_str::<serde_json::Value>(args) {
									Ok(json_params) => json_params,
									Err(_) => serde_json::Value::String(args.to_string())
								}
							};

							let tool_id = tool_call.get("id").and_then(|i| i.as_str()).unwrap_or("");
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

		// Extract token usage
		let usage: Option<TokenUsage> = if let Some(usage_obj) = response_json.get("usage") {
			let prompt_tokens = usage_obj.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
			let completion_tokens = usage_obj.get("completion_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
			let total_tokens = usage_obj.get("total_tokens").and_then(|v| v.as_u64()).unwrap_or(0);

			// Calculate cost using our pricing constants
			let cost = calculate_cost(model, prompt_tokens, completion_tokens);

			let completion_tokens_details = usage_obj.get("completion_tokens_details").map(|v| v.clone());
			let prompt_tokens_details = usage_obj.get("prompt_tokens_details").map(|v| v.clone());

			// OpenAI doesn't have the same breakdown structure as OpenRouter
			let breakdown = None;

			Some(TokenUsage {
				prompt_tokens,
				completion_tokens,
				total_tokens,
				cost,
				completion_tokens_details,
				prompt_tokens_details,
				breakdown,
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
		if msg.role == "user" && msg.content.starts_with("<fnr>") && msg.content.ends_with("</fnr>") {
			let content = msg.content.trim_start_matches("<fnr>").trim_end_matches("</fnr>").trim();

			if let Ok(tool_responses) = serde_json::from_str::<Vec<serde_json::Value>>(content) {
				if !tool_responses.is_empty() && tool_responses[0].get("role").map_or(false, |r| r.as_str().unwrap_or("") == "tool") {
					for tool_response in tool_responses {
						let tool_call_id = tool_response.get("tool_call_id")
							.and_then(|id| id.as_str())
							.unwrap_or("");

						let name = tool_response.get("name")
							.and_then(|n| n.as_str())
							.unwrap_or("");

						let content = tool_response.get("content")
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

		// Regular messages (OpenAI doesn't have the same caching as Anthropic, so we ignore cached flag)
		result.push(OpenAiMessage {
			role: msg.role.clone(),
			content: serde_json::json!(msg.content),
			tool_call_id: None,
			name: None,
			tool_calls: None,
		});
	}

	result
}
