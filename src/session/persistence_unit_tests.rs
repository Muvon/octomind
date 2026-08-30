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

//! Unit tests for the pure helpers in `persistence.rs`: incomplete-tool-call
//! detection and cleanup, log-line parsing (current JSON and legacy prefix
//! formats), session listing, project-session matching, and append framing.
//!
//! Filesystem-touching tests sandbox `OCTOMIND_DATA_DIR` and must stay
//! `#[serial_test::serial]` because env vars are process-global.

use super::*;
use serde_json::json;
use std::io::{BufRead, Cursor};
use std::path::{Path, PathBuf};

// ---- helpers ----

fn plain_msg(role: &str, content: &str) -> Message {
	Message {
		role: role.to_string(),
		content: content.to_string(),
		timestamp: 1_700_000_000,
		..Default::default()
	}
}

fn assistant_with_tool_calls(tool_calls: serde_json::Value) -> Message {
	Message {
		role: "assistant".to_string(),
		content: String::new(),
		tool_calls: Some(tool_calls),
		..Default::default()
	}
}

fn tool_msg(call_id: &str, name: &str) -> Message {
	Message {
		role: "tool".to_string(),
		content: "result".to_string(),
		tool_call_id: Some(call_id.to_string()),
		name: Some(name.to_string()),
		..Default::default()
	}
}

fn call(id: &str, name: &str) -> serde_json::Value {
	json!({
		"id": id,
		"type": "function",
		"function": {"name": name, "arguments": {}}
	})
}

fn test_session_info(name: &str) -> SessionInfo {
	SessionInfo {
		name: name.to_string(),
		created_at: 1_700_000_000,
		model: "test/model".to_string(),
		..Default::default()
	}
}

fn parse(lines: &[String]) -> ParsedLogLines {
	let joined = lines.join("\n");
	parse_log_lines(Cursor::new(joined)).expect("parse log lines")
}

fn summary_json_line(info: &SessionInfo, timestamp: u64) -> String {
	json!({
		"type": "SUMMARY",
		"timestamp": timestamp,
		"session_info": info,
	})
	.to_string()
}

fn msg_json_line(role: &str, content: &str) -> String {
	serde_json::to_string(&plain_msg(role, content)).expect("serialize message")
}

/// Point `OCTOMIND_DATA_DIR` at a fresh tempdir for the duration of a test,
/// restoring the previous value on drop. Tests using it must be
/// `#[serial_test::serial]` because env vars are process-global.
struct DataDirGuard {
	previous: Option<std::ffi::OsString>,
	_dir: tempfile::TempDir,
}

impl DataDirGuard {
	fn new() -> Self {
		let previous = std::env::var_os("OCTOMIND_DATA_DIR");
		let dir = tempfile::tempdir().expect("failed to create tempdir");
		std::env::set_var("OCTOMIND_DATA_DIR", dir.path());
		Self {
			previous,
			_dir: dir,
		}
	}
}

impl Drop for DataDirGuard {
	fn drop(&mut self) {
		match self.previous.take() {
			Some(v) => std::env::set_var("OCTOMIND_DATA_DIR", v),
			None => std::env::remove_var("OCTOMIND_DATA_DIR"),
		}
	}
}

/// Append lines to `<sessions dir>/<name>.jsonl.zst` and return the path.
fn write_named_session(name: &str, lines: &[String]) -> PathBuf {
	let dir = get_sessions_dir().expect("sessions dir");
	let path = dir.join(format!("{name}.jsonl.zst"));
	for line in lines {
		append_to_session_file(&path, line).expect("append session line");
	}
	path
}

fn set_mtime(path: &Path, secs: u64) {
	let file = std::fs::File::options()
		.write(true)
		.open(path)
		.expect("open for mtime");
	file.set_times(
		std::fs::FileTimes::new()
			.set_modified(std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs)),
	)
	.expect("set mtime");
}

// ---- has_incomplete_tool_calls ----

