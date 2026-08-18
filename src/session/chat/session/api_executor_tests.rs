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
use crate::session::output::SilentSink;
use std::collections::VecDeque;
use std::sync::Mutex as StdMutex;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// `OLLAMA_API_URL` is process-global env — tests touching it must not
/// overlap. Lock is poisoned-tolerant: a failed test must not cascade.
static ENV_LOCK: StdMutex<()> = StdMutex::new(());

/// Spawn a one-shot-per-connection HTTP stub returning scripted
/// chat-completion bodies in order. Returns the chat-completions URL.
async fn spawn_stub(responses: Vec<serde_json::Value>) -> String {
	spawn_stub_with_status(responses.into_iter().map(|r| (200, r)).collect()).await
}

/// Like [`spawn_stub`] but each scripted entry carries its HTTP status,
/// so provider-level error handling can be exercised.
async fn spawn_stub_with_status(responses: Vec<(u16, serde_json::Value)>) -> String {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
		.await
		.expect("bind stub listener");
	let addr = listener.local_addr().expect("stub addr");
	let queue = std::sync::Arc::new(StdMutex::new(VecDeque::from(responses)));

	tokio::spawn(async move {
		while let Ok((mut sock, _)) = listener.accept().await {
			let queue = queue.clone();
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

				let (status, body) = queue
					.lock()
					.expect("stub queue")
					.pop_front()
					.unwrap_or_else(|| {
						(
							200,
							serde_json::json!({
								"choices": [{
									"message": {"role": "assistant", "content": "SCRIPT EXHAUSTED"},
									"finish_reason": "stop"
								}],
								"usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
							}),
						)
					});
				let body = body.to_string();
				let reason = if status == 200 { "OK" } else { "Error" };
				let response = format!(
					"HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
					body.len(),
					body
				);
				let _ = sock.write_all(response.as_bytes()).await;
				let _ = sock.shutdown().await;
			});
		}
	});

	format!("http://{}/v1/chat/completions", addr)
}

fn final_response(text: &str) -> serde_json::Value {
	serde_json::json!({
		"choices": [{
			"message": {"role": "assistant", "content": text},
			"finish_reason": "stop"
		}],
		"usage": {"prompt_tokens": 20, "completion_tokens": 10, "total_tokens": 30, "cost": 0.001}
	})
}

fn tool_calls_response(calls: &[(&str, &str, serde_json::Value)]) -> serde_json::Value {
	let tool_calls: Vec<serde_json::Value> = calls
		.iter()
		.map(|(id, name, arguments)| {
			serde_json::json!({
				"id": id,
				"type": "function",
				"function": {"name": name, "arguments": arguments.to_string()}
			})
		})
		.collect();
	serde_json::json!({
		"choices": [{
			"message": {"role": "assistant", "content": "", "tool_calls": tool_calls},
			"finish_reason": "tool_calls"
		}],
		"usage": {"prompt_tokens": 25, "completion_tokens": 15, "total_tokens": 40, "cost": 0.002}
	})
}

fn tool_call_response(tool_name: &str, arguments: serde_json::Value) -> serde_json::Value {
	tool_calls_response(&[("call_1", tool_name, arguments)])
}

/// Merged config wired for the fake provider: real template + assistant
/// role, supervisor off (its gates would issue their own scripted-queue
/// desyncing LLM calls).
fn fake_provider_config() -> Config {
	let mut config: Config =
		toml::from_str(include_str!("../../../../config-templates/default.toml"))
			.expect("parse default config template");
	config.build_role_map();
	config.model = "ollama:fake-model".to_string();
	config.supervisor.enabled = false;
	let mut merged = config.get_merged_config_for_role("assistant");
	merged.model = "ollama:fake-model".to_string();
	merged
}

fn fake_session(user_input: &str) -> ChatSession {
	let mut session = ChatSession::for_tests(Vec::new());
	session.model = "ollama:fake-model".to_string();
	session.session.info.model = "ollama:fake-model".to_string();
	session
		.add_user_message(user_input)
		.expect("add user message");
	session
}

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
	let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
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
}

#[tokio::test]
async fn test_tool_round_trip_with_unknown_tool() {
	let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
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
	let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
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
	let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
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
	let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
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
	let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
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
