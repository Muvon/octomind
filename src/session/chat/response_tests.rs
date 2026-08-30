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
use crate::session::chat::test_support::{
	fake_provider_config, fake_session, final_response, spawn_stub, tool_call_response, ENV_LOCK,
};
use serde_json::json;

#[test]
fn test_preview_value_strings() {
	assert_eq!(preview_value(&json!("short")), "\"short\"");
	// Newlines flatten to spaces
	assert_eq!(preview_value(&json!("a\nb")), "\"a b\"");
	// Over 60 chars → truncated at 59 + ellipsis
	let long = "x".repeat(80);
	let preview = preview_value(&json!(long));
	assert_eq!(preview, format!("\"{}…\"", "x".repeat(59)));
}

#[test]
fn test_preview_value_arrays() {
	assert_eq!(preview_value(&json!([])), "[]");
	assert_eq!(preview_value(&json!(["only"])), "[\"only\"]");
	// Range-like scalar pair shows both values
	assert_eq!(preview_value(&json!([1, 150])), "[1, 150]");
	// Longer arrays collapse to first + count
	assert_eq!(preview_value(&json!([1, 2, 3])), "[1, +2]");
	// Two-element array with a non-scalar member is not a range pair
	assert_eq!(preview_value(&json!([1, {"k": 2}])), "[1, +1]");
}

#[test]
fn test_preview_value_scalars_and_objects() {
	assert_eq!(preview_value(&json!({"a": 1})), "{…}");
	assert_eq!(preview_value(&json!(null)), "null");
	assert_eq!(preview_value(&json!(42)), "42");
	assert_eq!(preview_value(&json!(true)), "true");
}

#[test]
fn test_resolve_tool_calls() {
	let call = crate::mcp::McpToolCall {
		tool_name: "shell".to_string(),
		parameters: json!({"cmd": "ls"}),
		tool_id: "id1".to_string(),
	};
	let mut some_calls = Some(vec![call]);
	let resolved = resolve_tool_calls(&mut some_calls, "ignored");
	assert_eq!(resolved.len(), 1);
	assert_eq!(resolved[0].tool_name, "shell");
	// The Option is consumed
	assert!(some_calls.is_none());

	let mut none_calls = None;
	assert!(resolve_tool_calls(&mut none_calls, "ignored").is_empty());
}

#[test]
fn test_check_cancellation() {
	let (tx, rx) = tokio::sync::watch::channel(false);
	assert!(check_cancellation(&rx).is_ok());

	tx.send(true).expect("send cancellation");
	let err = check_cancellation(&rx).expect_err("cancelled must error");
	assert!(crate::session::cancellation::is_cancelled(&err));
}

#[test]
fn test_capture_self_report_credits_only_ids_in_active_pack() {
	let mut session = crate::session::chat::session::ChatSession::for_tests(Vec::new());
	session.recalled_refs = vec![
		(
			"M1".to_string(),
			"first".to_string(),
			"role".to_string(),
			"project".to_string(),
		),
		(
			"M2".to_string(),
			"second".to_string(),
			"role".to_string(),
			"project".to_string(),
		),
	];
	let mut config = crate::session::chat::test_support::fake_provider_config();
	config.supervisor.enabled = true;
	let content = r#"answer
<sup>{"state":"progressing","focus":"used one memory","next":"continue","carry":[],"plan":null,"memories":["M2","M9"]}</sup>"#;
	let visible = capture_self_report(&mut session, &config, content);
	assert_eq!(visible, "answer");
	assert_eq!(session.used_memory_ids.len(), 1);
	assert!(session.used_memory_ids.contains("M2"));
}

fn template_config() -> Config {
	toml::from_str(include_str!("../../../config-templates/default.toml"))
		.expect("parse default config template")
}

/// OutputSink that records every emitted message for inspection.
#[derive(Clone)]
struct RecordingSink(std::sync::Arc<std::sync::Mutex<Vec<ServerMessage>>>);

impl OutputSink for RecordingSink {
	fn emit(&self, msg: ServerMessage) {
		self.0.lock().expect("sink lock").push(msg);
	}
}

