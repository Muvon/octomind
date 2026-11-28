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
use crate::session::chat::test_support::{fake_provider_config, fake_session, final_response, spawn_stub, ENV_LOCK};
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
	config.plugins.push(plugin_config());
	let content = r#"answer
<sup>{"state":"progressing","focus":"used one memory","next":"continue","carry":[],"plan":null,"memories":["M2","M9"]}</sup>"#;
	let visible = capture_self_report(&mut session, &config, content);
	assert_eq!(visible, "answer");
	assert_eq!(session.used_memory_ids.len(), 1);
	assert!(session.used_memory_ids.contains("M2"));
}

fn template_config() -> Config {
	toml::from_str(include_str!("../../../config-templates/default.toml")).expect("parse default config template")
}

/// The status line is read only when a plugin is configured.
fn plugin_config() -> crate::config::PluginConfig {
	toml::from_str("name = \"status-reader\"\ntype = \"stdio\"\ncommand = \"stub\"\n").expect("plugin config parses")
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
	let output_items = json!([
		{"type": "reasoning", "id": "rs_1", "encrypted_content": "opaque", "summary": []},
		{"type": "message", "id": "msg_1", "role": "assistant", "phase": "final_answer", "content": [{"type": "output_text", "text": "final answer"}]}
	]);

	handle_final_response(
		"final answer",
		&None,
		Some(output_items.clone()),
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
	assert_eq!(message.provider_items.as_ref(), Some(&output_items));
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
		json!({
			"tool_calls": [{"id": "c1", "type": "function", "function": {"name": "shell", "arguments": "{}"}}],
			"output_items": [
				{"type": "message", "role": "assistant", "phase": "commentary", "content": [{"type": "output_text", "text": "running tools\n<sup>progressing</sup>"}]},
				{"type": "function_call", "call_id": "c1"}
			]
		}),
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
	assert_eq!(
		message.provider_items.as_ref().expect("provider items")[0]["phase"],
		"commentary"
	);
	assert!(
		message.provider_items.as_ref().expect("provider items")[0]["content"][0]["text"]
			.as_str()
			.expect("output text")
			.contains("<sup>"),
		"display-only self-report stripping must not rewrite provider replay state"
	);
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

	add_assistant_message_with_tool_calls(&mut session, "the answer", &exchange, None, &None, &config, "assistant")
		.expect("assistant message");

	let message = &session.session.messages[0];
	assert!(message.tool_calls.is_none());
	assert_eq!(session.turn_answers, vec!["the answer".to_string()]);
}

#[test]
fn test_capture_self_report_disabled_hides_report_and_clears_state() {
	let config = template_config();
	let mut session = crate::session::chat::session::ChatSession::for_tests(Vec::new());
	session.last_self_report = Some(crate::session::self_report::SelfReport::Progressing);
	session.last_self_report_reason = Some("stale".to_string());

	let content = "answer\n<sup>{\"state\":\"done\",\"focus\":\"f\",\"next\":null,\"carry\":[],\"plan\":null,\"memories\":[],\"behaviors\":[]}</sup>";
	let visible = capture_self_report(&mut session, &config, content);

	assert_eq!(visible, "answer");
	assert!(session.last_self_report.is_none());
	assert!(session.last_self_report_reason.is_none());
	assert!(session.last_self_report_handoff.is_none());
}

#[test]
fn test_capture_self_report_no_token_keeps_state_clear() {
	let mut config = template_config();
	config.plugins.push(plugin_config());
	let mut session = crate::session::chat::session::ChatSession::for_tests(Vec::new());

	let visible = capture_self_report(&mut session, &config, "plain answer");

	assert_eq!(visible, "plain answer");
	assert!(session.last_self_report.is_none());
	assert!(session.last_self_report_reason.is_none());
}

#[test]
fn test_capture_self_report_captures_state_and_ignores_plan_signal() {
	let mut config = template_config();
	config.plugins.push(plugin_config());
	let mut session = crate::session::chat::session::ChatSession::for_tests(Vec::new());

	let content = "answer\n<sup>{\"state\":\"progressing\",\"focus\":\"mid turn\",\"next\":\"keep going\",\"carry\":[],\"plan\":\"request\",\"memories\":[],\"behaviors\":[]}</sup>";
	let visible = capture_self_report(&mut session, &config, content);

	assert_eq!(visible, "answer");
	assert_eq!(
		session.last_self_report,
		Some(crate::session::self_report::SelfReport::Progressing)
	);
	assert_eq!(session.last_self_report_reason.as_deref(), Some("mid turn"));
	assert!(session.last_self_report_handoff.is_some());
}

#[test]
fn test_capture_self_report_blocked_state() {
	let mut config = template_config();
	config.plugins.push(plugin_config());
	let mut session = crate::session::chat::session::ChatSession::for_tests(Vec::new());

	let content = "stuck\n<sup>{\"state\":\"blocked\",\"focus\":\"waiting on perms\",\"next\":null,\"carry\":[],\"plan\":null,\"memories\":[],\"behaviors\":[]}</sup>";
	let visible = capture_self_report(&mut session, &config, content);

	assert_eq!(visible, "stuck");
	assert_eq!(
		session.last_self_report,
		Some(crate::session::self_report::SelfReport::Blocked)
	);
	assert_eq!(session.last_self_report_reason.as_deref(), Some("waiting on perms"));
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
	assert_eq!(get_tool_server_name_async("zzz_no_such_tool", &config).await, "unknown");
}

/// The tool loop gate requires at least one configured MCP server; the
/// template has none, so tests that must enter the loop push a builtin one.
fn config_with_core_server(mut config: Config) -> Config {
	config.mcp.servers.push(crate::config::McpServerConfig::Builtin {
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
	std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod +x fixture");
}

/// Write an executable local-tool script under `<workdir>/.agents/tools/<name>`.
#[cfg(unix)]
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

	process_response(params).await.expect("final answer processing");

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
async fn test_process_response_hides_internal_notes_in_all_modes_but_preserves_session_log() {
	let content = "Answer.\n<system>private context</system>\n<sub>{\"state\":\"progressing\",\"focus\":\"checking result\",\"next\":\"report result\",\"carry\":[],\"plan\":null,\"memories\":[]}</sup>";
	let provider_items = json!([
		{"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": content}]}
	]);
	for plugin_configured in [false, true] {
		for mode in [
			OutputMode::Interactive,
			OutputMode::NonInteractive,
			OutputMode::Jsonl,
			OutputMode::WebSocket,
		] {
			let mut session = fake_session("hello");
			let dir = tempfile::tempdir().expect("session directory");
			let path = dir.path().join("test.jsonl.zst");
			session.session.session_file = Some(path.clone());
			session.session.save().expect("initialize session log");
			let mut config = fake_provider_config();
			if plugin_configured {
				config.plugins.push(plugin_config());
			}
			let (_tx, rx) = tokio::sync::watch::channel(false);
			let sink = recording_sink();
			process_response(ResponseProcessingParams {
				content: content.to_string(),
				exchange: ProviderExchange::new(json!({}), json!({"output_items": provider_items}), None, "test"),
				tool_calls: None,
				thinking: None,
				finish_reason: Some("stop".into()),
				response_id: Some("resp_internal".into()),
				chat_session: &mut session,
				config: &config,
				role: "assistant",
				operation_cancelled: rx,
				sink: sink.clone(),
				mode,
			})
			.await
			.expect("response processing");

			let messages = sink.0.lock().expect("sink lock");
			let answers: Vec<_> = messages
				.iter()
				.filter_map(|message| match message {
					ServerMessage::Assistant(answer) => Some(answer.content.as_str()),
					_ => None,
				})
				.collect();
			assert_eq!(answers, vec!["Answer."], "{mode:?}, plugin={plugin_configured}");
			assert!(matches!(messages.last(), Some(ServerMessage::Cost(_))));
			assert_eq!(session.last_response, "Answer.");
			assert_eq!(session.turn_answers, vec!["Answer."]);
			assert_eq!(
				session.last_self_report,
				plugin_configured.then_some(crate::session::self_report::SelfReport::Progressing)
			);
			let stored = crate::session::load_session(&path).expect("reload session log");
			let answer = stored.messages.last().expect("stored response");
			assert_eq!(answer.content, content);
			assert_eq!(answer.provider_items.as_ref(), Some(&provider_items));
			assert_eq!(answer.id.as_deref(), Some("resp_internal"));
			assert_eq!(session.session.messages.last().expect("response").content, content);
		}
	}
}

#[tokio::test]
async fn test_process_response_internal_only_reply_preserves_log_and_emits_empty_answer() {
	let content = "<system>private context</system>\n<sup>{\"state\":\"done\"}</sup>";
	let mut session = fake_session("hello");
	let config = fake_provider_config();
	let (_tx, rx) = tokio::sync::watch::channel(false);
	let sink = recording_sink();
	process_response(ResponseProcessingParams {
		content: content.to_string(),
		exchange: ProviderExchange::new(json!({}), json!({}), None, "test"),
		tool_calls: None,
		thinking: None,
		finish_reason: Some("stop".into()),
		response_id: None,
		chat_session: &mut session,
		config: &config,
		role: "assistant",
		operation_cancelled: rx,
		sink: sink.clone(),
		mode: OutputMode::WebSocket,
	})
	.await
	.expect("response processing");
	let messages = sink.0.lock().expect("sink lock");
	assert!(matches!(&messages[0], ServerMessage::Assistant(answer) if answer.content.is_empty()));
	assert!(matches!(messages.last(), Some(ServerMessage::Cost(_))));
	assert!(session.turn_answers.is_empty());
	assert!(session.last_response.is_empty());
	assert_eq!(
		session.session.messages.last().expect("stored response").content,
		content
	);
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
	let initial_content = "Working\n<system>private context</system>\n<sup>progressing</sup>";
	let final_content = "All done\n<system>private context</system>\n<sub>{\"state\":\"done\",\"focus\":\"finished\",\"next\":null,\"carry\":[]}</sup>";
	let url = spawn_stub(vec![final_response(final_content)]).await;
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
		content: initial_content.to_string(),
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

	process_response(params).await.expect("tool round processing");

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

	let roles: Vec<&str> = session.session.messages.iter().map(|m| m.role.as_str()).collect();
	assert_eq!(roles, vec!["user", "assistant", "tool", "assistant"]);
	assert_eq!(session.session.messages[1].content, initial_content);
	assert_eq!(session.session.messages[3].content, final_content);
	assert_eq!(session.last_response, "All done");
	assert_eq!(session.turn_answers, vec!["All done"]);
}

#[tokio::test]
async fn test_process_response_tool_round_without_keepalive_policy_records_no_tool_wait() {
	let _guard = ENV_LOCK.lock().await;
	let url = spawn_stub(vec![final_response("All done")]).await;
	std::env::set_var("OLLAMA_API_URL", &url);

	// Ollama has no keepalive policy: the gate must yield no handle even
	// though keepalive is enabled and the tool round runs.
	let mut config = config_with_core_server(fake_provider_config());
	config.cache_keepalive_enabled = true;
	let mut session = fake_session("run the thing");
	session.session.messages[0].cached = true;
	session.last_sent_messages = session.session.messages.clone();
	let (_tx, rx) = tokio::sync::watch::channel(false);

	let exchange = ProviderExchange::new(
		json!({}),
		json!({"tool_calls": [{"id": "call_1", "type": "function", "function": {"name": "zzz_missing_tool", "arguments": "{}"}}]}),
		None,
		"test",
	);
	let params = ResponseProcessingParams {
		content: "Working".to_string(),
		exchange,
		tool_calls: Some(vec![crate::mcp::McpToolCall {
			tool_name: "zzz_missing_tool".to_string(),
			parameters: json!({}),
			tool_id: "call_1".to_string(),
		}]),
		thinking: None,
		finish_reason: Some("tool_calls".to_string()),
		response_id: Some("resp_1".to_string()),
		chat_session: &mut session,
		config: &config,
		role: "assistant",
		operation_cancelled: rx,
		sink: recording_sink(),
		mode: OutputMode::Jsonl,
	};

	process_response(params).await.expect("tool round processing");

	let roles: Vec<&str> = session.session.messages.iter().map(|m| m.role.as_str()).collect();
	assert_eq!(roles, vec!["user", "assistant", "tool", "assistant"]);
	assert_eq!(session.last_response, "All done");
	assert!(session.wait_history.samples(WaitKind::Tools).is_empty());
}

#[tokio::test]
#[cfg(unix)]
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
		crate::session::context::init_session_services("assistant", Default::default());
		crate::mcp::workdir::set_session_working_directory(tmp.path().to_path_buf());

		let mut session = fake_session("run the slow tool");
		session.session.info.name = session_id.clone();
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

	process_response(params).await.expect("spending stop is not an error");

	// The tool round ran, but the follow-up was refused: no final answer
	let roles: Vec<&str> = session.session.messages.iter().map(|m| m.role.as_str()).collect();
	assert_eq!(roles, vec!["user", "assistant", "tool"]);
	assert!(session.turn_answers.is_empty());
}

#[tokio::test]
async fn cost_frame_reports_auxiliary_models_at_their_own_cost() {
	crate::session::accounting::isolated(async {
		let mut session = fake_session("hi");
		session.session.info.compression_stats.input_tokens = 1_000;
		session.session.info.compression_stats.cost = 0.002;
		session.session.info.supervisor_accounting.kinds.insert(
			crate::session::accounting::CallKind::Gate,
			crate::session::accounting::Usage {
				calls: 1,
				input_tokens: 50,
				cost: 0.003,
				blocking_wall_time_ms: 400,
				..Default::default()
			},
		);
		let frame = session_cost_payload(&mut session, "s".to_string());
		assert_eq!(frame.aux_cost, 0.005);
		assert_eq!(frame.aux_input_tokens, 1_050);
		assert_eq!(frame.supervisor_accounting.mechanics["gate"]["wall_blocking_ms"], 400);
	})
	.await;
}
