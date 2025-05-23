// OpenRouter API client for OctoDev

use anyhow::Result;
use reqwest::Client;
use serde::{Serialize, Deserialize};
use std::env;
use std::time::{SystemTime, UNIX_EPOCH};
use crate::config::Config;
use crate::log_debug;

// OpenRouter response with token usage
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TokenUsage {
	pub prompt_tokens: u64,
	pub completion_tokens: u64,
	pub total_tokens: u64,
	#[serde(default)]
	pub cost: Option<f64>,  // Cost in dollars as floating point number
	pub completion_tokens_details: Option<serde_json::Value>,
	pub prompt_tokens_details: Option<serde_json::Value>,
	pub breakdown: Option<std::collections::HashMap<String, serde_json::Value>>,  // Legacy field for cached tokens
}

// Store raw request/response for logging
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct OpenRouterExchange {
	pub request: serde_json::Value,
	pub response: serde_json::Value,
	pub timestamp: u64,
	pub usage: Option<TokenUsage>,
}

// Default OpenRouter API key environment variable name
const OPENROUTER_API_KEY_ENV: &str = "OPENROUTER_API_KEY";
const OPENROUTER_API_URL: &str = "https://openrouter.ai/api/v1/chat/completions";

// Get OpenRouter API key from config, falling back to environment
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

/// Message format for the OpenRouter API
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
	pub role: String,
	pub content: serde_json::Value,  // Can be string or object with cache_control
	#[serde(skip_serializing_if = "Option::is_none")]
	pub tool_call_id: Option<String>, // For tool messages: the ID of the tool call
	#[serde(skip_serializing_if = "Option::is_none")]
	pub name: Option<String>, // For tool messages: the name of the tool
	#[serde(skip_serializing_if = "Option::is_none")]
	pub tool_calls: Option<serde_json::Value>, // For assistant messages: array of tool calls
}

