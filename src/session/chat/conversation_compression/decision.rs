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

// Cost-aware compression decision math.
//
// Pure pricing/forecast helpers — no LLM calls, no session mutation. The
// orchestrator in `mod.rs` (`should_check_compression`) consults these to
// decide whether compression is profitable considering cache-invalidation
// costs vs. future savings over the predicted remaining turns.

use crate::log_debug;
use crate::session::chat::session::ChatSession;
use crate::session::estimate_tokens;

/// Calculate net benefit of compression using realistic cost analysis with REAL pricing
///
/// CRITICAL INSIGHT: Each API call pays for the ENTIRE context (base + all accumulated new tokens)
/// New tokens added in call N become part of the base for calls N+1, N+2, etc.
/// This cumulative effect makes compression MUCH more valuable!
///
/// Returns positive value if compression saves money, negative if it costs money
pub(super) async fn calculate_compression_net_benefit(
	session: &ChatSession,
	config: &crate::config::Config,
	current_tokens: usize,
	compressible_tokens: u64,
	compression_ratio: f64,
) -> f64 {
	let total_tokens = current_tokens as f64;
	let compressible_tokens = compressible_tokens as f64;
	let compressed_slice_tokens = compressible_tokens / compression_ratio;
	let headroom = compressible_tokens - compressed_slice_tokens;
	let estimated_future_turns = estimate_future_turns(session, headroom);
	let compressed_tokens = total_tokens - headroom;

	// Get decision model (used for compression) and session model (used for future calls)
	let decision_model = &config.compression.decision.model;
	let session_model = &session.model;
	// Get pricing for both models using provider factory
	let decision_pricing = get_model_pricing(decision_model, config);
	let session_pricing = get_model_pricing(session_model, config);

	// If we can't get pricing, check ignore_cost before giving up.
	// With ignore_cost=true the user explicitly opts out of cost-gating —
	// treat missing pricing as zero-cost and compress for context management.
	let ignore_cost = config.compression.decision.ignore_cost;
	let (decision_pricing, session_pricing) = match (decision_pricing, session_pricing) {
		(Some(d), Some(s)) => (d, s),
		_ => {
			if ignore_cost {
				log_debug!(
					"Cannot get pricing for models: decision='{}', session='{}' - ignore_cost=true, compressing anyway",
					decision_model,
					session_model
				);
				return 1.0; // Positive = compress (context management, cost ignored)
			}
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

	// Cache-less providers (per the provider's own supports_caching capability)
	// re-bill the ENTIRE context at full input price on every call. Pricing the
	// base at cache_read rate there (glm-5.2: $0.26/M vs $1.40/M input) understates
	// the no-compress cost ~5x and wrongly cost-gates compression OFF exactly
	// where it is most profitable.
	let session_caches = crate::session::model_supports_caching(session_model);

	// Estimate actual output tokens from compression API call
	// The AI generates a summary (not the full compressed_tokens size)
	// Use compressed_tokens as estimate, but cap at max_tokens if set
	let decision_max_tokens = config.compression.decision.max_tokens;
	let estimated_output_tokens = if decision_max_tokens > 0 {
		(compressed_slice_tokens as u64).min(decision_max_tokens as u64)
	} else {
		compressed_slice_tokens as u64
	};

	// SCENARIO A: NO compression
	// Each call pays: cache_read(base) + input(new_tokens), base grows each turn
	let mut total_cost_no_compress = 0.0;
	let mut base_context = total_tokens;

	for _ in 0..estimated_future_turns as i32 {
		// Pay cache_read for base (already cached) + input for NEW tokens.
		// Without provider caching the base is re-billed at input price.
		let context_cost = if session_caches {
			session_pricing.calculate_cost(
				avg_new_tokens_per_call as u64, // NEW tokens: input price
				0,                              // No cache write
				base_context as u64,            // BASE: cache_read price
				0,                              // No output in context calculation
			)
		} else {
			session_pricing.calculate_cost(
				(base_context + avg_new_tokens_per_call) as u64, // ALL at input price
				0,
				0,
				0,
			)
		};

		total_cost_no_compress += context_cost;

		// New tokens become part of base for next call
		base_context += avg_new_tokens_per_call;
	}

	// SCENARIO B: WITH compression
	// 1. Compression cost (one-time) using DECISION model pricing
	let compression_cost = if same_model && session_caches {
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
		// Subsequent calls: cache_read for base.
		// Without provider caching the compressed base is input-priced every call.
		let (input_tokens, cache_write, cache_read) = if !session_caches {
			(
				(base_context_compressed + avg_new_tokens_per_call) as u64,
				0,
				0,
			)
		} else if call_num == 0 {
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
pub(super) fn get_model_pricing(
	model: &str,
	_config: &crate::config::Config,
) -> Option<crate::providers::ModelPricing> {
	// Parse model string (format: "provider:model"). Split on the FIRST colon
	// only — model names legitimately contain colons (e.g. "ollama:llama3:8b"),
	// and splitting on all of them wrongly rejected such ids, silently disabling
	// compression for tagged local models.
	let Some((provider_name, model_name)) = model.split_once(':') else {
		log_debug!(
			"Invalid model format: '{}' (expected 'provider:model')",
			model
		);
		return None;
	};

	// Get provider instance and query pricing
	let provider = crate::providers::ProviderFactory::create_provider(provider_name).ok()?;
	provider.get_model_pricing(model_name)
}

/// Below this ratio a paid summarize call frees too little context to be worth
/// the round-trip and the cache invalidation it causes.
pub(super) const MIN_COMPRESSION_RATIO: f64 = 2.0;

/// Above this ratio the summary can no longer carry the evidence the folded
/// units must cite; deeper reduction destroys attribution instead of saving it.
pub(super) const MAX_COMPRESSION_RATIO: f64 = 16.0;

/// A compression must buy at least this many turns before re-firing, or the
/// cost analysis (and the paid call itself) is meaningless. Also the floor for
/// runway projections when a session is too young to have a symmetry signal.
pub(super) const MIN_RUNWAY_TURNS: f64 = 5.0;

/// Usable context ceiling: the hard cap the session must never cross.
/// The lower of the user's explicit safety limit (when set) and the session
/// model's physical window minus the reserved completion budget. A session
/// model that doesn't resolve to a provider contributes no physical bound
/// (such a session cannot make API calls anyway).
pub(super) fn context_ceiling(session: &ChatSession, config: &crate::config::Config) -> usize {
	let model_bound = crate::session::model_max_input_tokens(&session.model)
		.map(|window| window.saturating_sub(config.max_tokens as usize));
	match (config.max_session_tokens_threshold, model_bound) {
		(0, Some(bound)) => bound,
		(0, None) => usize::MAX,
		(configured, Some(bound)) => configured.min(bound),
		(configured, None) => configured,
	}
}

/// Measured full-context growth per API call.
///
/// After a compression, `context_tokens_after_last_compression` is an exact
/// recount of the surviving view. Comparing the live context against that
/// watermark captures every source of growth: assistant output, tool results,
/// user messages, and runtime injections. Before the first compression there is
/// no full-context checkpoint, so output-token growth is the conservative
/// fallback.
pub(super) fn measured_growth_rate(
	info: &crate::session::SessionInfo,
	current_tokens: usize,
) -> f64 {
	if info.context_tokens_after_last_compression > 0 {
		let calls_since = (info.total_api_calls - info.api_calls_at_last_compression).max(1) as f64;
		let context_growth =
			current_tokens.saturating_sub(info.context_tokens_after_last_compression) as f64;
		(context_growth / calls_since).max(1.0)
	} else {
		measured_output_growth_rate(info)
	}
}

fn measured_output_growth_rate(info: &crate::session::SessionInfo) -> f64 {
	if info.compression_stats.conversation_compressions > 0 {
		let calls_since = (info.total_api_calls - info.api_calls_at_last_compression).max(1) as f64;
		let output_since = info
			.output_tokens
			.saturating_sub(info.output_tokens_at_last_compression) as f64;
		(output_since / calls_since).max(1.0)
	} else {
		(info.output_tokens as f64 / (info.total_api_calls as f64).max(1.0)).max(1.0)
	}
}

/// Desired quiet runway between autonomous compression cycles.
///
/// A genuine user turn resets `consecutive_compressions` to zero. While one
/// turn continues autonomously, every successful compression doubles the
/// runway, so the next soft trigger expands from 5 to 10, 20, 40... measured
/// rounds. The hard ceiling still bounds the result.
pub(super) fn autonomous_runway(consecutive_compressions: u32) -> f64 {
	MIN_RUNWAY_TURNS * 2usize.saturating_pow(consecutive_compressions) as f64
}

/// Dynamic soft trigger derived from the actual post-compression working set.
///
/// `configured_threshold` remains the minimum fire line. After compression the
/// trigger expands to `post_tokens + runway * growth`, capped early enough to
/// leave `MIN_RUNWAY_TURNS` of safety below the physical ceiling.
pub(super) fn adaptive_fire_line(
	configured_threshold: usize,
	ceiling: usize,
	post_tokens: usize,
	growth: f64,
	runway: f64,
) -> usize {
	let safety_tokens = (growth * MIN_RUNWAY_TURNS) as usize;
	let safe_ceiling = ceiling.saturating_sub(safety_tokens);
	let baseline = configured_threshold.min(safe_ceiling);
	if post_tokens == 0 {
		// Before the first successful compression there is no post-state anchor.
		// A paid model rejection still increments the autonomous counter, so grow
		// from the configured baseline and avoid asking the same question again on
		// the next round.
		let extra_runway = (runway - MIN_RUNWAY_TURNS).max(0.0);
		return baseline
			.saturating_add((growth * extra_runway) as usize)
			.min(safe_ceiling);
	}

	let expanded = post_tokens.saturating_add((growth * runway) as usize);
	baseline.max(expanded.min(safe_ceiling))
}

/// Compute the compression ratio from measured session dynamics — the ladder
/// replacement. Picks the post-compression token target directly:
///
///   desired_after = fire_line − runway × growth
///
/// i.e. leave exactly enough headroom for the predicted remainder of the
/// session — a hot session compresses deep, a winding-down session compresses
/// gently. The target is clamped between the deepest and gentlest achievable
/// sizes and must land at least MIN_RUNWAY_TURNS × growth below the fire line
/// (a compression that refires immediately is worse than none).
///
/// Returns the derived ratio (∈ [MIN_COMPRESSION_RATIO, MAX_COMPRESSION_RATIO]
/// by construction), or None when even the deepest fold cannot land below the
/// re-fire bound — the caller sets the cooldown and skips.
pub(super) fn compression_depth(
	current_tokens: usize,
	compressible_tokens: u64,
	fire_line: usize,
	growth: f64,
	runway: f64,
) -> Option<f64> {
	if compressible_tokens == 0 {
		return None;
	}
	let compressible = compressible_tokens as f64;
	let surviving = (current_tokens as f64 - compressible).max(0.0);
	let deepest_after = surviving + compressible / MAX_COMPRESSION_RATIO;
	let gentlest_after = surviving + compressible / MIN_COMPRESSION_RATIO;

	let refire_bound = fire_line as f64 - growth * MIN_RUNWAY_TURNS;
	let upper = gentlest_after.min(refire_bound);
	if deepest_after > upper {
		return None;
	}

	let desired_after = fire_line as f64 - runway * growth;
	let target_after = desired_after.clamp(deepest_after, upper);
	let ratio = compressible / (target_after - surviving);

	crate::log_debug!(
		"Computed compression depth: current={}, compressible={:.0}, surviving={:.0}, \
		growth={:.0} tok/call, runway={:.1} calls, desired_after={:.0}, \
		band=[{:.0}, {:.0}], target_after={:.0} → ratio {:.1}x",
		current_tokens,
		compressible,
		surviving,
		growth,
		runway,
		desired_after,
		deepest_after,
		upper,
		target_after,
		ratio
	);

	Some(ratio)
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
pub(super) fn estimate_future_turns(session: &ChatSession, headroom: f64) -> f64 {
	let info = &session.session.info;
	let api_calls = info.total_api_calls as f64;

	// Growth rate: output tokens per call — pure new content added each turn
	// (see measured_growth_rate for the incremental-vs-lifetime rationale).
	let growth_rate = measured_output_growth_rate(info);

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
	let adjusted = (estimate * accuracy).max(MIN_RUNWAY_TURNS); // cooldown must be meaningful

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
pub(super) fn calculate_self_tuning_accuracy(info: &crate::session::SessionInfo) -> f64 {
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

#[cfg(test)]
mod tests {
	use super::*;
	use crate::session::SessionInfo;

	fn info_after_compression(
		predicted: f64,
		calls_at_last: usize,
		total_calls: usize,
	) -> SessionInfo {
		let mut info = SessionInfo {
			predicted_turns_at_last_compression: predicted,
			api_calls_at_last_compression: calls_at_last,
			total_api_calls: total_calls,
			..Default::default()
		};
		info.compression_stats.conversation_compressions = 1;
		info
	}

	#[test]
	fn accuracy_is_neutral_before_the_first_compression() {
		assert_eq!(calculate_self_tuning_accuracy(&SessionInfo::default()), 1.0);
	}

	#[test]
	fn growth_rate_uses_incremental_checkpoint_after_compression() {
		let mut info = SessionInfo {
			total_api_calls: 40,
			output_tokens: 100_000,
			api_calls_at_last_compression: 30,
			output_tokens_at_last_compression: 90_000,
			context_tokens_after_last_compression: 50_000,
			..Default::default()
		};
		info.compression_stats.conversation_compressions = 1;
		// Full context grew 20k over 10 calls. This includes user/tool/runtime
		// growth that output-only accounting misses.
		assert_eq!(measured_growth_rate(&info, 70_000), 2_000.0);

		info.context_tokens_after_last_compression = 0;
		info.compression_stats.conversation_compressions = 0;
		assert_eq!(measured_growth_rate(&info, 70_000), 2_500.0);
	}

	#[test]
	fn autonomous_runway_expands_until_a_user_turn_resets_the_counter() {
		assert_eq!(autonomous_runway(0), 5.0);
		assert_eq!(autonomous_runway(1), 10.0);
		assert_eq!(autonomous_runway(2), 20.0);
		assert_eq!(autonomous_runway(3), 40.0);
	}

	#[test]
	fn adaptive_fire_line_expands_but_keeps_ceiling_safety() {
		assert_eq!(adaptive_fire_line(80_000, 200_000, 0, 2_000.0, 5.0), 80_000);
		assert_eq!(
			adaptive_fire_line(80_000, 200_000, 70_000, 2_000.0, 10.0),
			90_000
		);
		assert_eq!(
			adaptive_fire_line(80_000, 200_000, 0, 2_000.0, 10.0),
			90_000,
			"a pre-first-compression rejection must also buy quiet runway"
		);
		assert_eq!(
			adaptive_fire_line(80_000, 100_000, 90_000, 2_000.0, 40.0),
			90_000
		);
	}

	#[test]
	fn accuracy_scales_down_when_we_overestimated() {
		// Predicted 20 more calls, only 10 happened → halve future estimates.
		let info = info_after_compression(20.0, 100, 110);
		assert!((calculate_self_tuning_accuracy(&info) - 0.5).abs() < 1e-9);
	}

	#[test]
	fn accuracy_scales_up_when_we_underestimated() {
		let info = info_after_compression(10.0, 100, 120);
		assert!((calculate_self_tuning_accuracy(&info) - 2.0).abs() < 1e-9);
	}

	#[test]
	fn accuracy_is_clamped_to_a_sane_band() {
		// One wild cycle must not dominate all later estimates.
		let way_over = info_after_compression(1.0, 100, 1_000);
		assert_eq!(calculate_self_tuning_accuracy(&way_over), 4.0);

		let way_under = info_after_compression(1_000.0, 100, 101);
		assert_eq!(calculate_self_tuning_accuracy(&way_under), 0.25);
	}

	#[test]
	fn accuracy_is_neutral_when_a_sample_is_degenerate() {
		// No calls since the last compression — nothing measured yet.
		assert_eq!(
			calculate_self_tuning_accuracy(&info_after_compression(20.0, 100, 100)),
			1.0
		);
		// No prediction recorded (older session logs).
		assert_eq!(
			calculate_self_tuning_accuracy(&info_after_compression(0.0, 100, 110)),
			1.0
		);
		// Counters out of order must not yield a negative multiplier.
		assert_eq!(
			calculate_self_tuning_accuracy(&info_after_compression(20.0, 200, 100)),
			1.0
		);
	}
}
