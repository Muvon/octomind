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

//! Behavioral tests for `src/mcp/agent/functions.rs` — dynamic-agent config
//! building, in-process execution failure paths, async job release into the
//! session inbox, ACP subprocess edge branches, and the low-level
//! `wait_for_response` / `record_tap_live` branches.

use super::*;
use serial_test::serial;

fn runtime_test_config() -> crate::config::Config {
	let mut config: crate::config::Config =
		toml::from_str(include_str!("../../../config-templates/default.toml"))
			.expect("parse default config template");
	config.build_role_map();
	config
}

fn agent_call(tool: &str, params: serde_json::Value) -> McpToolCall {
	McpToolCall {
		tool_name: tool.to_string(),
		tool_id: "test-call".to_string(),
		parameters: params,
	}
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

fn probe_dynamic_agent() -> crate::mcp::runtime::dynamic_agents::DynamicAgentConfig {
	crate::mcp::runtime::dynamic_agents::DynamicAgentConfig {
		name: "octo-probe-dyn".to_string(),
		description: "Probe agent for tests".to_string(),
		system: "You are a probe agent.".to_string(),
		welcome: String::new(),
		model: None,
		temperature: None,
		top_p: None,
		top_k: None,
		server_refs: Vec::new(),
		allowed_tools: Vec::new(),
		workdir: ".".to_string(),
	}
}

/// Poll the session inbox until a message arrives, bounded so a missing
/// release fails the test instead of hanging it.
async fn next_inbox_message() -> crate::session::inbox::InboxMessage {
	tokio::time::timeout(std::time::Duration::from_secs(10), async {
		loop {
			if let Some(message) = crate::session::inbox::try_pop_inbox_message() {
				return message;
			}
			tokio::time::sleep(std::time::Duration::from_millis(25)).await;
		}
	})
	.await
	.expect("an inbox message must arrive within the bound")
}

// --- build_agent_config ------------------------------------------------------

#[test]
#[serial]
fn build_agent_config_clears_mcp_without_server_refs_and_applies_model() {
	let agent = crate::mcp::runtime::dynamic_agents::DynamicAgentConfig {
		model: Some("openai:gpt-4o".to_string()),
		..probe_dynamic_agent()
	};
	let base = runtime_test_config();
	assert!(
		!base.mcp.servers.is_empty(),
		"default template ships MCP servers"
	);

	let merged = build_agent_config(&agent, &base);
	assert!(
		merged.mcp.servers.is_empty(),
		"no server_refs means MCP disabled for the agent"
	);
	assert!(
		merged.mcp.allowed_tools.is_empty(),
		"allowed_tools cleared with the servers"
	);
	assert_eq!(merged.model, "openai:gpt-4o", "model override applied");
}

#[test]
#[serial]
fn build_agent_config_resolves_server_refs_from_the_base_config() {
	// The dynamic-server merge branch needs a dynamic server that is
	// enabled=true, which only happens after a live MCP connection — that is
	// a real subprocess handshake and stays out of unit tests. The
	// server_refs resolution itself is covered against the static registry.
	let base = runtime_test_config();
	let server_name = base
		.mcp
		.servers
		.first()
		.expect("template config ships servers")
		.name()
		.to_string();

	let agent = crate::mcp::runtime::dynamic_agents::DynamicAgentConfig {
		server_refs: vec![server_name.clone()],
		allowed_tools: vec![format!("{server_name}:some_tool")],
		..probe_dynamic_agent()
	};
	let merged = build_agent_config(&agent, &base);
	let names: Vec<&str> = merged.mcp.servers.iter().map(|s| s.name()).collect();
	assert_eq!(
		names,
		vec![server_name.as_str()],
		"server_refs resolved against the base config servers"
	);
	assert_eq!(
		merged.mcp.allowed_tools,
		vec![format!("{server_name}:some_tool")],
		"agent allowed_tools carried into the merged config"
	);
}

// --- run_dynamic_agent_in_process cancellation ------------------------------

#[tokio::test]
async fn run_dynamic_agent_in_process_cancelled_before_start_fails_fast() {
	let (cancel_tx, cancel_rx) = watch::channel(false);
	cancel_tx.send(true).expect("pre-set cancellation");

	let err = run_dynamic_agent_in_process(
		&probe_dynamic_agent(),
		"do something",
		&runtime_test_config(),
		cancel_rx,
	)
	.await
	.expect_err("pre-cancelled run must fail before any API call");
	assert!(
		crate::session::cancellation::is_cancelled(&err),
		"failure must be the cancellation sentinel, got: {err:#}"
	);
}

// --- dynamic agent dispatch through execute_agent_command -------------------

#[tokio::test]
#[serial]
async fn dynamic_async_without_job_manager_is_a_tool_error() {
	crate::mcp::runtime::dynamic_agents::register_agent(probe_dynamic_agent())
		.expect("register probe agent");
	crate::mcp::runtime::dynamic_agents::enable_agent("octo-probe-dyn")
		.expect("enable probe agent");

	let result = execute_agent_command(
		&agent_call(
			"agent_octo-probe-dyn",
			serde_json::json!({"task": "x", "async": true}),
		),
		&runtime_test_config(),
		None,
	)
	.await
	.expect("dispatch");
	assert!(is_err(&result), "content: {}", text_of(&result));
	assert!(
		text_of(&result).contains("Async job manager not initialised"),
		"content: {}",
		text_of(&result)
	);

	crate::mcp::runtime::dynamic_agents::remove_agent("octo-probe-dyn")
		.expect("clean up probe agent");
}

#[tokio::test]
#[serial]
async fn dynamic_sync_api_failure_is_a_tool_error() {
	crate::mcp::runtime::dynamic_agents::register_agent(probe_dynamic_agent())
		.expect("register probe agent");
	crate::mcp::runtime::dynamic_agents::enable_agent("octo-probe-dyn")
		.expect("enable probe agent");

	// The default template has no provider credentials, so the model call must
	// fail — bounded so a hung provider still fails the test quickly.
	let result = tokio::time::timeout(
		std::time::Duration::from_secs(60),
		execute_agent_command(
			&agent_call("agent_octo-probe-dyn", serde_json::json!({"task": "x"})),
			&runtime_test_config(),
			None,
		),
	)
	.await
	.expect("model call must fail fast without credentials")
	.expect("dispatch");

	assert!(is_err(&result), "content: {}", text_of(&result));
	assert!(
		text_of(&result).contains("Agent execution failed"),
		"content: {}",
		text_of(&result)
	);

	crate::mcp::runtime::dynamic_agents::remove_agent("octo-probe-dyn")
		.expect("clean up probe agent");
}

#[tokio::test]
#[serial]
async fn dynamic_async_failure_releases_error_to_session_inbox() {
	// The agent registry is session-scoped while a session is active, so the
	// agent must be registered inside the session the tool runs in.
	let sid = "__agenttest_dyn_async".to_string();
	crate::session::context::with_session_id(sid.clone(), async {
		init_job_manager();
		crate::session::inbox::init_inbox_for_session();
		crate::mcp::runtime::dynamic_agents::register_agent(probe_dynamic_agent())
			.expect("register probe agent");
		crate::mcp::runtime::dynamic_agents::enable_agent("octo-probe-dyn")
			.expect("enable probe agent");

		let result = execute_agent_command(
			&agent_call(
				"agent_octo-probe-dyn",
				serde_json::json!({"task": "x", "async": true}),
			),
			&runtime_test_config(),
			None,
		)
		.await
		.expect("dispatch");
		assert!(!is_err(&result), "content: {}", text_of(&result));
		assert!(
			text_of(&result).contains("started asynchronously"),
			"content: {}",
			text_of(&result)
		);

		let manager = get_job_manager().expect("session job manager");
		let completed =
			tokio::time::timeout(std::time::Duration::from_secs(60), manager.wait_all())
				.await
				.expect("background job must finish");
		assert_eq!(completed, 1, "the background job completed");

		let message = next_inbox_message().await;
		assert!(
			message
				.content
				.contains("[Async agent 'octo-probe-dyn' failed]"),
			"content: {}",
			message.content
		);
		assert!(
			message.content.contains("OctoHub API error 401"),
			"the raw failure text is preserved: {}",
			message.content
		);
	})
	.await;

	// Drops the session-scoped agent registration, job manager and inbox.
	crate::session::context::cleanup_session(&sid);
}

// --- config-agent async failure release -------------------------------------

#[tokio::test]
#[serial]
async fn config_agent_async_failure_releases_error_to_session_inbox() {
	let mut config = runtime_test_config();
	config.agents = vec![crate::config::agents::AgentConfig {
		name: "scripted".to_string(),
		description: "Runs a canned command".to_string(),
		command: "octomind-test-no-such-binary".to_string(),
		workdir: ".".to_string(),
	}];

	let sid = "__agenttest_cfg_async".to_string();
	crate::session::context::with_session_id(sid, async {
		init_job_manager();
		crate::session::inbox::init_inbox_for_session();

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

		let manager = get_job_manager().expect("session job manager");
		let completed =
			tokio::time::timeout(std::time::Duration::from_secs(30), manager.wait_all())
				.await
				.expect("background job must finish");
		assert_eq!(completed, 1);

		let message = next_inbox_message().await;
		assert!(
			message.content.contains("[Async agent 'scripted' failed]"),
			"content: {}",
			message.content
		);
	})
	.await;
}

// --- ACP subprocess edge branches -------------------------------------------

#[cfg(unix)]
async fn run_script(script: &str) -> anyhow::Result<String> {
	let (_cancel_tx, cancel_rx) = watch::channel(false);
	let output = run_acp_command(
		"sh",
		&["-c", script],
		"task",
		&std::env::temp_dir(),
		cancel_rx,
		None,
		false,
	)
	.await;
	// Keep the sender alive through the await above; dropping it early would
	// read as cancellation.
	Ok(output?)
}

#[cfg(unix)]
#[tokio::test]
async fn acp_session_new_eof_after_initialize_fails_the_run() {
	// Answers initialize (id 1) then exits before the new/session response (id 2).
	let err = run_script("echo '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}'\nexit 0")
		.await
		.expect_err("EOF before the session response must fail");
	assert!(
		format!("{err:#}").contains("Subprocess closed before response"),
		"got: {err:#}"
	);
}

#[cfg(unix)]
#[tokio::test]
async fn acp_eof_before_prompt_response_skips_blank_and_garbage_lines() {
	// Handshake completes, then only blank/non-JSON noise precedes EOF — the
	// failure must be the no-partial-output variant.
	let err = run_script(concat!(
		"echo '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}'\n",
		"echo '{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"sessionId\":\"s\"}}'\n",
		"echo ''\n",
		"echo 'this is not json'\n",
		"exit 0",
	))
	.await
	.expect_err("EOF before the prompt response must fail");
	let message = format!("{err:#}");
	assert!(
		message.contains("ACP subprocess closed before the session/prompt response"),
		"got: {message}"
	);
	assert!(
		!message.contains("Partial output"),
		"no chunks were streamed: {message}"
	);
}

#[cfg(unix)]
#[tokio::test]
async fn acp_prompt_error_without_output_omits_partial_trailer() {
	let err = run_script(concat!(
		"echo '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}'\n",
		"echo '{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"sessionId\":\"s\"}}'\n",
		"echo '{\"jsonrpc\":\"2.0\",\"id\":3,\"error\":{\"code\":-32603,\"message\":\"Internal error\",\"data\":\"boom\"}}'\n",
		"exit 0",
	))
	.await
	.expect_err("prompt error must fail the run");
	let message = format!("{err:#}");
	assert!(
		message.contains("ACP prompt failed: boom"),
		"got: {message}"
	);
	assert!(
		!message.contains("Partial output"),
		"no chunks were streamed: {message}"
	);
}

#[cfg(unix)]
#[tokio::test]
async fn acp_kills_child_that_survives_stdout_eof() {
	// Completes the prompt response, closes stdout, but keeps running — the
	// run must still finish (bounded wait + process-group kill).
	let started = std::time::Instant::now();
	let output = tokio::time::timeout(
		std::time::Duration::from_secs(20),
		run_script(concat!(
			"echo '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}'\n",
			"echo '{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"sessionId\":\"s\"}}'\n",
			"echo '{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{\"update\":{\"sessionUpdate\":\"agent_message_chunk\",\"content\":{\"type\":\"text\",\"text\":\"hello\"}}}}'\n",
			"echo '{\"jsonrpc\":\"2.0\",\"id\":3,\"result\":{\"stopReason\":\"end_turn\"}}'\n",
			"exec 1>&-\n",
			"sleep 300",
		)),
	)
	.await
	.expect("run must terminate despite the surviving child")
	.expect("run succeeds");
	assert_eq!(output, "hello");
	assert!(
		started.elapsed() >= std::time::Duration::from_secs(4),
		"the bounded post-EOF wait must elapse before the kill: {:?}",
		started.elapsed()
	);
}

// --- wait_for_response low-level branches -----------------------------------

#[tokio::test]
async fn wait_for_response_cancelled_before_read_is_cancellation() {
	use tokio::io::{AsyncWriteExt, BufReader};

	let (mut writer, reader) = tokio::io::duplex(64);
	// Leave the stream open but never write: only the pre-set cancellation
	// can terminate the wait.
	let (_keep_tx, mut cancel_rx) = watch::channel(false);
	_keep_tx.send(true).expect("pre-set cancellation");
	let mut lines = BufReader::new(reader).lines();

	let err = wait_for_response(
		&mut lines,
		1,
		&mut cancel_rx,
		std::time::Duration::from_secs(5),
	)
	.await
	.expect_err("pre-set cancellation must fail the wait");
	assert!(
		crate::session::cancellation::is_cancelled(&err),
		"got: {err:#}"
	);
	let _ = writer.shutdown().await;
}

#[tokio::test]
async fn wait_for_response_ignores_spurious_false_notifications() {
	use tokio::io::{AsyncWriteExt, BufReader};

	let (mut writer, reader) = tokio::io::duplex(64);
	let (cancel_tx, mut cancel_rx) = watch::channel(false);
	let mut lines = BufReader::new(reader).lines();

	// A `false` notification must not abort the wait…
	cancel_tx.send(false).expect("spurious notification");
	// …give the wait a moment to observe it, then answer normally.
	tokio::time::sleep(std::time::Duration::from_millis(50)).await;
	writer
		.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":7,\"result\":{\"ok\":true}}\n")
		.await
		.expect("write response");

	let response = wait_for_response(
		&mut lines,
		7,
		&mut cancel_rx,
		std::time::Duration::from_secs(5),
	)
	.await
	.expect("a false notification must not cancel the wait");
	assert_eq!(
		response.pointer("/result/ok").and_then(|v| v.as_bool()),
		Some(true)
	);
	let _ = writer.shutdown().await;
}