#[test]
fn test_handle_final_response_records_assistant_message() {
	let mut session = crate::session::chat::session::ChatSession::for_tests(Vec::new());
	let config = template_config();

	handle_final_response(
		"final answer",
		&None,
		Some("resp_9".to_string()),
		&mut session,
		&config,
		"assistant",
		OutputMode::NonInteractive,
	)
	.expect("final response processing");

	assert_eq!(session.session.messages.len(), 1);
	let message = &session.session.messages[0];
	assert_eq!(message.role, "assistant");
	assert_eq!(message.content, "final answer");
	assert_eq!(message.id.as_deref(), Some("resp_9"));
	assert_eq!(session.last_response, "final answer");
	assert_eq!(session.turn_answers, vec!["final answer".to_string()]);
}

#[test]
fn test_handle_final_response_blank_content_skips_turn_answer() {
	let mut session = crate::session::chat::session::ChatSession::for_tests(Vec::new());
	let config = template_config();

	handle_final_response(
		"   ",
		&None,
		None,
		&mut session,
		&config,
		"assistant",
		OutputMode::NonInteractive,
	)
	.expect("final response processing");

	// Message is still recorded, but blank content is not a turn deliverable
	assert_eq!(session.session.messages.len(), 1);
	assert!(session.turn_answers.is_empty());
}

#[test]
fn test_add_assistant_message_with_tool_calls_preserves_exchange_shape() {
	let mut session = crate::session::chat::session::ChatSession::for_tests(Vec::new());
	let config = template_config();
	let exchange = ProviderExchange::new(
		json!({}),
		json!({"tool_calls": [{"id": "c1", "type": "function", "function": {"name": "shell", "arguments": "{}"}}]}),
		None,
		"test",
	);
	let thinking = Some(ThinkingBlock::new("pondering"));

	add_assistant_message_with_tool_calls(
		&mut session,
		"running tools",
		&exchange,
		Some("resp_1".to_string()),
		&thinking,
		&config,
		"assistant",
	)
	.expect("assistant message with tool calls");

	assert_eq!(session.session.messages.len(), 1);
	let message = &session.session.messages[0];
	assert_eq!(message.role, "assistant");
	// Unified-format tool_calls are stored verbatim from the exchange
	let calls = message.tool_calls.as_ref().expect("tool_calls preserved");
	assert_eq!(calls[0]["id"], json!("c1"));
	assert_eq!(calls[0]["function"]["name"], json!("shell"));
	// Thinking block is serialized onto the message
	assert!(message.thinking.is_some());
	assert_eq!(message.id.as_deref(), Some("resp_1"));
	// A message carrying tool calls is work in progress, not a turn answer
	assert!(session.turn_answers.is_empty());
	assert_eq!(session.last_response, "running tools");
}

#[test]
fn test_add_assistant_message_without_tool_calls_records_turn_answer() {
	let mut session = crate::session::chat::session::ChatSession::for_tests(Vec::new());
	let config = template_config();
	let exchange = ProviderExchange::new(json!({}), json!({}), None, "test");

	add_assistant_message_with_tool_calls(
		&mut session,
		"the answer",
		&exchange,
		None,
		&None,
		&config,
		"assistant",
	)
	.expect("assistant message");

	let message = &session.session.messages[0];
	assert!(message.tool_calls.is_none());
	assert_eq!(session.turn_answers, vec!["the answer".to_string()]);
}

#[test]
fn test_capture_self_report_disabled_returns_content_verbatim_and_clears_state() {
	let mut config = template_config();
	config.supervisor.enabled = false;
	let mut session = crate::session::chat::session::ChatSession::for_tests(Vec::new());
	session.last_self_report = Some(crate::supervisor::detect::SelfReport::Progressing);
	session.last_self_report_reason = Some("stale".to_string());
	session.pending_plan_signal = Some(crate::supervisor::plan::PlanSignal::Request);

	let content = "answer\n<sup>{\"state\":\"done\",\"focus\":\"f\",\"next\":null,\"carry\":[],\"plan\":null,\"memories\":[],\"behaviors\":[]}</sup>";
	let visible = capture_self_report(&mut session, &config, content);

	// Disabled supervisor: no stripping, and stale report state is wiped
	assert_eq!(visible, content);
	assert!(session.last_self_report.is_none());
	assert!(session.last_self_report_reason.is_none());
	assert!(session.last_self_report_handoff.is_none());
	assert!(session.pending_plan_signal.is_none());
}

#[test]
fn test_capture_self_report_no_token_keeps_state_clear() {
	let mut config = template_config();
	config.supervisor.enabled = true;
	let mut session = crate::session::chat::session::ChatSession::for_tests(Vec::new());

	let visible = capture_self_report(&mut session, &config, "plain answer");

	assert_eq!(visible, "plain answer");
	assert!(session.last_self_report.is_none());
	assert!(session.last_self_report_reason.is_none());
	assert!(session.pending_plan_signal.is_none());
}

