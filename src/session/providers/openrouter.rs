// OpenRouter provider implementation

use anyhow::Result;
use reqwest::Client;
use serde::{Serialize, Deserialize};
use std::env;
use crate::config::Config;
use crate::session::Message;
use super::{AiProvider, ProviderResponse, ProviderExchange, TokenUsage};
use crate::log_debug;

/// OpenRouter provider implementation
pub struct OpenRouterProvider;

impl OpenRouterProvider {
	pub fn new() -> Self {
		Self
	}
}

// Constants
const OPENROUTER_API_KEY_ENV: &str = "OPENROUTER_API_KEY";
const OPENROUTER_API_URL: &str = "https://openrouter.ai/api/v1/chat/completions";

/// Message format for the OpenRouter API
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenRouterMessage {
	pub role: String,
	pub content: serde_json::Value,  // Can be string or object with cache_control
	#[serde(skip_serializing_if = "Option::is_none")]
	pub tool_call_id: Option<String>, // For tool messages: the ID of the tool call
	#[serde(skip_serializing_if = "Option::is_none")]
	pub name: Option<String>, // For tool messages: the name of the tool
	#[serde(skip_serializing_if = "Option::is_none")]
	pub tool_calls: Option<serde_json::Value>, // For assistant messages: array of tool calls
}

#[async_trait::async_trait]
impl AiProvider for OpenRouterProvider {
	fn name(&self) -> &str {
		"openrouter"
	}

	fn supports_model(&self, model: &str) -> bool {
		// OpenRouter supports models in format "provider/model"
		// This is a broad check - in practice OpenRouter supports many models
		model.contains('/') ||
		model.starts_with("anthropic") ||
		model.starts_with("openai") ||
		model.starts_with("google") ||
		model.starts_with("meta-llama") ||
		model.starts_with("mistralai")
	}

