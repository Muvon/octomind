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

#[test]
fn test_schedule_function_schema_contract() {
	let f = get_schedule_function();
	assert_eq!(f.name, "schedule");
	let command = f
		.parameters
		.get("properties")
		.and_then(|p| p.get("command"))
		.expect("command property");
	let actions: Vec<&str> = command
		.get("enum")
		.and_then(|e| e.as_array())
		.expect("command enum")
		.iter()
		.filter_map(|v| v.as_str())
		.collect();
	assert_eq!(actions, vec!["add", "list", "remove", "edit"]);
	let required = f
		.parameters
		.get("required")
		.and_then(|r| r.as_array())
		.expect("required array");
	assert_eq!(required, &vec![json!("command")]);
}

#[tokio::test]
async fn test_non_string_command_is_error() {
	crate::session::context::with_session_id("sched-test-nonstring".to_string(), async {
		let result = execute_schedule_tool(&call(serde_json::json!({"command": 42})))
			.await
			.expect("tool returns a result");
		assert!(result.is_error());
		assert!(result
			.extract_content()
			.contains("'command' must be a non-empty string"));
	})
	.await;
}

#[tokio::test]
async fn test_has_pending_schedules_and_idle_flags() {
	crate::session::context::with_session_id("sched-test-pending-flags".to_string(), async {
		assert!(!has_pending_schedules());
		assert!(!has_pending_idle_schedules());

		let timed = execute_schedule_tool(&call(serde_json::json!({
			"command": "add",
			"message": "later",
			"when": "in 2h"
		})))
		.await
		.expect("add timed entry");
		assert!(!timed.is_error(), "content: {}", timed.extract_content());
		assert!(has_pending_schedules());
		assert!(
			!has_pending_idle_schedules(),
			"timed entry is not idle-mode"
		);

		let idle = execute_schedule_tool(&call(serde_json::json!({
			"command": "add",
			"message": "when the session settles"
		})))
		.await
		.expect("add idle entry");
		assert!(!idle.is_error(), "content: {}", idle.extract_content());
		assert!(has_pending_idle_schedules());
	})
	.await;
}

// --- add/edit/remove validation arms and snapshot restore paths -------------

use serial_test::serial;

fn unique_session(tag: &str) -> String {
	format!("sched-probe-{tag}-{}", uuid::Uuid::new_v4())
}

#[tokio::test]
async fn add_rejects_non_string_message_and_remove_rejects_non_string_id() {
	let sid = unique_session("validation");
	crate::session::context::with_session_id(sid, async {
		let cases: Vec<(serde_json::Value, &str)> = vec![
			(
				serde_json::json!({"command": "add", "message": 42}),
				"'message' must be a non-empty string",
			),
			(
				serde_json::json!({"command": "remove", "id": 99}),
				"'id' must be a non-empty string",
			),
			(
				serde_json::json!({"command": "edit"}),
				"missing required parameter 'id' for edit",
			),
		];
		for (params, expected) in cases {
			let result = execute_schedule_tool(&call(params.clone()))
				.await
				.expect("tool returns a result");
			assert!(result.is_error(), "expected error for {params}");
			assert!(
				result.extract_content().contains(expected),
				"expected '{expected}', got: {}",
				result.extract_content()
			);
		}
	})
	.await;
}

#[tokio::test]
async fn idle_add_with_description_echoes_it_in_the_response() {
	let sid = unique_session("idledesc");
	crate::session::context::with_session_id(sid, async {
		let added = execute_schedule_tool(&call(serde_json::json!({
			"command": "add",
			"message": "nudge me when idle",
			"description": "idle probe",
			"every": "idle"
		})))
		.await
		.expect("add");
		assert!(!added.is_error(), "add failed: {}", added.extract_content());
		let content = added.extract_content();
		assert!(
			content.contains("Description: idle probe"),
			"description must be echoed: {content}"
		);
		assert!(
			content.contains("Repeats: every idle"),
			"idle repeat must be echoed: {content}"
		);
	})
	.await;
}

/// Point `OCTOMIND_DATA_DIR` at `path` for the duration of a test.
struct DataDirAt {
	previous: Option<std::ffi::OsString>,
}

