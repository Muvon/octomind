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

//! End-to-end tests of the API/tool orchestration loop against a scripted
//! fake provider. A local HTTP stub speaks the OpenAI-compatible
//! chat-completions schema; `OLLAMA_API_URL` points octolib's ollama
//! provider at it, so the REAL stack runs: request building, HTTP, response
//! parsing, tool-call extraction, tool execution, follow-up calls, message
//! and cost bookkeeping. No network, no API keys, no side effects.

use super::*;
use crate::session::chat::test_support::{
	fake_provider_config, fake_session, final_response, spawn_stub, spawn_stub_with_status, tool_call_response,
	tool_calls_response, ENV_LOCK,
};
use crate::session::output::SilentSink;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn run_turn(session: &mut ChatSession, config: &Config) -> anyhow::Result<()> {
	let (_tx, rx) = tokio::sync::watch::channel(false);
	execute_api_call_and_process_response(
		session,
		config,
		"assistant",
		rx,
		crate::session::output::OutputMode::NonInteractive,
		SilentSink,
	)
	.await
}

#[tokio::test]
async fn test_simple_completion_turn() {
	let _guard = ENV_LOCK.lock().await;
	let url = spawn_stub(vec![final_response("Hello from stub")]).await;
	std::env::set_var("OLLAMA_API_URL", &url);

	let config = fake_provider_config();
	let mut session = fake_session("hi there");

	run_turn(&mut session, &config).await.expect("turn succeeds");

	let messages = &session.session.messages;
	assert_eq!(messages[0].role, "user");
	let assistant = messages
		.iter()
		.find(|m| m.role == "assistant")
		.expect("assistant reply recorded");
	assert!(assistant.content.contains("Hello from stub"));

	// Usage flowed into the session bookkeeping
	assert!(session.session.info.total_api_calls >= 1);
	assert!(session.session.info.output_tokens >= 10);
	assert!(session.session.info.total_cost > 0.0);
	assert_eq!(session.session.info.turn_timing.completed, 1);
	assert!(session.turn_started_at.is_none());
}
/// Like the shared `spawn_stub`, but records every request body it receives,
/// so tests can assert on what actually crossed the wire — not just on the
/// conversation state after the fact. Returns the chat-completions URL plus
/// the shared capture buffer.
async fn spawn_recording_stub(
	responses: Vec<serde_json::Value>,
) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
		.await
		.expect("bind recording stub listener");
	let addr = listener.local_addr().expect("recording stub addr");
	let queue = std::sync::Arc::new(std::sync::Mutex::new(std::collections::VecDeque::from(responses)));
	let requests: std::sync::Arc<std::sync::Mutex<Vec<String>>> =
		std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

	tokio::spawn({
		let requests = requests.clone();
		async move {
			while let Ok((mut sock, _)) = listener.accept().await {
				let queue = queue.clone();
				let requests = requests.clone();
				tokio::spawn(async move {
					// Read headers + Content-Length body of the POST request.
					let mut buf = Vec::new();
					let mut tmp = [0u8; 8192];
					let header_end = loop {
						let n = sock.read(&mut tmp).await.unwrap_or(0);
						if n == 0 {
							return;
						}
						buf.extend_from_slice(&tmp[..n]);
						if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
							break pos + 4;
						}
					};
					let headers = String::from_utf8_lossy(&buf[..header_end]).to_lowercase();
					let content_length: usize = headers
						.lines()
						.find_map(|l| l.strip_prefix("content-length:"))
						.and_then(|v| v.trim().parse().ok())
						.unwrap_or(0);
					while buf.len() < header_end + content_length {
						let n = sock.read(&mut tmp).await.unwrap_or(0);
						if n == 0 {
							break;
						}
						buf.extend_from_slice(&tmp[..n]);
					}

					// Capture the wire payload before answering, so the buffer is
					// populated by the time the caller has seen the response.
					requests
						.lock()
						.expect("recording stub requests")
						.push(String::from_utf8_lossy(&buf[header_end..]).to_string());

					let body = queue
						.lock()
						.expect("recording stub queue")
						.pop_front()
						.unwrap_or_else(|| final_response("SCRIPT EXHAUSTED"))
						.to_string();
					let response = format!(
						"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
						body.len(),
						body
					);
					let _ = sock.write_all(response.as_bytes()).await;
					let _ = sock.shutdown().await;
				});
			}
		}
	});

	(format!("http://{}/v1/chat/completions", addr), requests)
}

