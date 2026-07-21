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

// Chat completion wrappers: validation + provider dispatch

use crate::config::Config;
use crate::providers::{ChatCompletionParams, ProviderFactory, ProviderResponse};
use crate::session::token_counter::{estimate_full_context_tokens, estimate_session_tokens};
use crate::session::Message;
use anyhow::Result;
use tokio::sync::watch;

/// Parameters for chat completion with validation.
///
/// Groups all parameters needed for validated chat completion calls.
pub struct ChatCompletionWithValidationParams<'a> {
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
	/// Configuration object
	pub config: &'a Config,
	/// When true, validate against the *full* context window (system prompt +
	/// tool definitions + messages). When false, validate against the message
	/// list alone. Callers with an active session should set this so the
	/// request fails fast before the provider rejects an oversized payload.
	pub full_context_tokens: bool,
	/// Cancellation token for request abortion
	pub cancellation_token: Option<watch::Receiver<bool>>,
	/// Optional JSON schema for structured output
	pub schema: Option<serde_json::Value>,
	/// Optional reasoning effort override (falls back to `config.reasoning_effort`)
	pub reasoning_effort: Option<crate::config::ReasoningEffortConfig>,
	/// Attach MCP tools to the request (default true). Text-only internal
	/// calls (compression, learning extraction) disable this — see
	/// `crate::providers::ChatCompletionParams::tools`.
	pub tools: bool,
	/// Call origin for purpose-based routing (octohub `auto`). Defaults to
	/// Main; supervisor and compression call sites tag themselves.
	pub purpose: crate::providers::ModelPurpose,
}

impl<'a> ChatCompletionWithValidationParams<'a> {
	/// Create new chat completion with validation parameters
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
			max_retries: 0,
			config,
			full_context_tokens: false,
			cancellation_token: None,
			schema: None,
			reasoning_effort: None,
			tools: true,
			purpose: crate::providers::ModelPurpose::default(),
		}
	}

	/// Set maximum retry attempts
	pub fn with_max_retries(mut self, max_retries: u32) -> Self {
		self.max_retries = max_retries;
		self
	}

	/// Enable full-context token validation (system prompt + tools + messages)
	/// instead of message-only counting. Use this when a real session is
	/// driving the call — it catches oversized payloads before they reach
	/// the provider.
	pub fn with_full_context_tokens(mut self, enabled: bool) -> Self {
		self.full_context_tokens = enabled;
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
	pub fn with_purpose(mut self, purpose: crate::providers::ModelPurpose) -> Self {
		self.purpose = purpose;
		self
	}
}

/// Parameters for chat completion with provider
pub struct ChatCompletionProviderParams<'a> {
	pub messages: &'a [Message],
	pub model: &'a str,
	pub temperature: f32,
	pub top_p: f32,
	pub top_k: u32,
	pub max_tokens: u32,
	pub config: &'a Config,
	pub max_retries: u32,
	pub cancellation_token: Option<watch::Receiver<bool>>,
	/// Optional JSON schema for structured output
	pub schema: Option<serde_json::Value>,
}

/// A successful completion that carries nothing actionable — no text, no tool
/// calls, and no structured output. This is a provider fault, not a real
/// end-of-turn, so the caller surfaces it as an error. Extracted + unit-tested
/// so the classification can't silently drift into treating a tool-only or
/// structured-output response as empty (which would error out real turns).
fn is_empty_completion(
	content: &str,
	tool_calls: Option<&Vec<crate::mcp::McpToolCall>>,
	structured_output: Option<&serde_json::Value>,
) -> bool {
	content.trim().is_empty()
		&& tool_calls.is_none_or(|c| c.is_empty())
		&& structured_output.is_none()
}

/// Delay between empty-completion retries. The retry COUNT is the request's own
/// `max_retries` (0 = no retry): an empty completion is an infra flake the
/// provider's transport retries never see, so it reuses the same budget the
/// caller already set for flaky requests rather than inventing its own.
const EMPTY_COMPLETION_RETRY_DELAY_MS: u64 = 500;

