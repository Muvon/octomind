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
use crate::session::{estimate_full_context_tokens, estimate_tokens};
use crate::{log_debug, log_info};
use anyhow::Result;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Check if we should ask AI about compression
/// Returns (should_compress, target_ratio) tuple
pub fn should_check_compression(session: &ChatSession, config: &Config) -> (bool, f64) {
	// Check if compression is enabled
	if !config.compression.adaptive_threshold {
		return (false, 2.0);
	}

	// CRITICAL FIX: Use full context tokens, not cumulative cache counter
	// session.current_total_tokens tracks INPUT tokens since last cache checkpoint (resets to 0)
	// We need the FULL conversation context size for compression threshold decisions
	let current_tokens = estimate_full_context_tokens(&session.session.messages, None, None);

	// Find target compression ratio based on absolute token count
	let target_ratio = config
		.compression
		.pressure_levels
		.iter()
		.rev() // Start from highest threshold
		.find(|level| current_tokens >= level.threshold)
		.map(|level| level.target_ratio)
		.unwrap_or(2.0);

	let should_compress = current_tokens >= config.compression.pressure_trigger;

	if should_compress {
		log_debug!(
			"Context tokens: {} → target compression: {:.1}x",
			current_tokens,
			target_ratio
		);
	}

	(should_compress, target_ratio)
}

/// Main entry point: check if compression needed and perform if AI decides YES
pub async fn check_and_compress_conversation(
	session: &mut ChatSession,
	config: &Config,
) -> Result<()> {
	let (should_check, target_ratio) = should_check_compression(session, config);
	if !should_check {
		return Ok(());
	}

	// Show animation immediately to avoid perceived lag during decision/summary calls
	let animation_cancel = Arc::new(AtomicBool::new(false));
	let animation_cancel_clone = animation_cancel.clone();
	let current_cost = session.session.info.total_cost;
	let max_threshold = config.max_session_tokens_threshold;
	let current_context_tokens =
		estimate_full_context_tokens(&session.session.messages, None, None) as u64;
	let animation_task = tokio::spawn(async move {
		let _ = crate::session::chat::animation::show_smart_animation(
			animation_cancel_clone,
			current_cost,
			current_context_tokens,
			max_threshold,
		)
		.await;
	});

	// Give animation time to start (avoid race condition)
	tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

	log_debug!("Compression check triggered - asking AI about compression");

	// Ask AI if compression is beneficial (mutable reference for cost tracking)
	let should_compress = ask_ai_compression_decision(session, config).await?;

	if !should_compress {
		log_debug!("AI decided compression not beneficial at this point");
		animation_cancel.store(true, Ordering::SeqCst);
		let _ = animation_task.await;
		return Ok(());
	}

	log_info!("AI decided to compress older conversation exchanges");

	// Perform compression with target ratio (mutable reference for cost tracking)
	compress_older_conversation(session, config, target_ratio).await?;

	animation_cancel.store(true, Ordering::SeqCst);
	let _ = animation_task.await;

	Ok(())
}

/// Ask AI: should we compress older conversation?
/// Uses decision_model if configured for cost savings
async fn ask_ai_compression_decision(session: &mut ChatSession, config: &Config) -> Result<bool> {
	// Create decision prompt
	let decision_prompt = "Analyze the conversation history. Should older exchanges be compressed into a summary to save context space while preserving important information? Consider:\n\
	- Are there repetitive or resolved topics that can be summarized?\n\
	- Is there important context that must be preserved?\n\
	- Would compression help focus on current topics?\n\n\
	Respond with ONLY 'YES' to compress or 'NO' to keep as-is.";

	// CRITICAL FIX: Include conversation history for AI to analyze
	// Clone existing messages and append decision prompt
	let mut messages = session.session.messages.clone();
	messages.push(crate::session::Message {
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
	});

	// Extract values before mutable borrow
	let session_model = session.model.clone();
	let temperature = session.temperature;
	let top_p = session.top_p;
	let top_k = session.top_k;

	// Use decision_model if configured, otherwise fall back to session model
	let model_to_use = config
		.compression
		.decision_model
		.as_ref()
		.unwrap_or(&session_model);

	crate::log_debug!(
		"Using model '{}' for compression decision (session model: '{}')",
		model_to_use,
		session_model
	);

	// CRITICAL: Pass chat_session for cost tracking
	let params = crate::session::ChatCompletionWithValidationParams::new(
		&messages,
		model_to_use,
		temperature,
		top_p,
		top_k,
		512, // Small max_tokens for decision
		config,
	)
	.with_max_retries(1) // Single retry for decision
	.with_chat_session(session); // CRITICAL: Enable cost tracking

	let response = crate::session::chat_completion_with_validation(params).await?;

	// Check if response contains YES
	let decision = response.content.to_uppercase().contains("YES");

	log_debug!(
		"AI compression decision: {} (cost tracked in session)",
		if decision { "YES" } else { "NO" }
	);

	Ok(decision)
}