#[test]
fn has_incomplete_tool_calls_empty_is_false() {
	assert!(!has_incomplete_tool_calls(&[]));
}

#[test]
fn has_incomplete_tool_calls_complete_sequence_is_false() {
	let messages = vec![
		plain_msg("user", "go"),
		assistant_with_tool_calls(json!([call("call_1", "view")])),
		tool_msg("call_1", "view"),
		plain_msg("assistant", "done"),
	];
	assert!(!has_incomplete_tool_calls(&messages));
}

#[test]
fn has_incomplete_tool_calls_missing_response_is_true() {
	let messages = vec![
		plain_msg("user", "go"),
		assistant_with_tool_calls(json!([call("call_1", "view")])),
	];
	assert!(has_incomplete_tool_calls(&messages));
}

#[test]
fn has_incomplete_tool_calls_one_of_several_missing_is_true() {
	let messages = vec![
		assistant_with_tool_calls(json!([call("call_1", "view"), call("call_2", "shell")])),
		tool_msg("call_1", "view"),
	];
	assert!(has_incomplete_tool_calls(&messages));
}

#[test]
fn has_incomplete_tool_calls_response_before_assistant_does_not_count() {
	// The response must appear AFTER the assistant message that issued the call.
	let messages = vec![
		tool_msg("call_1", "view"),
		assistant_with_tool_calls(json!([call("call_1", "view")])),
	];
	assert!(has_incomplete_tool_calls(&messages));
}

#[test]
fn has_incomplete_tool_calls_non_assistant_messages_only_is_false() {
	let messages = vec![plain_msg("user", "hi"), plain_msg("tool", "orphan result")];
	assert!(!has_incomplete_tool_calls(&messages));
}

#[test]
fn has_incomplete_tool_calls_malformed_entries_are_skipped() {
	// A non-array tool_calls value and a call without an id carry no
	// checkable identity — neither may flag the conversation as incomplete.
	let messages = vec![
		Message {
			role: "assistant".to_string(),
			content: String::new(),
			tool_calls: Some(json!({"not": "an array"})),
			..Default::default()
		},
		Message {
			role: "assistant".to_string(),
			content: String::new(),
			tool_calls: Some(json!([{"type": "function", "function": {"name": "view"}}])),
			..Default::default()
		},
	];
	assert!(!has_incomplete_tool_calls(&messages));
}

// ---- clean_interrupted_tool_calls ----

#[test]
fn clean_interrupted_tool_calls_empty_is_noop() {
	let mut messages = Vec::new();
	assert!(!clean_interrupted_tool_calls(&mut messages, "test"));
	assert!(messages.is_empty());
}

#[test]
fn clean_interrupted_tool_calls_complete_sequence_untouched() {
	let messages = vec![
		assistant_with_tool_calls(json!([call("call_1", "view")])),
		tool_msg("call_1", "view"),
	];
	let mut cleaned = messages.clone();
	assert!(!clean_interrupted_tool_calls(&mut cleaned, "test"));
	assert_eq!(cleaned.len(), 2);
}

#[test]
fn clean_interrupted_tool_calls_inserts_synthetic_response() {
	let mut messages = vec![
		plain_msg("user", "go"),
		assistant_with_tool_calls(json!([call("call_1", "view")])),
		plain_msg("user", "interrupted here"),
	];
	assert!(clean_interrupted_tool_calls(&mut messages, "test"));

	assert_eq!(messages.len(), 4);
	let synthetic = &messages[2]; // directly after the assistant message
	assert_eq!(synthetic.role, "tool");
	assert_eq!(
		synthetic.content,
		"[Tool execution was interrupted by user]"
	);
	assert_eq!(synthetic.tool_call_id.as_deref(), Some("call_1"));
	assert_eq!(synthetic.name.as_deref(), Some("view"));
	assert!(synthetic.tool_calls.is_none());
}

