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

#[test]
#[serial_test::serial]
fn forward_session_update_maps_every_rendered_kind() {
	let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
	crate::mcp::process::set_notification_sender(None, tx);

	forward_session_update_to_parent(&serde_json::json!({
		"sessionUpdate": "agent_message_chunk",
		"content": {"type": "text", "text": "hi"}
	}));
	forward_session_update_to_parent(&serde_json::json!({
		"sessionUpdate": "agent_thought_chunk",
		"content": {"type": "text", "text": "thinking"}
	}));
	forward_session_update_to_parent(&serde_json::json!({
		"sessionUpdate": "tool_call",
		"toolCallId": "t1",
		"title": "shell",
		"rawInput": {"command": "ls"}
	}));
	forward_session_update_to_parent(&serde_json::json!({
		"sessionUpdate": "tool_call_update",
		"toolCallId": "t1",
		"status": "completed",
		"rawOutput": "ok"
	}));
	forward_session_update_to_parent(&serde_json::json!({
		"sessionUpdate": "tool_call_update",
		"toolCallId": "t2",
		"status": "failed",
		"rawOutput": {"error": "boom"}
	}));

	use crate::websocket::ServerMessage;
	match rx.try_recv().expect("assistant message") {
		ServerMessage::Assistant(p) => assert_eq!(p.content, "hi"),
		_ => panic!("expected Assistant"),
	}
	match rx.try_recv().expect("thinking message") {
		ServerMessage::Thinking(p) => assert_eq!(p.content, "thinking"),
		_ => panic!("expected Thinking"),
	}
	match rx.try_recv().expect("tool use message") {
		ServerMessage::ToolUse(p) => {
			assert_eq!(p.tool, "shell");
			assert_eq!(p.tool_id, "t1");
		}
		_ => panic!("expected ToolUse"),
	}
	match rx.try_recv().expect("tool result message") {
		ServerMessage::ToolResult(p) => {
			assert_eq!(p.tool_id, "t1");
			assert_eq!(p.content, "ok");
			assert!(p.success);
		}
		_ => panic!("expected ToolResult"),
	}
	match rx.try_recv().expect("failed tool result message") {
		ServerMessage::ToolResult(p) => {
			assert_eq!(p.tool_id, "t2");
			assert!(!p.success);
			assert_eq!(p.content, "{\"error\":\"boom\"}");
		}
		_ => panic!("expected ToolResult"),
	}
	assert!(rx.try_recv().is_err(), "no further messages expected");

	crate::mcp::process::clear_notification_sender(None);
}

#[test]
fn forward_session_update_ignores_unrendered_updates() {
	// Without a registered sender these are dropped; the contract under test
	// is that every ignored shape returns without panicking.
	for update in [
		serde_json::json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": ""}}),
		serde_json::json!({"sessionUpdate": "agent_thought_chunk", "content": {"type": "text", "text": ""}}),
		serde_json::json!({"sessionUpdate": "tool_call_update", "status": "pending"}),
		serde_json::json!({"sessionUpdate": "something_else"}),
		serde_json::json!({"no_kind": true}),
	] {
		forward_session_update_to_parent(&update);
	}
}

// ---------------------------------------------------------------------------
// Tap-run live mirroring and action formatting helpers
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial_test::serial]
async fn record_tap_live_updates_action_and_usage() {
	use crate::session::tap_runs::{self, TapJob, TapJobStatus, TapLiveState};
	use std::sync::{Arc, RwLock};
	use std::time::SystemTime;

	let sid = "__agenttest_taplive".to_string();
	crate::session::context::with_session_id(sid, async {
		let (cancel_tx, _keep_alive) = watch::channel(false);
		tap_runs::register_job(TapJob {
			mode: Default::default(),
			id: "tap-agenttest-live".to_string(),
			role: "test:live".to_string(),
			workdir: ".".to_string(),
			started_at: SystemTime::now(),
			status: Arc::new(RwLock::new(TapJobStatus::Running)),
			error: None,
			cancel_tx,
			live: Arc::new(RwLock::new(TapLiveState::default())),
		});

		// tool_call with a recognizable rawInput argument appends a hint
		record_tap_live(
			"tap-agenttest-live",
			&serde_json::json!({"params": {"update": {"sessionUpdate": "tool_call", "title": "shell", "rawInput": {"command": "cargo test"}}}}),
		);
		let job = tap_runs::find_job("tap-agenttest-live").expect("job registered");
		assert_eq!(job.live.last_action.as_deref(), Some("shell cargo test"));

		// tool_call without a hint falls back to the bare title
		record_tap_live(
			"tap-agenttest-live",
			&serde_json::json!({"params": {"update": {"sessionUpdate": "tool_call", "title": "plan"}}}),
		);
		assert_eq!(
			tap_runs::find_job("tap-agenttest-live")
				.unwrap()
				.live
				.last_action
				.as_deref(),
			Some("plan")
		);

		// an empty title changes nothing
		record_tap_live(
			"tap-agenttest-live",
			&serde_json::json!({"params": {"update": {"sessionUpdate": "tool_call", "title": ""}}}),
		);
		assert_eq!(
			tap_runs::find_job("tap-agenttest-live")
				.unwrap()
				.live
				.last_action
				.as_deref(),
			Some("plan")
		);

		// long agent text is collapsed to one capped line
		record_tap_live(
			"tap-agenttest-live",
			&serde_json::json!({"params": {"update": {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "a\nb"}}}}),
		);
		let long = "x".repeat(80);
		record_tap_live(
			"tap-agenttest-live",
			&serde_json::json!({"params": {"update": {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": long}}}}),
		);
		let action = tap_runs::find_job("tap-agenttest-live")
			.unwrap()
			.live
			.last_action
			.expect("action recorded");
		assert_eq!(action.chars().count(), 60, "capped at 60 chars: {action}");
		assert!(action.ends_with('…'));

		// usage meta is banked and short-circuits update handling
		record_tap_live(
			"tap-agenttest-live",
			&serde_json::json!({"params": {"_meta": {"octomind.usage": {"input_tokens": 5, "output_tokens": 6, "cache_read_tokens": 1, "session_cost": 0.25}}}}),
		);
		let usage = tap_runs::find_job("tap-agenttest-live")
			.unwrap()
			.live
			.usage
			.expect("usage recorded");
		assert_eq!(usage.input_tokens, 5);
		assert_eq!(usage.output_tokens, 6);
		assert_eq!(usage.cache_read_tokens, 1);
		assert!((usage.cost - 0.25).abs() < 1e-9);
	})
	.await;
}

#[test]
fn tool_arg_hint_prefers_first_descriptive_key() {
	let args = serde_json::json!({"query": "q", "file_path": "/a/b.rs", "command": "ls"});
	assert_eq!(tool_arg_hint(&args).as_deref(), Some("/a/b.rs"));
	assert_eq!(
		tool_arg_hint(&serde_json::json!({"url": "https://x"})).as_deref(),
		Some("https://x")
	);
	assert_eq!(tool_arg_hint(&serde_json::json!({"name": "  "})), None);
	assert_eq!(tool_arg_hint(&serde_json::json!({"count": 3})), None);
	assert_eq!(tool_arg_hint(&serde_json::json!({})), None);
}

#[test]
fn truncate_action_collapses_newlines_and_caps_length() {
	assert_eq!(truncate_action("a\nb\r\nc", 10), "a b  c");
	assert_eq!(truncate_action("hello", 5), "hello");
	let capped = truncate_action(&"x".repeat(10), 5);
	assert_eq!(capped.chars().count(), 5);
	assert!(capped.ends_with('…'));
}
