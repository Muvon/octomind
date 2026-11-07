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
fn ratio_and_runway_constants_hold_their_documented_values() {
	assert_eq!(MIN_COMPRESSION_RATIO, 2.0);
	assert_eq!(MAX_COMPRESSION_RATIO, 16.0);
	assert_eq!(MIN_RUNWAY_TURNS, 5.0);
	assert_eq!(FOLD_SENT_FRACTION, 0.45);
}

#[test]
fn ceiling_reached_uses_the_bare_ceiling_before_a_pace_is_measured() {
	// Fewer than MIN_RUNWAY_TURNS calls since the last compression: the
	// per-call rate is one or two samples, so only the bare ceiling applies.
	let info = SessionInfo::default();
	assert!(!ceiling_reached(&info, 99_999, 100_000));
	assert!(ceiling_reached(&info, 100_000, 100_000));
}

#[test]
fn ceiling_reached_fires_early_once_growth_projects_past_the_ceiling() {
	// Five calls since the last fold, watermark at 50k: the measured pace
	// earns a safety margin of growth × MIN_RUNWAY_TURNS below the ceiling.
	let info = SessionInfo {
		total_api_calls: 10,
		api_calls_at_last_compression: 5,
		context_tokens_after_last_compression: 50_000,
		..Default::default()
	};
	// growth = (60k − 50k) / 5 = 2k → margin 10k → 70k still under 100k.
	assert!(!ceiling_reached(&info, 60_000, 100_000));
	// growth = (95k − 50k) / 5 = 9k → margin 45k → 140k ≥ 100k: forced.
	assert!(ceiling_reached(&info, 95_000, 100_000));
}

#[test]
fn context_ceiling_falls_back_to_the_configured_limit_for_unresolvable_models() {
	let mut session = ChatSession::for_tests(Vec::new());
	session.model = "notaprovider:no-such-model".to_string();
	let mut config = crate::session::chat::test_support::fake_provider_config();

	config.max_session_tokens_threshold = 50_000;
	assert_eq!(context_ceiling(&session, &config), 50_000);

	// Zero means "no explicit safety limit" and an unresolvable model
	// contributes no physical bound either — nothing caps the session.
	config.max_session_tokens_threshold = 0;
	assert_eq!(context_ceiling(&session, &config), usize::MAX);
}

#[test]
fn get_model_pricing_rejects_ids_without_a_provider_prefix() {
	let config = crate::session::chat::test_support::fake_provider_config();
	assert!(get_model_pricing("claude-sonnet-4", &config).is_none());
}

// ---------------------------------------------------------------------------
// The deferral reason: the one thing a bare `false` cannot carry. Its truth
// table decides whether a veto is honoured or overruled.
// ---------------------------------------------------------------------------

fn deferral(reason: DeferReason, step: Option<usize>) -> FoldDeferral {
	FoldDeferral { reason, step }
}

#[test]
fn deferral_stands_only_while_the_same_step_is_open() {
	// An in-flight claim about step 1, corroborated by that same step still
	// being open and an agent that does not report itself blocked.
	assert!(deferral(DeferReason::MidDerivation, Some(1)).stands_against(Some(1), false));
	assert!(deferral(DeferReason::VerificationInFlight, Some(3)).stands_against(Some(3), false));

	// The plan ADVANCED past the step the decline was about: the claim is about
	// work that is no longer in flight, so it lapses. This is what stops one
	// paid decline from suppressing every soft fold for the plan's whole life —
	// a held round makes no fold call, so nothing can re-record the reason.
	assert!(!deferral(DeferReason::MidDerivation, Some(1)).stands_against(Some(2), false));

	// The same claim with no plan step behind it, with the plan gone, or with
	// the agent reporting itself blocked, is a premise the runtime can refute.
	assert!(!deferral(DeferReason::MidDerivation, None).stands_against(None, false));
	assert!(!deferral(DeferReason::MidDerivation, Some(1)).stands_against(None, false));
	assert!(!deferral(DeferReason::MidDerivation, None).stands_against(Some(1), false));
	assert!(!deferral(DeferReason::MidDerivation, Some(1)).stands_against(Some(1), true));

	// Reasons that never assert a running step are refuted by construction,
	// even with the same step still open.
	for reason in [DeferReason::None, DeferReason::Stuck, DeferReason::TranscriptMinimal] {
		assert!(
			!deferral(reason, Some(1)).stands_against(Some(1), false),
			"{reason:?} must never stand"
		);
	}
}