#[test]
fn clean_interrupted_tool_calls_patches_every_missing_call_in_order() {
	let mut messages = vec![assistant_with_tool_calls(json!([
		call("call_1", "view"),
		call("call_2", "shell"),
	]))];
	assert!(clean_interrupted_tool_calls(&mut messages, "test"));

	assert_eq!(messages.len(), 3);
	assert_eq!(messages[1].tool_call_id.as_deref(), Some("call_2"));
	assert_eq!(messages[2].tool_call_id.as_deref(), Some("call_1"));
	assert_eq!(
		messages[1].content,
		"[Tool execution was interrupted by user]"
	);
	assert_eq!(
		messages[2].content,
		"[Tool execution was interrupted by user]"
	);
}

#[test]
fn clean_interrupted_tool_calls_inserts_after_existing_responses() {
	// call_1 is already answered: the synthetic result for call_2 must land
	// after the real tool response, not between it and the assistant.
	let mut messages = vec![
		assistant_with_tool_calls(json!([call("call_1", "view"), call("call_2", "shell")])),
		tool_msg("call_1", "view"),
	];
	assert!(clean_interrupted_tool_calls(&mut messages, "test"));

	assert_eq!(messages.len(), 3);
	assert_eq!(messages[1].tool_call_id.as_deref(), Some("call_1"));
	assert_eq!(messages[1].content, "result");
	assert_eq!(messages[2].tool_call_id.as_deref(), Some("call_2"));
}

#[test]
fn clean_interrupted_tool_calls_is_idempotent_and_skips_idless_calls() {
	let mut messages = vec![assistant_with_tool_calls(json!([call("call_1", "view")]))];
	assert!(clean_interrupted_tool_calls(&mut messages, "test"));
	assert!(
		!clean_interrupted_tool_calls(&mut messages, "test"),
		"second run must find nothing to do"
	);
	assert_eq!(messages.len(), 2);

	// A call without an id cannot be paired with a synthetic response.
	let mut idless = vec![Message {
		role: "assistant".to_string(),
		content: String::new(),
		tool_calls: Some(json!([{"type": "function", "function": {"name": "view"}}])),
		..Default::default()
	}];
	assert!(!clean_interrupted_tool_calls(&mut idless, "test"));
	assert_eq!(idless.len(), 1);
}

#[test]
fn clean_interrupted_tool_calls_defaults_missing_tool_name_to_unknown() {
	let mut messages = vec![assistant_with_tool_calls(json!([{
		"id": "call_9",
		"type": "function",
		"function": {},
	}]))];
	assert!(clean_interrupted_tool_calls(&mut messages, "test"));
	assert_eq!(messages[1].name.as_deref(), Some("unknown"));
}

// ---- parse_log_lines ----

#[test]
fn parse_log_lines_empty_input_yields_empty_state() {
	let parsed = parse(&[]);
	assert!(parsed.session_info.is_none());
	assert!(parsed.messages.is_empty());
	assert!(parsed.restoration_messages.is_empty());
	assert!(!parsed.restoration_point_found);
}

#[test]
fn parse_log_lines_json_summary_populates_session_info() {
	let info = test_session_info("json-summary");
	let parsed = parse(&[summary_json_line(&info, 1_700_000_500)]);
	let parsed_info = parsed.session_info.expect("session info");
	assert_eq!(parsed_info.name, "json-summary");
	assert_eq!(parsed_info.model, "test/model");
}

#[test]
fn parse_log_lines_legacy_summary_prefix_populates_session_info() {
	let info = test_session_info("legacy-summary");
	let line = format!("SUMMARY: {}", serde_json::to_string(&info).unwrap());
	let parsed = parse(&[line]);
	assert_eq!(
		parsed.session_info.expect("session info").name,
		"legacy-summary"
	);
}

