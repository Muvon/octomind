// Context truncation functionality to manage token usage

use crate::session::chat::session::ChatSession;
use crate::config::Config;
use anyhow::Result;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use colored::Colorize;

// Perform automatic context truncation when token limit is approaching
pub async fn check_and_truncate_context(
	chat_session: &mut ChatSession,
	config: &Config,
	_operation_cancelled: Arc<AtomicBool>
) -> Result<()> {
	// Check if auto truncation is enabled in config
	if !config.openrouter.enable_auto_truncation {
		return Ok(());
	}

	// Estimate current token usage
	let current_tokens = crate::session::estimate_message_tokens(&chat_session.session.messages);

	// If we're under the threshold, nothing to do
	if current_tokens < config.openrouter.max_request_tokens_threshold {
		return Ok(());
	}

	// We need to truncate - inform the user with minimal info
	if config.openrouter.debug {
		// Detailed output in debug mode
		println!("{}", format!("\nℹ️  Message history exceeds configured token limit ({} > {})",
			current_tokens, config.openrouter.max_request_tokens_threshold).bright_blue());
		println!("{}", "Automatically truncating older messages to reduce context size.".bright_blue());
	} else {
		// Minimal output when debug is disabled
		println!("{}", "Truncating message history to reduce token usage".bright_blue());
	}

	// Strategy: Keep system message, last 2-3 exchanges, and remove older user/assistant exchanges
	let mut system_message = None;
	let mut recent_messages = Vec::new();

	// First, identify and keep system message
	for msg in &chat_session.session.messages {
		if msg.role == "system" {
			system_message = Some(msg.clone());
			break;
		}
	}

	// Then, collect the most recent messages (last 2-3 exchanges = 4-6 messages)
	if chat_session.session.messages.len() > 2 {
		let keep_count = std::cmp::min(6, chat_session.session.messages.len() - 1); // Keep at most 6 recent messages excluding system
		recent_messages = chat_session.session.messages.iter()
			.rev()
			.take(keep_count)
			.cloned()
			.collect::<Vec<_>>();
		recent_messages.reverse(); // Back to chronological order
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
		content: "[Context truncation: Older conversation history has been summarized to reduce token usage. The conversation continues below with the most recent messages.]".to_string(),
		timestamp: std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.unwrap_or_default()
			.as_secs(),
		cached: false,
		tool_call_id: None,
		name: None,
	};
	truncated_messages.push(summary_msg);

	// Add recent messages
	truncated_messages.extend(recent_messages);

	// Replace session messages with truncated version
	chat_session.session.messages = truncated_messages;

	// Calculate how many tokens we saved and display based on debug mode
	let new_token_count = crate::session::estimate_message_tokens(&chat_session.session.messages);
	let tokens_saved = current_tokens.saturating_sub(new_token_count);

	if config.openrouter.debug {
		println!("{}", format!("Truncation complete: {} tokens removed, new context size: {} tokens.",
			tokens_saved, new_token_count).bright_green());
	} else {
		println!("{}", format!("Reduced context size by {} tokens", tokens_saved).bright_green());
	}

	// Save the session with truncated messages
	chat_session.save()?;

	Ok(())
}
