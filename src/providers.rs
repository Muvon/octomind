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

/// The header carrying [`ModelPurpose`] to the octohub proxy.
pub const MODEL_PURPOSE_HEADER: &str = "X-Model-Purpose";

/// Where in octomind a model call originates. Sent on every completion as the
/// `X-Model-Purpose` header so octohub's virtual `auto` model can route each
/// purpose to a different real model; providers that aren't octohub ignore it.
///
/// This set is a CONTRACT with the control plane (the panel renders a model
/// picker per purpose) — extend it deliberately, never rename values.
/// Purposes are HIERARCHICAL on the octohub side, split on `-`: a map entry
/// for `supervisor` covers every `supervisor-*` purpose until a specific one
/// (e.g. `supervisor-gate`) is pinned. That's why each supervisor mechanic
/// sends its own purpose — redefinable individually, one row covers the family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ModelPurpose {
	/// The session's own conversation turns — also cache keepalive pings,
	/// which must hit the same model they are keeping warm.
	#[default]
	Main,
	/// Verify-gate completion checks.
	SupervisorGate,
	/// Tool-output condensation (task-aware narrowing).
	SupervisorCondense,
	/// End-of-trajectory lesson/orientation extraction.
	SupervisorDistill,
	/// Recall keyword/query preparation.
	SupervisorRecall,
	/// Conversation-compression decisions and summaries.
	Compression,
}