#[test]
fn test_capture_self_report_captures_plan_signal_and_state() {
	let mut config = template_config();
	config.supervisor.enabled = true;
	let mut session = crate::session::chat::session::ChatSession::for_tests(Vec::new());

	let content = "answer\n<sup>{\"state\":\"progressing\",\"focus\":\"mid turn\",\"next\":\"keep going\",\"carry\":[],\"plan\":\"request\",\"memories\":[],\"behaviors\":[]}</sup>";
	let visible = capture_self_report(&mut session, &config, content);

	assert_eq!(visible, "answer");
	assert_eq!(
		session.pending_plan_signal,
		Some(crate::supervisor::plan::PlanSignal::Request)
	);
	assert_eq!(
		session.last_self_report,
		Some(crate::supervisor::detect::SelfReport::Progressing)
	);
	assert_eq!(session.last_self_report_reason.as_deref(), Some("mid turn"));
	assert!(session.last_self_report_handoff.is_some());
}

#[test]
fn test_capture_self_report_blocked_state() {
	let mut config = template_config();
	config.supervisor.enabled = true;
	let mut session = crate::session::chat::session::ChatSession::for_tests(Vec::new());

	let content = "stuck\n<sup>{\"state\":\"blocked\",\"focus\":\"waiting on perms\",\"next\":null,\"carry\":[],\"plan\":null,\"memories\":[],\"behaviors\":[]}</sup>";
	let visible = capture_self_report(&mut session, &config, content);

	assert_eq!(visible, "stuck");
	assert_eq!(
		session.last_self_report,
		Some(crate::supervisor::detect::SelfReport::Blocked)
	);
	assert_eq!(
		session.last_self_report_reason.as_deref(),
		Some("waiting on perms")
	);
}

#[test]
fn test_params_builders_and_emit() {
	let mut session = crate::session::chat::session::ChatSession::for_tests(Vec::new());
	let config = template_config();
	let (_tx, rx) = tokio::sync::watch::channel(false);
	let sink = RecordingSink(std::sync::Arc::new(std::sync::Mutex::new(Vec::new())));

	let params = ResponseProcessingParams {
		content: "c".to_string(),
		exchange: ProviderExchange::new(json!({}), json!({}), None, "test"),
		tool_calls: None,
		thinking: None,
		finish_reason: None,
		response_id: None,
		chat_session: &mut session,
		config: &config,
		role: "assistant",
		operation_cancelled: rx,
		sink: sink.clone(),
		mode: OutputMode::Interactive,
	};

	let params = params
		.with_thinking(Some(ThinkingBlock::new("t")))
		.with_mode(OutputMode::Jsonl);
	assert!(params.thinking.is_some());
	assert_eq!(params.mode, OutputMode::Jsonl);

	params.emit(ServerMessage::error("boom".to_string()));
	emit_thinking_event(&params, &ThinkingBlock::new("think"), "sess-1");

	let messages = sink.0.lock().expect("sink lock");
	assert_eq!(messages.len(), 2);
	assert!(matches!(&messages[0], ServerMessage::Error(e) if e.message == "boom"));
	match &messages[1] {
		ServerMessage::Thinking(t) => {
			assert_eq!(t.content, "think");
			assert_eq!(t.session_id, "sess-1");
		}
		other => panic!("expected Thinking event, got {other:?}"),
	}
}

#[tokio::test]
async fn test_get_tool_server_name_async_unknown_tool() {
	let config = template_config();
	assert_eq!(
		get_tool_server_name_async("zzz_no_such_tool", &config).await,
		"unknown"
	);
}

/// The tool loop gate requires at least one configured MCP server; the
/// template has none, so tests that must enter the loop push a builtin one.
fn config_with_core_server(mut config: Config) -> Config {
	config
		.mcp
		.servers
		.push(crate::config::McpServerConfig::Builtin {
			name: "core".to_string(),
			timeout_seconds: 300,
			tools: vec![],
			auto_bind: None,
		});
	config
}

fn recording_sink() -> RecordingSink {
	RecordingSink(std::sync::Arc::new(std::sync::Mutex::new(Vec::new())))
}

#[cfg(unix)]
fn make_executable(path: &std::path::Path) {
	use std::os::unix::fs::PermissionsExt;
	std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
		.expect("chmod +x fixture");
}

