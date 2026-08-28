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