/// Compress older conversation exchanges using semantic chunking
async fn compress_older_conversation(
	session: &mut ChatSession,
	config: &Config,
	target_ratio: f64,
) -> Result<()> {
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

	// Chunk messages semantically
	let messages_to_compress = &session.session.messages[start_idx..=end_idx];
	let chunks = super::semantic_chunking::chunk_messages(messages_to_compress);

	// Calculate target tokens based on ratio
	let target_tokens = (tokens_before as f64 / target_ratio) as usize;

	// Select top chunks within budget
	let selected = super::semantic_chunking::select_chunks_within_budget(&chunks, target_tokens);

	// Separate by type
	let critical: Vec<_> = selected
		.iter()
		.filter(|c| matches!(c.chunk_type, super::semantic_chunking::ChunkType::Critical))
		.collect();
	let reference: Vec<_> = selected
		.iter()
		.filter(|c| matches!(c.chunk_type, super::semantic_chunking::ChunkType::Reference))
		.collect();
	let context: Vec<_> = selected
		.iter()
		.filter(|c| matches!(c.chunk_type, super::semantic_chunking::ChunkType::Context))
		.collect();

	// Format critical + reference verbatim (no AI summarization)
	let critical_text = format_chunks_verbatim(&critical);
	let reference_text = format_chunks_verbatim(&reference);

	// Combine critical and reference
	let preserved_text = if !critical_text.is_empty() && !reference_text.is_empty() {
		format!("{}\n{}", critical_text, reference_text)
	} else if !critical_text.is_empty() {
		critical_text
	} else {
		reference_text
	};

	// Summarize context chunks using AI
	let context_summary = if !context.is_empty() {
		summarize_context_chunks(session, config, &context).await?
	} else {
		String::new()
	};

	// Format compressed entry
	let compression_id = crate::mcp::dev::plan::compression::get_compression_id()
		.unwrap_or_else(|| "unknown".to_string());

	let compressed_entry =
		format_compressed_entry(&preserved_text, &context_summary, compression_id);

	let tokens_after = estimate_tokens(&compressed_entry) as u64;

	// Remove messages in range
	let messages_removed = session.remove_messages_in_range(start_idx, end_idx)?;

	// Insert compressed summary
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

/// Format chunks verbatim (no summarization)
fn format_chunks_verbatim(chunks: &[&super::semantic_chunking::SemanticChunk]) -> String {
	chunks
		.iter()
		.map(|c| c.content.trim())
		.filter(|s| !s.is_empty())
		.collect::<Vec<_>>()
		.join("\n- ")
}

/// Summarize context chunks using AI
async fn summarize_context_chunks(
	session: &mut ChatSession,
	config: &Config,
	chunks: &[&super::semantic_chunking::SemanticChunk],
) -> Result<String> {
	let context_text = chunks
		.iter()
		.map(|c| c.content.as_str())
		.collect::<Vec<_>>()
		.join("\n\n");

	let prompt = format!(
		"Summarize this context in 2-3 sentences (focus on what's needed to continue the conversation):\n\n{}",
		context_text
	);

	// Make API call for summary
	let messages = vec![crate::session::Message {
		role: "user".to_string(),
		content: prompt,
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

	// Extract values before mutable borrow
	let session_model = session.model.clone();
	let temperature = session.temperature;
	let top_p = session.top_p;
	let top_k = session.top_k;

	// Use decision_model if configured, otherwise fall back to session model
	let model_to_use = config
		.compression
		.decision_model
		.as_ref()
		.unwrap_or(&session_model);

	crate::log_debug!("Using model '{}' for context summarization", model_to_use);

	let params = crate::session::ChatCompletionWithValidationParams::new(
		&messages,
		model_to_use,
		temperature,
		top_p,
		top_k,
		512,
		config,
	)
	.with_max_retries(1)
	.with_chat_session(session);

	let response = crate::session::chat_completion_with_validation(params).await?;
	Ok(response.content.trim().to_string())
}

/// Format final compressed entry
fn format_compressed_entry(preserved: &str, context: &str, compression_id: String) -> String {
	let mut sections = Vec::new();

	if !preserved.is_empty() {
		sections.push(format!(
			"**CRITICAL** (preserved verbatim):\n- {}",
			preserved
		));
	}

	if !context.is_empty() {
		sections.push(format!("**CONTEXT**: {}", context));
	}

	format!(
		"## Conversation Summary [COMPRESSED: {}]\n\n{}\n\n\
		**Compression Info**:\n\
		- ID: `{}`\n\
		- Type: Semantic compression\n\
		---\n\
		*Compressed using importance-based semantic chunking.*",
		compression_id,
		sections.join("\n\n"),
		compression_id
	)
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

	if end_idx >= session.session.messages.len() {
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