/// A note a plugin queued is written at the safe pre-request point and lands
/// as a system-managed user-role message BEFORE the provider request is
/// built — so the one request the stub serves already contains it, ahead of
/// the assistant reply.
#[tokio::test]
async fn a_queued_plugin_note_is_injected_as_a_system_managed_user_message() {
	let _guard = ENV_LOCK.lock().await;
	let (url, requests) = spawn_recording_stub(vec![final_response("steer acknowledged")]).await;
	std::env::set_var("OLLAMA_API_URL", &url);

	let config = fake_provider_config();
	let mut session = fake_session("do the task");
	session.queue_plugin_notes(vec![
		"<pay-attention>change approach: use the indexed lookup</pay-attention>".to_string(),
	]);

	run_turn(&mut session, &config).await.expect("the turn succeeds");

	// The queued note was consumed, not left pending for a later turn
	assert!(session.plugin_notes.is_empty());

	let messages = &session.session.messages;
	let note_pos = messages
		.iter()
		.position(|m| m.role == "user" && m.content.contains("use the indexed lookup"))
		.expect("the note is injected as a user-role message");
	assert!(crate::session::is_system_managed_user_content(
		&messages[note_pos].content
	));
	// Injected before the request: it precedes the stub-served assistant
	// reply in the same conversation that was sent to the provider.
	let assistant_pos = messages
		.iter()
		.position(|m| m.role == "assistant")
		.expect("assistant reply recorded");
	assert!(
		note_pos < assistant_pos,
		"the note must precede the API request, got roles: {:?}",
		messages.iter().map(|m| m.role.as_str()).collect::<Vec<_>>()
	);
	assert!(messages[assistant_pos].content.contains("steer acknowledged"));

	// The wire proves it: the request the stub actually served already
	// carried the note as a user-role message in its payload.
	let captured = requests.lock().expect("captured requests");
	assert_eq!(captured.len(), 1, "exactly one provider request expected");
	let payload: serde_json::Value = serde_json::from_str(&captured[0]).expect("request body is JSON");
	let carried = payload["messages"]
		.as_array()
		.expect("request carries a messages array")
		.iter()
		.any(|m| {
			m["role"] == "user"
				&& m["content"]
					.as_str()
					.is_some_and(|c| c.contains("use the indexed lookup"))
		});
	assert!(carried, "the note must be inside the request payload: {}", captured[0]);
	std::env::remove_var("OLLAMA_API_URL");
}

/// Interactive-mode pre-request spending gates: the session threshold is
/// disabled (its check short-circuits to Ok(true) without prompting) and
/// the request threshold is exceeded, so the turn must end cleanly BEFORE
/// any provider call — the scripted stub is never hit and nothing is
/// recorded or injected.
#[tokio::test]
async fn request_spending_threshold_stops_turn_before_api_call() {
	let _guard = ENV_LOCK.lock().await;
	let url = spawn_stub(vec![final_response("must never be requested")]).await;
	std::env::set_var("OLLAMA_API_URL", &url);

	let mut config = fake_provider_config();
	// Session-level threshold disabled: its branch returns Ok(true), so the
	// request-level gate below is the one that fires.
	config.max_session_spending_threshold = 0.0;
	config.max_request_spending_threshold = 0.0001;

	let mut session = fake_session("spend past the cap");
	session.session.info.total_cost = 1.0;

	let (_tx, rx) = tokio::sync::watch::channel(false);
	execute_api_call_and_process_response(
		&mut session,
		&config,
		"assistant",
		rx,
		crate::session::output::OutputMode::Interactive,
		SilentSink,
	)
	.await
	.expect("threshold stop is a clean turn end");

	// The gate fired before the request: zero API calls (the stub queue is
	// untouched) and the conversation is exactly the original user message.
	assert_eq!(session.session.info.total_api_calls, 0);
	assert_eq!(session.session.messages.len(), 1);
	assert_eq!(session.session.messages[0].role, "user");
	assert!(session.turn_answers.is_empty());

	std::env::remove_var("OLLAMA_API_URL");
}

#[tokio::test]
async fn test_tool_round_trip_with_unknown_tool() {
	let _guard = ENV_LOCK.lock().await;
	let url = spawn_stub(vec![
		tool_call_response("stub_missing_tool", serde_json::json!({"arg": 1})),
		final_response("All done"),
	])
	.await;
	std::env::set_var("OLLAMA_API_URL", &url);

	let config = fake_provider_config();
	let mut session = fake_session("do the thing");

	run_turn(&mut session, &config).await.expect("turn succeeds");

	let messages = &session.session.messages;
	// The tool_calls assistant message is preserved for API pairing
	let tool_call_msg = messages
		.iter()
		.find(|m| m.tool_calls.is_some())
		.expect("assistant tool_calls message recorded");
	assert!(tool_call_msg
		.tool_calls
		.as_ref()
		.expect("calls")
		.to_string()
		.contains("stub_missing_tool"));

	// The unknown tool produced an error tool-result the model can see
	let tool_msg = messages
		.iter()
		.find(|m| m.role == "tool")
		.expect("tool result message recorded");
	assert!(
		tool_msg.content.contains("stub_missing_tool"),
		"tool error should name the tool, got: {}",
		tool_msg.content
	);

	// The follow-up call delivered the final answer
	let last_assistant = messages
		.iter()
		.rev()
		.find(|m| m.role == "assistant" && !m.content.is_empty())
		.expect("final assistant reply");
	assert!(last_assistant.content.contains("All done"));

	// Two API calls (initial + follow-up), both with usage/cost
	assert!(session.session.info.total_api_calls >= 2);
	assert!(session.session.info.total_cost >= 0.003 - 1e-9);
}

