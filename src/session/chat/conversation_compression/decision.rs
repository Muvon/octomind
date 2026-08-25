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

// Compression decision math.
//
// Pure helpers — no LLM calls, no session mutation. The orchestrator in
// `mod.rs` (`should_check_compression`) consults these for the adaptive fire
// line, the fold depth, and whether a fold behind the line is amortized by the
// work this session's own pace predicts.

use crate::log_debug;
use crate::session::chat::session::ChatSession;

/// Price ratios that decide whether a fold pays, each relative to ONE uncached
/// agent input token. Ratios instead of dollars keep the rule provider-agnostic
/// and defined even when a provider publishes no pricing — the old dollar gate
/// silently disabled the soft threshold whenever a price was missing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct FoldEconomics {
	/// Cached-prefix read cost per token of context carried into a call.
	pub cache_read: f64,
	/// Folder (decision model) input cost per transcript token it is sent.
	pub folder_input: f64,
	/// Folder output cost per summary token.
	pub folder_output: f64,
	/// One-off rewrite of the post-fold prefix into the cache.
	pub cache_write: f64,
}

impl FoldEconomics {
	/// Conservative stand-ins when a price is unknown: a 10% cache discount and
	/// a folder priced like the agent itself.
	pub(super) const DEFAULT: Self = Self {
		cache_read: 0.10,
		folder_input: 1.0,
		folder_output: 3.0,
		cache_write: 1.25,
	};

	pub(super) fn from_pricing(
		session: Option<&crate::providers::ModelPricing>,
		folder: Option<&crate::providers::ModelPricing>,
	) -> Self {
		let Some(session) = session.filter(|p| p.input_price_per_1m > 0.0) else {
			return Self::DEFAULT;
		};
		let unit = session.input_price_per_1m;
		Self {
			cache_read: session.cache_read_price_per_1m / unit,
			folder_input: folder
				.map_or(Self::DEFAULT.folder_input, |f| f.input_price_per_1m / unit),
			folder_output: folder.map_or(Self::DEFAULT.folder_output, |f| {
				f.output_price_per_1m / unit
			}),
			cache_write: session.cache_write_price_per_1m / unit,
		}
	}

	pub(super) fn resolve(session: &ChatSession, config: &crate::config::Config) -> Self {
		let session_pricing = get_model_pricing(&session.model, config);
		let folder_pricing = get_model_pricing(&config.compression.decision.model, config);
		if session_pricing.is_none() || folder_pricing.is_none() {
			crate::log_info!(
				"Fold economics: no pricing for {} — using default ratios",
				if session_pricing.is_none() {
					&session.model
				} else {
					&config.compression.decision.model
				}
			);
		}
		Self::from_pricing(session_pricing.as_ref(), folder_pricing.as_ref())
	}
}

/// Fraction of the drained range the fold prompt actually sends: recent bodies
/// whole, older tool bodies trimmed to 1/ratio (see `prompt::adaptive_preview`).
pub(super) const FOLD_SENT_FRACTION: f64 = 0.45;

/// True between a genuine user message and the first API call it triggers.
pub(super) fn at_turn_boundary(info: &crate::session::SessionInfo) -> bool {
	info.total_api_calls == info.api_calls_at_turn_start
}

/// Expected API calls still to come, from this session's own pace. The median
/// calls per completed genuine turn is the rate; the rest of the current turn
/// plus one such turn per turn already seen (Lindy: a session that has run N
/// turns is expected to run about N more) is the horizon. Never below the calls
/// already made this turn, so a long tool loop qualifies as it accumulates
/// calls while a session with no history stays conservative.
pub(super) fn expected_remaining_calls(info: &crate::session::SessionInfo) -> f64 {
	let this_turn = info
		.total_api_calls
		.saturating_sub(info.api_calls_at_turn_start) as f64;
	let mut counts = info.turn_call_counts.clone();
	if counts.is_empty() {
		return this_turn.max(1.0);
	}
	counts.sort_unstable();
	let median = counts[counts.len() / 2] as f64;
	let rest_of_turn = (median - this_turn).max(0.0);
	(rest_of_turn + median * counts.len() as f64).max(this_turn)
}

