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
