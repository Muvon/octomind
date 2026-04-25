// Copyright 2026 Muvon Un Limited
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

//! Provider abstraction layer - now powered by octolib
//!
//! This module serves as an adapter between Octomind and the octolib provider system.
//! It maintains backward compatibility while leveraging the self-sufficient octolib crate.

use crate::config::Config;
use crate::session::Message;
use tokio::sync::watch;

// Re-export octolib types with compatibility aliases
pub use octolib::llm::{
	AiProvider, AmazonBedrockProvider, AnthropicProvider, CloudflareWorkersAiProvider,
	DeepSeekProvider, GenericToolCall, GoogleVertexProvider, OpenAiProvider, OpenRouterProvider,
	ProviderFactory, StructuredOutputRequest,
};

// Re-export some octolib types directly
pub use octolib::llm::{ModelPricing, ProviderExchange, ThinkingBlock, TokenUsage};

// Define Octomind-specific ProviderResponse that uses McpToolCall
#[derive(Debug, Clone)]
pub struct ProviderResponse {
	pub content: String,
	pub exchange: ProviderExchange,
	pub tool_calls: Option<Vec<crate::mcp::McpToolCall>>,
	pub thinking: Option<ThinkingBlock>,
	pub finish_reason: Option<String>,
	pub response_id: Option<String>,
	pub structured_output: Option<serde_json::Value>,
}

// Keep the original ChatCompletionParams for backward compatibility
/// Parameters for chat completion requests (Octomind version)
///
/// This struct maintains the original Octomind API while adapting to octolib internally.
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
	/// Optional JSON schema for structured output
	pub schema: Option<serde_json::Value>,
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
			schema: None,
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

	/// Set JSON schema for structured output
	pub fn with_schema(mut self, schema: serde_json::Value) -> Self {
		self.schema = Some(schema);
		self
	}

	/// Convert to octolib ChatCompletionParams with MCP tools
	pub async fn to_octolib_params(
		&self,
	) -> Result<octolib::llm::ChatCompletionParams, octolib::MessageError> {
		let octolib_messages: Result<Vec<octolib::llm::Message>, _> = self
			.messages
			.iter()
			.map(convert_message_to_octolib)
			.collect();

		let mut octolib_messages = octolib_messages?;

		// Set long cache TTL on system message when configured
		if self.config.use_long_system_cache {
			if let Some(sys_msg) = octolib_messages
				.iter_mut()
				.find(|m| m.role == "system" && m.cached)
			{
				sys_msg.cache_ttl = Some("1h".to_string());
			}
		}

		// Some providers (e.g. Gemini, Mistral) require the last message to be from the user.
		// After conversation compression the last message can be an assistant summary, which
		// causes those providers to return an error.  Appending a lightweight "Please continue."
		// user message is the safest fix: it satisfies the constraint without altering session
		// state and is semantically neutral (the model simply continues from where it left off).
		let last_non_system_is_assistant = octolib_messages
			.iter()
			.rev()
			.find(|m| m.role != "system")
			.map(|m| m.role == "assistant")
			.unwrap_or(false);

		if last_non_system_is_assistant {
			crate::log_debug!(
				"Last message is assistant after compression - appending synthetic user message to satisfy provider requirements"
			);
			let synthetic = octolib::llm::MessageBuilder::user("Please continue.")
				.build()
				.map_err(|_| octolib::MessageError::InvalidRole {
					role: "synthetic_user".to_string(),
				})?;
			octolib_messages.push(synthetic);
		}

		let mut params = octolib::llm::ChatCompletionParams::new(
			&octolib_messages,
			self.model,
			self.temperature,
			self.top_p,
			self.top_k,
			self.max_tokens,
		)
		.with_max_retries(self.max_retries)
		.with_retry_timeout(self.retry_timeout)
		.with_request_timeout(match self.config.request_timeout_seconds {
			0 => None,
			n => Some(std::time::Duration::from_secs(n as u64)),
		})
		.with_long_cache(self.config.use_long_system_cache);

		if let Some(token) = &self.cancellation_token {
			params = params.with_cancellation_token(token.clone());
		}

		// Fetch and add MCP tools if MCP is configured
		if !self.config.mcp.servers.is_empty() {
			let mcp_functions = crate::mcp::get_available_functions(self.config).await;
			if !mcp_functions.is_empty() {
				// Convert MCP functions to octolib FunctionDefinitions
				let mut octolib_tools: Vec<octolib::llm::FunctionDefinition> = mcp_functions
					.into_iter()
					.map(|f| octolib::llm::FunctionDefinition {
						name: f.name,
						description: f.description,
						parameters: f.parameters,
						cache_control: None, // Will be set below if needed
					})
					.collect();

				// Add cache control to the LAST tool if system message is cached
				// This matches the old Anthropic provider behavior
				let system_cached = self.messages.iter().any(|m| m.role == "system" && m.cached);
				if system_cached && !octolib_tools.is_empty() {
					if let Some(last_tool) = octolib_tools.last_mut() {
						// Use same TTL logic as system message
						let ttl = if self.config.use_long_system_cache {
							"1h"
						} else {
							"5m"
						};
						last_tool.cache_control = Some(serde_json::json!({
							"type": "ephemeral",
							"ttl": ttl
						}));
					}
				}

				params = params.with_tools(octolib_tools);
			}
		}

		// Apply structured output schema if provided
		if let Some(ref schema) = self.schema {
			params = params.with_structured_output(
				StructuredOutputRequest::json_schema(schema.clone()).with_strict_mode(),
			);
		}

		Ok(params)
	}
}