#[test]
fn parse_log_lines_legacy_info_prefix_zeroes_counters() {
	let mut info = test_session_info("legacy-info");
	info.input_tokens = 1234;
	info.output_tokens = 5678;
	info.total_cost = 9.0;
	info.tool_calls = 42;
	let line = format!("INFO: {}", serde_json::to_string(&info).unwrap());
	let parsed = parse(&[line]);
	let parsed_info = parsed.session_info.expect("session info");
	assert_eq!(parsed_info.name, "legacy-info");
	assert_eq!(parsed_info.input_tokens, 0);
	assert_eq!(parsed_info.output_tokens, 0);
	assert_eq!(parsed_info.total_cost, 0.0);
	assert_eq!(parsed_info.tool_calls, 0);
}

#[test]
fn parse_log_lines_restoration_point_routes_subsequent_messages() {
	let parsed = parse(&[
		summary_json_line(&test_session_info("rp"), 1),
		msg_json_line("user", "before"),
		json!({"type": "RESTORATION_POINT"}).to_string(),
		msg_json_line("user", "after"),
	]);
	assert!(parsed.restoration_point_found);
	assert!(
		parsed.messages.is_empty(),
		"pre-restoration messages are dropped"
	);
	assert_eq!(parsed.restoration_messages.len(), 1);
	assert_eq!(parsed.restoration_messages[0].content, "after");
}

#[test]
fn parse_log_lines_legacy_restoration_point_prefix_also_resets() {
	let parsed = parse(&[
		msg_json_line("user", "before"),
		"RESTORATION_POINT: fresh start".to_string(),
		msg_json_line("assistant", "after"),
	]);
	assert!(parsed.restoration_point_found);
	assert!(parsed.messages.is_empty());
	assert_eq!(parsed.restoration_messages.len(), 1);
}

#[test]
fn parse_log_lines_compression_point_clears_messages() {
	let parsed = parse(&[
		msg_json_line("user", "compressed away"),
		json!({"type": "COMPRESSION_POINT"}).to_string(),
		msg_json_line("system", "[SNAPSHOT]"),
	]);
	assert_eq!(parsed.messages.len(), 1);
	assert_eq!(parsed.messages[0].content, "[SNAPSHOT]");
	assert!(!parsed.restoration_point_found);
}

#[test]
fn parse_log_lines_truncation_point_without_count_is_noop() {
	let parsed = parse(&[
		msg_json_line("user", "a"),
		msg_json_line("assistant", "b"),
		json!({"type": "TRUNCATION_POINT"}).to_string(),
	]);
	assert_eq!(
		parsed.messages.len(),
		2,
		"no message_count → nothing truncated"
	);
}

#[test]
fn parse_log_lines_output_mode_replace_and_restart_clear_messages() {
	let parsed = parse(&[
		msg_json_line("user", "one"),
		json!({"type": "OUTPUT_MODE_REPLACE", "command": "/compact"}).to_string(),
		msg_json_line("user", "rebuilt"),
		json!({"type": "OUTPUT_MODE_RESTART", "command": "/restart"}).to_string(),
		msg_json_line("user", "rebuilt again"),
	]);
	assert_eq!(parsed.messages.len(), 1);
	assert_eq!(parsed.messages[0].content, "rebuilt again");
}

#[test]
fn parse_log_lines_output_mode_replace_after_restoration_clears_restoration_messages() {
	let parsed = parse(&[
		json!({"type": "RESTORATION_POINT"}).to_string(),
		msg_json_line("user", "restored"),
		json!({"type": "OUTPUT_MODE_REPLACE", "command": "/compact"}).to_string(),
		msg_json_line("user", "rebuilt"),
	]);
	assert!(parsed.restoration_point_found);
	assert!(parsed.messages.is_empty());
	assert_eq!(parsed.restoration_messages.len(), 1);
	assert_eq!(parsed.restoration_messages[0].content, "rebuilt");
}

