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

//! Generic schedule action validation, CRUD, session isolation, and persistence.

use super::*;
use crate::config::{Config, McpServerConfig};
use crate::session::context::{cleanup_session, current_session_id, with_session_id};
use std::path::PathBuf;

fn call(parameters: Value) -> McpToolCall {
	McpToolCall {
		tool_name: "schedule".to_string(),
		tool_id: "schedule-action-test".to_string(),
		parameters,
	}
}

struct SessionCleanup(String);

impl Drop for SessionCleanup {
	fn drop(&mut self) {
		cleanup_session(&self.0);
	}
}

/// Use a real builtin, but never dispatch it: these tests exercise scheduling,
/// not execution. Isolate before initialization so parallel tests cannot grant tools.
async fn with_action_session(enable_tool: bool, future: impl std::future::Future<Output = ()>) {
	let session = format!("schedule-action-{}", uuid::Uuid::new_v4());
	let _cleanup = SessionCleanup(session.clone());
	let workdir = tempfile::tempdir().expect("temporary session workdir");
	with_session_id(session.clone(), async {
		crate::mcp::set_session_working_directory(workdir.path().to_path_buf());
		crate::mcp::tool_map::isolate_session_tool_map(&session);
		let mut config: Config = toml::from_str(include_str!("../../../../config-templates/default.toml"))
			.expect("parse default configuration");
		config.mcp.servers = if enable_tool {
			vec![McpServerConfig::builtin(
				"orchestration",
				30,
				vec!["schedule".to_string()],
			)]
		} else {
			vec![]
		};
		crate::mcp::tool_map::initialize_tool_map(&config)
			.await
			.expect("initialize isolated tool map");
		assert_eq!(
			crate::mcp::tool_map::get_server_for_tool("schedule").is_some(),
			enable_tool
		);
		future.await;
	})
	.await;
}

async fn success(parameters: Value) -> String {
	let result = execute_schedule_tool(&call(parameters))
		.await
		.expect("schedule failures must be returned as tool results");
	assert!(!result.is_error(), "{}", result.extract_content());
	result.extract_content()
}

fn extract_id(text: &str) -> String {
	let start = text.find('[').expect("entry id in add response") + 1;
	let end = text[start..].find(']').expect("closing id bracket") + start;
	text[start..end].to_string()
}

fn entries() -> Value {
	serde_json::to_value(get_store().lock().unwrap().entries()).expect("serialize scheduled entries")
}

fn invalid_payloads() -> Vec<(&'static str, Value, &'static str)> {
	vec![
		("empty message", json!({"message": ""}), "non-empty string"),
		("blank message", json!({"message": " \t\n\u{2003}"}), "non-empty string"),
		("null message", json!({"message": null}), "non-empty string"),
		("nonstring message", json!({"message": 7}), "non-empty string"),
		("object message", json!({"message": {}}), "non-empty string"),
		("null tool call", json!({"tool_call": null}), "tool_call"),
		("string tool call", json!({"tool_call": "schedule"}), "tool_call"),
		("array tool call", json!({"tool_call": []}), "tool_call"),
		(
			"positional tool call",
			json!({"tool_call": ["schedule", {}]}),
			"tool_call",
		),
		("missing fields", json!({"tool_call": {}}), "tool_call"),
		("missing name", json!({"tool_call": {"arguments": {}}}), "tool_call"),
		(
			"missing arguments",
			json!({"tool_call": {"name": "schedule"}}),
			"tool_call",
		),
		(
			"null name",
			json!({"tool_call": {"name": null, "arguments": {}}}),
			"tool_call",
		),
		(
			"nonstring name",
			json!({"tool_call": {"name": 7, "arguments": {}}}),
			"tool_call",
		),
		(
			"empty name",
			json!({"tool_call": {"name": "", "arguments": {}}}),
			"non-empty name",
		),
		(
			"blank name",
			json!({"tool_call": {"name": " \t\n\u{2003}", "arguments": {}}}),
			"non-empty name",
		),
		(
			"null arguments",
			json!({"tool_call": {"name": "schedule", "arguments": null}}),
			"object arguments",
		),
		(
			"array arguments",
			json!({"tool_call": {"name": "schedule", "arguments": []}}),
			"object arguments",
		),
		(
			"string arguments",
			json!({"tool_call": {"name": "schedule", "arguments": "{}"}}),
			"object arguments",
		),
		(
			"number arguments",
			json!({"tool_call": {"name": "schedule", "arguments": 1}}),
			"object arguments",
		),
		(
			"boolean arguments",
			json!({"tool_call": {"name": "schedule", "arguments": true}}),
			"object arguments",
		),
		(
			"unknown field",
			json!({"tool_call": {"name": "schedule", "arguments": {}, "extra": true}}),
			"unknown field",
		),
		(
			"both payloads",
			json!({"message": "hello", "tool_call": {"name": "schedule", "arguments": {}}}),
			"exactly one",
		),
		("both null", json!({"message": null, "tool_call": null}), "exactly one"),
		(
			"unknown tool",
			json!({"tool_call": {"name": "no_such_scheduled_tool", "arguments": {}}}),
			"not available",
		),
	]
}

