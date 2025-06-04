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

// Google Vertex AI provider implementation

use super::{AiProvider, ProviderExchange, ProviderResponse, TokenUsage};
use crate::config::Config;
use crate::log_debug;
use crate::session::Message;
use anyhow::Result;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::env;

/// Google Vertex AI pricing constants (per 1M tokens in USD)
/// Source: https://cloud.google.com/vertex-ai/generative-ai/pricing (as of January 2025)
const PRICING: &[(&str, f64, f64)] = &[
	// Model, Input price per 1M tokens, Output price per 1M tokens
	// Gemini 2.5 models (latest)
	("gemini-2.5-pro", 1.25, 10.00), // <= 200K tokens, higher rates for >200K
	("gemini-2.5-flash", 0.15, 0.60), // Consistent pricing
	// Gemini 2.0 models
	("gemini-2.0-flash", 0.15, 0.60), // Token-based pricing
	("gemini-2.0-flash-lite", 0.075, 0.30),
	// Gemini 1.5 models
	("gemini-1.5-pro", 0.3125, 1.25), // <= 128K tokens, converted from character pricing
	("gemini-1.5-flash", 0.075, 0.30), // <= 128K tokens, converted from character pricing
	// Gemini 1.0 models
	("gemini-1.0-pro", 0.50, 1.50), // Converted from character pricing
	("gemini-pro", 0.50, 1.50),     // Alias for gemini-1.0-pro
	// Legacy models
	("text-bison", 1.00, 2.00),
	("chat-bison", 1.00, 2.00),
	("code-bison", 1.00, 2.00),
	("codechat-bison", 1.00, 2.00),
];

/// Calculate cost for Google Vertex AI models
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

/// Google Vertex AI provider implementation
pub struct GoogleVertexProvider;

impl Default for GoogleVertexProvider {
	fn default() -> Self {
		Self::new()
	}
}

impl GoogleVertexProvider {
	pub fn new() -> Self {
		Self
	}
}

// Constants
const GOOGLE_APPLICATION_CREDENTIALS_ENV: &str = "GOOGLE_APPLICATION_CREDENTIALS";
const GOOGLE_PROJECT_ID_ENV: &str = "GOOGLE_PROJECT_ID";
const GOOGLE_REGION_ENV: &str = "GOOGLE_REGION";

/// Message format for the Google Vertex AI API
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VertexMessage {
	pub role: String,
	pub parts: Vec<serde_json::Value>,
}

#[async_trait::async_trait]
impl AiProvider for GoogleVertexProvider {
	fn name(&self) -> &str {
		"google"
	}

	fn supports_model(&self, model: &str) -> bool {
		// Google Vertex AI models
		model.starts_with("gemini-2.5")
			|| model.starts_with("gemini-2.0")
			|| model.starts_with("gemini-1.5")
			|| model.starts_with("gemini-1.0")
			|| model.starts_with("gemini")
			|| model.contains("bison")
			|| model.starts_with("text-")
			|| model.starts_with("chat-")
			|| model.starts_with("code")
	}

	fn get_api_key(&self, _config: &Config) -> Result<String> {
		// Google Vertex AI uses service account authentication
		// Check for required environment variables
		if env::var(GOOGLE_APPLICATION_CREDENTIALS_ENV).is_err() {
			return Err(anyhow::anyhow!(
				"Google Vertex AI requires service account authentication. Please set {} environment variable to path of your service account JSON file",
				GOOGLE_APPLICATION_CREDENTIALS_ENV
			));
		}

		if env::var(GOOGLE_PROJECT_ID_ENV).is_err() {
			return Err(anyhow::anyhow!(
				"Google Vertex AI requires project ID. Please set {} environment variable",
				GOOGLE_PROJECT_ID_ENV
			));
		}

		// For now, return a placeholder - actual implementation would need OAuth2 token
		Ok("service_account_auth".to_string())
	}

	fn supports_caching(&self, model: &str) -> bool {
		// Google Vertex AI supports caching for Gemini 1.5+ models
		// Source: https://cloud.google.com/vertex-ai/generative-ai/docs/context-cache
		model.contains("gemini-2.5") || model.contains("gemini-2.0") || model.contains("gemini-1.5")
	}

