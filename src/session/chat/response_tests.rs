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