#[test]
fn test_parse_action_message_preserves_unicode_and_whitespace() {
	for message in ["x", " 雪 🦀\n", "\tkeep surrounding whitespace\t"] {
		let action = parse_action(&call(json!({"message": message}))).expect("valid message");
		match action.expect("message action") {
			ScheduleAction::Message { message: actual } => assert_eq!(actual, message),
			other => panic!("expected message, got {other:?}"),
		}
	}
}

#[test]
fn test_parse_action_omitted_payload_leaves_edit_action_unchanged() {
	assert!(parse_action(&call(json!({"description": "metadata only"})))
		.expect("no payload is permitted for metadata edits")
		.is_none());
}

#[test]
fn test_parse_action_tool_call_without_active_session_is_rejected() {
	assert!(current_session_id().is_none());
	let error = parse_action(&call(json!({"tool_call": {"name": "schedule", "arguments": {}}})))
		.expect_err("direct tool schedules require a session");
	assert!(error.contains("active session"), "{error}");
}

#[tokio::test]
async fn test_parse_action_tool_call_preserves_arguments_and_captures_cwd() {
	with_action_session(true, async {
		let captured = crate::mcp::get_thread_working_directory();
		for arguments in [
			json!({}),
			json!({"nested": {"雪": [null, true, 0, "🦀"]}, "negative": -1}),
		] {
			let action = parse_action(&call(
				json!({"tool_call": {"name": "schedule", "arguments": arguments}}),
			))
			.expect("enabled tool with object arguments")
			.expect("tool action");
			match action {
				ScheduleAction::ToolCall { tool_call, workdir } => {
					assert_eq!(tool_call.name, "schedule");
					assert_eq!(tool_call.arguments, arguments);
					assert_eq!(workdir, captured);
				}
				other => panic!("expected tool call, got {other:?}"),
			}
		}
	})
	.await;
}

#[tokio::test]
async fn test_schedule_add_invalid_payload_returns_error_without_entry() {
	with_action_session(true, async {
		let mut cases = invalid_payloads();
		cases.push(("no payload", json!({}), "exactly one"));
		for (scenario, mut parameters, expected_error) in cases {
			parameters["command"] = json!("add");
			let result = execute_schedule_tool(&call(parameters))
				.await
				.expect("error result, not Err");
			assert!(result.is_error(), "{scenario}: {}", result.extract_content());
			assert!(
				result.extract_content().contains(expected_error),
				"{scenario}: {}",
				result.extract_content()
			);
			assert_eq!(entries(), json!([]), "{scenario} must not add an entry");
		}
	})
	.await;
}

#[tokio::test]
async fn test_schedule_tool_add_list_edit_remove_preserves_payload() {
	with_action_session(true, async {
		let captured = crate::mcp::get_thread_working_directory();
		let added = success(json!({
			"command": "add", "description": "tool lifecycle",
			"tool_call": {"name": "schedule", "arguments": {"command": "list"}}
		}))
		.await;
		let id = extract_id(&added);
		let initial = entries();
		assert_eq!(initial.as_array().unwrap().len(), 1);
		assert_eq!(initial[0]["id"], id);
		assert_eq!(
			initial[0]["tool_call"],
			json!({"name": "schedule", "arguments": {"command": "list"}})
		);
		assert_eq!(initial[0]["workdir"], json!(captured));
		assert!(initial[0].get("message").is_none());
		let listing = success(json!({"command": "list"})).await;
		for expected in [
			id.as_str(),
			"tool lifecycle",
			"Tool call: schedule",
			"\"command\":\"list\"",
		] {
			assert!(listing.contains(expected), "missing {expected:?}: {listing}");
		}

		success(json!({"command": "edit", "id": id, "tool_call": {"name": "schedule", "arguments": {}}})).await;
		let edited = entries();
		assert_eq!(edited[0]["id"], id);
		assert_eq!(edited[0]["tool_call"], json!({"name": "schedule", "arguments": {}}));
		assert_eq!(edited[0]["trigger_at"], initial[0]["trigger_at"]);
		assert_eq!(edited[0]["created_at"], initial[0]["created_at"]);
		assert_eq!(edited[0]["description"], "tool lifecycle");
		let listing = success(json!({"command": "list"})).await;
		assert!(listing.contains("Tool call: schedule {}"), "{listing}");
		assert!(
			!listing.contains("\"command\""),
			"old arguments survived edit: {listing}"
		);

		success(json!({"command": "remove", "id": id})).await;
		assert_eq!(entries(), json!([]));
		assert_eq!(success(json!({"command": "list"})).await, "No scheduled entries.");
	})
	.await;
}

