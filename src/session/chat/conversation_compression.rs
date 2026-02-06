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

//! Conversation compression - AI-driven automatic compression for normal conversations
//!
//! This module provides automatic compression of older conversation exchanges while preserving
//! recent context. It reuses the plan compression logic but applies it to regular conversations.
//!
//! Key features:
//! - AI decides when compression is beneficial (self-reflection)
//! - Preserves last 4 turns (2 exchanges) uncompressed for context continuity
//! - Reuses existing plan compression infrastructure
//! - Triggered BEFORE user message is added to avoid breaking conversation flow

use crate::config::Config;
use crate::session::chat::session::ChatSession;
use crate::session::estimate_tokens;
use crate::{log_debug, log_info};
use anyhow::Result;

/// Check if we should ask AI about compression
pub fn should_check_compression(session: &ChatSession, config: &Config) -> bool {
	// Feature disabled
	if config.compression.min_conversation_turns == 0 {
		return false;
	}

	// Count conversation turns (user + assistant pairs)
	let turn_count = count_conversation_turns(&session.session.messages);

	// Need minimum turns before AI can decide
	turn_count >= config.compression.min_conversation_turns
}

/// Count conversation turns (each turn = user + assistant message pair)
fn count_conversation_turns(messages: &[crate::session::Message]) -> usize {
	messages
		.iter()
		.filter(|m| m.role == "user" || m.role == "assistant")
		.count()
		/ 2
}

/// Main entry point: check if compression needed and perform if AI decides YES
pub async fn check_and_compress_conversation(
	session: &mut ChatSession,
	config: &Config,
) -> Result<()> {
	if !should_check_compression(session, config) {
		return Ok(());
	}

	log_debug!("Conversation turn threshold reached - asking AI about compression");

	// Ask AI if compression is beneficial
	let should_compress = ask_ai_compression_decision(session, config).await?;

	if !should_compress {
		log_debug!("AI decided compression not beneficial at this point");
		return Ok(());
	}

	log_info!("AI decided to compress older conversation exchanges");

	// Perform compression
	compress_older_conversation(session, config).await?;

	Ok(())
}

/// Ask AI: should we compress older conversation?
async fn ask_ai_compression_decision(session: &ChatSession, config: &Config) -> Result<bool> {
	// Create decision prompt
	let decision_prompt = "Analyze the conversation history. Should older exchanges be compressed into a summary to save context space while preserving important information? Consider:\n\
	- Are there repetitive or resolved topics that can be summarized?\n\
	- Is there important context that must be preserved?\n\
	- Would compression help focus on current topics?\n\n\
	Respond with ONLY 'YES' to compress or 'NO' to keep as-is.";

	// Make lightweight API call for decision
	let messages = vec![crate::session::Message {
		role: "user".to_string(),
		content: decision_prompt.to_string(),
		timestamp: std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.unwrap_or_default()
			.as_secs(),
		cached: false,
		tool_call_id: None,
		name: None,
		tool_calls: None,
		images: None,
		thinking: None,
		id: None,
	}];

	// Use existing chat completion infrastructure
	let params = crate::session::ChatCompletionWithValidationParams::new(
		&messages,
		&session.model,
		session.temperature,
		session.top_p,
		session.top_k,
		512, // Small max_tokens for decision
		config,
	)
	.with_max_retries(1); // Single retry for decision

	let response = crate::session::chat_completion_with_validation(params).await?;

	// Check if response contains YES
	let decision = response.content.to_uppercase().contains("YES");

	log_debug!(
		"AI compression decision: {}",
		if decision { "YES" } else { "NO" }
	);

	Ok(decision)
}