/// Write an executable local-tool script under `<workdir>/.agents/tools/<name>`.
fn write_local_tool(workdir: &std::path::Path, name: &str, body: &str) {
	let dir = workdir.join(".agents/tools");
	std::fs::create_dir_all(&dir).expect("create tools dir");
	let path = dir.join(name);
	std::fs::write(&path, body).expect("write tool script");
	make_executable(&path);
}

#[tokio::test]
async fn test_process_response_final_answer_emits_thinking_assistant_and_cost() {
	let mut session = crate::session::chat::session::ChatSession::for_tests(Vec::new());
	session.add_user_message("hello").expect("add user message");
	let config = template_config();
	let (_tx, rx) = tokio::sync::watch::channel(false);
	let sink = recording_sink();

	let params = ResponseProcessingParams {
		content: "final answer".to_string(),
		exchange: ProviderExchange::new(json!({}), json!({}), None, "test"),
		tool_calls: None,
		thinking: Some(ThinkingBlock::new("pondering")),
		finish_reason: Some("stop".to_string()),
		response_id: Some("resp_1".to_string()),
		chat_session: &mut session,
		config: &config,
		role: "assistant",
		operation_cancelled: rx,
		sink: sink.clone(),
		mode: OutputMode::Jsonl,
	};

	process_response(params)
		.await
		.expect("final answer processing");

	let messages = sink.0.lock().expect("sink lock");
	assert_eq!(messages.len(), 3, "{messages:?}");
	assert!(matches!(&messages[0], ServerMessage::Thinking(t) if t.content == "pondering"));
	assert!(matches!(&messages[1], ServerMessage::Assistant(a) if a.content == "final answer"));
	assert!(matches!(messages[2], ServerMessage::Cost(_)));
	drop(messages);

	assert_eq!(session.last_response, "final answer");
	assert_eq!(session.turn_answers, vec!["final answer".to_string()]);
	let last = session.session.messages.last().expect("assistant recorded");
	assert_eq!(last.role, "assistant");
	assert_eq!(last.content, "final answer");
	assert_eq!(last.id.as_deref(), Some("resp_1"));
}

#[tokio::test]
async fn test_process_response_terminal_mode_warns_when_last_message_is_not_user() {
	// Empty session + terminal mode: the edge-case warning prints, then the
	// final answer is still recorded normally.
	let mut session = crate::session::chat::session::ChatSession::for_tests(Vec::new());
	let config = template_config();
	let (_tx, rx) = tokio::sync::watch::channel(false);
	let sink = recording_sink();

	let params = ResponseProcessingParams {
		content: "answer".to_string(),
		exchange: ProviderExchange::new(json!({}), json!({}), None, "test"),
		tool_calls: None,
		thinking: None,
		finish_reason: Some("stop".to_string()),
		response_id: None,
		chat_session: &mut session,
		config: &config,
		role: "assistant",
		operation_cancelled: rx,
		sink,
		mode: OutputMode::NonInteractive,
	};

	process_response(params)
		.await
		.expect("processing continues past the warning");

	assert_eq!(session.session.messages.len(), 1);
	assert_eq!(session.session.messages[0].role, "assistant");
	assert_eq!(session.session.messages[0].content, "answer");
}

#[tokio::test]
async fn test_process_response_cancelled_at_start_returns_cancelled_error() {
	let mut session = crate::session::chat::session::ChatSession::for_tests(Vec::new());
	let config = template_config();
	let (tx, rx) = tokio::sync::watch::channel(false);
	tx.send(true).expect("pre-fire cancellation");
	let sink = recording_sink();

	let params = ResponseProcessingParams {
		content: "answer".to_string(),
		exchange: ProviderExchange::new(json!({}), json!({}), None, "test"),
		tool_calls: None,
		thinking: None,
		finish_reason: None,
		response_id: None,
		chat_session: &mut session,
		config: &config,
		role: "assistant",
		operation_cancelled: rx,
		sink,
		mode: OutputMode::Jsonl,
	};

	let err = process_response(params)
		.await
		.expect_err("cancelled at entry must error");
	assert!(crate::session::cancellation::is_cancelled(&err));
	assert!(session.session.messages.is_empty());
}