#[tokio::test]
async fn test_schedule_edit_switches_message_and_tool_actions_without_stale_fields() {
	with_action_session(true, async {
		let id = extract_id(&success(json!({"command": "add", "message": "original message"})).await);
		let captured = crate::mcp::get_thread_working_directory();
		success(json!({"command": "edit", "id": id, "tool_call": {"name": "schedule", "arguments": {}}})).await;
		let tool_entry = entries();
		assert_eq!(tool_entry[0]["tool_call"], json!({"name": "schedule", "arguments": {}}));
		assert_eq!(tool_entry[0]["workdir"], json!(captured));
		assert!(tool_entry[0].get("message").is_none());

		success(json!({"command": "edit", "id": id, "message": "replacement 雪"})).await;
		let message_entry = entries();
		assert_eq!(message_entry[0]["id"], id);
		assert_eq!(message_entry[0]["message"], "replacement 雪");
		assert!(message_entry[0].get("tool_call").is_none());
		assert!(message_entry[0].get("workdir").is_none());
		let listing = success(json!({"command": "list"})).await;
		assert!(listing.contains("Message: replacement 雪"), "{listing}");
		assert!(!listing.contains("Tool call:"), "{listing}");
	})
	.await;
}

#[tokio::test]
async fn test_schedule_edit_metadata_only_retains_tool_payload_and_captured_cwd() {
	with_action_session(true, async {
		let id =
			extract_id(&success(json!({"command": "add", "tool_call": {"name": "schedule", "arguments": {}}})).await);
		let before = entries();
		crate::mcp::set_thread_working_directory(PathBuf::from("later-workdir"));
		success(json!({"command": "edit", "id": id, "description": "new description"})).await;
		let after = entries();
		assert_eq!(after[0]["description"], "new description");
		assert_eq!(after[0]["tool_call"], before[0]["tool_call"]);
		assert_eq!(after[0]["workdir"], before[0]["workdir"]);
		assert_eq!(after[0]["trigger_at"], before[0]["trigger_at"]);
		assert_eq!(after[0]["interval_secs"], before[0]["interval_secs"]);
	})
	.await;
}

#[tokio::test]
async fn test_schedule_edit_invalid_payload_does_not_mutate_either_action_variant() {
	with_action_session(true, async {
		for payload in [
			json!({"message": "keep me"}),
			json!({"tool_call": {"name": "schedule", "arguments": {"keep": true}}}),
		] {
			let mut add = payload;
			add["command"] = json!("add");
			add["description"] = json!("original description");
			add["when"] = json!("in 1h");
			add["every"] = json!("5m");
			let id = extract_id(&success(add).await);
			let before = entries();
			for (scenario, mut parameters, expected_error) in invalid_payloads() {
				parameters["command"] = json!("edit");
				parameters["id"] = json!(id);
				parameters["description"] = json!("must not be applied");
				parameters["when"] = json!("in 2h");
				parameters["every"] = json!("off");
				let result = execute_schedule_tool(&call(parameters))
					.await
					.expect("error result, not Err");
				assert!(result.is_error(), "{scenario}: {}", result.extract_content());
				assert!(
					result.extract_content().contains(expected_error),
					"{scenario}: {}",
					result.extract_content()
				);
				assert_eq!(entries(), before, "{scenario} must leave the entire store unchanged");
			}
			success(json!({"command": "remove", "id": id})).await;
		}
	})
	.await;
}