/// Compress older conversation exchanges (reuse plan compression logic)
async fn compress_older_conversation(session: &mut ChatSession, config: &Config) -> Result<()> {
	// Find range to compress (keep last 4 turns raw = 2 exchanges)
	let (start_idx, end_idx) = find_compression_range(&session.session.messages)?;

	if start_idx >= end_idx {
		log_debug!("No messages to compress (range invalid)");
		return Ok(());
	}

	log_debug!(
		"Compressing conversation messages {}-{} (preserving recent context)",
		start_idx,
		end_idx
	);

	// Calculate tokens before compression
	let tokens_before = calculate_range_tokens(session, start_idx, end_idx)?;

	// Generate summary using AI
	let summary = generate_conversation_summary(session, config, start_idx, end_idx).await?;

	// Format compressed entry
	let compressed_entry = format!(
		"## Conversation Summary\n\n\
		{}\n\n\
		---\n\
		*Compressed - Older exchanges have been summarized to optimize context.*",
		summary
	);

	let tokens_after = estimate_tokens(&compressed_entry) as u64;

	// Remove messages in range (reuse plan logic)
	let messages_removed = session.remove_messages_in_range(start_idx, end_idx)?;

	// Insert compressed summary (reuse plan logic)
	session.insert_compressed_knowledge(start_idx, compressed_entry)?;

	// Calculate metrics
	let tokens_saved = tokens_before.saturating_sub(tokens_after);
	let compression_ratio = if tokens_before > 0 {
		tokens_saved as f64 / tokens_before as f64
	} else {
		0.0
	};

	log_info!(
		"✅ Conversation compressed: {} messages → summary, {} tokens saved ({:.1}% reduction)",
		messages_removed,
		tokens_saved,
		compression_ratio * 100.0
	);

	// Track stats
	session
		.session
		.info
		.compression_stats
		.add_conversation_compression(messages_removed, tokens_saved);

	Ok(())
}

/// Find which messages to compress (keep last 4 turns = 2 exchanges raw)
fn find_compression_range(messages: &[crate::session::Message]) -> Result<(usize, usize)> {
	// Find system message index
	let system_idx = messages
		.iter()
		.position(|m| m.role == "system")
		.unwrap_or(0);

	// Collect conversation message indices (user + assistant only)
	let conversation_indices: Vec<usize> = messages
		.iter()
		.enumerate()
		.filter(|(_, m)| m.role == "user" || m.role == "assistant")
		.map(|(idx, _)| idx)
		.collect();

	// Need at least 6 turns to compress (keep 4, compress 2+)
	if conversation_indices.len() <= 4 {
		return Ok((0, 0)); // Not enough to compress
	}

	// Compress everything except last 4 turns
	let preserve_count = 4;
	let compress_count = conversation_indices.len() - preserve_count;

	let start_idx = system_idx + 1; // Start after system message
	let end_idx = conversation_indices[compress_count - 1]; // End before preserved turns

	Ok((start_idx, end_idx))
}

/// Calculate tokens in message range
fn calculate_range_tokens(session: &ChatSession, start_idx: usize, end_idx: usize) -> Result<u64> {
	let mut total_tokens = 0u64;

	// Validate range
	if start_idx >= session.session.messages.len() {
		return Err(anyhow::anyhow!("Invalid start_index in range"));
	}

	if end_idx > session.session.messages.len() {
		return Err(anyhow::anyhow!("Invalid end_index in range"));
	}

	// Count tokens in range (start_idx+1 to end_idx inclusive, matching removal logic)
	for i in (start_idx + 1)..=end_idx {
		if let Some(message) = session.session.messages.get(i) {
			let tokens = estimate_tokens(&message.content) as u64;
			total_tokens += tokens;
		}
	}

	Ok(total_tokens)
}

/// Generate conversation summary using AI
async fn generate_conversation_summary(
	session: &ChatSession,
	config: &Config,
	start_idx: usize,
	end_idx: usize,
) -> Result<String> {
	// Extract messages to summarize
	let messages_to_summarize = &session.session.messages[start_idx..=end_idx];

	// Format messages for summary prompt
	let conversation_text = messages_to_summarize
		.iter()
		.filter(|m| m.role == "user" || m.role == "assistant")
		.map(|m| format!("{}: {}", m.role, m.content))
		.collect::<Vec<_>>()
		.join("\n\n");

	// Create summary prompt
	let summary_prompt = format!(
		"Summarize the following conversation exchanges concisely. Preserve:\n\
		- Key topics and decisions\n\
		- Important context for future conversation\n\
		- Unresolved questions or pending actions\n\
		- Critical information that should not be lost\n\n\
		Provide a concise summary (2-4 sentences):\n\n\
		{}\n\n\
		Summary:",
		conversation_text
	);

	// Make API call for summary
	let messages = vec![crate::session::Message {
		role: "user".to_string(),
		content: summary_prompt,
		timestamp: std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.unwrap_or_default()
			.as_secs(),
		cached: false,
		tool_call_id: None,
		name: None,
		tool_calls: None,
		images: None,
		thinking: None,
		id: None,
	}];

	let params = crate::session::ChatCompletionWithValidationParams::new(
		&messages,
		&session.model,
		session.temperature,
		session.top_p,
		session.top_k,
		1024, // Reasonable max_tokens for summary
		config,
	)
	.with_max_retries(1);

	let response = crate::session::chat_completion_with_validation(params).await?;

	Ok(response.content.trim().to_string())
}