#[tokio::test]
async fn test_parallel_tools_and_multi_round_chain() {
	let _guard = ENV_LOCK.lock().await;
	let url = spawn_stub(vec![
		// Round 1: two parallel tool calls in one assistant message
		tool_calls_response(&[
			("call_a", "stub_tool_a", serde_json::json!({"n": 1})),
			("call_b", "stub_tool_b", serde_json::json!({"n": 2})),
		]),
		// Round 2: the loop continues with another tool call
		tool_call_response("stub_tool_c", serde_json::json!({"n": 3})),
		// Round 3: final answer ends the turn
		final_response("Chain complete"),
	])
	.await;
	std::env::set_var("OLLAMA_API_URL", &url);

	let config = fake_provider_config();
	let mut session = fake_session("run the chain");

	run_turn(&mut session, &config).await.expect("turn succeeds");

	let messages = &session.session.messages;
	// Every scripted call produced a tool-result message, ids preserved
	let tool_ids: Vec<&str> = messages
		.iter()
		.filter(|m| m.role == "tool")
		.filter_map(|m| m.tool_call_id.as_deref())
		.collect();
	assert!(tool_ids.contains(&"call_a"), "got tool ids: {tool_ids:?}");
	assert!(tool_ids.contains(&"call_b"), "got tool ids: {tool_ids:?}");
	assert_eq!(tool_ids.len(), 3, "got tool ids: {tool_ids:?}");

	let last_assistant = messages
		.iter()
		.rev()
		.find(|m| m.role == "assistant" && !m.content.is_empty())
		.expect("final assistant reply");
	assert!(last_assistant.content.contains("Chain complete"));

	// Three API round trips were made and billed
	assert!(session.session.info.total_api_calls >= 3);
}

#[tokio::test]
async fn test_reasoning_content_is_preserved_as_thinking() {
	let _guard = ENV_LOCK.lock().await;
	let url = spawn_stub(vec![serde_json::json!({
		"choices": [{
			"message": {
				"role": "assistant",
				"content": "The answer is 4.",
				"reasoning": "2 + 2 must be 4 because arithmetic."
			},
			"finish_reason": "stop"
		}],
		"usage": {"prompt_tokens": 8, "completion_tokens": 12, "total_tokens": 20}
	})])
	.await;
	std::env::set_var("OLLAMA_API_URL", &url);

	let config = fake_provider_config();
	let mut session = fake_session("what is 2+2?");
	run_turn(&mut session, &config).await.expect("turn succeeds");

	let assistant = session
		.session
		.messages
		.iter()
		.rev()
		.find(|m| m.role == "assistant")
		.expect("assistant reply");
	assert!(assistant.content.contains("The answer is 4."));
	// Reasoning must never leak into the visible content; whether it is
	// retained as a thinking block is model-policy, so only assert shape
	// when present.
	assert!(!assistant.content.contains("arithmetic"));
	if let Some(thinking) = &assistant.thinking {
		let serialized = serde_json::to_string(thinking).unwrap_or_default();
		assert!(
			serialized.contains("arithmetic"),
			"stored thinking lost the reasoning: {serialized}"
		);
	}
}

#[tokio::test]
async fn test_provider_error_surfaces_as_turn_error() {
	let _guard = ENV_LOCK.lock().await;
	// Persistent 500s: retries (if any) also hit an error response
	let url = spawn_stub_with_status(vec![
		(500, serde_json::json!({"error": {"message": "stub exploded"}})),
		(500, serde_json::json!({"error": {"message": "stub exploded"}})),
		(500, serde_json::json!({"error": {"message": "stub exploded"}})),
		(500, serde_json::json!({"error": {"message": "stub exploded"}})),
	])
	.await;
	std::env::set_var("OLLAMA_API_URL", &url);

	let mut config = fake_provider_config();
	// Keep the failure path fast: no exponential-backoff marathon
	config.max_retries = 1;
	config.retry_timeout = 1;
	let mut session = fake_session("hi");

	let result = run_turn(&mut session, &config).await;
	assert!(result.is_err(), "persistent 500s must fail the turn");
	// No assistant message was fabricated for the failed call
	assert!(!session.session.messages.iter().any(|m| m.role == "assistant"));
}

#[tokio::test]
async fn test_empty_response_is_retried_by_validation() {
	let _guard = ENV_LOCK.lock().await;
	let url = spawn_stub(vec![
		// Empty completion: no content, no tool calls — the validation layer
		// must not accept this as a final answer.
		serde_json::json!({
			"choices": [{
				"message": {"role": "assistant", "content": ""},
				"finish_reason": "stop"
			}],
			"usage": {"prompt_tokens": 5, "completion_tokens": 0, "total_tokens": 5}
		}),
		final_response("Recovered answer"),
	])
	.await;
	std::env::set_var("OLLAMA_API_URL", &url);

	let config = fake_provider_config();
	let mut session = fake_session("hello?");

	// Whether the empty completion is retried internally or surfaced, the
	// turn must not panic and must not record a fabricated non-empty answer.
	match run_turn(&mut session, &config).await {
		Ok(()) => {
			if let Some(last) = session.session.messages.iter().rev().find(|m| m.role == "assistant") {
				assert!(
					last.content.is_empty() || last.content.contains("Recovered answer"),
					"unexpected fabricated content: {}",
					last.content
				);
			}
		}
		Err(error) => {
			let text = error.to_string().to_lowercase();
			assert!(
				text.contains("empty") || text.contains("no content") || text.contains("response"),
				"unexpected error kind: {error}"
			);
		}
	}
}

