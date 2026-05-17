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

mod ai;
mod apply;
mod knowledge;
mod prompt;
mod range;

// Submodule entrypoints used by this orchestrator file:
// - `ai::ask_ai_decision_and_summary` runs the LLM round-trip (it builds the
//   prompt internally via `prompt::build_compression_prompt`).
// - `apply::{apply_compression, collect_preserved_skills}` materialises the
//   chosen drain range against the session.
// - `range::{find_compression_range, calculate_range_tokens}` decides which
//   indices to drain and what they cost in tokens.
use ai::ask_ai_decision_and_summary;
use apply::{apply_compression, collect_preserved_skills};
use range::{calculate_range_tokens, find_compression_range};

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

	// HARD CEILING: max_session_tokens_threshold is the user's explicit safety limit.
	// When set and exceeded, force compression unconditionally — no cooldown, no cost
	// analysis, no "won't bring below threshold" checks. This is the last line of defense.
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
			"Max session token threshold exceeded ({} >= {}) - FORCE triggering compression with ratio {:.1}x (bypasses all gates)",
			current_tokens,
			config.max_session_tokens_threshold,
			ratio
		);
		return (true, ratio);
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

	// RATIO SELECTION: Pick pressure level by token threshold, then escalate
	// based on consecutive compressions (without user interaction).
	// Each consecutive compression bumps to the next level (round-robin).
	// This prevents infinite loops at level 0 when compress-all drops context
	// hard and it grows back to the same threshold repeatedly.
	let matched_level = config
		.compression
		.pressure_levels
		.iter()
		.filter(|l| current_tokens >= l.threshold)
		.max_by(|a, b| a.threshold.cmp(&b.threshold));

	let num_levels = config.compression.pressure_levels.len();
	if num_levels == 0 {
		log_debug!("No pressure levels configured - compression disabled");
		return (false, 2.0);
	}

	let level = match matched_level {
		Some(_) => {
			// Escalate: use consecutive_compressions to index into levels (round-robin)
			let n = session.session.info.consecutive_compressions as usize;
			let escalated_idx = n % num_levels;
			&config.compression.pressure_levels[escalated_idx]
		}
		None => {
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
			return (false, 2.0);
		}
	};

	// ADAPTIVE COMPRESSION RATIO: Adjust based on session patterns
	let adjusted_ratio = calculate_adaptive_compression_ratio(session, level.target_ratio);

	log_debug!(
		"✓ Threshold exceeded! Context tokens: {} → base compression: {:.1}x → adaptive: {:.1}x (threshold: {})",
		current_tokens,
		level.target_ratio,
		adjusted_ratio,
		level.threshold
	);

	// EXPONENTIAL COOLDOWN: Each consecutive compression (without a user message)
	// doubles the required token growth before re-compression is allowed.
	// 1st: 10%, 2nd: 20%, 3rd: 40%, 4th+: 80-100%.
	// This prevents futile loops while still allowing compression when context genuinely grows.
	let tokens_after_last = session.session.info.context_tokens_after_last_compression;

	if tokens_after_last > 0 {
		let n = session.session.info.consecutive_compressions;
		// 0.10 * 2^n, capped at 1.0 (i.e. require 100% growth = context must double)
		let growth_factor = (0.10 * 2.0_f64.powi(n as i32)).min(1.0);
		let min_tokens_for_recompression =
			(tokens_after_last as f64 * (1.0 + growth_factor)) as usize;
		if current_tokens < min_tokens_for_recompression {
			let actual_growth_pct =
				((current_tokens as f64 / tokens_after_last as f64 - 1.0) * 100.0) as i32;
			log_debug!(
				"Exponential cooldown active (n={}): need {:.0}% growth, have {}% (current={}, required={}, base={})",
				n,
				growth_factor * 100.0,
				actual_growth_pct,
				current_tokens,
				min_tokens_for_recompression,
				tokens_after_last
			);
			return (false, 2.0);
		}
	}

	log_debug!(
		"Compression cooldown passed: current_tokens={}, tokens_after_last_compression={}, consecutive={}",
		current_tokens,
		tokens_after_last,
		session.session.info.consecutive_compressions
	);

	// CACHE-AWARE DECISION: Calculate if compression is profitable
	let net_benefit =
		calculate_compression_net_benefit(session, config, current_tokens, adjusted_ratio).await;

	if net_benefit > 0.0 {
		// Verify compression will actually reduce context meaningfully
		let (start_idx, end_idx) = match find_compression_range(
			&session.session.messages,
			session.first_prompt_idx,
			false,
		) {
			Ok(range) => range,
			Err(e) => {
				log_debug!("Failed to find compression range: {}", e);
				return (false, 2.0);
			}
		};

		if start_idx >= end_idx {
			log_debug!(
				"Invalid compression range ({} >= {}), setting cooldown to prevent re-analysis loop",
				start_idx,
				end_idx
			);
			session.session.info.context_tokens_after_last_compression = current_tokens;
			return (false, 2.0);
		}

		// Count only start_idx+1..=end_idx — the anchor at start_idx is kept
		let compressible_tokens = match calculate_range_tokens(session, start_idx + 1, end_idx) {
			Ok(tokens) => tokens,
			Err(e) => {
				log_debug!("Failed to calculate range tokens: {}", e);
				return (false, 2.0);
			}
		};

		let estimated_compressed_size = (compressible_tokens as f64 / adjusted_ratio) as u64;
		let estimated_after_compression = (current_tokens as u64)
			.saturating_sub(compressible_tokens)
			.saturating_add(estimated_compressed_size);

		if estimated_after_compression >= level.threshold as u64 {
			log_debug!(
				"Compression won't bring context below threshold: {} → {} (threshold: {}). Compressible: {} → {}. Setting cooldown.",
				current_tokens,
				estimated_after_compression,
				level.threshold,
				compressible_tokens,
				estimated_compressed_size
			);
			session.session.info.context_tokens_after_last_compression = current_tokens;
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
	let headroom = total_tokens - (total_tokens / compression_ratio);
	let estimated_future_turns = estimate_future_turns(session, headroom);
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
///    headroom  = actual tokens freed by compression (passed by caller)
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
fn estimate_future_turns(session: &ChatSession, headroom: f64) -> f64 {
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
	// headroom = actual tokens freed by compression (provided by caller).
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

pub enum CompressionTrigger {
	/// Normal automatic compression — respects thresholds/cooldowns, preserves all active skills.
	Automatic,
	/// `/done` command — bypasses thresholds, preserves only env-loaded skills (OCTOMIND_SKILLS).
	Done,
}

/// Main entry point: check if compression needed and perform if AI decides YES
/// Returns true if compression was performed, false otherwise
pub async fn check_and_compress_conversation(
	session: &mut ChatSession,
	config: &Config,
	operation_rx: tokio::sync::watch::Receiver<bool>,
	trigger: CompressionTrigger,
) -> Result<bool> {
	let (should_check, computed_ratio) = should_check_compression(session, config).await;

	let force = matches!(trigger, CompressionTrigger::Done);

	if !force && !should_check {
		return Ok(false);
	}

	// When max_session_tokens_threshold is exceeded, force compression — AI cannot refuse.
	// This is the user's explicit safety ceiling; the decision model has no veto here.
	let force = force
		|| (config.max_session_tokens_threshold > 0 && {
			let current_tokens = session.get_full_context_tokens(config).await;
			current_tokens >= config.max_session_tokens_threshold
		});

	// When force=true (/done or skill-forget), use fixed level 1 pressure ratio (no adaptive adjustment).
	// Regular automatic compressions use the adaptive ratio from should_check_compression.
	let target_ratio = if force {
		config
			.compression
			.pressure_levels
			.first()
			.map(|l| l.target_ratio)
			.unwrap_or(2.0)
	} else {
		computed_ratio
	};

	// Check for cancellation before starting compression (which involves an API call)
	if *operation_rx.borrow() {
		return Err(anyhow::Error::new(crate::session::cancellation::Cancelled));
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

	// Surface the phase on the spinner — compression can take several seconds
	// (decision model + summary call). RAII guard guarantees clear_phase
	// runs on every exit path (success, `return`, or `?` propagation).
	animation_manager
		.set_phase("Compressing conversation…")
		.await;
	struct PhaseGuard<'a>(&'a crate::session::chat::animation_manager::AnimationManager);
	impl Drop for PhaseGuard<'_> {
		fn drop(&mut self) {
			self.0.clear_phase();
		}
	}
	let _phase_guard = PhaseGuard(animation_manager);

	log_debug!("Compression check triggered - asking AI for decision and summary in one call");

	// OPTIMIZATION: Do semantic chunking BEFORE AI call (local, no API cost)
	// This allows us to send context chunks to AI in the same call as decision
	let (start_idx, end_idx) =
		find_compression_range(&session.session.messages, session.first_prompt_idx, force)?;

	// end_idx is already safe from find_compression_range

	if start_idx >= end_idx {
		log_debug!("No messages to compress (range invalid)");
		return Ok(false);
	}

	// SKILL PRESERVATION: skill injections land as user-role messages with
	// content wrapped in <skill name="..."> tags (see add_user_message in
	// skill_auto::load_env_skills and skill::execute_use → inbox). If they
	// fall inside the drain range they get wiped by compression, and the AI
	// loses the domain guidance that was active. Extract them here so
	// apply_compression can re-insert them between the anchor and the summary.
	//
	// When trigger=Done (/done), preserve ONLY env-loaded skills (OCTOMIND_SKILLS).
	// Auto-activated skills are context-dependent and should re-activate if
	// the context still matches after the summary.
	//
	// When trigger=Automatic or SkillForget, preserve all active skills.
	let skill_names_to_preserve: Vec<String> = if matches!(trigger, CompressionTrigger::Done) {
		crate::session::context::current_session_id()
			.map(|sid| crate::session::context::get_env_skills(&sid))
			.unwrap_or_default()
	} else {
		crate::session::context::current_session_id()
			.map(|sid| crate::session::context::get_active_skills(&sid))
			.unwrap_or_default()
	};
	let preserved_skills = collect_preserved_skills(
		&session.session.messages,
		start_idx + 1,
		end_idx,
		&skill_names_to_preserve,
	);

	// COMPRESS-ALL: Extract user messages BEFORE compression.
	// - Last user message → re-injected as raw session message after summary
	// - Last 4 user messages (excluding the appended one) → USER TASKS section in summary
	// No intersection: the appended message is NOT in USER TASKS.
	// Skill messages are filtered out — they're preserved verbatim via
	// preserved_skills and must never show up as "user tasks" or get
	// re-injected as the raw user prompt after the summary.
	let all_user_msgs: Vec<&crate::session::Message> = session.session.messages
		[start_idx + 1..=end_idx]
		.iter()
		.filter(|m| {
			m.role == "user"
				&& !m.content.trim().is_empty()
				&& !crate::mcp::core::skill::is_skill_message(&m.content)
		})
		.collect();

	// Last user message for raw re-injection after summary
	let last_user_message = all_user_msgs.last().cloned().cloned();

	// Last 4 user messages EXCLUDING the appended one → USER TASKS in summary
	let user_tasks_msgs: Vec<String> = {
		let exclude_last = if all_user_msgs.len() > 1 {
			&all_user_msgs[..all_user_msgs.len() - 1]
		} else {
			&[]
		};
		exclude_last
			.iter()
			.rev()
			.take(4)
			.rev()
			.map(|m| {
				let content = m.content.trim();
				if content.len() > 200 {
					format!(
						"{}…",
						&content[..content
							.char_indices()
							.take_while(|&(i, _)| i <= 200)
							.last()
							.map(|(i, _)| i)
							.unwrap_or(200)]
					)
				} else {
					content.to_string()
				}
			})
			.collect()
	};

	// Calculate tokens before compression (all messages that will be removed)
	let tokens_before = calculate_range_tokens(session, start_idx + 1, end_idx)?;

	// Skill messages are preserved verbatim (see preserved_skills above) —
	// exclude them from the AI summarizer input so we don't burn tokens
	// paraphrasing instructions we'll re-inject word-for-word.
	let messages_to_compress: Vec<crate::session::Message> = session.session.messages
		[start_idx + 1..=end_idx]
		.iter()
		.filter(|m| !(m.role == "user" && crate::mcp::core::skill::is_skill_message(&m.content)))
		.cloned()
		.collect();

	// OPTIMIZATION: Single API call for decision + summary (1-hop instead of 2-hop)
	let (should_compress, context_summary) = ask_ai_decision_and_summary(
		session,
		config,
		&messages_to_compress,
		operation_rx,
		force,
		target_ratio,
	)
	.await?;

	if !should_compress {
		log_debug!("AI decided compression not beneficial at this point");
		return Ok(false);
	}

	log_info!("AI decided to compress older conversation exchanges");

	// Apply compression with the summary we got from AI
	apply_compression(
		session,
		start_idx,
		end_idx,
		&context_summary,
		tokens_before,
		current_context_tokens,
		user_tasks_msgs,
		last_user_message,
		preserved_skills,
	)
	.await?;

	// Intermediate learning: extract lessons during auto-compaction if enough user messages.
	// Fire-and-forget — must NOT block compression on a second LLM round-trip.
	if config.learning.enabled {
		let user_msg_count = session
			.session
			.messages
			.iter()
			.filter(|m| m.role == "user")
			.count();
		if user_msg_count >= config.learning.min_messages_for_intermediate {
			let role = crate::config::get_thread_role().unwrap_or_default();
			crate::learning::extract::spawn_lesson_extraction(session, config, role, None);
		}
	}

	if force {
		// /done resets cooldown — treat as fresh session phase boundary.
		session.session.info.consecutive_compressions = 0;
		session.session.info.context_tokens_after_last_compression = 0;
		log_debug!("Forced compression: cooldown counters reset (fresh session phase)");
	} else {
		// EXPONENTIAL COOLDOWN: Increment consecutive compressions counter.
		// Each consecutive compression (without a user message) doubles the required
		// token growth before the next compression is allowed.
		// Resets to 0 on every new user message (see main_loop.rs).
		session.session.info.consecutive_compressions += 1;
		log_debug!(
			"Exponential cooldown: consecutive_compressions now {} (next requires {:.0}% growth)",
			session.session.info.consecutive_compressions,
			(0.10 * 2.0_f64.powi(session.session.info.consecutive_compressions as i32)).min(1.0)
				* 100.0
		);
	}

	// PhaseGuard above clears the phase on drop — no manual call needed.
	Ok(true)
}

#[cfg(test)]
mod tests;
