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

//! Schedule tool lifecycle tests through the real tool-call interface.
//! Each test runs inside a unique task-local session id, so the store is
//! session-scoped and parallel tests never share state.

use super::*;

fn call(params: serde_json::Value) -> McpToolCall {
	McpToolCall {
		tool_name: "schedule".to_string(),
		parameters: params,
		tool_id: "sched-test".to_string(),
	}
}

/// Extract the `[id]` from an add-command success message.
fn extract_id(text: &str) -> String {
	let start = text.find('[').expect("id bracket in add response") + 1;
	let end = text[start..].find(']').expect("closing bracket") + start;
	text[start..end].to_string()
}

#[tokio::test]
async fn test_add_list_edit_remove_lifecycle() {
	crate::session::context::with_session_id("sched-test-lifecycle".to_string(), async {
		let added = execute_schedule_tool(&call(serde_json::json!({
			"command": "add",
			"message": "check the build",
			"description": "ci poll",
			"when": "in 5m"
		})))
		.await
		.expect("add");
		assert!(!added.is_error(), "add failed: {}", added.extract_content());
		let id = extract_id(&added.extract_content());

		assert!(has_pending_schedules());
		let listing = execute_schedule_tool(&call(serde_json::json!({"command": "list"})))
			.await
			.expect("list")
			.extract_content();
		assert!(listing.contains(&id), "listing: {listing}");
		assert!(listing.contains("ci poll"), "listing: {listing}");

		let edited = execute_schedule_tool(&call(serde_json::json!({
			"command": "edit",
			"id": id,
			"message": "check the deploy",
			"every": "10m"
		})))
		.await
		.expect("edit");
		assert!(
			!edited.is_error(),
			"edit failed: {}",
			edited.extract_content()
		);
		let listing = render_pending_entries().expect("entries pending");
		assert!(listing.contains("10m"), "listing after edit: {listing}");

		let removed = execute_schedule_tool(&call(serde_json::json!({
			"command": "remove",
			"id": id
		})))
		.await
		.expect("remove");
		assert!(!removed.is_error());
		assert!(!has_pending_schedules());
		assert!(render_pending_entries().is_none());
	})
	.await;
}

#[tokio::test]
async fn test_idle_default_and_due_flush() {
	crate::session::context::with_session_id("sched-test-flush".to_string(), async {
		// Message-only add defaults to a one-shot idle entry
		let added = execute_schedule_tool(&call(serde_json::json!({
			"command": "add",
			"message": "wrap up"
		})))
		.await
		.expect("add idle");
		assert!(!added.is_error());
		assert!(has_pending_idle_schedules());

		// Idle flush consumes the one-shot idle entry into the inbox
		flush_idle_to_inbox();
		assert!(!has_pending_idle_schedules());

		// A due repeating entry is rescheduled by the flush, not consumed
		let added = execute_schedule_tool(&call(serde_json::json!({
			"command": "add",
			"message": "poll status",
			"when": "now",
			"every": "10m"
		})))
		.await
		.expect("add repeating");
		assert!(!added.is_error());
		let id = extract_id(&added.extract_content());
		flush_due_to_inbox();
		assert!(
			has_pending_schedules(),
			"repeating entry must be rescheduled after firing"
		);

		// The ID survives the reschedule, otherwise the entry can never be removed.
		let listing = render_pending_entries().expect("rescheduled entry listed");
		assert!(
			listing.contains(&id),
			"rescheduled entry must keep id {id}: {listing}"
		);
		let removed = execute_schedule_tool(&call(serde_json::json!({
			"command": "remove",
			"id": id
		})))
		.await
		.expect("remove");
		assert!(!removed.is_error(), "{}", removed.extract_content());
		assert!(!has_pending_schedules());
	})
	.await;
}

#[tokio::test]
async fn test_error_paths() {
	crate::session::context::with_session_id("sched-test-errors".to_string(), async {
		for (params, expect) in [
			(serde_json::json!({}), "command"),
			(serde_json::json!({"command": "explode"}), "unknown command"),
			(serde_json::json!({"command": "add"}), "message"),
			(serde_json::json!({"command": "remove"}), "id"),
			(
				serde_json::json!({"command": "remove", "id": "nope1234"}),
				"nope1234",
			),
			(
				serde_json::json!({"command": "edit", "id": "nope1234"}),
				"edit requires at least one of",
			),
			(
				serde_json::json!({"command": "edit", "id": "nope1234", "message": "x"}),
				"nope1234",
			),
			(
				serde_json::json!({"command": "add", "message": "x", "when": "in potato"}),
				"potato",
			),
		] {
			let result = execute_schedule_tool(&call(params.clone()))
				.await
				.expect("tool returns a result");
			assert!(result.is_error(), "expected error for {params}");
			assert!(
				result.extract_content().contains(expect),
				"error for {params} should mention '{expect}', got: {}",
				result.extract_content()
			);
		}
	})
	.await;
}
