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

#[tokio::test]
async fn test_process_tool_results_request_spending_stop_returns_none() {
	let mut config = template_config();
	config.supervisor.enabled = true;
	config.supervisor.plan.enabled = true;
	config.max_request_spending_threshold = 0.0001;

	let mut session = crate::session::chat::session::ChatSession::for_tests(Vec::new());
	session.session.info.total_cost = 1.0;
	session.request_spending_checkpoint = 0.0;

	let results = vec![
		crate::mcp::McpToolResult::success(
			"view".to_string(),
			"i1".to_string(),
			"body one".to_string(),
		),
		crate::mcp::McpToolResult::error("shell".to_string(), "i2".to_string(), "boom".to_string()),
	];
	let (_tx, rx) = tokio::sync::watch::channel(false);

	let outcome = process_tool_results(results, 700, &mut session, &config, "assistant", rx)
		.await
		.expect("spending stop is a clean exit, not an error");

	assert!(outcome.is_none());
	// Both tool results were appended as tool-role messages before the stop
	let tool_messages: Vec<&crate::session::Message> = session
		.session
		.messages
		.iter()
		.filter(|m| m.role == "tool")
		.collect();
	assert_eq!(tool_messages.len(), 2);
	assert_eq!(tool_messages[0].content, "body one");
	assert_eq!(tool_messages[1].content, "boom");
	assert_eq!(session.session.info.total_tool_time_ms, 700);
}

#[tokio::test]
async fn test_process_tool_results_follow_up_final_answer() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let url = crate::session::chat::test_support::spawn_stub(vec![
		crate::session::chat::test_support::final_response("All done"),
	])
	.await;
	std::env::set_var("OLLAMA_API_URL", &url);

	let config = crate::session::chat::test_support::fake_provider_config();
	let mut session = crate::session::chat::test_support::fake_session("do the work");
	let results = vec![crate::mcp::McpToolResult::success(
		"view".to_string(),
		"i1".to_string(),
		"body".to_string(),
	)];
	let (_tx, rx) = tokio::sync::watch::channel(false);

	let outcome = process_tool_results(results, 10, &mut session, &config, "assistant", rx)
		.await
		.expect("follow-up round trip");

	let (content, _exchange, tool_calls, _response_id, _thinking) =
		outcome.expect("final answer is Some");
	assert!(content.contains("All done"));
	assert!(tool_calls.is_none());
	assert_eq!(session.session.info.total_api_calls, 1);
	assert!(session.session.info.total_cost > 0.0);
}

#[tokio::test]
async fn test_process_tool_results_follow_up_more_tools_continues() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let url = crate::session::chat::test_support::spawn_stub(vec![
		crate::session::chat::test_support::tool_call_response("view", serde_json::json!({})),
	])
	.await;
	std::env::set_var("OLLAMA_API_URL", &url);

	let config = crate::session::chat::test_support::fake_provider_config();
	let mut session = crate::session::chat::test_support::fake_session("keep going");
	let results = vec![crate::mcp::McpToolResult::success(
		"view".to_string(),
		"i1".to_string(),
		"body".to_string(),
	)];
	let (_tx, rx) = tokio::sync::watch::channel(false);

	let outcome = process_tool_results(results, 10, &mut session, &config, "assistant", rx)
		.await
		.expect("follow-up round trip");

	let (_content, _exchange, tool_calls, _response_id, _thinking) =
		outcome.expect("tool_calls finish keeps the loop alive");
	let calls = tool_calls.expect("structured tool calls present");
	assert_eq!(calls.len(), 1);
	assert_eq!(calls[0].tool_name, "view");
}

#[tokio::test]
async fn test_process_tool_results_injects_pending_hints_before_follow_up() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let url = crate::session::chat::test_support::spawn_stub(vec![
		crate::session::chat::test_support::final_response("noted"),
	])
	.await;
	std::env::set_var("OLLAMA_API_URL", &url);

	let session_id = "tool-result-hints-test".to_string();
	crate::session::context::with_session_id(session_id.clone(), async {
		crate::mcp::hint_accumulator::push_hint("Prefer `view` over shell cat");

		let config = crate::session::chat::test_support::fake_provider_config();
		let mut session = crate::session::chat::test_support::fake_session("do the work");
		let results = vec![crate::mcp::McpToolResult::success(
			"shell".to_string(),
			"i1".to_string(),
			"body".to_string(),
		)];
		let (_tx, rx) = tokio::sync::watch::channel(false);

		let outcome = process_tool_results(results, 10, &mut session, &config, "assistant", rx)
			.await
			.expect("follow-up round trip");
		assert!(outcome.is_some());

		let hint_message = session
			.session
			.messages
			.iter()
			.find(|m| m.role == "user" && m.content.contains("Tool usage notice"))
			.expect("hint injected as user message");
		assert!(hint_message
			.content
			.contains("Prefer `view` over shell cat"));

		crate::session::context::cleanup_session(&session_id);
	})
	.await;
}