	async fn chat_completion(
		&self,
		messages: &[Message],
		model: &str,
		temperature: f32,
		config: &Config,
	) -> Result<ProviderResponse> {
		// Get required environment variables
		let project_id = env::var(GOOGLE_PROJECT_ID_ENV)
			.map_err(|_| anyhow::anyhow!("GOOGLE_PROJECT_ID environment variable is required"))?;

		let region = env::var(GOOGLE_REGION_ENV).unwrap_or_else(|_| "us-central1".to_string());

		// Get OAuth2 token (simplified - real implementation would use proper OAuth2)
		let access_token = self.get_access_token().await?;

		// Convert messages to Vertex AI format
		let vertex_messages = convert_messages(messages);

		// Build the API URL
		let api_url = format!(
			"https://{}-aiplatform.googleapis.com/v1/projects/{}/locations/{}/publishers/google/models/{}:generateContent",
			region, project_id, region, model
		);

		// Create the request body
		let mut request_body = serde_json::json!({
				"contents": vertex_messages,
				"generationConfig": {
				"temperature": temperature,
				"maxOutputTokens": 8192,
				"candidateCount": 1
			}
		});

		// Add tool definitions if MCP has any servers configured (simplified for Vertex AI)
		if !config.mcp.servers.is_empty() {
			let functions = crate::mcp::get_available_functions(config).await;
			if !functions.is_empty() {
				// CRITICAL FIX: Ensure tool definitions are ALWAYS in the same order
				// Sort functions by name to guarantee consistent ordering across API calls
				let mut sorted_functions = functions;
				sorted_functions.sort_by(|a, b| a.name.cmp(&b.name));

				let tools = sorted_functions
					.iter()
					.map(|f| {
						serde_json::json!({
								"functionDeclarations": [{
								"name": f.name,
								"description": f.description,
								"parameters": f.parameters
							}]
						})
					})
					.collect::<Vec<_>>();

				request_body["tools"] = serde_json::json!(tools);
			}
		}

		// Create HTTP client
		let client = Client::new();

		// Make the actual API request
		let response = client
			.post(&api_url)
			.header("Authorization", format!("Bearer {}", access_token))
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

			if let Some(error_obj) = response_json.get("error") {
				if let Some(msg) = error_obj.get("message").and_then(|m| m.as_str()) {
					error_details.push(format!("Message: {}", msg));
				}
				if let Some(code) = error_obj.get("code").and_then(|c| c.as_i64()) {
					error_details.push(format!("Code: {}", code));
				}
			}

			if error_details.len() == 1 {
				error_details.push(format!("Raw response: {}", response_text));
			}

			let full_error = error_details.join(" | ");
			return Err(anyhow::anyhow!(
				"Google Vertex AI API error: {}",
				full_error
			));
		}

		// Extract content from response
		let mut content = String::new();
		let mut tool_calls = None;

		if let Some(candidates) = response_json.get("candidates").and_then(|c| c.as_array()) {
			if let Some(candidate) = candidates.first() {
				if let Some(content_parts) = candidate
					.get("content")
					.and_then(|c| c.get("parts"))
					.and_then(|p| p.as_array())
				{
					for part in content_parts {
						if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
							content.push_str(text);
						} else if let Some(function_call) = part.get("functionCall") {
							// Handle function calls
							if tool_calls.is_none() {
								tool_calls = Some(Vec::new());
							}

							if let (Some(name), Some(args)) = (
								function_call.get("name").and_then(|n| n.as_str()),
								function_call.get("args"),
							) {
								// CRITICAL FIX: Generate consistent tool IDs for Google Vertex AI
								// Instead of random UUIDs, create deterministic IDs based on function name and args
								let args_hash = {
									let args_str = serde_json::to_string(args).unwrap_or_default();
									use std::collections::hash_map::DefaultHasher;
									use std::hash::{Hash, Hasher};
									let mut hasher = DefaultHasher::new();
									name.hash(&mut hasher);
									args_str.hash(&mut hasher);
									hasher.finish()
								};
								let tool_id = format!("vertex_{}_{:x}", name, args_hash);

								let mcp_call = crate::mcp::McpToolCall {
									tool_name: name.to_string(),
									parameters: args.clone(),
									tool_id,
								};

								if let Some(ref mut calls) = tool_calls {
									calls.push(mcp_call);
								}
							}
						}
					}
				}

				// Extract finish_reason
				let finish_reason = candidate
					.get("finishReason")
					.and_then(|fr| fr.as_str())
					.map(|s| s.to_string());

				if let Some(ref reason) = finish_reason {
					log_debug!("Finish reason: {}", reason);
				}
			}
		}

		// Extract token usage
		let usage: Option<TokenUsage> = if let Some(usage_obj) = response_json.get("usageMetadata")
		{
			let prompt_tokens = usage_obj
				.get("promptTokenCount")
				.and_then(|v| v.as_u64())
				.unwrap_or(0);
			let completion_tokens = usage_obj
				.get("candidatesTokenCount")
				.and_then(|v| v.as_u64())
				.unwrap_or(0);
			let total_tokens = usage_obj
				.get("totalTokenCount")
				.and_then(|v| v.as_u64())
				.unwrap_or_else(|| prompt_tokens + completion_tokens);

			// Calculate cost using our pricing constants
			let cost = calculate_cost(model, prompt_tokens, completion_tokens);

			Some(TokenUsage {
				prompt_tokens,
				completion_tokens,
				total_tokens,
				cost,
				completion_tokens_details: None,
				prompt_tokens_details: None,
				breakdown: None,
				request_time_ms: None, // TODO: Add API timing for Google
			})
		} else {
			None
		};

		// CRITICAL FIX: Store the original content parts for proper function call reconstruction
		// This ensures functionResponse messages can reference the correct function call
		let stored_tool_calls = if tool_calls.is_some() {
			// If we found function calls, store the complete content parts
			// This preserves both text content and functionCall blocks for conversation history
			response_json
				.get("candidates")
				.and_then(|c| c.as_array())
				.and_then(|candidates| candidates.first())
				.and_then(|candidate| candidate.get("content"))
				.and_then(|content| content.get("parts"))
				.cloned()
		} else {
			None
		};

		// Create exchange record
		let mut exchange = ProviderExchange::new(request_body, response_json, usage, self.name());

		// CRITICAL FIX: Store the original function calls in the exchange for later reconstruction
		if let Some(ref content_parts) = stored_tool_calls {
			exchange.response["tool_calls_content"] = content_parts.clone();
		}

		Ok(ProviderResponse {
			content,
			exchange,
			tool_calls,
			finish_reason: None, // Vertex AI doesn't provide finish_reason in the same format
		})
	}
}

