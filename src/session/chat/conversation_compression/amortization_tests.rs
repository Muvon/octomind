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

//! The fold decision behind the fire line, replayed over the session shapes
//! that broke the old dollar gate: it must not fire on a session about to end,
//! and it must fire once a long tool loop has shown its pace.

use super::*;
use crate::providers::ModelPricing;
use crate::session::{SessionInfo, TURN_HISTORY};

/// A session mid-turn: `history` = calls per completed genuine turn, `this_turn`
/// = calls made since the current turn started.
fn mid_turn(history: &[u32], this_turn: usize) -> SessionInfo {
	let start = 1000;
	SessionInfo {
		turn_call_counts: history.to_vec(),
		api_calls_at_turn_start: start,
		total_api_calls: start + this_turn,
		..Default::default()
	}
}

/// A 108k context folding to 45k: 80k drainable, 8k summary — the DuckDB shape.
fn decide(info: &SessionInfo, runway: f64, econ: FoldEconomics) -> bool {
	fold_decision(info, 108_000.0, 45_000.0, 80_000.0, 8_000.0, runway, econ)
}

fn pricing(input: f64, output: f64, cache_write: f64, cache_read: f64) -> ModelPricing {
	ModelPricing::new(input, output, cache_write, cache_read)
}

#[test]
fn short_session_does_not_fold() {
	// Eight calls into the first turn, no history: only eight calls of savings
	// are in evidence, nowhere near a fold's cost.
	let info = mid_turn(&[], 8);
	assert!(!decide(&info, 5.0, FoldEconomics::DEFAULT));
}

#[test]
fn burst_loop_folds_once_it_has_shown_its_pace() {
	// One turn, no history, calls accumulating: the fold becomes justified as
	// the loop demonstrates it will keep going.
	let first_fire = (1..=80).find(|&n| decide(&mid_turn(&[], n), 5.0, FoldEconomics::DEFAULT));
	let n = first_fire.expect("a long tool loop must fold");
	assert!(
		(15..=40).contains(&n),
		"fold fired at call {n}; expected once the pace is evident, well before the loop ends"
	);
	// Monotone: once justified it stays justified as calls keep coming.
	assert!((n..=80).all(|m| decide(&mid_turn(&[], m), 5.0, FoldEconomics::DEFAULT)));
}

#[test]
fn drip_session_folds_sparsely() {
	// Four calls per turn. Early in the session the horizon is short; after
	// ten turns the same context size is worth folding.
	let early = mid_turn(&[4, 4], 2);
	assert!(!decide(&early, 5.0, FoldEconomics::DEFAULT));
	let late = mid_turn(&[4; 10], 2);
	assert!(decide(&late, 5.0, FoldEconomics::DEFAULT));
}

#[test]
fn cross_then_stop_does_not_fold() {
	// Two six-call turns; the threshold is crossed on the last call of the
	// second turn. The pace predicts ~6 more calls — not worth a fold.
	let info = mid_turn(&[6], 6);
	assert!(!decide(&info, 5.0, FoldEconomics::DEFAULT));
}

#[test]
fn turn_boundary_folds_without_amortization() {
	// Between a user message and its first call: nothing is mid-flight, so
	// crossing the line is enough even with no history and a tall runway.
	let info = mid_turn(&[], 0);
	assert!(at_turn_boundary(&info));
	assert!(decide(&info, 40.0, FoldEconomics::DEFAULT));
	let after_one_call = mid_turn(&[], 1);
	assert!(!at_turn_boundary(&after_one_call));
}

#[test]
fn ladder_levels_demand_more_expected_calls() {
	// Same session state; each in-turn fold doubles the runway, so the k-th
	// consecutive fold needs a longer predicted horizon.
	let info = mid_turn(&[30], 10);
	assert!(decide(&info, 5.0, FoldEconomics::DEFAULT));
	assert!(decide(&info, 20.0, FoldEconomics::DEFAULT));
	assert!(!decide(&info, 80.0, FoldEconomics::DEFAULT));
}

#[test]
fn expected_calls_follow_the_session_pace() {
	// No history: only what this turn has shown, never below one.
	assert_eq!(expected_remaining_calls(&mid_turn(&[], 0)), 1.0);
	assert_eq!(expected_remaining_calls(&mid_turn(&[], 7)), 7.0);
	// Median 10 over three turns, 4 done this turn: 6 left + 10 × 3 seen.
	assert_eq!(expected_remaining_calls(&mid_turn(&[10, 2, 30], 4)), 36.0);
	// A turn already past its median still counts the calls it has made.
	assert_eq!(expected_remaining_calls(&mid_turn(&[4], 20)), 20.0);
}

#[test]
fn turn_bookkeeping_caps_history_and_skips_empty_turns() {
	let mut info = SessionInfo::default();
	info.note_turn_start();
	assert!(
		info.turn_call_counts.is_empty(),
		"a turn with no calls is no signal"
	);
	for n in 1..=(TURN_HISTORY + 4) {
		info.total_api_calls += n;
		info.note_turn_start();
		assert_eq!(info.api_calls_at_turn_start, info.total_api_calls);
	}
	assert_eq!(info.turn_call_counts.len(), TURN_HISTORY);
	assert_eq!(
		*info.turn_call_counts.last().unwrap() as usize,
		TURN_HISTORY + 4,
		"most recent turn is kept, oldest dropped"
	);
}

#[test]
fn unknown_pricing_uses_default_ratios_never_a_skip() {
	assert_eq!(
		FoldEconomics::from_pricing(None, None),
		FoldEconomics::DEFAULT
	);
	// Zero-priced session model (local): defaults, not division by zero.
	let free = pricing(0.0, 0.0, 0.0, 0.0);
	assert_eq!(
		FoldEconomics::from_pricing(Some(&free), None),
		FoldEconomics::DEFAULT
	);
	// A priced session model with an unpriced folder still yields ratios.
	let agent = pricing(4.0, 20.0, 5.0, 0.40);
	let econ = FoldEconomics::from_pricing(Some(&agent), None);
	assert!((econ.cache_read - 0.10).abs() < 1e-9);
	assert!((econ.cache_write - 1.25).abs() < 1e-9);
	assert_eq!(econ.folder_input, FoldEconomics::DEFAULT.folder_input);
	// And the decision still fires when the pace justifies it.
	assert!(decide(&mid_turn(&[30, 30], 10), 5.0, econ));
}

#[test]
fn economics_flip_with_price_ratios() {
	// Ten calls per turn, five in: ~25 expected calls.
	let info = mid_turn(&[10, 10], 5);
	// Cheap-cache agent with a folder pricier than the agent (luna + 397b):
	// folding the DuckDB shape does not pay at this horizon.
	let luna = pricing(0.20, 1.20, 0.25, 0.02);
	let big_folder = pricing(0.50, 1.50, 0.50, 0.50);
	let pricey = FoldEconomics::from_pricing(Some(&luna), Some(&big_folder));
	assert!(!decide(&info, 5.0, pricey));
	// Expensive agent with a cheap folder (sol + flash): the same state folds.
	let sol = pricing(4.0, 20.0, 5.0, 0.40);
	let flash = pricing(0.14, 0.28, 0.14, 0.014);
	let cheap = FoldEconomics::from_pricing(Some(&sol), Some(&flash));
	assert!(decide(&info, 5.0, cheap));
	// No cache discount at all: every carried token is paid in full, fold.
	let uncached = pricing(1.0, 2.0, 1.0, 1.0);
	let none = FoldEconomics::from_pricing(Some(&uncached), Some(&flash));
	assert!(decide(&mid_turn(&[8], 4), 5.0, none));
}