#[tokio::test]
async fn test_process_tool_results_injects_supervisor_steer_note() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let url = crate::session::chat::test_support::spawn_stub(vec![
		crate::session::chat::test_support::final_response("steered"),
	])
	.await;
	std::env::set_var("OLLAMA_API_URL", &url);

	let config = crate::session::chat::test_support::fake_provider_config();
	let mut session = crate::session::chat::test_support::fake_session("do the work");
	session.steer_pending = Some("<pay-attention>change approach</pay-attention>".to_string());
	let results = vec![crate::mcp::McpToolResult::success(
		"view".to_string(),
		"i1".to_string(),
		"body".to_string(),
	)];
	let (_tx, rx) = tokio::sync::watch::channel(false);

	let outcome = process_tool_results(results, 10, &mut session, &config, "assistant", rx)
		.await
		.expect("follow-up round trip");
	assert!(outcome.is_some());

	assert!(
		session
			.session
			.messages
			.iter()
			.any(|m| m.role == "user" && m.content.contains("change approach")),
		"steer note injected during the tool loop"
	);
	assert!(session.steer_pending.is_none());
}

#[tokio::test]
async fn test_process_tool_results_follow_up_error_propagates() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let error_body = serde_json::json!({"error": {"message": "upstream exploded"}});
	let url = crate::session::chat::test_support::spawn_stub_with_status(vec![
		(500, error_body.clone()),
		(500, error_body.clone()),
		(500, error_body),
	])
	.await;
	std::env::set_var("OLLAMA_API_URL", &url);

	let config = crate::session::chat::test_support::fake_provider_config();
	let mut session = crate::session::chat::test_support::fake_session("do the work");
	session.max_retries = 0;
	let results = vec![crate::mcp::McpToolResult::success(
		"view".to_string(),
		"i1".to_string(),
		"body".to_string(),
	)];
	let (_tx, rx) = tokio::sync::watch::channel(false);

	let outcome = process_tool_results(results, 10, &mut session, &config, "assistant", rx).await;
	assert!(
		outcome.is_err(),
		"provider failure must propagate to the caller"
	);
}

#[tokio::test]
async fn test_process_tool_results_delivers_background_results_mid_turn() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let url = crate::session::chat::test_support::spawn_stub(vec![
		crate::session::chat::test_support::final_response("acked"),
	])
	.await;
	std::env::set_var("OLLAMA_API_URL", &url);

	let session_id = "tool-result-inbox-test".to_string();
	crate::session::context::with_session_id(session_id.clone(), async {
		crate::session::inbox::init_inbox_for_session();
		crate::session::inbox::push_inbox_message(crate::session::inbox::InboxMessage {
			source: crate::session::inbox::InboxSource::BackgroundJob {
				id: "80551-17".to_string(),
			},
			content: "<background_job>exit 0</background_job>".to_string(),
		});
		crate::session::inbox::push_inbox_message(crate::session::inbox::InboxMessage {
			source: crate::session::inbox::InboxSource::Inject,
			content: "a new user request".to_string(),
		});

		let config = crate::session::chat::test_support::fake_provider_config();
		let mut session = crate::session::chat::test_support::fake_session("do the work");
		let results = vec![crate::mcp::McpToolResult::success(
			"shell".to_string(),
			"i1".to_string(),
			"tool body".to_string(),
		)];
		let (_tx, rx) = tokio::sync::watch::channel(false);

		let outcome = process_tool_results(results, 10, &mut session, &config, "assistant", rx)
			.await
			.expect("follow-up round trip");
		assert!(outcome.is_some());

		let tool_index = session
			.session
			.messages
			.iter()
			.position(|m| m.role == "tool")
			.expect("tool result recorded");
		let job_index = session
			.session
			.messages
			.iter()
			.position(|m| m.role == "user" && m.content.contains("exit 0"))
			.expect("job result delivered inside the turn");
		assert!(
			job_index > tool_index,
			"the result lands after this round's tool output, never between the call and its result"
		);

		// A human-shaped injection is not delivered mid-turn — it owns its own turn.
		assert!(!session
			.session
			.messages
			.iter()
			.any(|m| m.content.contains("a new user request")));
		let queued =
			crate::session::inbox::try_pop_inbox_message().expect("user injection still queued");
		assert_eq!(queued.content, "a new user request");

		crate::session::context::cleanup_session(&session_id);
	})
	.await;
}
