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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Check if we should ask AI about compression
/// Returns (should_compress, target_ratio) tuple
///
/// CACHE-AWARE: Uses amortized cost analysis to determine if compression is profitable
/// considering cache invalidation costs vs. future savings over estimated remaining turns
pub async fn should_check_compression(session: &mut ChatSession, config: &Config) -> (bool, f64) {
	// Check if compression is enabled
	if !config.compression.adaptive_threshold {
		return (false, 2.0);
	}

	// Check if we have any pressure levels configured
	if config.compression.pressure_levels.is_empty() {
		return (false, 2.0);
	}

	// UNIFIED TOKEN CALCULATION - Use the single source of truth
	// This ensures consistency with display, continuation, and all other systems
	let current_tokens = session.get_full_context_tokens(config).await;

	// Find the highest threshold we've exceeded and its target ratio
	// This determines both IF we should compress and HOW MUCH
	let matching_level = config
		.compression
		.pressure_levels
		.iter()
		.rev() // Start from highest threshold
		.find(|level| current_tokens >= level.threshold);

	match matching_level {
		Some(level) => {
			log_debug!(
				"Context tokens: {} → target compression: {:.1}x (threshold: {})",
				current_tokens,
				level.target_ratio,
				level.threshold
			);

			// CACHE-AWARE DECISION: Calculate if compression is profitable
			let net_benefit =
				calculate_compression_net_benefit(session, current_tokens, level.target_ratio)
					.await;

			if net_benefit > 0.0 {
				log_debug!(
					"Cache-aware analysis: Net benefit ${:.5} → COMPRESS",
					net_benefit
				);
				(true, level.target_ratio)
			} else {
				log_debug!(
					"Cache-aware analysis: Net benefit ${:.5} → SKIP (would lose money)",
					net_benefit
				);
				(false, 2.0)
			}
		}
		None => {
			// Haven't reached any threshold yet
			(false, 2.0)
		}
	}
}

/// Calculate net benefit of compression using amortized cost analysis
///
/// This function implements cache-aware compression economics:
/// - Cache write costs 1.25x base (Anthropic 5-min TTL standard)
/// - Cache read costs 0.1x base
/// - Compression invalidates cache, forcing rewrite
/// - But smaller context = lower costs for future turns
///
/// Returns positive value if compression saves money, negative if it costs money
async fn calculate_compression_net_benefit(
	session: &ChatSession,
	current_tokens: usize,
	compression_ratio: f64,
) -> f64 {
	let total_tokens = current_tokens as f64;

	// Estimate remaining turns in this session
	let estimated_future_turns = estimate_future_turns(session);

	// Tokens after compression
	let compressed_tokens = total_tokens / compression_ratio;

	// SCENARIO A: NO compression (assume full cache eventually)
	// All tokens become cached over time, each future turn costs 0.1x
	let cost_per_turn_no_compress = total_tokens * 0.1;
	let total_cost_no_compress = estimated_future_turns * cost_per_turn_no_compress;

	// SCENARIO B: WITH compression
	// 1. Compression API call: input (full context) + output (summary at 5x cost)
	let compression_input_cost = total_tokens * 1.0;
	let compression_output_cost = compressed_tokens * 5.0;
	let compression_api_cost = compression_input_cost + compression_output_cost;

	// 2. Cache write after compression (1.25x)
	let cache_write_cost = compressed_tokens * 1.25;

	// 3. Future turns: cache reads at 0.1x
	let cache_read_cost_per_turn = compressed_tokens * 0.1;
	let future_cache_reads_cost = (estimated_future_turns - 1.0) * cache_read_cost_per_turn;

	let total_cost_with_compress =
		compression_api_cost + cache_write_cost + future_cache_reads_cost;

	// Net benefit (positive = compression saves money)
	let net_benefit = total_cost_no_compress - total_cost_with_compress;

	log_debug!(
		"Cache-aware compression analysis:\n  \
		Current: {:.0} tokens\n  \
		After compression: {:.0} tokens ({:.1}x ratio)\n  \
		Estimated future API calls: {:.0}\n  \
		SCENARIO A (no compress): {:.2} units ({:.0} calls × {:.2} per call)\n  \
		SCENARIO B (compress): {:.2} units\n    \
		- Compression API: {:.2} (input: {:.2} + output: {:.2})\n    \
		- Cache write: {:.2}\n    \
		- Future reads: {:.2} ({:.0} calls × {:.2} per call)\n  \
		Net benefit: {:.2} units → {}",
		total_tokens,
		compressed_tokens,
		compression_ratio,
		estimated_future_turns,
		total_cost_no_compress,
		estimated_future_turns,
		cost_per_turn_no_compress,
		total_cost_with_compress,
		compression_api_cost,
		compression_input_cost,
		compression_output_cost,
		cache_write_cost,
		future_cache_reads_cost,
		estimated_future_turns - 1.0,
		cache_read_cost_per_turn,
		net_benefit,
		if net_benefit > 0.0 {
			"COMPRESS"
		} else {
			"SKIP"
		}
	);

	net_benefit
}

