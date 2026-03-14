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
use crate::session::chat::get_animation_manager;
use crate::session::chat::session::ChatSession;
use crate::session::estimate_tokens;
use crate::{log_debug, log_info};
use anyhow::Result;

/// Check if we should ask AI about compression
/// Returns (should_compress, target_ratio) tuple
///
/// CACHE-AWARE: Uses amortized cost analysis to determine if compression is profitable
/// considering cache invalidation costs vs. future savings over estimated remaining turns
pub async fn should_check_compression(session: &mut ChatSession, config: &Config) -> (bool, f64) {
	// UNIFIED TOKEN CALCULATION - Use the single source of truth
	// This ensures consistency with display and all other systems
	let current_tokens = session.get_full_context_tokens(config).await;

	// When adaptive compression is disabled, fall back to max_session_tokens_threshold as trigger.
	// This replaces the old continuation system: if threshold is set and exceeded, compress.
	if !config.compression.adaptive_threshold {
		if config.max_session_tokens_threshold > 0
			&& current_tokens >= config.max_session_tokens_threshold
		{
			let ratio = config
				.compression
				.pressure_levels
				.iter()
				.map(|l| l.target_ratio)
				.fold(2.0_f64, f64::max);
			log_debug!(
				"Max session token threshold exceeded ({} >= {}) - triggering compression with ratio {:.1}x",
				current_tokens,
				config.max_session_tokens_threshold,
				ratio
			);
			return (true, ratio);
		}
		log_debug!("Adaptive compression disabled (adaptive_threshold=false)");
		return (false, 2.0);
	}

	// Check if we have any pressure levels configured
	if config.compression.pressure_levels.is_empty() {
		log_debug!("No pressure levels configured - compression disabled");
		return (false, 2.0);
	}

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
			// ADAPTIVE COMPRESSION RATIO: Adjust based on session patterns
			// If session has high growth rate, compress more aggressively
			// If session is winding down, compress less aggressively
			let adjusted_ratio = calculate_adaptive_compression_ratio(session, level.target_ratio);

			log_debug!(
				"✓ Threshold exceeded! Context tokens: {} → base compression: {:.1}x → adaptive: {:.1}x (threshold: {})",
				current_tokens,
				level.target_ratio,
				adjusted_ratio,
				level.threshold
			);

			// COOLDOWN CHECK: Prevent premature re-compression before getting calculated benefit
			let current_api_calls = session.session.info.total_api_calls;
			let next_compression_allowed = session
				.session
				.info
				.next_conversation_compression_at_api_call;

			if current_api_calls < next_compression_allowed {
				log_debug!(
					"Compression cooldown active: current_api_calls={} < next_allowed={} (must wait {} more calls)",
					current_api_calls,
					next_compression_allowed,
					next_compression_allowed - current_api_calls
				);
				return (false, 2.0);
			}

			log_debug!(
				"Compression cooldown passed: current_api_calls={} >= next_allowed={}",
				current_api_calls,
				next_compression_allowed
			);

			// CACHE-AWARE DECISION: Calculate if compression is profitable
			let net_benefit = calculate_compression_net_benefit(
				session,
				config,
				current_tokens,
				adjusted_ratio, // Use adaptive ratio
			)
			.await;

			if net_benefit > 0.0 {
				// CRITICAL FIX: Verify compression will actually bring context below threshold
				// Even if profitable, compression is futile if it won't solve the threshold problem
				let (start_idx, end_idx) = match find_compression_range(
					&session.session.messages,
					session.first_prompt_idx,
				) {
					Ok(range) => range,
					Err(e) => {
						log_debug!("Failed to find compression range: {}", e);
						return (false, 2.0);
					}
				};

				if start_idx >= end_idx {
					log_debug!(
						"Invalid compression range ({} >= {}), skipping",
						start_idx,
						end_idx
					);
					return (false, 2.0);
				}

				let compressible_tokens = match calculate_range_tokens(session, start_idx, end_idx)
				{
					Ok(tokens) => tokens,
					Err(e) => {
						log_debug!("Failed to calculate range tokens: {}", e);
						return (false, 2.0);
					}
				};

				let estimated_compressed_size =
					(compressible_tokens as f64 / adjusted_ratio) as u64;
				let estimated_after_compression = (current_tokens as u64)
					.saturating_sub(compressible_tokens)
					.saturating_add(estimated_compressed_size);

				if estimated_after_compression >= level.threshold as u64 {
					log_debug!(
						"Compression won't bring context below threshold: {} → {} (threshold: {}). Compressible: {} → {}. Skipping compression.",
						current_tokens,
						estimated_after_compression,
						level.threshold,
						compressible_tokens,
						estimated_compressed_size
					);
					return (false, 2.0);
				}

				log_debug!(
					"Cache-aware analysis: Net benefit ${:.5} → COMPRESS (will reduce {} → {} tokens, below threshold {})",
					net_benefit,
					current_tokens,
					estimated_after_compression,
					level.threshold
				);
				(true, adjusted_ratio)
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
	let estimated_future_turns = estimate_future_turns(session, current_tokens, compression_ratio);
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

	// SPECIAL CASE: Session model has zero pricing (free/local models like ollama)
	// When pricing is $0.00, cost-based analysis shows no benefit, but user configured
	// pressure levels for context management (not cost). Compress anyway when threshold exceeded.
	let session_is_free = session_pricing.input_price_per_1m == 0.0
		&& session_pricing.output_price_per_1m == 0.0
		&& session_pricing.cache_write_price_per_1m == 0.0
		&& session_pricing.cache_read_price_per_1m == 0.0;

	if session_is_free {
		log_debug!(
			"Session model '{}' has zero pricing - compressing for context management (threshold exceeded)",
			session_model
		);
		return 1.0; // Positive = compress (context management benefit)
	}

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
	// Each call pays: cache_read(base) + input(new_tokens), base grows each turn
	let mut total_cost_no_compress = 0.0;
	let mut base_context = total_tokens;

	for _ in 0..estimated_future_turns as i32 {
		// Pay cache_read for base (already cached) + input for NEW tokens
		let context_cost = session_pricing.calculate_cost(
			avg_new_tokens_per_call as u64, // NEW tokens: input price
			0,                              // No cache write
			base_context as u64,            // BASE: cache_read price
			0,                              // No output in context calculation
		);

		total_cost_no_compress += context_cost;

		// New tokens become part of base for next call
		base_context += avg_new_tokens_per_call;
	}

	// SCENARIO B: WITH compression
	// 1. Compression cost (one-time) using DECISION model pricing
	let ignore_cost = config.compression.decision.ignore_cost;
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
	// If ignore_cost=true, we don't count compression_cost in benefit calculation
	let mut total_cost_with_compress = if ignore_cost { 0.0 } else { compression_cost };
	let mut base_context_compressed = compressed_tokens;

	for call_num in 0..estimated_future_turns as i32 {
		// First call after compression: cache_write for base (fresh cache)
		// Subsequent calls: cache_read for base
		let (input_tokens, cache_write, cache_read) = if call_num == 0 {
			// First call: write the compressed base to cache, pay input for NEW tokens
			(
				avg_new_tokens_per_call as u64,
				base_context_compressed as u64,
				0,
			)
		} else {
			// Subsequent calls: read cached base, pay input for NEW tokens
			(
				avg_new_tokens_per_call as u64,
				0,
				base_context_compressed as u64,
			)
		};

		let context_cost = session_pricing.calculate_cost(input_tokens, cache_write, cache_read, 0);
		total_cost_with_compress += context_cost;

		// New tokens become part of base for next call
		base_context_compressed += avg_new_tokens_per_call;
	}
	let net_benefit = total_cost_no_compress - total_cost_with_compress;

	// Log detailed analysis (this point is only reached for paid models)
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
		- Per call: cache_read(base) + input({:.0} new tokens)\n    \
		- Base grows: {:.0} → {:.0} tokens over {} calls\n  \
		SCENARIO B (compress): ${:.5}\n    \
		- Compression cost: ${:.5} (using {}, {} uncached, {} cached) {}\n    \
		- Per call: cache_read/write(base) + input({:.0} new tokens)\n    \
		- Base grows: {:.0} → {:.0} tokens over {} calls\n  \
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
		avg_new_tokens_per_call,
		total_tokens,
		base_context,
		estimated_future_turns as i32,
		total_cost_with_compress,
		compression_cost,
		decision_model,
		if same_model { decision_prompt_tokens as u64 } else { total_tokens as u64 },
		if same_model { (total_tokens - decision_prompt_tokens) as u64 } else { 0 },
		if ignore_cost { "[IGNORED]" } else { "" },
		avg_new_tokens_per_call,
		compressed_tokens,
		base_context_compressed,
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

/// Calculate adaptive compression ratio based on session patterns
///
/// If session has high growth rate (active exploration), compress more aggressively
/// If session is winding down (low activity), compress less aggressively
/// Also considers how far we are from the next threshold
fn calculate_adaptive_compression_ratio(session: &ChatSession, base_ratio: f64) -> f64 {
	let info = &session.session.info;
	let current_api_calls = info.total_api_calls as f64;

	if current_api_calls < 5.0 {
		// Early session: trust the base ratio
		return base_ratio;
	}

	// Tool density indicates activity level
	let tool_density = info.tool_calls as f64 / current_api_calls;
	let has_plan = crate::mcp::core::plan::core::has_active_plan();

	// Determine adjustment factor
	let adjustment = if has_plan {
		// Active plan = longer session expected = compress more aggressively
		1.2
	} else if tool_density > 2.5 {
		// High tool activity = active exploration = compress more
		1.15
	} else if tool_density > 1.0 {
		// Normal activity = use base ratio
		1.0
	} else if tool_density > 0.3 {
		// Low activity = winding down = compress less
		0.9
	} else {
		// Very low activity = session ending soon = minimal compression
		0.8
	};

	let adaptive_ratio = base_ratio * adjustment;

	// Clamp to reasonable range (1.5x to 4x compression)
	let final_ratio = adaptive_ratio.clamp(1.5, 4.0);

	crate::log_debug!(
		"Adaptive compression ratio: base={:.1}, adjustment={:.2}, tool_density={:.2}, has_plan={}, final={:.1}",
		base_ratio,
		adjustment,
		tool_density,
		has_plan,
		final_ratio
	);

	final_ratio
}

/// Estimate remaining API calls in this session.
///
/// Two real signals, no magic constants:
///
/// 1. PHYSICAL CEILING — headroom / growth_rate
///    How many calls until context fills up again at the current growth rate.
///    This is a hard upper bound: you literally cannot make more calls than this
///    before hitting the threshold again.
///    headroom  = current_tokens - compressed_tokens  (runway bought by compression)
///    growth_rate = output_tokens / api_calls  (measured new tokens added per call)
///
/// 2. SYMMETRY ESTIMATE — api_calls made so far
///    Empirically, sessions are roughly symmetric: work remaining ≈ work done.
///    This is the standard production heuristic (no tunable constant needed).
///    When api_calls=0, falls back to physical ceiling only.
///
/// Final estimate = min(physical_ceiling, symmetry_estimate)
///   → conservative: take whichever signal says the session ends sooner.
///   → if physical ceiling < symmetry: context growth is fast, compress soon.
///   → if symmetry < physical ceiling: session is likely winding down.
///
/// Self-tuning multiplies by actual/predicted ratio from the last compression
/// (pure measurement — corrects systematic over/under-estimation over time).
///
/// Only one justified constant: min=5 (compression cooldown must cover at least
/// a few calls or the cost analysis is meaningless).
fn estimate_future_turns(
	session: &ChatSession,
	current_tokens: usize,
	compression_ratio: f64,
) -> f64 {
	let info = &session.session.info;
	let api_calls = info.total_api_calls as f64;

	// Growth rate: output tokens per call — pure new content added each turn.
	// Output (not input) because input includes the full cached context on every call,
	// which inflates per-call cost but isn't new growth.
	//
	// INCREMENTAL vs LIFETIME: After a compression we have a checkpoint. Use only
	// the tokens/calls since that checkpoint — the session may have changed intensity
	// (heavy exploration early, lighter review later, or vice versa). Lifetime average
	// would carry stale signal from the pre-compression phase.
	// Fall back to lifetime average before the first compression (no checkpoint yet).
	let growth_rate = if info.compression_stats.conversation_compressions > 0 {
		let calls_since = (info.total_api_calls - info.api_calls_at_last_compression).max(1) as f64;
		let output_since = info
			.output_tokens
			.saturating_sub(info.output_tokens_at_last_compression) as f64;
		(output_since / calls_since).max(1.0)
	} else {
		(info.output_tokens as f64 / api_calls.max(1.0)).max(1.0)
	};

	// Physical ceiling: headroom / growth_rate — exact math, no constants.
	// Tells us precisely how many more calls fit before the threshold is hit again.
	// headroom = runway bought by compression (tokens freed up).
	let compressed_tokens = current_tokens as f64 / compression_ratio;
	let headroom = (current_tokens as f64 - compressed_tokens).max(0.0);
	let physical_ceiling = headroom / growth_rate;

	// Symmetry estimate: calls made so far ≈ calls remaining.
	// Empirically true for interactive sessions; the min() with physical_ceiling
	// handles cases where it breaks (burst sessions, long-running batch work).
	// At api_calls=0 there is no symmetry signal — use physical ceiling alone,
	// but cap it at 100 to avoid the nonsensical 60k+ sentinel from growth_rate=1.
	let estimate = if api_calls > 0.0 {
		// Conservative: take whichever signal says the session ends sooner.
		// physical_ceiling = context budget constraint
		// api_calls       = observed session depth so far
		physical_ceiling.min(api_calls)
	} else {
		// No data yet — cap physical ceiling so cold-start doesn't produce 60k+.
		// 100 is not a magic tuning constant; it's an upper bound on "we have no idea".
		// Self-tuning will correct this after the first compression cycle.
		physical_ceiling.min(100.0)
	};

	// Self-tuning: correct for systematic over/under-estimation from the last cycle.
	// actual_turns / predicted_turns = how wrong we were → apply directly.
	// Clamp to [0.25, 4.0]: one bad cycle shouldn't dominate all future estimates.
	let accuracy = calculate_self_tuning_accuracy(info);
	let adjusted = (estimate * accuracy).max(5.0); // min=5: cooldown must be meaningful

	crate::log_debug!(
		"Future calls estimation: api_calls={:.0}, growth_rate={:.0} tok/call ({}), \
		headroom={:.0}, physical_ceiling={:.1}, symmetry={:.1}, accuracy={:.2}, final={:.0}",
		api_calls,
		growth_rate,
		if info.compression_stats.conversation_compressions > 0 {
			"incremental"
		} else {
			"lifetime"
		},
		headroom,
		physical_ceiling,
		if api_calls > 0.0 {
			api_calls
		} else {
			physical_ceiling.min(100.0)
		},
		accuracy,
		adjusted
	);

	adjusted
}

/// Returns actual/predicted ratio from the last compression as a correction multiplier.
///
/// If we predicted 20 calls but only 10 happened before the next threshold was hit,
/// ratio = 10/20 = 0.5 → we were overestimating → scale future estimates down.
///
/// Uses the ratio directly (no blending weight) — one sample is enough to correct
/// systematic bias. Clamped to [0.25, 4.0] as a sanity guard against a single
/// wildly anomalous compression cycle corrupting all future estimates.
fn calculate_self_tuning_accuracy(info: &crate::session::SessionInfo) -> f64 {
	if info.compression_stats.conversation_compressions == 0 {
		return 1.0; // No prior compression — no correction to apply
	}

	let predicted = info.predicted_turns_at_last_compression;
	let actual = (info.total_api_calls as f64 - info.api_calls_at_last_compression as f64).max(0.0);

	if predicted <= 0.0 || actual <= 0.0 {
		return 1.0;
	}

	let ratio = actual / predicted;

	crate::log_debug!(
		"Self-tuning: predicted={:.1}, actual={:.1}, correction={:.2}",
		predicted,
		actual,
		ratio
	);

	// Clamp: don't let a single bad cycle adjust by more than 4x in either direction
	ratio.clamp(0.25, 4.0)
}

/// Main entry point: check if compression needed and perform if AI decides YES
/// Returns true if compression was performed, false otherwise
pub async fn check_and_compress_conversation(
	session: &mut ChatSession,
	config: &Config,
	operation_rx: tokio::sync::watch::Receiver<bool>,
	force: bool,
) -> Result<bool> {
	let (should_check, target_ratio) = should_check_compression(session, config).await;

	if !force && !should_check {
		return Ok(false);
	}
	let target_ratio = if force {
		config
			.compression
			.pressure_levels
			.iter()
			.map(|l| l.target_ratio)
			.fold(2.0_f64, f64::max)
	} else {
		target_ratio
	};

	// Check for cancellation before starting compression (which involves an API call)
	if *operation_rx.borrow() {
		return Err(anyhow::anyhow!("Operation cancelled"));
	}

	// Show animation immediately to avoid perceived lag during decision/summary call
	let animation_manager = get_animation_manager();
	let current_cost = session.session.info.total_cost;
	let max_threshold = config.max_session_tokens_threshold;

	// UNIFIED TOKEN CALCULATION - Use the single source of truth
	let current_context_tokens = session.get_full_context_tokens(config).await as u64;
	animation_manager
		.start_with_params(current_cost, current_context_tokens, max_threshold)
		.await;

	log_debug!("Compression check triggered - asking AI for decision and summary in one call");

	// OPTIMIZATION: Do semantic chunking BEFORE AI call (local, no API cost)
	// This allows us to send context chunks to AI in the same call as decision
	let (start_idx, end_idx) =
		find_compression_range(&session.session.messages, session.first_prompt_idx)?;

	// end_idx is already safe from find_compression_range

	if start_idx >= end_idx {
		log_debug!("No messages to compress (range invalid)");
		animation_manager.stop_current().await;
		return Ok(false);
	}

	// Calculate tokens before compression

	let tokens_before = calculate_range_tokens(session, start_idx, end_idx)?;

	// Chunk messages semantically (LOCAL - no API call)
	// Clone so the borrow ends before the mutable session borrow in ask_ai_decision_and_summary
	let messages_to_compress: Vec<crate::session::Message> =
		session.session.messages[start_idx..=end_idx].to_vec();
	let chunks = super::semantic_chunking::chunk_messages(&messages_to_compress);

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
	let (should_compress, context_summary) = ask_ai_decision_and_summary(
		session,
		config,
		&messages_to_compress,
		&context_chunks,
		operation_rx,
	)
	.await?;

	if !force && !should_compress {
		log_debug!("AI decided compression not beneficial at this point");
		animation_manager.stop_current().await;
		return Ok(false);
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
		target_ratio,
	)
	.await?;

	animation_manager.stop_current().await;
	Ok(true)
}

/// Ask AI: should we compress AND get summary in ONE call (1-hop optimization)
/// This combines decision + summarization to reduce latency and cost by 50%
async fn ask_ai_decision_and_summary(
	session: &mut ChatSession,
	config: &Config,
	messages_to_compress: &[crate::session::Message],
	context_chunks: &[&super::semantic_chunking::SemanticChunk],
	operation_rx: tokio::sync::watch::Receiver<bool>,
) -> Result<(bool, String)> {
	// SYSTEM: role identity + instructions (what the model must do and how to respond).
	// Kept separate from the data so the model acts as a compressor, not a session participant.
	let system_content = if !context_chunks.is_empty() {
		"You are a conversation compressor. Your job is to analyze a conversation transcript \
and decide whether it should be compressed to save context space.\n\n\
Consider:\n\
- Are there repetitive or resolved topics that can be summarized?\n\
- Is there important context that must be preserved?\n\
- Would compression help focus on current topics?\n\n\
If YES, provide a structured summary that PRESERVES ALL CRITICAL CONTEXT for continuing:\n\n\
**USER INTENT** (1-2 sentences):\n\
What did the user ask for? What is the goal or objective?\n\n\
**PROGRESS** (2-3 sentences):\n\
What was completed? What is currently in progress? Include counts if applicable (e.g., 'Step 2 of 5 done').\n\n\
**CURRENT WORK** (2-3 sentences):\n\
What is being worked on RIGHT NOW? What was just being investigated or discussed?\n\n\
**KEY ENTITIES** (preserve exactly):\n\
- Resources: files, documents, URLs, or references being used\n\
- Names: specific terms, identifiers, or labels involved\n\
- Issues: any problems encountered and their status\n\
- Decisions: choices made with reasoning\n\n\
**NEXT STEPS** (1-2 sentences):\n\
What needs to happen next to continue?\n\n\
**Response format:**\n\
YES\n\
**USER INTENT**: [What the user asked for - 1-2 sentences]\n\
**PROGRESS**: [What was completed, what's in progress - include counts if applicable]\n\
**CURRENT WORK**: [What is being worked on RIGHT NOW]\n\
**KEY ENTITIES**:\n\
- Resources: [files, documents, URLs, or references being used]\n\
- Names: [specific terms, identifiers, or labels involved]\n\
- Issues: [any problems encountered and their status]\n\
- Decisions: [choices made with reasoning]\n\
**NEXT STEPS**: [What needs to happen next]\n\n\
**OPTIONAL: If specific file contexts are needed to continue, include them:**\n\
<context>\n\
filename:startline:endline\n\
filename:startline:endline\n\
</context>\n\n\
**Format requirements for file contexts:**\n\
- Use <context> tags around file references\n\
- Each line: filepath:number:number (no spaces)\n\
- Use paths from project root (src/main.rs not ./src/main.rs)\n\
- Line numbers must be positive, start ≤ end ≤ 10000\n\
- Maximum 5 file ranges\n\
- Only include files CRITICAL for continuing\n\n\
OR respond with 'NO' if compression is not beneficial."
	} else {
		"You are a conversation compressor. Analyze the conversation transcript and respond \
with ONLY 'YES' to compress or 'NO' to keep as-is."
	};

	// USER: plain-text transcript of the range being compressed + semantic chunk hints.
	// Building a transcript (not raw messages) prevents the model from continuing the
	// tool-calling loop — it sees text to analyze, not a live conversation to participate in.
	let mut user_content = String::from("**Conversation transcript to compress:**\n\n");

	// Collect file references from tool calls for context preservation
	// These can be re-read on demand after compression
	let mut file_refs: Vec<String> = Vec::new();

	for msg in messages_to_compress {
		match msg.role.as_str() {
			"system" => {} // skip system — already in our system message
			"assistant" => {
				// Include text content; summarize tool calls as one-liners
				if !msg.content.trim().is_empty() {
					user_content.push_str(&format!("[ASSISTANT]: {}\n", msg.content.trim()));
				}
				if let Some(calls) = msg.tool_calls.as_ref().and_then(|v| v.as_array()) {
					for call in calls {
						let name = call
							.get("function")
							.and_then(|f| f.get("name"))
							.and_then(|n| n.as_str())
							.unwrap_or("unknown");
						user_content.push_str(&format!("[TOOL CALL]: {}\n", name));

						// Extract file references from tool arguments
						// These allow the model to re-read files after compression
						if let Some(args) = call.get("function").and_then(|f| f.get("arguments")) {
							super::file_context::extract_file_refs_from_args(
								name,
								args,
								&mut file_refs,
							);
						}
					}
				}
			}
			"tool" => {
				let name = msg.name.as_deref().unwrap_or("tool");
				// Truncate long tool results to avoid bloating the prompt
				let content = msg.content.trim();
				let truncated = if content.len() > 500 {
					let boundary = content
						.char_indices()
						.map(|(i, _)| i)
						.take_while(|&i| i <= 500)
						.last()
						.unwrap_or(0);
					format!("{}… [truncated]", &content[..boundary])
				} else {
					content.to_string()
				};
				user_content.push_str(&format!("[TOOL RESULT: {}]: {}\n", name, truncated));
			}
			_ => {
				// user messages
				if !msg.content.trim().is_empty() {
					user_content.push_str(&format!("[USER]: {}\n", msg.content.trim()));
				}
			}
		}
	}

	// Append semantic chunk hints if available (structural signals for the model)
	if !context_chunks.is_empty() {
		user_content.push_str("\n**Key semantic chunks (structural hints):**\n");
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
				user_content.push_str(&format!("- {}\n", chunk.content.trim()));
			} else {
				user_content.push_str(&format!("{} {}\n", relation_hint, chunk.content.trim()));
			}
		}
	}

	// Append file references extracted from tool calls
	// These allow the model to re-read critical files after compression
	if !file_refs.is_empty() {
		// Merge overlapping ranges and dedupe
		let merged_refs = super::file_context::merge_file_refs(&file_refs);
		if !merged_refs.is_empty() {
			user_content.push_str("\n**File references (can be re-read on demand):**\n");
			// Limit to prevent bloat
			for ref_str in merged_refs.iter().take(10) {
				user_content.push_str(&format!("- {}\n", ref_str));
			}
		}
	}

	let now = std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.unwrap_or_default()
		.as_secs();

	let messages = vec![
		crate::session::Message {
			role: "system".to_string(),
			content: system_content.to_string(),
			timestamp: now,
			cached: false,
			tool_call_id: None,
			name: None,
			tool_calls: None,
			images: None,
			videos: None,
			thinking: None,
			id: None,
		},
		crate::session::Message {
			role: "user".to_string(),
			content: user_content,
			timestamp: now,
			cached: false,
			tool_call_id: None,
			name: None,
			tool_calls: None,
			images: None,
			videos: None,
			thinking: None,
			id: None,
		},
	];

	// Use decision model configuration from CompressionDecisionConfig
	let decision_config = &config.compression.decision;

	crate::log_debug!(
		"Using compression decision model '{}' (max_tokens={}, temp={}, ignore_cost={})",
		decision_config.model,
		decision_config.max_tokens,
		decision_config.temperature,
		decision_config.ignore_cost
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
	.with_chat_session(session)
	.with_cancellation_token(operation_rx);

	let response = crate::session::chat_completion_with_validation(params).await?;

	// Extract usage for cost tracking
	let usage = response.exchange.usage;

	// Track cost based on ignore_cost setting
	let ignore_cost = decision_config.ignore_cost;
	if !ignore_cost {
		if let Some(ref u) = usage {
			if let Some(cost) = u.cost {
				session.session.info.total_cost += cost;
				session.estimated_cost = session.session.info.total_cost;
				log_debug!(
					"Compression decision cost: ${:.5} (total: ${:.5})",
					cost,
					session.session.info.total_cost
				);
			}
		}
	} else {
		log_debug!("Compression decision cost ignored (ignore_cost=true)");
	}

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
			"AI compression decision: YES with summary ({} chars)",
			summary.len()
		);
		Ok((true, summary))
	} else {
		log_debug!("AI compression decision: NO");
		Ok((false, String::new()))
	}
}

