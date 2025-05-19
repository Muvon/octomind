// OpenRouter API client for OctoDev

use anyhow::Result;
use reqwest::Client;
use serde::{Serialize, Deserialize};
use std::env;
use std::time::{SystemTime, UNIX_EPOCH};
use crate::config::Config;

// Store raw request/response for logging
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct OpenRouterExchange {
	pub request: serde_json::Value,
	pub response: serde_json::Value,
	pub timestamp: u64,
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
	pub content: String,
}

// Convert our session messages to OpenRouter format
pub fn convert_messages(messages: &[super::Message]) -> Vec<Message> {
	messages.iter().map(|msg| {
		Message {
			role: msg.role.clone(),
			content: msg.content.clone(),
		}
	}).collect()
}

// Send a chat completion request to OpenRouter
pub async fn chat_completion(
	messages: Vec<Message>,
	model: &str,
	temperature: f32,
	config: &Config,
) -> Result<(String, OpenRouterExchange)> {
	// Get API key
	let api_key = get_api_key(config)?;

	// Create the request body
	let mut request_body = serde_json::json!({
		"model": model,
		"messages": messages,
		"temperature": temperature,
		"max_tokens": 256,
	});

	// Add tool definitions if MCP is enabled
	if config.mcp.enabled {
		let functions = crate::session::mcp::get_available_functions(config).await;
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
		let error_msg = if let Some(msg) = response_json.get("error").and_then(|e| e.get("message")).and_then(|m| m.as_str()) {
			msg
		} else if let Some(msg) = response_json.get("error").and_then(|e| e.as_str()) {
			msg
		} else {
			"Unknown API error"
		};

		return Err(anyhow::anyhow!("OpenRouter API error: {}", error_msg));
	}

	// Extract content from response
	let message = response_json
		.get("choices")
		.and_then(|choices| choices.get(0))
		.and_then(|choice| choice.get("message"))
		.ok_or_else(|| anyhow::anyhow!("Invalid response format from OpenRouter: {}", response_text))?;

	// Check if the response contains tool calls
	let mut content = String::new();

	// First check for content
	if let Some(text) = message.get("content").and_then(|c| c.as_str()) {
		content = text.to_string();
	}

	// Then check for tool calls
	if let Some(tool_calls) = message.get("tool_calls") {
		if tool_calls.is_array() && !tool_calls.as_array().unwrap().is_empty() {
			// Parse the tool calls into a format the MCP handler can use
			let mut mcp_tool_calls = Vec::new();

			// Iterate through the tool calls array
			for tool_call in tool_calls.as_array().unwrap() {
				// Extract the function information
				if let Some(function) = tool_call.get("function") {
					if let (Some(name), Some(args)) = (function.get("name").and_then(|n| n.as_str()),
												  function.get("arguments").and_then(|a| a.as_str())) {
						// Parse the arguments as JSON
						let params = match serde_json::from_str::<serde_json::Value>(args) {
							Ok(json_params) => json_params,
							Err(_) => {
								// Fallback: use arguments as a raw string if parsing fails
								serde_json::Value::String(args.to_string())
							}
						};

						// Create an MCP tool call
						let _tool_id = tool_call.get("id").and_then(|i| i.as_str()).unwrap_or("");
						let mcp_call = crate::session::mcp::McpToolCall {
							tool_name: name.to_string(),
							parameters: params,
						};

						mcp_tool_calls.push(mcp_call);
					}
				} else if let (Some(_id), Some(name)) = (
					tool_call.get("id").and_then(|i| i.as_str()),
					tool_call.get("name").and_then(|n| n.as_str())
				) {
					// Handle the direct tool call format (used by some models)
					let params = if let Some(params_obj) = tool_call.get("parameters") {
						params_obj.clone()
					} else {
						serde_json::json!({})
					};

					let mcp_call = crate::session::mcp::McpToolCall {
						tool_name: name.to_string(),
						parameters: params,
					};

					mcp_tool_calls.push(mcp_call);
				}
			}

			// Create the exchange record for logging
			let exchange = OpenRouterExchange {
				request: request_body,
				response: response_json.clone(),
				timestamp: SystemTime::now()
					.duration_since(UNIX_EPOCH)
					.unwrap_or_default()
					.as_secs(),
			};

			// Format tool calls in MCP-compatible format for parsing
			let tool_calls_json = serde_json::to_string(&mcp_tool_calls).unwrap_or_else(|_| "[]".to_string());
			let formatted_tool_calls = format!("<function_calls>{}\n</function_calls>", tool_calls_json);

			// If there's already content, keep it and append the tool calls in MCP format
			if !content.is_empty() {
				content = format!("{}

{}", content, formatted_tool_calls);
			} else {
				// If there's no content, just use the formatted tool calls
				content = formatted_tool_calls;
			}

			// Return with the properly formatted tool calls that MCP parser can handle
			return Ok((content, exchange));

		} else if content.is_empty() {
			return Err(anyhow::anyhow!("Invalid response: no content or tool calls"));
		}
	} else if content.is_empty() {
		return Err(anyhow::anyhow!("Invalid response: no content or tool calls"));
	}

	// Create exchange record for logging
	let exchange = OpenRouterExchange {
		request: request_body,
		response: response_json.clone(),
		timestamp: SystemTime::now()
			.duration_since(UNIX_EPOCH)
			.unwrap_or_default()
			.as_secs(),
	};

	Ok((content, exchange))
}
