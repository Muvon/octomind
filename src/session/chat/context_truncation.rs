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

// Context truncation functionality to manage token usage

use crate::config::Config;
use crate::log_conditional;
use crate::session::chat::session::ChatSession;
use anyhow::Result;
use colored::Colorize;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

// Perform smart context truncation when token limit is approaching
pub async fn check_and_truncate_context(
	chat_session: &mut ChatSession,
	config: &Config,
	_role: &str,
	_operation_cancelled: Arc<AtomicBool>,
) -> Result<()> {
	// Check if auto truncation is enabled in config
	if !config.enable_auto_truncation {
		return Ok(());
	}

	// Estimate current token usage
	let current_tokens = crate::session::estimate_message_tokens(&chat_session.session.messages);

	// If we're under the threshold, nothing to do
	if current_tokens < config.max_request_tokens_threshold {
		return Ok(());
	}

	// Delegate to the core truncation logic
	perform_smart_truncation(chat_session, config, current_tokens).await
}

// Perform smart context truncation without checking auto-truncation settings
pub async fn perform_smart_truncation(
	chat_session: &mut ChatSession,
	config: &Config,
	current_tokens: usize,
) -> Result<()> {
	// We need to truncate - inform the user with minimal info
	log_conditional!(
		debug: format!("\nℹ️  Message history exceeds configured token limit ({} > {})\nApplying smart truncation to reduce context size.",
			current_tokens, config.max_request_tokens_threshold).bright_blue(),
		default: "Applying smart truncation to reduce token usage".bright_blue()
	);

	// SMART TRUNCATION STRATEGY:
	// 1. Always keep system message
	// 2. Keep recent conversation with complete tool sequences
	// 3. Prioritize assistant messages that contain important results/summaries
	// 4. Preserve file modification context and technical decisions

	let mut system_message = None;
	let mut preserved_messages = Vec::new();

	// Extract system message
	for msg in &chat_session.session.messages {
		if msg.role == "system" {
			system_message = Some(msg.clone());
			break;
		}
	}

	let non_system_messages: Vec<_> = chat_session
		.session
		.messages
		.iter()
		.filter(|msg| msg.role != "system")
		.collect();

	if !non_system_messages.is_empty() {
		// Calculate how many messages we can keep based on token budget
		let system_tokens = system_message
			.as_ref()
			.map(|msg| crate::session::estimate_tokens(&msg.content))
			.unwrap_or(0);

		let available_tokens = config
			.max_request_tokens_threshold
			.saturating_sub(system_tokens);
		let target_tokens = (available_tokens as f64 * 0.8) as usize; // Leave 20% buffer

		// Work backwards and prioritize important messages
		let mut selected_messages = Vec::new();
		let mut current_token_count = 0usize;

		// Start from the most recent and work backwards
		let mut i = non_system_messages.len();
		let mut incomplete_tool_sequence = false;

		while i > 0 && current_token_count < target_tokens {
			i -= 1;
			let msg = non_system_messages[i];
			let msg_tokens = crate::session::estimate_tokens(&msg.content);

			// Check if adding this message would exceed our budget
			if current_token_count + msg_tokens > target_tokens && !selected_messages.is_empty() {
				break;
			}

			// Tool sequence preservation logic
			match msg.role.as_str() {
				"tool" => {
					// Always include tool messages to avoid breaking sequences
					selected_messages.push(msg.clone());
					current_token_count += msg_tokens;
					incomplete_tool_sequence = true;
				}
				"assistant" => {
					// Include assistant message
					selected_messages.push(msg.clone());
					current_token_count += msg_tokens;

					// If this assistant message has tool_calls, we have a complete sequence
					if msg.tool_calls.is_some() && incomplete_tool_sequence {
						incomplete_tool_sequence = false;
					}
				}
				"user" => {
					// User messages are good natural breakpoints
					// Include if we have a complete tool sequence or no tool sequence
					if !incomplete_tool_sequence {
						selected_messages.push(msg.clone());
						current_token_count += msg_tokens;
					} else {
						// If we have incomplete tool sequence, we need to include this user message too
						// to maintain context, but check token budget
						if current_token_count + msg_tokens <= target_tokens {
							selected_messages.push(msg.clone());
							current_token_count += msg_tokens;
						} else {
							// Can't fit, but we need to break cleanly
							break;
						}
					}
				}
				_ => {
					// Other message types
					if current_token_count + msg_tokens <= target_tokens {
						selected_messages.push(msg.clone());
						current_token_count += msg_tokens;
					}
				}
			}
		}

		// Reverse to get chronological order
		selected_messages.reverse();
		preserved_messages = selected_messages;

		log_conditional!(
			debug: format!("Smart truncation: preserving {} of {} messages ({} tokens)",
				preserved_messages.len(), non_system_messages.len(), current_token_count).bright_blue(),
			default: format!("Preserving {} recent messages", preserved_messages.len()).bright_blue()
		);
	}

	// Build the new truncated message list
	let mut truncated_messages = Vec::new();

	// Add system message first if available
	if let Some(sys_msg) = system_message {
		truncated_messages.push(sys_msg);
	}

	// Add context note only if we actually removed messages
	if preserved_messages.len() < non_system_messages.len() {
		let removed_count = non_system_messages.len() - preserved_messages.len();
		let context_note = format!(
			"[Smart truncation applied: {} older messages removed to optimize token usage. Tool sequences and recent context preserved.]",
			removed_count
		);

		let summary_msg = crate::session::Message {
			role: "assistant".to_string(),
			content: context_note,
			timestamp: std::time::SystemTime::now()
				.duration_since(std::time::UNIX_EPOCH)
				.unwrap_or_default()
				.as_secs(),
			cached: false,
			tool_call_id: None,
			name: None,
			tool_calls: None,
		};
		truncated_messages.push(summary_msg);
	}

	// Add preserved messages
	truncated_messages.extend(preserved_messages);

	// Replace session messages with truncated version
	chat_session.session.messages = truncated_messages;

	// Calculate and report savings
	let new_token_count = crate::session::estimate_message_tokens(&chat_session.session.messages);
	let tokens_saved = current_tokens.saturating_sub(new_token_count);

	log_conditional!(
		debug: format!("Smart truncation complete: {} tokens removed, new context size: {} tokens.",
			tokens_saved, new_token_count).bright_green(),
		default: format!("Reduced context size by {} tokens", tokens_saved).bright_green()
	);

	// Save the session with truncated messages
	chat_session.save()?;

	Ok(())
}