#[test]
fn veto_is_offered_only_when_the_runtime_could_check_it() {
	assert!(veto_checkable(Some(0), false));
	// No plan: nothing could corroborate an in-flight claim, so asking would
	// only buy a decline the next round has to overrule.
	assert!(!veto_checkable(None, false));
	// A blocked agent is the case a fold exists for, plan or not.
	assert!(!veto_checkable(Some(0), true));
	assert!(!veto_checkable(None, true));
}

/// Sonnet-shaped prices: write 1.25x and read 0.1x the input, output 5x.
fn cache_priced() -> crate::providers::ModelPricing {
	crate::providers::ModelPricing::new(3.0, 15.0, 3.75, 0.30)
}

/// Session and folder both on `pricing`.
fn econ(pricing: &crate::providers::ModelPricing) -> FoldEconomics {
	FoldEconomics::from_known_pricing(Some(pricing), Some(pricing)).expect("priced")
}

/// A session whose completed turns each made `calls` API calls, so a new turn
/// expects `2 * calls` more.
fn paced_session(calls: u32) -> SessionInfo {
	SessionInfo {
		turn_call_counts: vec![calls],
		..Default::default()
	}
}

// The fold under test: 100k of history, 90k of it compressible, leaving 20k
// with a 9k summary. The folder call costs 90k · 0.45 · 1 + 9k · 5 = 85.5k
// input-token units on a same-priced folder.
const HISTORY: f64 = 100_000.0;
const AFTER: f64 = 20_000.0;
const COMPRESSIBLE: f64 = 90_000.0;
const SUMMARY: f64 = 9_000.0;

#[test]
fn fold_call_cost_is_the_folder_input_and_output() {
	let cost = fold_call_cost(COMPRESSIBLE, SUMMARY, econ(&cache_priced()));
	assert!((cost - 85_500.0).abs() < 1e-6, "folder call cost: {cost}");
}

#[test]
fn idle_fold_pays_on_the_write_premium_alone() {
	// One expected call writes the prefix and nothing reads it after:
	// 1.25 · 80k = 100k saved against the 85.5k folder call.
	let info = SessionInfo::default();
	assert!(idle_fold_pays(
		&info,
		HISTORY,
		AFTER,
		COMPRESSIBLE,
		SUMMARY,
		econ(&cache_priced())
	));
}

#[test]
fn idle_fold_without_a_write_premium_needs_later_reads() {
	// Write billed as input: 1 · 80k = 80k < 85.5k with no later reads.
	let at_input = crate::providers::ModelPricing::new(3.0, 15.0, 3.0, 0.30);
	assert!(!idle_fold_pays(
		&SessionInfo::default(),
		HISTORY,
		AFTER,
		COMPRESSIBLE,
		SUMMARY,
		econ(&at_input)
	));
	// Two completed one-call turns expect 3 calls: 2 later reads add
	// 2 · 0.1 · 80k = 16k, so 96k > 85.5k.
	let info = SessionInfo {
		turn_call_counts: vec![1, 1],
		..Default::default()
	};
	assert_eq!(expected_remaining_calls(&info), 3.0);
	assert!(idle_fold_pays(
		&info,
		HISTORY,
		AFTER,
		COMPRESSIBLE,
		SUMMARY,
		econ(&at_input)
	));
}

