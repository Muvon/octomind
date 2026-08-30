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