#[tokio::test]
async fn test_process_response_tool_round_emits_tooluse_result_and_final_answer() {
	let _guard = ENV_LOCK.lock().await;
	let url = spawn_stub(vec![final_response("All done")]).await;
	std::env::set_var("OLLAMA_API_URL", &url);

	let config = config_with_core_server(fake_provider_config());
	let mut session = fake_session("run the thing");
	let (_tx, rx) = tokio::sync::watch::channel(false);
	let sink = recording_sink();

	let exchange = ProviderExchange::new(
		json!({}),
		json!({"tool_calls": [{"id": "call_1", "type": "function", "function": {"name": "zzz_missing_tool", "arguments": "{}"}}]}),
		None,
		"test",
	);
	let params = ResponseProcessingParams {
		content: String::new(),
		exchange,
		tool_calls: Some(vec![crate::mcp::McpToolCall {
			tool_name: "zzz_missing_tool".to_string(),
			parameters: json!({}),
			tool_id: "call_1".to_string(),
		}]),
		thinking: Some(ThinkingBlock::new("planning the call")),
		finish_reason: Some("tool_calls".to_string()),
		response_id: Some("resp_1".to_string()),
		chat_session: &mut session,
		config: &config,
		role: "assistant",
		operation_cancelled: rx,
		sink: sink.clone(),
		mode: OutputMode::Jsonl,
	};

	process_response(params)
		.await
		.expect("tool round processing");

	let messages = sink.0.lock().expect("sink lock");
	assert!(messages.iter().any(
		|m| matches!(m, ServerMessage::ToolUse(u) if u.tool == "zzz_missing_tool"
					&& u.tool_id == "call_1")
	));
	assert!(messages
		.iter()
		.any(|m| matches!(m, ServerMessage::ToolResult(r) if r.tool_id == "call_1" && !r.success)));
	assert!(messages
		.iter()
		.any(|m| matches!(m, ServerMessage::Assistant(a) if a.content == "All done")));
	assert!(matches!(messages.last(), Some(ServerMessage::Cost(_))));
	// Thinking is emitted once before execution; the final emit is suppressed
	// because the same block was already delivered.
	assert_eq!(
		messages
			.iter()
			.filter(|m| matches!(m, ServerMessage::Thinking(_)))
			.count(),
		1
	);
	drop(messages);

	let roles: Vec<&str> = session
		.session
		.messages
		.iter()
		.map(|m| m.role.as_str())
		.collect();
	assert_eq!(roles, vec!["user", "assistant", "tool", "assistant"]);
	assert_eq!(session.session.messages[3].content, "All done");
}

#[tokio::test]
async fn test_process_response_supervisor_loop_fires_steer_mid_turn() {
	let _guard = ENV_LOCK.lock().await;
	// Three identical tool rounds: round 1 comes from params, rounds 2-3 from
	// the stub; the loop detector fires on round 3 and the steer note is
	// injected before the final follow-up.
	let url = spawn_stub(vec![
		tool_call_response("loopdump", json!({})),
		tool_call_response("loopdump", json!({})),
		final_response("done"),
	])
	.await;
	std::env::set_var("OLLAMA_API_URL", &url);

	let tmp = tempfile::tempdir().expect("tempdir");
	write_local_tool(
		tmp.path(),
		"loopdump",
		"#!/bin/sh\n# @description Print a fixed line.\nprintf 'identical output\\n'\n",
	);

	let mut config = config_with_core_server(fake_provider_config());
	config.supervisor.enabled = true;
	// The planner would issue its own scripted-queue LLM calls; no plan signal
	// is emitted here, so disabling keeps the stub queue in sync.
	config.supervisor.plan.enabled = false;

	let session_id = "resp-steer-loop-test".to_string();
	crate::session::context::with_session_id(session_id.clone(), async {
		crate::session::context::init_session_services("assistant");
		crate::mcp::workdir::set_session_working_directory(tmp.path().to_path_buf());

		let mut session = fake_session("keep dumping");
		let (_tx, rx) = tokio::sync::watch::channel(false);
		let sink = recording_sink();

		let params = ResponseProcessingParams {
			content: String::new(),
			exchange: ProviderExchange::new(json!({}), json!({}), None, "test"),
			tool_calls: Some(vec![crate::mcp::McpToolCall {
				tool_name: "loopdump".to_string(),
				parameters: json!({}),
				tool_id: "c1".to_string(),
			}]),
			thinking: None,
			finish_reason: Some("tool_calls".to_string()),
			response_id: None,
			chat_session: &mut session,
			config: &config,
			role: "assistant",
			operation_cancelled: rx,
			sink,
			mode: OutputMode::Jsonl,
		};

		process_response(params)
			.await
			.expect("loop turn completes under the supervisor");

		let steered = session.session.messages.iter().any(|m| {
			m.role == "user"
				&& m.content
					.contains("identical to one already in your context")
		});
		assert!(
			steered,
			"steer note missing: {:?}",
			session
				.session
				.messages
				.iter()
				.map(|m| (&m.role, &m.content))
				.collect::<Vec<_>>()
		);
		assert!(session.steer_pending.is_none());
		assert_eq!(session.steer_attempt, 0);
		assert!(matches!(
			session.steer_last_signal,
			crate::supervisor::detect::DetectorSignal::Loop
		));
		assert_eq!(session.last_response, "done");

		crate::session::context::cleanup_session(&session_id);
	})
	.await;
}

