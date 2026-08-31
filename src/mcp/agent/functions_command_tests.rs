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

//! Validation-arm tests for the `agent_*` builtin tool dispatcher. Nothing
//! here spawns an agent — only the parameter/lookup failures that must come
//! back as structured tool errors, never as process work.

use super::*;

fn agent_call(tool_name: &str, params: serde_json::Value) -> McpToolCall {
	McpToolCall {
		tool_name: tool_name.to_string(),
		parameters: params,
		tool_id: "t-agent".to_string(),
	}
}

fn test_config() -> crate::config::Config {
	let mut config: crate::config::Config =
		toml::from_str(include_str!("../../../config-templates/default.toml"))
			.expect("parse default config template");
	config.build_role_map();
	config
}

fn text_of(result: &McpToolResult) -> String {
	result
		.result
		.content
		.iter()
		.filter_map(|block| match block {
			rmcp::model::ContentBlock::Text(t) => Some(t.text.clone()),
			_ => None,
		})
		.collect()
}

fn is_err(result: &McpToolResult) -> bool {
	result.result.is_error.unwrap_or(false)
}

#[tokio::test]
async fn test_agent_tool_validation_arms() {
	let config = test_config();

	// Tool name without the agent_ prefix
	let result = execute_agent_command(
		&agent_call("not_an_agent_tool", serde_json::json!({"task": "x"})),
		&config,
		None,
	)
	.await
	.expect("dispatch");
	assert!(is_err(&result));
	assert!(text_of(&result).contains("Invalid agent tool name"));

	// Missing / empty task
	for params in [serde_json::json!({}), serde_json::json!({"task": "   "})] {
		let result = execute_agent_command(&agent_call("agent_developer", params), &config, None)
			.await
			.expect("dispatch");
		assert!(is_err(&result));
		assert!(text_of(&result).contains("task"));
	}

	// Agent that exists neither in config nor as a dynamic agent
	let result = execute_agent_command(
		&agent_call(
			"agent___functest_nonexistent",
			serde_json::json!({"task": "do it"}),
		),
		&config,
		None,
	)
	.await
	.expect("dispatch");
	assert!(is_err(&result));
	assert!(text_of(&result).contains("not configured"));
}

// ---------------------------------------------------------------------------
// Job-manager plumbing and tool-surface generation
// ---------------------------------------------------------------------------

#[test]
fn get_max_concurrent_jobs_is_at_least_four() {
	assert!(get_max_concurrent_jobs() >= 4);
}

#[tokio::test]
async fn job_manager_is_session_scoped_and_kill_all_is_safe() {
	let sid = "__agenttest_jobs".to_string();
	crate::session::context::with_session_id(sid, async {
		init_job_manager();
		assert!(get_job_manager().is_some(), "session manager must register");
		kill_all_jobs(); // no jobs registered — must be a safe no-op
	})
	.await;
}

#[test]
fn get_all_functions_maps_config_agents_to_tools() {
	let mut config = test_config();
	config.agents = vec![
		crate::config::agents::AgentConfig {
			name: "researcher".to_string(),
			description: "Finds facts".to_string(),
			command: "octomind acp --role researcher".to_string(),
			workdir: ".".to_string(),
		},
		crate::config::agents::AgentConfig {
			name: "critic".to_string(),
			description: "Reviews work".to_string(),
			command: "octomind acp --role critic".to_string(),
			workdir: "/tmp".to_string(),
		},
	];
	let functions = get_all_functions(&config);
	let names: Vec<&str> = functions.iter().map(|f| f.name.as_str()).collect();
	assert_eq!(names, vec!["agent_researcher", "agent_critic"]);
	for (f, agent) in functions.iter().zip(config.agents.iter()) {
		assert!(f.description.contains(&agent.description));
		let required = f
			.parameters
			.get("required")
			.and_then(|r| r.as_array())
			.expect("required array");
		assert!(required.iter().any(|v| v.as_str() == Some("task")));
	}
}

// ---------------------------------------------------------------------------
// Config-agent execution (subprocess path)
// ---------------------------------------------------------------------------

/// Minimal ACP handshake: initialize (id=1), session/new (id=2), one streamed
/// message chunk. The prompt response (id=3) is appended per test.
#[cfg(unix)]
const AGENT_HANDSHAKE: &str = r#"echo '{"jsonrpc":"2.0","id":1,"result":{}}'
echo '{"jsonrpc":"2.0","id":2,"result":{"sessionId":"s"}}'
echo '{"jsonrpc":"2.0","method":"session/update","params":{"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"hello from agent"}}}}'
"#;