#[tokio::test]
async fn test_schedule_add_action_variants_keep_idle_time_and_repeat_defaults() {
	with_action_session(true, async {
		for payload in [
			json!({"message": "unchanged defaults"}),
			json!({"tool_call": {"name": "schedule", "arguments": {}}}),
		] {
			for (timing, mode, interval) in [
				(json!({}), TriggerMode::Idle, None),
				(json!({"when": "idle"}), TriggerMode::Idle, None),
				(json!({"every": "idle"}), TriggerMode::Idle, Some(0)),
				(json!({"when": "in 1h"}), TriggerMode::Time, None),
				(json!({"when": "in 1h", "every": "5m"}), TriggerMode::Time, Some(300)),
			] {
				let mut parameters = payload.clone();
				parameters["command"] = json!("add");
				parameters
					.as_object_mut()
					.unwrap()
					.extend(timing.as_object().unwrap().clone());
				let before = chrono::Local::now();
				let id = extract_id(&success(parameters).await);
				let after = chrono::Local::now();
				let entry = get_store().lock().unwrap().entries()[0].clone();
				assert_eq!(entry.trigger_mode, mode);
				assert_eq!(entry.interval_secs, interval);
				assert!(entry.description.is_empty());
				if mode == TriggerMode::Time {
					assert!(entry.trigger_at >= before + chrono::Duration::hours(1));
					assert!(entry.trigger_at <= after + chrono::Duration::hours(1));
				}
				success(json!({"command": "remove", "id": id})).await;
			}
		}
	})
	.await;
}

#[tokio::test]
async fn test_schedule_tool_permissions_entries_and_workdir_are_session_isolated() {
	with_action_session(true, async {
		let first_session = current_session_id().unwrap();
		let captured = crate::mcp::get_thread_working_directory();
		let id =
			extract_id(&success(json!({"command": "add", "tool_call": {"name": "schedule", "arguments": {}}})).await);
		crate::mcp::set_thread_working_directory(captured.join("changed-after-add"));
		let first_entries = entries();
		assert_eq!(first_entries[0]["workdir"], json!(captured));

		with_action_session(false, async {
			assert_ne!(current_session_id().unwrap(), first_session);
			assert_ne!(crate::mcp::get_thread_working_directory(), captured);
			assert_eq!(success(json!({"command": "list"})).await, "No scheduled entries.");
			let denied = execute_schedule_tool(&call(json!({
				"command": "add", "tool_call": {"name": "schedule", "arguments": {}}
			})))
			.await
			.expect("permission error result");
			assert!(denied.is_error());
			assert!(
				denied.extract_content().contains("not available"),
				"{}",
				denied.extract_content()
			);
			let own_id = extract_id(&success(json!({"command": "add", "message": "second session"})).await);
			let second_entries = entries();
			let denied_edit = execute_schedule_tool(&call(json!({
				"command": "edit", "id": own_id, "description": "must not change",
				"tool_call": {"name": "schedule", "arguments": {}}
			})))
			.await
			.expect("disabled tool edit is an error result");
			assert!(denied_edit.is_error());
			assert!(
				denied_edit.extract_content().contains("not available"),
				"{}",
				denied_edit.extract_content()
			);
			assert_eq!(
				entries(),
				second_entries,
				"a tool grant in another session must not permit this edit"
			);
			for command in ["edit", "remove"] {
				let result =
					execute_schedule_tool(&call(json!({"command": command, "id": id, "message": "intrusion"})))
						.await
						.expect("cross-session id is an error result");
				assert!(result.is_error(), "{command}: {}", result.extract_content());
				assert!(
					result.extract_content().contains("No scheduled entry"),
					"{}",
					result.extract_content()
				);
				assert_eq!(
					entries(),
					second_entries,
					"cross-session {command} mutated local entries"
				);
			}
			success(json!({"command": "remove", "id": own_id})).await;
			assert_eq!(entries(), json!([]));
		})
		.await;

		assert_eq!(entries(), first_entries, "the other session must not mutate the owner");
		let replacement_cwd = crate::mcp::get_thread_working_directory();
		success(json!({"command": "edit", "id": id, "tool_call": {"name": "schedule", "arguments": {"fresh": true}}}))
			.await;
		assert_eq!(
			entries()[0]["workdir"],
			json!(replacement_cwd),
			"a supplied tool payload captures the new cwd"
		);
	})
	.await;
}

#[test]
fn test_scheduled_tool_call_deserialization_requires_fields_and_denies_unknown_fields() {
	let valid = json!({"name": "schedule", "arguments": {"雪": [null, 1, true]}});
	let tool: ScheduledToolCall = serde_json::from_value(valid.clone()).expect("typed tool call");
	assert_eq!(tool.name, "schedule");
	assert_eq!(tool.arguments, valid["arguments"]);
	assert_eq!(serde_json::to_value(tool).unwrap(), valid);
	for invalid in [
		json!({}),
		json!({"arguments": {}}),
		json!({"name": "schedule"}),
		json!({"name": 1, "arguments": {}}),
		json!({"name": "schedule", "arguments": {}, "extra": true}),
	] {
		assert!(
			serde_json::from_value::<ScheduledToolCall>(invalid.clone()).is_err(),
			"accepted {invalid}"
		);
	}
}

