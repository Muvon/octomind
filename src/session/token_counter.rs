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

// Token counting utilities

use std::sync::OnceLock;
use tiktoken_rs::{cl100k_base, CoreBPE};

// Global tokenizer instance - created once and reused
static TOKENIZER: OnceLock<CoreBPE> = OnceLock::new();

// Get or initialize the global tokenizer instance
fn get_tokenizer() -> &'static CoreBPE {
	TOKENIZER.get_or_init(|| {
		cl100k_base().unwrap_or_else(|_| {
			// Fallback - this shouldn't happen in practice
			panic!("Failed to initialize tokenizer")
		})
	})
}

// Simple token counter that uses tiktoken to estimate token counts
pub fn estimate_tokens(text: &str) -> usize {
	// Use the cached global tokenizer
	let tokenizer = get_tokenizer();
	let tokens = tokenizer.encode_ordinary(text);
	tokens.len()
}

/// Calculate tokens for a single message including ALL fields
///
/// Implements OpenAI's official token counting formula:
/// - Base overhead: 3 tokens per message
/// - role, content, tool_calls, thinking, name, images
///
/// Based on: https://github.com/openai/openai-cookbook/blob/main/examples/How_to_count_tokens_with_tiktoken.ipynb
pub fn estimate_message_tokens(message: &crate::session::Message) -> usize {
	let mut tokens = 0;

	// Per-message overhead (OpenAI formula: 3 tokens for message formatting)
	tokens += 3;

	// Count role tokens
	tokens += estimate_tokens(&message.role);

	// Count content tokens
	if !message.content.is_empty() {
		tokens += estimate_tokens(&message.content);
	}

	// Count tool_calls tokens if present (can be MASSIVE - 500-2000 tokens per call)
	if let Some(tool_calls) = &message.tool_calls {
		if let Ok(json_str) = serde_json::to_string(tool_calls) {
			tokens += estimate_tokens(&json_str);
		}
	}

	// Count thinking tokens if present
	if let Some(thinking) = &message.thinking {
		if let Ok(json_str) = serde_json::to_string(thinking) {
			tokens += estimate_tokens(&json_str);
		}
	}

	// Count name field tokens if present (with +1 overhead per OpenAI formula)
	if let Some(name) = &message.name {
		tokens += estimate_tokens(name);
		tokens += 1;
	}

	// Count image tokens if present
	if let Some(images) = &message.images {
		tokens += images.len() * 85;
	}

	tokens
}

// Estimate tokens for multiple messages
pub fn estimate_session_tokens(messages: &[crate::session::Message]) -> usize {
	let mut total = 0;

	// Count each message
	for msg in messages {
		total += estimate_message_tokens(msg);
	}

	// Add conversation priming overhead (OpenAI formula: +3 for <|start|>assistant<|message|>)
	if !messages.is_empty() {
		total += 3;
	}

	total
}

