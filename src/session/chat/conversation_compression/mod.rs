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
	.with_full_context_tokens(true)
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

/// Minimum acceptable length (after trim + knowledge-tag strip) for a compression summary.
///
/// A 200-OK response from the AI is no guarantee the body is usable. The model can
/// return:
/// - bare `"YES"` with no summary line,
/// - `"YES\n<knowledge>...</knowledge>"` (knowledge stripped → empty),
/// - `force=true` response that is ONLY knowledge tags (also strips to empty),
/// - whitespace, a stray punctuation, or other near-empty noise.
///
/// If we accept any of those, `apply_compression` would drain dozens of messages and
/// insert a header-only "## Conversation Summary" block — wiping the entire context.
/// This guard refuses to compress in that case so the caller sets a cooldown and the
/// session keeps its real history.
const MIN_SUMMARY_LEN: usize = 20;

/// True if a candidate summary is substantive enough to replace compressed messages.
fn is_summary_valid(summary: &str) -> bool {
	summary.trim().chars().count() >= MIN_SUMMARY_LEN
}

/// Parse the AI response into a compression decision and optional summary text.
///
/// `force=true`: entire response is the summary (no YES/NO gate).
/// `force=false`: first line must be YES to proceed; NO means skip compression.
///
/// SAFETY: A response that yields a too-short summary (after trim + knowledge-tag
/// strip) is treated as a compression failure and returns `(false, "")` — even on
/// the `force` path. Better to skip compression than to wipe the conversation with
/// an empty summary. See `MIN_SUMMARY_LEN`.
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
		if !is_summary_valid(&summary) {
			log_info!(
				"Compression aborted: AI returned too-short summary ({} chars, force=true). Skipping compression to avoid context loss.",
				summary.trim().chars().count()
			);
			return Ok((false, String::new()));
		}
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
		if !is_summary_valid(&summary) {
			log_info!(
				"Compression aborted: AI said YES but summary is too short ({} chars). Skipping compression to avoid context loss.",
				summary.trim().chars().count()
			);
			return Ok((false, String::new()));
		}
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

	// CRITICAL: Capture the most recent assistant response_id from the range we're
	// about to drain. The Responses API (OpenAI + OctoHub) chains via this id —
	// the server stores prior turns under it and reconstructs full history from
	// the chain. If we drain every id-bearing assistant and leave the summary
	// without one, the next request finds no `previous_id`, falls into the
	// "initial request" branch of `messages_to_input`, which filters out the
	// summary (role=assistant) entirely. The model then receives only the
	// re-injected user turn with zero context — exactly the "lost YES / plan
	// approval" failure mode. Inheriting the id keeps the server-side chain
	// intact while local view shrinks for token budget.
	let inherited_response_id: Option<String> = session.session.messages[start_idx + 1..=end_idx]
		.iter()
		.rev()
		.find(|m| m.role == "assistant" && m.id.is_some())
		.and_then(|m| m.id.clone());

	if let Some(ref id) = inherited_response_id {
		log_debug!(
			"Compression: inheriting last assistant response_id={} onto summary to preserve chain continuity",
			id
		);
	} else {
		log_debug!(
			"Compression: no assistant response_id found in drained range; summary will start a fresh chain"
		);
	}

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

	// Summary message (no cache marker — sits between anchor marker and user marker).
	// The `id` is inherited from the most recent assistant turn in the drained range
	// so the provider can chain via `previous_response_id` on the next API call.
	let summary_msg = crate::session::Message {
		role: "assistant".to_string(),
		content: compressed_entry,
		timestamp: now,
		cached: false,
		name: Some("plan_compression".to_string()),
		id: inherited_response_id,
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
mod tests;