/// Estimate remaining API calls in current session
///
/// CRITICAL: This is NOT about user turns, but about API calls!
/// Each API call = cache write or cache read opportunity
///
/// API calls include:
/// - User messages
/// - Tool execution loops (can be 5-20+ calls per user message)
/// - Thinking tokens
/// - Continuation calls
/// - Layer processing
///
/// Uses adaptive estimation based on current session API call velocity:
/// - Bootstrap: Start with baseline of 10 API calls
/// - Adaptive: As session progresses, estimate based on current velocity
/// - Conservative: Use max(estimated, 10) to ensure compression is worthwhile
fn estimate_future_turns(session: &ChatSession) -> f64 {
	let current_api_calls = session.session.info.total_api_calls as f64;

	// Bootstrap: If we're early in session (< 5 API calls), use baseline
	if current_api_calls < 5.0 {
		return 10.0; // Conservative baseline: assume at least 10 more API calls
	}

	// Adaptive estimation: Assume session will continue at similar pace
	// Formula: remaining = baseline + (current * growth_factor)
	// This adapts: early sessions get conservative estimate, longer sessions get more
	let baseline = 10.0;
	let growth_factor = 0.5; // Assume 50% more API calls than current
	let estimated_remaining = baseline + (current_api_calls * growth_factor);

	// Conservative: At least 10 API calls to ensure compression is worthwhile
	// (Break-even is ~2 cache reads, but we want margin of safety)
	estimated_remaining.max(10.0)
}

/// Main entry point: check if compression needed and perform if AI decides YES
pub async fn check_and_compress_conversation(
	session: &mut ChatSession,
	config: &Config,
) -> Result<()> {
	let (should_check, target_ratio) = should_check_compression(session, config).await;
	if !should_check {
		return Ok(());
	}

	// Show animation immediately to avoid perceived lag during decision/summary call
	let animation_cancel = Arc::new(AtomicBool::new(false));
	let animation_cancel_clone = animation_cancel.clone();
	let current_cost = session.session.info.total_cost;
	let max_threshold = config.max_session_tokens_threshold;

	// UNIFIED TOKEN CALCULATION - Use the single source of truth
	let current_context_tokens = session.get_full_context_tokens(config).await as u64;
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

	log_debug!("Compression check triggered - asking AI for decision and summary in one call");

	// OPTIMIZATION: Do semantic chunking BEFORE AI call (local, no API cost)
	// This allows us to send context chunks to AI in the same call as decision
	let (start_idx, end_idx) = find_compression_range(&session.session.messages)?;

	// end_idx is already safe from find_compression_range

	if start_idx >= end_idx {
		log_debug!("No messages to compress (range invalid)");
		animation_cancel.store(true, Ordering::SeqCst);
		let _ = animation_task.await;
		return Ok(());
	}

	// Calculate tokens before compression
	let tokens_before = calculate_range_tokens(session, start_idx, end_idx)?;

	// Chunk messages semantically (LOCAL - no API call)
	let messages_to_compress = &session.session.messages[start_idx..=end_idx];
	let chunks = super::semantic_chunking::chunk_messages(messages_to_compress);

	// Calculate target tokens based on ratio
	let target_tokens = (tokens_before as f64 / target_ratio) as usize;

	// Select top chunks within budget (LOCAL - no API call)
	let selected = super::semantic_chunking::select_chunks_within_budget(&chunks, target_tokens);

	// Group by type and relation (LOCAL - no API call)
	let (critical_text, reference_text, context_chunks) = group_chunks_by_type(&selected);

	// Combine critical and reference
	let preserved_text = if !critical_text.is_empty() && !reference_text.is_empty() {
		format!("{}\n{}", critical_text, reference_text)
	} else if !critical_text.is_empty() {
		critical_text
	} else {
		reference_text
	};

	// OPTIMIZATION: Single API call for decision + summary (1-hop instead of 2-hop)
	let (should_compress, context_summary) =
		ask_ai_decision_and_summary(session, config, &context_chunks).await?;

	if !should_compress {
		log_debug!("AI decided compression not beneficial at this point");
		animation_cancel.store(true, Ordering::SeqCst);
		let _ = animation_task.await;
		return Ok(());
	}

	log_info!("AI decided to compress older conversation exchanges");

	// Apply compression with the summary we got from AI
	apply_compression(
		session,
		start_idx,
		end_idx,
		&preserved_text,
		&context_summary,
		tokens_before,
	)?;

	animation_cancel.store(true, Ordering::SeqCst);
	let _ = animation_task.await;

	Ok(())
}