#[test]
fn parse_log_lines_output_mode_append_and_last_keep_messages() {
	let parsed = parse(&[
		msg_json_line("user", "one"),
		json!({"type": "OUTPUT_MODE_APPEND", "command": "/out append"}).to_string(),
		msg_json_line("assistant", "two"),
		json!({"type": "OUTPUT_MODE_LAST", "command": "/out last"}).to_string(),
	]);
	assert_eq!(parsed.messages.len(), 2);
}

#[test]
fn parse_log_lines_command_and_unknown_types_are_skipped() {
	let parsed = parse(&[
		json!({"type": "COMMAND", "command": "/model test"}).to_string(),
		json!({"type": "API_REQUEST", "payload": "x"}).to_string(),
		json!({"type": "SOME_FUTURE_TYPE"}).to_string(),
		msg_json_line("user", "real"),
	]);
	assert_eq!(parsed.messages.len(), 1);
	assert!(parsed.session_info.is_none());
}

#[test]
fn parse_log_lines_stats_before_any_summary_are_ignored() {
	let parsed = parse(&[json!({
		"type": "STATS",
		"timestamp": 500u64,
		"input_tokens": 10u64,
	})
	.to_string()]);
	assert!(
		parsed.session_info.is_none(),
		"no SUMMARY → nothing to update"
	);
}

#[test]
fn parse_log_lines_stats_newer_update_every_counter_upward() {
	let mut info = test_session_info("stats");
	info.input_tokens = 100;
	info.output_tokens = 200;
	info.cache_read_tokens = 300;
	info.cache_write_tokens = 400;
	info.total_cost = 0.5;
	info.tool_calls = 7;
	info.total_api_time_ms = 1_000;
	info.total_tool_time_ms = 2_000;
	info.total_layer_time_ms = 3_000;
	let parsed = parse(&[
		summary_json_line(&info, 1_000),
		json!({
			"type": "STATS",
			"timestamp": 2_000u64,
			"input_tokens": 150u64,
			"output_tokens": 250u64,
			"cache_read_tokens": 350u64,
			"cache_write_tokens": 450u64,
			"total_cost": 0.75,
			"tool_calls": 9u64,
			"total_api_time_ms": 1_500u64,
			"total_tool_time_ms": 2_500u64,
			"total_layer_time_ms": 3_500u64,
		})
		.to_string(),
	]);
	let updated = parsed.session_info.expect("session info");
	assert_eq!(updated.input_tokens, 150);
	assert_eq!(updated.output_tokens, 250);
	assert_eq!(updated.cache_read_tokens, 350);
	assert_eq!(updated.cache_write_tokens, 450);
	assert_eq!(updated.total_cost, 0.75);
	assert_eq!(updated.tool_calls, 9);
	assert_eq!(updated.total_api_time_ms, 1_500);
	assert_eq!(updated.total_tool_time_ms, 2_500);
	assert_eq!(updated.total_layer_time_ms, 3_500);
}

#[test]
fn parse_log_lines_summary_without_session_info_keeps_none() {
	let parsed = parse(&[json!({"type": "SUMMARY", "timestamp": 100u64}).to_string()]);
	assert!(parsed.session_info.is_none());
}

#[test]
fn parse_log_lines_invalid_session_info_in_summary_errors() {
	let result = parse_log_lines(Cursor::new(
		json!({"type": "SUMMARY", "session_info": {"name": 123}}).to_string(),
	));
	assert!(result.is_err());
}

#[test]
fn parse_log_lines_legacy_summary_with_invalid_json_errors() {
	let result = parse_log_lines(Cursor::new("SUMMARY: not json at all".to_string()));
	assert!(result.is_err());
}