// Convert our session messages to OpenRouter format
pub fn convert_messages(messages: &[super::Message]) -> Vec<Message> {
	// Add debug tracking for cached messages
	let mut cached_count = 0;
	let mut result = Vec::new();

	let mut i = 0;
	while i < messages.len() {
		let msg = &messages[i];

		// Check if this is a tool response message (has <fnr> tags)
		if msg.role == "user" && msg.content.starts_with("<fnr>") && msg.content.ends_with("</fnr>") {
			// Extract content between the <fnr> tags
			let content = msg.content.trim_start_matches("<fnr>").trim_end_matches("</fnr>").trim();

			// Try to parse as tool responses array
			if let Ok(tool_responses) = serde_json::from_str::<Vec<serde_json::Value>>(content) {
				// Check if these are formatted as OpenAI tool responses with role: "tool"
				if !tool_responses.is_empty() && tool_responses[0].get("role").map_or(false, |r| r.as_str().unwrap_or("") == "tool") {
					// These are proper OpenAI-formatted tool responses
					// Add each tool response as a separate message
					for tool_response in tool_responses {
						// Extract the fields needed for OpenRouter format
						let tool_call_id = tool_response.get("tool_call_id")
							.and_then(|id| id.as_str())
							.unwrap_or("");

						let name = tool_response.get("name")
							.and_then(|n| n.as_str())
							.unwrap_or("");

						let content = tool_response.get("content")
							.and_then(|c| c.as_str())
							.unwrap_or("");

						// Create a properly formatted tool message
						// FIXED: tool_call_id and name are now top-level fields
						result.push(Message {
							role: "tool".to_string(),
							content: serde_json::json!(content), // Just the content string
							tool_call_id: Some(tool_call_id.to_string()),
							name: Some(name.to_string()),
							tool_calls: None,
						});
					}

					// Skip the standard message creation since we've already added the tool messages
					i += 1;
					continue;
				} else {
					// Legacy format - add a single tool message
					// FIXED: tool_call_id and name are now top-level fields
					result.push(Message {
						role: "tool".to_string(),
						content: serde_json::json!(content), // Just the content string
						tool_call_id: Some("legacy_tool_call".to_string()),
						name: Some("legacy_tool".to_string()),
						tool_calls: None,
					});
					i += 1;
					continue;
				}
			}
		} else if msg.role == "tool" {
			// This is a standard tool response message from our updated format
			// Get the tool call id and name from the message
			let tool_call_id = msg.tool_call_id.clone().unwrap_or_default();
			let name = msg.name.clone().unwrap_or_default();

			// Debug logging for tool message conversion
			let config = match crate::config::Config::load() {
				Ok(cfg) => cfg,
				Err(_) => crate::config::Config::default()
			};

			if config.openrouter.log_level.is_debug_enabled() {
				use colored::*;
				println!("{}", format!("Debug: Converting tool message - ID: {}, Name: {}, Content: {:.100}...",
					tool_call_id, name, msg.content).bright_magenta());
			}

			// Create a properly formatted tool message for OpenRouter/OpenAI format
			// FIXED: tool_call_id and name are now top-level fields, not nested in content
			result.push(Message {
				role: "tool".to_string(),
				content: serde_json::json!(msg.content), // Content is just the actual content string
				tool_call_id: Some(tool_call_id),
				name: Some(name),
				tool_calls: None,
			});
			i += 1;
			continue;
		} else if msg.role == "assistant" {
			// Handle assistant messages - check if we have stored tool_calls to include
			let mut assistant_message = if msg.cached {
				// Increment cached count for debugging
				cached_count += 1;

				// For a cached message, create a multipart message with cache control
				if msg.content.contains("\n") {
					// For multiline content, create a message with multiple parts
					let parts: Vec<&str> = msg.content.splitn(2, "\n").collect();
					if parts.len() > 1 {
						Message {
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
						// Fallback if splitting failed
						Message {
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
					// For single-line content, just wrap it in cache_control
					Message {
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
				// Regular message, no caching
				Message {
					role: msg.role.clone(),
					content: serde_json::json!(msg.content),
					tool_call_id: None,
					name: None,
					tool_calls: None,
				}
			};

			// If this assistant message has stored tool_calls, include them directly
			if let Some(ref tool_calls_data) = msg.tool_calls {
				assistant_message.tool_calls = Some(tool_calls_data.clone());

				// Debug logging
				let config = match crate::config::Config::load() {
					Ok(cfg) => cfg,
					Err(_) => crate::config::Config::default()
				};

				if config.openrouter.log_level.is_debug_enabled() {
					use colored::*;
					println!("{}", format!("Debug: Restored tool_calls for assistant message from stored data").bright_green());
				}
			}

			result.push(assistant_message);
			i += 1;
			continue;
		}

		// Handle cache breakpoints for non-assistant messages
		if msg.cached {
			// Increment cached count for debugging
			cached_count += 1;

			// For a cached message, create a multipart message with cache control
			if msg.content.contains("\n") {
				// For multiline content, create a message with multiple parts
				let parts: Vec<&str> = msg.content.splitn(2, "\n").collect();
				if parts.len() > 1 {
					result.push(Message {
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
					// Fallback if splitting failed
					result.push(Message {
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
				// For single-line content, just wrap it in cache_control
				result.push(Message {
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
			// Regular message, no caching
			result.push(Message {
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
		// Get config to check debug flag
		let config = match crate::config::Config::load() {
			Ok(cfg) => cfg,
			Err(_) => {
				// If we can't load config, assume debug is false
				crate::config::Config::default()
			}
		};

		if config.openrouter.log_level.is_debug_enabled() {
			use colored::*;
			println!("{}", format!("{} system/user messages marked for caching", cached_count).bright_magenta());
		}
	}

	result
}

// Send a chat completion request to OpenRouter
pub async fn chat_completion(
	messages: Vec<Message>,
	model: &str,
	temperature: f32,
	config: &Config,
) -> Result<(String, OpenRouterExchange, Option<Vec<crate::session::mcp::McpToolCall>>, Option<String>)> {
	// Get API key
	let api_key = get_api_key(config)?;

	// Create the request body
	// Always include usage tracking to ensure cost information is returned
	let mut request_body = serde_json::json!({
		"model": model,
		"messages": messages,
		"temperature": temperature,
		"top_p": 0.3,
		"repetition_penalty": 1.1,
		// "max_tokens": 200,
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
		let functions = crate::session::mcp::get_available_functions(config).await;
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
			if let Some(model) = request_body.get("model").and_then(|m| m.as_str()) {
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
			}

			request_body["tools"] = serde_json::json!(tools);
			// Always use "auto" to ensure more consistent tool usage
			// This is especially important for Claude which needs this directive
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

	// Handle error responses with improved error reporting
	if !status.is_success() {
		// Try to extract detailed error information
		let mut error_details = Vec::new();

		// Add HTTP status
		error_details.push(format!("HTTP {}", status));

		// Try to extract OpenRouter error message
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
		} else if let Some(msg) = response_json.get("error").and_then(|e| e.as_str()) {
			error_details.push(format!("Error: {}", msg));
		}

		// If no specific error details found, include the raw response for debugging
		if error_details.len() == 1 { // Only HTTP status was added
			error_details.push(format!("Raw response: {}", response_text));
		}

		let full_error = error_details.join(" | ");
		return Err(anyhow::anyhow!("OpenRouter API error: {}", full_error));
	}

	// CRITICAL FIX: Check for errors in response body even with HTTP 200
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

		if config.openrouter.log_level.is_debug_enabled() {
			error_details.push(format!("Raw response: {}", response_text));
		}

		let full_error = error_details.join(" | ");
		return Err(anyhow::anyhow!("OpenRouter API error: {}", full_error));
	}

	// Extract content from response
	let message = response_json
		.get("choices")
		.and_then(|choices| choices.get(0))
		.and_then(|choice| choice.get("message"))
		.ok_or_else(|| anyhow::anyhow!("Invalid response format from OpenRouter: {}", response_text))?;

	// Extract finish_reason from the first choice
	let finish_reason = response_json
		.get("choices")
		.and_then(|choices| choices.get(0))
		.and_then(|choice| choice.get("finish_reason"))
		.and_then(|fr| fr.as_str())
		.map(|s| s.to_string());

		if let Some(ref reason) = finish_reason {
			log_debug!("Finish reason: {}", reason);
		} else {
			log_debug!("No finish_reason in response");
		}

	// Check if the response contains tool calls
	let mut content = String::new();

	// First check for content
	if let Some(text) = message.get("content").and_then(|c| c.as_str()) {
		content = text.to_string();
	}

	// Check if the response contains tool calls
	if let Some(tool_calls) = message.get("tool_calls") {
		if tool_calls.is_array() && !tool_calls.as_array().unwrap().is_empty() {
			// Extract the tool calls directly and ensure they have valid IDs
			let mut extracted_tool_calls = Vec::new();

			// Iterate through the tool calls array
			for tool_call in tool_calls.as_array().unwrap() {
				// Extract the function information
				if let Some(function) = tool_call.get("function") {
					if let (Some(name), Some(args)) = (function.get("name").and_then(|n| n.as_str()),
						function.get("arguments").and_then(|a| a.as_str())) {
						// Parse the arguments as JSON
						let params = if args.trim().is_empty() {
							// Empty arguments should be an empty object, not an empty string
							serde_json::json!({})
						} else {
							match serde_json::from_str::<serde_json::Value>(args) {
								Ok(json_params) => json_params,
								Err(_) => {
									// Fallback: use arguments as a raw string if parsing fails
									serde_json::Value::String(args.to_string())
								}
							}
						};

						// Create an MCP tool call
						let tool_id = tool_call.get("id").and_then(|i| i.as_str()).unwrap_or("");
						let mcp_call = crate::session::mcp::McpToolCall {
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
					// Handle the direct tool call format (used by some models)
					let params = if let Some(params_obj) = tool_call.get("parameters") {
						// Ensure even empty parameters are formatted as objects, not strings
						if params_obj.is_string() && params_obj.as_str().unwrap_or("").is_empty() {
							serde_json::json!({})
						} else {
							params_obj.clone()
						}
					} else {
						serde_json::json!({})
					};

					let tool_id = tool_call.get("id").and_then(|i| i.as_str()).unwrap_or("");
					let mcp_call = crate::session::mcp::McpToolCall {
						tool_name: name.to_string(),
						parameters: params,
						tool_id: tool_id.to_string(),
					};

					extracted_tool_calls.push(mcp_call);
				}
			}

			// Ensure all tool calls have valid IDs
			crate::session::mcp::ensure_tool_call_ids(&mut extracted_tool_calls);

			// Extract token usage from the response
			let usage: Option<TokenUsage> = if let Some(usage_obj) = response_json.get("usage") {
				let prompt_tokens = usage_obj.get("prompt_tokens")
					.and_then(|v| v.as_u64())
					.unwrap_or(0);
				let completion_tokens = usage_obj.get("completion_tokens")
					.and_then(|v| v.as_u64())
					.unwrap_or(0);
				let total_tokens = usage_obj.get("total_tokens")
					.and_then(|v| v.as_u64())
					.unwrap_or(0);
				let cost = usage_obj.get("cost")
					.and_then(|v| v.as_f64());
				let completion_tokens_details = usage_obj.get("completion_tokens_details")
					.map(|v| v.clone());
				let prompt_tokens_details = usage_obj.get("prompt_tokens_details")
					.map(|v| v.clone());

				// Extract breakdown info (cached tokens, etc)
				let breakdown = usage_obj.get("breakdown")
					.and_then(|b| {
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

				// No token usage logging here - moved to after response handling in chat.rs

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

			// Create the exchange record for logging
			let exchange = OpenRouterExchange {
				request: request_body,
				response: response_json.clone(),
				timestamp: SystemTime::now()
					.duration_since(UNIX_EPOCH)
					.unwrap_or_default()
					.as_secs(),
				usage,
			};

			// Directly return the extracted tool calls
			return Ok((content, exchange, Some(extracted_tool_calls), finish_reason));

		} else if content.is_empty() {
			// If content is empty but we have a valid response structure, treat it as a valid empty response
			// This can happen when the model responds to tool results with no additional text
			if config.openrouter.log_level.is_debug_enabled() {
				use colored::*;
				println!("{}", "Debug: Received valid response with empty content and no tool calls".bright_yellow());
			}
			// Don't return an error - treat empty content as valid when it's a proper response structure
		}
	} else {
		// No tool_calls field at all
		if content.is_empty() {
			// If content is empty and no tool_calls field, this could be a valid empty response
			// This happens when the model responds to tool results with no additional text
			if config.openrouter.log_level.is_debug_enabled() {
				use colored::*;
				println!("{}", "Debug: Received valid response with empty content and no tool_calls field".bright_yellow());
			}
			// Don't return an error - treat as valid empty response
		}
	}

	// Extract token usage from the response
	let usage: Option<TokenUsage> = if let Some(usage_obj) = response_json.get("usage") {
		let prompt_tokens = usage_obj.get("prompt_tokens")
			.and_then(|v| v.as_u64())
			.unwrap_or(0);
		let completion_tokens = usage_obj.get("completion_tokens")
			.and_then(|v| v.as_u64())
			.unwrap_or(0);
		let total_tokens = usage_obj.get("total_tokens")
			.and_then(|v| v.as_u64())
			.unwrap_or(0);
		let cost = usage_obj.get("cost")
			.and_then(|v| v.as_f64());
		let completion_tokens_details = usage_obj.get("completion_tokens_details")
			.map(|v| v.clone());
		let prompt_tokens_details = usage_obj.get("prompt_tokens_details")
			.map(|v| v.clone());

		// Extract breakdown info (cached tokens, etc)
		let breakdown = usage_obj.get("breakdown")
			.and_then(|b| {
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

		// No token usage logging here - moved to after response handling in chat.rs

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

	// Create exchange record for logging
	let exchange = OpenRouterExchange {
		request: request_body,
		response: response_json.clone(),
		timestamp: SystemTime::now()
			.duration_since(UNIX_EPOCH)
			.unwrap_or_default()
			.as_secs(),
		usage,
	};

	Ok((content, exchange, None, finish_reason))
}