/// Ask AI: should we compress AND get summary in ONE call (1-hop optimization)
/// This combines decision + summarization to reduce latency and cost by 50%
async fn ask_ai_decision_and_summary(
	session: &mut ChatSession,
	config: &Config,
	context_chunks: &[&super::semantic_chunking::SemanticChunk],
) -> Result<(bool, String)> {
	// Build prompt that asks for decision + summary in one response
	let mut decision_prompt = String::from(
		"Analyze the conversation history. Should older exchanges be compressed into a summary to save context space while preserving important information? Consider:\n\
		- Are there repetitive or resolved topics that can be summarized?\n\
		- Is there important context that must be preserved?\n\
		- Would compression help focus on current topics?\n\n"
	);

	// If there are context chunks, include them for summarization
	if !context_chunks.is_empty() {
		decision_prompt.push_str(
			"If YES, also provide a 2-3 sentence summary preserving logical structure (focus on what's needed to continue the conversation):\n\n"
		);

		// Add chunks with discourse relation markers for better AI understanding
		for chunk in context_chunks {
			let relation_hint = match chunk.discourse_relation {
				super::semantic_chunking::DiscourseRelation::Cause => "[REASONING]",
				super::semantic_chunking::DiscourseRelation::Contrast => "[ALTERNATIVE]",
				super::semantic_chunking::DiscourseRelation::Sequence => "[STEP]",
				super::semantic_chunking::DiscourseRelation::Background => "[CONTEXT]",
				super::semantic_chunking::DiscourseRelation::Elaboration => "[DETAIL]",
				super::semantic_chunking::DiscourseRelation::None => "",
			};

			if relation_hint.is_empty() {
				decision_prompt.push_str(&format!("- {}\n", chunk.content.trim()));
			} else {
				decision_prompt.push_str(&format!("{} {}\n", relation_hint, chunk.content.trim()));
			}
		}

		decision_prompt.push_str(
			"\n\nRespond with:\n\
			'YES' followed by the summary on the next line, OR\n\
			'NO' if compression is not beneficial.\n\n\
			Example format:\n\
			YES\n\
			[Your 2-3 sentence summary here]",
		);
	} else {
		decision_prompt.push_str("Respond with ONLY 'YES' to compress or 'NO' to keep as-is.");
	}

	// CRITICAL FIX: Include conversation history for AI to analyze
	let mut messages = session.session.messages.clone();
	messages.push(crate::session::Message {
		role: "user".to_string(),
		content: decision_prompt,
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
		"Using model '{}' for 1-hop compression decision+summary (session model: '{}')",
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
		1024, // Increased from 512 to allow for summary text
		config,
	)
	.with_max_retries(1)
	.with_chat_session(session);

	let response = crate::session::chat_completion_with_validation(params).await?;

	// Parse response: check if it starts with YES and extract summary
	let content = response.content.trim();
	let lines: Vec<&str> = content.lines().collect();

	if lines.is_empty() {
		log_debug!("AI compression decision: NO (empty response)");
		return Ok((false, String::new()));
	}

	let first_line = lines[0].trim().to_uppercase();
	let decision = first_line.contains("YES");

	if decision {
		// Extract summary from lines after "YES"
		let summary = if lines.len() > 1 {
			lines[1..].join("\n").trim().to_string()
		} else {
			String::new()
		};

		log_debug!(
			"AI compression decision: YES with summary ({} chars, cost tracked in session)",
			summary.len()
		);
		Ok((true, summary))
	} else {
		log_debug!("AI compression decision: NO (cost tracked in session)");
		Ok((false, String::new()))
	}
}

/// Apply compression by replacing message range with compressed summary
fn apply_compression(
	session: &mut ChatSession,
	start_idx: usize,
	end_idx: usize,
	preserved_text: &str,
	context_summary: &str,
	tokens_before: u64,
) -> Result<()> {
	// Format compressed entry
	let compression_id = crate::mcp::dev::plan::compression::get_compression_id()
		.unwrap_or_else(|| "unknown".to_string());

	let compressed_entry = format_compressed_entry(preserved_text, context_summary, compression_id);

	let tokens_after = estimate_tokens(&compressed_entry) as u64;

	// Remove messages in range
	let messages_removed = session.remove_messages_in_range(start_idx, end_idx)?;

	// Insert compressed summary
	session.insert_compressed_knowledge(start_idx, compressed_entry)?;

	// Calculate metrics
	let tokens_saved = tokens_before.saturating_sub(tokens_after);

	let metrics = crate::mcp::dev::plan::compression::CompressionMetrics::new(
		messages_removed,
		tokens_saved,
		tokens_before,
	);

	crate::session::chat::cost_tracker::CostTracker::display_compression_result(
		"Conversation",
		&metrics,
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

/// Group chunks by type and format with discourse relation awareness
/// Returns (critical_text, reference_text, context_chunks_for_ai)
fn group_chunks_by_type(
	selected: &[super::semantic_chunking::SemanticChunk],
) -> (
	String,
	String,
	Vec<&super::semantic_chunking::SemanticChunk>,
) {
	// Critical: Always preserve verbatim
	let critical: Vec<_> = selected
		.iter()
		.filter(|c| matches!(c.chunk_type, super::semantic_chunking::ChunkType::Critical))
		.collect();

	// Reference: Always preserve verbatim
	let reference: Vec<_> = selected
		.iter()
		.filter(|c| matches!(c.chunk_type, super::semantic_chunking::ChunkType::Reference))
		.collect();

	// Context: Pass to AI with relation markers
	let context: Vec<_> = selected
		.iter()
		.filter(|c| matches!(c.chunk_type, super::semantic_chunking::ChunkType::Context))
		.collect();

	let critical_text = format_chunks_verbatim(&critical);
	let reference_text = format_chunks_verbatim(&reference);

	(critical_text, reference_text, context)
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
	let mut end_idx = conversation_indices[compress_count - 1]; // End before preserved turns

	// SAFETY: If end_idx lands on a tool-use boundary, extend through contiguous tool results
	if end_idx < messages.len() {
		let end_msg = &messages[end_idx];
		let ends_on_tool_boundary =
			(end_msg.role == "assistant" && end_msg.tool_calls.is_some()) || end_msg.role == "tool";
		if ends_on_tool_boundary {
			while end_idx + 1 < messages.len() && messages[end_idx + 1].role == "tool" {
				end_idx += 1;
			}
		}
	}

	Ok((start_idx, end_idx))
}

/// Calculate tokens in message range using accurate token counting
/// This now counts ALL message fields: content, tool_calls, thinking, images, etc.
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
	// Use accurate token counting that includes tool_calls, thinking, images, etc.
	for i in (start_idx + 1)..=end_idx {
		if let Some(message) = session.session.messages.get(i) {
			let tokens = crate::session::estimate_message_tokens(message) as u64;
			total_tokens += tokens;
		}
	}

	Ok(total_tokens)
}

#[cfg(test)]
mod tests {
	use super::find_compression_range;
	use crate::session::Message;
	use serde_json::json;

	fn msg(role: &str) -> Message {
		Message {
			role: role.to_string(),
			content: String::new(),
			..Default::default()
		}
	}

	#[test]
	fn extends_range_to_include_tool_results() {
		let mut messages = Vec::new();
		messages.push(msg("system")); // 0
		messages.push(msg("user")); // 1
		let mut assistant = msg("assistant"); // 2
		assistant.tool_calls = Some(json!([
			{"id": "call_123", "type": "function", "function": {"name": "tool1"}}
		]));
		messages.push(assistant);
		let mut tool = msg("tool"); // 3
		tool.tool_call_id = Some("call_123".to_string());
		tool.name = Some("tool1".to_string());
		messages.push(tool);
		messages.push(msg("user")); // 4

		let (start_idx, end_idx) = find_compression_range(&messages).unwrap();

		assert_eq!(start_idx, 1);
		assert_eq!(end_idx, 3);
	}
}