#[test]
fn test_schedule_entry_flattened_actions_roundtrip_and_legacy_message_defaults_to_time() {
	let workdir = PathBuf::from("workspace").join("雪");
	for (action, expected_payload) in [
		(
			ScheduleAction::Message {
				message: "legacy 雪".to_string(),
			},
			json!({"message": "legacy 雪"}),
		),
		(
			ScheduleAction::ToolCall {
				tool_call: ScheduledToolCall {
					name: "schedule".to_string(),
					arguments: json!({"nested": [null, false, 0]}),
				},
				workdir: workdir.clone(),
			},
			json!({"tool_call": {"name": "schedule", "arguments": {"nested": [null, false, 0]}}, "workdir": workdir}),
		),
	] {
		let entry = ScheduleEntry::new("snapshot".to_string(), action, chrono::Local::now(), Some(300));
		let encoded = serde_json::to_value(&entry).expect("serialize action entry");
		assert!(encoded.get("action").is_none(), "action must be flattened: {encoded}");
		for key in ["message", "tool_call", "workdir"] {
			assert_eq!(encoded.get(key), expected_payload.get(key), "flattened field {key}");
		}
		let decoded: ScheduleEntry = serde_json::from_value(encoded.clone()).expect("deserialize action entry");
		assert_eq!(serde_json::to_value(decoded).unwrap(), encoded);
		if expected_payload.get("message").is_some() {
			let mut legacy = encoded;
			legacy.as_object_mut().unwrap().remove("trigger_mode");
			let decoded: ScheduleEntry = serde_json::from_value(legacy).expect("pre-action message snapshot");
			assert_eq!(decoded.trigger_mode, TriggerMode::Time);
			match decoded.action {
				ScheduleAction::Message { message } => assert_eq!(message, "legacy 雪"),
				other => panic!("legacy message became {other:?}"),
			}
		}
	}
}

// Only the real-log test changes the data directory; match the existing serial
// snapshot fixtures and restore the environment even if an assertion panics.
struct TempDataDir {
	previous: Option<std::ffi::OsString>,
	_dir: tempfile::TempDir,
}

impl TempDataDir {
	fn new() -> Self {
		let previous = std::env::var_os("OCTOMIND_DATA_DIR");
		let dir = tempfile::tempdir().expect("temporary snapshot data directory");
		std::env::set_var("OCTOMIND_DATA_DIR", dir.path());
		Self { previous, _dir: dir }
	}
}

impl Drop for TempDataDir {
	fn drop(&mut self) {
		match self.previous.take() {
			Some(previous) => std::env::set_var("OCTOMIND_DATA_DIR", previous),
			None => std::env::remove_var("OCTOMIND_DATA_DIR"),
		}
	}
}

#[serial_test::serial]
#[tokio::test]
async fn test_schedule_snapshot_restores_tool_arguments_cwd_and_latest_removal() {
	let _data_dir = TempDataDir::new();
	with_action_session(true, async {
		let session = current_session_id().unwrap();
		let id = extract_id(
			&success(json!({
				"command": "add", "when": "in 1h", "every": "5m", "description": "persisted tool",
				"tool_call": {"name": "schedule", "arguments": {"nested": {"雪": [null, true, 1]}}}
			}))
			.await,
		);
		success(json!({"command": "add", "message": "message survives alongside tool"})).await;
		let before = entries();
		let path = crate::session::logger::get_session_log_path(&session).expect("real snapshot log path");
		assert!(path.is_file(), "API mutations must persist a real log");
		crate::session::context::clear_schedule_storage(&session);
		assert_eq!(entries(), json!([]));
		crate::mcp::set_thread_working_directory(PathBuf::from("cwd-after-resume"));
		restore_schedule_for_session(&session);
		assert_eq!(
			entries(),
			before,
			"restore must preserve both flattened payloads, cwd, and repeat metadata"
		);

		success(json!({"command": "remove", "id": id})).await;
		let after_remove = entries();
		assert_eq!(after_remove.as_array().unwrap().len(), 1);
		assert_eq!(after_remove[0]["message"], "message survives alongside tool");
		crate::session::context::clear_schedule_storage(&session);
		restore_schedule_for_session(&session);
		assert_eq!(
			entries(),
			after_remove,
			"latest snapshot must not resurrect the removed tool action"
		);
	})
	.await;
}