	fn get_api_key(&self, config: &Config) -> Result<String> {
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

	fn supports_caching(&self, model: &str) -> bool {
		// OpenRouter supports caching for Claude models
		model.contains("claude") || model.contains("anthropic")
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

		// Convert messages to OpenRouter format
		let openrouter_messages = convert_messages(messages);

		// Create the request body
		let mut request_body = serde_json::json!({
			"model": model,
			"messages": openrouter_messages,
			"temperature": temperature,
			"top_p": 0.3,
			"repetition_penalty": 1.1,
			"usage": {
				"include": true  // Always enable usage tracking for all requests
			},
			"provider": {
				"order": [
					"Anthropic",
					"OpenAI",
					"Amazon Bedrock",
					"Azure",
					"Cloudflare",
					"Google Vertex",
					"xAI",
				],
				"allow_fallbacks": true,
			},
		});

		// Add tool definitions if MCP is enabled
		if config.mcp.enabled {
			let functions = crate::mcp::get_available_functions(config).await;
			if !functions.is_empty() {
				let mut tools = functions.iter().map(|f| {
					serde_json::json!({
						"type": "function",
						"function": {
							"name": f.name,
							"description": f.description,
							"parameters": f.parameters
						}
					})
				}).collect::<Vec<_>>();

				// Add web search tool if using Claude 3.7 Sonnet
				if model.contains("claude-sonnet-4") {
					tools.push(serde_json::json!({
						"type": "web_search_20250305",
						"name": "web_search"
					}));
					tools.push(serde_json::json!({
						"type": "text_editor_20250124",
						"name": "text_editor"
					}));
				}

				request_body["tools"] = serde_json::json!(tools);
				request_body["tool_choice"] = serde_json::json!("auto");
			}
		}

		// Create HTTP client
		let client = Client::new();

		// Make the actual API request
		let response = client.post(OPENROUTER_API_URL)
			.header("Authorization", format!("Bearer {}", api_key))
			.header("Content-Type", "application/json")
			.header("HTTP-Referer", "https://github.com/muvon/octodev")
			.header("X-Title", "Octodev")
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
			return Err(anyhow::anyhow!("OpenRouter API error: {}", full_error));
		}

		// Check for errors in response body even with HTTP 200
		if let Some(error_obj) = response_json.get("error") {
			let mut error_details = Vec::new();
			error_details.push("HTTP 200 but error in response".to_string());

			if let Some(msg) = error_obj.get("message").and_then(|m| m.as_str()) {
				error_details.push(format!("Message: {}", msg));
			}

			let full_error = error_details.join(" | ");
			return Err(anyhow::anyhow!("OpenRouter API error: {}", full_error));
		}

		// Extract content and tool calls from response
		let message = response_json
			.get("choices")
			.and_then(|choices| choices.get(0))
			.and_then(|choice| choice.get("message"))
			.ok_or_else(|| anyhow::anyhow!("Invalid response format from OpenRouter: {}", response_text))?;

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
					} else if let (Some(_id), Some(name)) = (
						tool_call.get("id").and_then(|i| i.as_str()),
						tool_call.get("name").and_then(|n| n.as_str())
					) {
						let params = if let Some(params_obj) = tool_call.get("parameters") {
							if params_obj.is_string() && params_obj.as_str().unwrap_or("").is_empty() {
								serde_json::json!({})
							} else {
								params_obj.clone()
							}
						} else {
							serde_json::json!({})
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
			let cost = usage_obj.get("cost").and_then(|v| v.as_f64());
			let completion_tokens_details = usage_obj.get("completion_tokens_details").map(|v| v.clone());
			let prompt_tokens_details = usage_obj.get("prompt_tokens_details").map(|v| v.clone());

			let breakdown = usage_obj.get("breakdown").and_then(|b| {
				if let Some(obj) = b.as_object() {
					let mut map = std::collections::HashMap::new();
					for (k, v) in obj {
						map.insert(k.clone(), v.clone());
					}
					Some(map)
				} else {
					None
				}
			});

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

// Convert our session messages to OpenRouter format
fn convert_messages(messages: &[Message]) -> Vec<OpenRouterMessage> {
	let mut cached_count = 0;
	let mut result = Vec::new();

	let mut i = 0;
	while i < messages.len() {
		let msg = &messages[i];

		// Check if this is a tool response message (has <fnr> tags)
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

						result.push(OpenRouterMessage {
							role: "tool".to_string(),
							content: serde_json::json!(content),
							tool_call_id: Some(tool_call_id.to_string()),
							name: Some(name.to_string()),
							tool_calls: None,
						});
					}

					i += 1;
					continue;
				} else {
					result.push(OpenRouterMessage {
						role: "tool".to_string(),
						content: serde_json::json!(content),
						tool_call_id: Some("legacy_tool_call".to_string()),
						name: Some("legacy_tool".to_string()),
						tool_calls: None,
					});
					i += 1;
					continue;
				}
			}
		} else if msg.role == "tool" {
			let tool_call_id = msg.tool_call_id.clone().unwrap_or_default();
			let name = msg.name.clone().unwrap_or_default();

			result.push(OpenRouterMessage {
				role: "tool".to_string(),
				content: serde_json::json!(msg.content),
				tool_call_id: Some(tool_call_id),
				name: Some(name),
				tool_calls: None,
			});
			i += 1;
			continue;
		} else if msg.role == "assistant" {
			let mut assistant_message = if msg.cached {
				cached_count += 1;

				if msg.content.contains("\n") {
					let parts: Vec<&str> = msg.content.splitn(2, "\n").collect();
					if parts.len() > 1 {
						OpenRouterMessage {
							role: msg.role.clone(),
							content: serde_json::json!([
								{
									"type": "text",
									"text": parts[0],
								},
								{
									"type": "text",
									"text": parts[1],
									"cache_control": {
										"type": "ephemeral"
									}
								}
							]),
							tool_call_id: None,
							name: None,
							tool_calls: None,
						}
					} else {
						OpenRouterMessage {
							role: msg.role.clone(),
							content: serde_json::json!([
								{
									"type": "text",
									"text": msg.content,
									"cache_control": {
									"type": "ephemeral"
								}
							}
							]),
							tool_call_id: None,
							name: None,
							tool_calls: None,
						}
					}
				} else {
					OpenRouterMessage {
						role: msg.role.clone(),
						content: serde_json::json!([
							{
								"type": "text",
								"text": msg.content,
								"cache_control": {
								"type": "ephemeral"
							}
						}
						]),
						tool_call_id: None,
						name: None,
						tool_calls: None,
					}
				}
			} else {
				OpenRouterMessage {
					role: msg.role.clone(),
					content: serde_json::json!(msg.content),
					tool_call_id: None,
					name: None,
					tool_calls: None,
				}
			};

			if let Some(ref tool_calls_data) = msg.tool_calls {
				assistant_message.tool_calls = Some(tool_calls_data.clone());
			}

			result.push(assistant_message);
			i += 1;
			continue;
		}

		// Handle cache breakpoints for non-assistant messages
		if msg.cached {
			cached_count += 1;

			if msg.content.contains("\n") {
				let parts: Vec<&str> = msg.content.splitn(2, "\n").collect();
				if parts.len() > 1 {
					result.push(OpenRouterMessage {
						role: msg.role.clone(),
						content: serde_json::json!([
							{
								"type": "text",
								"text": parts[0],
							},
							{
								"type": "text",
								"text": parts[1],
								"cache_control": {
									"type": "ephemeral"
								}
							}
						]),
						tool_call_id: None,
						name: None,
						tool_calls: None,
					});
				} else {
					result.push(OpenRouterMessage {
						role: msg.role.clone(),
						content: serde_json::json!([
							{
								"type": "text",
								"text": msg.content,
								"cache_control": {
								"type": "ephemeral"
							}
						}
						]),
						tool_call_id: None,
						name: None,
						tool_calls: None,
					});
				}
			} else {
				result.push(OpenRouterMessage {
					role: msg.role.clone(),
					content: serde_json::json!([
						{
							"type": "text",
							"text": msg.content,
							"cache_control": {
							"type": "ephemeral"
						}
					}
					]),
					tool_call_id: None,
					name: None,
					tool_calls: None,
				});
			}
		} else {
			result.push(OpenRouterMessage {
				role: msg.role.clone(),
				content: serde_json::json!(msg.content),
				tool_call_id: None,
				name: None,
				tool_calls: None,
			});
		}

		i += 1;
	}

	// Log debug info for cached messages only if debug mode is enabled
	if cached_count > 0 {
		log_debug!("{} system/user messages marked for caching", cached_count);
	}

	result
}