/// High-level function to send a chat completion with input validation and context management.
/// Checks input size and returns an error when limits are exceeded.
pub async fn chat_completion_with_validation(
	params: ChatCompletionWithValidationParams<'_>,
) -> Result<ProviderResponse> {
	// Check for cancellation before starting
	if let Some(ref token) = params.cancellation_token {
		if *token.borrow() {
			return Err(anyhow::anyhow!("Request cancelled before validation"));
		}
	}

	// Parse the model string and get the appropriate provider
	let (provider, actual_model) = ProviderFactory::get_provider_for_model(params.model)?;

	// Fail fast if a schema is requested but the model can't enforce structured
	// output. Covers mid-session model swaps that bypass the up-front setup check.
	if params.schema.is_some() {
		ensure_structured_output_support(params.model)?;
	}

	// Get maximum input tokens for this provider/model (actual context window)
	let max_input_tokens = provider.get_max_input_tokens(&actual_model);

	// Calculate EXACTLY what we're about to send to the API using enhanced token counting
	let total_input_tokens = if params.full_context_tokens {
		// Use enhanced token counting that includes system prompt + tools.
		// Skip the tool fetch when tools are disabled — they won't be sent,
		// so counting them would overestimate and reject valid requests.
		let tools = if params.tools {
			crate::mcp::get_available_functions(params.config).await
		} else {
			Vec::new()
		};
		estimate_full_context_tokens(
			params.messages,
			if tools.is_empty() { None } else { Some(&tools) },
		)
	} else {
		// Fallback for cases without chat session - use basic counting
		estimate_session_tokens(params.messages)
	};
	if total_input_tokens > max_input_tokens {
		return Err(anyhow::anyhow!(
			"Input size ({} tokens) exceeds provider limit ({} tokens) for {} {}",
			total_input_tokens,
			max_input_tokens,
			provider.name(),
			actual_model
		));
	}

	// Check for cancellation before API call
	if let Some(ref token) = params.cancellation_token {
		if *token.borrow() {
			return Err(anyhow::anyhow!("Request cancelled before API call"));
		}
	}

	// Input size is acceptable, proceed with API call
	let mut chat_params = ChatCompletionParams::new(
		params.messages,
		&actual_model,
		params.temperature,
		params.top_p,
		params.top_k,
		params.max_tokens,
		params.config,
	)
	.with_max_retries(params.max_retries)
	.with_purpose(params.purpose);

	if !params.tools {
		chat_params = chat_params.without_tools();
	}

	let chat_params = if let Some(schema) = params.schema {
		chat_params.with_schema(schema)
	} else {
		chat_params
	};

	let cancellation_token = params.cancellation_token.clone();
	let chat_params = if let Some(token) = params.cancellation_token {
		chat_params.with_cancellation_token(token)
	} else {
		chat_params
	};

	let chat_params = if let Some(effort) = params.reasoning_effort {
		chat_params.with_reasoning_effort(effort)
	} else {
		chat_params
	};

	// An empty completion — a successful HTTP response with no content, no tool
	// calls, and no structured output — is a provider fault (e.g. some providers
	// return finish_reason=stop with an empty body), so it is treated exactly
	// like a transport error: retried under the request's own `max_retries`
	// budget (0 = no retry), then surfaced as an error. The provider's transport
	// retries never see these because the HTTP call succeeded. Erroring is what
	// keeps the turn honest: returning an empty response would read as a normal
	// end-of-turn, render nothing, and silently strand the user at the prompt.
	let attempts = params.max_retries as usize + 1;
	let mut last_finish_reason = None;
	for attempt in 0..attempts {
		if attempt > 0 {
			if let Some(ref token) = cancellation_token {
				if *token.borrow() {
					return Err(anyhow::anyhow!(
						"Request cancelled during empty-completion retry"
					));
				}
			}
			tokio::time::sleep(std::time::Duration::from_millis(
				EMPTY_COMPLETION_RETRY_DELAY_MS,
			))
			.await;
		}
		let octolib_params = chat_params
			.to_octolib_params()
			.await
			.map_err(|e| anyhow::anyhow!("Failed to convert message parameters: {}", e))?;
		let octolib_response = provider.chat_completion(octolib_params).await?;
		let response = crate::providers::convert_response_from_octolib(octolib_response);
		if !is_empty_completion(
			&response.content,
			response.tool_calls.as_ref(),
			response.structured_output.as_ref(),
		) {
			return Ok(response);
		}
		last_finish_reason = response.finish_reason.clone();
		crate::log_debug!(
			"Provider '{}' returned an empty completion (attempt {}/{})",
			provider.name(),
			attempt + 1,
			attempts
		);
	}
	Err(anyhow::anyhow!(
		"Provider '{}' returned an empty response (finish_reason={:?}, no content or tool calls) for model '{}' after {} attempt(s)",
		provider.name(),
		last_finish_reason,
		actual_model,
		attempts
	))
}

/// High-level function to send a chat completion using the provider abstraction.
/// Handles model parsing and provider selection automatically.
pub async fn chat_completion_with_provider(
	params: ChatCompletionProviderParams<'_>,
) -> Result<ProviderResponse> {
	// Parse the model string and get the appropriate provider
	let (provider, actual_model) = ProviderFactory::get_provider_for_model(params.model)?;

	// Fail fast if a schema is requested but the model can't enforce structured output
	if params.schema.is_some() {
		ensure_structured_output_support(params.model)?;
	}

	let chat_params = ChatCompletionParams::new(
		params.messages,
		&actual_model,
		params.temperature,
		params.top_p,
		params.top_k,
		params.max_tokens,
		params.config,
	)
	.with_max_retries(params.max_retries);

	let chat_params = if let Some(schema) = params.schema {
		chat_params.with_schema(schema)
	} else {
		chat_params
	};

	// Convert to octolib params and call provider
	let octolib_params = chat_params
		.to_octolib_params()
		.await
		.map_err(|e| anyhow::anyhow!("Failed to convert message parameters: {}", e))?;

	let octolib_response = provider.chat_completion(octolib_params).await?;

	// Convert response back to Octomind format
	Ok(crate::providers::convert_response_from_octolib(
		octolib_response,
	))
}