// Estimate tokens for full context including system prompt and tools
// This provides accurate estimates that match what's actually sent to API providers
pub fn estimate_full_context_tokens(
	messages: &[crate::session::Message],
	system_prompt: Option<&str>,
	tools: Option<&[crate::mcp::McpFunction]>,
) -> usize {
	// Start with session tokens
	let mut total = estimate_session_tokens(messages);

	// Add system prompt tokens if present
	if let Some(prompt) = system_prompt {
		total += estimate_tokens(prompt);
		// Add API formatting overhead for system message
		total += 10;
	}

	// Add tool definition tokens if present
	if let Some(tool_list) = tools {
		for tool in tool_list {
			// Estimate tokens for tool definition JSON
			// Create a simplified representation of the tool for token counting
			let tool_json = serde_json::json!({
				"name": tool.name,
				"description": tool.description,
				"input_schema": tool.parameters
			});
			let tool_str = serde_json::to_string(&tool_json).unwrap_or_default();
			total += estimate_tokens(&tool_str);
		}
		// Add JSON formatting overhead per tool (arrays, brackets, etc.)
		total += tool_list.len() * 5;
		// Add tools array overhead
		total += 10;
	}

	total
}
/// Calculate minimum tokens required for a session with given role and config
/// This includes system prompt + tool definitions + safety margin
pub async fn calculate_minimum_session_tokens(
	config: &crate::config::Config,
	role: &str,
	current_dir: &std::path::Path,
) -> anyhow::Result<usize> {
	// Get system prompt for the role
	let (_, _, _, _, system_prompt) = config.get_role_config(role);
	let system_tokens = estimate_tokens(system_prompt);

	// Get tool definitions tokens
	let tool_tokens = if !config.mcp.servers.is_empty() {
		let tools = crate::mcp::get_available_functions(config).await;
		let mut total = 0;
		for tool in &tools {
			let tool_json = serde_json::json!({
				"name": tool.name,
				"description": tool.description,
				"input_schema": tool.parameters
			});
			let tool_str = serde_json::to_string(&tool_json).unwrap_or_default();
			total += estimate_tokens(&tool_str);
		}
		total + (tools.len() * 5) + 10 // JSON overhead
	} else {
		0
	};

	// Get initial messages tokens (welcome + instructions)
	let initial_messages_tokens = match crate::session::chat::session::get_initial_messages(
		config,
		role,
		current_dir,
	)
	.await
	{
		Ok(messages) => {
			let mut total = 0;
			for message in &messages {
				// Calculate tokens for message content
				total += estimate_tokens(&message.content);
				// Add overhead for message structure (role, timestamp, etc.)
				total += 20; // JSON overhead per message
			}
			total
		}
		Err(_) => {
			// If we can't get initial messages, use conservative estimate
			// Welcome message ~100 tokens + instructions ~200 tokens + overhead
			320
		}
	};

	// Add message array overhead and request structure overhead
	let request_overhead = 50; // JSON structure, message array, etc.

	Ok(system_tokens + tool_tokens + initial_messages_tokens + request_overhead)
}

/// Validate that max_session_tokens_threshold is sufficient for role requirements
pub async fn validate_session_token_threshold(
	config: &crate::config::Config,
	role: &str,
	current_dir: &std::path::Path,
) -> anyhow::Result<()> {
	if config.max_session_tokens_threshold == 0 {
		return Ok(()); // Disabled, no validation needed
	}

	let minimum_tokens = calculate_minimum_session_tokens(config, role, current_dir).await?;
	let threshold = config.max_session_tokens_threshold;

	// Get detailed breakdown for error message
	let (_, _, _, _, system_prompt) = config.get_role_config(role);
	let system_tokens = estimate_tokens(system_prompt);

	// Calculate tool tokens
	let tool_tokens = if !config.mcp.servers.is_empty() {
		let tools = crate::mcp::get_available_functions(config).await;
		let mut total = 0;
		for tool in &tools {
			let tool_json = serde_json::json!({
				"name": tool.name,
				"description": tool.description,
				"input_schema": tool.parameters
			});
			let tool_str = serde_json::to_string(&tool_json).unwrap_or_default();
			total += estimate_tokens(&tool_str);
		}
		total + (tools.len() * 5) + 10
	} else {
		0
	};

	let initial_messages_tokens = minimum_tokens - system_tokens - tool_tokens;

	// Apply 2x safety check
	if minimum_tokens * 2 > threshold {
		return Err(anyhow::anyhow!(
			"max_session_tokens_threshold ({}) is too low for role '{}'
Minimum required: {} tokens (system prompt + tools + initial messages)
Recommended minimum: {} tokens (2x safety margin)

Breakdown:
- System prompt: {} tokens
- Tool definitions: {} tokens
- Initial messages: {} tokens
- Safety margin: 2x multiplier

Please increase max_session_tokens_threshold to at least {}",
			threshold,
			role,
			minimum_tokens,
			minimum_tokens * 2,
			system_tokens,
			tool_tokens,
			initial_messages_tokens,
			minimum_tokens * 2
		));
	}

	// Warn if threshold is close to minimum (less than 3x)
	if minimum_tokens * 3 > threshold {
		crate::log_info!(
			"⚠️  max_session_tokens_threshold ({}) is close to minimum requirements ({} tokens).
Consider increasing for better session continuity.",
			threshold,
			minimum_tokens
		);
	}

	Ok(())
}