/// A real successful tool round: the model calls the builtin orchestration
/// `schedule` tool (list on an empty store), the dispatcher routes and
/// executes it in-process, and the follow-up call produces the final answer.
#[tokio::test]
async fn test_real_builtin_tool_round_schedule_list() {
	let _guard = ENV_LOCK.lock().await;
	let url = spawn_stub(vec![
		tool_call_response("schedule", serde_json::json!({"command": "list"})),
		final_response("schedule round complete"),
	])
	.await;
	std::env::set_var("OLLAMA_API_URL", &url);

	let config = fake_provider_config();
	// Tool routing is tool-map-only; build it from the merged config so this
	// test never depends on another test having initialized the global map.
	crate::mcp::tool_map::initialize_tool_map(&config)
		.await
		.expect("init tool map");
	let mut session = fake_session("list my schedules");
	run_turn(&mut session, &config).await.expect("tool round turn");

	let tool_msg = session
		.session
		.messages
		.iter()
		.find(|m| m.role == "tool")
		.expect("tool result message recorded");
	assert_eq!(tool_msg.name.as_deref(), Some("schedule"));
	assert_eq!(tool_msg.tool_call_id.as_deref(), Some("call_1"));
	assert!(
		!tool_msg.content.to_lowercase().contains("not implemented"),
		"schedule must execute for real, got: {}",
		tool_msg.content
	);
	let last = session.session.messages.last().expect("final assistant message");
	assert_eq!(last.role, "assistant");
	assert!(last.content.contains("schedule round complete"));

	std::env::remove_var("OLLAMA_API_URL");
}

/// Cancellation signalled before the turn starts: the turn must end without
/// recording any assistant output — gracefully (Ok) or as a cancel error,
/// but never with a fabricated answer.
#[tokio::test]
async fn test_pre_cancelled_turn_records_nothing() {
	let _guard = ENV_LOCK.lock().await;
	let url = spawn_stub(vec![final_response("must never be recorded")]).await;
	std::env::set_var("OLLAMA_API_URL", &url);

	let config = fake_provider_config();
	let mut session = fake_session("do something");
	let (tx, rx) = tokio::sync::watch::channel(false);
	tx.send(true).expect("signal cancel");

	let result = execute_api_call_and_process_response(
		&mut session,
		&config,
		"assistant",
		rx,
		crate::session::output::OutputMode::NonInteractive,
		SilentSink,
	)
	.await;

	let recorded_answer = session
		.session
		.messages
		.iter()
		.any(|m| m.role == "assistant" && m.content.contains("must never be recorded"));
	assert!(
		!recorded_answer,
		"cancelled turn must not record the answer (result was {result:?})"
	);

	std::env::remove_var("OLLAMA_API_URL");
}

/// Interactive output mode: the same tool round now renders headers, close
/// lines, and (with a tiny threshold) the truncation indicator — the paths
/// non-interactive runs suppress entirely.
#[tokio::test]
async fn test_interactive_mode_tool_round_renders_and_truncates() {
	let _guard = ENV_LOCK.lock().await;
	let url = spawn_stub(vec![
		tool_call_response("schedule", serde_json::json!({"command": "list"})),
		final_response("interactive round done"),
	])
	.await;
	std::env::set_var("OLLAMA_API_URL", &url);

	let mut config = fake_provider_config();
	// Force the truncation display arm for any non-trivial tool output
	config.mcp_response_tokens_threshold = 5;
	crate::mcp::tool_map::initialize_tool_map(&config)
		.await
		.expect("init tool map");

	let mut session = fake_session("list my schedules");
	let (_tx, rx) = tokio::sync::watch::channel(false);
	execute_api_call_and_process_response(
		&mut session,
		&config,
		"assistant",
		rx,
		crate::session::output::OutputMode::Interactive,
		SilentSink,
	)
	.await
	.expect("interactive turn");

	let last = session.session.messages.last().expect("final assistant message");
	assert_eq!(last.role, "assistant");
	assert!(last.content.contains("interactive round done"));

	std::env::remove_var("OLLAMA_API_URL");
}