// --- record_tap_live ignore branches ----------------------------------------

#[tokio::test]
#[serial]
async fn record_tap_live_ignores_messages_without_tool_or_usage_updates() {
	let sid = "__agenttest_taplive".to_string();
	crate::session::context::with_session_id(sid, async {
		crate::session::tap_runs::init_for_session();
		let (cancel_tx, _cancel_rx) = watch::channel(false);
		crate::session::tap_runs::register_job(crate::session::tap_runs::TapJob {
			id: "probe-run".to_string(),
			role: "probe:agent".to_string(),
			workdir: ".".to_string(),
			started_at: std::time::SystemTime::now(),
			status: std::sync::Arc::new(std::sync::RwLock::new(
				crate::session::tap_runs::TapJobStatus::Running,
			)),
			cancel_tx,
			live: std::sync::Arc::new(std::sync::RwLock::new(Default::default())),
		});

		// A tool_call update sets the live action…
		record_tap_live(
			"probe-run",
			&serde_json::json!({
				"params": {"update": {"sessionUpdate": "tool_call",
					"toolCallId": "t1", "title": "running probe"}}
			}),
		);
		let recorded = crate::session::tap_runs::find_job("probe-run")
			.expect("job registered")
			.live
			.last_action
			.clone();
		assert_eq!(recorded.as_deref(), Some("running probe"));

		// …messages without a recognized update kind must not touch it.
		record_tap_live(
			"probe-run",
			&serde_json::json!({
				"params": {"update": {"sessionUpdate": "plan",
					"plan": "some plan text"}}
			}),
		);
		record_tap_live("probe-run", &serde_json::json!({"params": {"_meta": {}}}));
		let after = crate::session::tap_runs::find_job("probe-run")
			.expect("job still registered")
			.live
			.last_action
			.clone();
		assert_eq!(
			after, recorded,
			"unrecognized updates must leave the live action untouched"
		);
	})
	.await;
}