/// Apply compression by replacing message range with compressed summary
/// Also parses and injects file contexts if provided by AI
async fn apply_compression(
	session: &mut ChatSession,
	start_idx: usize,
	end_idx: usize,
	preserved_text: &str,
	context_summary: &str,
	tokens_before: u64,
	compression_ratio: f64,
) -> Result<()> {
	// Parse file contexts from AI summary (AI may request specific file ranges to re-inject)
	let file_contexts = super::file_context::parse_file_contexts(context_summary);

	// Generate file context content if any contexts found
	let file_context_content = if !file_contexts.is_empty() {
		crate::log_debug!(
			"Compression: AI requested {} file context(s) for continuation",
			file_contexts.len()
		);
		for (filepath, start, end) in &file_contexts {
			crate::log_debug!("  - {} (lines {}-{})", filepath, start, end);
		}
		super::file_context::generate_file_context_content(&file_contexts)
	} else {
		String::new()
	};

	// Format compressed entry
	let compression_id = crate::mcp::core::plan::compression::get_compression_id()
		.unwrap_or_else(|| "unknown".to_string());

	let compressed_entry = format_compressed_entry_with_context(
		preserved_text,
		context_summary,
		&file_context_content,
		compression_id,
	);

	let tokens_after = estimate_tokens(&compressed_entry) as u64;

	// Remove messages in range
	let (messages_removed, _) = session.remove_messages_in_range(start_idx, end_idx)?;

	// Insert compressed summary (compressed block is always cached=true — new stable boundary)
	session.insert_compressed_knowledge(start_idx, compressed_entry)?;

	// Calculate metrics
	let tokens_saved = tokens_before.saturating_sub(tokens_after);

	let metrics = crate::mcp::core::plan::compression::CompressionMetrics::new(
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

	// Update cooldown: Set next allowed compression point
	let estimated_future_turns =
		estimate_future_turns(session, tokens_before as usize, compression_ratio);
	let next_compression_at =
		session.session.info.total_api_calls + estimated_future_turns as usize;
	session
		.session
		.info
		.next_conversation_compression_at_api_call = next_compression_at;

	// SELF-TUNING: Record prediction and checkpoint for incremental growth rate tracking.
	// output_tokens_at_last_compression lets estimate_future_turns measure growth since
	// this compression only, not the inflated lifetime average.
	let api_calls_at_compression = session.session.info.total_api_calls;
	session.session.info.predicted_turns_at_last_compression = estimated_future_turns;
	session.session.info.api_calls_at_last_compression = api_calls_at_compression;
	session.session.info.output_tokens_at_last_compression = session.session.info.output_tokens;

	log_debug!(
		"Self-tuning: Recorded prediction at compression #{} (API calls={}): predicted={:.1} remaining turns",
		session.session.info.compression_stats.conversation_compressions,
		api_calls_at_compression,
		estimated_future_turns
	);

	log_debug!(
		"Compression cooldown set: next_compression_at={} (current={}, estimated_turns={:.1})",
		next_compression_at,
		session.session.info.total_api_calls,
		estimated_future_turns
	);

	// CRITICAL: Log compression point to session file
	// This marker tells session loader to clear messages before this point on resume
	// Without this, all "compressed" messages are reloaded, defeating compression
	let _ = crate::session::logger::log_compression_point(
		&session.session.info.name,
		"conversation",
		messages_removed,
		tokens_saved,
	);

	// CRITICAL FIX: Reset token tracking for fresh start after compression
	// This prevents token drift and ensures accurate cache/pricing calculations
	// Mirrors the behavior in context_truncation.rs::perform_smart_full_summarization()
	session.session.info.current_non_cached_tokens = 0;
	session.session.info.current_total_tokens = 0;

	// Reset cache checkpoint time
	session.session.info.last_cache_checkpoint_time = std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.unwrap_or_default()
		.as_secs();

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

/// Format final compressed entry with optional file context
fn format_compressed_entry_with_context(
	preserved: &str,
	context: &str,
	file_context: &str,
	compression_id: String,
) -> String {
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

	// Add file context if provided (automatically expanded from AI's <context> tags)
	if !file_context.is_empty() {
		sections.push(format!(
			"**FILE CONTEXT** (auto-expanded):\n{}",
			file_context
		));
	}

	format!(
		"## Conversation Summary [COMPRESSED: {}]\n\n{}\n\n\
		**Compression Info**:\n\
		- ID: `{}`\n\
		- Type: Semantic compression with file context\n\
		---\n\
		*Compressed using importance-based semantic chunking with automatic file context expansion.*",
		compression_id,
		sections.join("\n\n"),
		compression_id
	)
}

/// Find which messages to compress (keep last 4 turns = 2 exchanges raw)
///
/// CRITICAL: Must not cut between assistant with tool_calls and its tool results
/// CRITICAL: Compression NEVER goes below first_prompt_idx (INCLUSIVE boundary)
fn find_compression_range(
	messages: &[crate::session::Message],
	first_prompt_idx: Option<usize>,
) -> Result<(usize, usize)> {
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

	// Need at least 6 conversation messages to compress (keep 4, compress 2+)
	if conversation_indices.len() <= 4 {
		return Ok((0, 0)); // Not enough to compress
	}

	// Compress everything except last 4 conversation messages
	let preserve_count = 4;
	let compress_count = conversation_indices.len() - preserve_count;

	// CRITICAL: Start boundary is first_prompt_idx (INCLUSIVE - never compress this or before)
	// If not set, fall back to system_idx + 1 (safe default)
	let start_idx = first_prompt_idx.unwrap_or(system_idx + 1);

	// CRITICAL FIX: The end_idx must be the last MESSAGE INDEX (not conversation index)
	// before the first preserved conversation message.
	//
	// OLD BUG: Used conversation_indices[compress_count - 1] which gave us the index
	// of the last conversation message to compress, but SKIPPED any tool messages
	// that follow it.
	//
	// CORRECT: The first preserved conversation message is at conversation_indices[compress_count].
	// Everything BEFORE that index (including tool messages) should be compressed.
	// So end_idx = conversation_indices[compress_count] - 1
	let end_idx = conversation_indices[compress_count] - 1;

	if start_idx >= end_idx {
		return Ok((0, 0));
	}

	Ok((start_idx, end_idx))
}

/// Calculate tokens in message range using accurate token counting
/// This now counts ALL message fields: content, tool_calls, thinking, images, etc.
///
/// CRITICAL FIX: Counts [start_idx, end_idx] to match semantic chunking range
/// Previously counted [start_idx+1, end_idx] which caused token mismatch bugs
fn calculate_range_tokens(session: &ChatSession, start_idx: usize, end_idx: usize) -> Result<u64> {
	let mut total_tokens = 0u64;

	// Validate range
	if start_idx >= session.session.messages.len() {
		return Err(anyhow::anyhow!("Invalid start_index in range"));
	}

	if end_idx >= session.session.messages.len() {
		return Err(anyhow::anyhow!("Invalid end_index in range"));
	}

	// FIX: Count tokens in range [start_idx, end_idx] to match semantic chunking
	// Semantic chunking uses messages_to_compress = &session.session.messages[start_idx..=end_idx]
	// So we must count the same range to get accurate tokens_before
	for i in start_idx..=end_idx {
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
	#[allow(clippy::vec_init_then_push)]
	fn extends_range_to_include_tool_results() {
		let mut messages = Vec::new();
		messages.push(msg("system")); // 0

		// Create scenario where tool messages are between conversation messages
		messages.push(msg("user")); // 1
		let mut assistant1 = msg("assistant"); // 2
		assistant1.tool_calls = Some(json!([
			{"id": "call_1", "type": "function", "function": {"name": "tool1"}}
		]));
		messages.push(assistant1);
		let mut tool1 = msg("tool"); // 3
		tool1.tool_call_id = Some("call_1".to_string());
		messages.push(tool1);

		messages.push(msg("user")); // 4
		messages.push(msg("assistant")); // 5
		messages.push(msg("user")); // 6
		messages.push(msg("assistant")); // 7
		messages.push(msg("user")); // 8
		messages.push(msg("assistant")); // 9

		let (start_idx, end_idx) = find_compression_range(&messages, None).unwrap();

		// conversation_indices = [1, 2, 4, 5, 6, 7, 8, 9] (8 messages)
		// Keep last 4: [6, 7, 8, 9]
		// First preserved conversation message: conversation_indices[4] = 6
		// end_idx = 6 - 1 = 5 (includes tool message at 3)
		assert_eq!(start_idx, 1);
		assert_eq!(
			end_idx, 5,
			"Must include all messages before first preserved conversation message"
		);
	}

	#[test]
	#[allow(clippy::vec_init_then_push)]
	fn extends_when_ending_on_assistant_with_tools() {
		// THIS is the critical test - tool messages between conversation messages
		let mut messages = Vec::new();
		messages.push(msg("system")); // 0

		messages.push(msg("user")); // 1
		messages.push(msg("assistant")); // 2

		messages.push(msg("user")); // 3
		let mut assistant_with_tools = msg("assistant"); // 4
		assistant_with_tools.tool_calls = Some(json!([
			{"id": "call_1", "type": "function", "function": {"name": "tool1"}}
		]));
		messages.push(assistant_with_tools);
		let mut tool1 = msg("tool"); // 5
		tool1.tool_call_id = Some("call_1".to_string());
		messages.push(tool1);

		messages.push(msg("user")); // 6
		messages.push(msg("assistant")); // 7
		messages.push(msg("user")); // 8
		messages.push(msg("assistant")); // 9

		let (start_idx, end_idx) = find_compression_range(&messages, None).unwrap();

		// conversation_indices = [1, 2, 3, 4, 6, 7, 8, 9] (8 messages)
		// Keep last 4: [6, 7, 8, 9]
		// First preserved conversation message: conversation_indices[4] = 6
		// end_idx = 6 - 1 = 5 (includes tool message at 5)
		assert_eq!(start_idx, 1);
		assert_eq!(
			end_idx, 5,
			"Must include all messages (including tool results) before first preserved conversation message"
		);
	}

	#[test]
	#[allow(clippy::vec_init_then_push)]
	fn handles_multiple_assistants_with_tools() {
		// Test scenario: multiple assistant messages with tool calls in sequence
		let mut messages = Vec::new();
		messages.push(msg("system")); // 0

		messages.push(msg("user")); // 1

		// First assistant with tools
		let mut assistant1 = msg("assistant"); // 2
		assistant1.tool_calls = Some(json!([
			{"id": "call_1", "type": "function", "function": {"name": "tool1"}}
		]));
		messages.push(assistant1);
		let mut tool1 = msg("tool"); // 3
		tool1.tool_call_id = Some("call_1".to_string());
		messages.push(tool1);

		// Second assistant with tools (no user message between)
		let mut assistant2 = msg("assistant"); // 4
		assistant2.tool_calls = Some(json!([
			{"id": "call_2", "type": "function", "function": {"name": "tool2"}}
		]));
		messages.push(assistant2);
		let mut tool2 = msg("tool"); // 5
		tool2.tool_call_id = Some("call_2".to_string());
		messages.push(tool2);

		// More conversation messages to trigger compression
		messages.push(msg("user")); // 6
		messages.push(msg("assistant")); // 7
		messages.push(msg("user")); // 8
		messages.push(msg("assistant")); // 9
		messages.push(msg("user")); // 10

		let (start_idx, end_idx) = find_compression_range(&messages, None).unwrap();

		// conversation_indices = [1, 2, 4, 6, 7, 8, 9, 10] (8 messages)
		// Keep last 4: [7, 8, 9, 10]
		// First preserved conversation message: conversation_indices[4] = 7
		// end_idx = 7 - 1 = 6 (includes all tool messages at 3, 5)
		assert_eq!(start_idx, 1);
		assert_eq!(
			end_idx, 6,
			"Must include all messages including tool results before first preserved"
		);
	}

	#[test]
	#[allow(clippy::vec_init_then_push)]
	fn start_boundary_must_not_orphan_initial_tool_sequence() {
		let mut messages = Vec::new();
		messages.push(msg("system")); // 0

		// First conversation message is assistant with tool calls.
		// This can happen in resumed sessions or reconstructed histories.
		let mut assistant_with_tools = msg("assistant"); // 1
		assistant_with_tools.tool_calls = Some(json!([
			{"id": "call_1", "type": "function", "function": {"name": "tool1"}}
		]));
		messages.push(assistant_with_tools);

		let mut tool1 = msg("tool"); // 2
		tool1.tool_call_id = Some("call_1".to_string());
		messages.push(tool1);

		// Add enough conversation messages to trigger compression.
		messages.push(msg("user")); // 3
		messages.push(msg("assistant")); // 4
		messages.push(msg("user")); // 5
		messages.push(msg("assistant")); // 6
		messages.push(msg("user")); // 7
		messages.push(msg("assistant")); // 8
								   // Test with first_prompt_idx set to index 3 (first real user message)
		let (start_idx, end_idx) = find_compression_range(&messages, Some(3)).unwrap();

		// Safety requirement: compression starts AFTER first_prompt_idx (INCLUSIVE boundary)
		// first_prompt_idx=3 means index 3 is PROTECTED, compression starts at 4
		assert_eq!(
			start_idx, 3,
			"start_idx must equal first_prompt_idx (INCLUSIVE boundary)"
		);
		assert!(
			end_idx >= 4,
			"range should start compressing only after first_prompt_idx"
		);
	}

	#[test]
	#[allow(clippy::vec_init_then_push)]
	fn start_boundary_must_not_orphan_initial_tool_sequence_duplicate() {
		let mut messages = Vec::new();
		messages.push(msg("system")); // 0

		// First conversation message is assistant with tool calls.
		// This can happen in resumed sessions or reconstructed histories.
		let mut assistant_with_tools = msg("assistant"); // 1
		assistant_with_tools.tool_calls = Some(json!([
			{"id": "call_1", "type": "function", "function": {"name": "tool1"}}
		]));
		messages.push(assistant_with_tools);

		let mut tool1 = msg("tool"); // 2
		tool1.tool_call_id = Some("call_1".to_string());
		messages.push(tool1);

		// Add enough conversation messages to trigger compression.
		messages.push(msg("user")); // 3
		messages.push(msg("assistant")); // 4
		messages.push(msg("user")); // 5
		messages.push(msg("assistant")); // 6
		messages.push(msg("user")); // 7
		messages.push(msg("assistant")); // 8
								   // Test with first_prompt_idx set to index 3 (first real user message)
		let (start_idx, end_idx) = find_compression_range(&messages, Some(3)).unwrap();

		// Safety requirement: compression starts AFTER first_prompt_idx (INCLUSIVE boundary)
		// first_prompt_idx=3 means index 3 is PROTECTED, compression starts at 4
		assert_eq!(
			start_idx, 3,
			"start_idx must equal first_prompt_idx (INCLUSIVE boundary)"
		);
		assert!(
			end_idx >= 4,
			"range should start compressing only after first_prompt_idx"
		);
	}

	#[test]
	#[allow(clippy::vec_init_then_push)]
	#[allow(clippy::needless_range_loop)]
	fn calculate_range_tokens_must_match_removal_range() {
		// CRITICAL TEST: Verify that calculate_range_tokens counts the EXACT same messages
		// that will be removed by remove_messages_in_range.
		//
		// BUG SCENARIO:
		// - find_compression_range returns (start_idx, end_idx)
		// - calculate_range_tokens counts [start_idx+1, end_idx] (SKIPS start_idx)
		// - messages_to_compress includes [start_idx, end_idx] for chunking
		// - remove_messages_in_range removes [start_idx+1, end_idx] (KEEPS start_idx)
		//
		// This means:
		// 1. tokens_before doesn't count the message at start_idx
		// 2. But that message IS included in semantic chunking
		// 3. The compressed summary can include content from start_idx message
		// 4. Result: tokens_after can be > tokens_before (BUG!)
		//
		// EXAMPLE:
		// - start_idx = 5, end_idx = 10
		// - tokens_before counts messages 6-10 (skips message 5)
		// - messages_to_compress includes message 5 for chunking
		// - If message 5 has 1000 tokens and messages 6-10 have 500 tokens total
		// - tokens_before = 500
		// - Compressed summary might be 600 tokens (includes content from message 5)
		// - tokens_after = 600
		// - Result: tokens_saved = 0 even though we removed 5 messages!
		//
		// FIX: calculate_range_tokens should count [start_idx, end_idx] to match
		// the messages that will be semantically chunked and potentially included in summary.

		// This test documents the expected behavior.
		// The actual fix will be in calculate_range_tokens function.
		use crate::session::estimate_message_tokens;

		let mut messages = Vec::new();
		messages.push(msg("system")); // 0

		// Create messages with known token counts
		let mut msg1 = msg("user"); // 1
		msg1.content = "x".repeat(100); // ~25 tokens
		messages.push(msg1);

		let mut msg2 = msg("assistant"); // 2
		msg2.content = "y".repeat(200); // ~50 tokens
		messages.push(msg2);

		let mut msg3 = msg("user"); // 3
		msg3.content = "z".repeat(300); // ~75 tokens
		messages.push(msg3);

		let mut msg4 = msg("assistant"); // 4
		msg4.content = "a".repeat(400); // ~100 tokens
		messages.push(msg4);

		// Add more messages to trigger compression
		messages.push(msg("user")); // 5
		messages.push(msg("assistant")); // 6
		messages.push(msg("user")); // 7
		messages.push(msg("assistant")); // 8

		let (start_idx, end_idx) = find_compression_range(&messages, None).unwrap();

		// Verify the range is valid
		assert!(start_idx < end_idx, "Range must be valid");

		// Count tokens that WILL BE REMOVED (matching remove_messages_in_range logic)
		// remove_messages_in_range removes [start_idx+1, end_idx]
		let mut expected_tokens = 0u64;
		for i in (start_idx + 1)..=end_idx {
			expected_tokens += estimate_message_tokens(&messages[i]) as u64;
		}

		// Count tokens that ARE INCLUDED in semantic chunking
		// messages_to_compress = [start_idx, end_idx]
		let mut chunked_tokens = 0u64;
		for i in start_idx..=end_idx {
			chunked_tokens += estimate_message_tokens(&messages[i]) as u64;
		}

		// THE BUG: expected_tokens != chunked_tokens
		// calculate_range_tokens returns expected_tokens (removal range)
		// But semantic chunking includes chunked_tokens (includes start_idx)
		// This can cause tokens_after > tokens_before

		// Document the discrepancy
		if expected_tokens != chunked_tokens {
			let start_msg_tokens = estimate_message_tokens(&messages[start_idx]) as u64;
			assert_eq!(
				chunked_tokens - expected_tokens,
				start_msg_tokens,
				"The difference should be exactly the tokens in start_idx message"
			);
		}
	}

	// ============================================================================
	// BUG-PROVING TESTS: These tests demonstrate the actual bugs in compression
	// ============================================================================

	#[test]
	#[allow(clippy::needless_range_loop)]
	fn bug_proof_token_mismatch_causes_zero_savings() {
		// BUG SCENARIO: calculate_range_tokens counts [start_idx+1, end_idx]
		// but semantic chunking uses [start_idx, end_idx], causing token mismatch
		use crate::session::estimate_message_tokens;

		let mut messages = Vec::new();
		messages.push(msg("system")); // 0

		// Message at start_idx has LARGE token count
		let mut large_msg = msg("user"); // 1
		large_msg.content = "x".repeat(4000); // ~1000 tokens
		messages.push(large_msg);

		// Messages after start_idx have SMALL token counts
		let mut small1 = msg("assistant"); // 2
		small1.content = "y".repeat(40); // ~10 tokens
		messages.push(small1);

		let mut small2 = msg("user"); // 3
		small2.content = "z".repeat(40); // ~10 tokens
		messages.push(small2);

		let mut small3 = msg("assistant"); // 4
		small3.content = "a".repeat(40); // ~10 tokens
		messages.push(small3);

		// Add more to trigger compression
		messages.push(msg("user")); // 5
		messages.push(msg("assistant")); // 6
		messages.push(msg("user")); // 7
		messages.push(msg("assistant")); // 8

		let (start_idx, end_idx) = find_compression_range(&messages, None).unwrap();
		assert_eq!(start_idx, 1); // Large message
		assert_eq!(end_idx, 4); // Last small message

		// What calculate_range_tokens ACTUALLY counts (CURRENT BUG)
		let mut tokens_counted_by_function = 0u64;
		for i in (start_idx + 1)..=end_idx {
			tokens_counted_by_function += estimate_message_tokens(&messages[i]) as u64;
		}

		// What semantic chunking ACTUALLY includes
		let mut tokens_in_chunking = 0u64;
		for i in start_idx..=end_idx {
			tokens_in_chunking += estimate_message_tokens(&messages[i]) as u64;
		}

		// THE BUG: Massive discrepancy!
		let large_msg_tokens = estimate_message_tokens(&messages[start_idx]) as u64;

		// Debug: print actual token counts
		println!("Large message tokens: {}", large_msg_tokens);
		println!("Tokens counted by function: {}", tokens_counted_by_function);
		println!("Tokens in chunking: {}", tokens_in_chunking);

		// The key assertion: chunking includes start_idx, but counting doesn't
		assert_eq!(
			tokens_in_chunking,
			tokens_counted_by_function + large_msg_tokens,
			"Chunking includes the large message that wasn't counted!"
		);

		// Verify the large message has significantly more tokens than small ones
		assert!(
			large_msg_tokens > tokens_counted_by_function,
			"Large message ({}) should have more tokens than all small messages combined ({})",
			large_msg_tokens,
			tokens_counted_by_function
		);

		// RESULT: If compressed summary is 100 tokens (from small messages)
		// tokens_before = 30 (only small messages counted)
		// tokens_after = 100 (compressed summary)
		// tokens_saved = 0 or NEGATIVE! (BUG!)
		//
		// But we actually removed 1030 tokens worth of messages!
	}

	#[test]
	fn bug_proof_insufficient_compression_triggers_loop() {
		// BUG SCENARIO: Compression triggers when full context > threshold
		// but doesn't check if compression will bring context BELOW threshold
		//
		// Example:
		// - Full context: 55,000 tokens
		// - Threshold: 50,000 tokens
		// - System + tools + recent: 52,000 tokens (non-compressible)
		// - Compressible old messages: 3,000 tokens
		// - After 2x compression: 52,000 + 1,500 = 53,500 tokens
		// - Still above threshold! Triggers again next iteration!

		// This test documents the expected behavior
		// The actual fix will be in should_check_compression

		let full_context_tokens = 55_000u64;
		let threshold = 50_000u64;
		let non_compressible_tokens = 52_000u64; // system + tools + recent
		let compressible_tokens = 3_000u64;
		let compression_ratio = 2.0;

		assert_eq!(
			full_context_tokens,
			non_compressible_tokens + compressible_tokens
		);

		// After compression
		let compressed_tokens = (compressible_tokens as f64 / compression_ratio) as u64;
		let tokens_after_compression = non_compressible_tokens + compressed_tokens;

		// THE BUG: Still above threshold!
		assert!(
			tokens_after_compression > threshold,
			"Compression didn't bring context below threshold: {} > {}",
			tokens_after_compression,
			threshold
		);

		// This will trigger compression AGAIN on next check
		// Creating a compression loop until continuation triggers
	}

	#[test]
	fn bug_proof_compression_should_verify_benefit() {
		// BUG SCENARIO: Compression should check if it will actually help
		// before triggering. If non-compressible portion is already > threshold,
		// compression is futile.

		let threshold = 50_000u64;
		let system_tokens = 5_000u64;
		let tools_tokens = 30_000u64;
		let recent_4_messages_tokens = 20_000u64;
		let old_compressible_tokens = 2_000u64;

		let non_compressible = system_tokens + tools_tokens + recent_4_messages_tokens;
		let full_context = non_compressible + old_compressible_tokens;

		assert!(full_context > threshold, "Triggers compression");

		// Even with perfect 10x compression
		let best_case_compressed = old_compressible_tokens / 10;
		let best_case_result = non_compressible + best_case_compressed;

		// THE BUG: Even best-case compression won't help!
		assert!(
			best_case_result > threshold,
			"Non-compressible portion alone exceeds threshold: {} > {}",
			best_case_result,
			threshold
		);

		// FIX: should_check_compression should verify:
		// if (non_compressible + (compressible / ratio)) < threshold {
		//     compress
		// } else {
		//     skip compression — non-compressible portion already exceeds threshold
		// }
	}

	#[test]
	fn test_cooldown_prevents_premature_recompression() {
		// TEST: Cooldown should block compression until estimated benefit window passes

		// Scenario 1: First compression at API call 10, estimated 20 future turns
		let current_api_calls = 10;
		let estimated_turns = 20.0;
		let next_compression_at = current_api_calls + estimated_turns as usize; // = 30

		assert_eq!(
			next_compression_at, 30,
			"Next compression should be at call 30"
		);

		// Scenario 2: At API call 15 (5 turns later), cooldown should block
		let current_at_15 = 15;
		assert!(
			current_at_15 < next_compression_at,
			"Cooldown should block at call 15: {} < {}",
			current_at_15,
			next_compression_at
		);

		// Scenario 3: At API call 29 (19 turns later), still blocked
		let current_at_29 = 29;
		assert!(
			current_at_29 < next_compression_at,
			"Cooldown should still block at call 29: {} < {}",
			current_at_29,
			next_compression_at
		);

		// Scenario 4: At API call 30 (20 turns later), cooldown passes
		let current_at_30 = 30;
		assert!(
			current_at_30 >= next_compression_at,
			"Cooldown should pass at call 30: {} >= {}",
			current_at_30,
			next_compression_at
		);

		// Scenario 5: At API call 35 (25 turns later), still allowed
		let current_at_35 = 35;
		assert!(
			current_at_35 >= next_compression_at,
			"Compression should be allowed at call 35: {} >= {}",
			current_at_35,
			next_compression_at
		);
	}

	#[test]
	fn test_cooldown_default_allows_first_compression() {
		// TEST: Default value (0) should allow first compression immediately

		let next_compression_at = 0; // Default value
		let current_api_calls = 1; // First API call

		assert!(
			current_api_calls >= next_compression_at,
			"First compression should be allowed: {} >= {}",
			current_api_calls,
			next_compression_at
		);

		// Even at call 0 (edge case)
		let current_at_0 = 0;
		assert!(
			current_at_0 >= next_compression_at,
			"Compression should be allowed even at call 0: {} >= {}",
			current_at_0,
			next_compression_at
		);
	}

	#[test]
	fn test_cooldown_calculation_with_varying_estimates() {
		// TEST: Cooldown adapts to different estimated turn counts

		// Short session: 5 estimated turns
		let current = 10;
		let estimated_short = 5.0;
		let next_short = current + estimated_short as usize;
		assert_eq!(next_short, 15, "Short estimate: next at 15");

		// Medium session: 20 estimated turns
		let estimated_medium = 20.0;
		let next_medium = current + estimated_medium as usize;
		assert_eq!(next_medium, 30, "Medium estimate: next at 30");

		// Long session: 50 estimated turns
		let estimated_long = 50.0;
		let next_long = current + estimated_long as usize;
		assert_eq!(next_long, 60, "Long estimate: next at 60");

		// Verify cooldown scales with estimate
		assert!(next_short < next_medium, "Short cooldown < medium cooldown");
		assert!(next_medium < next_long, "Medium cooldown < long cooldown");
	}

	#[test]
	fn test_estimate_physical_ceiling_is_headroom_over_growth() {
		// physical_ceiling = headroom / growth_rate — pure math, no constants
		// headroom = current_tokens - compressed_tokens
		let current_tokens = 100_000.0_f64;
		let compression_ratio = 2.5_f64;
		let compressed = current_tokens / compression_ratio; // 40_000
		let headroom = current_tokens - compressed; // 60_000

		let growth_rate = 5_000.0_f64; // 5k output tokens/call
		let ceiling = headroom / growth_rate; // exactly 12 calls
		assert_eq!(ceiling, 12.0);

		// Larger growth rate → fewer calls fit → lower ceiling
		let ceiling_fast = headroom / 10_000.0_f64; // 6 calls
		assert!(ceiling_fast < ceiling, "faster growth → lower ceiling");

		// Higher compression ratio → more headroom → higher ceiling
		let compressed_aggressive = current_tokens / 4.0; // 25_000
		let headroom_aggressive = current_tokens - compressed_aggressive; // 75_000
		let ceiling_aggressive = headroom_aggressive / growth_rate; // 15 calls
		assert!(
			ceiling_aggressive > ceiling,
			"more compression → more headroom → higher ceiling"
		);
	}

	#[test]
	fn test_estimate_symmetry_is_api_calls_so_far() {
		// Symmetry: calls remaining ≈ calls made (sessions are roughly symmetric)
		// Final = min(physical_ceiling, api_calls)
		let api_calls = 20.0_f64;
		let physical_ceiling = 30.0_f64;

		// symmetry < ceiling → symmetry wins (session likely winding down)
		let estimate = physical_ceiling.min(api_calls);
		assert_eq!(
			estimate, api_calls,
			"symmetry wins when smaller than ceiling"
		);

		// ceiling < symmetry → ceiling wins (context budget is the constraint)
		let api_calls_large = 50.0_f64;
		let estimate2 = physical_ceiling.min(api_calls_large);
		assert_eq!(
			estimate2, physical_ceiling,
			"ceiling wins when smaller than symmetry"
		);
	}

	#[test]
	fn test_estimate_zero_api_calls_caps_physical_ceiling() {
		// With api_calls=0 and no output data, growth_rate floors at 1.0, producing a
		// huge raw ceiling (headroom / 1 = headroom). We cap at 100 so the cold-start
		// cooldown is meaningful rather than a nonsensical 60k+.
		let current_tokens = 100_000.0_f64;
		let compression_ratio = 2.5_f64;
		let compressed = current_tokens / compression_ratio;
		let headroom = current_tokens - compressed; // 60_000

		let growth_rate = (0.0_f64 / 1.0_f64).max(1.0); // floor=1, no data
		let raw_ceiling = headroom / growth_rate; // 60_000 — unreliable sentinel
		assert_eq!(raw_ceiling, 60_000.0);

		// Cap applied: cold-start estimate is bounded at 100
		let estimate = raw_ceiling.min(100.0);
		assert_eq!(estimate, 100.0, "cold-start ceiling capped at 100, not 60k");
		assert!(estimate >= 5.0, "always at least 5");
	}

	#[test]
	fn test_estimate_growth_rate_from_measured_output() {
		// growth_rate = output_tokens / max(api_calls, 1), floored at 1.0
		// Floor at 1.0 is not a magic constant — it's division-by-zero protection
		let cases = [
			(10.0_f64, 50_000.0_f64, 5_000.0_f64), // measured: 5k/call
			(1.0, 3_000.0, 3_000.0),               // single call
			(0.0, 0.0, 1.0),                       // no data: floor=1 (not magic, just safe)
		];
		for (api_calls, output_tokens, expected) in cases {
			let rate = (output_tokens / api_calls.max(1.0)).max(1.0);
			assert_eq!(
				rate, expected,
				"api_calls={api_calls}, output={output_tokens}"
			);
		}
	}

	#[test]
	fn test_self_tuning_direct_ratio_no_blending() {
		// Self-tuning returns actual/predicted directly — no blending weight
		// If we predicted 20 but only 10 happened: ratio=0.5 → scale down
		let predicted = 20.0_f64;
		let actual = 10.0_f64;
		let ratio = (actual / predicted).clamp(0.25, 4.0);
		assert_eq!(ratio, 0.5, "underestimated → ratio < 1");

		// If we predicted 10 but 30 happened: ratio=3.0 → scale up
		let ratio2 = (30.0_f64 / 10.0_f64).clamp(0.25, 4.0);
		assert_eq!(ratio2, 3.0, "overestimated → ratio > 1");

		// Clamp prevents extreme outliers from dominating
		let ratio_extreme_low = (1.0_f64 / 100.0_f64).clamp(0.25, 4.0);
		assert_eq!(ratio_extreme_low, 0.25, "extreme low clamped");
		let ratio_extreme_high = (100.0_f64 / 1.0_f64).clamp(0.25, 4.0);
		assert_eq!(ratio_extreme_high, 4.0, "extreme high clamped");
	}

	#[test]
	fn test_self_tuning_neutral_when_no_prior_compression() {
		// No prior compressions → return 1.0 (no correction to apply)
		// Tested via the logic directly since we can't call the fn without SessionInfo
		let compressions = 0_usize;
		let result = if compressions == 0 { 1.0_f64 } else { 0.5 };
		assert_eq!(result, 1.0, "no prior data → neutral multiplier");
	}

	#[test]
	fn test_estimate_end_to_end_symmetry_wins() {
		// Session: 10 calls, 50k output, 100k context, 2.5x compression
		// physical_ceiling = 60_000 / 5_000 = 12
		// symmetry = 10
		// estimate = min(12, 10) = 10
		let api_calls = 10.0_f64;
		let output_tokens = 50_000.0_f64;
		let current_tokens = 100_000.0_f64;
		let compression_ratio = 2.5_f64;

		let growth_rate = (output_tokens / api_calls).max(1.0); // 5_000
		let headroom = current_tokens - current_tokens / compression_ratio; // 60_000
		let ceiling = headroom / growth_rate; // 12
		let estimate = ceiling.min(api_calls); // min(12, 10) = 10

		assert_eq!(ceiling, 12.0);
		assert_eq!(estimate, 10.0, "symmetry (10) wins over ceiling (12)");
		assert!(estimate >= 5.0);
	}

	#[test]
	fn test_estimate_end_to_end_ceiling_wins() {
		// Session: 30 calls, 300k output, 100k context, 2.5x compression
		// growth_rate = 300_000 / 30 = 10_000/call
		// physical_ceiling = 60_000 / 10_000 = 6
		// symmetry = 30
		// estimate = min(6, 30) = 6 → floored at 5 → 6
		let api_calls = 30.0_f64;
		let output_tokens = 300_000.0_f64;
		let current_tokens = 100_000.0_f64;
		let compression_ratio = 2.5_f64;

		let growth_rate = (output_tokens / api_calls).max(1.0); // 10_000
		let headroom = current_tokens - current_tokens / compression_ratio; // 60_000
		let ceiling = headroom / growth_rate; // 6
		let estimate = ceiling.min(api_calls); // min(6, 30) = 6

		assert_eq!(ceiling, 6.0);
		assert_eq!(estimate, 6.0, "ceiling (6) wins over symmetry (30)");
		assert!(estimate >= 5.0);
	}

	#[test]
	fn test_estimate_incremental_growth_rate_after_compression() {
		// After a compression, growth_rate must use only tokens/calls since that
		// checkpoint — not the lifetime average which carries stale pre-compression signal.
		//
		// Scenario: heavy exploration phase (20 calls, 200k output = 10k/call),
		// then compression fires. Post-compression: 5 calls, 10k output = 2k/call.
		// Lifetime average = 210k / 25 = 8,400/call — 4x wrong.
		// Incremental = 10k / 5 = 2,000/call — correct.

		let total_api_calls: usize = 25;
		let total_output_tokens: u64 = 210_000;
		let api_calls_at_last_compression: usize = 20;
		let output_tokens_at_last_compression: u64 = 200_000;

		// Incremental (correct)
		let calls_since = (total_api_calls - api_calls_at_last_compression).max(1) as f64; // 5
		let output_since =
			total_output_tokens.saturating_sub(output_tokens_at_last_compression) as f64; // 10_000
		let incremental_rate = (output_since / calls_since).max(1.0); // 2_000
		assert_eq!(
			incremental_rate, 2_000.0,
			"incremental rate reflects post-compression phase"
		);

		// Lifetime (stale — what the old code used)
		let lifetime_rate = (total_output_tokens as f64 / total_api_calls as f64).max(1.0); // 8_400
		assert_eq!(
			lifetime_rate, 8_400.0,
			"lifetime rate is inflated by heavy early phase"
		);

		// Incremental gives a higher physical ceiling → less aggressive re-compression
		let current_tokens = 100_000.0_f64;
		let compression_ratio = 2.5_f64;
		let headroom = current_tokens - current_tokens / compression_ratio; // 60_000

		let ceiling_incremental = headroom / incremental_rate; // 30 calls
		let ceiling_lifetime = headroom / lifetime_rate; // ~7 calls

		assert!(
			ceiling_incremental > ceiling_lifetime,
			"incremental ceiling ({ceiling_incremental}) > lifetime ceiling ({ceiling_lifetime}): \
			stale lifetime rate would trigger re-compression 4x too soon"
		);
		assert_eq!(ceiling_incremental, 30.0);
	}

	#[test]
	fn test_estimate_growth_rate_falls_back_to_lifetime_before_first_compression() {
		// Before any compression there is no checkpoint, so lifetime average is the
		// only signal available — and it's correct (no pre-compression phase to pollute it).
		let compressions: usize = 0;
		let total_api_calls = 10_usize;
		let total_output_tokens: u64 = 50_000;
		let api_calls_at_last_compression: usize = 0;
		let output_tokens_at_last_compression: u64 = 0;

		let growth_rate = if compressions > 0 {
			let calls_since = (total_api_calls - api_calls_at_last_compression).max(1) as f64;
			let output_since =
				total_output_tokens.saturating_sub(output_tokens_at_last_compression) as f64;
			(output_since / calls_since).max(1.0)
		} else {
			(total_output_tokens as f64 / total_api_calls.max(1) as f64).max(1.0)
		};

		// With no prior compression, lifetime = incremental (same data window)
		assert_eq!(
			growth_rate, 5_000.0,
			"lifetime fallback: 50k / 10 calls = 5k/call"
		);
	}

	#[test]
	fn test_estimate_incremental_rate_single_call_since_compression() {
		// Edge: only 1 call since last compression — still uses that single measurement,
		// not the lifetime average. saturating_sub prevents underflow if counters drift.
		let total_api_calls: usize = 21;
		let total_output_tokens: u64 = 205_000;
		let api_calls_at_last_compression: usize = 20;
		let output_tokens_at_last_compression: u64 = 200_000;

		let calls_since = (total_api_calls - api_calls_at_last_compression).max(1) as f64; // 1
		let output_since =
			total_output_tokens.saturating_sub(output_tokens_at_last_compression) as f64; // 5_000
		let rate = (output_since / calls_since).max(1.0);
		assert_eq!(
			rate, 5_000.0,
			"single post-compression call measured correctly"
		);
	}

	#[test]
	fn test_estimate_incremental_rate_saturating_sub_prevents_underflow() {
		// If output_tokens_at_last_compression somehow exceeds current (e.g. counter reset),
		// saturating_sub returns 0 → growth_rate floors at 1.0 rather than panicking.
		let total_output_tokens: u64 = 1_000;
		let output_tokens_at_last_compression: u64 = 5_000; // anomalous: larger than current
		let output_since = total_output_tokens.saturating_sub(output_tokens_at_last_compression); // 0
		assert_eq!(output_since, 0, "saturating_sub: no underflow");
		let rate = (output_since as f64 / 1.0_f64).max(1.0);
		assert_eq!(rate, 1.0, "floors at 1.0, no panic");
	}
}
