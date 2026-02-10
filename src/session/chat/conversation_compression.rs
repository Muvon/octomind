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
		log_debug!("Adaptive compression disabled (adaptive_threshold=false)");
		return (false, 2.0);
	}

	// Check if we have any pressure levels configured
	if config.compression.pressure_levels.is_empty() {
		log_debug!("No pressure levels configured - compression disabled");
		return (false, 2.0);
	}

	// UNIFIED TOKEN CALCULATION - Use the single source of truth
	// This ensures consistency with display, continuation, and all other systems
	let current_tokens = session.get_full_context_tokens(config).await;

	log_debug!(
		"Compression check: current_tokens={}, thresholds={:?}",
		current_tokens,
		config
			.compression
			.pressure_levels
			.iter()
			.map(|l| l.threshold)
			.collect::<Vec<_>>()
	);

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
				"✓ Threshold exceeded! Context tokens: {} → target compression: {:.1}x (threshold: {})",
				current_tokens,
				level.target_ratio,
				level.threshold
			);

			// CACHE-AWARE DECISION: Calculate if compression is profitable
			let net_benefit = calculate_compression_net_benefit(
				session,
				config,
				current_tokens,
				level.target_ratio,
			)
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
			log_debug!(
				"No threshold exceeded (current: {}, lowest threshold: {})",
				current_tokens,
				config
					.compression
					.pressure_levels
					.first()
					.map(|l| l.threshold)
					.unwrap_or(0)
			);
			(false, 2.0)
		}
	}
}

