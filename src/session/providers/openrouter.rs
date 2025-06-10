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

// OpenRouter provider implementation

use super::{AiProvider, ProviderExchange, ProviderResponse, TokenUsage};
use crate::config::Config;
use crate::log_debug;
use crate::session::Message;
use anyhow::Result;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::env;
use std::sync::OnceLock;

// Global HTTP client with optimized settings - PERFORMANCE BEAST! 🔥
static HTTP_CLIENT: OnceLock<Client> = OnceLock::new();

fn get_optimized_client() -> &'static Client {
	HTTP_CLIENT.get_or_init(|| {
		Client::builder()
			.pool_max_idle_per_host(10) // Keep connections alive
			.pool_idle_timeout(std::time::Duration::from_secs(90)) // Connection reuse
			.timeout(std::time::Duration::from_secs(300)) // 5 min timeout
			.build()
			.expect("Failed to create optimized HTTP client")
	})
}

/// OpenRouter provider implementation
pub struct OpenRouterProvider;

impl Default for OpenRouterProvider {
	fn default() -> Self {
		Self::new()
	}
}

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
	pub content: serde_json::Value, // Can be string or object with cache_control
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
		model.contains('/')
			|| model.starts_with("anthropic")
			|| model.starts_with("openai")
			|| model.starts_with("google")
			|| model.starts_with("meta-llama")
			|| model.starts_with("mistralai")
	}

	fn get_api_key(&self, _config: &Config) -> Result<String> {
		// API keys now only from environment variables for security
		match env::var(OPENROUTER_API_KEY_ENV) {
			Ok(key) => Ok(key),
			Err(_) => Err(anyhow::anyhow!(
				"OpenRouter API key not found in environment variable: {}",
				OPENROUTER_API_KEY_ENV
			)),
		}
	}

	fn supports_caching(&self, model: &str) -> bool {
		// OpenRouter supports caching for Claude models and Gemini models
		// This should match the logic in CacheManager::validate_cache_support
		model.contains("claude") || model.contains("gemini")
	}

	fn supports_vision(&self, model: &str) -> bool {
		// OpenRouter supports vision through various models
		model.contains("gpt-4o")
			|| model.contains("gpt-4.1")
			|| model.contains("gpt-4-vision")
			|| model.contains("gpt-4-turbo")
			|| model.contains("claude-3")
			|| model.contains("claude-4")
			|| model.contains("gemini")
	}

	fn get_max_input_tokens(&self, model: &str) -> usize {
		// OpenRouter model input limits depend on underlying provider
		// Claude models through OpenRouter: 200K total context
		if model.contains("claude") {
			return 200_000 - 32_768; // Reserve 32K for output = ~167K input max
		}
		// GPT models through OpenRouter: varies by model
		if model.contains("gpt-4o") {
			return 128_000 - 4_096;
		}
		if model.contains("gpt-4") {
			return 128_000 - 4_096;
		}
		if model.contains("gpt-3.5") {
			return 16_384 - 2_048;
		}
		// Gemini models through OpenRouter
		if model.contains("gemini-2.5") {
			return 2_000_000 - 8_192;
		}
		if model.contains("gemini-2.0") || model.contains("gemini-1.5") {
			return 1_000_000 - 8_192;
		}
		if model.contains("gemini") {
			return 32_768 - 2_048;
		}
		// Llama models through OpenRouter: typically 128K
		if model.contains("llama") {
			return 128_000 - 4_096;
		}
		// Default conservative limit for unknown models
		32_768 - 2_048
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
		let openrouter_messages = convert_messages(messages, config);

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

		// Add tool definitions if MCP has any servers configured
		if !config.mcp.servers.is_empty() {
			let functions = crate::mcp::get_available_functions(config).await;
			if !functions.is_empty() {
				// CRITICAL FIX: Ensure tool definitions are ALWAYS in the same order
				// Sort functions by name to guarantee consistent ordering across API calls
				let mut sorted_functions = functions;
				sorted_functions.sort_by(|a, b| a.name.cmp(&b.name));

				let mut tools = sorted_functions
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

				// REMOVED: Extra OpenRouter-specific tools that break cache consistency
				// These tools (text_editor_20250124, web_search_20250305) are not available
				// in our MCP setup and cause different tool arrays between Anthropic and OpenRouter,
				// breaking cache effectiveness. Only use tools from MCP configuration.

				// CRITICAL FIX: Cache control should be handled consistently
				// Add cache control to the LAST tool definition ONLY if the model supports caching
				// and we actually want to cache tool definitions (check session state)
				if self.supports_caching(model) && !tools.is_empty() {
					// Check if any system message is cached - if so, we should cache tool definitions too
					let system_cached = messages
						.iter()
						.any(|msg| msg.role == "system" && msg.cached);

					if system_cached {
						if let Some(last_tool) = tools.last_mut() {
							last_tool["cache_control"] = serde_json::json!({
								"type": "ephemeral",
								"ttl": "1h"
							});
						}
					}
				}

				request_body["tools"] = serde_json::json!(tools);
				request_body["tool_choice"] = serde_json::json!("auto");
			}
		}

		// Create HTTP client - USE THE OPTIMIZED GLOBAL POOL! 🚀
		let client = get_optimized_client();

		// Track API request time
		let api_start = std::time::Instant::now();

		// Make the actual API request
		let response = client
			.post(OPENROUTER_API_URL)
			.header("Authorization", format!("Bearer {}", api_key))
			.header("Content-Type", "application/json")
			.header("HTTP-Referer", "https://github.com/muvon/octomind")
			.header("X-Title", "Octomind")
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

		// Enhanced error handling with detailed logging
		if !status.is_success() {
			let mut error_details = Vec::new();
			error_details.push(format!("HTTP {}", status));
			error_details.push(format!("Model: {}", model));

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

				// Extract metadata for better debugging
				if let Some(metadata) = error_obj.get("metadata") {
					if let Some(provider_name) =
						metadata.get("provider_name").and_then(|p| p.as_str())
					{
						error_details.push(format!("Provider: {}", provider_name));
					}
					if let Some(provider_error) =
						metadata.get("provider_error").and_then(|p| p.as_str())
					{
						error_details.push(format!("Provider error: {}", provider_error));
					}
				}
			}

			// Always include raw response for debugging when there's an HTTP error
			error_details.push(format!("Raw response: {}", response_text));

			let full_error = error_details.join(" | ");

			// Log detailed error information using the log_error! macro
			crate::log_error!("OpenRouter API HTTP Error Details:");
			crate::log_error!("  Status: {}", status);
			crate::log_error!("  Model: {}", model);
			crate::log_error!("  Temperature: {}", temperature);
			crate::log_error!(
				"  Request size: {} chars",
				serde_json::to_string(&request_body).map_or(0, |s| s.len())
			);
			crate::log_error!("  Response: {}", response_text);

			// If in debug mode, also log the full request
			if config.get_log_level().is_debug_enabled() {
				if let Ok(request_str) = serde_json::to_string_pretty(&request_body) {
					crate::log_error!("  Request body: {}", request_str);
				}
			}

			return Err(anyhow::anyhow!("OpenRouter API error: {}", full_error));
		}

		// Enhanced error handling for HTTP 200 responses with errors
		if let Some(error_obj) = response_json.get("error") {
			let mut error_details = Vec::new();
			error_details.push("HTTP 200 but error in response".to_string());
			error_details.push(format!("Model: {}", model));

			let error_message = error_obj
				.get("message")
				.and_then(|m| m.as_str())
				.unwrap_or("Unknown error");
			error_details.push(format!("Message: {}", error_message));

			// Extract provider information for better debugging
			if let Some(metadata) = error_obj.get("metadata") {
				if let Some(provider_name) = metadata.get("provider_name").and_then(|p| p.as_str())
				{
					error_details.push(format!("Provider: {}", provider_name));
				}
				if let Some(provider_error) =
					metadata.get("provider_error").and_then(|p| p.as_str())
				{
					error_details.push(format!("Provider error: {}", provider_error));
				}
			}

			let full_error = error_details.join(" | ");

			// Log comprehensive error information using the log_error! macro
			crate::log_error!("OpenRouter API Response Error Details:");
			crate::log_error!("  Model: {}", model);
			crate::log_error!("  Temperature: {}", temperature);
			crate::log_error!("  Error message: {}", error_message);
			crate::log_error!(
				"  Request size: {} chars",
				serde_json::to_string(&request_body).map_or(0, |s| s.len())
			);
			crate::log_error!("  Full response: {}", response_text);

			// If in debug mode, log the full request and parsed error object
			if config.get_log_level().is_debug_enabled() {
				if let Ok(request_str) = serde_json::to_string_pretty(&request_body) {
					crate::log_error!("  Request body: {}", request_str);
				}
				if let Ok(error_str) = serde_json::to_string_pretty(&error_obj) {
					crate::log_error!("  Error object: {}", error_str);
				}
			}

			return Err(anyhow::anyhow!("OpenRouter API error: {}", full_error));
		}

		// Extract content and tool calls from response
		let message = response_json
			.get("choices")
			.and_then(|choices| choices.get(0))
			.and_then(|choice| choice.get("message"))
			.ok_or_else(|| {
				anyhow::anyhow!("Invalid response format from OpenRouter: {}", response_text)
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
					} else if let (Some(_id), Some(name)) = (
						tool_call.get("id").and_then(|i| i.as_str()),
						tool_call.get("name").and_then(|n| n.as_str()),
					) {
						let params = if let Some(params_obj) = tool_call.get("parameters") {
							if params_obj.is_string()
								&& params_obj.as_str().unwrap_or("").is_empty()
							{
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
			let cost = usage_obj.get("cost").and_then(|v| v.as_f64());

			// Extract cached tokens from OpenRouter's detailed response
			let cached_tokens = usage_obj
				.get("prompt_tokens_details")
				.and_then(|details| details.get("cached_tokens"))
				.and_then(|v| v.as_u64())
				.unwrap_or(0);

			Some(TokenUsage {
				prompt_tokens,
				output_tokens: completion_tokens,
				total_tokens,
				cached_tokens, // OpenRouter provides cached token information
				cost,
				request_time_ms: Some(api_time_ms),
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
fn convert_messages(messages: &[Message], config: &Config) -> Vec<OpenRouterMessage> {
	let mut cached_count = 0;
	let mut result = Vec::new();

	// Cache markers should already be properly set by session logic
	// We just need to respect them when converting to API format

	for msg in messages {
		// Handle all message types with simplified structure
		match msg.role.as_str() {
			"system" => {
				// System messages with proper OpenRouter format
				let content = if msg.cached {
					cached_count += 1;
					let mut text_content = serde_json::json!({
						"type": "text",
						"text": msg.content
					});
					let ttl = if config.use_long_system_cache {
						"1h"
					} else {
						"5m"
					};
					text_content["cache_control"] = serde_json::json!({
						"type": "ephemeral",
						"ttl": ttl
					});
					serde_json::json!([text_content])
				} else {
					serde_json::json!(msg.content)
				};

				result.push(OpenRouterMessage {
					role: msg.role.clone(),
					content,
					tool_call_id: None,
					name: None,
					tool_calls: None,
				});
			}
			"tool" => {
				// Tool messages with proper OpenRouter format
				let tool_call_id = msg.tool_call_id.clone().unwrap_or_default();
				let name = msg.name.clone().unwrap_or_default();

				let content = if msg.cached {
					cached_count += 1;
					let mut text_content = serde_json::json!({
						"type": "text",
						"text": msg.content
					});
					text_content["cache_control"] = serde_json::json!({
						"type": "ephemeral"
					});
					serde_json::json!([text_content])
				} else {
					serde_json::json!(msg.content)
				};

				result.push(OpenRouterMessage {
					role: "tool".to_string(),
					content,
					tool_call_id: Some(tool_call_id),
					name: Some(name),
					tool_calls: None,
				});
			}
			"assistant" => {
				// Assistant messages with proper structure
				let content = if msg.cached {
					cached_count += 1;
					let mut text_content = serde_json::json!({
						"type": "text",
						"text": msg.content
					});
					text_content["cache_control"] = serde_json::json!({
						"type": "ephemeral"
					});
					serde_json::json!([text_content])
				} else {
					serde_json::json!(msg.content)
				};

				let mut assistant_msg = OpenRouterMessage {
					role: msg.role.clone(),
					content,
					tool_call_id: None,
					name: None,
					tool_calls: None,
				};

				// Preserve tool calls if they exist
				if let Some(ref tool_calls_data) = msg.tool_calls {
					assistant_msg.tool_calls = Some(tool_calls_data.clone());
				}

				result.push(assistant_msg);
			}
			"user" => {
				// Handle legacy <fnr> format for backwards compatibility
				if msg.content.starts_with("<fnr>") && msg.content.ends_with("</fnr>") {
					let content = msg
						.content
						.trim_start_matches("<fnr>")
						.trim_end_matches("</fnr>")
						.trim();

					if let Ok(tool_responses) =
						serde_json::from_str::<Vec<serde_json::Value>>(content)
					{
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

								result.push(OpenRouterMessage {
									role: "tool".to_string(),
									content: serde_json::json!(content),
									tool_call_id: Some(tool_call_id.to_string()),
									name: Some(name.to_string()),
									tool_calls: None,
								});
							}
							continue;
						} else {
							result.push(OpenRouterMessage {
								role: "tool".to_string(),
								content: serde_json::json!(content),
								tool_call_id: Some("legacy_tool_call".to_string()),
								name: Some("legacy_tool".to_string()),
								tool_calls: None,
							});
							continue;
						}
					}
				}

				// Handle user messages with images - use OpenAI/OpenRouter multimodal format
				if msg.images.is_some() {
					let mut content_parts = Vec::new();

					// Add text content if not empty
					if !msg.content.trim().is_empty() {
						let mut text_content = serde_json::json!({
							"type": "text",
							"text": msg.content
						});

						// Add cache_control if needed
						if msg.cached {
							cached_count += 1;
							text_content["cache_control"] = serde_json::json!({
								"type": "ephemeral"
							});
						}

						content_parts.push(text_content);
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

					result.push(OpenRouterMessage {
						role: msg.role.clone(),
						content: serde_json::json!(content_parts),
						tool_call_id: None,
						name: None,
						tool_calls: None,
					});
				} else {
					// Regular user messages with proper structure
					let content = if msg.cached {
						cached_count += 1;
						let mut text_content = serde_json::json!({
							"type": "text",
							"text": msg.content
						});
						text_content["cache_control"] = serde_json::json!({
							"type": "ephemeral"
						});
						serde_json::json!([text_content])
					} else {
						serde_json::json!(msg.content)
					};

					result.push(OpenRouterMessage {
						role: msg.role.clone(),
						content,
						tool_call_id: None,
						name: None,
						tool_calls: None,
					});
				}
			}
			_ => {
				// All other message types with proper structure
				let content = if msg.cached {
					cached_count += 1;
					let mut text_content = serde_json::json!({
						"type": "text",
						"text": msg.content
					});
					text_content["cache_control"] = serde_json::json!({
						"type": "ephemeral"
					});
					serde_json::json!([text_content])
				} else {
					serde_json::json!(msg.content)
				};

				result.push(OpenRouterMessage {
					role: msg.role.clone(),
					content,
					tool_call_id: None,
					name: None,
					tool_calls: None,
				});
			}
		}
	}

	// Log debug info for cached messages only if debug mode is enabled
	if cached_count > 0 {
		log_debug!("{} messages marked for caching", cached_count);
	}

	result
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_supports_vision() {
		let provider = OpenRouterProvider::new();

		// OpenAI models through OpenRouter
		assert!(provider.supports_vision("openai/gpt-4o"));
		assert!(provider.supports_vision("openai/gpt-4o-mini"));
		assert!(provider.supports_vision("openai/gpt-4-turbo"));
		assert!(provider.supports_vision("openai/gpt-4-vision-preview"));

		// Anthropic models through OpenRouter
		assert!(provider.supports_vision("anthropic/claude-3-opus"));
		assert!(provider.supports_vision("anthropic/claude-3-sonnet"));
		assert!(provider.supports_vision("anthropic/claude-3-haiku"));
		assert!(provider.supports_vision("anthropic/claude-3.5-sonnet"));
		assert!(provider.supports_vision("anthropic/claude-3.5-haiku"));
		assert!(provider.supports_vision("anthropic/claude-3.7-sonnet"));
		assert!(provider.supports_vision("anthropic/claude-4-opus"));

		// Google models through OpenRouter
		assert!(provider.supports_vision("google/gemini-1.5-pro"));
		assert!(provider.supports_vision("google/gemini-1.5-flash"));

		// Meta models through OpenRouter
		assert!(provider.supports_vision("meta-llama/llama-3.2-vision"));

		// Mistral models through OpenRouter
		assert!(provider.supports_vision("mistralai/pixtral-12b"));

		// Models that should NOT support vision
		assert!(!provider.supports_vision("openai/gpt-3.5-turbo"));
		assert!(!provider.supports_vision("anthropic/claude-2"));
		assert!(!provider.supports_vision("openai/text-davinci-003"));
		assert!(!provider.supports_vision("cohere/command-r"));
	}

	#[test]
	fn test_supports_caching() {
		let provider = OpenRouterProvider::new();

		// Models that should support caching
		assert!(provider.supports_caching("anthropic/claude-3.5-sonnet"));
		assert!(provider.supports_caching("claude-3-opus"));
		assert!(provider.supports_caching("google/gemini-1.5-pro"));
		assert!(provider.supports_caching("gemini-1.5-flash"));

		// Models that should NOT support caching
		assert!(!provider.supports_caching("openai/gpt-4o"));
		assert!(!provider.supports_caching("openai/gpt-3.5-turbo"));
		assert!(!provider.supports_caching("cohere/command-r"));
	}
}