#[test]
fn parse_log_lines_tool_call_markers_reconstruct_assistant_message() {
	let parsed = parse(&[
		msg_json_line("user", "go"),
		json!({"type": "TOOL_CALL", "tool_name": "shell", "tool_id": "c1", "parameters": {"cmd": "ls"}}).to_string(),
		serde_json::to_string(&tool_msg("c1", "shell")).unwrap(),
	]);
	assert_eq!(
		parsed.messages.len(),
		3,
		"user + reconstructed assistant + tool"
	);
	let assistant = &parsed.messages[1];
	assert_eq!(assistant.role, "assistant");
	let calls = assistant
		.tool_calls
		.as_ref()
		.expect("tool_calls")
		.as_array()
		.expect("array");
	assert_eq!(calls.len(), 1);
	assert_eq!(calls[0]["id"], "c1");
	assert_eq!(calls[0]["function"]["name"], "shell");
	assert_eq!(
		calls[0]["function"]["arguments"],
		json!("{\"cmd\":\"ls\"}"),
		"arguments are serialized to a JSON string"
	);
}

#[test]
fn parse_log_lines_tool_call_marker_without_id_is_not_collected() {
	let parsed = parse(&[
		json!({"type": "TOOL_CALL", "tool_name": "shell", "parameters": {}}).to_string(),
		serde_json::to_string(&tool_msg("c1", "shell")).unwrap(),
	]);
	assert_eq!(
		parsed.messages.len(),
		1,
		"no reconstruction without a tool_id"
	);
	assert_eq!(parsed.messages[0].role, "tool");
}

#[test]
fn parse_log_lines_tool_call_reconstruction_skipped_when_assistant_already_logged() {
	let assistant = assistant_with_tool_calls(json!([call("c1", "shell")]));
	let parsed = parse(&[
		serde_json::to_string(&assistant).unwrap(),
		json!({"type": "TOOL_CALL", "tool_name": "shell", "tool_id": "c1", "parameters": {}})
			.to_string(),
		serde_json::to_string(&tool_msg("c1", "shell")).unwrap(),
	]);
	assert_eq!(
		parsed.messages.len(),
		2,
		"existing assistant message must not be duplicated"
	);
	assert!(parsed.messages[0].tool_calls.is_some());
}

#[test]
fn parse_log_lines_consecutive_tool_call_markers_share_one_assistant_message() {
	let parsed = parse(&[
		json!({"type": "TOOL_CALL", "tool_name": "shell", "tool_id": "c1", "parameters": {}})
			.to_string(),
		json!({"type": "TOOL_CALL", "tool_name": "view", "tool_id": "c2", "parameters": {}})
			.to_string(),
		serde_json::to_string(&tool_msg("c1", "shell")).unwrap(),
		serde_json::to_string(&tool_msg("c2", "view")).unwrap(),
	]);
	assert_eq!(parsed.messages.len(), 3, "one assistant + two tool results");
	let calls = parsed.messages[0]
		.tool_calls
		.as_ref()
		.expect("tool_calls")
		.as_array()
		.expect("array");
	assert_eq!(calls.len(), 2);
}

#[test]
fn parse_log_lines_skips_noise_and_unparseable_lines() {
	let parsed = parse(&[
		"API_REQUEST: POST /v1/chat".to_string(),
		"TOOL_CALL: shell".to_string(),
		"TOOL_RESULT: ok".to_string(),
		"CACHE: hit".to_string(),
		"ERROR: boom".to_string(),
		"EXCHANGE: whatever".to_string(),
		String::new(),
		"plain garbage that is not json".to_string(),
		"{\"role\": \"user\", \"content\": truncated".to_string(),
		json!({"untyped": true}).to_string(),
		msg_json_line("user", "survives"),
	]);
	assert_eq!(parsed.messages.len(), 1);
	assert_eq!(parsed.messages[0].content, "survives");
	assert!(parsed.session_info.is_none());
}

// ---- list_available_sessions ----

#[test]
#[serial_test::serial]
fn list_available_sessions_empty_directory_returns_empty() {
	let _guard = DataDirGuard::new();
	let sessions = list_available_sessions().expect("list sessions");
	assert!(sessions.is_empty());
}