#[test]
fn idle_fold_treats_a_zero_write_price_as_the_input_price() {
	let info = paced_session(1);
	let unbilled = crate::providers::ModelPricing::new(3.0, 15.0, 0.0, 0.30);
	let at_input = crate::providers::ModelPricing::new(3.0, 15.0, 3.0, 0.30);
	for after in [10_000.0, AFTER, 40_000.0] {
		assert_eq!(
			idle_fold_pays(&info, HISTORY, after, COMPRESSIBLE, SUMMARY, econ(&unbilled)),
			idle_fold_pays(&info, HISTORY, after, COMPRESSIBLE, SUMMARY, econ(&at_input)),
		);
	}
}

#[test]
fn idle_fold_skips_when_the_folder_costs_more_than_it_saves() {
	// A folder at 10x the session price: 90k · 0.45 · 10 + 9k · 50 = 855k
	// against 100k saved on the rewrite.
	let expensive = crate::providers::ModelPricing::new(30.0, 150.0, 37.5, 3.0);
	let econ = FoldEconomics::from_known_pricing(Some(&cache_priced()), Some(&expensive)).expect("priced");
	assert!(!idle_fold_pays(
		&SessionInfo::default(),
		HISTORY,
		AFTER,
		COMPRESSIBLE,
		SUMMARY,
		econ
	));
	// Enough later reads still repay it: 219 reads · 0.1 · 80k = 1.75M.
	assert!(idle_fold_pays(
		&paced_session(110),
		HISTORY,
		AFTER,
		COMPRESSIBLE,
		SUMMARY,
		econ
	));
}

#[test]
fn idle_fold_saves_nothing_when_the_fold_frees_nothing() {
	assert!(!idle_fold_pays(
		&paced_session(110),
		HISTORY,
		HISTORY,
		COMPRESSIBLE,
		SUMMARY,
		econ(&cache_priced())
	));
}

#[test]
fn idle_economics_need_both_prices() {
	let priced = cache_priced();
	let free = crate::providers::ModelPricing::new(0.0, 0.0, 0.0, 0.0);
	assert!(FoldEconomics::from_known_pricing(None, Some(&priced)).is_none());
	assert!(FoldEconomics::from_known_pricing(Some(&priced), None).is_none());
	assert!(FoldEconomics::from_known_pricing(Some(&free), Some(&priced)).is_none());
	assert_eq!(
		FoldEconomics::from_known_pricing(Some(&priced), Some(&priced)),
		Some(FoldEconomics::from_pricing(Some(&priced), Some(&priced)))
	);
}

fn touched(at: u64, lifetime: Option<u64>, model: Option<&str>) -> SessionInfo {
	SessionInfo {
		last_cache_touch: Some(at),
		cache_lifetime_secs: lifetime,
		cache_model: model.map(str::to_string),
		..Default::default()
	}
}

#[test]
fn cache_expires_once_the_idle_gap_reaches_the_lifetime() {
	let info = touched(1_000, Some(300), Some("anthropic:a"));
	assert!(!cache_known_expired(&info, "anthropic:a", 1_299));
	assert!(cache_known_expired(&info, "anthropic:a", 1_300));
}

#[test]
fn another_models_cache_is_gone_for_the_current_model() {
	// Touched a second ago, but under the model the session ran before.
	let info = touched(1_000, Some(3_600), Some("anthropic:a"));
	assert!(cache_known_expired(&info, "openai:b", 1_001));
	// No lifetime needed: the new model never wrote a cache.
	let info = touched(1_000, None, Some("anthropic:a"));
	assert!(cache_known_expired(&info, "openai:b", 1_001));
}

#[test]
fn nothing_known_means_not_expired() {
	assert!(!cache_known_expired(&SessionInfo::default(), "anthropic:a", 1_000_000));
	// Same model, no known lifetime.
	assert!(!cache_known_expired(
		&touched(1_000, None, Some("anthropic:a")),
		"anthropic:a",
		1_000_000
	));
	// A touch whose model is unknown proves nothing.
	assert!(!cache_known_expired(
		&touched(1_000, Some(300), None),
		"anthropic:a",
		1_000_000
	));
}