/// Calculate net benefit of compression using realistic cost analysis with REAL pricing
///
/// CRITICAL INSIGHT: Each API call pays for the ENTIRE context (base + all accumulated new tokens)
/// New tokens added in call N become part of the base for calls N+1, N+2, etc.
/// This cumulative effect makes compression MUCH more valuable!
///
/// Returns positive value if compression saves money, negative if it costs money
async fn calculate_compression_net_benefit(
	session: &ChatSession,
	config: &crate::config::Config,
	current_tokens: usize,
	compression_ratio: f64,
) -> f64 {
	let total_tokens = current_tokens as f64;
	let estimated_future_turns = estimate_future_turns(session);
	let compressed_tokens = total_tokens / compression_ratio;

	// Get decision model (used for compression) and session model (used for future calls)
	let decision_model = &config.compression.decision.model;
	let session_model = &session.model;

	// Get pricing for both models using provider factory
	let decision_pricing = get_model_pricing(decision_model, config);
	let session_pricing = get_model_pricing(session_model, config);

	// If we can't get pricing, fall back to conservative estimate (don't compress)
	let (decision_pricing, session_pricing) = match (decision_pricing, session_pricing) {
		(Some(d), Some(s)) => (d, s),
		_ => {
			log_debug!(
				"Cannot get pricing for models: decision='{}', session='{}' - skipping compression",
				decision_model,
				session_model
			);
			return -1.0; // Negative = don't compress
		}
	};

	// Calculate average NEW tokens per API call from session history
	// CRITICAL: Use OUTPUT tokens only - they represent true incremental growth
	// input_tokens includes cold-start (first call with no cache) which inflates average
	// output_tokens = pure new content added per call (steady-state growth rate)
	let total_api_calls = session.session.info.total_api_calls.max(1) as f64;
	let avg_new_tokens_per_call =
		(session.session.info.output_tokens as f64 / total_api_calls).max(2000.0);

	// CRITICAL FIX: Estimate decision prompt size (the NEW content for compression API call)
	// This is the only uncached part when using same model
	let decision_prompt_tokens = estimate_tokens(
		"Analyze the conversation history. Should older exchanges be compressed into a summary to save context space while preserving important information? Consider:\n\
		- Are there repetitive or resolved topics that can be summarized?\n\
		- Is there important context that must be preserved?\n\
		- Would compression help focus on current topics?\n\n\
		If YES, also provide a 2-3 sentence summary preserving logical structure (focus on what's needed to continue the conversation):\n\n\
		[context chunks placeholder - ~500 tokens average]\n\n\
		Respond with:\n\
		'YES' followed by the summary on the next line, OR\n\
		'NO' if compression is not beneficial."
	) as f64;

	// Check if decision model can reuse session cache
	let same_model = decision_model == session_model;

	// Estimate actual output tokens from compression API call
	// The AI generates a summary (not the full compressed_tokens size)
	// Use compressed_tokens as estimate, but cap at max_tokens if set
	let decision_max_tokens = config.compression.decision.max_tokens;
	let estimated_output_tokens = if decision_max_tokens > 0 {
		(compressed_tokens as u64).min(decision_max_tokens as u64)
	} else {
		compressed_tokens as u64
	};

	// SCENARIO A: NO compression
	// Each call pays for ENTIRE accumulated context (which grows each call)
	let mut total_cost_no_compress = 0.0;
	let mut accumulated_context = total_tokens;

	for i in 0..estimated_future_turns as i32 {
		// Pay for ENTIRE accumulated context using SESSION model pricing
		let context_cost = if i == 0 {
			// First call: everything is already cached from previous session calls
			session_pricing.calculate_cost(0, 0, accumulated_context as u64, 0)
		} else {
			// Subsequent calls: all cached (but GROWING)
			session_pricing.calculate_cost(0, 0, accumulated_context as u64, 0)
		};

		total_cost_no_compress += context_cost;

		// Add new tokens to accumulated context (they'll be paid for in ALL future calls)
		accumulated_context += avg_new_tokens_per_call;
	}

	// SCENARIO B: WITH compression
	// 1. Compression cost (one-time) using DECISION model pricing
	let compression_cost = if same_model {
		// Same model: session context is already cached, only decision prompt is new
		decision_pricing.calculate_cost(
			decision_prompt_tokens as u64, // Only new prompt is uncached
			0,                             // No cache write
			(total_tokens - decision_prompt_tokens) as u64, // Rest is cached
			estimated_output_tokens,       // Actual output tokens (capped by max_tokens)
		)
	} else {
		// Different model: NO cache reuse, everything is uncached
		decision_pricing.calculate_cost(
			total_tokens as u64,     // ALL tokens uncached
			0,                       // No cache write
			0,                       // NO cache
			estimated_output_tokens, // Actual output tokens (capped by max_tokens)
		)
	};

	// 2. Future calls with SMALLER accumulated context using SESSION model pricing
	let mut total_cost_with_compress = compression_cost;
	let mut accumulated_context_compressed = compressed_tokens;

	for i in 0..estimated_future_turns as i32 {
		// Pay for ENTIRE accumulated context (but starting from smaller base)
		let context_cost = if i == 0 {
			// First call after compression: cache write
			session_pricing.calculate_cost(accumulated_context_compressed as u64, 0, 0, 0)
		} else {
			// Subsequent calls: cached (but GROWING from smaller base)
			session_pricing.calculate_cost(0, 0, accumulated_context_compressed as u64, 0)
		};

		total_cost_with_compress += context_cost;

		// Add new tokens to accumulated context (same growth rate as scenario A)
		accumulated_context_compressed += avg_new_tokens_per_call;
	}

	let net_benefit = total_cost_no_compress - total_cost_with_compress;

	log_debug!(
		"Compression analysis (REAL PRICING):\n  \
		Decision model: {} (input: ${:.2}/1M, output: ${:.2}/1M, cache_write: ${:.2}/1M, cache_read: ${:.2}/1M)\n  \
		Session model: {} (input: ${:.2}/1M, output: ${:.2}/1M, cache_write: ${:.2}/1M, cache_read: ${:.2}/1M)\n  \
		Models match: {} (cache reuse: {})\n  \
		Current: {:.0} tokens (decision prompt: ~{:.0} tokens)\n  \
		After compression: {:.0} tokens ({:.1}x ratio) - saves {:.0} tokens\n  \
		Avg new tokens/call: {:.0} (output_tokens={}, api_calls={})\n  \
		Future calls: {:.0}\n  \
		SCENARIO A (no compress): ${:.5}\n    \
		- Pays for growing context: {:.0} → {:.0} tokens over {} calls (all cached)\n  \
		SCENARIO B (compress): ${:.5}\n    \
		- Compression cost: ${:.5} (using {}, {} uncached, {} cached)\n    \
		- Pays for growing context: {:.0} → {:.0} tokens over {} calls\n  \
		Net benefit: ${:.5} → {}",
		decision_model,
		decision_pricing.input_price_per_1m,
		decision_pricing.output_price_per_1m,
		decision_pricing.cache_write_price_per_1m,
		decision_pricing.cache_read_price_per_1m,
		session_model,
		session_pricing.input_price_per_1m,
		session_pricing.output_price_per_1m,
		session_pricing.cache_write_price_per_1m,
		session_pricing.cache_read_price_per_1m,
		if same_model { "YES" } else { "NO" },
		if same_model { "YES" } else { "NO" },
		total_tokens,
		decision_prompt_tokens,
		compressed_tokens,
		compression_ratio,
		total_tokens - compressed_tokens,
		avg_new_tokens_per_call,
		session.session.info.output_tokens,
		session.session.info.total_api_calls,
		estimated_future_turns,
		total_cost_no_compress,
		total_tokens,
		total_tokens + (avg_new_tokens_per_call * (estimated_future_turns - 1.0)),
		estimated_future_turns as i32,
		total_cost_with_compress,
		compression_cost,
		decision_model,
		if same_model { decision_prompt_tokens as u64 } else { total_tokens as u64 },
		if same_model { (total_tokens - decision_prompt_tokens) as u64 } else { 0 },
		compressed_tokens,
		compressed_tokens + (avg_new_tokens_per_call * (estimated_future_turns - 1.0)),
		estimated_future_turns as i32,
		net_benefit,
		if net_benefit > 0.0 {
			"COMPRESS ✓"
		} else {
			"SKIP"
		}
	);

	net_benefit
}