#[test]
#[serial_test::serial]
fn list_available_sessions_collects_json_and_legacy_summaries_newest_first() {
	let _guard = DataDirGuard::new();

	let mut newer = test_session_info("s-json");
	newer.created_at = 2_000;
	write_named_session("s-json", &[summary_json_line(&newer, 2_000)]);

	let mut older = test_session_info("s-legacy");
	older.created_at = 1_000;
	write_named_session(
		"s-legacy",
		&[format!(
			"SUMMARY: {}",
			serde_json::to_string(&older).unwrap()
		)],
	);

	// A .zst file without a SUMMARY in the first 10 lines is not a session.
	write_named_session("s-nosummary", &vec![msg_json_line("user", "x"); 10]);

	// Non-zst files are ignored entirely.
	let dir = get_sessions_dir().expect("sessions dir");
	std::fs::write(dir.join("s-plain.txt"), "SUMMARY: {}").expect("write plain file");

	let sessions = list_available_sessions().expect("list sessions");
	assert_eq!(sessions.len(), 2);
	assert_eq!(sessions[0].0, "s-json", "sorted newest first");
	assert_eq!(sessions[0].1.created_at, 2_000);
	assert_eq!(sessions[1].0, "s-legacy");
	assert_eq!(sessions[1].1.created_at, 1_000);
}

// ---- find_most_recent_session_for_project ----

#[test]
#[serial_test::serial]
fn find_most_recent_session_empty_project_basename_returns_none() {
	let _guard = DataDirGuard::new();
	write_named_session(
		"260101-120000-myproj-aaaa",
		&[summary_json_line(&test_session_info("x"), 1)],
	);
	let result = find_most_recent_session_for_project(Path::new("/")).expect("find");
	assert_eq!(result, None);
}

#[test]
#[serial_test::serial]
fn find_most_recent_session_without_matches_returns_none() {
	let _guard = DataDirGuard::new();
	write_named_session(
		"260101-120000-otherproj-aaaa",
		&[summary_json_line(&test_session_info("x"), 1)],
	);
	let result = find_most_recent_session_for_project(Path::new("/work/myproj")).expect("find");
	assert_eq!(result, None);
}

#[test]
#[serial_test::serial]
fn find_most_recent_session_matches_dash_delimited_segment_only() {
	let _guard = DataDirGuard::new();
	write_named_session(
		"260101-120000-myapp-aaaa",
		&[summary_json_line(&test_session_info("x"), 1)],
	);
	write_named_session(
		"260101-120001-application-bbbb",
		&[summary_json_line(&test_session_info("x"), 1)],
	);

	// "app" must NOT match "myapp" or "application" — segment match, not substring.
	assert_eq!(
		find_most_recent_session_for_project(Path::new("/work/app")).expect("find"),
		None
	);
	// The full segment does match.
	assert_eq!(
		find_most_recent_session_for_project(Path::new("/work/myapp")).expect("find"),
		Some("260101-120000-myapp-aaaa".to_string())
	);
}

#[test]
#[serial_test::serial]
fn find_most_recent_session_returns_latest_modification_time() {
	let _guard = DataDirGuard::new();
	let older = write_named_session(
		"260101-120000-proj-old",
		&[summary_json_line(&test_session_info("x"), 1)],
	);
	let newer = write_named_session(
		"260101-120001-proj-new",
		&[summary_json_line(&test_session_info("x"), 1)],
	);

	set_mtime(&older, 1_000);
	set_mtime(&newer, 2_000);

	assert_eq!(
		find_most_recent_session_for_project(Path::new("/work/proj")).expect("find"),
		Some("260101-120001-proj-new".to_string())
	);
}

// ---- append_to_session_file ----

