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

//! Tests for the `monitor` tool: validation arms, session-context
//! requirement, and a real start/list/stop lifecycle around a short-lived
//! shell command.

use super::*;
use serial_test::serial;

fn monitor_call(params: serde_json::Value) -> crate::mcp::McpToolCall {
	crate::mcp::McpToolCall {
		tool_name: "monitor".to_string(),
		parameters: params,
		tool_id: "t-mon".to_string(),
	}
}

fn text_of(result: &crate::mcp::McpToolResult) -> String {
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

fn is_err(result: &crate::mcp::McpToolResult) -> bool {
	result.result.is_error.unwrap_or(false)
}

#[tokio::test]
#[serial]
async fn test_monitor_requires_session_and_valid_action() {
	// Outside any session scope, start must refuse
	let result = execute_monitor_tool(&monitor_call(
		serde_json::json!({"action": "start", "command": "echo hi"}),
	))
	.await
	.expect("dispatch");
	assert!(is_err(&result));
	assert!(text_of(&result).contains("session"));

	// Missing / unknown action
	let result = execute_monitor_tool(&monitor_call(serde_json::json!({})))
		.await
		.expect("dispatch");
	assert!(is_err(&result));

	let result = execute_monitor_tool(&monitor_call(serde_json::json!({"action": "explode"})))
		.await
		.expect("dispatch");
	assert!(is_err(&result));
	assert!(text_of(&result).contains("unknown action"));
}

#[tokio::test]
#[serial]
async fn test_monitor_start_list_stop_lifecycle() {
	let sid = "__monitor_test_session".to_string();
	crate::session::context::with_session_id(sid.clone(), async {
		// start without a command → validation error
		let result = execute_monitor_tool(&monitor_call(serde_json::json!({"action": "start"})))
			.await
			.expect("dispatch");
		assert!(is_err(&result));
		assert!(text_of(&result).contains("command"));

		// Real start around a short-lived echo
		let result = execute_monitor_tool(&monitor_call(
			serde_json::json!({"action": "start", "command": "echo MONITOR-LIFECYCLE-OUT"}),
		))
		.await
		.expect("start dispatches");
		assert!(!is_err(&result), "start failed: {}", text_of(&result));
		let start_text = text_of(&result);

		// list answers (running or already finished — both are valid states)
		let result = execute_monitor_tool(&monitor_call(serde_json::json!({"action": "list"})))
			.await
			.expect("list dispatches");
		assert!(!is_err(&result), "list failed: {}", text_of(&result));

		// stop of a bogus id is a structured error
		let result = execute_monitor_tool(&monitor_call(
			serde_json::json!({"action": "stop", "id": "mon-does-not-exist"}),
		))
		.await
		.expect("stop dispatches");
		assert!(is_err(&result), "got: {}", text_of(&result));

		// If the start output names an id (mon-N), stopping it must work
		// whether it is still running or already done.
		if let Some(id) = start_text
			.split_whitespace()
			.find(|w| w.starts_with("mon-"))
			.map(|w| w.trim_matches(|c: char| !c.is_alphanumeric() && c != '-'))
		{
			let _ = execute_monitor_tool(&monitor_call(
				serde_json::json!({"action": "stop", "id": id}),
			))
			.await
			.expect("stop dispatches");
		}
	})
	.await;
	crate::session::context::cleanup_session(&sid);
}

// --- parameter validation, lifecycle outcomes, and registry edges -----------

use crate::session::inbox::InboxMessage;

/// Bounded wait for the next inbox message in the current session.
async fn next_message_within(bound: std::time::Duration) -> InboxMessage {
	tokio::time::timeout(bound, async {
		loop {
			if let Some(message) = crate::session::inbox::try_pop_inbox_message() {
				return message;
			}
			tokio::time::sleep(std::time::Duration::from_millis(10)).await;
		}
	})
	.await
	.expect("inbox message must arrive within the bound")
}

/// Default-bounded wait for the next inbox message in the current session.
async fn next_message() -> InboxMessage {
	next_message_within(std::time::Duration::from_secs(5)).await
}

#[test]
fn init_for_session_without_session_context_creates_no_global_bucket() {
	// Outside a session the initializer must return without touching the
	// process-global registry.
	init_for_session();
	let guard = MONITORS.read().expect("monitors registry lock");
	assert!(
		guard
			.as_ref()
			.map(|registry| registry.is_empty())
			.unwrap_or(true),
		"no session bucket may be created outside a session"
	);
}

#[tokio::test]
async fn monitor_start_validates_typed_parameters() {
	let sid = format!("monitor-validation-{}", uuid::Uuid::new_v4());
	crate::session::context::with_session_id(sid.clone(), async {
		let cases: Vec<(serde_json::Value, &str)> = vec![
			(
				serde_json::json!({"action": "start", "command": "sleep 1",
					"flush_interval_seconds": 0}),
				"flush_interval_seconds",
			),
			(
				serde_json::json!({"action": "start", "command": "sleep 1",
					"flush_interval_seconds": "fast"}),
				"must be an integer",
			),
			(
				serde_json::json!({"action": "start", "command": "sleep 1",
					"max_batch_bytes": 10}),
				"max_batch_bytes",
			),
			(
				serde_json::json!({"action": "start", "command": "sleep 1",
					"timeout_ms": false}),
				"timeout_ms",
			),
			(
				serde_json::json!({"action": "start", "command": "sleep 1",
					"persistent": "yes"}),
				"persistent",
			),
			(
				serde_json::json!({"action": "start", "command": "sleep 1",
					"working_directory": 42}),
				"working_directory",
			),
			(
				serde_json::json!({"action": "start", "command": "sleep 1",
					"working_directory": "/octomind-no-such-dir-xyz"}),
				"is invalid",
			),
			(
				serde_json::json!({"action": "start", "command": "sleep 1",
					"description": 7}),
				"description",
			),
		];
		for (params, expected) in cases {
			let result = execute_monitor_tool(&monitor_call(params))
				.await
				.expect("dispatch");
			assert!(is_err(&result), "expected error for {expected}");
			assert!(
				text_of(&result).contains(expected),
				"expected '{expected}' in: {}",
				text_of(&result)
			);
		}
	})
	.await;
	crate::session::context::cleanup_session(&sid);
}

#[cfg(unix)]
#[tokio::test]
async fn monitor_start_accepts_relative_and_absolute_workdirs_and_persistent_flag() {
	let sid = format!("monitor-workdir-{}", uuid::Uuid::new_v4());
	let dir = tempfile::tempdir().expect("tempdir");
	crate::session::context::with_session_id(sid.clone(), async {
		crate::session::inbox::init_inbox_for_session();
		init_for_session();

		for workdir in [dir.path().to_string_lossy().to_string(), ".".to_string()] {
			let result = execute_monitor_tool(&monitor_call(
				serde_json::json!({"action": "start", "command": "echo wd-ok",
					"working_directory": workdir, "persistent": true,
					"description": "workdir probe"}),
			))
			.await
			.expect("dispatch");
			assert!(!is_err(&result), "start failed: {}", text_of(&result));
		}

		// Both monitors finish on their own; each delivers one batch.
		let first = next_message().await;
		let second = next_message().await;
		assert!(first.content.contains("wd-ok"));
		assert!(second.content.contains("wd-ok"));
	})
	.await;
	crate::session::context::cleanup_session(&sid);
}

#[tokio::test]
async fn monitor_start_reports_invalid_workdir_as_tool_error() {
	// The `sh -c` spawn itself cannot be made to fail on a healthy box (the
	// workdir is canonicalized first), so the reachable start-error path is
	// the workdir validation that precedes the spawn.
	let sid = format!("monitor-spawn-{}", uuid::Uuid::new_v4());
	crate::session::context::with_session_id(sid.clone(), async {
		init_for_session();
		let result = execute_monitor_tool(&monitor_call(serde_json::json!({
			"action": "start",
			"command": "echo hi",
			"working_directory": "/octomind-no-such-dir-xyz"
		})))
		.await
		.expect("dispatch");
		assert!(is_err(&result), "got: {}", text_of(&result));
		assert!(
			text_of(&result).contains("is invalid"),
			"got: {}",
			text_of(&result)
		);
	})
	.await;
	crate::session::context::cleanup_session(&sid);
}

#[tokio::test]
async fn monitor_list_and_stop_require_session_context() {
	for action in ["list", "stop"] {
		let result = execute_monitor_tool(&monitor_call(
			serde_json::json!({"action": action, "id": "mon-x"}),
		))
		.await
		.expect("dispatch");
		assert!(is_err(&result), "got: {}", text_of(&result));
		assert!(
			text_of(&result).contains("active session"),
			"got: {}",
			text_of(&result)
		);
	}
}

#[tokio::test]
async fn monitor_stop_requires_a_string_id() {
	let sid = format!("monitor-stopid-{}", uuid::Uuid::new_v4());
	crate::session::context::with_session_id(sid.clone(), async {
		let result = execute_monitor_tool(&monitor_call(
			serde_json::json!({"action": "stop", "id": 123}),
		))
		.await
		.expect("dispatch");
		assert!(is_err(&result), "got: {}", text_of(&result));
		assert!(
			text_of(&result).contains("'id' must be a non-empty string"),
			"got: {}",
			text_of(&result)
		);
	})
	.await;
	crate::session::context::cleanup_session(&sid);
}

#[cfg(unix)]
#[tokio::test]
async fn monitor_stop_cancels_a_running_monitor() {
	let sid = format!("monitor-stop-{}", uuid::Uuid::new_v4());
	crate::session::context::with_session_id(sid.clone(), async {
		crate::session::inbox::init_inbox_for_session();
		init_for_session();

		let result = execute_monitor_tool(&monitor_call(
			serde_json::json!({"action": "start", "command": "echo pending; sleep 30",
				"flush_interval_seconds": 5, "timeout_ms": 60000}),
		))
		.await
		.expect("dispatch");
		assert!(!is_err(&result), "start failed: {}", text_of(&result));
		let id = text_of(&result)
			.split_whitespace()
			.find(|w| w.starts_with("[mon-"))
			.expect("start output names the monitor id")
			.trim_matches(|c| c == '[' || c == ']' || c == '.')
			.to_string();

		// Wait until the monitor is registered, then stop it.
		let running = tokio::time::timeout(std::time::Duration::from_secs(5), async {
			while !has_running_monitors() {
				tokio::time::sleep(std::time::Duration::from_millis(10)).await;
			}
		})
		.await;
		assert!(running.is_ok(), "monitor must register as running");

		let result = execute_monitor_tool(&monitor_call(
			serde_json::json!({"action": "stop", "id": id}),
		))
		.await
		.expect("dispatch");
		assert!(!is_err(&result), "stop failed: {}", text_of(&result));
		assert!(
			text_of(&result).contains("Stopping monitor"),
			"got: {}",
			text_of(&result)
		);

		let stopped = tokio::time::timeout(std::time::Duration::from_secs(5), async {
			while has_running_monitors() {
				tokio::time::sleep(std::time::Duration::from_millis(10)).await;
			}
		})
		.await;
		assert!(stopped.is_ok(), "cancelled monitor must be reaped");
	})
	.await;
	crate::session::context::cleanup_session(&sid);
}

#[cfg(unix)]
#[tokio::test]
async fn monitor_timeout_kills_the_command_and_reports_it() {
	let sid = format!("monitor-timeout-{}", uuid::Uuid::new_v4());
	crate::session::context::with_session_id(sid.clone(), async {
		crate::session::inbox::init_inbox_for_session();
		init_for_session();

		let result = execute_monitor_tool(&monitor_call(
			serde_json::json!({"action": "start", "command": "sleep 30",
				"flush_interval_seconds": 5, "timeout_ms": 1000}),
		))
		.await
		.expect("dispatch");
		assert!(!is_err(&result), "start failed: {}", text_of(&result));

		let message = next_message().await;
		assert!(
			message.content.contains("monitor reached timeout"),
			"got: {}",
			message.content
		);
		assert!(!has_running_monitors(), "timed-out monitor must be removed");
	})
	.await;
	crate::session::context::cleanup_session(&sid);
}

#[cfg(unix)]
#[tokio::test]
async fn monitor_successful_exit_delivers_one_terminal_batch() {
	let sid = format!("monitor-exit-{}", uuid::Uuid::new_v4());
	crate::session::context::with_session_id(sid.clone(), async {
		crate::session::inbox::init_inbox_for_session();
		init_for_session();

		// The 5s flush tick fires while the command is silent and produces no
		// delivery (empty batch, no terminal state); only the exit batch lands.
		let result = execute_monitor_tool(&monitor_call(
			serde_json::json!({"action": "start", "command": "sleep 6; echo final",
				"flush_interval_seconds": 5, "timeout_ms": 60000}),
		))
		.await
		.expect("dispatch");
		assert!(!is_err(&result), "start failed: {}", text_of(&result));

		// The command exits at ~6s; the bound must outlive the whole scenario.
		let message = next_message_within(std::time::Duration::from_secs(20)).await;
		assert!(
			message.content.contains("command exited successfully"),
			"got: {}",
			message.content
		);
		assert!(
			message.content.contains("final"),
			"post-exit drain must keep the buffered output: {}",
			message.content
		);
		assert!(
			crate::session::inbox::try_pop_inbox_message().is_none(),
			"the silent tick must not deliver a batch"
		);
		assert!(!has_running_monitors());
	})
	.await;
	crate::session::context::cleanup_session(&sid);
}

#[cfg(unix)]
#[tokio::test]
async fn monitor_failed_exit_without_stderr_omits_stderr_section() {
	let sid = format!("monitor-failquiet-{}", uuid::Uuid::new_v4());
	crate::session::context::with_session_id(sid.clone(), async {
		crate::session::inbox::init_inbox_for_session();
		init_for_session();

		let result = execute_monitor_tool(&monitor_call(
			serde_json::json!({"action": "start", "command": "exit 9",
				"flush_interval_seconds": 5, "timeout_ms": 60000}),
		))
		.await
		.expect("dispatch");
		assert!(!is_err(&result), "start failed: {}", text_of(&result));

		let message = next_message().await;
		assert!(
			message
				.content
				.contains("command exited unsuccessfully (9)"),
			"got: {}",
			message.content
		);
		assert!(
			!message.content.contains("stderr:"),
			"no stderr trailer without output: {}",
			message.content
		);
	})
	.await;
	crate::session::context::cleanup_session(&sid);
}

#[cfg(unix)]
#[tokio::test]
async fn monitor_start_rejects_workdir_that_is_a_file() {
	let sid = format!("monitor-filewd-{}", uuid::Uuid::new_v4());
	let dir = tempfile::tempdir().expect("tempdir");
	let file = dir.path().join("not-a-dir");
	std::fs::write(&file, "x").expect("write file");
	crate::session::context::with_session_id(sid.clone(), async {
		let result = execute_monitor_tool(&monitor_call(
			serde_json::json!({"action": "start", "command": "echo x",
				"working_directory": file.to_string_lossy()}),
		))
		.await
		.expect("dispatch");
		assert!(is_err(&result), "got: {}", text_of(&result));
		assert!(
			text_of(&result).contains("is not a directory"),
			"got: {}",
			text_of(&result)
		);
	})
	.await;
	crate::session::context::cleanup_session(&sid);
}