/// Get model pricing from provider
fn get_model_pricing(
	model: &str,
	_config: &crate::config::Config,
) -> Option<crate::providers::ModelPricing> {
	// Parse model string (format: "provider:model")
	let parts: Vec<&str> = model.split(':').collect();
	if parts.len() != 2 {
		log_debug!(
			"Invalid model format: '{}' (expected 'provider:model')",
			model
		);
		return None;
	}

	let provider_name = parts[0];
	let model_name = parts[1];

	// Get provider instance and query pricing
	let provider = crate::providers::ProviderFactory::create_provider(provider_name).ok()?;
	provider.get_model_pricing(model_name)
}

/// Estimate remaining API calls in current session using realistic data-driven calculation
///
/// Uses exponential decay model based on actual session behavior:
/// 1. Sessions naturally slow down as tasks complete (decay factor)
/// 2. Future duration estimated as fraction of elapsed time (not arbitrary minutes)
/// 3. Bounds based on historical data (not arbitrary minimums)
fn estimate_future_turns(session: &ChatSession) -> f64 {
	let current_api_calls = session.session.info.total_api_calls as f64;

	// Bootstrap: Early sessions use conservative baseline
	// Don't over-commit to compression without sufficient data
	if current_api_calls < 5.0 {
		return 10.0; // Conservative: assume 10 more calls
	}

	// Calculate session duration in minutes
	let session_start = session.session.info.created_at;
	let current_time = std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.unwrap_or_default()
		.as_secs();
	let session_duration_secs = (current_time - session_start).max(60); // At least 1 minute
	let session_duration_mins = session_duration_secs as f64 / 60.0;

	// Calculate current API call velocity (calls per minute)
	let call_velocity = current_api_calls / session_duration_mins;

	// REALISTIC ASSUMPTION: Session will continue for a fraction of elapsed time
	// Not arbitrary "30 minutes" - use actual session behavior
	// Longer sessions → assume more remaining (but with diminishing returns)
	let continuation_factor = if session_duration_mins < 10.0 {
		0.8 // Early session: likely 80% more time
	} else if session_duration_mins < 30.0 {
		0.6 // Mid session: likely 60% more time
	} else {
		0.4 // Long session: likely 40% more time (winding down)
	};
	let estimated_remaining_mins = session_duration_mins * continuation_factor;

	// DECAY FACTOR: Sessions slow down over time (fatigue, task completion, context review)
	// High velocity sessions slow down more (burst activity)
	// Low velocity sessions maintain pace (steady work)
	let velocity_decay = if call_velocity > 2.0 {
		0.6 // High velocity: expect 40% slowdown
	} else if call_velocity > 1.0 {
		0.75 // Medium velocity: expect 25% slowdown
	} else {
		0.85 // Low velocity: expect 15% slowdown (already steady)
	};

	// Calculate future calls with realistic decay
	let estimated_remaining = call_velocity * estimated_remaining_mins * velocity_decay;

	// Apply data-driven bounds with context awareness
	// Check for active plan or high tool usage patterns
	let tool_density = session.session.info.tool_calls as f64 / current_api_calls.max(1.0);
	let has_plan = crate::mcp::dev::plan::core::has_active_plan();

	// Adaptive bounds based on session patterns:
	// - Normal sessions: 2x current calls, max 100 (conservative)
	// - Active plan or high tool density (>3.0): 3x current calls, max 200 (realistic for workflows)
	let max_estimate = if has_plan || tool_density > 3.0 {
		(current_api_calls * 3.0).min(200.0) // High activity workflows
	} else {
		(current_api_calls * 2.0).min(100.0) // Normal sessions
	};

	// Minimum: 5 calls (compression needs some future benefit)
	let final_estimate = estimated_remaining.clamp(5.0, max_estimate);

	crate::log_debug!(
		"Future calls estimation: current_calls={:.0}, velocity={:.2} calls/min, \
		session_duration={:.1}min, continuation_factor={:.2}, \
		estimated_remaining_mins={:.1}, velocity_decay={:.2}, \
		tool_density={:.2}, has_plan={}, \
		raw_estimate={:.1}, final_estimate={:.0} (bounds: 5.0-{:.0})",
		current_api_calls,
		call_velocity,
		session_duration_mins,
		continuation_factor,
		estimated_remaining_mins,
		velocity_decay,
		tool_density,
		has_plan,
		estimated_remaining,
		final_estimate,
		max_estimate
	);

	final_estimate
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

	// Use decision model configuration from CompressionDecisionConfig
	let decision_config = &config.compression.decision;

	crate::log_debug!(
		"Using compression decision model '{}' (max_tokens={}, temp={}, session model: '{}')",
		decision_config.model,
		decision_config.max_tokens,
		decision_config.temperature,
		session.model
	);

	// CRITICAL: Pass chat_session for cost tracking
	let params = crate::session::ChatCompletionWithValidationParams::new(
		&messages,
		&decision_config.model,
		decision_config.temperature,
		decision_config.top_p,
		decision_config.top_k,
		decision_config.max_tokens,
		config,
	)
	.with_max_retries(decision_config.max_retries)
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