#[test]
fn append_to_session_file_flattens_embedded_newlines() {
	let tmp = tempfile::Builder::new()
		.suffix(".jsonl.zst")
		.tempfile()
		.expect("tempfile");
	let path = tmp.path().to_path_buf();

	append_to_session_file(&path, "line1\nline2\r\nend").expect("append");

	let file = std::fs::File::open(&path).expect("open");
	let reader = std::io::BufReader::new(zstd::stream::read::Decoder::new(file).expect("decoder"));
	let lines: Vec<String> = reader.lines().map(|l| l.expect("line")).collect();
	assert_eq!(lines, vec!["line1 line2  end".to_string()]);
}

// ---- restore_session_info (load_session without SUMMARY) ----

#[test]
fn load_session_without_summary_synthesizes_default_info() {
	let dir = tempfile::tempdir().expect("tempdir");
	let path = dir.path().join("fallback.jsonl.zst");
	append_to_session_file(&path, &msg_json_line("user", "hello")).expect("append");

	let session = load_session(&path).expect("load session");
	assert_eq!(
		session.info.name, "fallback",
		"name derives from the file stem"
	);
	assert_eq!(session.info.model, "openrouter:anthropic/claude-sonnet-4");
	assert_eq!(session.messages.len(), 1);
	assert_eq!(session.messages[0].content, "hello");
}

#[test]
fn load_session_without_summary_recovers_stats_entries() {
	let dir = tempfile::tempdir().expect("tempdir");
	let path = dir.path().join("stats-recovery.jsonl.zst");
	append_to_session_file(&path, &msg_json_line("user", "hi")).expect("append");
	append_to_session_file(
		&path,
		&json!({
			"type": "STATS",
			"input_tokens": 5u64,
			"total_cost": 0.25,
			"tool_calls": 3u64,
		})
		.to_string(),
	)
	.expect("append");
	append_to_session_file(
		&path,
		&json!({"type": "STATS", "input_tokens": 9u64}).to_string(),
	)
	.expect("append");

	let session = load_session(&path).expect("load session");
	assert_eq!(session.info.input_tokens, 9, "last STATS wins in recovery");
	assert_eq!(session.info.total_cost, 0.25);
	assert_eq!(session.info.tool_calls, 3);
}

// ---- resume_role ----

#[test]
#[serial_test::serial]
fn resume_role_missing_session_returns_none() {
	let _guard = DataDirGuard::new();
	assert_eq!(resume_role("no-such-session"), None);
}

#[test]
#[serial_test::serial]
fn resume_role_prefers_logged_role_command_over_summary() {
	let _guard = DataDirGuard::new();
	write_named_session(
		"role-cmd",
		&[
			summary_json_line(&test_session_info("role-cmd"), 1),
			json!({"type": "COMMAND", "command": "/role developer:general"}).to_string(),
		],
	);
	assert_eq!(
		resume_role("role-cmd"),
		Some("developer:general".to_string())
	);
}

#[test]
#[serial_test::serial]
fn resume_role_falls_back_to_summary_role() {
	let _guard = DataDirGuard::new();
	let mut info = test_session_info("role-sum");
	info.role = "analyst".to_string();
	write_named_session("role-sum", &[summary_json_line(&info, 1)]);
	assert_eq!(resume_role("role-sum"), Some("analyst".to_string()));

	// An empty role in the SUMMARY is not a role.
	let mut empty = test_session_info("role-empty");
	empty.role = String::new();
	write_named_session("role-empty", &[summary_json_line(&empty, 1)]);
	assert_eq!(resume_role("role-empty"), None);
}

// ---- summary_log_entry ----

#[test]
fn summary_log_entry_snapshots_type_timestamp_and_session_info() {
	let info = test_session_info("snapshot");
	let entry = summary_log_entry(&info);

	assert_eq!(entry["type"], "SUMMARY");
	assert_eq!(entry["session_info"]["name"], "snapshot");
	assert_eq!(entry["session_info"]["model"], "test/model");
	assert_eq!(entry["session_info"]["created_at"], 1_700_000_000u64);
	assert!(
		entry["timestamp"].as_u64().is_some(),
		"timestamp must serialize as seconds"
	);
}