/// The fold decision behind the fire line. At a turn boundary a fold loses no
/// execution state (nothing is mid-flight) and the new turn rewrites the cache
/// tail anyway, so crossing the line is enough. Mid-turn a fold must be
/// amortized: the runway ladder sets how many expected calls the k-th
/// consecutive fold needs, and the freed context times the cache discount over
/// those calls must cover what the fold costs.
pub(super) fn fold_decision(
	info: &crate::session::SessionInfo,
	current_tokens: f64,
	target_after: f64,
	compressible_tokens: f64,
	summary_tokens: f64,
	runway: f64,
	econ: FoldEconomics,
) -> bool {
	if at_turn_boundary(info) {
		return true;
	}
	let expected_calls = expected_remaining_calls(info);
	if expected_calls < runway {
		return false;
	}
	let freed = (current_tokens - target_after).max(0.0);
	let gain = freed * econ.cache_read * expected_calls;
	let cost = compressible_tokens * FOLD_SENT_FRACTION * econ.folder_input
		+ summary_tokens * econ.folder_output
		+ target_after * econ.cache_write;
	gain >= cost
}

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
/// user messages, and runtime injections. Before the first compression the
/// whole live context accumulated over the calls made so far, so
/// `current / calls` is the honest per-call rate — output-only accounting
/// missed tool-result growth (the dominant source in agent sessions),
/// underestimated growth by an order of magnitude, and made every
/// runway/safety margin a sliver that re-fired within a call or two.
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
		let full_rate = (current_tokens as f64 / (info.total_api_calls as f64).max(1.0)).max(1.0);
		full_rate.max(measured_output_growth_rate(info))
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

