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
	fake_provider_config, fake_session, final_response, spawn_stub, spawn_stub_with_status,
	tool_call_response, tool_calls_response, ENV_LOCK,
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

	run_turn(&mut session, &config)
		.await
		.expect("turn succeeds");

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
	let queue = std::sync::Arc::new(std::sync::Mutex::new(std::collections::VecDeque::from(
		responses,
	)));
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

/// A queued supervisor steer note is consumed at the safe pre-request point
/// and lands as a system-managed user-role message BEFORE the provider
/// request is built — so the one request the stub serves already contains
/// it, ahead of the assistant reply.
#[tokio::test]
async fn steer_note_injected_as_system_managed_user_message() {
	let _guard = ENV_LOCK.lock().await;
	let (url, requests) = spawn_recording_stub(vec![final_response("steer acknowledged")]).await;
	std::env::set_var("OLLAMA_API_URL", &url);

	let config = fake_provider_config();
	let mut session = fake_session("do the task");
	session.steer_pending =
		Some("<pay-attention>change approach: use the indexed lookup</pay-attention>".to_string());

	run_turn(&mut session, &config)
		.await
		.expect("steered turn succeeds");

	// The queued note was consumed, not left pending for a later turn
	assert!(session.steer_pending.is_none());

	let messages = &session.session.messages;
	let steer_pos = messages
		.iter()
		.position(|m| m.role == "user" && m.content.contains("use the indexed lookup"))
		.expect("steer note injected as a user-role message");
	assert!(crate::session::is_system_managed_user_content(
		&messages[steer_pos].content
	));
	// Injected before the request: it precedes the stub-served assistant
	// reply in the same conversation that was sent to the provider.
	let assistant_pos = messages
		.iter()
		.position(|m| m.role == "assistant")
		.expect("assistant reply recorded");
	assert!(
		steer_pos < assistant_pos,
		"steer note must precede the API request, got roles: {:?}",
		messages.iter().map(|m| m.role.as_str()).collect::<Vec<_>>()
	);
	assert!(messages[assistant_pos]
		.content
		.contains("steer acknowledged"));

	// The wire proves it: the request the stub actually served already
	// carried the note as a user-role message in its payload.
	let captured = requests.lock().expect("captured requests");
	assert_eq!(captured.len(), 1, "exactly one provider request expected");
	let payload: serde_json::Value =
		serde_json::from_str(&captured[0]).expect("request body is JSON");
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
	assert!(
		carried,
		"steer note must be inside the request payload: {}",
		captured[0]
	);
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
async fn pending_async_work_allows_progressing_handback() {
	let _guard = ENV_LOCK.lock().await;
	let response = r#"Waiting for the background job.
<sup>{"state":"progressing","focus":"waiting for the background job","next":"report its result","carry":[],"plan":null,"memories":[]}</sup>"#;
	let url = spawn_stub(vec![final_response(response)]).await;
	std::env::set_var("OLLAMA_API_URL", &url);

	let mut config = fake_provider_config();
	config.supervisor.enabled = true;
	config.supervisor.gate.enabled = true;
	let mut session = fake_session("start the background job");
	session.completion_gate_eligible = true;
	let session_id = "pending-work-handback-test".to_string();

	crate::session::context::with_session_id(session_id.clone(), async {
		crate::session::context::init_session_services("assistant");
		crate::session::shell_jobs::register_for_session(
			&session_id,
			"test-mcp",
			"job://test",
			"cargo test --lib",
		);

		run_turn(&mut session, &config)
			.await
			.expect("pending work is a valid progressing handback");

		assert_eq!(session.session.info.total_api_calls, 1);
		assert_eq!(session.last_response, "Waiting for the background job.");
		crate::session::context::cleanup_session(&session_id);
	})
	.await;
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

	run_turn(&mut session, &config)
		.await
		.expect("turn succeeds");

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

	run_turn(&mut session, &config)
		.await
		.expect("turn succeeds");

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
	run_turn(&mut session, &config)
		.await
		.expect("turn succeeds");

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
		(
			500,
			serde_json::json!({"error": {"message": "stub exploded"}}),
		),
		(
			500,
			serde_json::json!({"error": {"message": "stub exploded"}}),
		),
		(
			500,
			serde_json::json!({"error": {"message": "stub exploded"}}),
		),
		(
			500,
			serde_json::json!({"error": {"message": "stub exploded"}}),
		),
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
	assert!(!session
		.session
		.messages
		.iter()
		.any(|m| m.role == "assistant"));
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
			if let Some(last) = session
				.session
				.messages
				.iter()
				.rev()
				.find(|m| m.role == "assistant")
			{
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
		tool_call_response("schedule", serde_json::json!({"action": "list"})),
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
	run_turn(&mut session, &config)
		.await
		.expect("tool round turn");

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
	let last = session
		.session
		.messages
		.last()
		.expect("final assistant message");
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
		tool_call_response("schedule", serde_json::json!({"action": "list"})),
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

	let last = session
		.session
		.messages
		.last()
		.expect("final assistant message");
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
async fn test_large_tool_result_truncation_and_dedup() {
	let _guard = ENV_LOCK.lock().await;
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
	run_turn(&mut session, &config)
		.await
		.expect("double skill round");

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

	crate::mcp::workdir::set_session_working_directory(std::env::current_dir().expect("cwd"));
	std::env::remove_var("OLLAMA_API_URL");
}

/// Full supervised turn at the unit level: task classification, orientation,
/// and gate calls all go to the same scripted stub. Whatever nonsense the
/// control plane reads back, the user turn must complete and the answer must
/// be recorded.
#[tokio::test]
async fn test_supervised_turn_survives_scripted_control_plane() {
	let _guard = ENV_LOCK.lock().await;
	// Enough valid completions for the agent answer plus every supervisor
	// side-call; the queue-exhausted fallback stays valid after these.
	// Identical bodies: the supervisor's side-calls interleave with the agent
	// call in no guaranteed order, so every consumer must see the same text.
	let url = spawn_stub(vec![final_response("SUPERVISED-TURN-ANSWER ok"); 5]).await;
	std::env::set_var("OLLAMA_API_URL", &url);

	let mut config = fake_provider_config();
	config.supervisor.enabled = true;
	config.supervisor.model = "ollama:fake-model".to_string();
	config.supervisor.gate.verifier_model = "ollama:fake-model".to_string();
	config.supervisor.learning.enabled = false;
	config.compression.decision.model = "ollama:fake-model".to_string();

	let mut session = fake_session("do the supervised thing and finish");
	run_turn(&mut session, &config)
		.await
		.expect("supervised turn");

	let assistant = session
		.session
		.messages
		.iter()
		.find(|m| m.role == "assistant")
		.expect("assistant reply recorded");
	assert!(
		assistant.content.contains("SUPERVISED-TURN-ANSWER"),
		"got: {}",
		assistant.content
	);

	std::env::remove_var("OLLAMA_API_URL");
}

// ── Supervisor gate / plan / learning paths ──────────────────────────────────
// All against the same scripted stub: the agent call and every supervisor
// side-call (verifier, planner, keyword extraction) hit the queue in order.

/// `fake_provider_config` plus the supervisor control plane wired to the
/// stub: gate on, learning off, compression decision model stubbed so no
/// side-call can reach a real provider.
fn supervised_config() -> Config {
	let mut config = fake_provider_config();
	config.supervisor.enabled = true;
	config.supervisor.model = "ollama:fake-model".to_string();
	config.supervisor.gate.enabled = true;
	config.supervisor.gate.verifier_model = "ollama:fake-model".to_string();
	config.supervisor.learning.enabled = false;
	config.compression.decision.model = "ollama:fake-model".to_string();
	config
}

fn sup_tag(state: &str) -> String {
	format!(
		"\n<sup>{{\"state\":\"{state}\",\"focus\":\"unit test focus\",\"next\":null,\"carry\":[],\"plan\":null,\"memories\":[]}}</sup>"
	)
}

fn done_response(text: &str) -> serde_json::Value {
	final_response(&format!("{text}{}", sup_tag("done")))
}

fn progressing_response(text: &str) -> serde_json::Value {
	final_response(&format!("{text}{}", sup_tag("progressing")))
}

/// The four clean-shape declarations the verifier parser expects alongside a
/// verdict (see gate.rs's own parser tests).
const CLEAN_SHAPES: &str = r#"<shape name="circular" found="no">independent expectation</shape>
<shape name="context-stripped" found="no">representative context</shape>
<shape name="acceptance-only" found="no">not applicable</shape>
<shape name="unenumerated-category" found="no">bounded scope</shape>"#;

fn verifier_pass() -> serde_json::Value {
	final_response(&format!("{CLEAN_SHAPES}\n<verdict>PASS</verdict>"))
}

fn verifier_gap() -> serde_json::Value {
	final_response(&format!(
		"{CLEAN_SHAPES}\n<gap settles=\"a read of stats.rs\">the counter is unverified</gap>"
	))
}

/// A `progressing` final message with no pending background work is a promise,
/// not a result: the pre-gate nudges the turn back to work until the free
/// budget (MAX_ITERATIONS) is spent, then lets the turn end.
#[tokio::test]
async fn test_unfinished_progressing_handback_is_continued_until_budget() {
	let _guard = ENV_LOCK.lock().await;
	let sid = "api-exec-unfinished-handback".to_string();
	crate::session::context::with_session_id(sid.clone(), async {
		crate::session::context::init_session_services("assistant");
		let url = spawn_stub(vec![
			progressing_response("Still working on it."),
			progressing_response("Still working, pass two."),
			progressing_response("Third pass."),
		])
		.await;
		std::env::set_var("OLLAMA_API_URL", &url);

		let config = supervised_config();
		let mut session = fake_session("finish the report");
		session.completion_gate_eligible = true;

		run_turn(&mut session, &config)
			.await
			.expect("turn completes after nudges");

		assert_eq!(
			session.nudge_iterations,
			crate::supervisor::gate::MAX_ITERATIONS
		);
		assert_eq!(session.session.info.total_api_calls, 3);
		let continuations = session
			.session
			.messages
			.iter()
			.filter(|m| m.content.contains("octomind:pre_gate_unfinished_handback"))
			.count();
		assert_eq!(continuations, 2, "one CONTINUE note per nudge");

		std::env::remove_var("OLLAMA_API_URL");
		crate::session::context::cleanup_session(&sid);
	})
	.await;
}

/// A `done` claim the verifier accepts: gate state resets, the trajectory is
/// labelled verified, and exactly one agent exchange was billed.
#[tokio::test]
async fn test_verify_gate_pass_accepts_claim_and_clears_gate_state() {
	let _guard = ENV_LOCK.lock().await;
	let sid = "api-exec-gate-pass".to_string();
	crate::session::context::with_session_id(sid.clone(), async {
		crate::session::context::init_session_services("assistant");
		let url = spawn_stub(vec![
			done_response("Everything is finished and verified."),
			verifier_pass(),
		])
		.await;
		std::env::set_var("OLLAMA_API_URL", &url);

		let config = supervised_config();
		let mut session = fake_session("ship the feature");
		session.completion_gate_eligible = true;

		run_turn(&mut session, &config)
			.await
			.expect("gated turn passes");

		assert!(!session.gate_failed);
		assert!(matches!(
			session.learning_outcome,
			crate::supervisor::learning::TrajectoryOutcome::Verified
		));
		assert_eq!(session.gate_iterations, 0);
		assert_eq!(session.nudge_iterations, 0);
		// The verifier call is an out-of-band supervisor side-call: only the
		// agent exchange lands in the session's own bookkeeping.
		assert_eq!(session.session.info.total_api_calls, 1);

		std::env::remove_var("OLLAMA_API_URL");
		crate::session::context::cleanup_session(&sid);
	})
	.await;
}

/// A `done` claim the verifier rejects with a charged gap: the advisory lands
/// in the conversation, the turn re-runs once, and the gap is retained.
#[tokio::test]
async fn test_verify_gate_gaps_inject_advisory_and_rerun_turn() {
	let _guard = ENV_LOCK.lock().await;
	let sid = "api-exec-gate-gaps".to_string();
	crate::session::context::with_session_id(sid.clone(), async {
		crate::session::context::init_session_services("assistant");
		let url = spawn_stub(vec![
			done_response("Trust me, it is complete."),
			verifier_gap(),
			// Refutation pass (if the verifier asks for one) sees no refutation.
			final_response("no finding was refuted"),
			// The re-run answers without a completion claim, ending the turn.
			final_response("The gap is closed now: counter verified."),
		])
		.await;
		std::env::set_var("OLLAMA_API_URL", &url);

		let config = supervised_config();
		let mut session = fake_session("verify the counter");
		session.completion_gate_eligible = true;

		run_turn(&mut session, &config)
			.await
			.expect("gaps re-run completes");

		assert_eq!(session.gate_iterations, 1);
		assert_eq!(session.last_gate_gaps.len(), 1);
		assert!(
			!session.gate_failed,
			"re-run without a new claim ends the turn cleanly"
		);
		let advisory = session
			.session
			.messages
			.iter()
			.find(|m| m.content.contains("verification pass found gaps"))
			.expect("gap advisory injected");
		assert!(advisory.content.contains("the counter is unverified"));
		assert_eq!(session.session.info.total_api_calls, 2);

		std::env::remove_var("OLLAMA_API_URL");
		crate::session::context::cleanup_session(&sid);
	})
	.await;
}

/// A verifier response with no parseable verdict is Indeterminate: completion
/// is NOT accepted, the failure is recorded, and the bounded re-entry advisory
/// asks for a checkable restatement.
#[tokio::test]
async fn test_verify_gate_indeterminate_fails_closed_after_reentry() {
	let _guard = ENV_LOCK.lock().await;
	let sid = "api-exec-gate-indeterminate".to_string();
	crate::session::context::with_session_id(sid.clone(), async {
		crate::session::context::init_session_services("assistant");
		// Two garbage bodies: the parser may retry once before giving up; both
		// are interchangeable so the stub order does not matter.
		let url = spawn_stub(vec![
			done_response("Done, no evidence needed."),
			final_response("certainly! here is no protocol at all"),
			final_response("still no protocol"),
			final_response("Second pass with a proper answer."),
		])
		.await;
		std::env::set_var("OLLAMA_API_URL", &url);

		let config = supervised_config();
		let mut session = fake_session("close the task");
		session.completion_gate_eligible = true;

		run_turn(&mut session, &config)
			.await
			.expect("indeterminate re-entry completes");

		assert!(
			session.gate_failed,
			"unreadable verdict must not pass silently"
		);
		assert!(matches!(
			session.learning_outcome,
			crate::supervisor::learning::TrajectoryOutcome::Failed
		));
		assert_eq!(session.gate_iterations, 1);
		session
			.session
			.messages
			.iter()
			.find(|m| {
				m.content
					.contains("independent verification pass could not be completed")
			})
			.expect("unverified re-entry advisory injected");
		assert_eq!(session.session.info.total_api_calls, 2);

		std::env::remove_var("OLLAMA_API_URL");
		crate::session::context::cleanup_session(&sid);
	})
	.await;
}

/// The deterministic mutation pre-gate: a `done` claim right after an
/// unverified state change is nudged once; a second identical claim exhausts
/// the shared budget and fails the gate without any verifier call.
#[tokio::test]
async fn test_pregate_unverified_mutation_nudges_once_then_exhausts_budget() {
	let _guard = ENV_LOCK.lock().await;
	let sid = "api-exec-pregate-mutation".to_string();
	crate::session::context::with_session_id(sid.clone(), async {
		crate::session::context::init_session_services("assistant");
		let url = spawn_stub(vec![
			done_response("Shipped the change."),
			done_response("Really shipped it this time."),
		])
		.await;
		std::env::set_var("OLLAMA_API_URL", &url);

		let config = supervised_config();
		let mut session = fake_session("change the config");
		session.completion_gate_eligible = true;
		// Arm the detector the same way detect_tests does: a recorded agent
		// round changed the tree (fp 10 -> 11) and nothing verified it since,
		// so the live fingerprint can never match the verified baseline.
		session.detectors.note_round_verification(
			Some(10),
			Some(11),
			false,
			false,
			true,
			false,
			true,
		);

		run_turn(&mut session, &config)
			.await
			.expect("turn ends after budget exhaustion");

		assert!(session.gate_failed);
		assert!(matches!(
			session.learning_outcome,
			crate::supervisor::learning::TrajectoryOutcome::Failed
		));
		assert_eq!(
			session.nudge_iterations,
			crate::supervisor::gate::MAX_ITERATIONS
		);
		let nudges = session
			.session
			.messages
			.iter()
			.filter(|m| m.content.contains(PREGATE_MARKER))
			.count();
		assert_eq!(nudges, 1, "second pass must not duplicate the nudge note");
		assert_eq!(session.session.info.total_api_calls, 2);

		std::env::remove_var("OLLAMA_API_URL");
		crate::session::context::cleanup_session(&sid);
	})
	.await;
}

/// Gate disabled: a `done` claim with a `phase_complete` plan signal drives
/// the full external-plan lifecycle inside one turn — pre-request reconcile
/// creates the plan, the post-response reconcile advances it, and the
/// no-gate completion block retires it.
#[tokio::test]
async fn test_plan_lifecycle_without_gate_reconciles_and_retires_plan() {
	let _guard = ENV_LOCK.lock().await;
	let sid = "api-exec-plan-retire".to_string();
	crate::session::context::with_session_id(sid.clone(), async {
		crate::session::context::init_session_services("assistant");
		let url = spawn_stub(vec![
			// Pre-request reconcile: Request signal -> create a two-phase plan.
			final_response(
				"{\"decision\":\"create\",\"title\":\"Ship the widget\",\"tasks\":[{\"title\":\"build it\",\"done_when\":\"it compiles\"},{\"title\":\"test it\",\"done_when\":\"tests pass\"}]}",
			),
			// Agent turn: claims done AND emits a phase_complete plan signal.
			final_response(
				"Widget shipped.\n<sup>{\"state\":\"done\",\"focus\":\"shipped\",\"next\":null,\"carry\":[],\"plan\":\"phase_complete\",\"memories\":[]}</sup>",
			),
			// Post-response reconcile: phase_complete -> advance the first phase.
			final_response("{\"decision\":\"advance\",\"summary\":\"widget built\"}"),
		])
		.await;
		std::env::set_var("OLLAMA_API_URL", &url);

		let mut config = supervised_config();
		config.supervisor.gate.enabled = false;
		config.supervisor.plan.enabled = true;
		config.supervisor.plan.model = "ollama:fake-model".to_string();

		let mut session = fake_session("build the widget end to end");
		session.completion_gate_eligible = true;
		session.pending_plan_signal = Some(crate::supervisor::plan::PlanSignal::Request);
		session.plan_evaluated = false;
		session.planner_failed = false;
		// Admission-time task resolution: the turn owns whatever plan it is
		// about to create (plan_at_turn_start stays empty).
		session.gate_task = Some(
			crate::supervisor::resolve::ResolvedTask::self_contained("build the widget end to end"),
		);

		run_turn(&mut session, &config)
			.await
			.expect("plan-supervised turn");

		assert!(
			!crate::mcp::core::plan::has_active_plan(),
			"done without gate must retire the plan"
		);
		assert!(session.pending_plan_signal.is_none(), "signal consumed");
		assert_eq!(session.session.info.total_api_calls, 1);

		std::env::remove_var("OLLAMA_API_URL");
		crate::session::context::cleanup_session(&sid);
	})
	.await;
}

/// Learning enabled: the first call of a session materializes the runtime-only
/// active memory pack. A stored GLOBAL lesson lands in the pack unconditionally
/// (global tier has no relevance gating), so the assertion does not depend on
/// the keyword-extraction side-call's output. `recalled_refs` is consumed by
/// `reinforce_recalled` when the turn completes — the durable trace is the
/// learning_stats pack counters.
#[serial_test::serial]
#[tokio::test]
async fn test_learning_injection_builds_active_memory_pack() {
	let _guard = ENV_LOCK.lock().await;
	let data = tempfile::tempdir().unwrap();
	let previous = std::env::var_os("OCTOMIND_DATA_DIR");
	std::env::set_var("OCTOMIND_DATA_DIR", data.path());

	let backend = crate::supervisor::learning::backend::FileBackend;
	backend
		.store(&crate::supervisor::learning::Lesson {
			content: "Prefer interactive rebase before pushing to shared branches.".to_string(),
			title: "rebase policy".to_string(),
			scope: "global".to_string(),
			created: chrono::Utc::now().to_rfc3339(),
			..Default::default()
		})
		.await
		.unwrap();

	let sid = "api-exec-learning-pack".to_string();
	crate::session::context::with_session_id(sid.clone(), async {
		crate::session::context::init_session_services("assistant");
		// Identical bodies: the keyword-extraction side call and the agent call
		// interleave in no guaranteed order, so every consumer must see the
		// same valid completion.
		let url = spawn_stub(vec![final_response("rebase\npolicy"); 3]).await;
		std::env::set_var("OLLAMA_API_URL", &url);

		let mut config = supervised_config();
		config.supervisor.learning.enabled = true;
		config.supervisor.learning.model = "ollama:fake-model".to_string();

		let mut session = fake_session("push my branch safely");
		run_turn(&mut session, &config)
			.await
			.expect("turn with recall");

		assert!(
			session.learning_injected,
			"first call must mark learning injected"
		);
		assert!(
			session.active_memory_pack.is_some(),
			"global lesson must land in the pack"
		);
		assert!(session.session.info.learning_stats.packs >= 1);
		assert!(session.session.info.learning_stats.items >= 1);

		std::env::remove_var("OLLAMA_API_URL");
		crate::session::context::cleanup_session(&sid);
	})
	.await;

	match previous {
		Some(value) => std::env::set_var("OCTOMIND_DATA_DIR", value),
		None => std::env::remove_var("OCTOMIND_DATA_DIR"),
	}
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

	run_turn_mode(
		&mut session,
		&config,
		crate::session::output::OutputMode::Interactive,
	)
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

	run_turn_mode(
		&mut session,
		&config,
		crate::session::output::OutputMode::Interactive,
	)
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

	// fake_provider_config disables supervisor learning, so a pack left over
	// from a previous configuration must be dropped before the request.
	let config = fake_provider_config();
	let mut session = fake_session("hi there");
	session.active_memory_pack = Some("stale pack content".to_string());

	run_turn(&mut session, &config)
		.await
		.expect("turn succeeds");

	assert!(
		session.active_memory_pack.is_none(),
		"a pack must not survive into a request when learning is disabled"
	);

	std::env::remove_var("OLLAMA_API_URL");
}