/// A genuinely oversized tool result drives the hard truncation cap, and
/// re-issuing the identical call drives the dedup placeholder — the two
/// large-output defenses. The skill list is grown here from a temp workdir
/// rather than whatever tap the machine happens to have: on a bare CI runner
/// the real list is "No skills found", which is under both thresholds.
#[tokio::test]
#[serial_test::serial]
async fn test_large_tool_result_truncation_and_dedup() {
	let _guard = ENV_LOCK.lock().await;
	let _data = crate::session::chat::test_support::DataDirGuard::new();
	let workdir = tempfile::tempdir().expect("temp workdir");
	let skills_root = workdir.path().join(".agents").join("skills");
	for i in 0..40 {
		let dir = skills_root.join(format!("bulk-skill-{i:02}"));
		std::fs::create_dir_all(&dir).expect("skill dir");
		std::fs::write(
			dir.join("SKILL.md"),
			format!(
				"---\nname: bulk-skill-{i:02}\ndescription: filler skill {i:02} used to grow the list past the truncation and dedup thresholds\n---\n\nbody\n"
			),
		)
		.expect("write SKILL.md");
	}
	let sid = "api-exec-large-tool-result".to_string();
	crate::session::context::with_session_id(sid.clone(), async {
		crate::session::context::init_session_services("assistant");
		crate::mcp::workdir::set_session_working_directory(workdir.path().to_path_buf());

		let url = spawn_stub(vec![
			tool_calls_response(&[("call_s1", "skill", serde_json::json!({"action": "list"}))]),
			tool_calls_response(&[("call_s2", "skill", serde_json::json!({"action": "list"}))]),
			final_response("spill round done"),
		])
		.await;
		std::env::set_var("OLLAMA_API_URL", &url);

		let mut config = fake_provider_config();
		config.mcp_response_tokens_threshold = 50;
		crate::mcp::tool_map::initialize_tool_map(&config)
			.await
			.expect("init tool map");

		let mut session = fake_session("list every skill twice");
		session.session.info.name = sid.clone();
		run_turn(&mut session, &config).await.expect("double skill round");

		let tool_contents: Vec<(&str, &str)> = session
			.session
			.messages
			.iter()
			.filter(|m| m.role == "tool")
			.map(|m| (m.tool_call_id.as_deref().unwrap_or(""), m.content.as_str()))
			.collect();
		assert_eq!(tool_contents.len(), 2, "both rounds must record results");

		// First round: hard cap applied — the result cannot exceed the threshold
		// by more than the truncation notice itself.
		let first = tool_contents[0].1;
		assert!(
			first.contains(crate::utils::truncation::TRUNCATION_NOTICE_TAG),
			"truncation notice missing: {first}"
		);
		assert!(
			crate::session::token_counter::estimate_tokens(first) < 400,
			"oversized result was not capped ({} chars)",
			first.len()
		);

		// Second round: identical call → dedup placeholder, not a re-send
		let second = tool_contents[1].1;
		assert!(
			second.contains("duplicate tool call"),
			"dedup placeholder missing: {second}"
		);

		let last = session.session.messages.last().expect("final message");
		assert!(last.content.contains("spill round done"));

		std::env::remove_var("OLLAMA_API_URL");
		crate::session::context::cleanup_session(&sid);
	})
	.await;
}

/// The loop hard stop is the turn's last word: the same call returning the same
/// result `max_identical_rounds` times ends the turn on the stop notice and the
/// turn is not requested again. The script has a spare entry that only such a
/// re-request would consume.
#[tokio::test]
#[cfg(unix)]
async fn test_loop_stop_ends_the_turn_without_gate_reentry() {
	use std::os::unix::fs::PermissionsExt;
	let _guard = ENV_LOCK.lock().await;
	// A read-only local tool: a write tool's repeat counts as progress, never as a loop.
	let tmp = tempfile::tempdir().expect("tempdir");
	let tools = tmp.path().join(".agents/tools");
	std::fs::create_dir_all(&tools).expect("create tools dir");
	let script = tools.join("probe");
	std::fs::write(
		&script,
		"#!/bin/sh\n# @description Print a fixed line.\nprintf 'False\\n'\n",
	)
	.expect("write tool script");
	std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod tool script");

	let sid = "api-exec-loop-stop".to_string();
	crate::session::context::with_session_id(sid.clone(), async {
		crate::session::context::init_session_services("assistant");
		crate::mcp::workdir::set_session_working_directory(tmp.path().to_path_buf());
		let url = spawn_stub(vec![
			tool_call_response("probe", serde_json::json!({})),
			tool_call_response("probe", serde_json::json!({})),
			tool_call_response("probe", serde_json::json!({})),
			final_response("never requested"),
		])
		.await;
		std::env::set_var("OLLAMA_API_URL", &url);

		let mut config = fake_provider_config();
		config.max_identical_rounds = 3;
		// The tool loop needs at least one configured MCP server.
		config.mcp.servers.push(crate::config::McpServerConfig::Builtin {
			name: "core".to_string(),
			timeout_seconds: 300,
			tools: vec![],
			auto_bind: None,
		});
		let mut session = fake_session("check the library");
		session.session.info.name = sid.clone();
		session.completion_gate_eligible = true;

		run_turn(&mut session, &config).await.expect("loop stop ends the turn");

		assert_eq!(session.session.info.total_api_calls, 3, "no request after the stop");
		let last = session.session.messages.last().expect("final assistant message");
		assert_eq!(last.role, "assistant");
		assert!(last
			.content
			.starts_with("Stopped by the supervisor: the last 3 tool rounds"));
		assert!(!session
			.session
			.messages
			.iter()
			.any(|m| m.content.contains("never requested")));

		std::env::remove_var("OLLAMA_API_URL");
		crate::session::context::cleanup_session(&sid);
	})
	.await;
}

// ---------------------------------------------------------------------------
// Interactive-mode spending gates
// ---------------------------------------------------------------------------

/// Run one turn in a chosen output mode, mirroring `run_turn`.
async fn run_turn_mode(
	session: &mut ChatSession,
	config: &Config,
	mode: crate::session::output::OutputMode,
) -> anyhow::Result<()> {
	let (_tx, rx) = tokio::sync::watch::channel(false);
	execute_api_call_and_process_response(session, config, "assistant", rx, mode, SilentSink).await
}