#[tokio::test]
async fn test_process_response_cancelled_mid_execution_skips_assistant_message() {
	let tmp = tempfile::tempdir().expect("tempdir");
	// The tool touches a marker file first, giving the test a deterministic
	// sync point: cancellation fires only once execution has really started.
	write_local_tool(
		tmp.path(),
		"slowtool",
		"#!/bin/sh\n# @description Signal start then sleep.\ntouch \"$OCTOMIND_WORKDIR/slowtool-started\"\nsleep 5\necho done\n",
	);

	let config = config_with_core_server(fake_provider_config());
	let session_id = "resp-cancel-mid-test".to_string();
	crate::session::context::with_session_id(session_id.clone(), async {
		crate::session::context::init_session_services("assistant");
		crate::mcp::workdir::set_session_working_directory(tmp.path().to_path_buf());

		let mut session = fake_session("run the slow tool");
		let (tx, rx) = tokio::sync::watch::channel(false);
		let sink = recording_sink();

		let marker = tmp.path().join("slowtool-started");
		tokio::spawn(async move {
			let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
			while !marker.exists() && std::time::Instant::now() < deadline {
				tokio::time::sleep(std::time::Duration::from_millis(10)).await;
			}
			let _ = tx.send(true);
		});

		let params = ResponseProcessingParams {
			content: String::new(),
			exchange: ProviderExchange::new(json!({}), json!({}), None, "test"),
			tool_calls: Some(vec![crate::mcp::McpToolCall {
				tool_name: "slowtool".to_string(),
				parameters: json!({}),
				tool_id: "c1".to_string(),
			}]),
			thinking: None,
			finish_reason: Some("tool_calls".to_string()),
			response_id: None,
			chat_session: &mut session,
			config: &config,
			role: "assistant",
			operation_cancelled: rx,
			sink,
			mode: OutputMode::NonInteractive,
		};

		process_response(params)
			.await
			.expect("cancelled mid-execution returns Ok");

		// Tools never completed: no assistant message was added
		assert_eq!(session.session.messages.len(), 1);
		assert_eq!(session.session.messages[0].role, "user");

		crate::session::context::cleanup_session(&session_id);
	})
	.await;
}

#[tokio::test]
async fn test_process_response_request_spending_stop_ends_turn_without_final_answer() {
	let mut config = config_with_core_server(fake_provider_config());
	config.max_request_spending_threshold = 0.0001;

	let mut session = fake_session("spend it");
	session.session.info.total_cost = 1.0;
	let (_tx, rx) = tokio::sync::watch::channel(false);
	let sink = recording_sink();

	let params = ResponseProcessingParams {
		content: String::new(),
		exchange: ProviderExchange::new(json!({}), json!({}), None, "test"),
		tool_calls: Some(vec![crate::mcp::McpToolCall {
			tool_name: "zzz_missing_tool".to_string(),
			parameters: json!({}),
			tool_id: "c1".to_string(),
		}]),
		thinking: None,
		finish_reason: Some("tool_calls".to_string()),
		response_id: None,
		chat_session: &mut session,
		config: &config,
		role: "assistant",
		operation_cancelled: rx,
		sink,
		mode: OutputMode::Jsonl,
	};

	process_response(params)
		.await
		.expect("spending stop is not an error");

	// The tool round ran, but the follow-up was refused: no final answer
	let roles: Vec<&str> = session
		.session
		.messages
		.iter()
		.map(|m| m.role.as_str())
		.collect();
	assert_eq!(roles, vec!["user", "assistant", "tool"]);
	assert!(session.turn_answers.is_empty());
}
