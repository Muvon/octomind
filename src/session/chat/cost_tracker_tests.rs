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

#[test]
fn test_track_exchange_cost_without_cost_field_still_counts_api_call() {
	let config = test_config();
	let mut session = ChatSession::for_tests(Vec::new());
	// Drain external-spend residue so assertions stay deterministic.
	CostTracker::track_exchange_cost(&mut session, &exchange_with_usage(None), &config)
		.expect("no-usage tracking");
	session.session.info.total_cost = 0.0;

	let usage = TokenUsage {
		input_tokens: 10,
		cache_read_tokens: 0,
		cache_write_tokens: 0,
		output_tokens: 5,
		reasoning_tokens: 0,
		total_tokens: 15,
		cost: None,
		request_time_ms: None,
	};
	CostTracker::track_exchange_cost(&mut session, &exchange_with_usage(Some(usage)), &config)
		.expect("tracking succeeds");

	let info = &session.session.info;
	assert_eq!(
		info.total_api_calls, 1,
		"usage without a cost figure is still one completed API call"
	);
	assert_eq!(info.input_tokens, 10);
	assert_eq!(info.output_tokens, 5);
	assert_eq!(info.total_api_time_ms, 0);
	assert!(
		(info.total_cost - 0.0).abs() < 1e-9,
		"unreported cost must leave the total untouched: {}",
		info.total_cost
	);
}

#[test]
fn test_track_exchange_cost_accumulates_across_exchanges() {
	let config = test_config();
	let mut session = ChatSession::for_tests(Vec::new());
	CostTracker::track_exchange_cost(&mut session, &exchange_with_usage(None), &config)
		.expect("no-usage tracking");
	session.session.info.total_cost = 0.0;
	session.session.info.total_api_calls = 0;
	session.session.info.total_api_time_ms = 0;

	for (cost, latency_ms) in [(0.25, 100u64), (0.75, 200)] {
		let usage = TokenUsage {
			input_tokens: 1,
			cache_read_tokens: 0,
			cache_write_tokens: 0,
			output_tokens: 1,
			reasoning_tokens: 0,
			total_tokens: 2,
			cost: Some(cost),
			request_time_ms: Some(latency_ms),
		};
		CostTracker::track_exchange_cost(&mut session, &exchange_with_usage(Some(usage)), &config)
			.expect("tracking succeeds");
	}

	let info = &session.session.info;
	assert!((info.total_cost - 1.0).abs() < 1e-9, "costs must sum");
	assert!(
		(session.estimated_cost - 1.0).abs() < 1e-9,
		"estimate must track the total"
	);
	assert_eq!(info.total_api_calls, 2);
	assert_eq!(info.total_api_time_ms, 300, "latencies must sum");
}

#[test]
fn test_display_cost_line_is_silent_at_zero_cost() {
	let session = ChatSession::for_tests(Vec::new());
	// Zero-cost guard must return without rendering — and without panicking.
	CostTracker::display_cost_line(&session);
	CostTracker::display_intermediate_cost_breakdown(&session);
}

#[test]
fn test_display_cost_line_skips_breakdown_when_no_tokens_tracked() {
	let mut session = ChatSession::for_tests(Vec::new());
	session.session.info.total_cost = 1.0;
	// Cost set but every token counter zero → breakdown early-returns.
	CostTracker::display_cost_line(&session);
}

#[test]
fn test_display_cost_line_known_model_renders_priced_breakdown() {
	let mut session = ChatSession::for_tests(Vec::new());
	session.session.info.total_cost = 0.01234;
	session.session.info.input_tokens = 1_000;
	session.session.info.output_tokens = 500;
	session.session.info.cache_read_tokens = 200;
	session.session.info.cache_write_tokens = 50;
	// Provider-prefixed model exercises the `split_once(':')` strip before lookup.
	session.session.info.model = "anthropic:claude-opus-4-7".to_string();
	CostTracker::display_cost_line(&session);
	CostTracker::display_intermediate_cost_breakdown(&session);
}

#[test]
fn test_display_cost_line_unknown_model_renders_token_counts() {
	let mut session = ChatSession::for_tests(Vec::new());
	session.session.info.total_cost = 0.5;
	session.session.info.input_tokens = 100;
	session.session.info.output_tokens = 40;
	session.session.info.model = "totally-unknown-model-xyz".to_string();
	// No cache tokens → short `N in · N out` form.
	CostTracker::display_cost_line(&session);
	// Cache tokens present → extended form with the cache figure.
	session.session.info.cache_read_tokens = 30;
	session.session.info.cache_write_tokens = 10;
	CostTracker::display_cost_line(&session);
}

#[test]
fn test_display_compression_result_covers_every_type_label() {
	let metrics = crate::mcp::core::plan::compression::CompressionMetrics::new(5, 1_000, 4_000);
	for compression_type in ["Task", "Phase", "Project", "Conversation", "Custom"] {
		CostTracker::display_compression_result(compression_type, &metrics);
	}
}

#[test]
fn test_display_session_usage_renders_empty_and_full_states() {
	// All counters zero → every conditional row is skipped.
	let mut session = ChatSession::for_tests(Vec::new());
	CostTracker::display_session_usage(&session);

	// Every counter non-zero → cache write, reasoning, cache read and time rows render.
	session.session.info.input_tokens = 100;
	session.session.info.cache_read_tokens = 50;
	session.session.info.cache_write_tokens = 10;
	session.session.info.output_tokens = 40;
	session.session.info.reasoning_tokens = 5;
	session.session.info.total_cost = 0.5;
	session.session.info.total_api_time_ms = 1_200;
	session.session.info.total_tool_time_ms = 300;
	session.session.info.total_layer_time_ms = 100;
	CostTracker::display_session_usage(&session);
}
