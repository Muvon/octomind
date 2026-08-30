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

fn response_with_finish(finish_reason: Option<&str>) -> crate::providers::ProviderResponse {
	crate::providers::ProviderResponse {
		content: String::new(),
		exchange: crate::providers::ProviderExchange::new(
			serde_json::json!({}),
			serde_json::json!({}),
			None,
			"test",
		),
		tool_calls: None,
		thinking: None,
		finish_reason: finish_reason.map(str::to_string),
		response_id: None,
		structured_output: None,
	}
}

#[test]
fn test_check_should_continue() {
	let config =
		toml::from_str::<Config>(include_str!("../../../../config-templates/default.toml"))
			.expect("parse default config template");

	// Tool-call finish reasons continue regardless of has_more_tools
	for reason in ["tool_calls", "tool_use"] {
		assert!(check_should_continue(
			&response_with_finish(Some(reason)),
			&config,
			false
		));
	}

	// Terminal finish reasons stop even with tools pending
	for reason in ["stop", "length", "end_turn"] {
		assert!(!check_should_continue(
			&response_with_finish(Some(reason)),
			&config,
			true
		));
	}

	// Unknown finish reason is conservative: continue
	assert!(check_should_continue(
		&response_with_finish(Some("weird_reason")),
		&config,
		false
	));

	// No finish reason: fall back to whether tools are pending
	assert!(check_should_continue(
		&response_with_finish(None),
		&config,
		true
	));
	assert!(!check_should_continue(
		&response_with_finish(None),
		&config,
		false
	));
}

fn template_config() -> Config {
	toml::from_str(include_str!("../../../../config-templates/default.toml"))
		.expect("parse default config template")
}

fn full_usage() -> crate::providers::TokenUsage {
	crate::providers::TokenUsage {
		input_tokens: 10,
		cache_read_tokens: 5,
		cache_write_tokens: 3,
		output_tokens: 20,
		reasoning_tokens: 7,
		total_tokens: 45,
		cost: Some(0.5),
		request_time_ms: Some(120),
	}
}

#[test]
fn test_extract_tool_content_success_and_error() {
	let success =
		crate::mcp::McpToolResult::success("t".to_string(), "i".to_string(), "ok body".to_string());
	let error =
		crate::mcp::McpToolResult::error("t".to_string(), "i".to_string(), "bad body".to_string());

	assert_eq!(extract_tool_content(&success), "ok body");
	assert_eq!(extract_tool_content(&error), "bad body");
}

#[test]
fn test_handle_follow_up_cost_tracking_full_usage() {
	let mut session = crate::session::chat::session::ChatSession::for_tests(Vec::new());
	let config = template_config();
	let exchange = crate::providers::ProviderExchange::new(
		serde_json::json!({}),
		serde_json::json!({}),
		Some(full_usage()),
		"test",
	);

	handle_follow_up_cost_tracking(&mut session, &exchange, &config);

	let info = &session.session.info;
	assert_eq!(info.total_api_calls, 1);
	assert_eq!(info.input_tokens, 10);
	assert_eq!(info.output_tokens, 20);
	assert_eq!(info.cache_read_tokens, 5);
	assert_eq!(info.cache_write_tokens, 3);
	assert_eq!(info.reasoning_tokens, 7);
	assert_eq!(info.total_api_time_ms, 120);
	assert!((info.total_cost - 0.5).abs() < 1e-9);
	assert!((session.estimated_cost - 0.5).abs() < 1e-9);
}

#[test]
fn test_handle_follow_up_cost_tracking_raw_cost_fallback() {
	let mut session = crate::session::chat::session::ChatSession::for_tests(Vec::new());
	let config = template_config();
	let mut usage = full_usage();
	usage.cost = None;
	let exchange = crate::providers::ProviderExchange::new(
		serde_json::json!({}),
		serde_json::json!({"usage": {"cost": 0.25}}),
		Some(usage),
		"test",
	);

	handle_follow_up_cost_tracking(&mut session, &exchange, &config);

	// Normalized usage.cost absent → raw response.usage.cost is used
	assert!((session.session.info.total_cost - 0.25).abs() < 1e-9);
	assert_eq!(session.session.info.total_api_calls, 1);
}

