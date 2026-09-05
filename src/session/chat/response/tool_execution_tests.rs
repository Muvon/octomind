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

fn tap_call(action: &str) -> crate::mcp::McpToolCall {
	crate::mcp::McpToolCall {
		tool_name: "tap".to_string(),
		parameters: serde_json::json!({ "action": action }),
		tool_id: "id".to_string(),
	}
}

#[test]
fn test_is_tap_capability_call() {
	assert!(is_tap_capability_call(&tap_call("capability")));
	assert!(!is_tap_capability_call(&tap_call("run")));

	let other = crate::mcp::McpToolCall {
		tool_name: "shell".to_string(),
		parameters: serde_json::json!({ "action": "capability" }),
		tool_id: "id".to_string(),
	};
	assert!(!is_tap_capability_call(&other));
}

#[test]
fn test_error_messages() {
	let loop_msg = loop_error_message("shell", 3, "exit code 1");
	assert!(loop_msg.contains("LOOP DETECTED"));
	assert!(loop_msg.contains("'shell'"));
	assert!(loop_msg.contains("3 consecutive"));
	assert!(loop_msg.contains("exit code 1"));

	let attempt_msg = attempt_error_message(2, 3, "no such file");
	assert!(attempt_msg.contains("attempt 2/3"));
	assert!(attempt_msg.contains("no such file"));
}

/// The invariant handle_large_tool_results must hold: truncation may replace
/// the body but must NEVER flip is_error() — a truncated error entering the
/// dedup cache as "success" would get elided exactly when the model needs
/// the error text most.
#[tokio::test]
async fn test_truncation_preserves_error_flag() {
	let mut config: Config =
		toml::from_str(include_str!("../../../../config-templates/default.toml"))
			.expect("parse default config template");
	config.mcp_response_tokens_threshold = 10;

	let big = "line of output\n".repeat(500);
	let results = vec![
		crate::mcp::McpToolResult::success("shell".to_string(), "id1".to_string(), big.clone()),
		crate::mcp::McpToolResult::error("shell".to_string(), "id2".to_string(), big.clone()),
	];

	let processed = handle_large_tool_results(results, &config, OutputMode::NonInteractive)
		.await
		.expect("truncation never fails");

	assert_eq!(processed.len(), 2);
	// Both bodies were truncated below the original size
	assert!(processed[0].extract_content().len() < big.len());
	assert!(processed[1].extract_content().len() < big.len());
	// The error flag survives truncation
	assert!(!processed[0].is_error());
	assert!(processed[1].is_error());
}

/// Rich (non-plain-text) results pass through untouched — flattening them
/// would discard resource/image/structured-content semantics.
#[tokio::test]
async fn test_rich_results_bypass_truncation() {
	let mut config: Config =
		toml::from_str(include_str!("../../../../config-templates/default.toml"))
			.expect("parse default config template");
	config.mcp_response_tokens_threshold = 10;

	let big = "line of output\n".repeat(500);
	let rich = crate::mcp::McpToolResult::success_with_metadata(
		"tool".to_string(),
		"id".to_string(),
		big.clone(),
		serde_json::json!({"k": "v"}),
	);

	let processed = handle_large_tool_results(vec![rich], &config, OutputMode::NonInteractive)
		.await
		.expect("passthrough never fails");
	assert!(processed[0].extract_content().contains(&big[..100]));
}

fn template_config() -> Config {
	toml::from_str(include_str!("../../../../config-templates/default.toml"))
		.expect("parse default config template")
}

#[tokio::test]
async fn authorizer_covers_inline_capability_calls_before_activation() {
	let mut config = template_config();
	config.supervisor.authorizer.enabled = true;
	let mut session = ChatSession::for_tests(Vec::new());
	session.session.info.name = "authorizer-inline-test".into();
	let mut processor = ToolProcessor::new();
	let (_tx, rx) = tokio::sync::watch::channel(false);
	let mut call = tap_call("capability");
	call.parameters["prompt"] = serde_json::json!("activate shell");
	let (results, _) = execute_tools_parallel(
		vec![call],
		"",
		&mut session,
		&config,
		&mut processor,
		rx,
		OutputMode::Jsonl,
	)
	.await
	.unwrap();
	assert!(results[0].is_error());
	assert!(results[0].extract_content().contains("[authorizer]"));
	assert!(crate::session::guardrails::get_call_log(&session.session.info.name).is_empty());
	crate::session::context::cleanup_session(&session.session.info.name);
}