/// The soft trigger. Geometric per-turn ladder: the k-th consecutive
/// autonomous fold (or paid decline) in one turn doubles the line —
/// threshold, 2x, 4x… capped just under the ceiling — so a single long turn
/// earns progressively more room instead of re-folding at the same mark
/// (measured failure: 7 folds in 4 turns, each at ~80k). A genuine user turn
/// resets the level. Two floors keep it sane: never below the configured
/// threshold, and never inside `MIN_RUNWAY_TURNS × growth` of the last fold's
/// surviving context (a line the next few calls would re-cross buys nothing).
pub(super) fn adaptive_fire_line(
	configured_threshold: usize,
	ceiling: usize,
	post_tokens: usize,
	growth: f64,
	consecutive_compressions: u32,
) -> usize {
	let safety_tokens = (growth * MIN_RUNWAY_TURNS) as usize;
	let safe_ceiling = ceiling.saturating_sub(safety_tokens);
	let level = consecutive_compressions.min(16);
	let ladder = configured_threshold.saturating_mul(1usize << level);
	ladder
		.max(post_tokens.saturating_add(safety_tokens))
		.min(safe_ceiling)
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
///    growth = full-context tokens added per call (measured_growth_rate, passed
///    by caller — output-only accounting missed tool-result growth and inflated
///    this ceiling by an order of magnitude)
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

#[cfg(test)]
#[path = "amortization_tests.rs"]
mod amortization_tests;

#[cfg(test)]
mod tests {
	use super::*;
	use crate::session::SessionInfo;

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
	fn adaptive_fire_line_doubles_per_consecutive_fold() {
		// Level 0 fires at the configured threshold; each in-turn fold (or
		// paid decline) doubles it; the cap sits one safety margin under the
		// ceiling.
		assert_eq!(adaptive_fire_line(70_000, 200_000, 0, 2_000.0, 0), 70_000);
		assert_eq!(
			adaptive_fire_line(70_000, 200_000, 45_000, 2_000.0, 1),
			140_000
		);
		assert_eq!(
			adaptive_fire_line(70_000, 200_000, 45_000, 2_000.0, 2),
			190_000,
			"level 2 (280k) is capped at ceiling minus safety"
		);
	}

	#[test]
	fn fire_line_never_sits_inside_the_post_fold_safety_margin() {
		// A gentle fold that lands just under the line must not re-fire within
		// a few calls: the post-fold floor lifts the line above the watermark.
		assert_eq!(
			adaptive_fire_line(70_000, 200_000, 85_000, 2_000.0, 0),
			95_000
		);
		// The ceiling safety still wins over both floors.
		assert_eq!(
			adaptive_fire_line(70_000, 100_000, 95_000, 2_000.0, 0),
			90_000
		);
	}

	#[test]
	fn composed_in_turn_cycles_double_the_fire_line_until_the_cap() {
		// Production wiring: fold → consecutive_compressions += 1 → next check.
		// One long turn must see 70k → 140k → 190k (cap), never a re-fold at
		// the same mark; a genuine user turn resets to the threshold.
		let threshold = 70_000;
		let ceiling = 200_000;
		let growth = 2_000.0;
		let lines: Vec<usize> = (0..4u32)
			.map(|k| adaptive_fire_line(threshold, ceiling, 45_000, growth, k))
			.collect();
		assert_eq!(lines, vec![70_000, 140_000, 190_000, 190_000]);
		assert!(lines.windows(2).all(|w| w[1] >= w[0]));
		// User turn resets the level (consecutive_compressions = 0).
		assert_eq!(
			adaptive_fire_line(threshold, ceiling, 45_000, growth, 0),
			70_000
		);
	}

	#[test]
	fn pre_compression_growth_uses_full_context_when_it_dominates() {
		// Regression: tool-result growth dominates agent sessions. Before the
		// first compression the fallback must not underestimate growth by
		// looking at assistant output alone.
		let info = SessionInfo {
			total_api_calls: 40,
			output_tokens: 10_000,
			..Default::default()
		};
		// 200k live context over 40 calls = 5k/call vs 250/call output-only.
		assert_eq!(measured_growth_rate(&info, 200_000), 5_000.0);
	}

	#[test]
	fn growth_rate_floors_at_one_when_context_shrank_below_watermark() {
		// Dedup/truncation can leave the live context BELOW the last watermark;
		// the rate must floor at 1, not go negative or divide runways by zero.
		let mut info = SessionInfo {
			total_api_calls: 35,
			api_calls_at_last_compression: 30,
			context_tokens_after_last_compression: 50_000,
			..Default::default()
		};
		info.compression_stats.conversation_compressions = 1;
		assert_eq!(measured_growth_rate(&info, 40_000), 1.0);
	}

	#[test]
	fn growth_rate_survives_zero_calls_since_compression() {
		// Checked immediately after a fold (total == at_last): the divisor
		// clamps to 1 instead of dividing by zero.
		let mut info = SessionInfo {
			total_api_calls: 30,
			api_calls_at_last_compression: 30,
			context_tokens_after_last_compression: 50_000,
			..Default::default()
		};
		info.compression_stats.conversation_compressions = 1;
		assert_eq!(measured_growth_rate(&info, 56_000), 6_000.0);
	}

	#[test]
	fn pre_compression_growth_floors_at_output_rate() {
		// Dedup can shrink the live context below what the model actually
		// produced; the lifetime fallback must not underestimate below the
		// measured output rate.
		let info = SessionInfo {
			total_api_calls: 10,
			output_tokens: 50_000,
			..Default::default()
		};
		// full rate = 30k/10 = 3k/call, output rate = 50k/10 = 5k/call.
		assert_eq!(measured_growth_rate(&info, 30_000), 5_000.0);
	}

	#[test]
	fn depth_targets_runway_below_fire_line() {
		// desired_after = fire − runway·growth lands inside the achievable band:
		// the ratio reproduces it exactly and honors the re-fire guarantee.
		let ratio = compression_depth(100_000, 70_000, 100_000, 2_000.0, 20.0).unwrap();
		let surviving = 30_000.0;
		assert!((ratio - 70_000.0 / 30_000.0).abs() < 1e-9);
		let post = surviving + 70_000.0 / ratio;
		assert!(post <= 100_000.0 - 2_000.0 * MIN_RUNWAY_TURNS);
	}

	#[test]
	fn depth_clamps_to_gentlest_and_deepest() {
		// Short runway asks for a post-state above the gentlest achievable →
		// clamp to MIN ratio.
		let gentle = compression_depth(100_000, 70_000, 100_000, 2_000.0, 5.0).unwrap();
		assert!((gentle - MIN_COMPRESSION_RATIO).abs() < 1e-9);

		// Huge runway asks for a post-state below the deepest achievable →
		// clamp to MAX ratio.
		let deep = compression_depth(100_000, 70_000, 100_000, 2_000.0, 40.0).unwrap();
		assert!((deep - MAX_COMPRESSION_RATIO).abs() < 1e-9);
	}

	#[test]
	fn depth_is_none_when_no_fold_lands_below_refire_bound() {
		// refire_bound = 40k − 10k = 30k, but even a 16x fold leaves
		// 30k + 70k/16 ≈ 34.4k — compressing would re-fire immediately.
		assert_eq!(
			compression_depth(100_000, 70_000, 40_000, 2_000.0, 5.0),
			None
		);
		// Nothing to compress is never feasible.
		assert_eq!(compression_depth(100_000, 0, 100_000, 2_000.0, 5.0), None);
	}
}