/// Convert Octomind Message to octolib Message with proper error handling
fn convert_message_to_octolib(
	msg: &Message,
) -> Result<octolib::llm::Message, octolib::MessageError> {
	let mut builder = match msg.role.as_str() {
		"user" => octolib::llm::MessageBuilder::user(&msg.content),
		"assistant" => {
			let mut builder = octolib::llm::MessageBuilder::assistant(&msg.content);
			// CRITICAL: Convert tool_calls to unified GenericToolCall format
			if let Some(ref tool_calls) = msg.tool_calls {
				let generic_calls = convert_to_generic_tool_calls(tool_calls);
				if !generic_calls.is_empty() {
					builder = builder.with_tool_calls(generic_calls);
				}
			}
			builder
		}
		"system" => octolib::llm::MessageBuilder::system(&msg.content),
		"tool" => {
			let tool_call_id = msg.tool_call_id.as_deref().ok_or_else(|| {
				octolib::MessageError::MissingToolField {
					field: "tool_call_id".to_string(),
				}
			})?;
			let name =
				msg.name
					.as_deref()
					.ok_or_else(|| octolib::MessageError::MissingToolField {
						field: "name".to_string(),
					})?;
			octolib::llm::MessageBuilder::tool(
				msg.content.clone(),
				tool_call_id.to_string(),
				name.to_string(),
			)
		}
		_ => {
			return Err(octolib::MessageError::InvalidRole {
				role: msg.role.clone(),
			})
		}
	};

	// Set timestamp
	builder = builder.timestamp(msg.timestamp);

	// Set message ID if present (for assistant messages with tool calls)
	if let Some(ref id) = msg.id {
		builder = builder.id(id);
	}

	// Set cache marker and TTL if needed
	if msg.cached {
		builder = builder.cached();
		if let Some(ref ttl) = msg.cache_ttl {
			builder = builder.cache_ttl(ttl);
		}
	}

	// Convert images if present
	if let Some(images) = &msg.images {
		let octolib_images: Vec<octolib::llm::ImageAttachment> =
			images.iter().map(convert_image_to_octolib).collect();
		builder = builder.with_images(octolib_images);
	}

	// Convert videos if present
	if let Some(videos) = &msg.videos {
		let octolib_videos: Vec<octolib::llm::VideoAttachment> =
			videos.iter().map(convert_video_to_octolib).collect();
		builder = builder.with_videos(octolib_videos);
	}

	// CRITICAL FIX: Convert thinking field for Moonshot and other thinking models
	// Moonshot requires reasoning_content for assistant messages with tool_calls
	// The thinking field is stored as serde_json::Value, convert to ThinkingBlock
	if let Some(ref thinking_value) = msg.thinking {
		match serde_json::from_value::<octolib::ThinkingBlock>(thinking_value.clone()) {
			Ok(thinking_block) => {
				builder = builder.thinking(thinking_block);
			}
			Err(e) => {
				// Only log failures - success is expected and too verbose
				crate::log_debug!(
					"Failed to deserialize thinking field for {} message: {}. Value: {:?}",
					msg.role,
					e,
					thinking_value
				);
			}
		}
	}

	builder.build()
}

/// Convert Octomind ImageAttachment to octolib ImageAttachment
fn convert_image_to_octolib(
	img: &crate::session::image::ImageAttachment,
) -> octolib::llm::ImageAttachment {
	let data = match &img.data {
		crate::session::image::ImageData::Base64(data) => {
			octolib::llm::ImageData::Base64(data.clone())
		}
		crate::session::image::ImageData::Url(url) => octolib::llm::ImageData::Url(url.clone()),
	};

	let source_type = match &img.source_type {
		crate::session::image::SourceType::File(path) => {
			octolib::llm::SourceType::File(path.clone())
		}
		crate::session::image::SourceType::Clipboard => octolib::llm::SourceType::Clipboard,
		crate::session::image::SourceType::Url => octolib::llm::SourceType::Url,
	};

	octolib::llm::ImageAttachment {
		data,
		media_type: img.media_type.clone(),
		source_type,
		dimensions: img.dimensions,
		size_bytes: img.size_bytes,
	}
}

