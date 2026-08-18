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
use crate::session::TokenUsage;

fn test_config() -> Config {
	toml::from_str(include_str!("../../../config-templates/default.toml"))
		.expect("parse default config template")
}

fn exchange_with_usage(usage: Option<TokenUsage>) -> ProviderExchange {
	ProviderExchange::new(serde_json::json!({}), serde_json::json!({}), usage, "test")
}

#[test]
fn test_track_exchange_cost_accumulates_everything() {
	let config = test_config();
	let mut session = ChatSession::for_tests(Vec::new());
	// Drain any external-spend residue left by other tests sharing the
	// global ledger, so the cost assertion below is deterministic.
	CostTracker::track_exchange_cost(&mut session, &exchange_with_usage(None), &config)
		.expect("no-usage tracking");
	session.session.info.total_cost = 0.0;
	session.session.info.total_api_calls = 0;

	let usage = TokenUsage {
		input_tokens: 100,
		cache_read_tokens: 20,
		cache_write_tokens: 10,
		output_tokens: 50,
		reasoning_tokens: 5,
		total_tokens: 185,
		cost: Some(0.5),
		request_time_ms: Some(250),
	};
	CostTracker::track_exchange_cost(&mut session, &exchange_with_usage(Some(usage)), &config)
		.expect("tracking succeeds");

	let info = &session.session.info;
	assert!((info.total_cost - 0.5).abs() < 1e-9);
	assert!((session.estimated_cost - 0.5).abs() < 1e-9);
	assert_eq!(info.total_api_calls, 1);
	assert_eq!(info.total_api_time_ms, 250);
	assert_eq!(info.input_tokens, 100);
	assert_eq!(info.output_tokens, 50);
	assert_eq!(info.cache_read_tokens, 20);
	assert_eq!(info.cache_write_tokens, 10);
	assert_eq!(info.reasoning_tokens, 5);
	// Threshold counters: total includes cache reads, non-cached does not
	assert_eq!(info.current_total_tokens, 120);
	assert_eq!(info.current_non_cached_tokens, 100);
}

#[test]
fn test_track_exchange_without_usage_counts_nothing() {
	let config = test_config();
	let mut session = ChatSession::for_tests(Vec::new());
	CostTracker::track_exchange_cost(&mut session, &exchange_with_usage(None), &config)
		.expect("tracking succeeds");

	let info = &session.session.info;
	// No usage payload → no API call counted, no tokens attributed
	assert_eq!(info.total_api_calls, 0);
	assert_eq!(info.input_tokens, 0);
	assert_eq!(info.total_api_time_ms, 0);
}