/// Validate that the provider for `model` can enforce structured output (JSON
/// schema). Fails fast with a clear, actionable error otherwise. The single
/// source of truth for the capability gate — used by the CLI `run --schema`
/// path (checked up front in session setup) and both completion entry points.
pub fn ensure_structured_output_support(model: &str) -> Result<()> {
	let (provider, actual_model) = ProviderFactory::get_provider_for_model(model)?;
	if !provider.supports_structured_output(&actual_model) {
		return Err(anyhow::anyhow!(
			"Model '{model}' (provider '{}') does not support structured output — a JSON schema cannot be enforced. Use a structured-output-capable model.",
			provider.name()
		));
	}
	Ok(())
}

/// Load and validate a JSON Schema document from `path` (for `run --schema`).
/// Reads the file, parses it as JSON, and requires a top-level object — the only
/// shape usable for structured output. Finer schema errors surface via the
/// provider's strict-mode validation at request time.
pub fn load_structured_output_schema(path: &str) -> Result<serde_json::Value> {
	let raw = std::fs::read_to_string(path)
		.map_err(|e| anyhow::anyhow!("Failed to read schema file '{path}': {e}"))?;
	let schema: serde_json::Value = serde_json::from_str(&raw)
		.map_err(|e| anyhow::anyhow!("Invalid JSON in schema file '{path}': {e}"))?;
	if !schema.is_object() {
		return Err(anyhow::anyhow!(
			"Schema file '{path}' must contain a JSON object (a JSON Schema document)"
		));
	}
	Ok(schema)
}

#[cfg(test)]
mod empty_completion_tests {
	use super::is_empty_completion;
	use crate::mcp::McpToolCall;
	use serde_json::json;

	fn call() -> McpToolCall {
		McpToolCall {
			tool_name: "view".into(),
			parameters: json!({}),
			tool_id: "t1".into(),
		}
	}

	#[test]
	fn blank_content_no_tools_no_schema_is_empty() {
		assert!(is_empty_completion("", None, None));
		assert!(is_empty_completion("   \n\t ", None, None));
	}

	#[test]
	fn empty_tool_vec_is_still_empty() {
		// Some providers hand back `Some([])` rather than `None`.
		assert!(is_empty_completion("", Some(&vec![]), None));
	}

	#[test]
	fn text_response_is_not_empty() {
		assert!(!is_empty_completion("hello", None, None));
	}

	#[test]
	fn tool_only_response_is_not_empty() {
		// Tool calls with no prose is a valid turn — must NOT be flagged as empty.
		assert!(!is_empty_completion("", Some(&vec![call()]), None));
	}

	#[test]
	fn structured_output_is_not_empty() {
		// Structured-output replies carry empty content by design — not empty.
		assert!(!is_empty_completion("", None, Some(&json!({"ok": true}))));
	}
}

#[cfg(test)]
mod tests {
	use super::{ensure_structured_output_support, load_structured_output_schema};
	use std::io::Write;

	#[test]
	fn structured_output_supported_for_openai() {
		// OpenAI reports structured-output support for all of its models.
		assert!(ensure_structured_output_support("openai:gpt-4.1").is_ok());
	}

	#[test]
	fn structured_output_unsupported_for_anthropic() {
		let err = ensure_structured_output_support("anthropic:claude-sonnet-4-6")
			.expect_err("anthropic must be rejected")
			.to_string();
		assert!(
			err.contains("does not support structured output"),
			"unexpected error: {err}"
		);
	}

	#[test]
	fn loads_valid_object_schema() {
		let mut f = tempfile::NamedTempFile::new().unwrap();
		f.write_all(br#"{"type":"object","properties":{"x":{"type":"string"}}}"#)
			.unwrap();
		f.flush().unwrap();
		let schema = load_structured_output_schema(f.path().to_str().unwrap()).unwrap();
		assert_eq!(schema["type"].as_str(), Some("object"));
	}

	#[test]
	fn rejects_non_object_schema() {
		let mut f = tempfile::NamedTempFile::new().unwrap();
		f.write_all(b"[1, 2, 3]").unwrap();
		f.flush().unwrap();
		let err = load_structured_output_schema(f.path().to_str().unwrap())
			.expect_err("array must be rejected")
			.to_string();
		assert!(
			err.contains("must contain a JSON object"),
			"unexpected error: {err}"
		);
	}

	#[test]
	fn rejects_invalid_json() {
		let mut f = tempfile::NamedTempFile::new().unwrap();
		f.write_all(b"{not valid json").unwrap();
		f.flush().unwrap();
		let err = load_structured_output_schema(f.path().to_str().unwrap())
			.expect_err("invalid json must be rejected")
			.to_string();
		assert!(err.contains("Invalid JSON"), "unexpected error: {err}");
	}

	#[test]
	fn reports_missing_schema_file() {
		let err = load_structured_output_schema("/nonexistent/path/schema-xyzzy.json")
			.expect_err("missing file must error")
			.to_string();
		assert!(
			err.contains("Failed to read schema file"),
			"unexpected error: {err}"
		);
	}
}