/// Convert Octomind VideoAttachment to octolib VideoAttachment
fn convert_video_to_octolib(
	video: &crate::session::video::VideoAttachment,
) -> octolib::llm::VideoAttachment {
	let data = match &video.data {
		crate::session::video::VideoData::Base64(data) => {
			octolib::llm::VideoData::Base64(data.clone())
		}
		crate::session::video::VideoData::Url(url) => octolib::llm::VideoData::Url(url.clone()),
	};

	let source_type = match &video.source_type {
		crate::session::video::SourceType::File(path) => {
			octolib::llm::SourceType::File(path.clone())
		}
		crate::session::video::SourceType::Clipboard => octolib::llm::SourceType::Clipboard,
		crate::session::video::SourceType::Url => octolib::llm::SourceType::Url,
	};

	octolib::llm::VideoAttachment {
		data,
		media_type: video.media_type.clone(),
		source_type,
		dimensions: video.dimensions,
		size_bytes: video.size_bytes,
		duration_secs: video.duration_secs,
	}
}

/// Convert tool_calls from session format to unified GenericToolCall format
///
/// Session loading reconstructs tool_calls in OpenAI format. This function converts
/// them to the unified GenericToolCall format that octolib requires.
/// NO FALLBACKS - unified format is MANDATORY.
fn convert_to_generic_tool_calls(
	tool_calls: &serde_json::Value,
) -> Vec<octolib::llm::GenericToolCall> {
	// Check if it's already in unified GenericToolCall format
	if let Ok(calls) =
		serde_json::from_value::<Vec<octolib::llm::GenericToolCall>>(tool_calls.clone())
	{
		return calls;
	}

	// Handle OpenAI format (array with "type": "function") - from session loading
	if let Some(calls_array) = tool_calls.as_array() {
		let mut generic_calls = Vec::new();
		for call in calls_array {
			if let Some(function) = call.get("function") {
				if let (Some(id), Some(name), Some(args_str)) = (
					call.get("id").and_then(|v| v.as_str()),
					function.get("name").and_then(|v| v.as_str()),
					function.get("arguments").and_then(|v| v.as_str()),
				) {
					// Parse arguments string to JSON
					let arguments = if args_str.trim().is_empty() {
						serde_json::json!({})
					} else {
						match serde_json::from_str::<serde_json::Value>(args_str) {
							Ok(json_args) => json_args,
							Err(e) => {
								panic!("Failed to parse tool call arguments '{}': {}", args_str, e);
							}
						}
					};

					generic_calls.push(octolib::llm::GenericToolCall {
						id: id.to_string(),
						name: name.to_string(),
						arguments,
						meta: None, // Preserve meta from session if present
					});
				} else {
					panic!("Invalid OpenAI tool call format - missing required fields");
				}
			} else {
				panic!("Invalid tool call format - missing 'function' field");
			}
		}
		return generic_calls;
	}

	panic!("Unsupported tool_calls format - must be Vec<GenericToolCall> or OpenAI format array");
}

/// Convert octolib ProviderResponse to Octomind ProviderResponse
pub fn convert_response_from_octolib(response: octolib::llm::ProviderResponse) -> ProviderResponse {
	// Convert tool calls if present
	let tool_calls = response.tool_calls.map(|calls| {
		calls
			.into_iter()
			.map(|call| crate::mcp::McpToolCall {
				tool_name: call.name,
				tool_id: call.id,
				parameters: call.arguments,
			})
			.collect()
	});

	ProviderResponse {
		content: response.content,
		exchange: response.exchange,
		tool_calls,
		thinking: response.thinking,
		finish_reason: response.finish_reason,
		response_id: response.id,
		structured_output: response.structured_output,
	}
}

// Keep the retry module for backward compatibility
pub mod retry {
	pub use octolib::llm::retry::*;
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_thinking_block_conversion() {
		// Test that ThinkingBlock can be serialized to JSON and back
		let thinking_block = ThinkingBlock {
			content: "Test thinking content".to_string(),
			tokens: 42,
		};

		// Serialize to JSON (simulating storage in session)
		let json_value = serde_json::to_value(&thinking_block).expect("Failed to serialize");
		println!("Serialized: {}", json_value);

		// Deserialize back (simulating loading from session)
		let deserialized: ThinkingBlock =
			serde_json::from_value(json_value).expect("Failed to deserialize");
		println!("Deserialized: {:?}", deserialized);

		assert_eq!(deserialized.content, "Test thinking content");
		assert_eq!(deserialized.tokens, 42);
	}
}
