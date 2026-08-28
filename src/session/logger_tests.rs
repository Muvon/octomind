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
use std::io::BufRead;

/// Point `OCTOMIND_DATA_DIR` at a fresh tempdir for the duration of a test,
/// restoring the previous value on drop. Tests using it must be `#[serial]`
/// because env vars are process-global.
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

/// Decode the multi-frame zstd session log and parse each JSONL line.
/// Mirrors how `persistence` reads session files back on load.
fn read_session_lines(session_name: &str) -> Vec<serde_json::Value> {
	let path = get_session_log_file(session_name).unwrap();
	let file = std::fs::File::open(path).unwrap();
	let reader = std::io::BufReader::new(zstd::stream::read::Decoder::new(file).unwrap());
	reader
		.lines()
		.map(|l| serde_json::from_str(&l.expect("readable line")).expect("valid JSON line"))
		.collect()
}

/// Minimal Message via serde — only the required fields; the rest default.
fn msg(role: &str, content: &str) -> Message {
	serde_json::from_value(serde_json::json!({
		"role": role,
		"content": content,
		"timestamp": 1_700_000_000,
	}))
	.expect("valid Message")
}

#[test]
#[serial_test::serial]
fn session_log_file_ends_in_jsonl_zst() {
	let _guard = DataDirGuard::new();
	let path = get_session_log_file("s").unwrap();
	assert!(path.to_string_lossy().ends_with(".jsonl.zst"));
}

#[test]
#[serial_test::serial]
fn session_log_path_alias_matches_log_file() {
	let _guard = DataDirGuard::new();
	assert_eq!(
		get_session_log_path("s").unwrap(),
		get_session_log_file("s").unwrap()
	);
}

#[test]
#[serial_test::serial]
fn restoration_point_writes_marker_with_payload() {
	let _guard = DataDirGuard::new();
	log_restoration_point("s", "do the thing", "did the thing").unwrap();
	let lines = read_session_lines("s");
	assert_eq!(lines.len(), 1);
	assert_eq!(lines[0]["type"], "RESTORATION_POINT");
	assert_eq!(lines[0]["user_message"], "do the thing");
	assert_eq!(lines[0]["assistant_response"], "did the thing");
	assert!(lines[0]["timestamp"].is_u64());
}

#[test]
#[serial_test::serial]
fn compression_point_writes_marker_then_message_snapshot() {
	let _guard = DataDirGuard::new();
	let messages = [msg("user", "hello"), msg("assistant", "hi")];
	log_compression_point("s", "conversation", 12, 3_000, &messages).unwrap();
	let lines = read_session_lines("s");
	assert_eq!(lines.len(), 3);
	assert_eq!(lines[0]["type"], "COMPRESSION_POINT");
	assert_eq!(lines[0]["compression_type"], "conversation");
	assert_eq!(lines[0]["messages_removed"], 12);
	assert_eq!(lines[0]["tokens_saved"], 3_000);
	// Post-compression snapshot follows the marker, in order.
	assert_eq!(lines[1]["role"], "user");
	assert_eq!(lines[1]["content"], "hello");
	assert_eq!(lines[2]["role"], "assistant");
	assert_eq!(lines[2]["content"], "hi");
}

#[test]
#[serial_test::serial]
fn knowledge_entry_writes_content() {
	let _guard = DataDirGuard::new();
	log_knowledge_entry("s", "the cache lives in ~/.cache/foo").unwrap();
	let lines = read_session_lines("s");
	assert_eq!(lines.len(), 1);
	assert_eq!(lines[0]["type"], "KNOWLEDGE_ENTRY");
	assert_eq!(lines[0]["content"], "the cache lives in ~/.cache/foo");
}

#[test]
#[serial_test::serial]
fn session_command_writes_command_line() {
	let _guard = DataDirGuard::new();
	log_session_command("s", "/model anthropic:claude-sonnet-4-5").unwrap();
	let lines = read_session_lines("s");
	assert_eq!(lines.len(), 1);
	assert_eq!(lines[0]["type"], "COMMAND");
	assert_eq!(lines[0]["command"], "/model anthropic:claude-sonnet-4-5");
}

#[test]
#[serial_test::serial]
fn plan_snapshot_writes_serialized_plan() {
	let _guard = DataDirGuard::new();
	let plan = ExecutionPlan {
		title: "ship it".to_string(),
		tasks: vec![],
		current_task_index: 0,
		created_at: chrono::Utc::now(),
		status: crate::mcp::core::plan::storage::PlanStatus::Active,
	};
	log_plan_snapshot("s", &plan).unwrap();
	let lines = read_session_lines("s");
	assert_eq!(lines.len(), 1);
	assert_eq!(lines[0]["type"], "PLAN_SNAPSHOT");
	assert_eq!(lines[0]["plan"]["title"], "ship it");
}

#[test]
#[serial_test::serial]
fn plan_cleared_writes_marker() {
	let _guard = DataDirGuard::new();
	log_plan_cleared("s").unwrap();
	let lines = read_session_lines("s");
	assert_eq!(lines.len(), 1);
	assert_eq!(lines[0]["type"], "PLAN_CLEARED");
}

#[test]
#[serial_test::serial]
fn schedule_snapshot_writes_entries() {
	let _guard = DataDirGuard::new();
	let entry = ScheduleEntry::new(
		"check builds".to_string(),
		"run cargo check".to_string(),
		chrono::Local::now(),
		None,
	);
	log_schedule_snapshot("s", &[entry]).unwrap();
	let lines = read_session_lines("s");
	assert_eq!(lines.len(), 1);
	assert_eq!(lines[0]["type"], "SCHEDULE_SNAPSHOT");
	let entries = lines[0]["entries"].as_array().unwrap();
	assert_eq!(entries.len(), 1);
	assert_eq!(entries[0]["description"], "check builds");
}

#[test]
#[serial_test::serial]
fn schedule_snapshot_with_empty_entries_is_meaningful() {
	let _guard = DataDirGuard::new();
	log_schedule_snapshot("s", &[]).unwrap();
	let lines = read_session_lines("s");
	assert_eq!(lines.len(), 1);
	assert_eq!(lines[0]["type"], "SCHEDULE_SNAPSHOT");
	// An empty entries vec records that the store was cleared — still present.
	assert_eq!(lines[0]["entries"].as_array().unwrap().len(), 0);
}

#[test]
fn timestamp_is_unix_epoch_seconds() {
	let now = get_timestamp();
	// Sanity window: after 2023, before 2100 — catches ms/µs mixups.
	assert!(now > 1_700_000_000);
	assert!(now < 4_102_444_800);
}
