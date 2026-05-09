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
	/// Optional chat session for context management
	pub chat_session: Option<&'a mut crate::session::chat::session::ChatSession>,
	/// Cancellation token for request abortion
	pub cancellation_token: Option<watch::Receiver<bool>>,
	/// Optional JSON schema for structured output
	pub schema: Option<serde_json::Value>,
	/// Optional reasoning effort override (falls back to `config.reasoning_effort`)
	pub reasoning_effort: Option<crate::config::ReasoningEffortConfig>,
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
			chat_session: None,
			cancellation_token: None,
			schema: None,
			reasoning_effort: None,
		}
	}

	/// Set maximum retry attempts
	pub fn with_max_retries(mut self, max_retries: u32) -> Self {
		self.max_retries = max_retries;
		self
	}

	/// Set chat session for context management
	pub fn with_chat_session(
		mut self,
		chat_session: &'a mut crate::session::chat::session::ChatSession,
	) -> Self {
		self.chat_session = Some(chat_session);
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

	// Get maximum input tokens for this provider/model (actual context window)
	let max_input_tokens = provider.get_max_input_tokens(&actual_model);

	// Calculate EXACTLY what we're about to send to the API using enhanced token counting
	let total_input_tokens = if params.chat_session.is_some() {
		// Use enhanced token counting that includes system prompt + tools
		let tools = crate::mcp::get_available_functions(params.config).await;
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

/// High-level function to send a chat completion using the provider abstraction.
/// Handles model parsing and provider selection automatically.
pub async fn chat_completion_with_provider(
	params: ChatCompletionProviderParams<'_>,
) -> Result<ProviderResponse> {
	// Parse the model string and get the appropriate provider
	let (provider, actual_model) = ProviderFactory::get_provider_for_model(params.model)?;

	// Fail fast if schema requested but provider doesn't support structured output
	if params.schema.is_some() && !provider.supports_structured_output(&actual_model) {
		return Err(anyhow::anyhow!(
			"Provider '{}' does not support structured output for model '{}'. Remove --schema or use a compatible provider.",
			provider.name(),
			actual_model
		));
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
