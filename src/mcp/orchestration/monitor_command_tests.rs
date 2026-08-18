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