#[cfg(unix)]
fn write_agent_script(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
	use std::os::unix::fs::PermissionsExt;
	let path = dir.join("fake-agent.sh");
	std::fs::write(&path, format!("#!/bin/sh\n{body}")).expect("write fake agent script");
	std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
		.expect("make fake agent script executable");
	path
}

fn config_with_agent(command: &str) -> crate::config::Config {
	let mut config = test_config();
	config.agents = vec![crate::config::agents::AgentConfig {
		name: "scripted".to_string(),
		description: "Runs a canned command".to_string(),
		command: command.to_string(),
		workdir: ".".to_string(),
	}];
	config
}

#[cfg(unix)]
#[tokio::test]
async fn sync_config_agent_returns_subprocess_output() {
	let dir = tempfile::tempdir().expect("tempdir");
	let script = write_agent_script(
		dir.path(),
		&format!(
			"{AGENT_HANDSHAKE}echo '{{\"jsonrpc\":\"2.0\",\"id\":3,\"result\":{{\"stopReason\":\"end_turn\"}}}}'\ncat >/dev/null"
		),
	);
	let config = config_with_agent(&script.to_string_lossy());
	let result = execute_agent_command(
		&agent_call("agent_scripted", serde_json::json!({"task": "do it"})),
		&config,
		None,
	)
	.await
	.expect("dispatch");
	assert!(!is_err(&result), "content: {}", text_of(&result));
	assert_eq!(text_of(&result), "hello from agent");
}

#[cfg(unix)]
#[tokio::test]
async fn sync_config_agent_failure_is_a_tool_error() {
	let dir = tempfile::tempdir().expect("tempdir");
	// Exits before answering the initialize handshake.
	let script = write_agent_script(dir.path(), "exit 0");
	let config = config_with_agent(&script.to_string_lossy());
	let result = execute_agent_command(
		&agent_call("agent_scripted", serde_json::json!({"task": "do it"})),
		&config,
		None,
	)
	.await
	.expect("dispatch");
	assert!(is_err(&result));
	assert!(
		text_of(&result).contains("Agent execution failed"),
		"content: {}",
		text_of(&result)
	);
}

#[tokio::test]
async fn async_config_agent_without_job_manager_is_error() {
	// Outside any session (and with the CLI-global manager unset) async
	// execution must refuse instead of silently running synchronously.
	let config = config_with_agent("octomind-test-no-such-binary");
	let result = execute_agent_command(
		&agent_call(
			"agent_scripted",
			serde_json::json!({"task": "x", "async": true}),
		),
		&config,
		None,
	)
	.await
	.expect("dispatch");
	assert!(is_err(&result));
	assert!(
		text_of(&result).contains("Async job manager not initialised"),
		"content: {}",
		text_of(&result)
	);
}

#[cfg(unix)]
#[tokio::test]
#[serial_test::serial]
async fn async_config_agent_runs_to_completion_in_background() {
	let dir = tempfile::tempdir().expect("tempdir");
	let script = write_agent_script(
		dir.path(),
		&format!(
			"{AGENT_HANDSHAKE}echo '{{\"jsonrpc\":\"2.0\",\"id\":3,\"result\":{{\"stopReason\":\"end_turn\"}}}}'\ncat >/dev/null"
		),
	);
	let config = config_with_agent(&script.to_string_lossy());

	let sid = "__agenttest_async".to_string();
	crate::session::context::with_session_id(sid, async {
		init_job_manager();
		let result = execute_agent_command(
			&agent_call(
				"agent_scripted",
				serde_json::json!({"task": "x", "async": true}),
			),
			&config,
			None,
		)
		.await
		.expect("dispatch");
		assert!(!is_err(&result), "content: {}", text_of(&result));
		assert!(text_of(&result).contains("started asynchronously"));

		let manager = get_job_manager().expect("session job manager");
		let completed = manager.wait_all().await;
		assert_eq!(
			completed, 1,
			"the background job must complete and be reaped"
		);
		assert_eq!(manager.active_count(), 0, "the slot must be released");
	})
	.await;
}

#[tokio::test]
#[serial_test::serial]
async fn async_config_agent_rejects_when_job_limit_is_reached() {
	let sid = "__agenttest_limit".to_string();
	crate::session::context::with_session_id(sid, async {
		init_job_manager();
		let manager = get_job_manager().expect("session job manager");
		// Exhaust every concurrency slot, then verify the structured error.
		while manager.try_acquire().is_ok() {}
		let config = config_with_agent("octomind-test-no-such-binary");
		let result = execute_agent_command(
			&agent_call(
				"agent_scripted",
				serde_json::json!({"task": "x", "async": true}),
			),
			&config,
			None,
		)
		.await
		.expect("dispatch");
		assert!(is_err(&result));
		assert!(
			text_of(&result).contains("Async job limit reached"),
			"content: {}",
			text_of(&result)
		);
	})
	.await;
}