impl ModelPurpose {
	pub fn as_str(&self) -> &'static str {
		match self {
			Self::Main => "main",
			Self::SupervisorGate => "supervisor-gate",
			Self::SupervisorCondense => "supervisor-condense",
			Self::SupervisorDistill => "supervisor-distill",
			Self::SupervisorRecall => "supervisor-recall",
			Self::Compression => "compression",
		}
	}
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
	/// Optional reasoning effort override (falls back to `config.reasoning_effort`)
	pub reasoning_effort: Option<crate::config::ReasoningEffortConfig>,
	/// Attach MCP tools to the request (default true). Text-only internal
	/// calls (compression, learning extraction) disable this: the model never
	/// calls tools there, the definitions waste input tokens, and their
	/// presence blocks schema enforcement on proxy providers.
	pub tools: bool,
	/// Where this call originates (main | supervisor | compression) — becomes
	/// the `X-Model-Purpose` header. Defaults to Main.
	pub purpose: ModelPurpose,
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
			reasoning_effort: None,
			tools: true,
			purpose: ModelPurpose::default(),
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

	/// Override reasoning effort for this call (otherwise inherits from config).
	pub fn with_reasoning_effort(mut self, effort: crate::config::ReasoningEffortConfig) -> Self {
		self.reasoning_effort = Some(effort);
		self
	}

	/// Don't attach MCP tools — for text-only calls (compression, learning).
	pub fn without_tools(mut self) -> Self {
		self.tools = false;
		self
	}

	/// Tag this call's origin for purpose-based routing (octohub `auto`).
	pub fn with_purpose(mut self, purpose: ModelPurpose) -> Self {
		self.purpose = purpose;
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

		// Long cache TTL on system message — always enabled (Anthropic 1h cache).
		if let Some(sys_msg) = octolib_messages
			.iter_mut()
			.find(|m| m.role == "system" && m.cached)
		{
			sys_msg.cache_ttl = Some("1h".to_string());
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
		.with_long_cache(true)
		.with_reasoning_effort(
			self.reasoning_effort
				.unwrap_or(self.config.reasoning_effort)
				.to_octolib(),
		)
		// Sent on every request: only the octohub proxy interprets it (for the
		// virtual `auto` model); other providers ignore an unknown X- header.
		.with_extra_headers(std::collections::HashMap::from([(
			MODEL_PURPOSE_HEADER.to_string(),
			self.purpose.as_str().to_string(),
		)]));

		if let Some(token) = &self.cancellation_token {
			params = params.with_cancellation_token(token.clone());
		}

		// Fetch and add MCP tools if MCP is configured
		if self.tools && !self.config.mcp.servers.is_empty() {
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
						last_tool.cache_control = Some(serde_json::json!({
							"type": "ephemeral",
							"ttl": "1h"
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
			// CRITICAL: Convert tool_calls to unified GenericToolCall format.
			// Malformed shapes return a typed error (see convert_to_generic_tool_calls)
			// rather than panicking, so a misbehaving model fails the request
			// cleanly instead of bringing the whole process down.
			if let Some(ref tool_calls) = msg.tool_calls {
				let generic_calls = convert_to_generic_tool_calls(tool_calls)?;
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

/// Convert tool_calls from session format to unified GenericToolCall format.
///
/// Session loading reconstructs tool_calls in OpenAI format. This function converts
/// them to the unified GenericToolCall format that octolib requires. Hostile or
/// buggy model output that doesn't match an expected shape returns a typed error
/// so the request fails cleanly — it must NOT crash the long-running process.
fn convert_to_generic_tool_calls(
	tool_calls: &serde_json::Value,
) -> Result<Vec<octolib::llm::GenericToolCall>, octolib::MessageError> {
	// Check if it's already in unified GenericToolCall format
	if let Ok(calls) =
		serde_json::from_value::<Vec<octolib::llm::GenericToolCall>>(tool_calls.clone())
	{
		return Ok(calls);
	}

	// Handle OpenAI format (array with "type": "function") - from session loading
	if let Some(calls_array) = tool_calls.as_array() {
		let mut generic_calls = Vec::new();
		for call in calls_array {
			let function =
				call.get("function")
					.ok_or_else(|| octolib::MessageError::MissingToolField {
						field: "function".to_string(),
					})?;

			let (id, name, args_str) = match (
				call.get("id").and_then(|v| v.as_str()),
				function.get("name").and_then(|v| v.as_str()),
				function.get("arguments").and_then(|v| v.as_str()),
			) {
				(Some(id), Some(name), Some(args)) => (id, name, args),
				_ => {
					return Err(octolib::MessageError::MissingToolField {
						field: "function.{id|name|arguments}".to_string(),
					})
				}
			};

			// Parse arguments string to JSON. `ToolCallsError` wraps the
			// underlying serde_json::Error via `#[from]`, surfacing the
			// exact parse failure to the caller.
			let arguments = if args_str.trim().is_empty() {
				serde_json::json!({})
			} else {
				serde_json::from_str::<serde_json::Value>(args_str)?
			};

			// Root-level meta, preserved exactly like the unified-format
			// branch above; absent or non-object meta stays None.
			let meta = call.get("meta").and_then(|m| m.as_object()).cloned();

			generic_calls.push(octolib::llm::GenericToolCall {
				id: id.to_string(),
				name: name.to_string(),
				arguments,
				meta,
			});
		}
		return Ok(generic_calls);
	}

	Err(octolib::MessageError::MissingToolField {
		field: "tool_calls (root must be Vec<GenericToolCall> or OpenAI array)".to_string(),
	})
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
	fn test_model_purpose_contract_strings() {
		// These exact strings are a cross-repo CONTRACT: octohub routes by them
		// and the panel renders a picker per purpose. Renaming one silently
		// breaks purpose routing for every deployed CLI — this test is the tripwire.
		assert_eq!(MODEL_PURPOSE_HEADER, "X-Model-Purpose");
		assert_eq!(ModelPurpose::Main.as_str(), "main");
		assert_eq!(ModelPurpose::Compression.as_str(), "compression");
		// Supervisor purposes share the `supervisor-` prefix ON PURPOSE: octohub
		// resolves hierarchically on `-`, so one `supervisor` map row covers all
		// of these until a specific one is pinned.
		assert_eq!(ModelPurpose::SupervisorGate.as_str(), "supervisor-gate");
		assert_eq!(
			ModelPurpose::SupervisorCondense.as_str(),
			"supervisor-condense"
		);
		assert_eq!(
			ModelPurpose::SupervisorDistill.as_str(),
			"supervisor-distill"
		);
		assert_eq!(ModelPurpose::SupervisorRecall.as_str(), "supervisor-recall");
		// Untagged calls are MAIN traffic — session turns must never silently
		// become something a cheaper purpose route would catch.
		assert_eq!(ModelPurpose::default(), ModelPurpose::Main);
	}

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

	// ── convert_to_generic_tool_calls ──────────────────────────────

	#[test]
	fn test_generic_tool_calls_passthrough_unified_format() {
		let value = serde_json::json!([
			{ "id": "call_1", "name": "read", "arguments": {"path": "/x"} },
			{
				"id": "call_2",
				"name": "write",
				"arguments": {},
				"meta": {"origin": "test"}
			}
		]);
		let calls =
			convert_to_generic_tool_calls(&value).expect("unified format must pass through");
		assert_eq!(calls.len(), 2);
		assert_eq!(calls[0].id, "call_1");
		assert_eq!(calls[0].name, "read");
		assert_eq!(calls[0].arguments, serde_json::json!({"path": "/x"}));
		assert!(calls[0].meta.is_none());
		assert_eq!(
			calls[1].meta.as_ref().and_then(|m| m.get("origin")),
			Some(&serde_json::json!("test")),
			"meta must survive the passthrough"
		);
	}

	#[test]
	fn test_openai_format_converts_with_parsed_arguments() {
		let value = serde_json::json!([
			{
				"id": "call_1",
				"type": "function",
				"function": {
					"name": "read",
					"arguments": "{\"path\": \"/x\"}"
				}
			}
		]);
		let calls = convert_to_generic_tool_calls(&value).expect("OpenAI format must convert");
		assert_eq!(calls.len(), 1);
		assert_eq!(calls[0].id, "call_1");
		assert_eq!(calls[0].name, "read");
		assert_eq!(calls[0].arguments, serde_json::json!({"path": "/x"}));
	}

	#[test]
	fn test_openai_format_preserves_root_meta_when_present() {
		// WITH meta at the tool-call root → preserved verbatim, like the
		// unified branch
		let with_meta = serde_json::json!([
			{
				"id": "call_1",
				"type": "function",
				"function": {"name": "read", "arguments": "{}"},
				"meta": {"origin": "session"}
			}
		]);
		let calls = convert_to_generic_tool_calls(&with_meta)
			.expect("OpenAI format with meta must convert");
		assert_eq!(calls.len(), 1);
		let expected_meta: serde_json::Map<String, serde_json::Value> =
			serde_json::from_value(with_meta[0]["meta"].clone()).unwrap();
		assert_eq!(calls[0].meta, Some(expected_meta));

		// WITHOUT meta → None
		let without_meta = serde_json::json!([
			{
				"id": "call_2",
				"type": "function",
				"function": {"name": "read", "arguments": "{}"}
			}
		]);
		let calls = convert_to_generic_tool_calls(&without_meta)
			.expect("OpenAI format without meta must convert");
		assert!(calls[0].meta.is_none());
	}

	#[test]
	fn test_openai_format_blank_arguments_become_empty_object() {
		for args in ["", "   "] {
			let value = serde_json::json!([
				{
					"id": "call_1",
					"type": "function",
					"function": {"name": "ping", "arguments": args}
				}
			]);
			let calls = convert_to_generic_tool_calls(&value)
				.expect("blank arguments must convert to an empty object");
			assert_eq!(calls[0].arguments, serde_json::json!({}), "args = {args:?}");
		}
	}

	#[test]
	fn test_openai_format_missing_function_errors() {
		let value = serde_json::json!([{ "id": "call_1", "type": "function" }]);
		let err = convert_to_generic_tool_calls(&value).expect_err("missing function must fail");
		match err {
			octolib::MessageError::MissingToolField { field } => {
				assert_eq!(field, "function", "unexpected field name: {field}")
			}
			other => panic!("expected MissingToolField, got {other:?}"),
		}
	}

	#[test]
	fn test_openai_format_missing_id_name_or_arguments_errors() {
		let value = serde_json::json!([
			{
				"type": "function",
				"function": {"name": "read", "arguments": "{}"}
			}
		]);
		let err = convert_to_generic_tool_calls(&value).expect_err("missing id must fail");
		match err {
			octolib::MessageError::MissingToolField { field } => assert_eq!(
				field, "function.{id|name|arguments}",
				"unexpected field name: {field}"
			),
			other => panic!("expected MissingToolField, got {other:?}"),
		}
	}

	#[test]
	fn test_openai_format_invalid_json_arguments_error() {
		let value = serde_json::json!([
			{
				"id": "call_1",
				"type": "function",
				"function": {"name": "read", "arguments": "not json at all"}
			}
		]);
		let err =
			convert_to_generic_tool_calls(&value).expect_err("invalid JSON arguments must fail");
		assert!(
			matches!(err, octolib::MessageError::ToolCallsError(_)),
			"expected ToolCallsError, got {err:?}"
		);
	}

	#[test]
	fn test_non_array_tool_calls_root_errors() {
		for value in [serde_json::json!({"foo": 1}), serde_json::json!("string")] {
			let err = convert_to_generic_tool_calls(&value).expect_err("non-array root must fail");
			match err {
				octolib::MessageError::MissingToolField { field } => assert_eq!(
					field, "tool_calls (root must be Vec<GenericToolCall> or OpenAI array)",
					"unexpected field name: {field}"
				),
				other => panic!("expected MissingToolField, got {other:?}"),
			}
		}
	}

	#[test]
	fn test_empty_tool_calls_array_returns_empty_vec() {
		let calls = convert_to_generic_tool_calls(&serde_json::json!([]))
			.expect("empty array is valid unified format");
		assert!(calls.is_empty());
	}

	// ── convert_message_to_octolib ─────────────────────────────────

	fn msg(role: &str, content: &str) -> Message {
		Message {
			role: role.to_string(),
			content: content.to_string(),
			timestamp: 1_700_000_000,
			cached: false,
			cache_ttl: None,
			tool_call_id: None,
			name: None,
			tool_calls: None,
			images: None,
			videos: None,
			thinking: None,
			id: None,
		}
	}

	#[test]
	fn test_convert_preserves_role_content_and_timestamp() {
		for role in ["user", "assistant", "system"] {
			let converted = convert_message_to_octolib(&msg(role, "hello"))
				.unwrap_or_else(|e| panic!("{role} must convert: {e:?}"));
			assert_eq!(converted.role, role);
			assert_eq!(converted.content, "hello");
			assert_eq!(converted.timestamp, 1_700_000_000);
		}
	}

	#[test]
	fn test_convert_tool_message_requires_call_id_and_name() {
		let missing_id = Message {
			name: Some("tool_a".to_string()),
			..msg("tool", "result")
		};
		let err = convert_message_to_octolib(&missing_id)
			.expect_err("tool message without tool_call_id must fail");
		match err {
			octolib::MessageError::MissingToolField { field } => {
				assert_eq!(field, "tool_call_id")
			}
			other => panic!("expected MissingToolField, got {other:?}"),
		}

		let missing_name = Message {
			tool_call_id: Some("call_1".to_string()),
			..msg("tool", "result")
		};
		let err = convert_message_to_octolib(&missing_name)
			.expect_err("tool message without name must fail");
		match err {
			octolib::MessageError::MissingToolField { field } => {
				assert_eq!(field, "name")
			}
			other => panic!("expected MissingToolField, got {other:?}"),
		}
	}

	#[test]
	fn test_convert_complete_tool_message() {
		let message = Message {
			tool_call_id: Some("call_1".to_string()),
			name: Some("tool_a".to_string()),
			..msg("tool", "result")
		};
		let converted =
			convert_message_to_octolib(&message).expect("complete tool message must convert");
		assert_eq!(converted.role, "tool");
		assert_eq!(converted.tool_call_id.as_deref(), Some("call_1"));
		assert_eq!(converted.name.as_deref(), Some("tool_a"));
	}

	#[test]
	fn test_convert_rejects_invalid_role() {
		let err =
			convert_message_to_octolib(&msg("bogus", "x")).expect_err("unknown role must fail");
		match err {
			octolib::MessageError::InvalidRole { role } => assert_eq!(role, "bogus"),
			other => panic!("expected InvalidRole, got {other:?}"),
		}
	}

	#[test]
	fn test_convert_assistant_tool_calls_become_generic() {
		let message = Message {
			tool_calls: Some(serde_json::json!([
				{
					"id": "call_9",
					"type": "function",
					"function": {
					"name": "shell",
					"arguments": "{\"cmd\": \"ls\"}"
					}
				}
			])),
			..msg("assistant", "")
		};
		let converted =
			convert_message_to_octolib(&message).expect("assistant with tool calls must convert");
		let raw = converted.tool_calls.expect("tool_calls must be set");
		let calls: Vec<octolib::llm::GenericToolCall> = serde_json::from_value(raw)
			.expect("stored tool_calls must be unified GenericToolCall JSON");
		assert_eq!(calls.len(), 1);
		assert_eq!(calls[0].name, "shell");
		assert_eq!(calls[0].arguments, serde_json::json!({"cmd": "ls"}));
	}

	#[test]
	fn test_convert_assistant_malformed_tool_calls_fail() {
		let message = Message {
			tool_calls: Some(serde_json::json!({"bad": 1})),
			..msg("assistant", "")
		};
		assert!(
			convert_message_to_octolib(&message).is_err(),
			"malformed tool_calls must fail the request, not panic"
		);
	}

	#[test]
	fn test_convert_cache_marker_and_ttl() {
		let cached = Message {
			cached: true,
			cache_ttl: Some("5m".to_string()),
			..msg("system", "sysprompt")
		};
		let converted = convert_message_to_octolib(&cached).expect("cached message must convert");
		assert!(converted.cached, "cache marker must survive");
		assert_eq!(converted.cache_ttl.as_deref(), Some("5m"));

		// TTL without the cache marker is ignored — only cached messages carry it.
		let uncached = Message {
			cache_ttl: Some("5m".to_string()),
			..msg("system", "sysprompt")
		};
		let converted =
			convert_message_to_octolib(&uncached).expect("uncached message must convert");
		assert!(!converted.cached);
		assert_eq!(
			converted.cache_ttl, None,
			"TTL must not apply without the marker"
		);
	}

	#[test]
	fn test_convert_message_id_propagates() {
		let message = Message {
			id: Some("resp_123".to_string()),
			..msg("assistant", "hi")
		};
		let converted = convert_message_to_octolib(&message).expect("must convert");
		assert_eq!(converted.id.as_deref(), Some("resp_123"));
	}

	#[test]
	fn test_convert_valid_thinking_json_becomes_block() {
		let message = Message {
			thinking: Some(serde_json::json!({"content": "let me think", "tokens": 7})),
			..msg("assistant", "answer")
		};
		let converted = convert_message_to_octolib(&message).expect("must convert");
		let thinking = converted.thinking.expect("thinking must be set");
		assert_eq!(thinking.content, "let me think");
		assert_eq!(thinking.tokens, 7);
	}

	#[test]
	fn test_convert_invalid_thinking_json_is_dropped_not_fatal() {
		let message = Message {
			thinking: Some(serde_json::json!("not a thinking block")),
			..msg("assistant", "answer")
		};
		let converted = convert_message_to_octolib(&message)
			.expect("invalid thinking must not fail the conversion");
		assert!(
			converted.thinking.is_none(),
			"invalid thinking must be dropped"
		);
	}

	#[test]
	fn test_convert_images_and_videos() {
		let message = Message {
			images: Some(vec![crate::session::image::ImageAttachment {
				data: crate::session::image::ImageData::Base64("aGVsbG8=".to_string()),
				media_type: "image/png".to_string(),
				source_type: crate::session::image::SourceType::File(std::path::PathBuf::from(
					"/tmp/x.png",
				)),
				dimensions: Some((800, 600)),
				size_bytes: Some(1024),
			}]),
			videos: Some(vec![crate::session::video::VideoAttachment {
				data: crate::session::video::VideoData::Url(
					"https://example.test/v.mp4".to_string(),
				),
				media_type: "video/mp4".to_string(),
				source_type: crate::session::video::SourceType::Url,
				dimensions: None,
				size_bytes: None,
				duration_secs: Some(1.5),
			}]),
			..msg("user", "look")
		};
		let converted = convert_message_to_octolib(&message).expect("must convert");

		let images = converted.images.expect("images must convert");
		assert_eq!(images.len(), 1);
		assert_eq!(images[0].media_type, "image/png");
		assert_eq!(images[0].dimensions, Some((800, 600)));
		assert_eq!(images[0].size_bytes, Some(1024));
		assert!(matches!(
			&images[0].data,
			octolib::llm::ImageData::Base64(b) if b == "aGVsbG8="
		));
		assert!(matches!(
			&images[0].source_type,
			octolib::llm::SourceType::File(p) if p == &std::path::PathBuf::from("/tmp/x.png")
		));

		let videos = converted.videos.expect("videos must convert");
		assert_eq!(videos.len(), 1);
		assert_eq!(videos[0].media_type, "video/mp4");
		assert_eq!(videos[0].duration_secs, Some(1.5));
		assert!(matches!(
			&videos[0].data,
			octolib::llm::VideoData::Url(u) if u == "https://example.test/v.mp4"
		));
		assert!(matches!(
			videos[0].source_type,
			octolib::llm::SourceType::Url
		));
	}

	// ── convert_response_from_octolib ──────────────────────────────

	fn octolib_response(
		tool_calls: Option<Vec<octolib::llm::ToolCall>>,
	) -> octolib::llm::ProviderResponse {
		octolib::llm::ProviderResponse {
			content: "done".to_string(),
			thinking: Some(ThinkingBlock::with_tokens("reasoning", 12)),
			exchange: ProviderExchange::new(
				serde_json::json!({"q": 1}),
				serde_json::json!({"a": 2}),
				None,
				"test",
			),
			tool_calls,
			finish_reason: Some("stop".to_string()),
			structured_output: Some(serde_json::json!({"ok": true})),
			id: Some("resp_1".to_string()),
		}
	}

	#[test]
	fn test_convert_response_maps_tool_calls_to_mcp_format() {
		let response = octolib_response(Some(vec![octolib::llm::ToolCall {
			id: "call_1".to_string(),
			name: "read".to_string(),
			arguments: serde_json::json!({"path": "/x"}),
		}]));
		let converted = convert_response_from_octolib(response);
		let calls = converted.tool_calls.expect("tool calls must map");
		assert_eq!(calls.len(), 1);
		assert_eq!(calls[0].tool_name, "read");
		assert_eq!(calls[0].tool_id, "call_1");
		assert_eq!(calls[0].parameters, serde_json::json!({"path": "/x"}));
	}

	#[test]
	fn test_convert_response_passes_fields_through() {
		let converted = convert_response_from_octolib(octolib_response(None));
		assert_eq!(converted.content, "done");
		assert!(converted.tool_calls.is_none());
		assert_eq!(converted.finish_reason.as_deref(), Some("stop"));
		assert_eq!(converted.response_id.as_deref(), Some("resp_1"));
		assert_eq!(
			converted.structured_output,
			Some(serde_json::json!({"ok": true}))
		);
		assert_eq!(converted.thinking.as_ref().expect("thinking").tokens, 12);
		assert_eq!(converted.exchange.provider, "test");
	}

	// ── ChatCompletionParams::to_octolib_params ────────────────────

	fn test_config() -> Config {
		let mut config: Config = toml::from_str(include_str!("../config-templates/default.toml"))
			.expect("parse default config template");
		// Keep the conversion offline: no MCP servers → no tool fetching.
		config.mcp.servers.clear();
		config
	}

	#[tokio::test]
	async fn test_octolib_params_defaults_and_passthrough() {
		let config = test_config();
		let messages = vec![msg("user", "hi")];
		let params = ChatCompletionParams::new(&messages, "test-model", 0.5, 0.9, 7, 1234, &config);
		let octo = params
			.to_octolib_params()
			.await
			.expect("conversion succeeds");

		assert_eq!(octo.model, "test-model");
		assert_eq!(octo.temperature, 0.5);
		assert_eq!(octo.top_p, 0.9);
		assert_eq!(octo.top_k, 7);
		assert_eq!(octo.max_tokens, 1234);
		// Defaults come from the config template: max_retries=1, retry_timeout=30s.
		assert_eq!(octo.max_retries, 1);
		assert_eq!(octo.retry_timeout, std::time::Duration::from_secs(30));
		assert_eq!(
			octo.request_timeout,
			Some(std::time::Duration::from_secs(300))
		);
		assert!(octo.use_long_cache, "long cache is always enabled");
		assert!(octo.tools.is_none(), "no MCP servers → no tools attached");
		assert_eq!(octo.messages.len(), 1);
		assert_eq!(octo.messages[0].timestamp, 1_700_000_000);
	}

	#[tokio::test]
	async fn test_octolib_params_builder_overrides() {
		let config = test_config();
		let messages = vec![msg("user", "hi")];
		let params =
			ChatCompletionParams::new(&messages, "m", 0.1, 1.0, 0, 10, &config).with_max_retries(7);
		let octo = params
			.to_octolib_params()
			.await
			.expect("conversion succeeds");
		assert_eq!(octo.max_retries, 7);
	}

	#[tokio::test]
	async fn test_octolib_params_cached_system_message_gets_one_hour_ttl() {
		let config = test_config();
		let messages = vec![
			Message {
				cached: true,
				..msg("system", "sysprompt")
			},
			msg("user", "hi"),
		];
		let params = ChatCompletionParams::new(&messages, "m", 0.1, 1.0, 0, 10, &config);
		let octo = params
			.to_octolib_params()
			.await
			.expect("conversion succeeds");
		assert_eq!(octo.messages[0].role, "system");
		assert!(octo.messages[0].cached);
		assert_eq!(octo.messages[0].cache_ttl.as_deref(), Some("1h"));
	}

	#[tokio::test]
	async fn test_octolib_params_appends_synthetic_user_after_assistant() {
		let config = test_config();
		let messages = vec![msg("user", "hi"), msg("assistant", "hello")];
		let params = ChatCompletionParams::new(&messages, "m", 0.1, 1.0, 0, 10, &config);
		let octo = params
			.to_octolib_params()
			.await
			.expect("conversion succeeds");
		assert_eq!(
			octo.messages.len(),
			3,
			"synthetic continuation must be appended"
		);
		let last = octo.messages.last().expect("non-empty");
		assert_eq!(last.role, "user");
		assert_eq!(last.content, "Please continue.");
	}

	#[tokio::test]
	async fn test_octolib_params_no_synthetic_user_after_user_message() {
		let config = test_config();
		let messages = vec![
			Message {
				cached: true,
				..msg("system", "sysprompt")
			},
			msg("user", "hi"),
		];
		let params = ChatCompletionParams::new(&messages, "m", 0.1, 1.0, 0, 10, &config);
		let octo = params
			.to_octolib_params()
			.await
			.expect("conversion succeeds");
		assert_eq!(octo.messages.len(), 2, "no synthetic message expected");
		assert_eq!(octo.messages.last().expect("non-empty").role, "user");
	}

	#[tokio::test]
	async fn test_octolib_params_system_only_messages_get_no_synthetic_user() {
		let config = test_config();
		let messages = vec![msg("system", "sysprompt")];
		let params = ChatCompletionParams::new(&messages, "m", 0.1, 1.0, 0, 10, &config);
		let octo = params
			.to_octolib_params()
			.await
			.expect("conversion succeeds");
		assert_eq!(
			octo.messages.len(),
			1,
			"no non-system message → no synthetic append"
		);
	}

	#[tokio::test]
	async fn test_octolib_params_purpose_header_sent_on_every_request() {
		let config = test_config();
		let messages = vec![msg("user", "hi")];

		let params = ChatCompletionParams::new(&messages, "m", 0.1, 1.0, 0, 10, &config);
		let octo = params
			.to_octolib_params()
			.await
			.expect("conversion succeeds");
		assert_eq!(
			octo.extra_headers
				.as_ref()
				.and_then(|h| h.get(MODEL_PURPOSE_HEADER)),
			Some(&"main".to_string()),
			"untagged calls are MAIN traffic"
		);

		let params = ChatCompletionParams::new(&messages, "m", 0.1, 1.0, 0, 10, &config)
			.with_purpose(ModelPurpose::Compression);
		let octo = params
			.to_octolib_params()
			.await
			.expect("conversion succeeds");
		assert_eq!(
			octo.extra_headers
				.as_ref()
				.and_then(|h| h.get(MODEL_PURPOSE_HEADER)),
			Some(&"compression".to_string())
		);
	}

	#[tokio::test]
	async fn test_octolib_params_reasoning_effort_override_beats_config() {
		let config = test_config();
		let messages = vec![msg("user", "hi")];

		let params = ChatCompletionParams::new(&messages, "m", 0.1, 1.0, 0, 10, &config);
		let octo = params
			.to_octolib_params()
			.await
			.expect("conversion succeeds");
		// Config template default is "medium".
		assert_eq!(
			octo.reasoning_effort,
			Some(octolib::llm::ReasoningEffort::Medium)
		);

		let params = ChatCompletionParams::new(&messages, "m", 0.1, 1.0, 0, 10, &config)
			.with_reasoning_effort(crate::config::ReasoningEffortConfig::High);
		let octo = params
			.to_octolib_params()
			.await
			.expect("conversion succeeds");
		assert_eq!(
			octo.reasoning_effort,
			Some(octolib::llm::ReasoningEffort::High)
		);
	}

	#[tokio::test]
	async fn test_octolib_params_schema_becomes_strict_structured_output() {
		let config = test_config();
		let messages = vec![msg("user", "hi")];
		let schema = serde_json::json!({
			"type": "object",
			"properties": {"answer": {"type": "string"}},
			"required": ["answer"]
		});
		let params = ChatCompletionParams::new(&messages, "m", 0.1, 1.0, 0, 10, &config)
			.with_schema(schema.clone());
		let octo = params
			.to_octolib_params()
			.await
			.expect("conversion succeeds");
		let rf = octo
			.response_format
			.expect("schema must set response_format");
		assert!(matches!(rf.format, octolib::llm::OutputFormat::JsonSchema));
		assert!(matches!(rf.mode, octolib::llm::ResponseMode::Strict));
		assert_eq!(rf.schema, Some(schema));
	}

	#[tokio::test]
	async fn test_octolib_params_zero_request_timeout_disables_deadline() {
		let mut config = test_config();
		config.request_timeout_seconds = 0;
		let messages = vec![msg("user", "hi")];
		let params = ChatCompletionParams::new(&messages, "m", 0.1, 1.0, 0, 10, &config);
		let octo = params
			.to_octolib_params()
			.await
			.expect("conversion succeeds");
		assert_eq!(
			octo.request_timeout, None,
			"0 must mean no per-request timeout"
		);
	}

	#[tokio::test]
	async fn test_octolib_params_cancellation_token_attached() {
		let config = test_config();
		let messages = vec![msg("user", "hi")];
		let (_tx, rx) = watch::channel(false);
		let params = ChatCompletionParams::new(&messages, "m", 0.1, 1.0, 0, 10, &config)
			.with_cancellation_token(rx);
		let octo = params
			.to_octolib_params()
			.await
			.expect("conversion succeeds");
		assert!(octo.cancellation_token.is_some(), "token must be forwarded");
	}

	#[tokio::test]
	async fn test_octolib_params_without_tools_keeps_tools_empty() {
		let config = test_config();
		let messages = vec![msg("user", "hi")];
		let params =
			ChatCompletionParams::new(&messages, "m", 0.1, 1.0, 0, 10, &config).without_tools();
		let octo = params
			.to_octolib_params()
			.await
			.expect("conversion succeeds");
		assert!(
			octo.tools.is_none(),
			"text-only calls must not attach tools"
		);
	}
}
