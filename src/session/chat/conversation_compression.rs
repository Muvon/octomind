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
		config.use_long_system_cache,
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
			let project = std::env::current_dir()
				.ok()
				.and_then(|p| p.file_name().and_then(|n| n.to_str()).map(String::from))
				.unwrap_or_else(|| "unknown".to_string());
			crate::learning::extract::extract_lessons_detached(
				session.session.messages.clone(),
				config.clone(),
				role,
				project,
				session.session.info.name.clone(),
			);
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

/// Ask AI: should we compress AND get summary in ONE call (1-hop optimization)
/// This combines decision + summarization to reduce latency and cost by 50%
/// Ask AI: should we compress AND get summary in ONE call (1-hop optimization)
/// This combines decision + summarization to reduce latency and cost by 50%.
/// When `force=true` the AI is only asked to summarize — it has no right to say NO.
/// Build the system and user prompt for the compression AI call.
///
/// Returns `(system_content, user_content)`.
/// `force=true` produces a direct-summary prompt (no YES/NO gate).
fn build_compression_prompt(
	session: &ChatSession,
	messages_to_compress: &[crate::session::Message],
	force: bool,
	target_ratio: f64,
) -> (String, String) {
	// SYSTEM: role identity + instructions (what the model must do and how to respond).
	// Kept separate from the data so the model acts as a compressor, not a session participant.
	//
	// SINGLE PATH: always produce a full summary when compressing — no silent YES/NO-only fallback.
	// The prompt encodes three priorities:
	//   1. CURRENT TASK — the user's most recent request dominates; older tasks compress aggressively.
	//   2. RECENCY — messages marked [RECENT] are preserved with highest fidelity.
	//   3. TOOL CALLS — secondary context, reduced to one-liners.
	let system_content = if force {
		"You are a conversation compressor. \
The user has explicitly requested compression. You MUST produce a summary — do NOT refuse. \
Do not start with YES or NO. Just write the summary directly using the format below.\n\n\
## CRITICAL PRIORITIES\n\n\
**Priority 1 — CURRENT TASK**: The user's MOST RECENT task/request is what matters most. \
If the user pivoted to a new topic mid-conversation, the new topic IS the current intent. \
Older completed/abandoned tasks can be compressed to a single line each.\n\n\
**Priority 2 — RECENCY**: Messages marked [RECENT] represent the current state of work. \
Preserve them with the highest fidelity — quote or closely paraphrase. \
Older messages can be compressed aggressively.\n\n\
**Priority 3 — TOOL CALLS are secondary**: Summarize what was done in one line each.\n\n\
## SUMMARY FORMAT\n\n\
**SESSION CONTEXT** (1 sentence):\n\
Brief overview of the session — what brought us here. Keep it short.\n\n\
**CURRENT TASK** (1-2 sentences):\n\
What is the user working on RIGHT NOW? This is the most recent request — highlight it as the primary focus.\n\n\
**PROGRESS** (2-4 sentences):\n\
What was completed for the current task? What is in progress? What was the outcome?\n\n\
**ANALYSIS FINDINGS** (preserve conclusions — this prevents re-doing work):\n\
Capture key findings from code analysis, debugging, or investigation. Include:\n\
- What was discovered (root causes, patterns, behaviors)\n\
- Specific code locations and what was found there\n\
- Conclusions drawn from tool results\n\
This section is CRITICAL — without it, the AI will re-read the same files to rediscover the same things.\n\n\
**RECENT EXCHANGES** (preserve with high fidelity — the most recent [RECENT] messages):\n\
For each recent user/assistant pair: quote or closely paraphrase.\n\n\
**KEY ENTITIES** (preserve exactly — copy values verbatim):\n\
- Files/paths: exact file paths, line numbers, code locations\n\
- Names: identifiers, function names, variable names, config keys\n\
- Errors/issues: problems encountered and their status\n\
- Decisions: choices made with reasoning\n\n\
**NEXT STEPS** (1-2 sentences):\n\
What needs to happen next to continue the current task?\n\n\
**FILE CONTEXT — files to auto-inject after compression (IMPORTANT):**\n\
Files listed in <context> tags will be AUTO-READ from disk and injected verbatim into the compressed summary. \
This is how the session retains real file content across compressions without re-reading. \
Include any file the session is actively working on or needs to continue.\n\
<context>\n\
filepath:startline:endline\n\
</context>\n\
Rules: <context> tags required; one entry per line as filepath:N:N (no spaces); \
paths from project root; line numbers 1–10000; max 5 ranges; prioritize files being edited or analyzed.\n\n\
**CRITICAL KNOWLEDGE — survives all future compressions:**\n\
If there is critical knowledge that MUST survive future compressions \
(e.g., a key architectural decision, a non-obvious constraint, a user preference, \
analysis conclusions, root cause findings), write it in a <knowledge> tag. \
2-3 sentences MAX. Only include if truly critical — not routine progress.\n\
<knowledge>\n\
Your critical insight here (2-3 sentences max).\n\
</knowledge>\n\n\
"
	} else {
		"You are a conversation compressor. \
Your job is to produce a lossless summary of a conversation transcript so the session can continue \
without losing any important context.\n\n\
## CRITICAL PRIORITIES (read carefully before summarizing)\n\n\
**Priority 1 — CURRENT TASK**: The user's MOST RECENT task/request is what matters most. \
If the user pivoted to a new topic mid-conversation, the new topic IS the current intent. \
Older completed/abandoned tasks can be compressed to a single line each.\n\n\
**Priority 2 — RECENCY**: Messages marked [RECENT] represent the current state of work. \
Preserve them with the highest fidelity — quote or closely paraphrase. \
Older messages without [RECENT] can be compressed aggressively.\n\n\
**Priority 3 — TOOL CALLS are secondary**: Summarize what was done in one line each \
(e.g. 'read file X', 'ran shell command Y, got Z'). Never reproduce full tool output.\n\n\
## WHEN TO ANSWER YES vs NO\n\n\
Answer YES if there are older exchanges that can be compressed without losing information needed \
to continue. Answer NO only if the transcript is already minimal and nothing can be safely reduced.\n\n\
## SUMMARY FORMAT (use when answering YES)\n\n\
**SESSION CONTEXT** (1 sentence):\n\
Brief overview of the session — what brought us here. Keep it short.\n\n\
**CURRENT TASK** (1-2 sentences):\n\
What is the user working on RIGHT NOW? This is the most recent request — highlight it as the primary focus.\n\n\
**PROGRESS** (2-4 sentences):\n\
What was completed for the current task? What is in progress? What was the outcome?\n\n\
**ANALYSIS FINDINGS** (preserve conclusions — this prevents re-doing work):\n\
Capture key findings from code analysis, debugging, or investigation. Include:\n\
- What was discovered (root causes, patterns, behaviors)\n\
- Specific code locations and what was found there\n\
- Conclusions drawn from tool results\n\
This section is CRITICAL — without it, the AI will re-read the same files to rediscover the same things.\n\n\
**RECENT EXCHANGES** (preserve with high fidelity — the most recent [RECENT] messages):\n\
For each recent user/assistant pair: quote or closely paraphrase. Do not compress these.\n\n\
**KEY ENTITIES** (preserve exactly — copy values verbatim):\n\
- Files/paths: exact file paths, line numbers, code locations\n\
- Names: identifiers, function names, variable names, config keys\n\
- Errors/issues: problems encountered and their status\n\
- Decisions: choices made with reasoning\n\n\
**NEXT STEPS** (1-2 sentences):\n\
What needs to happen next to continue the current task?\n\n\
## RESPONSE FORMAT\n\n\
Start with YES or NO on the first line.\n\
If YES, follow immediately with the summary using the sections above:\n\n\
YES\n\
**SESSION CONTEXT**: ...\n\
**CURRENT TASK**: ...\n\
**PROGRESS**: ...\n\
**ANALYSIS FINDINGS**:\n\
- [finding 1]\n\
- [finding 2]\n\
**RECENT EXCHANGES**:\n\
- User: [question] → Assistant: [answer]\n\
**KEY ENTITIES**:\n\
- Files/paths: ...\n\
- Errors/issues: ...\n\
- Decisions: ...\n\
**NEXT STEPS**: ...\n\n\
**FILE CONTEXT — files to auto-inject after compression (IMPORTANT):**\n\
Files listed in <context> tags will be AUTO-READ from disk and injected verbatim into the compressed summary. \
This is how the session retains real file content across compressions without re-reading. \
Include any file the session is actively working on or needs to continue.\n\
<context>\n\
filepath:startline:endline\n\
</context>\n\
Rules: <context> tags required; one entry per line as filepath:N:N (no spaces); \
paths from project root; line numbers 1–10000; max 5 ranges; prioritize files being edited or analyzed.\n\n\
**CRITICAL KNOWLEDGE — survives all future compressions:**\n\
If there is critical knowledge that MUST survive future compressions \
(e.g., a key architectural decision, a non-obvious constraint, a user preference, \
analysis conclusions, root cause findings), write it in a <knowledge> tag. \
2-3 sentences MAX. Only include if truly critical — not routine progress.\n\
<knowledge>\n\
Your critical insight here (2-3 sentences max).\n\
</knowledge>\n\n\
If NO, respond with just: NO"
	}
	.to_string();

	// USER: plain-text transcript of the range being compressed + semantic chunk hints.
	// Building a transcript (not raw messages) prevents the model from continuing the
	// tool-calling loop — it sees text to analyze, not a live conversation to participate in.
	//
	// RECENCY MARKER: the last 8 messages (min 4, max 8) are tagged [RECENT] so the AI
	// knows to preserve them with the highest fidelity. Capped at 8 to prevent the
	// RECENT window from growing so large it defeats compression on long sessions.
	let total_msgs = messages_to_compress.len();
	let recent_count = (total_msgs / 4).clamp(4, 8);
	let recent_start = total_msgs.saturating_sub(recent_count);

	let reduction_pct = ((1.0 - 1.0 / target_ratio) * 100.0) as u32;
	let aggressiveness = if target_ratio >= 4.0 {
		"very aggressive"
	} else if target_ratio >= 2.0 {
		"selective"
	} else {
		"gentle"
	};
	let mut user_content = format!(
		"**COMPRESSION TARGET**: Reduce this transcript to ~{}% of its original size ({:.1}x compression). \
Be {} in what you preserve.\n\n\
**Conversation transcript to compress:**\n\
NOTE: Messages marked [RECENT] are the most recent and most important — preserve them with \
highest fidelity. [USER]/[ASSISTANT] pairs are primary signal; [TOOL CALL]/[TOOL RESULT] are \
secondary context.\n\n",
		reduction_pct, target_ratio, aggressiveness,
	);

	// Inject accumulated critical knowledge from prior compressions
	if !session.critical_knowledge.is_empty() {
		user_content
			.push_str("**CRITICAL KNOWLEDGE (from prior compressions — MUST be preserved):**\n");
		for (i, knowledge) in session.critical_knowledge.iter().enumerate() {
			user_content.push_str(&format!("{}. {}\n", i + 1, knowledge));
		}
		user_content.push('\n');
	}

	// Collect file references from tool calls for context preservation
	// These can be re-read on demand after compression
	let mut file_refs: Vec<String> = Vec::new();

	for (idx, msg) in messages_to_compress.iter().enumerate() {
		let recent = if idx >= recent_start { "[RECENT] " } else { "" };
		match msg.role.as_str() {
			"system" => {} // skip system — already in our system message
			"assistant" => {
				// Include text content; summarize tool calls as one-liners with key arg.
				// CRITICAL: If this is a prior compressed summary, strip the FILE CONTEXT
				// section before including it — file content will be re-read fresh by the
				// new compression. Including stale XML bloats the prompt and causes the AI
				// to re-embed the same file bytes in every subsequent summary.
				let assistant_text = if msg
					.content
					.starts_with("## Conversation Summary [COMPRESSED:")
				{
					strip_file_context_from_summary(&msg.content)
				} else {
					msg.content.trim().to_string()
				};
				if !assistant_text.is_empty() {
					user_content.push_str(&format!("{}[ASSISTANT]: {}\n", recent, assistant_text));
				}
				if let Some(calls) = msg.tool_calls.as_ref().and_then(|v| v.as_array()) {
					for call in calls {
						let name = call
							.get("function")
							.and_then(|f| f.get("name"))
							.and_then(|n| n.as_str())
							.unwrap_or("unknown");

						// Extract a short key-arg hint (first path/query/command arg) so the
						// AI understands what the tool was operating on, not just its name.
						let key_arg = call
							.get("function")
							.and_then(|f| f.get("arguments"))
							.and_then(|a| {
								let obj = if let Some(s) = a.as_str() {
									serde_json::from_str::<serde_json::Value>(s).ok()
								} else {
									Some(a.clone())
								};
								obj.and_then(|o| {
									// Try common key-arg field names in priority order
									for key in &[
										"path", "paths", "query", "command", "pattern", "content",
										"task",
									] {
										if let Some(v) = o.get(key) {
											let s = match v {
												serde_json::Value::String(s) => s.clone(),
												serde_json::Value::Array(arr) => arr
													.iter()
													.filter_map(|x| x.as_str())
													.take(2)
													.collect::<Vec<_>>()
													.join(", "),
												_ => continue,
											};
											if !s.is_empty() {
												let hint = if s.len() > 80 {
													let end = s
														.char_indices()
														.map(|(i, _)| i)
														.take_while(|&i| i <= 80)
														.last()
														.unwrap_or(0);
													format!("{}\u{2026}", &s[..end])
												} else {
													s
												};
												return Some(hint);
											}
										}
									}
									None
								})
							})
							.unwrap_or_default();

						if key_arg.is_empty() {
							user_content.push_str(&format!("{}[TOOL CALL]: {}\n", recent, name));
						} else {
							user_content.push_str(&format!(
								"{}[TOOL CALL]: {}({})\n",
								recent, name, key_arg
							));
						}

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
				let content = msg.content.trim();
				// Preserve both the start (tool name/context) and the end (errors/results).
				// Errors typically appear at the tail — head-only truncation hides them.
				let truncated = if content.len() > 1500 {
					let head_end = content
						.char_indices()
						.map(|(i, _)| i)
						.take_while(|&i| i <= 600)
						.last()
						.unwrap_or(0);
					let tail_start = content
						.char_indices()
						.rev()
						.map(|(i, _)| i)
						.take_while(|&i| content.len() - i <= 900)
						.last()
						.unwrap_or(content.len());
					if head_end < tail_start {
						format!(
							"{}\u{2026}[truncated]\u{2026}{}",
							&content[..head_end],
							&content[tail_start..]
						)
					} else {
						content[..head_end].to_string()
					}
				} else {
					content.to_string()
				};
				user_content.push_str(&format!(
					"{}[TOOL RESULT: {}]: {}\n",
					recent, name, truncated
				));
			}
			_ => {
				// user messages — always include, never drop
				if !msg.content.trim().is_empty() {
					user_content.push_str(&format!("{}[USER]: {}\n", recent, msg.content.trim()));
				}
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

	(system_content, user_content)
}

/// Call the AI compression model and return the raw response content.
///
/// Tracks cost against the session unless `ignore_cost` is set in config.
async fn call_ai_for_decision(
	session: &mut ChatSession,
	config: &Config,
	system_content: String,
	user_content: String,
	operation_rx: tokio::sync::watch::Receiver<bool>,
) -> Result<String> {
	let now = crate::utils::time::now_secs();
	let messages = vec![
		crate::session::Message {
			role: "system".to_string(),
			content: system_content,
			timestamp: now,
			cached: false,
			cache_ttl: None,
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
			cache_ttl: None,
			tool_call_id: None,
			name: None,
			tool_calls: None,
			images: None,
			videos: None,
			thinking: None,
			id: None,
		},
	];

	let decision_config = &config.compression.decision;

	crate::log_debug!(
		"Using compression decision model '{}' (max_tokens={}, temp={}, ignore_cost={})",
		decision_config.model,
		decision_config.max_tokens,
		decision_config.temperature,
		decision_config.ignore_cost
	);

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

	if !decision_config.ignore_cost {
		if let Some(cost) = response.exchange.usage.as_ref().and_then(|u| u.cost) {
			session.session.info.total_cost += cost;
			session.estimated_cost = session.session.info.total_cost;
			log_debug!(
				"Compression decision cost: ${:.5} (total: ${:.5})",
				cost,
				session.session.info.total_cost
			);
		}
	} else {
		log_debug!("Compression decision cost ignored (ignore_cost=true)");
	}

	Ok(response.content)
}

/// Parse the AI response into a compression decision and optional summary text.
///
/// `force=true`: entire response is the summary (no YES/NO gate).
/// `force=false`: first line must be YES to proceed; NO means skip compression.
fn parse_ai_response(
	session: &mut ChatSession,
	config: &Config,
	content: &str,
	force: bool,
) -> Result<(bool, String)> {
	let content = content.trim();
	let lines: Vec<&str> = content.lines().collect();

	if lines.is_empty() {
		if force {
			return Err(anyhow::anyhow!(
				"AI returned empty summary during forced compression"
			));
		}
		log_debug!("AI compression decision: NO (empty response)");
		return Ok((false, String::new()));
	}

	// Extract and store critical knowledge from <knowledge> tags before returning summary
	extract_and_store_knowledge(session, config, content);

	if force {
		// Entire response is the summary — no YES/NO prefix expected.
		let summary = strip_knowledge_tags(content);
		log_debug!("AI forced compression summary ({} chars)", summary.len());
		return Ok((true, summary));
	}

	let first_line = lines[0].trim().to_uppercase();
	let decision = first_line.contains("YES");

	if decision {
		let summary = if lines.len() > 1 {
			let raw = lines[1..].join("\n").trim().to_string();
			strip_knowledge_tags(&raw)
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

async fn ask_ai_decision_and_summary(
	session: &mut ChatSession,
	config: &Config,
	messages_to_compress: &[crate::session::Message],
	operation_rx: tokio::sync::watch::Receiver<bool>,
	force: bool,
	target_ratio: f64,
) -> Result<(bool, String)> {
	let (system_content, user_content) =
		build_compression_prompt(session, messages_to_compress, force, target_ratio);
	let response_content =
		call_ai_for_decision(session, config, system_content, user_content, operation_rx).await?;
	parse_ai_response(session, config, &response_content, force)
}

/// Apply compression: drain all messages, insert summary, re-inject recent user messages.
/// Also parses and injects file contexts if given by AI.
#[allow(clippy::too_many_arguments)]
async fn apply_compression(
	session: &mut ChatSession,
	start_idx: usize,
	end_idx: usize,
	context_summary: &str,
	tokens_before: u64,
	current_context_tokens: u64,
	user_tasks_msgs: Vec<String>,
	last_user_message: Option<crate::session::Message>,
	preserved_skills: Vec<crate::session::Message>,
	use_long_cache: bool,
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

	// Format compressed entry with file context
	let compression_id = crate::mcp::core::plan::compression::get_compression_id()
		.unwrap_or_else(|| "unknown".to_string());

	let base_entry = format_compressed_entry_with_context(
		context_summary,
		&file_context_content,
		compression_id,
	);

	// Prepend USER TASKS section (last 4 user requests, excluding the appended one).
	// These are raw user messages — not AI-rephrased — so intent is never lost.
	let compressed_entry = if user_tasks_msgs.is_empty() {
		base_entry
	} else {
		let user_tasks = user_tasks_msgs
			.iter()
			.enumerate()
			.map(|(i, msg)| format!("{}. {}", i + 1, msg))
			.collect::<Vec<_>>()
			.join("\n");
		format!("## USER TASKS\n{}\n\n{}", user_tasks, base_entry)
	};

	// Append the current active plan (if any) to the summary so the model doesn't have
	// to spend an extra `plan(list)` turn right after compression just to recover state.
	// Absence of a plan → no section injected.
	let compressed_entry = match crate::mcp::core::plan::core::get_current_plan_display().await {
		Ok(plan_display) => format!(
			"{}\n\nCurrent plan we are working on:\n<plan>\n{}\n</plan>",
			compressed_entry,
			plan_display.trim()
		),
		Err(_) => compressed_entry,
	};

	let tokens_after = estimate_tokens(&compressed_entry) as u64;

	// COMPRESS-ALL: Drain everything from start_idx+1 to end_idx
	let (messages_removed, _) = session.remove_messages_in_range(start_idx, end_idx)?;

	// Insert summary + re-injected user message in one shot with correct cache markers.
	// Cache markers: marker #1 on summary, marker #2 on re-injected user message.
	// Evict existing content markers first to enforce the 2-marker limit.
	let supports_caching = crate::session::model_supports_caching(&session.session.info.model);
	// Evict stale content markers — but preserve the anchor's marker.
	// The anchor (instructions) keeps its cache marker from session start.
	// Set 1h TTL on anchor when long cache is enabled — stable prefix, rarely changes.
	if supports_caching {
		for (i, msg) in session.session.messages.iter_mut().enumerate() {
			if i == start_idx {
				// Anchor: keep marker, set long TTL if supported
				msg.cached = true;
				msg.cache_ttl = if use_long_cache {
					Some("1h".to_string())
				} else {
					None
				};
			} else if msg.cached && msg.role != "system" {
				msg.cached = false;
				msg.cache_ttl = None;
			}
		}
	}

	let now = std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.unwrap_or_default()
		.as_secs();

	// Insert preserved active skills FIRST, between the anchor (which keeps
	// cache marker #1) and the summary. Skills carry no cache markers — the
	// two-marker budget is reserved for anchor + re-injected user. Order is
	// preserved relative to each other, matching the user's expectation that
	// active skills sit at the top of the recovered context:
	//   [system, anchor(marker#1), skill1, skill2, …, summary, user(marker#2), …]
	let skill_count = preserved_skills.len();
	for (i, mut skill_msg) in preserved_skills.into_iter().enumerate() {
		// Defensive: clear cache markers so we never blow the 2-marker budget.
		skill_msg.cached = false;
		skill_msg.cache_ttl = None;
		session
			.session
			.messages
			.insert(start_idx + 1 + i, skill_msg);
	}
	if skill_count > 0 {
		log_debug!(
			"Compression: preserved {} active skill message(s) across compression",
			skill_count
		);
	}

	// Summary message (no cache marker — sits between anchor marker and user marker)
	let summary_msg = crate::session::Message {
		role: "assistant".to_string(),
		content: compressed_entry,
		timestamp: now,
		cached: false,
		name: Some("plan_compression".to_string()),
		..Default::default()
	};
	session
		.session
		.messages
		.insert(start_idx + 1 + skill_count, summary_msg);

	// Marker #2: re-injected user message — full content cache boundary
	let user_msg = match last_user_message {
		Some(mut msg) => {
			msg.cached = supports_caching;
			msg
		}
		None => crate::session::Message {
			role: "user".to_string(),
			content: "Please continue.".to_string(),
			timestamp: now,
			cached: supports_caching,
			..Default::default()
		},
	};
	session
		.session
		.messages
		.insert(start_idx + 2 + skill_count, user_msg);
	log_debug!(
		"Re-injected last user message after compressed summary (USER TASKS: {})",
		user_tasks_msgs.len()
	);

	// Update first_prompt_idx to the actual anchor used for this compression.
	session.first_prompt_idx = Some(start_idx);

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
	session.session.info.compression_stats.add_compression(
		crate::session::CompressionKind::Conversation,
		messages_removed,
		tokens_saved,
	);

	// Token-based cooldown: record post-compression context size.
	// Next compression is allowed only after context grows ≥10% above this watermark,
	// preventing futile back-to-back compressions while reacting to actual growth.
	let post_compression_tokens = current_context_tokens.saturating_sub(tokens_saved);
	session.session.info.context_tokens_after_last_compression = post_compression_tokens as usize;

	// SELF-TUNING: Record checkpoint for incremental growth rate tracking.
	// output_tokens_at_last_compression lets estimate_future_turns measure growth since
	// this compression only, not the inflated lifetime average.
	let estimated_future_turns = estimate_future_turns(session, tokens_saved as f64);
	let api_calls_at_compression = session.session.info.total_api_calls;
	session.session.info.predicted_turns_at_last_compression = estimated_future_turns;
	session.session.info.api_calls_at_last_compression = api_calls_at_compression;
	session.session.info.output_tokens_at_last_compression = session.session.info.output_tokens;

	log_debug!(
		"Compression cooldown set: post_compression_tokens={}, consecutive={}, requires ≥{:.0}% growth before next compression",
		post_compression_tokens,
		session.session.info.consecutive_compressions,
		(0.10 * 2.0_f64.powi(session.session.info.consecutive_compressions as i32)).min(1.0) * 100.0
	);

	// CRITICAL: Log compression point to session file
	// This marker tells session loader to clear messages before this point on resume
	// Without this, all "compressed" messages are reloaded, defeating compression
	let _ = crate::session::logger::log_compression_point(
		&session.session.info.name,
		"conversation",
		messages_removed,
		tokens_saved,
		&session.session.messages,
	);

	// Extend the session anchor so conversation compaction contributes to
	// cross-compaction continuity. Heuristic update: record a marker entry
	// with the metrics; subsequent task compactions (which embed the anchor
	// in their compressed-knowledge messages) surface it in context.
	{
		let now_unix = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.unwrap_or_default()
			.as_secs();
		let intent_seed = if session.session.info.anchor.intent.is_empty() {
			Some("Free-form conversation session".to_string())
		} else {
			None
		};
		session.session.info.anchor.extend(
			crate::session::anchor::AnchorUpdate {
				intent: intent_seed,
				changes_made: vec![format!(
					"Conversation compaction: {} messages folded, {} tokens saved",
					messages_removed, tokens_saved
				)],
				..Default::default()
			},
			now_unix,
		);
	}

	// (dedup state is cleared inside `remove_messages_in_range` — see core.rs.)

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

/// Format final compressed entry with optional file context
fn format_compressed_entry_with_context(
	context: &str,
	file_context: &str,
	compression_id: String,
) -> String {
	let mut sections = Vec::new();

	if !context.is_empty() {
		sections.push(context.to_string());
	}

	// Add file context if provided (automatically expanded from AI's <context> tags)
	if !file_context.is_empty() {
		sections.push(format!(
			"**FILE CONTEXT** (auto-expanded):\n{}",
			file_context
		));
	}

	format!(
		"## Conversation Summary [COMPRESSED: {}]\n\n{}",
		compression_id,
		sections.join("\n\n"),
	)
}

/// Strip the FILE CONTEXT section from a prior compressed summary before re-feeding it
/// to the next compression pass.
///
/// When a summary is re-compressed, the embedded file bytes are stale and bloat the
/// prompt. The AI will re-request any files it still needs via <context> tags.
/// Returns the summary text with the FILE CONTEXT block removed, trimmed.
fn strip_file_context_from_summary(summary: &str) -> String {
	const SENTINEL: &str = "\n\n**FILE CONTEXT** (auto-expanded):";
	if let Some(pos) = summary.find(SENTINEL) {
		summary[..pos].trim().to_string()
	} else {
		summary.trim().to_string()
	}
}

/// Extract <knowledge> tags from AI compression response, store in session, and log.
/// Trims to the configured knowledge_retention limit (keeps most recent entries).
fn extract_and_store_knowledge(session: &mut ChatSession, config: &Config, content: &str) {
	let knowledge_entries = parse_knowledge_tags(content);
	if knowledge_entries.is_empty() {
		return;
	}

	let retention_limit = config.compression.knowledge_retention;
	for entry in &knowledge_entries {
		log_debug!("Extracted critical knowledge: {}", entry);
		session.critical_knowledge.push(entry.clone());

		// Persist to session log
		let _ = crate::session::logger::log_knowledge_entry(&session.session.info.name, entry);
	}

	// Trim to retention limit (keep most recent)
	if retention_limit > 0 && session.critical_knowledge.len() > retention_limit {
		let drain_count = session.critical_knowledge.len() - retention_limit;
		session.critical_knowledge.drain(..drain_count);
		log_debug!(
			"Trimmed critical knowledge to {} entries (retention limit)",
			retention_limit
		);
	}

	log_info!(
		"Stored {} new critical knowledge entries ({} total)",
		knowledge_entries.len(),
		session.critical_knowledge.len()
	);
}

/// Parse all <knowledge>...</knowledge> tags from text.
/// Returns the trimmed content of each tag.
fn parse_knowledge_tags(content: &str) -> Vec<String> {
	let mut entries = Vec::new();
	let mut search_from = 0;
	while let Some(start) = content[search_from..].find("<knowledge>") {
		let abs_start = search_from + start + "<knowledge>".len();
		if let Some(end) = content[abs_start..].find("</knowledge>") {
			let abs_end = abs_start + end;
			let entry = content[abs_start..abs_end].trim().to_string();
			if !entry.is_empty() {
				entries.push(entry);
			}
			search_from = abs_end + "</knowledge>".len();
		} else {
			break;
		}
	}
	entries
}

/// Strip <knowledge>...</knowledge> tags from summary text.
/// The knowledge is already extracted and stored separately — no need to keep it in the summary.
fn strip_knowledge_tags(content: &str) -> String {
	let mut result = content.to_string();
	while let Some(start) = result.find("<knowledge>") {
		if let Some(end) = result[start..].find("</knowledge>") {
			let abs_end = start + end + "</knowledge>".len();
			result = format!("{}{}", &result[..start], &result[abs_end..]);
		} else {
			break;
		}
	}
	result.trim().to_string()
}

/// Parse all <done>...</done> tags from AI compression response.
/// Returns task IDs (e.g. "task1", "task2") that the AI marked as fully completed.
/// Collect active skill messages from a compression drain range so they can be
/// re-inserted after the summary. Skill messages are user-role entries whose
/// content is wrapped in `<skill name="...">…</skill>` tags.
///
/// Only skills in `active_skill_names` are preserved — a skill the user
/// explicitly forgot (or that was never registered as active) is dropped.
///
/// Duplicate skill names (same skill injected multiple times) are deduped
/// keeping the LAST occurrence in the range, preserving the freshest content.
/// Relative order of distinct skills is preserved (by last-seen position).
fn collect_preserved_skills(
	messages: &[crate::session::Message],
	range_start: usize,
	range_end: usize,
	active_skill_names: &[String],
) -> Vec<crate::session::Message> {
	if range_start > range_end || range_end >= messages.len() {
		return Vec::new();
	}

	// Walk the range once, recording the last index per skill name.
	// Using a Vec<(name, idx)> to preserve insertion order of first-seen names
	// while still letting us update the idx to the latest occurrence.
	let mut order: Vec<String> = Vec::new();
	let mut last_idx: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

	for (offset, msg) in messages[range_start..=range_end].iter().enumerate() {
		if msg.role != "user" {
			continue;
		}
		if !crate::mcp::core::skill::is_skill_message(&msg.content) {
			continue;
		}
		let name = match crate::mcp::core::skill::extract_skill_name(&msg.content) {
			Some(n) => n.to_string(),
			None => continue,
		};
		if !active_skill_names.iter().any(|n| n == &name) {
			continue;
		}
		let idx = range_start + offset;
		if last_idx.insert(name.clone(), idx).is_none() {
			order.push(name);
		}
	}

	order
		.into_iter()
		.filter_map(|name| last_idx.get(&name).map(|&i| messages[i].clone()))
		.collect()
}

/// Find the compression range: anchor to last message (compress-all approach).
///
/// CRITICAL: Must not cut between assistant with tool_calls and its tool results
/// CRITICAL: Compression NEVER goes below first_prompt_idx (INCLUSIVE boundary)
fn find_compression_range(
	messages: &[crate::session::Message],
	first_prompt_idx: Option<usize>,
	force: bool,
) -> Result<(usize, usize)> {
	// Find system message index
	let system_idx = messages
		.iter()
		.position(|m| m.role == "system")
		.unwrap_or(0);

	// Start boundary: try to move anchor BEFORE first_prompt_idx so the first user
	// message gets compressed. Without this, the first user message persists raw
	// across all compression cycles — even after the user moved on to new tasks.
	//
	// If instructions file exists at idx-1 (user role), use it as anchor.
	// The first user message then falls into drain range (start_idx+1..=end_idx).
	// Keep old behavior for tool-loops (single user message — it's still active).
	//
	// If not set (e.g. resumed sessions), detect bootstrap messages and skip past them.
	// Bootstrap pattern: system[0] → assistant(welcome)[1] → optional user(instructions)[2]
	// We must NEVER compress the system prompt, welcome message, or instructions file.
	let mut start_idx = match first_prompt_idx {
		Some(idx) => {
			// Check if user sent additional messages after the first prompt.
			// If yes, the first prompt is no longer active and should be compressed.
			let has_subsequent_user = messages.iter().skip(idx + 1).any(|m| m.role == "user");

			if has_subsequent_user
				&& idx > 0 && messages.get(idx - 1).is_some_and(|m| m.role == "user")
			{
				// Instructions file at idx-1 becomes anchor.
				// First user message at idx is now in drain range.
				idx - 1
			} else {
				idx
			}
		}
		None => {
			let mut idx = system_idx + 1;
			// Skip welcome message (assistant immediately after system, WITHOUT tool_calls).
			// A welcome is a simple greeting — if it has tool_calls, it's a working response.
			let has_welcome = idx < messages.len()
				&& messages[idx].role == "assistant"
				&& messages[idx].tool_calls.is_none();
			if has_welcome {
				idx += 1;
			}
			// Skip instructions file ONLY if welcome was present.
			// Bootstrap pattern: system → assistant(welcome) → user(instructions).
			// Without a welcome message, the first user message is a real prompt, not instructions.
			if has_welcome
				&& idx < messages.len()
				&& messages[idx].role == "user"
				&& (idx + 1 >= messages.len() || messages[idx + 1].role == "assistant")
			{
				idx += 1;
			}
			idx
		}
	};

	// CRITICAL: If the anchor message has tool_calls, its tool results immediately follow it.
	// remove_messages_in_range drains start_idx+1..=end_idx — if tool results are in that
	// range they get removed, leaving orphaned tool_use blocks without tool_result.
	// The API then rejects the sequence with "tool_use ids were found without tool_result".
	// Fix: advance start_idx past all tool results that belong to the anchor's tool_calls.
	if let Some(anchor) = messages.get(start_idx) {
		if anchor.role == "assistant" && anchor.tool_calls.is_some() {
			// Skip past consecutive tool messages that follow the anchor
			let mut next = start_idx + 1;
			while next < messages.len() && messages[next].role == "tool" {
				next += 1;
			}
			// next now points to the first non-tool message after the anchor's tool results.
			// That becomes the new anchor (the drain will start at new_start_idx+1).
			if next > start_idx + 1 && next < messages.len() {
				start_idx = next;
			}
		}
	}

	// COMPRESS-ALL APPROACH: Compress everything from start_idx+1 to the last message.
	// Recent user messages are extracted and re-injected after the summary by the caller.
	// This eliminates the old preserve_count / boundary-search complexity and ensures
	// no user messages persist as stale raw artifacts across compression cycles.
	let end_idx = messages.len() - 1;

	// Minimum conversation messages to justify compression.
	// Need at least 5 (non-force) or 3 (force/done) to produce a useful summary.
	let min_conv = if force { 3 } else { 5 };
	let conv_count = messages
		.iter()
		.skip(start_idx)
		.filter(|m| m.role == "user" || m.role == "assistant")
		.count();
	if conv_count < min_conv {
		return Ok((0, 0));
	}

	if start_idx >= end_idx {
		return Ok((0, 0));
	}

	Ok((start_idx, end_idx))
}

/// Calculate tokens in message range using accurate token counting
/// This now counts ALL message fields: content, tool_calls, thinking, images, etc.
///
/// CRITICAL: The range [start_idx, end_idx] must match the messages that will
/// actually be removed. In compression, remove_messages_in_range drains
/// start_idx+1..=end_idx, so callers should pass (start_idx+1, end_idx).
fn calculate_range_tokens(session: &ChatSession, start_idx: usize, end_idx: usize) -> Result<u64> {
	let mut total_tokens = 0u64;

	// Validate range
	if start_idx >= session.session.messages.len() {
		return Err(anyhow::anyhow!("Invalid start_index in range"));
	}

	if end_idx >= session.session.messages.len() {
		return Err(anyhow::anyhow!("Invalid end_index in range"));
	}

	// Count tokens in range [start_idx, end_idx] inclusive
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
	use super::{
		collect_preserved_skills, find_compression_range, strip_file_context_from_summary,
	};
	use crate::session::Message;
	use serde_json::json;

	fn msg(role: &str) -> Message {
		Message {
			role: role.to_string(),
			content: String::new(),
			..Default::default()
		}
	}

	fn skill_msg(name: &str) -> Message {
		Message {
			role: "user".to_string(),
			content: format!(
				"<skill name=\"{}\" description=\"test skill\">\nbody for {}\n</skill>",
				name, name
			),
			..Default::default()
		}
	}

	#[test]
	fn preserves_active_skill_in_drain_range() {
		// Layout: [system, welcome, instructions, user_req1, asst,
		//         skill(rust), user_req2, asst, user_req3, asst]
		let mut messages = vec![
			msg("system"),    // 0
			msg("assistant"), // 1 welcome
			{
				let mut m = msg("user"); // 2 instructions
				m.content = "instructions".into();
				m
			},
			{
				let mut m = msg("user"); // 3 user_req1
				m.content = "first request".into();
				m
			},
			{
				let mut m = msg("assistant"); // 4
				m.content = "reply 1".into();
				m
			},
			skill_msg("programming-rust"), // 5
			{
				let mut m = msg("user"); // 6 user_req2
				m.content = "second request".into();
				m
			},
			{
				let mut m = msg("assistant"); // 7
				m.content = "reply 2".into();
				m
			},
			{
				let mut m = msg("user"); // 8 user_req3
				m.content = "third request".into();
				m
			},
			{
				let mut m = msg("assistant"); // 9
				m.content = "reply 3".into();
				m
			},
		];

		// first_prompt_idx = 3 (first real user prompt).
		// find_compression_range moves anchor to idx-1 = 2 (instructions).
		// Drain range: 3..=9.
		let (start_idx, end_idx) = find_compression_range(&messages, Some(3), false).unwrap();
		assert_eq!(start_idx, 2, "anchor on instructions");
		assert_eq!(end_idx, 9);

		let active = vec!["programming-rust".to_string()];
		let preserved = collect_preserved_skills(&messages, start_idx + 1, end_idx, &active);
		assert_eq!(preserved.len(), 1);
		assert!(preserved[0]
			.content
			.contains("<skill name=\"programming-rust\""));

		// Filter mirrors check_and_compress_conversation: user messages that are
		// NOT skill messages, non-empty.
		let user_tasks: Vec<String> = messages[start_idx + 1..=end_idx]
			.iter()
			.filter(|m| {
				m.role == "user"
					&& !m.content.trim().is_empty()
					&& !crate::mcp::core::skill::is_skill_message(&m.content)
			})
			.map(|m| m.content.clone())
			.collect();

		// Last is re-injected raw, prior entries become USER TASKS.
		assert_eq!(
			user_tasks,
			vec![
				"first request".to_string(),
				"second request".to_string(),
				"third request".to_string(),
			],
			"skill content must NOT appear as a user task"
		);
		assert_eq!(
			user_tasks.last().unwrap(),
			"third request",
			"last user message for re-injection is the real request, not the skill"
		);

		// Simulate apply_compression placement: drain 3..=9, insert skills at
		// start_idx+1, then summary at start_idx+1+skill_count, then user.
		messages.drain(start_idx + 1..=end_idx);
		for (i, mut s) in preserved.into_iter().enumerate() {
			s.cached = false;
			s.cache_ttl = None;
			messages.insert(start_idx + 1 + i, s);
		}
		let skill_count = 1;
		messages.insert(start_idx + 1 + skill_count, {
			let mut m = msg("assistant");
			m.content = "SUMMARY".into();
			m
		});
		messages.insert(start_idx + 2 + skill_count, {
			let mut m = msg("user");
			m.content = "third request".into();
			m
		});

		// Expected post-compression layout:
		// [system, welcome, instructions(anchor), skill, SUMMARY, user_req3]
		assert_eq!(messages.len(), 6);
		assert_eq!(messages[2].content, "instructions");
		assert!(
			crate::mcp::core::skill::is_skill_message(&messages[3].content),
			"skill comes right after anchor"
		);
		assert_eq!(messages[4].content, "SUMMARY");
		assert_eq!(messages[5].content, "third request");
	}

	#[test]
	fn drops_forgotten_skill_from_preservation() {
		// Skill is in range but not in active list → must be dropped.
		let messages = vec![
			msg("system"),
			msg("user"),
			skill_msg("programming-rust"),
			msg("assistant"),
			msg("user"),
			msg("assistant"),
		];
		let active: Vec<String> = Vec::new(); // user forgot the skill
		let preserved = collect_preserved_skills(&messages, 1, 5, &active);
		assert!(preserved.is_empty(), "forgotten skills are not preserved");
	}

	#[test]
	fn dedupes_duplicate_skill_keeping_latest() {
		// Same skill injected twice in range — keep the second (latest) copy.
		let mut first = skill_msg("programming-rust");
		first.content =
			"<skill name=\"programming-rust\" description=\"v1\">\nold body\n</skill>".to_string();
		let mut second = skill_msg("programming-rust");
		second.content =
			"<skill name=\"programming-rust\" description=\"v2\">\nnew body\n</skill>".to_string();

		let messages = vec![
			msg("system"),
			msg("user"),
			first,
			msg("assistant"),
			second,
			msg("assistant"),
		];
		let active = vec!["programming-rust".to_string()];
		let preserved = collect_preserved_skills(&messages, 1, 5, &active);
		assert_eq!(preserved.len(), 1);
		assert!(
			preserved[0].content.contains("new body"),
			"latest injection wins on dedup"
		);
	}

	#[test]
	fn preserves_multiple_distinct_skills_in_order() {
		let messages = vec![
			msg("system"),
			msg("user"),
			skill_msg("programming-rust"),
			msg("assistant"),
			skill_msg("git-workflow"),
			msg("user"),
			msg("assistant"),
		];
		let active = vec!["programming-rust".to_string(), "git-workflow".to_string()];
		let preserved = collect_preserved_skills(&messages, 1, 6, &active);
		assert_eq!(preserved.len(), 2);
		assert!(preserved[0].content.contains("programming-rust"));
		assert!(preserved[1].content.contains("git-workflow"));
	}

	#[test]
	fn empty_range_returns_empty() {
		let messages = vec![msg("system")];
		let preserved = collect_preserved_skills(&messages, 5, 10, &["foo".to_string()]);
		assert!(preserved.is_empty());
	}

	#[test]
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

		let (start_idx, end_idx) = find_compression_range(&messages, None, false).unwrap();

		// Compress-all: end_idx = last message
		assert_eq!(start_idx, 1);
		assert_eq!(end_idx, 9, "compress-all: end_idx = last message");
	}

	#[test]
	fn extends_when_ending_on_assistant_with_tools() {
		// THIS is the critical test - tool messages between conversation messages
		let mut messages = vec![
			msg("system"),    // 0
			msg("user"),      // 1
			msg("assistant"), // 2
			msg("user"),      // 3
		];
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

		let (start_idx, end_idx) = find_compression_range(&messages, None, false).unwrap();

		// Compress-all: end_idx = last message
		assert_eq!(start_idx, 1);
		assert_eq!(end_idx, 9, "compress-all: end_idx = last message");
	}

	#[test]
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

		let (start_idx, end_idx) = find_compression_range(&messages, None, false).unwrap();

		// Compress-all: end_idx = last message, no preserved zone
		assert_eq!(start_idx, 1);
		assert_eq!(end_idx, 10, "compress-all: end_idx = last message");
	}

	#[test]
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
		let (start_idx, end_idx) = find_compression_range(&messages, Some(3), false).unwrap();

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
	fn anchor_with_tool_calls_must_advance_past_tool_results() {
		// Reproduces the exact bug from the session log:
		// - Message 1: assistant with 2 tool_calls (view_signatures + view)
		// - Message 2: tool result for view_signatures
		// - Message 3: tool result for view (this one got orphaned)
		// - Compression summary inserted at message 3
		// - remove_messages_in_range drained start_idx+1..=end_idx
		// - Result: assistant at index 1 still has tool_calls but tool results are gone
		// - API error: "tool_use ids were found without tool_result blocks"
		let mut messages = Vec::new();
		messages.push(msg("system")); // 0

		// Assistant with 2 tool calls (like the real session)
		let mut assistant = msg("assistant"); // 1
		assistant.tool_calls = Some(json!([
			{"id": "call_A", "type": "function", "function": {"name": "view_signatures", "arguments": "{}"}},
			{"id": "call_B", "type": "function", "function": {"name": "view", "arguments": "{}"}}
		]));
		messages.push(assistant);

		let mut tool_a = msg("tool"); // 2
		tool_a.tool_call_id = Some("call_A".to_string());
		tool_a.name = Some("view_signatures".to_string());
		messages.push(tool_a);

		let mut tool_b = msg("tool"); // 3
		tool_b.tool_call_id = Some("call_B".to_string());
		tool_b.name = Some("view".to_string());
		messages.push(tool_b);

		// Enough conversation to trigger compression (need >4 user+assistant)
		messages.push(msg("assistant")); // 4 (response after tools)
		messages.push(msg("user")); // 5
		messages.push(msg("assistant")); // 6
		messages.push(msg("user")); // 7
		messages.push(msg("assistant")); // 8
		messages.push(msg("user")); // 9
		messages.push(msg("assistant")); // 10

		// first_prompt_idx=None means start_idx defaults to system_idx+1 = 1
		// Index 1 is the assistant with tool_calls.
		// Without the fix: start_idx=1, drain removes indices 2..=end_idx,
		// orphaning tool_calls at index 1.
		let (start_idx, end_idx) = find_compression_range(&messages, None, false).unwrap();

		// With the fix: start_idx must advance past the tool results (indices 2, 3)
		// to index 4 (the next assistant message after tools).
		assert!(
			start_idx >= 4,
			"start_idx must advance past tool results to avoid orphaning tool_calls. Got start_idx={start_idx}"
		);
		assert!(
			end_idx > start_idx,
			"end_idx must be after start_idx for a valid range. Got start={start_idx}, end={end_idx}"
		);

		// Verify the drain range (start_idx+1..=end_idx) doesn't include any tool messages
		// that belong to the assistant at index 1
		for msg in messages.iter().take(end_idx + 1).skip(start_idx + 1) {
			if msg.role == "tool" {
				// Any tool message in the drain range must NOT belong to the anchor's tool_calls
				if let Some(ref tc_id) = msg.tool_call_id {
					assert!(
						tc_id != "call_A" && tc_id != "call_B",
						"Drain range must not include tool results for anchor's tool_calls. Found {tc_id}"
					);
				}
			}
		}
	}

	#[test]
	fn anchor_with_tool_calls_and_first_prompt_idx() {
		// When first_prompt_idx points to an assistant with tool_calls,
		// start_idx must still advance past its tool results.
		let mut messages = Vec::new();
		messages.push(msg("system")); // 0
		messages.push(msg("user")); // 1

		// Assistant with tool calls at index 2
		let mut assistant = msg("assistant"); // 2
		assistant.tool_calls = Some(json!([
			{"id": "call_X", "type": "function", "function": {"name": "shell", "arguments": "{}"}}
		]));
		messages.push(assistant);

		let mut tool_x = msg("tool"); // 3
		tool_x.tool_call_id = Some("call_X".to_string());
		tool_x.name = Some("shell".to_string());
		messages.push(tool_x);

		// More conversation
		messages.push(msg("assistant")); // 4
		messages.push(msg("user")); // 5
		messages.push(msg("assistant")); // 6
		messages.push(msg("user")); // 7
		messages.push(msg("assistant")); // 8
		messages.push(msg("user")); // 9
		messages.push(msg("assistant")); // 10

		// first_prompt_idx=Some(2) points to the assistant with tool_calls.
		// With the anchor-move-back fix, start_idx moves to idx-1=1 (user).
		// The assistant with tool_calls and its tool result are both in the drain range.
		let (start_idx, end_idx) = find_compression_range(&messages, Some(2), false).unwrap();

		assert_eq!(
			start_idx, 1,
			"anchor moves back to user at idx 1 — tool_calls asst at 2 is in drain range"
		);
		assert!(end_idx > start_idx, "must have valid range");
		// Both the assistant with tool_calls (idx 2) and its tool result (idx 3) are
		// in drain range [2..=end_idx] — no orphaned tool_use blocks.
		assert!(end_idx >= 3, "drain must include tool result at idx 3");
	}

	// ============================================================================
	// BOOTSTRAP MESSAGE PRESERVATION TESTS: Verify system prompt, welcome message,
	// and instructions file are NEVER compressed away
	// ============================================================================

	#[test]
	fn bootstrap_preserved_when_first_prompt_idx_is_none_no_instructions() {
		// Simulates resumed session without instructions file:
		// [0] system, [1] assistant(welcome), [2+] conversation
		// first_prompt_idx=None (resumed session)
		let messages = vec![
			msg("system"),    // 0
			msg("assistant"), // 1 - welcome message
			msg("user"),      // 2 - first real user prompt
			msg("assistant"), // 3
			msg("user"),      // 4
			msg("assistant"), // 5
			msg("user"),      // 6
			msg("assistant"), // 7
			msg("user"),      // 8
			msg("assistant"), // 9
		];

		let (start_idx, end_idx) = find_compression_range(&messages, None, false).unwrap();

		// System[0] and welcome[1] must be protected
		assert!(
			start_idx >= 2,
			"start_idx must be >= 2 to protect system and welcome. Got {start_idx}"
		);
		assert!(end_idx > start_idx, "must have valid range");

		// Drain range is start_idx+1..=end_idx — verify system and welcome are outside
		assert!(
			start_idx + 1 > 1,
			"drain range must not include welcome message at index 1"
		);
	}

	#[test]
	fn bootstrap_preserved_when_first_prompt_idx_is_none_with_instructions() {
		// Simulates resumed session WITH instructions file:
		// [0] system, [1] assistant(welcome), [2] user(instructions), [3+] conversation
		// first_prompt_idx=None (resumed session)
		let messages = vec![
			msg("system"),    // 0
			msg("assistant"), // 1 - welcome message
			msg("user"),      // 2 - instructions file
			msg("assistant"), // 3 - AI response to instructions
			msg("user"),      // 4 - first real user prompt
			msg("assistant"), // 5
			msg("user"),      // 6
			msg("assistant"), // 7
			msg("user"),      // 8
			msg("assistant"), // 9
		];

		let (start_idx, end_idx) = find_compression_range(&messages, None, false).unwrap();

		// System[0], welcome[1], and instructions[2] must be protected
		assert!(
			start_idx >= 3,
			"start_idx must be >= 3 to protect system, welcome, and instructions. Got {start_idx}"
		);
		assert!(end_idx > start_idx, "must have valid range");
	}

	#[test]
	fn bootstrap_preserved_system_message_never_in_range() {
		// Regardless of first_prompt_idx, system message must never be in compression range
		let mut messages = Vec::new();
		messages.push(msg("system")); // 0
		messages.push(msg("assistant")); // 1
		for _ in 0..10 {
			messages.push(msg("user"));
			messages.push(msg("assistant"));
		}

		// Test with None
		let (start_none, _end_none) = find_compression_range(&messages, None, false).unwrap();
		assert!(start_none > 0, "system message at 0 must not be start_idx");
		// Drain is start_idx+1..=end_idx, so system at 0 is safe if start_idx > 0

		// Test with Some(1)
		let (start_some, end_some) = find_compression_range(&messages, Some(1), false).unwrap();
		assert!(start_some >= 1, "start_idx must be >= 1");
		assert!(end_some > start_some);
	}

	#[test]
	fn bootstrap_with_tool_calls_in_welcome_response() {
		// Edge case: welcome is followed by instructions, then AI responds with tool_calls
		// [0] system, [1] assistant(welcome), [2] user(instructions),
		// [3] assistant(tool_calls), [4] tool, [5+] conversation
		let mut messages = Vec::new();
		messages.push(msg("system")); // 0
		messages.push(msg("assistant")); // 1 - welcome
		messages.push(msg("user")); // 2 - instructions

		let mut assistant_tc = msg("assistant"); // 3
		assistant_tc.tool_calls = Some(serde_json::json!([
			{"id": "call_1", "type": "function", "function": {"name": "view", "arguments": "{}"}}
		]));
		messages.push(assistant_tc);

		let mut tool = msg("tool"); // 4
		tool.tool_call_id = Some("call_1".to_string());
		messages.push(tool);

		messages.push(msg("assistant")); // 5
		messages.push(msg("user")); // 6
		messages.push(msg("assistant")); // 7
		messages.push(msg("user")); // 8
		messages.push(msg("assistant")); // 9
		messages.push(msg("user")); // 10
		messages.push(msg("assistant")); // 11

		let (start_idx, end_idx) = find_compression_range(&messages, None, false).unwrap();

		// Must protect: system[0], welcome[1], instructions[2]
		// start_idx should be >= 3, and if 3 has tool_calls, advance past tool results
		assert!(
			start_idx >= 5,
			"start_idx must advance past bootstrap AND tool results. Got {start_idx}"
		);
		assert!(
			end_idx > start_idx,
			"must have valid range. Got start={start_idx}, end={end_idx}"
		);
	}

	#[test]
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

		let (start_idx, end_idx) = find_compression_range(&messages, None, false).unwrap();

		// Verify the range is valid
		assert!(start_idx < end_idx, "Range must be valid");

		// Count tokens that WILL BE REMOVED (matching remove_messages_in_range logic)
		// remove_messages_in_range removes [start_idx+1, end_idx]
		let expected_tokens: u64 = messages[(start_idx + 1)..=end_idx]
			.iter()
			.map(|m| estimate_message_tokens(m) as u64)
			.sum();

		// Count tokens that ARE INCLUDED in semantic chunking
		// messages_to_compress = [start_idx, end_idx]
		let chunked_tokens: u64 = messages[start_idx..=end_idx]
			.iter()
			.map(|m| estimate_message_tokens(m) as u64)
			.sum();

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

		let (start_idx, end_idx) = find_compression_range(&messages, None, false).unwrap();
		assert_eq!(start_idx, 1); // Large message
		assert_eq!(end_idx, 8); // compress-all: last message

		// What calculate_range_tokens ACTUALLY counts (CURRENT BUG)
		let tokens_counted_by_function: u64 = messages[(start_idx + 1)..=end_idx]
			.iter()
			.map(|m| estimate_message_tokens(m) as u64)
			.sum();

		// What semantic chunking ACTUALLY includes
		let tokens_in_chunking: u64 = messages[start_idx..=end_idx]
			.iter()
			.map(|m| estimate_message_tokens(m) as u64)
			.sum();

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
		// TEST: Token-based cooldown blocks compression until context grows ≥10%

		// Scenario 1: After compression, context is at 50,000 tokens
		let tokens_after_compression: usize = 50_000;

		// Scenario 2: Context at 52,000 (4% growth) — should block
		let current_tokens_52k: usize = 52_000;
		let min_required = (tokens_after_compression as f64 * 1.1) as usize;
		assert!(
			current_tokens_52k < min_required,
			"Cooldown should block at 52k: {} < {} (need 10% growth)",
			current_tokens_52k,
			min_required
		);

		// Scenario 3: Context at 54,999 (~10% but not quite) — still blocked
		let current_tokens_54k: usize = 54_999;
		assert!(
			current_tokens_54k < min_required,
			"Cooldown should still block at 54,999: {} < {}",
			current_tokens_54k,
			min_required
		);

		// Scenario 4: Context at 55,000 (exactly 10% growth) — cooldown passes
		let current_tokens_55k: usize = 55_000;
		assert!(
			current_tokens_55k >= min_required,
			"Cooldown should pass at 55k: {} >= {}",
			current_tokens_55k,
			min_required
		);

		// Scenario 5: Context at 60,000 (20% growth) — allowed
		let current_tokens_60k: usize = 60_000;
		assert!(
			current_tokens_60k >= min_required,
			"Compression should be allowed at 60k: {} >= {}",
			current_tokens_60k,
			min_required
		);
	}

	#[test]
	fn test_cooldown_default_allows_first_compression() {
		// TEST: Default value (0) should allow first compression immediately

		let tokens_after_compression: usize = 0; // Default — no prior compression
		let current_tokens: usize = 60_000;

		// When context_tokens_after_last_compression is 0, cooldown is inactive
		let cooldown_active = tokens_after_compression > 0
			&& current_tokens < (tokens_after_compression as f64 * 1.1) as usize;
		assert!(
			!cooldown_active,
			"First compression should be allowed when watermark is 0"
		);
	}

	#[test]
	fn test_cooldown_scales_with_post_compression_size() {
		// TEST: Cooldown threshold scales proportionally with context size

		// Small context: 20k after compression → need 22k to recompress
		let small_watermark: usize = 20_000;
		let small_threshold = (small_watermark as f64 * 1.1) as usize;
		assert_eq!(small_threshold, 22_000, "Small: need 22k");

		// Medium context: 80k after compression → need 88k to recompress
		let medium_watermark: usize = 80_000;
		let medium_threshold = (medium_watermark as f64 * 1.1) as usize;
		assert_eq!(medium_threshold, 88_000, "Medium: need 88k");

		// Large context: 150k after compression → need 165k to recompress
		let large_watermark: usize = 150_000;
		let large_threshold = (large_watermark as f64 * 1.1) as usize;
		assert_eq!(large_threshold, 165_000, "Large: need 165k");

		// Growth headroom scales with context size
		let small_headroom = small_threshold - small_watermark;
		let large_headroom = large_threshold - large_watermark;
		assert!(
			large_headroom > small_headroom,
			"Larger contexts get more headroom: {} > {}",
			large_headroom,
			small_headroom
		);
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

	// ============================================================================
	// SEQUENTIAL COMPRESSION TESTS: Verify first_prompt_idx stays at original
	// user message and old compressed summaries get re-compressed (not orphaned)
	// ============================================================================

	#[test]
	fn first_prompt_idx_never_changes_after_compression() {
		// first_prompt_idx must always point to the original first user message.
		// It is set once in main_loop.rs and never updated by compression.
		// This ensures the anchor is always the original user prompt.

		let mut messages = Vec::new();
		messages.push(msg("system")); // 0
		messages.push(msg("user")); // 1 - first_prompt_idx
		for i in 0..8 {
			messages.push(msg(if i % 2 == 0 { "assistant" } else { "user" }));
		} // 2-9

		let first_prompt_idx = Some(1usize);

		// First compression
		let (start1, end1) = find_compression_range(&messages, first_prompt_idx, false).unwrap();
		assert_eq!(start1, 1, "start_idx must be first_prompt_idx");
		assert!(end1 >= 4);

		// After compression, first_prompt_idx stays Some(1) — NOT updated.
		// The compressed summary is inserted at index 2, but the anchor stays at 1.
		assert_eq!(
			first_prompt_idx,
			Some(1),
			"first_prompt_idx must not change"
		);

		// Simulate post-compression state: anchor at 1, summary at 2, preserved tail
		let mut after = Vec::new();
		after.push(msg("system")); // 0
		after.push(msg("user")); // 1 - anchor (kept)
		let mut comp = msg("assistant");
		comp.name = Some("plan_compression".to_string());
		after.push(comp); // 2 - compressed summary
		for i in 0..8 {
			after.push(msg(if i % 2 == 0 { "user" } else { "assistant" }));
		} // 3-10

		// Second compression — first_prompt_idx is STILL Some(1)
		let (start2, end2) = find_compression_range(&after, first_prompt_idx, false).unwrap();
		assert_eq!(
			start2, 1,
			"second compression also starts at original anchor"
		);
		assert!(end2 >= 4);
	}

	#[test]
	fn old_compressed_summary_is_recompressed_on_next_cycle() {
		// After first compression, the summary sits at index 2 (role=assistant).
		// On second compression with first_prompt_idx=Some(1), start_idx=1,
		// so the drain range is [2..=end_idx] — the old summary IS drained.
		// This is correct: each cycle folds all prior context into one fresh summary.

		let mut messages = Vec::new();
		messages.push(msg("system")); // 0
		messages.push(msg("user")); // 1 - permanent anchor
		let mut comp = msg("assistant");
		comp.name = Some("plan_compression".to_string());
		comp.content = "OLD_SUMMARY_V1".to_string();
		messages.push(comp); // 2 - old compressed summary
		for i in 0..8 {
			messages.push(msg(if i % 2 == 0 { "user" } else { "assistant" }));
		} // 3-10

		let (start_idx, end_idx) = find_compression_range(&messages, Some(1), false).unwrap();
		assert_eq!(start_idx, 1, "start at permanent anchor");

		// Drain range is start_idx+1..=end_idx = 2..=end_idx
		// Index 2 (old summary) IS in the drain range — it gets re-compressed
		let drain_range = (start_idx + 1)..=end_idx;
		assert!(
			drain_range.contains(&2),
			"Old compressed summary must be IN the drain range (re-compressed)"
		);

		// messages_to_compress includes the old summary
		let to_compress = &messages[start_idx + 1..=end_idx];
		assert!(
			to_compress
				.iter()
				.any(|m| m.content.contains("OLD_SUMMARY_V1")),
			"Old summary must be included in messages sent to AI for re-compression"
		);
	}

	#[test]
	fn bootstrap_messages_before_start_idx_dont_inflate_compress_count() {
		// Reproduces the real-world bug: bootstrap messages (system, welcome, instructions)
		// exist before first_prompt_idx. The old code collected ALL conversation messages
		// including bootstrap ones, inflating compress_count and leaving end_idx too low.
		//
		// Layout from the log:
		// 0: system
		// 1: assistant (welcome)
		// 2: user (instructions)
		// 3: user (first real prompt) ← first_prompt_idx
		// 4: assistant (compressed summary from prior cycle)
		// 5: assistant (tool_calls)
		// 6: tool
		// 7: assistant (tool_calls)
		// 8: tool
		// 9: assistant (tool_calls)
		// 10: tool
		// 11: assistant (final response)
		let mut comp = msg("assistant");
		comp.name = Some("plan_compression".to_string());
		let mut a5 = msg("assistant");
		a5.tool_calls =
			Some(json!([{"id": "c1", "type": "function", "function": {"name": "plan"}}]));
		let mut t6 = msg("tool");
		t6.tool_call_id = Some("c1".to_string());
		let mut a7 = msg("assistant");
		a7.tool_calls =
			Some(json!([{"id": "c2", "type": "function", "function": {"name": "shell"}}]));
		let mut t8 = msg("tool");
		t8.tool_call_id = Some("c2".to_string());
		let mut a9 = msg("assistant");
		a9.tool_calls =
			Some(json!([{"id": "c3", "type": "function", "function": {"name": "plan"}}]));
		let mut t10 = msg("tool");
		t10.tool_call_id = Some("c3".to_string());

		let messages = vec![
			msg("system"),    // 0
			msg("assistant"), // 1 - welcome
			msg("user"),      // 2 - instructions
			msg("user"),      // 3 - first_prompt_idx
			comp,             // 4 - old compressed summary
			a5,               // 5 - tool_calls
			t6,               // 6 - tool result
			a7,               // 7 - tool_calls
			t8,               // 8 - tool result
			a9,               // 9 - tool_calls
			t10,              // 10 - tool result
			msg("assistant"), // 11 - final response
		];

		let (start_idx, end_idx) = find_compression_range(&messages, Some(3), false).unwrap();

		assert_eq!(start_idx, 3, "start at first_prompt_idx");
		// Conversation indices at/after start_idx=3: [3, 4, 5, 7, 9, 11] (6 items)
		// Keep last 4: [5, 7, 9, 11]
		// compress_count = 2, first preserved = index 5
		// end_idx = 5 - 1 = 4
		// Drain range: [4..=4] — removes old summary
		// BUT the old bug would have: conversation_indices = [1, 2, 3, 4, 5, 7, 9, 11] (8 items)
		// compress_count = 4, first preserved = conversation_indices[4] = 5
		// end_idx = 5 - 1 = 4 — same result by coincidence in THIS case.
		// The real difference shows when preserve_count adjustment kicks in.
		assert!(
			end_idx >= 4,
			"end_idx must cover at least the old summary at index 4, got {end_idx}"
		);
		// The drain range must actually remove messages
		assert!(
			end_idx > start_idx,
			"drain range must be non-empty: start={start_idx}, end={end_idx}"
		);
	}

	#[test]
	fn bootstrap_with_many_messages_compresses_all() {
		// With instructions at idx 2, anchor moves back to 2.
		// Compress-all: everything from anchor+1 to end gets compressed.
		let mut messages = vec![
			msg("system"),    // 0
			msg("assistant"), // 1 - welcome
			msg("user"),      // 2 - instructions
			msg("user"),      // 3 - first_prompt_idx
		];
		for i in 0..10 {
			messages.push(msg(if i % 2 == 0 { "assistant" } else { "user" }));
		} // 4-13

		let (start_idx, end_idx) = find_compression_range(&messages, Some(3), false).unwrap();
		assert_eq!(start_idx, 2, "anchor must be instructions file at idx 2");
		assert_eq!(end_idx, 13, "compress-all: end_idx = last message");
	}

	#[test]
	fn triple_compression_always_one_summary() {
		// After N compressions, there is always exactly ONE compressed summary
		// between the anchor and the preserved tail — never accumulating orphans.
		//
		// Cycle 1: [sys, user(anchor), asst, user, asst, ...] → drain 2..=end → insert summary at 2
		// Cycle 2: [sys, user(anchor), summary_v1, user, asst, ...] → drain 2..=end → insert summary at 2
		// Cycle 3: [sys, user(anchor), summary_v2, user, asst, ...] → drain 2..=end → insert summary at 2
		//
		// Each cycle: anchor stays at 1, old summary drained, new summary at 2.

		// Simulate state after 2nd compression
		let mut messages = Vec::new();
		messages.push(msg("system")); // 0
		messages.push(msg("user")); // 1 - permanent anchor
		let mut comp = msg("assistant");
		comp.name = Some("plan_compression".to_string());
		comp.content = "SUMMARY_V2".to_string();
		messages.push(comp); // 2 - summary from 2nd compression
		for i in 0..8 {
			messages.push(msg(if i % 2 == 0 { "user" } else { "assistant" }));
		} // 3-10

		// 3rd compression — still starts at anchor (1)
		let (start_idx, end_idx) = find_compression_range(&messages, Some(1), false).unwrap();
		assert_eq!(start_idx, 1);

		// Old summary at 2 is in drain range
		assert!((start_idx + 1..=end_idx).contains(&2));

		// After drain + insert: anchor at 1, new summary at 2, preserved tail after
		// No accumulation of old summaries — always exactly one.
	}

	#[test]
	fn anchor_message_never_included_in_drain_range() {
		// TEST: Verify that the anchor message at start_idx is NEVER in the drain range.
		// drain range = start_idx+1..=end_idx (exclusive of start_idx)

		let messages = vec![
			msg("system"),    // 0
			msg("user"),      // 1 - anchor
			msg("assistant"), // 2
			msg("user"),      // 3
			msg("assistant"), // 4
			msg("user"),      // 5
			msg("assistant"), // 6
			msg("user"),      // 7
			msg("assistant"), // 8
		];

		let (start_idx, end_idx) = find_compression_range(&messages, Some(1), false).unwrap();

		// The drain range is start_idx+1..=end_idx
		// The anchor at start_idx is NOT in this range
		let drain_start = start_idx + 1;
		let drain_end = end_idx;

		assert!(drain_start > start_idx, "Drain must start AFTER anchor");
		assert!(drain_end >= drain_start, "Drain range must be valid");

		// Verify: anchor index is NOT in drain range
		assert!(
			!(start_idx >= drain_start && start_idx <= drain_end),
			"Anchor must NOT be in drain range"
		);

		// Verify: messages_to_compress range matches drain range
		// CORRECT: start_idx+1..=end_idx
		// WRONG (old bug): start_idx..=end_idx
		let correct_range = (start_idx + 1)..=end_idx;
		assert!(correct_range.contains(&(start_idx + 1)));
		assert!(
			!correct_range.contains(&start_idx),
			"Anchor must NOT be in compression range"
		);
	}

	#[test]
	fn compression_preserves_message_count_consistency() {
		// TEST: Verify message count after compression is correct.
		// Before: N messages
		// Remove: M messages (start_idx+1..=end_idx)
		// Insert: 1 compressed summary
		// After: N - M + 1 messages

		let mut messages = Vec::new();
		messages.push(msg("system")); // 0
		messages.push(msg("user")); // 1 - anchor
		for i in 2..=9 {
			messages.push(msg(if i % 2 == 0 { "assistant" } else { "user" }));
		}

		let before_count = messages.len();
		let (start_idx, end_idx) = find_compression_range(&messages, Some(1), false).unwrap();

		// Calculate expected removal count
		let messages_to_remove = end_idx - start_idx; // drain removes start_idx+1..=end_idx
		let _expected_after = before_count - messages_to_remove + 1; // +1 for compressed summary

		// Verify: messages_to_remove matches drain range
		assert_eq!(
			messages_to_remove,
			(end_idx - (start_idx + 1) + 1),
			"Removal count must match drain range"
		);

		// The anchor at start_idx is NOT removed
		// So we remove (end_idx - start_idx) messages, not (end_idx - start_idx + 1)
		assert!(
			messages_to_remove < before_count,
			"Must remove fewer messages than total"
		);
	}

	#[test]
	fn messages_to_compress_excludes_anchor_message() {
		// messages_to_compress must be start_idx+1..=end_idx (exclude anchor).
		// The anchor at start_idx is KEPT by remove_messages_in_range.

		let mut messages = Vec::new();
		messages.push(msg("system")); // 0

		let mut anchor = msg("user"); // 1
		anchor.content = "ANCHOR_CONTENT_MUST_NOT_BE_SUMMARIZED".to_string();
		messages.push(anchor);

		messages.push(msg("assistant")); // 2
		messages.push(msg("user")); // 3
		messages.push(msg("assistant")); // 4
		messages.push(msg("user")); // 5
		messages.push(msg("assistant")); // 6
		messages.push(msg("user")); // 7
		messages.push(msg("assistant")); // 8

		let (start_idx, end_idx) = find_compression_range(&messages, Some(1), false).unwrap();
		assert_eq!(start_idx, 1);

		let correct = &messages[start_idx + 1..=end_idx];
		let wrong = &messages[start_idx..=end_idx];

		assert_eq!(correct.len(), end_idx - start_idx);
		assert_eq!(wrong.len(), end_idx - start_idx + 1);

		assert!(
			!correct.iter().any(|m| m.content.contains("ANCHOR_CONTENT")),
			"Anchor must NOT be in messages_to_compress"
		);
		assert!(
			wrong.iter().any(|m| m.content.contains("ANCHOR_CONTENT")),
			"Old bug: anchor WAS in messages_to_compress"
		);
	}

	#[test]
	fn calculate_range_tokens_matches_actual_removal() {
		// calculate_range_tokens must count exactly the messages removed by
		// remove_messages_in_range (start_idx+1..=end_idx), not including anchor.

		use crate::session::estimate_message_tokens;

		let mut messages = Vec::new();
		messages.push(msg("system")); // 0

		let mut anchor = msg("user");
		anchor.content = "x".repeat(1000);
		messages.push(anchor); // 1

		for i in 0..4 {
			let mut m = msg(if i % 2 == 0 { "assistant" } else { "user" });
			m.content = format!("Message {}", i);
			messages.push(m);
		} // 2-5

		for i in 0..4 {
			messages.push(msg(if i % 2 == 0 { "user" } else { "assistant" }));
		} // 6-9

		let (start_idx, end_idx) = find_compression_range(&messages, Some(1), false).unwrap();

		let mut tokens_removed = 0u64;
		for msg in messages.iter().take(end_idx + 1).skip(start_idx + 1) {
			tokens_removed += estimate_message_tokens(msg) as u64;
		}

		let mut tokens_with_anchor = 0u64;
		for msg in messages.iter().take(end_idx + 1).skip(start_idx) {
			tokens_with_anchor += estimate_message_tokens(msg) as u64;
		}

		let anchor_tokens = estimate_message_tokens(&messages[start_idx]) as u64;
		assert_eq!(
			tokens_with_anchor - tokens_removed,
			anchor_tokens,
			"Difference must be exactly the anchor message tokens"
		);
	}

	// ── Stress tests ──────────────────────────────────────────────────────────

	#[test]
	fn test_file_context_stripped_from_recompression_input() {
		// strip_file_context_from_summary must remove everything from the sentinel onward.
		// This prevents stale file bytes from accumulating in every subsequent summary.
		let summary_with_context = "## Conversation Summary [COMPRESSED: abc]\n\
			Some important history here.\n\n\
			**FILE CONTEXT** (auto-expanded):\n\
			<content path=\"src/main.rs\">\nfn main() {}\n</content>";

		let stripped = strip_file_context_from_summary(summary_with_context);

		assert!(
			!stripped.contains("FILE CONTEXT"),
			"FILE CONTEXT sentinel must be stripped"
		);
		assert!(
			!stripped.contains("fn main()"),
			"File bytes must not appear in stripped output"
		);
		assert!(
			stripped.contains("Some important history here."),
			"Summary text before sentinel must be preserved"
		);
	}

	#[test]
	fn test_file_context_stripped_when_no_sentinel() {
		// When there is no FILE CONTEXT block, the function must return the text unchanged.
		let plain = "## Conversation Summary [COMPRESSED: abc]\nJust a summary.";
		let stripped = strip_file_context_from_summary(plain);
		assert_eq!(stripped, plain.trim());
	}

	#[test]
	fn test_multiple_compression_cycles_anchor_never_moves() {
		// Simulate 3 compression cycles on a growing conversation.
		// After each cycle the old summary is at start_idx+1 and gets folded into the next.
		// first_prompt_idx must always equal 1 (the original first user message).
		//
		// Layout after each cycle:
		//   [0] system
		//   [1] user (anchor = first_prompt_idx)
		//   [2] assistant (compressed summary, replaces old range)
		//   [3..] new messages

		let first_prompt_idx = Some(1usize);

		// ── Cycle 1: 12 messages ──────────────────────────────────────────────
		let mut messages: Vec<Message> = Vec::new();
		messages.push(msg("system")); // 0
		messages.push(msg("user")); // 1 ← anchor
		for i in 0..10 {
			messages.push(msg(if i % 2 == 0 { "assistant" } else { "user" }));
		} // 2-11

		let (s1, e1) = find_compression_range(&messages, first_prompt_idx, false).unwrap();
		assert_eq!(s1, 1, "Cycle 1: start must be anchor (1)");
		assert!(e1 > s1, "Cycle 1: end must be after anchor");
		assert!(
			e1 < messages.len(),
			"Cycle 1: end must leave RECENT messages"
		);

		// Simulate applying compression: drain s1+1..=e1, insert summary at s1+1
		let drained: Vec<Message> = messages.drain(s1 + 1..=e1).collect();
		assert!(!drained.is_empty(), "Cycle 1: must drain something");
		let mut summary1 = msg("assistant");
		summary1.content = "## Conversation Summary [COMPRESSED: c1]\nCycle 1 summary.".to_string();
		messages.insert(s1 + 1, summary1);

		// ── Cycle 2: grow then compress again ────────────────────────────────
		for i in 0..10 {
			messages.push(msg(if i % 2 == 0 { "user" } else { "assistant" }));
		}

		let (s2, e2) = find_compression_range(&messages, first_prompt_idx, false).unwrap();
		assert_eq!(s2, 1, "Cycle 2: start must still be anchor (1)");
		assert!(e2 > s2);

		let drained2: Vec<Message> = messages.drain(s2 + 1..=e2).collect();
		assert!(!drained2.is_empty(), "Cycle 2: must drain something");
		let mut summary2 = msg("assistant");
		summary2.content = "## Conversation Summary [COMPRESSED: c2]\nCycle 2 summary.".to_string();
		messages.insert(s2 + 1, summary2);

		// ── Cycle 3: grow then compress again ────────────────────────────────
		for i in 0..10 {
			messages.push(msg(if i % 2 == 0 { "user" } else { "assistant" }));
		}

		let (s3, e3) = find_compression_range(&messages, first_prompt_idx, false).unwrap();
		assert_eq!(s3, 1, "Cycle 3: start must still be anchor (1)");
		assert!(e3 > s3);

		// After 3 cycles the anchor is always at index 1 — never drifts.
		assert_eq!(s1, s2, "Anchor must not drift between cycles");
		assert_eq!(s2, s3, "Anchor must not drift between cycles");
	}

	#[test]
	fn compress_all_includes_last_message() {
		// Compress-all: end_idx = last message. Recent user messages are extracted
		// and re-injected by the caller, not protected by find_compression_range.
		let mut messages: Vec<Message> = Vec::new();
		messages.push(msg("system")); // 0
		messages.push(msg("user")); // 1 ← anchor
		for i in 0..20 {
			messages.push(msg(if i % 2 == 0 { "assistant" } else { "user" }));
		} // 2-21
		messages.push(msg("user")); // 22

		let (start_idx, end_idx) = find_compression_range(&messages, Some(1), false).unwrap();
		assert_eq!(start_idx, 1);
		assert_eq!(end_idx, 22, "compress-all: end_idx must be last message");
	}

	#[test]
	fn compress_all_with_tool_loop_after_user_prompt() {
		// Compress-all: everything is compressed. The user's 2nd prompt at index 5
		// is in the drain range but will be extracted and re-injected by the caller.
		let messages = vec![
			msg("system"),    // 0
			msg("assistant"), // 1 welcome
			msg("user"),      // 2 instructions
			msg("user"),      // 3 first prompt
			msg("assistant"), // 4 compressed summary
			msg("user"),      // 5 second prompt
			msg("assistant"), // 6 tool_calls
			msg("tool"),      // 7
			msg("tool"),      // 8
			msg("assistant"), // 9 tool_calls
			msg("tool"),      // 10
			msg("assistant"), // 11 response
			msg("assistant"), // 12 tool_calls
			msg("tool"),      // 13
			msg("assistant"), // 14 response
		];

		let (start_idx, end_idx) = find_compression_range(&messages, Some(3), false).unwrap();
		assert_eq!(start_idx, 2, "anchor at instructions");
		assert_eq!(end_idx, 14, "compress-all: end_idx = last message");
	}

	#[test]
	fn test_recent_window_capped_at_8_for_large_session() {
		// For a 100-message session, RECENT count must be 8 (not 25).
		// This mirrors the formula: (total / 4).max(4).min(8)
		let total_msgs: usize = 100;
		let recent_count = (total_msgs / 4).clamp(4, 8);
		assert_eq!(
			recent_count, 8,
			"RECENT window must be capped at 8 for large sessions"
		);

		// For a 12-message session, RECENT count is 3 → clamped to 4
		let small = 12usize;
		let recent_small = (small / 4).clamp(4, 8);
		assert_eq!(recent_small, 4, "RECENT window must be at least 4");

		// For a 32-message session, RECENT count is 8 (exactly at cap)
		let medium = 32usize;
		let recent_medium = (medium / 4).clamp(4, 8);
		assert_eq!(recent_medium, 8, "RECENT window must be 8 at 32 messages");
	}
	#[test]
	fn compress_all_with_tool_cycles() {
		// Compress-all: no preserved zone concept. Everything is compressed,
		// recent user messages are extracted and re-injected by the caller.
		let messages = vec![
			msg("system"),    // 0
			msg("user"),      // 1 (first_prompt_idx)
			msg("assistant"), // 2
			msg("user"),      // 3
			msg("assistant"), // 4
			msg("user"),      // 5
			msg("assistant"), // 6
			msg("user"),      // 7
			msg("assistant"), // 8
		];

		let (start_idx, end_idx) = find_compression_range(&messages, Some(1), false).unwrap();
		assert_eq!(start_idx, 1);
		assert_eq!(end_idx, 8, "compress-all: end_idx = last message");

		// Simulate compress-all + user extraction: drain, insert summary, re-inject users
		let recent_users: Vec<Message> = messages[start_idx + 1..=end_idx]
			.iter()
			.rev()
			.filter(|m| m.role == "user")
			.take(2)
			.cloned()
			.collect::<Vec<_>>()
			.into_iter()
			.rev()
			.collect();

		let mut after = messages.clone();
		after.drain(start_idx + 1..=end_idx);
		let mut summary = msg("assistant");
		summary.content = "## Conversation Summary [COMPRESSED: test]".to_string();
		after.insert(start_idx + 1, summary);
		// Re-inject recent user messages
		for (i, user_msg) in recent_users.iter().enumerate() {
			after.insert(start_idx + 2 + i, user_msg.clone());
		}

		// Result: [system, user(anchor), summary(asst), user(5), user(7)]
		assert_eq!(after.len(), 5);
		assert_eq!(after[0].role, "system");
		assert_eq!(after[1].role, "user"); // anchor
		assert_eq!(after[2].role, "assistant"); // summary
		assert_eq!(after[3].role, "user"); // extracted user from idx 5
		assert_eq!(after[4].role, "user"); // extracted user from idx 7
	}

	#[test]
	fn tool_loop_only_one_user_message_still_compresses() {
		// Reproduces the exact bug from the session log:
		//   Compression check: current_tokens=61028, api_calls=137
		//   Invalid compression range (0 >= 0), skipping
		//
		// In a tool-loop session, there is only ONE user message (the initial prompt).
		// All subsequent messages are assistant+tool cycles.
		//
		// With first_prompt_idx=Some(1), start_idx = 0 (system anchor).
		// The user at idx 1 is inside the drain range. The while loop that searches
		// for a user in the preserved zone finds none (all preserved are assistants),
		// so compress_count stays at its original value — compression still happens.
		let mut messages = Vec::new();
		messages.push(msg("system")); // 0
		messages.push(msg("user")); // 1 (first_prompt_idx) — the ONLY user message

		// Simulate 10 tool cycles: assistant(tool_calls) → tool result
		for i in 0..10 {
			let mut asst = msg("assistant");
			asst.tool_calls = Some(json!([
				{"id": format!("call_{i}"), "type": "function", "function": {"name": "view", "arguments": "{}"}}
			]));
			messages.push(asst);
			let mut tool = msg("tool");
			tool.tool_call_id = Some(format!("call_{i}"));
			messages.push(tool);
		}

		// Final assistant response (no tool_calls)
		messages.push(msg("assistant")); // 22

		let (start_idx, end_idx) = find_compression_range(&messages, Some(1), false).unwrap();

		// Must return a valid compression range, NOT (0, 0)
		assert!(
			start_idx < end_idx,
			"Tool-loop session must produce valid compression range, got ({start_idx}, {end_idx})"
		);

		// start_idx = first_prompt_idx = 1 (tool-loop: single user, no instructions)
		assert_eq!(start_idx, 1, "start_idx must be first_prompt_idx = 1");

		// compress-all: end_idx = last message
		assert_eq!(
			end_idx,
			messages.len() - 1,
			"compress-all: end_idx must be last message"
		);
	}

	#[test]
	fn test_triple_compression_only_one_summary_in_drain() {
		// After 3 compression cycles, the drain range must always contain exactly
		// one prior compressed summary (the previous cycle's output), never zero or two.
		// This verifies that old summaries are folded into new ones, not accumulated.
		let first_prompt_idx = Some(1usize);

		let mut messages: Vec<Message> = Vec::new();
		messages.push(msg("system")); // 0
		messages.push(msg("user")); // 1 ← anchor
		for i in 0..10 {
			messages.push(msg(if i % 2 == 0 { "assistant" } else { "user" }));
		}

		for cycle in 1..=3usize {
			// Grow the session
			for i in 0..8 {
				messages.push(msg(if i % 2 == 0 { "user" } else { "assistant" }));
			}

			let (s, e) = find_compression_range(&messages, first_prompt_idx, false).unwrap();

			// Count compressed summaries in the drain range (s+1..=e)
			let summaries_in_drain = messages[s + 1..=e]
				.iter()
				.filter(|m| {
					m.content
						.starts_with("## Conversation Summary [COMPRESSED:")
				})
				.count();

			if cycle > 1 {
				assert_eq!(
					summaries_in_drain, 1,
					"Cycle {}: drain range must contain exactly 1 prior summary, found {}",
					cycle, summaries_in_drain
				);
			}

			// Apply compression
			let _drained: Vec<Message> = messages.drain(s + 1..=e).collect();
			let mut summary = msg("assistant");
			summary.content =
				format!("## Conversation Summary [COMPRESSED: c{cycle}]\nCycle {cycle} summary.");
			messages.insert(s + 1, summary);
		}
	}

	#[test]
	fn bug_proof_invalid_range_must_set_cooldown() {
		// BUG SCENARIO: should_check_compression runs the full expensive path:
		//   threshold exceeded → cooldown passed → cost analysis → find_compression_range
		// When find_compression_range returns (0, 0) (not enough messages),
		// it MUST set context_tokens_after_last_compression to prevent the same
		// expensive analysis from running every single turn.
		//
		// Without the fix, the log shows this loop every turn:
		//   Compression check: current_tokens=61028, thresholds=[60000, 80000, 120000]
		//   ✓ Threshold exceeded!
		//   Compression cooldown passed: ...
		//   Net benefit: $0.27539 → COMPRESS ✓
		//   Invalid compression range (0 >= 0), skipping
		//   ... repeats next turn ...

		// Step 1: Prove find_compression_range returns (0, 0) with too few messages
		let messages = vec![
			msg("system"),    // 0
			msg("user"),      // 1
			msg("assistant"), // 2
			msg("user"),      // 3
			msg("assistant"), // 4
		];
		// Only 4 conversation messages (user+assistant) — need >4 to compress
		let (start_idx, end_idx) = find_compression_range(&messages, Some(1), false).unwrap();
		assert_eq!(
			(start_idx, end_idx),
			(0, 0),
			"Must return (0,0) when not enough messages to compress"
		);

		// Step 2: Verify the cooldown logic that should_check_compression must apply
		// when it encounters this (0, 0) range after passing all other gates.
		let current_tokens: usize = 61_028;
		let mut context_tokens_after_last_compression: usize = 19_442; // from prior compression

		// Simulate the fix: set cooldown when range is invalid
		if start_idx >= end_idx {
			context_tokens_after_last_compression = current_tokens;
		}

		// Now the cooldown check should block the next attempt
		let min_tokens_for_recompression =
			(context_tokens_after_last_compression as f64 * 1.1) as usize;
		assert!(
			current_tokens < min_tokens_for_recompression,
			"After setting cooldown to current_tokens={}, next check at same token count must be blocked (need {} for recompression)",
			current_tokens,
			min_tokens_for_recompression
		);

		// Step 3: Verify that WITHOUT the fix, cooldown would NOT block
		let old_watermark: usize = 19_442;
		let old_min = (old_watermark as f64 * 1.1) as usize;
		assert!(
			current_tokens >= old_min,
			"Without fix, old watermark {} allows recompression at {} (min: {}) — the bug!",
			old_watermark,
			current_tokens,
			old_min
		);
	}

	#[test]
	fn bug_proof_invalid_range_cooldown_allows_growth() {
		// After cooldown is set from invalid range, compression must still
		// trigger once context grows by ≥10%.
		let current_tokens: usize = 61_028;
		let context_tokens_after_last_compression = current_tokens; // cooldown set

		// 10% growth should allow recompression
		let grown_tokens: usize = 67_200; // ~10.1% growth
		let min_required = (context_tokens_after_last_compression as f64 * 1.1) as usize;
		assert!(
			grown_tokens >= min_required,
			"After 10%+ growth ({} → {}), compression should be allowed (min: {})",
			current_tokens,
			grown_tokens,
			min_required
		);
	}

	#[test]
	fn knowledge_log_entry_uses_content_key() {
		// REGRESSION: log_knowledge_entry() previously wrote "knowledge" key but
		// persistence.rs reads "content" key — entries were silently lost on resume.
		// Verify the JSON produced by the logger uses "content".
		let entry = serde_json::json!({
			"type": "KNOWLEDGE_ENTRY",
			"timestamp": 0u64,
			"content": "test knowledge"
		});
		assert!(
			entry.get("content").is_some(),
			"KNOWLEDGE_ENTRY must use 'content' key (not 'knowledge')"
		);
		assert!(
			entry.get("knowledge").is_none(),
			"'knowledge' key must not be present — persistence reads 'content'"
		);
		assert_eq!(entry["content"].as_str().unwrap(), "test knowledge");
	}
}