// ---------------------------------------------------------------------------
// Dynamic-agent dispatch arms that need no API call
// ---------------------------------------------------------------------------

fn dyn_agent(name: &str) -> crate::mcp::runtime::dynamic_agents::DynamicAgentConfig {
	crate::mcp::runtime::dynamic_agents::DynamicAgentConfig {
		name: name.to_string(),
		description: "test agent".to_string(),
		system: "you are a test".to_string(),
		welcome: String::new(),
		model: Default::default(),
		server_refs: Vec::new(),
		allowed_tools: Vec::new(),
		workdir: ".".to_string(),
	}
}

#[tokio::test]
#[serial_test::serial]
async fn dynamic_agent_conflicting_with_config_agent_is_ambiguous() {
	let config = config_with_agent("octomind acp --role scripted");
	let sid = "__agenttest_clash".to_string();
	crate::session::context::with_session_id(sid, async {
		crate::mcp::runtime::dynamic_agents::register_agent(dyn_agent("scripted"))
			.expect("register dynamic agent");
		crate::mcp::runtime::dynamic_agents::enable_agent("scripted").expect("enable");
		let result = execute_agent_command(
			&agent_call("agent_scripted", serde_json::json!({"task": "x"})),
			&config,
			None,
		)
		.await
		.expect("dispatch");
		assert!(is_err(&result));
		assert!(
			text_of(&result).contains("exists in both config and dynamic agents"),
			"content: {}",
			text_of(&result)
		);
		crate::mcp::runtime::dynamic_agents::remove_agent("scripted");
	})
	.await;
}

#[tokio::test]
#[serial_test::serial]
async fn async_dynamic_agent_rejects_when_job_limit_is_reached() {
	let sid = "__agenttest_dynlimit".to_string();
	crate::session::context::with_session_id(sid, async {
		init_job_manager();
		crate::mcp::runtime::dynamic_agents::register_agent(dyn_agent("dynlimited"))
			.expect("register dynamic agent");
		crate::mcp::runtime::dynamic_agents::enable_agent("dynlimited").expect("enable");
		let manager = get_job_manager().expect("session job manager");
		while manager.try_acquire().is_ok() {}
		let config = test_config();
		let result = execute_agent_command(
			&agent_call(
				"agent_dynlimited",
				serde_json::json!({"task": "x", "async": true}),
			),
			&config,
			None,
		)
		.await
		.expect("dispatch");
		assert!(is_err(&result));
		assert!(
			text_of(&result).contains("Async job limit reached"),
			"content: {}",
			text_of(&result)
		);
		crate::mcp::runtime::dynamic_agents::remove_agent("dynlimited");
	})
	.await;
}

// ---------------------------------------------------------------------------
// build_agent_config — merged-config resolution (pure)
// ---------------------------------------------------------------------------

#[test]
fn build_agent_config_without_server_refs_disables_mcp() {
	let base = test_config();
	let agent = dyn_agent("nomcp");
	let merged = build_agent_config(&agent, &base);
	assert!(merged.mcp.servers.is_empty(), "servers must be cleared");
	assert!(
		merged.mcp.allowed_tools.is_empty(),
		"tools filter must be cleared"
	);
	assert_eq!(merged.model, base.model, "model must be untouched");
}

#[test]
fn build_agent_config_resolves_refs_filters_tools_and_overrides_model() {
	let mut base = test_config();
	base.mcp.servers = vec![
		crate::config::McpServerConfig::stdin("alpha", "echo", Vec::new(), 5, Vec::new()),
		crate::config::McpServerConfig::stdin("beta", "echo", Vec::new(), 5, Vec::new()),
	];
	let mut agent = dyn_agent("withrefs");
	agent.server_refs = vec!["alpha".to_string()];
	agent.allowed_tools = vec!["alpha:tool_a".to_string()];
	agent.model.model = Some("openai:gpt-4o".to_string());
	let merged = build_agent_config(&agent, &base);
	let names: Vec<&str> = merged.mcp.servers.iter().map(|s| s.name()).collect();
	assert_eq!(
		names,
		vec!["alpha"],
		"only the referenced server is enabled"
	);
	assert_eq!(merged.mcp.allowed_tools, vec!["alpha:tool_a".to_string()]);
	assert_eq!(merged.model, "openai:gpt-4o");
}

// ---------------------------------------------------------------------------
// Parent-notification forwarding (observed through the CLI sender)
// ---------------------------------------------------------------------------

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
			id: "tap-agenttest-live".to_string(),
			role: "test:live".to_string(),
			workdir: ".".to_string(),
			started_at: SystemTime::now(),
			status: Arc::new(RwLock::new(TapJobStatus::Running)),
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