#[tokio::test]
async fn test_interactive_mode_session_threshold_decline_skips_api_call() {
	let _guard = ENV_LOCK.lock().await;
	// No stub: if the gate leaks, the provider call fails loudly and the test
	// fails — proving the decline path really returns before any request.
	let mut config = fake_provider_config();
	config.max_session_spending_threshold = 0.01;
	config.max_request_spending_threshold = 0.0;

	let mut session = fake_session("hi there");
	session.session.info.total_cost = 1.0;
	let messages_before = session.session.messages.len();
	let calls_before = session.session.info.total_api_calls;

	run_turn_mode(&mut session, &config, crate::session::output::OutputMode::Interactive)
		.await
		.expect("declined turn returns Ok");

	assert_eq!(
		session.session.messages.len(),
		messages_before,
		"a declined turn must not add messages"
	);
	assert_eq!(
		session.session.info.total_api_calls, calls_before,
		"a declined turn must not call the API"
	);
}

#[tokio::test]
async fn test_interactive_mode_request_threshold_decline_skips_api_call() {
	let _guard = ENV_LOCK.lock().await;
	let mut config = fake_provider_config();
	config.max_session_spending_threshold = 0.0;
	config.max_request_spending_threshold = 0.01;

	let mut session = fake_session("hi there");
	session.session.info.total_cost = 1.0;
	let messages_before = session.session.messages.len();

	run_turn_mode(&mut session, &config, crate::session::output::OutputMode::Interactive)
		.await
		.expect("declined turn returns Ok");

	assert_eq!(
		session.session.messages.len(),
		messages_before,
		"a request-threshold decline must not add messages"
	);
}

