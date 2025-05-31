// Context truncation functionality to manage token usage

use crate::session::chat::session::ChatSession;
use crate::config::Config;
use crate::log_conditional;
use anyhow::Result;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use colored::Colorize;

// Perform automatic context truncation when token limit is approaching
pub async fn check_and_truncate_context(
	chat_session: &mut ChatSession,
	config: &Config,
	_role: &str,
	_operation_cancelled: Arc<AtomicBool>
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

	// We need to truncate - inform the user with minimal info
	log_conditional!(
		debug: format!("\nℹ️  Message history exceeds configured token limit ({} > {})\nAutomatically truncating older messages to reduce context size.",
			current_tokens, config.max_request_tokens_threshold).bright_blue(),
		default: "Truncating message history to reduce token usage".bright_blue()
	);

	// ENHANCED STRATEGY: Keep system message, find safe truncation point that preserves tool call sequences
	let mut system_message = None;
	let mut recent_messages = Vec::new();

	// First, identify and keep system message
	for msg in &chat_session.session.messages {
		if msg.role == "system" {
			system_message = Some(msg.clone());
			break;
		}
	}

	// Find safe truncation point working backwards from the end
	// We need to preserve complete tool call sequences: assistant(tool_calls) → tool(tool_call_id) → ... → tool(tool_call_id)
	let non_system_messages: Vec<_> = chat_session.session.messages.iter()
		.filter(|msg| msg.role != "system")
		.collect();

	if !non_system_messages.is_empty() {
		// Start from the end and work backwards to find a safe truncation point
		let mut safe_start_index = non_system_messages.len().saturating_sub(1);
		let min_keep = std::cmp::min(4, non_system_messages.len()); // Try to keep at least 4 messages (2 exchanges)

		// Work backwards from the end to find the earliest safe truncation point
		for i in (0..non_system_messages.len()).rev() {
			let msg = non_system_messages[i];

			// Check if this is a safe truncation point
			let is_safe_point = match msg.role.as_str() {
				"user" => {
					// User messages are always safe truncation points
					true
				},
				"assistant" => {
					// Assistant messages are safe ONLY if they don't have tool_calls
					// If they have tool_calls, we must keep all following tool messages
					msg.tool_calls.as_ref().is_none_or(|tc| {
						// Check if it's an empty array or null
						tc.is_null() || (tc.is_array() && tc.as_array().is_none_or(|arr| arr.is_empty()))
					})
				},
				"tool" => {
					// Tool messages are never safe truncation points by themselves
					// We need to check if there are more tool messages following this one
					false
				},
				_ => true, // Other roles (like "system") are generally safe
			};

			if is_safe_point {
				// Found a safe point - now check if we should use it
				let messages_to_keep = non_system_messages.len() - i;

				if messages_to_keep >= min_keep {
					// We have enough messages and found a safe point
					safe_start_index = i;
					break;
				} else if messages_to_keep < min_keep && i > 0 {
					// Not enough messages yet, continue looking backwards
					continue;
				} else {
					// We're at the beginning, use this point regardless
					safe_start_index = i;
					break;
				}
			}
		}

		// Additional validation: make sure we don't start with orphaned tool messages
		while safe_start_index < non_system_messages.len() {
			let start_msg = non_system_messages[safe_start_index];
			if start_msg.role == "tool" {
				// We're starting with a tool message - this could be orphaned
				// Look backwards to see if we can find its assistant message with tool_calls
				let mut found_parent = false;
				for j in (0..safe_start_index).rev() {
					let prev_msg = non_system_messages[j];
					if prev_msg.role == "assistant" && prev_msg.tool_calls.is_some() {
						// Found the parent assistant message, include it
						safe_start_index = j;
						found_parent = true;
						break;
					} else if prev_msg.role == "user" {
						// Hit a user message without finding parent - tool is orphaned
						break;
					}
				}

				if !found_parent {
					// Couldn't find parent, skip this tool message
					safe_start_index += 1;
				} else {
					break;
				}
			} else {
				// Starting with non-tool message is fine
				break;
			}
		}

		// Collect messages from the safe truncation point
		recent_messages = non_system_messages[safe_start_index..].iter().cloned().cloned().collect();

		log_conditional!(
			debug: format!("Smart truncation: keeping {} of {} non-system messages from safe point",
				recent_messages.len(), non_system_messages.len()).bright_blue(),
			default: format!("Preserving {} recent messages", recent_messages.len()).bright_blue()
		);
	}

	// Create a new truncated messages vector
	let mut truncated_messages = Vec::new();

	// Add system message first if available
	if let Some(sys_msg) = system_message {
		truncated_messages.push(sys_msg);
	}

	// Add summary message to provide context
	let summary_msg = crate::session::Message {
		role: "assistant".to_string(),
		content: "[Context truncated: Older conversation history removed to reduce token usage. Tool call sequences preserved to maintain conversation integrity. Recent messages continue below.]".to_string(),
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

	// Add recent messages
	truncated_messages.extend(recent_messages);

	// Replace session messages with truncated version
	chat_session.session.messages = truncated_messages;

	// Calculate how many tokens we saved and display based on debug mode
	let new_token_count = crate::session::estimate_message_tokens(&chat_session.session.messages);
	let tokens_saved = current_tokens.saturating_sub(new_token_count);

	log_conditional!(
		debug: format!("Truncation complete: {} tokens removed, new context size: {} tokens.",
			tokens_saved, new_token_count).bright_green(),
		default: format!("Reduced context size by {} tokens", tokens_saved).bright_green()
	);

	// Save the session with truncated messages
	chat_session.save()?;

	Ok(())
}