impl DataDirAt {
	fn new(path: std::path::PathBuf) -> Self {
		let previous = std::env::var_os("OCTOMIND_DATA_DIR");
		std::env::set_var("OCTOMIND_DATA_DIR", path);
		Self { previous }
	}
}

impl Drop for DataDirAt {
	fn drop(&mut self) {
		match self.previous.take() {
			Some(v) => std::env::set_var("OCTOMIND_DATA_DIR", v),
			None => std::env::remove_var("OCTOMIND_DATA_DIR"),
		}
	}
}

#[tokio::test]
#[serial]
async fn add_survives_an_unloggable_snapshot_and_restore_handles_a_blocked_data_dir() {
	// A file where the data dir should be makes every path under it fail.
	let tmp = tempfile::tempdir().expect("tempdir");
	let blocker = tmp.path().join("blocker");
	std::fs::write(&blocker, "not a directory").expect("write blocker");
	let _guard = DataDirAt::new(blocker.join("nested"));

	let sid = unique_session("blocked");
	crate::session::context::with_session_id(sid, async {
		// Persistence is best-effort: a failing snapshot log must not break add.
		let added = execute_schedule_tool(&call(serde_json::json!({
			"command": "add",
			"message": "still schedules",
			"when": "in 10m"
		})))
		.await
		.expect("add");
		assert!(
			!added.is_error(),
			"add must survive a failed snapshot log: {}",
			added.extract_content()
		);
		assert!(has_pending_schedules());

		// Restore from an unresolvable log path is a silent no-op.
		restore_schedule_for_session("any-session");
	})
	.await;
}

#[cfg(unix)]
fn chmod(path: &std::path::Path, mode: u32) {
	use std::os::unix::fs::PermissionsExt;
	std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).expect("set permissions");
}

#[tokio::test]
#[serial]
#[cfg(unix)]
async fn restore_is_a_noop_for_unreadable_and_non_zstd_logs() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let sessions = tmp.path().join("sessions");
	std::fs::create_dir_all(&sessions).expect("create sessions dir");
	let _guard = DataDirAt::new(tmp.path().to_path_buf());

	let unreadable = sessions.join("unreadable.jsonl.zst");
	std::fs::write(&unreadable, b"placeholder").expect("write log");
	chmod(&unreadable, 0o000);

	let garbage = sessions.join("garbage.jsonl.zst");
	std::fs::write(&garbage, b"definitely not zstd data").expect("write log");

	let sid = unique_session("restore-noop");
	crate::session::context::with_session_id(sid, async {
		restore_schedule_for_session("unreadable");
		restore_schedule_for_session("garbage");
		assert!(
			!has_pending_schedules(),
			"failed restores must leave the store empty"
		);
	})
	.await;

	chmod(&unreadable, 0o644);
}

#[tokio::test]
#[serial]
async fn restore_replays_the_latest_snapshot_into_a_new_session() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let _guard = DataDirAt::new(tmp.path().to_path_buf());

	// Session one schedules something — the snapshot lands in its log.
	let writer = unique_session("writer");
	crate::session::context::with_session_id(writer.clone(), async {
		let added = execute_schedule_tool(&call(serde_json::json!({
			"command": "add",
			"message": "resume me",
			"description": "resume probe",
			"when": "in 30m"
		})))
		.await
		.expect("add");
		assert!(!added.is_error(), "add failed: {}", added.extract_content());
	})
	.await;

	// Simulate a process restart: the in-memory store for the writer session
	// is gone, only the persisted log remains.
	crate::session::context::cleanup_session(&writer);

	crate::session::context::with_session_id(writer.clone(), async {
		assert!(
			!has_pending_schedules(),
			"store must be empty after the simulated restart"
		);
		restore_schedule_for_session(&writer);
		assert!(
			has_pending_schedules(),
			"the snapshot must be replayed into the restarted session"
		);
		let listing = render_pending_entries().expect("entries pending");
		assert!(
			listing.contains("resume me"),
			"restored entry must be listed: {listing}"
		);
	})
	.await;

	crate::session::context::cleanup_session(&writer);
}