#[test]
fn test_handle_follow_up_cost_tracking_no_usage_is_noop() {
	let mut session = crate::session::chat::session::ChatSession::for_tests(Vec::new());
	let config = template_config();
	let exchange = crate::providers::ProviderExchange::new(
		serde_json::json!({}),
		serde_json::json!({}),
		None,
		"test",
	);

	handle_follow_up_cost_tracking(&mut session, &exchange, &config);

	assert_eq!(session.session.info.total_api_calls, 0);
	assert_eq!(session.session.info.total_cost, 0.0);
}

#[test]
fn test_display_rate_limit_info_all_provider_branches() {
	// Smoke coverage: every branch only logs; the value is exercising all
	// header-combination paths without panicking.
	let exchange_with_headers =
		|provider: &str, headers: std::collections::HashMap<String, String>| {
			let mut exchange = crate::providers::ProviderExchange::new(
				serde_json::json!({}),
				serde_json::json!({}),
				None,
				provider,
			);
			exchange.rate_limit_headers = Some(headers);
			exchange
		};

	let mut headers = std::collections::HashMap::new();
	headers.insert("tokens_remaining".to_string(), "1000".to_string());
	headers.insert("tokens_limit".to_string(), "2000".to_string());
	headers.insert("input_tokens_remaining".to_string(), "900".to_string());
	headers.insert("input_tokens_limit".to_string(), "1000".to_string());
	headers.insert("output_tokens_remaining".to_string(), "500".to_string());
	headers.insert("output_tokens_limit".to_string(), "600".to_string());
	display_rate_limit_info(&exchange_with_headers("anthropic", headers));

	// Partial anthropic headers: only the tokens pair is present
	let mut partial = std::collections::HashMap::new();
	partial.insert("tokens_remaining".to_string(), "1".to_string());
	partial.insert("tokens_limit".to_string(), "2".to_string());
	display_rate_limit_info(&exchange_with_headers("anthropic", partial));

	let mut openai_headers = std::collections::HashMap::new();
	openai_headers.insert("requests_remaining".to_string(), "58".to_string());
	openai_headers.insert("requests_limit".to_string(), "60".to_string());
	openai_headers.insert("tokens_remaining".to_string(), "1000".to_string());
	openai_headers.insert("tokens_limit".to_string(), "2000".to_string());
	openai_headers.insert("request_reset".to_string(), "1h".to_string());
	display_rate_limit_info(&exchange_with_headers("openai", openai_headers));

	let mut generic_headers = std::collections::HashMap::new();
	generic_headers.insert("x-rpm".to_string(), "30".to_string());
	display_rate_limit_info(&exchange_with_headers("groq", generic_headers));

	// No headers at all: early return
	let plain = crate::providers::ProviderExchange::new(
		serde_json::json!({}),
		serde_json::json!({}),
		None,
		"test",
	);
	display_rate_limit_info(&plain);
}

#[tokio::test]
async fn test_process_tool_results_cancelled_returns_none() {
	let (tx, rx) = tokio::sync::watch::channel(false);
	tx.send(true).expect("send cancellation");

	let mut session = crate::session::chat::session::ChatSession::for_tests(Vec::new());
	let config = template_config();
	let results = vec![crate::mcp::McpToolResult::success(
		"t".to_string(),
		"i".to_string(),
		"body".to_string(),
	)];

	let outcome = process_tool_results(results, 500, &mut session, &config, "assistant", rx)
		.await
		.expect("cancelled path returns Ok(None)");

	assert!(outcome.is_none());
	// The accumulated tool time is recorded before the cancellation check
	assert_eq!(session.session.info.total_tool_time_ms, 500);
}