// ---------------------------------------------------------------------------
// Structured-output schema branch
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_schema_branch_completes_structured_turn() {
	let _guard = ENV_LOCK.lock().await;
	// The stub replies with a JSON object satisfying the requested schema.
	let url = spawn_stub(vec![final_response(r#"{"message":"structured hello"}"#)]).await;
	std::env::set_var("OLLAMA_API_URL", &url);

	let config = fake_provider_config();
	let mut session = fake_session("give me a string");
	session.model = "ollama:qwen2.5:72b".to_string();
	session.session.info.model = session.model.clone();
	session.schema = Some(serde_json::json!({
		"type": "object",
		"properties": {"message": {"type": "string"}},
		"required": ["message"],
		"additionalProperties": false
	}));

	run_turn(&mut session, &config)
		.await
		.expect("schema-constrained turn succeeds");

	let assistant = session
		.session
		.messages
		.iter()
		.find(|m| m.role == "assistant")
		.expect("assistant reply recorded");
	assert!(
		assistant.content.contains("structured hello"),
		"unexpected reply: {}",
		assistant.content
	);

	std::env::remove_var("OLLAMA_API_URL");
}

// ---------------------------------------------------------------------------
// Active memory pack lifecycle
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_stale_active_memory_pack_cleared_when_learning_disabled() {
	let _guard = ENV_LOCK.lock().await;
	let url = spawn_stub(vec![final_response("Hello from stub")]).await;
	std::env::set_var("OLLAMA_API_URL", &url);

	// A pack left over from an earlier round must be dropped before the request.
	let config = fake_provider_config();
	let mut session = fake_session("hi there");
	session.active_memory_pack = Some("stale pack content".to_string());

	run_turn(&mut session, &config).await.expect("turn succeeds");

	assert!(
		session.active_memory_pack.is_none(),
		"a pack must not survive into a request when learning is disabled"
	);

	std::env::remove_var("OLLAMA_API_URL");
}

// ---------------------------------------------------------------------------
// Plugin events around a turn
// ---------------------------------------------------------------------------

use crate::plugins::test_support::{connect_stub, StubPlugin};
use serde_json::json;

/// A session under its own id: stubs attach to the process-wide plugin host by
/// session id, and tests run in parallel.
fn plugin_session(id: &str) -> ChatSession {
	let mut session = ChatSession::for_tests(Vec::new());
	session.model = "ollama:fake-model".to_string();
	session.session.info.model = "ollama:fake-model".to_string();
	session.session.info.name = id.to_string();
	session
}

/// Connect `stub` to the session and add the user's prompt as the real loop does.
async fn start_plugin_turn(session: &mut ChatSession, stub: &StubPlugin, input: &str) {
	connect_stub(&session.session.info.name, "assistant", std::path::Path::new("."), stub).await;
	let prompt = session
		.submit_prompt(input.to_string(), input)
		.await
		.expect("prompt accepted");
	session.add_user_message(&prompt).expect("user message added");
}

/// The events the stub was sent, in order, without the handshake.
fn events_heard(stub: &StubPlugin) -> Vec<String> {
	stub.received()
		.into_iter()
		.map(|(method, _)| method)
		.filter(|method| event::ALL.contains(&method.as_str()))
		.collect()
}

#[tokio::test]
async fn plugin_hears_prompt_request_and_turn_end_in_order() {
	let _guard = ENV_LOCK.lock().await;
	let (url, _requests) = spawn_recording_stub(vec![final_response("All done")]).await;
	std::env::set_var("OLLAMA_API_URL", &url);
	let config = fake_provider_config();
	let stub = StubPlugin::new(&[event::PROMPT_SUBMIT, event::REQUEST_BEFORE, event::TURN_END]);
	let mut session = plugin_session("o5-order");
	start_plugin_turn(&mut session, &stub, "fix the bug").await;

	run_turn(&mut session, &config).await.expect("turn succeeds");

	assert_eq!(events_heard(&stub), ["prompt.submit", "request.before", "turn.end"]);
	let request = &stub.sent(event::REQUEST_BEFORE)[0];
	assert_eq!(request["phase"], "turn_start");
	assert_eq!(request["turn"], "user");
	assert!(
		request["tools"].is_array(),
		"the first request of a turn carries the tool list"
	);
	let end = &stub.sent(event::TURN_END)[0];
	assert_eq!(end["turn"], "user");
	assert_eq!(end["requests"], 1);
	assert!(end["answers"][0].as_str().unwrap().contains("All done"));
	assert!(end["changes"].as_array().unwrap().is_empty());
	assert!(end.get("stopped").is_none());

	crate::plugins::end_session("o5-order");
	std::env::remove_var("OLLAMA_API_URL");
}

#[tokio::test]
async fn continue_effect_reruns_the_request_once_as_continuation() {
	let _guard = ENV_LOCK.lock().await;
	let (url, requests) = spawn_recording_stub(vec![final_response("first"), final_response("second")]).await;
	std::env::set_var("OLLAMA_API_URL", &url);
	let config = fake_provider_config();
	let turn_ends = std::sync::atomic::AtomicUsize::new(0);
	let stub = StubPlugin::new(&[event::REQUEST_BEFORE, event::TURN_END]).respond(move |method, _| {
		let first_turn_end =
			method == event::TURN_END && turn_ends.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0;
		Ok(if first_turn_end {
			json!({"continue": "keep going"})
		} else {
			json!({})
		})
	});
	let mut session = plugin_session("o5-continue");
	start_plugin_turn(&mut session, &stub, "fix the bug").await;

	run_turn(&mut session, &config).await.expect("turn succeeds");

	let phases: Vec<_> = stub
		.sent(event::REQUEST_BEFORE)
		.iter()
		.map(|params| params["phase"].clone())
		.collect();
	assert_eq!(phases, [json!("turn_start"), json!("continuation")]);
	assert!(
		stub.sent(event::REQUEST_BEFORE)[1].get("tools").is_none(),
		"only the first request of a turn carries the tool list"
	);
	let ends = stub.sent(event::TURN_END);
	assert_eq!(ends.len(), 2);
	assert_eq!(ends[1]["answers"].as_array().unwrap().len(), 2);
	assert_eq!(requests.lock().unwrap().len(), 2);
	assert_eq!(session.plugin_continuations, 1);
	let text = session
		.session
		.messages
		.iter()
		.find(|message| message.role == "user" && message.content.contains("keep going"))
		.expect("the continue text is written as a system note");
	assert!(crate::session::is_system_managed_user_content(&text.content));

	crate::plugins::end_session("o5-continue");
	std::env::remove_var("OLLAMA_API_URL");
}

#[tokio::test]
async fn continuations_stop_at_the_cap() {
	let _guard = ENV_LOCK.lock().await;
	let (url, requests) = spawn_recording_stub(Vec::new()).await;
	std::env::set_var("OLLAMA_API_URL", &url);
	let config = fake_provider_config();
	let stub = StubPlugin::new(&[event::TURN_END]).answer(json!({"continue": "again"}));
	let mut session = plugin_session("o5-cap");
	start_plugin_turn(&mut session, &stub, "fix the bug").await;

	run_turn(&mut session, &config).await.expect("turn succeeds");

	// The first request plus five re-runs; the sixth `continue` is not followed.
	assert_eq!(requests.lock().unwrap().len(), 6);
	assert_eq!(stub.sent(event::TURN_END).len(), 6);
	assert_eq!(session.plugin_continuations, MAX_PLUGIN_CONTINUATIONS);

	crate::plugins::end_session("o5-cap");
	std::env::remove_var("OLLAMA_API_URL");
}

#[tokio::test]
async fn context_blocks_reach_the_provider_and_never_the_history() {
	let _guard = ENV_LOCK.lock().await;
	let (url, requests) = spawn_recording_stub(vec![final_response("ok")]).await;
	std::env::set_var("OLLAMA_API_URL", &url);
	let config = fake_provider_config();
	let stub = StubPlugin::new(&[event::REQUEST_BEFORE])
		.respond(|_, _| Ok(json!({"context": [{"name": "one", "text": "ALPHA"}, {"name": "two", "text": "BETA"}]})));
	let mut session = plugin_session("o5-context");
	start_plugin_turn(&mut session, &stub, "fix the bug").await;

	run_turn(&mut session, &config).await.expect("turn succeeds");

	let sent = requests.lock().unwrap();
	assert_eq!(sent.len(), 1);
	assert!(
		sent[0].contains("ALPHABETA"),
		"blocks of one plugin are joined with nothing between"
	);
	assert!(session
		.session
		.messages
		.iter()
		.all(|message| !message.content.contains("ALPHA") && message.name.as_deref() != Some("__active_memory_pack")));
	drop(sent);

	crate::plugins::end_session("o5-context");
	std::env::remove_var("OLLAMA_API_URL");
}

#[tokio::test]
async fn blocks_marked_keep_survive_the_ceiling_fallback() {
	let _guard = ENV_LOCK.lock().await;
	let (url, requests) = spawn_recording_stub(vec![final_response("Done")]).await;
	std::env::set_var("OLLAMA_API_URL", &url);
	let mut config = fake_provider_config();
	config.compression.threshold = 0;
	let optional = "OPTIONAL_CONTEXT ".repeat(4_000);
	let stub = StubPlugin::new(&[event::REQUEST_BEFORE, event::TURN_END]).respond(move |method, _| {
		Ok(if method == event::REQUEST_BEFORE {
			json!({"context": [
				{"name": "rules", "text": "KEEP_RULES", "keep": true},
				{"name": "extra", "text": optional},
			]})
		} else {
			json!({})
		})
	});
	let mut session = plugin_session("o5-keep");
	start_plugin_turn(&mut session, &stub, "fix the bug").await;
	config.max_session_tokens_threshold = session.get_full_context_tokens(&config).await + 256;

	run_turn(&mut session, &config).await.expect("turn succeeds");

	let sent = requests.lock().unwrap();
	assert_eq!(sent.len(), 1);
	assert!(sent[0].contains("KEEP_RULES"));
	assert!(!sent[0].contains("OPTIONAL_CONTEXT"));
	drop(sent);
	assert_eq!(session.active_memory_pack.as_deref(), Some("KEEP_RULES"));
	assert_eq!(stub.sent(event::TURN_END)[0]["dropped_context"], json!(["extra"]));

	crate::plugins::end_session("o5-keep");
	std::env::remove_var("OLLAMA_API_URL");
}

#[tokio::test]
async fn notes_land_before_the_request_and_at_turn_end() {
	let _guard = ENV_LOCK.lock().await;
	let (url, requests) = spawn_recording_stub(vec![final_response("ok")]).await;
	std::env::set_var("OLLAMA_API_URL", &url);
	let config = fake_provider_config();
	let stub = StubPlugin::new(&[event::REQUEST_BEFORE, event::TURN_END]).respond(|method, _| {
		Ok(if method == event::REQUEST_BEFORE {
			json!({"note": ["NOTE-BEFORE"]})
		} else {
			json!({"note": ["NOTE-AT-END"]})
		})
	});
	let mut session = plugin_session("o5-notes");
	start_plugin_turn(&mut session, &stub, "fix the bug").await;

	run_turn(&mut session, &config).await.expect("turn succeeds");

	let order: Vec<_> = session
		.session
		.messages
		.iter()
		.map(|message| {
			if message.content.contains("NOTE-BEFORE") {
				"before"
			} else if message.content.contains("NOTE-AT-END") {
				"end"
			} else {
				message.role.as_str()
			}
		})
		.collect();
	assert_eq!(order, ["user", "before", "assistant", "end"]);
	let sent = requests.lock().unwrap();
	assert!(sent[0].contains("NOTE-BEFORE"));
	assert!(!sent[0].contains("NOTE-AT-END"));
	drop(sent);

	crate::plugins::end_session("o5-notes");
	std::env::remove_var("OLLAMA_API_URL");
}

#[tokio::test]
async fn turn_end_reports_the_hard_stop_on_repeated_rounds() {
	let _guard = ENV_LOCK.lock().await;
	let (url, _requests) = spawn_recording_stub(vec![final_response("stopped")]).await;
	std::env::set_var("OLLAMA_API_URL", &url);
	let mut config = fake_provider_config();
	config.max_identical_rounds = 3;
	let stub = StubPlugin::new(&[event::TURN_END]);
	let mut session = plugin_session("o5-loop");
	start_plugin_turn(&mut session, &stub, "fix the bug").await;
	let parameters = json!({"path": "a.rs"});
	for _ in 0..3 {
		session
			.repeat_guard
			.note_round(&[crate::session::repeat_guard::RoundCall {
				tool: "view",
				parameters: &parameters,
				result: "same",
				is_error: false,
				is_mutation: false,
				free_form: false,
			}]);
	}

	run_turn(&mut session, &config).await.expect("turn succeeds");

	assert_eq!(stub.sent(event::TURN_END)[0]["stopped"], "loop");

	crate::plugins::end_session("o5-loop");
	std::env::remove_var("OLLAMA_API_URL");
}

#[tokio::test]
async fn nothing_is_sent_for_a_session_without_a_plugin() {
	let _guard = ENV_LOCK.lock().await;
	let (url, _requests) = spawn_recording_stub(vec![final_response("ok")]).await;
	std::env::set_var("OLLAMA_API_URL", &url);
	let config = fake_provider_config();
	// A plugin that is on for another session hears nothing of this one.
	let stub = StubPlugin::new(&event::ALL);
	connect_stub("o5-elsewhere", "assistant", std::path::Path::new("."), &stub).await;
	let mut session = plugin_session("o5-alone");
	session.add_user_message("fix the bug").unwrap();

	run_turn(&mut session, &config).await.expect("turn succeeds");

	assert!(events_heard(&stub).is_empty());
	assert!(session.active_memory_pack.is_none());

	crate::plugins::end_session("o5-elsewhere");
	std::env::remove_var("OLLAMA_API_URL");
}