#[tokio::test]
#[serial_test::serial]
#[cfg(unix)]
async fn authorizer_mixed_batch_executes_only_allowed_tool_and_skips_denied_hook() {
	use crate::session::chat::test_support::{
		fake_provider_config, final_response, spawn_stub, ENV_LOCK,
	};
	let _guard = ENV_LOCK.lock().await;
	let tmp = tempfile::tempdir().unwrap();
	write_local_tool(
		tmp.path(),
		"allowed",
		"#!/bin/sh\n# @description Read requested data.\necho permitted\n",
	);
	write_local_tool(
		tmp.path(),
		"denied",
		"#!/bin/sh\n# @description Unrequested file write.\ntouch forbidden-marker\n",
	);
	write_local_tool(
		tmp.path(),
		"blocked-hook",
		"#!/bin/sh\n# @description Test hook.\ntouch hook-marker\n",
	);
	std::fs::write(
		tmp.path().join(".agents/guardrails.toml"),
		"[[hook]]\nresult = 'authorizer'\nscript = '.agents/tools/blocked-hook'\n",
	)
	.unwrap();
	let id = "authorizer-execution".to_string();
	crate::session::context::with_session_id(id.clone(), async {
		crate::mcp::workdir::set_session_working_directory(tmp.path().to_path_buf());
		crate::session::context::set_session_workdir(&id, tmp.path().to_path_buf());
		crate::session::context::init_session_services("assistant");
		let url = spawn_stub(vec![final_response(r#"{"decisions":[{"id":"0","decision":"allow","reason":"Read requested data","user_source":"","user_quote":"","overridden_guards":[]},{"id":"1","decision":"block","reason":"Unrequested write","user_source":"u1","user_quote":"Do not modify files","overridden_guards":[]}]}"#)]).await;
		std::env::set_var("OLLAMA_API_URL", &url);
		let mut config = fake_provider_config();
		config.supervisor.enabled = true;
		config.supervisor.authorizer.enabled = true;
		config.supervisor.condense.enabled = false;
		config.supervisor.model.model = Some("ollama:fake-model".into());
		let mut session = ChatSession::for_tests(vec![crate::session::Message {role:"user".into(),content:"Read data. Do not modify files".into(),id:Some("u1".into()),..Default::default()}]);
		session.session.info.name = id.clone();
		let mut processor = ToolProcessor::new();
		let mut context = main_session_context(&mut session, &mut processor);
		let calls = ["allowed", "denied"].iter().map(|name| crate::mcp::McpToolCall {tool_name:name.to_string(),tool_id:name.to_string(),parameters:serde_json::json!({})}).collect();
		let (_tx,rx) = tokio::sync::watch::channel(false);
		let (results, _) = execute_tools_in_context(calls, &mut context, &config, Some(rx), OutputMode::Jsonl).await.unwrap();
		assert!(!results[0].is_error(), "{}",results[0].extract_content());
		assert!(results[0].extract_content().contains("permitted"));
		assert!(results[1].is_error());
		assert_eq!(results[1].tool_id,"denied");
		assert!(results[1].extract_content().contains("[authorizer]"));
		assert!(!tmp.path().join("forbidden-marker").exists());
		assert!(!tmp.path().join("hook-marker").exists());
		assert_eq!(crate::session::guardrails::get_call_log(&id).len(),1);
		crate::session::context::cleanup_session(&id);
		std::env::remove_var("OLLAMA_API_URL");
	}).await;
}

fn message(role: &str, content: &str) -> crate::session::Message {
	crate::session::Message {
		role: role.to_string(),
		content: content.to_string(),
		..Default::default()
	}
}

#[test]
fn test_context_accessors_main_session_and_layer() {
	let mut session = ChatSession::for_tests(Vec::new());
	let mut processor = ToolProcessor::new();
	let mut context = ToolExecutionContext::MainSession {
		chat_session: &mut session,
		tool_processor: &mut processor,
		tool_round_intent: "",
	};

	assert_eq!(context.session_name(), "test");
	assert!(context.execution_context().is_none());
	assert!(context.is_tool_allowed("anything"));
	assert!(context.error_tracker().is_some());

	let mut layer = ToolExecutionContext::Layer {
		session_name: "layer-sess".to_string(),
		layer_name: "reviewer".to_string(),
	};
	assert_eq!(layer.session_name(), "layer-sess");
	assert_eq!(layer.execution_context().as_deref(), Some("reviewer"));
	assert!(layer.is_tool_allowed("anything"));
	assert!(layer.error_tracker().is_none());
}

#[test]
fn test_increment_tool_calls_counts_main_session_only() {
	let mut session = ChatSession::for_tests(Vec::new());
	assert_eq!(session.session.info.tool_calls, 0);
	let mut processor = ToolProcessor::new();
	let mut context = ToolExecutionContext::MainSession {
		chat_session: &mut session,
		tool_processor: &mut processor,
		tool_round_intent: "",
	};

	context.increment_tool_calls("view");
	context.increment_tool_calls("shell");
	if let ToolExecutionContext::MainSession { chat_session, .. } = &context {
		assert_eq!(chat_session.session.info.tool_calls, 2);
	} else {
		panic!("expected MainSession context");
	}

	// Layer context has no session — the call is a telemetry-only no-op
	let mut layer = ToolExecutionContext::Layer {
		session_name: "s".to_string(),
		layer_name: "l".to_string(),
	};
	layer.increment_tool_calls("view");
}

#[tokio::test]
async fn test_execute_tools_in_context_empty_batch_returns_empty() {
	let config = template_config();
	let mut context = ToolExecutionContext::Layer {
		session_name: "layer-sess".to_string(),
		layer_name: "reviewer".to_string(),
	};

	let (results, total_ms) =
		execute_tools_in_context(Vec::new(), &mut context, &config, None, OutputMode::Jsonl)
			.await
			.expect("empty batch");
	assert!(results.is_empty());
	assert_eq!(total_ms, 0);
}

#[tokio::test]
async fn test_execute_tools_in_context_unknown_tool_layer_error_result() {
	let config = template_config();
	let mut context = ToolExecutionContext::Layer {
		session_name: "layer-sess".to_string(),
		layer_name: "reviewer".to_string(),
	};

	let calls = vec![
		crate::mcp::McpToolCall {
			tool_name: "definitely_missing_tool_xyz".to_string(),
			parameters: serde_json::json!({"q": 1}),
			tool_id: "tid1".to_string(),
		},
		crate::mcp::McpToolCall {
			tool_name: "also_missing_tool_abc".to_string(),
			parameters: serde_json::json!({}),
			tool_id: "tid2".to_string(),
		},
	];

	let (results, _total_ms) =
		execute_tools_in_context(calls, &mut context, &config, None, OutputMode::Jsonl)
			.await
			.expect("unknown tools produce error results, not Err");

	// Parallel batch preserves order and identity
	assert_eq!(results.len(), 2);
	assert_eq!(results[0].tool_name, "definitely_missing_tool_xyz");
	assert_eq!(results[0].tool_id, "tid1");
	assert_eq!(results[1].tool_name, "also_missing_tool_abc");
	assert_eq!(results[1].tool_id, "tid2");
	for result in &results {
		assert!(result.is_error());
		assert!(result.extract_content().contains("not found"));
	}
}

#[tokio::test]
async fn test_execute_tools_in_context_error_tracker_attempts_and_loop() {
	let config = template_config();
	let mut session = ChatSession::for_tests(Vec::new());
	let mut processor = ToolProcessor::new();
	let mut context = ToolExecutionContext::MainSession {
		chat_session: &mut session,
		tool_processor: &mut processor,
		tool_round_intent: "",
	};

	for attempt in 1..=3 {
		let call = crate::mcp::McpToolCall {
			tool_name: "missing_tool_abc".to_string(),
			parameters: serde_json::json!({"attempt": attempt}),
			tool_id: format!("id{attempt}"),
		};
		let (results, _) =
			execute_tools_in_context(vec![call], &mut context, &config, None, OutputMode::Jsonl)
				.await
				.expect("error results, not Err");
		assert_eq!(results.len(), 1);
		let content = results[0].extract_content();
		assert!(results[0].is_error(), "{content}");
		if attempt < 3 {
			assert!(
				content.contains(&format!("attempt {attempt}/3")),
				"{content}"
			);
		} else {
			assert!(content.contains("LOOP DETECTED"), "{content}");
		}
	}
}

#[test]
fn test_parent_task_context_goal_and_request() {
	let mut session = ChatSession::for_tests(vec![message("user", "fix the login bug")]);
	session.session.info.anchor.intent = "Ship the refactor".to_string();

	let task = parent_task_context(&session);
	assert!(task.contains("Goal: Ship the refactor"), "{task}");
	assert!(
		task.contains("Current request: fix the login bug"),
		"{task}"
	);

	// No anchor and no real user turn: neither segment appears
	let bare = ChatSession::for_tests(Vec::new());
	let empty = parent_task_context(&bare);
	assert!(!empty.contains("Goal:"), "{empty}");
	assert!(!empty.contains("Current request:"), "{empty}");
}

#[test]
fn test_parent_agent_context_filters_trusted_messages() {
	let messages = vec![
		message("system", "  sys prompt  "),
		message("user", "<instructions>\nbe careful\n</instructions>"),
		message("user", "do the thing"),
		message("assistant", "working on it"),
		message("user", "<skill name=\"rust\">\ntips\n</skill>"),
		message("user", "   "),
	];
	let session = ChatSession::for_tests(messages);

	// System + <instructions> survive; plain user turns, assistant text,
	// inactive skill injections and blank content are excluded. No session is
	// active in this test, so every skill counts as inactive.
	assert_eq!(
		parent_agent_context(&session),
		"sys prompt\n\n<instructions>\nbe careful\n</instructions>"
	);
}

#[tokio::test]
async fn test_execute_tap_capability_inline_missing_prompt() {
	let config = template_config();
	let mut session = ChatSession::for_tests(Vec::new());
	let call = crate::mcp::McpToolCall {
		tool_name: "tap".to_string(),
		parameters: serde_json::json!({"action": "capability"}),
		tool_id: "t1".to_string(),
	};

	let (result, _elapsed_ms) = execute_tap_capability_inline(&call, &mut session, &config).await;
	assert!(result.is_error());
	assert!(result
		.extract_content()
		.contains("Missing required parameter 'prompt'"));
}

#[tokio::test]
async fn test_handle_large_tool_results_short_content_untouched() {
	let config = template_config();
	let results = vec![
		crate::mcp::McpToolResult::success("t".to_string(), "i".to_string(), "small".to_string()),
		crate::mcp::McpToolResult::error("t2".to_string(), "i2".to_string(), "bad".to_string()),
	];

	let processed = handle_large_tool_results(results, &config, OutputMode::NonInteractive)
		.await
		.expect("truncation never fails");

	assert_eq!(processed[0].extract_content(), "small");
	assert!(!processed[0].is_error());
	assert_eq!(processed[1].extract_content(), "bad");
	assert!(processed[1].is_error());
}

#[cfg(unix)]
fn make_executable(path: &std::path::Path) {
	use std::os::unix::fs::PermissionsExt;
	std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
		.expect("chmod +x fixture");
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

fn main_session_context<'a>(
	session: &'a mut ChatSession,
	processor: &'a mut ToolProcessor,
) -> ToolExecutionContext<'a> {
	ToolExecutionContext::MainSession {
		chat_session: session,
		tool_processor: processor,
		tool_round_intent: "",
	}
}

/// `ChatSession::for_tests` reports session name "test", and spawned tool
/// tasks re-establish the session id from `context.session_name()` — so the
/// session-scoped workdir registry must be keyed "test" for local-tool
/// discovery to see the fixture directory.
const TEST_SESSION: &str = "test";

#[tokio::test]
async fn test_execute_tap_capability_inline_short_prompt_reports_no_match() {
	let config = template_config();
	let mut session = ChatSession::for_tests(Vec::new());
	// "x" is below the auto-activation signal floor, so both the skill and
	// capability scanners abstain deterministically — no model, no filesystem.
	let call = crate::mcp::McpToolCall {
		tool_name: "tap".to_string(),
		parameters: serde_json::json!({"action": "capability", "prompt": "x"}),
		tool_id: "t1".to_string(),
	};

	let (result, _elapsed_ms) = execute_tap_capability_inline(&call, &mut session, &config).await;
	assert!(!result.is_error(), "{}", result.extract_content());

	let content = result.extract_content();
	let parsed: serde_json::Value = serde_json::from_str(&content).expect("json payload");
	assert_eq!(parsed["activated_skills"], serde_json::json!([]));
	assert_eq!(parsed["activated_capabilities"], serde_json::json!([]));
	assert_eq!(
		parsed["message"],
		"No skill or capability matched the prompt."
	);
}

#[tokio::test]
async fn test_execute_tools_parallel_routes_single_tap_capability_inline() {
	let config = template_config();
	let mut session = ChatSession::for_tests(Vec::new());
	let mut processor = ToolProcessor::new();
	let (_tx, rx) = tokio::sync::watch::channel(false);

	let (results, _elapsed_ms) = execute_tools_parallel(
		vec![crate::mcp::McpToolCall {
			tool_name: "tap".to_string(),
			parameters: serde_json::json!({"action": "capability", "prompt": "x"}),
			tool_id: "t1".to_string(),
		}],
		"",
		&mut session,
		&config,
		&mut processor,
		rx,
		OutputMode::Jsonl,
	)
	.await
	.expect("inline tap capability execution");

	assert_eq!(results.len(), 1);
	assert!(!results[0].is_error(), "{}", results[0].extract_content());
	assert!(results[0]
		.extract_content()
		.contains("No skill or capability matched"));
	// The inline path counts the call itself
	assert_eq!(session.session.info.tool_calls, 1);
}

/// Pre-fired cancellation with a tool that cannot finish: `select!` has only
/// the cancel branch ready, so the empty-return branch is deterministic.
#[tokio::test]
#[serial_test::serial]
#[cfg(unix)]
async fn test_execute_tools_in_context_cancelled_before_execution_returns_empty() {
	let tmp = tempfile::tempdir().expect("tempdir");
	write_local_tool(
		tmp.path(),
		"slowtool",
		"#!/bin/sh\n# @description Sleep for a long time.\nsleep 30\n",
	);

	let config = template_config();
	crate::session::context::with_session_id(TEST_SESSION.to_string(), async {
		crate::session::context::init_session_services("assistant");
		crate::mcp::workdir::set_session_working_directory(tmp.path().to_path_buf());

		let mut session = ChatSession::for_tests(Vec::new());
		let mut processor = ToolProcessor::new();
		let mut context = main_session_context(&mut session, &mut processor);
		let (tx, rx) = tokio::sync::watch::channel(false);
		tx.send(true).expect("pre-fire cancellation");

		let (results, total_ms) = execute_tools_in_context(
			vec![crate::mcp::McpToolCall {
				tool_name: "slowtool".to_string(),
				parameters: serde_json::json!({}),
				tool_id: "t1".to_string(),
			}],
			&mut context,
			&config,
			Some(rx),
			OutputMode::NonInteractive,
		)
		.await
		.expect("cancellation is Ok(empty), not Err");

		assert!(results.is_empty());
		assert_eq!(total_ms, 0);

		crate::session::context::cleanup_session(&TEST_SESSION.to_string());
	})
	.await;
}

/// Two identical successful calls in one batch: the sequential post-loop
/// records the first `(tool, args, content)` triple and elides the second into
/// the dedup placeholder error. 600 chars is above MIN_DEDUP_CONTENT_LEN.
#[tokio::test]
#[serial_test::serial]
#[cfg(unix)]
async fn test_execute_tools_in_context_local_tool_success_then_duplicate_placeholder() {
	let tmp = tempfile::tempdir().expect("tempdir");
	write_local_tool(
		tmp.path(),
		"dumpbig",
		&format!(
			"#!/bin/sh\n# @description Dump a fixed payload.\nprintf '%s' '{}'\n",
			"A".repeat(600)
		),
	);

	let config = template_config();
	crate::session::context::with_session_id(TEST_SESSION.to_string(), async {
		crate::session::context::init_session_services("assistant");
		crate::mcp::workdir::set_session_working_directory(tmp.path().to_path_buf());

		let mut session = ChatSession::for_tests(Vec::new());
		let mut processor = ToolProcessor::new();
		let mut context = main_session_context(&mut session, &mut processor);
		let (_tx, rx) = tokio::sync::watch::channel(false);

		let call = |id: &str| crate::mcp::McpToolCall {
			tool_name: "dumpbig".to_string(),
			parameters: serde_json::json!({}),
			tool_id: id.to_string(),
		};
		let (results, _total_ms) = execute_tools_in_context(
			vec![call("d1"), call("d2")],
			&mut context,
			&config,
			Some(rx),
			OutputMode::NonInteractive,
		)
		.await
		.expect("local tool execution");

		assert_eq!(results.len(), 2);
		assert!(!results[0].is_error(), "{}", results[0].extract_content());
		assert!(results[0].extract_content().len() >= 500);
		assert!(results[1].is_error());
		assert!(
			results[1].extract_content().contains("duplicate tool call"),
			"{}",
			results[1].extract_content()
		);
		assert_eq!(session.session.info.tool_calls, 2);

		crate::session::context::cleanup_session(&TEST_SESSION.to_string());
	})
	.await;
}

#[tokio::test]
async fn test_execute_layer_tool_calls_parallel_unknown_tool_error_result() {
	let config = template_config();
	let (_tx, rx) = tokio::sync::watch::channel(false);

	let (results, _total_ms) = execute_layer_tool_calls_parallel(
		&config,
		LayerToolExecutionParams {
			tool_calls: vec![crate::mcp::McpToolCall {
				tool_name: "layer_missing_tool".to_string(),
				parameters: serde_json::json!({}),
				tool_id: "l1".to_string(),
			}],
			session_name: "layer-sess".to_string(),
			layer_name: "reviewer".to_string(),
			operation_cancelled: Some(rx),
			mode: OutputMode::Jsonl,
		},
	)
	.await
	.expect("layer execution");

	assert_eq!(results.len(), 1);
	assert!(results[0].is_error());
	assert!(results[0].extract_content().contains("not found"));
}

#[tokio::test]
async fn test_parent_task_context_includes_active_plan_checklist() {
	use crate::mcp::core::plan::storage::PlanStorage;

	let session_id = "tool-exec-plan-ctx-test".to_string();
	crate::session::context::with_session_id(session_id.clone(), async {
		let storage = crate::session::context::get_plan_storage(&session_id);
		storage
			.lock()
			.expect("plan storage lock")
			.create_plan(
				"Ship the feature".to_string(),
				vec![crate::mcp::core::plan::storage::TaskData::new(
					"Write tests".to_string(),
					"Cover the loop detector".to_string(),
					None,
					None,
				)],
			)
			.expect("create plan");

		let session = ChatSession::for_tests(Vec::new());
		let task = parent_task_context(&session);
		assert!(task.contains("Live plan"), "{task}");
		assert!(task.contains("Write tests"), "{task}");

		crate::session::context::cleanup_session(&session_id);
	})
	.await;
}