impl GoogleVertexProvider {
	// Simplified OAuth2 token retrieval - real implementation would be more robust
	async fn get_access_token(&self) -> Result<String> {
		// This is a simplified implementation
		// Real implementation would:
		// 1. Read service account JSON file
		// 2. Create JWT token
		// 3. Exchange for OAuth2 access token
		// 4. Cache and refresh tokens as needed

		// For now, return an error with instructions
		Err(anyhow::anyhow!(
			"Google Vertex AI provider requires proper OAuth2 implementation. \
			This is a placeholder implementation. You would need to implement proper \
			service account authentication using the google-cloud-auth crate or similar."
		))
	}
}

// Convert our session messages to Vertex AI format
// NOTE: Google Vertex AI supports caching for Gemini 1.5 models using context cache
// Cache markers are handled for supported models
fn convert_messages(messages: &[Message]) -> Vec<VertexMessage> {
	let mut result = Vec::new();

	for msg in messages {
		// Skip system messages - Vertex AI handles them differently
		if msg.role == "system" {
			continue;
		}

		// Handle tool response messages (has <fnr> tags)
		if msg.role == "user" && msg.content.starts_with("<fnr>") && msg.content.ends_with("</fnr>")
		{
			let content = msg
				.content
				.trim_start_matches("<fnr>")
				.trim_end_matches("</fnr>")
				.trim();

			if let Ok(tool_responses) = serde_json::from_str::<Vec<serde_json::Value>>(content) {
				if !tool_responses.is_empty()
					&& tool_responses[0]
						.get("role")
						.is_some_and(|r| r.as_str().unwrap_or("") == "tool")
				{
					for tool_response in tool_responses {
						let content_text = tool_response
							.get("content")
							.and_then(|c| c.as_str())
							.unwrap_or("");

						result.push(VertexMessage {
							role: "user".to_string(),
							parts: vec![serde_json::json!({
								"functionResponse": {
									"name": "tool_result",
									"response": {
										"content": content_text
									}
								}
							})],
						});
					}
					continue;
				}
			}
		} else if msg.role == "tool" {
			result.push(VertexMessage {
				role: "user".to_string(),
				parts: vec![serde_json::json!({
					"functionResponse": {
						"name": "tool_result",
						"response": {
							"content": msg.content
						}
					}
				})],
			});
			continue;
		}

		// Convert role to Vertex AI format
		let vertex_role = match msg.role.as_str() {
			"assistant" => "model",
			_ => "user",
		};

		// CRITICAL FIX: Handle assistant messages with function calls
		if msg.role == "assistant" {
			let mut parts = Vec::new();

			// Add text content if not empty
			if !msg.content.is_empty() {
				parts.push(serde_json::json!({
					"text": msg.content
				}));
			}

			// CRITICAL FIX: Preserve function calls from original API response
			// This ensures functionResponse messages can reference the correct function call
			if let Some(ref tool_calls_data) = msg.tool_calls {
				// Handle function calls from Google Vertex AI format
				if let Some(content_parts) = tool_calls_data.as_array() {
					// If tool_calls contains Vertex AI format content parts, extract functionCall blocks
					for content_part in content_parts {
						if content_part.get("functionCall").is_some() {
							parts.push(content_part.clone());
						}
					}
				}
			}

			result.push(VertexMessage {
				role: vertex_role.to_string(),
				parts,
			});
			continue;
		}

		// Regular messages
		result.push(VertexMessage {
			role: vertex_role.to_string(),
			parts: vec![serde_json::json!({
				"text": msg.content
			})],
		});
	}

	result
}
