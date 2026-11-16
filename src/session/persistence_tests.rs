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
use serde_json::json;
use tempfile::NamedTempFile;

// ---- helpers ----

fn msg(role: &str, content: &str) -> Message {
	Message {
		role: role.to_string(),
		content: content.to_string(),
		timestamp: 1_700_000_000,
		cached: false,
		cache_ttl: None,
		tool_call_id: None,
		name: None,
		tool_calls: None,
		provider_items: None,
		images: None,
		videos: None,
		thinking: None,
		id: None,
	}
}

fn assistant_with_tool_calls(content: &str, tool_calls: serde_json::Value) -> Message {
	Message {
		role: "assistant".to_string(),
		content: content.to_string(),
		timestamp: 1_700_000_000,
		cached: false,
		cache_ttl: None,
		tool_call_id: None,
		name: None,
		tool_calls: Some(tool_calls),
		provider_items: None,
		images: None,
		videos: None,
		thinking: None,
		id: None,
	}
}

fn tool_response(call_id: &str, name: &str, content: &str) -> Message {
	Message {
		role: "tool".to_string(),
		content: content.to_string(),
		timestamp: 1_700_000_000,
		cached: false,
		cache_ttl: None,
		tool_call_id: Some(call_id.to_string()),
		name: Some(name.to_string()),
		tool_calls: None,
		provider_items: None,
		images: None,
		videos: None,
		thinking: None,
		id: None,
	}
}

/// Build a SUMMARY line for the session file.
fn summary_line(name: &str, timestamp: u64) -> String {
	let info = SessionInfo {
		name: name.to_string(),
		created_at: timestamp,
		model: "test/model".to_string(),
		..Default::default()
	};
	serde_json::to_string(&json!({
		"type": "SUMMARY",
		"timestamp": timestamp,
		"session_info": info,
	}))
	.unwrap()
}

fn compression_point_line(kind: &str, removed: u64, tokens: u64, ts: u64) -> String {
	serde_json::to_string(&json!({
		"type": "COMPRESSION_POINT",
		"timestamp": ts,
		"compression_type": kind,
		"messages_removed": removed,
		"tokens_saved": tokens,
	}))
	.unwrap()
}

fn restoration_point_line(user: &str, assistant: &str, ts: u64) -> String {
	serde_json::to_string(&json!({
		"type": "RESTORATION_POINT",
		"timestamp": ts,
		"user_message": user,
		"assistant_response": assistant,
	}))
	.unwrap()
}

fn truncation_point_line(count: u64, ts: u64) -> String {
	serde_json::to_string(&json!({
		"type": "TRUNCATION_POINT",
		"timestamp": ts,
		"message_count": count,
	}))
	.unwrap()
}

fn tool_call_marker_line(tool_name: &str, tool_id: &str, params: serde_json::Value) -> String {
	serde_json::to_string(&json!({
		"type": "TOOL_CALL",
		"timestamp": 1_700_000_000u64,
		"tool_name": tool_name,
		"tool_id": tool_id,
		"parameters": params,
	}))
	.unwrap()
}

/// Write a session file with the provided line slice in zstd format and return a TempFile
/// keeping it alive for the test duration.
fn write_session(lines: &[&str]) -> NamedTempFile {
	let file = tempfile::Builder::new()
		.suffix(".jsonl.zst")
		.tempfile()
		.expect("tempfile");
	for line in lines {
		let append_file = std::fs::OpenOptions::new()
			.append(true)
			.open(file.path())
			.expect("open for append");
		let mut encoder = zstd::stream::write::Encoder::new(append_file, 1).expect("encoder");
		encoder.write_all(line.as_bytes()).expect("write");
		encoder.write_all(b"\n").expect("newline");
		encoder.finish().expect("finish");
	}
	file
}

// ---- tests ----

#[test]
fn round_trip_without_compression_preserves_messages() {
	let s = summary_line("round-trip", 1_700_000_000);
	let m1 = serde_json::to_string(&msg("user", "hi")).unwrap();
	let m2 = serde_json::to_string(&msg("assistant", "hello")).unwrap();
	let m3 = serde_json::to_string(&msg("user", "how are you?")).unwrap();
	let m4 = serde_json::to_string(&msg("assistant", "fine")).unwrap();

	let file = write_session(&[&s, &m1, &m2, &m3, &m4]);
	let session = load_session(&file.path().to_path_buf()).expect("load");

	assert_eq!(session.messages.len(), 4);
	assert_eq!(session.messages[0].role, "user");
	assert_eq!(session.messages[0].content, "hi");
	assert_eq!(session.messages[3].content, "fine");
	assert_eq!(session.info.name, "round-trip");
}

#[test]
fn provider_items_survive_session_round_trip_and_default_for_legacy_messages() {
	let s = summary_line("provider-items", 1_700_000_000);
	let items = json!([
		{"type": "reasoning", "id": "rs_1", "encrypted_content": "opaque", "summary": []},
		{"type": "message", "role": "assistant", "phase": "final_answer", "content": [{"type": "output_text", "text": "done"}]}
	]);
	let mut assistant = msg("assistant", "done");
	assistant.provider_items = Some(items.clone());
	let current = serde_json::to_string(&assistant).unwrap();
	let legacy = json!({
		"role": "assistant",
		"content": "old",
		"timestamp": 1_700_000_001,
		"cached": false
	})
	.to_string();

	let file = write_session(&[&s, &current, &legacy]);
	let session = load_session(&file.path().to_path_buf()).expect("load");

	assert_eq!(session.messages[0].provider_items.as_ref(), Some(&items));
	assert!(session.messages[1].provider_items.is_none());
}

#[test]
fn compression_clears_pre_marker_messages_and_keeps_post_marker() {
	// Before compression: user, assistant, user, assistant (wiped on reload)
	// After marker: compressed summary + preserved tail
	let s = summary_line("comp-basic", 1_700_000_000);
	let pre1 = serde_json::to_string(&msg("user", "old1")).unwrap();
	let pre2 = serde_json::to_string(&msg("assistant", "old2")).unwrap();
	let pre3 = serde_json::to_string(&msg("user", "old3")).unwrap();
	let pre4 = serde_json::to_string(&msg("assistant", "old4")).unwrap();
	let cp = compression_point_line("task", 2, 1000, 1_700_000_100);
	let post1 = serde_json::to_string(&msg("system", "[COMPRESSED SUMMARY]")).unwrap();
	let post2 = serde_json::to_string(&msg("user", "old3")).unwrap();
	let post3 = serde_json::to_string(&msg("assistant", "old4")).unwrap();

	let file = write_session(&[&s, &pre1, &pre2, &pre3, &pre4, &cp, &post1, &post2, &post3]);
	let session = load_session(&file.path().to_path_buf()).expect("load");

	// Only post-compression messages should survive
	assert_eq!(session.messages.len(), 3);
	assert_eq!(session.messages[0].content, "[COMPRESSED SUMMARY]");
	assert_eq!(session.messages[1].content, "old3");
	assert_eq!(session.messages[2].content, "old4");
}

/// THE BUG FIX: compression as the last action before exit must still
/// preserve the post-compression snapshot. Previously the COMPRESSION_POINT
/// marker was written alone, so resume wiped everything and found nothing.
#[test]
fn compression_as_last_action_preserves_post_state() {
	let s = summary_line("comp-last", 1_700_000_000);
	let pre1 = serde_json::to_string(&msg("user", "q1")).unwrap();
	let pre2 = serde_json::to_string(&msg("assistant", "a1")).unwrap();
	let pre3 = serde_json::to_string(&msg("user", "q2")).unwrap();
	let pre4 = serde_json::to_string(&msg("assistant", "a2")).unwrap();
	let cp = compression_point_line("conversation", 3, 2000, 1_700_000_100);
	// Post-compression snapshot written by log_compression_point:
	let post_summary = serde_json::to_string(&msg("system", "[COMPRESSED]")).unwrap();
	let post_tail = serde_json::to_string(&msg("assistant", "a2")).unwrap();

	// Note: NO messages after the snapshot — this is the critical scenario.
	let file = write_session(&[&s, &pre1, &pre2, &pre3, &pre4, &cp, &post_summary, &post_tail]);
	let session = load_session(&file.path().to_path_buf()).expect("load");

	// Must NOT be empty — this was the bug.
	assert!(!session.messages.is_empty(), "resume lost post-compression snapshot");
	assert_eq!(session.messages.len(), 2);
	assert_eq!(session.messages[0].content, "[COMPRESSED]");
	assert_eq!(session.messages[1].content, "a2");
}

#[test]
fn two_consecutive_compressions_keep_only_latest_snapshot() {
	let s = summary_line("comp-two", 1_700_000_000);
	let m1 = serde_json::to_string(&msg("user", "v1")).unwrap();
	let cp1 = compression_point_line("task", 1, 500, 1_700_000_050);
	let snap1 = serde_json::to_string(&msg("system", "[SNAP1]")).unwrap();
	let mid = serde_json::to_string(&msg("user", "v2")).unwrap();
	let cp2 = compression_point_line("phase", 2, 1000, 1_700_000_100);
	let snap2 = serde_json::to_string(&msg("system", "[SNAP2]")).unwrap();
	let tail = serde_json::to_string(&msg("user", "v3")).unwrap();

	let file = write_session(&[&s, &m1, &cp1, &snap1, &mid, &cp2, &snap2, &tail]);
	let session = load_session(&file.path().to_path_buf()).expect("load");

	assert_eq!(session.messages.len(), 2);
	assert_eq!(session.messages[0].content, "[SNAP2]");
	assert_eq!(session.messages[1].content, "v3");
}

#[test]
fn restoration_point_discards_prior_messages() {
	let s = summary_line("restore", 1_700_000_000);
	let old1 = serde_json::to_string(&msg("user", "before")).unwrap();
	let old2 = serde_json::to_string(&msg("assistant", "before-reply")).unwrap();
	let rp = restoration_point_line("start fresh", "ok", 1_700_000_100);
	let new1 = serde_json::to_string(&msg("user", "after")).unwrap();
	let new2 = serde_json::to_string(&msg("assistant", "after-reply")).unwrap();

	let file = write_session(&[&s, &old1, &old2, &rp, &new1, &new2]);
	let session = load_session(&file.path().to_path_buf()).expect("load");

	assert_eq!(session.messages.len(), 2);
	assert_eq!(session.messages[0].content, "after");
	assert_eq!(session.messages[1].content, "after-reply");
}

#[test]
fn truncation_point_truncates_messages() {
	let s = summary_line("trunc", 1_700_000_000);
	let m1 = serde_json::to_string(&msg("user", "a")).unwrap();
	let m2 = serde_json::to_string(&msg("assistant", "b")).unwrap();
	let m3 = serde_json::to_string(&msg("user", "c")).unwrap();
	let m4 = serde_json::to_string(&msg("assistant", "d")).unwrap();
	let tp = truncation_point_line(2, 1_700_000_100);

	let file = write_session(&[&s, &m1, &m2, &m3, &m4, &tp]);
	let session = load_session(&file.path().to_path_buf()).expect("load");

	assert_eq!(session.messages.len(), 2);
	assert_eq!(session.messages[0].content, "a");
	assert_eq!(session.messages[1].content, "b");
}

#[test]
fn interleaved_compressions_and_messages() {
	let s = summary_line("mix", 1_700_000_000);
	let m1 = serde_json::to_string(&msg("user", "m1")).unwrap();
	let m2 = serde_json::to_string(&msg("assistant", "m2")).unwrap();
	let cp = compression_point_line("task", 2, 100, 1_700_000_050);
	let snap = serde_json::to_string(&msg("system", "[SNAP]")).unwrap();
	let m3 = serde_json::to_string(&msg("user", "after-comp-1")).unwrap();
	let m4 = serde_json::to_string(&msg("assistant", "after-comp-2")).unwrap();
	let m5 = serde_json::to_string(&msg("user", "after-comp-3")).unwrap();

	let file = write_session(&[&s, &m1, &m2, &cp, &snap, &m3, &m4, &m5]);
	let session = load_session(&file.path().to_path_buf()).expect("load");

	assert_eq!(session.messages.len(), 4);
	assert_eq!(session.messages[0].content, "[SNAP]");
	assert_eq!(session.messages[1].content, "after-comp-1");
	assert_eq!(session.messages[3].content, "after-comp-3");
}

#[test]
fn tool_calls_survive_round_trip_when_embedded_in_message() {
	let s = summary_line("tools", 1_700_000_000);
	let user = serde_json::to_string(&msg("user", "list files")).unwrap();
	let tool_calls_value = json!([{
		"id": "call_1",
		"type": "function",
		"function": {
			"name": "list_files",
			"arguments": "{\"dir\":\".\"}"
		}
	}]);
	let assistant = serde_json::to_string(&assistant_with_tool_calls("", tool_calls_value)).unwrap();
	let tool = serde_json::to_string(&tool_response("call_1", "list_files", "a.txt")).unwrap();
	let final_asst = serde_json::to_string(&msg("assistant", "done")).unwrap();

	let file = write_session(&[&s, &user, &assistant, &tool, &final_asst]);
	let session = load_session(&file.path().to_path_buf()).expect("load");

	assert_eq!(session.messages.len(), 4);
	assert_eq!(session.messages[1].role, "assistant");
	assert!(
		session.messages[1].tool_calls.is_some(),
		"tool_calls lost on round-trip"
	);
	assert_eq!(session.messages[2].role, "tool");
	assert_eq!(session.messages[2].tool_call_id.as_deref(), Some("call_1"));
}

/// TOOL_CALL markers should reconstruct an assistant message with tool_calls
/// when the assistant message itself is missing from the log (pre-existing behavior).
#[test]
fn tool_call_markers_reconstruct_assistant_when_missing() {
	let s = summary_line("tc-marker", 1_700_000_000);
	let user = serde_json::to_string(&msg("user", "go")).unwrap();
	let tc = tool_call_marker_line("list_files", "call_X", json!({"dir": "."}));
	let tool = serde_json::to_string(&tool_response("call_X", "list_files", "out")).unwrap();
	let final_asst = serde_json::to_string(&msg("assistant", "done")).unwrap();

	let file = write_session(&[&s, &user, &tc, &tool, &final_asst]);
	let session = load_session(&file.path().to_path_buf()).expect("load");

	// Expected: user, reconstructed assistant(tool_calls), tool, assistant
	assert_eq!(session.messages.len(), 4);
	assert_eq!(session.messages[1].role, "assistant");
	assert!(session.messages[1].tool_calls.is_some());
	assert_eq!(session.messages[2].role, "tool");
}

#[test]
fn stats_older_than_summary_are_ignored() {
	// STATS with timestamp < SUMMARY timestamp must not overwrite SUMMARY stats
	let summary_ts = 1_700_000_500;
	let info = SessionInfo {
		name: "stats-test".to_string(),
		created_at: 1_700_000_000,
		model: "test/model".to_string(),
		input_tokens: 9999,
		output_tokens: 8888,
		..Default::default()
	};
	let s = serde_json::to_string(&json!({
		"type": "SUMMARY",
		"timestamp": summary_ts,
		"session_info": info,
	}))
	.unwrap();
	// STATS with older timestamp — must be ignored
	let old_stats = serde_json::to_string(&json!({
		"type": "STATS",
		"timestamp": 1_700_000_100u64,
		"input_tokens": 1u64,
		"output_tokens": 1u64,
	}))
	.unwrap();
	let m = serde_json::to_string(&msg("user", "hi")).unwrap();

	let file = write_session(&[&s, &old_stats, &m]);
	let session = load_session(&file.path().to_path_buf()).expect("load");

	assert_eq!(session.info.input_tokens, 9999);
	assert_eq!(session.info.output_tokens, 8888);
}

#[test]
fn legacy_summary_without_evidence_field_loads() {
	// Session files written before EvidenceLedger persistence carry a
	// session_info without the `evidence` key — #[serde(default)] must
	// accept them so old sessions keep loading without migration.
	let s = serde_json::to_string(&json!({
		"type": "SUMMARY",
		"timestamp": 1_700_000_000u64,
		"session_info": {
			"name": "legacy-no-evidence",
			"created_at": 1_700_000_000u64,
			"model": "test/model",
			"role": "",
			"input_tokens": 5u64,
			"output_tokens": 7u64,
			"cache_read_tokens": 0u64,
			"cache_write_tokens": 0u64,
			"total_cost": 0.0,
			"duration_seconds": 0u64,
			"layer_stats": [],
		},
	}))
	.unwrap();
	let m = serde_json::to_string(&msg("user", "hi")).unwrap();

	let file = write_session(&[&s, &m]);
	let session = load_session(&file.path().to_path_buf()).expect("legacy load");

	assert_eq!(session.info.name, "legacy-no-evidence");
	assert_eq!(session.info.input_tokens, 5);
}

#[test]
fn summary_with_fields_of_the_removed_supervisor_state_still_loads() {
	// Session files written before the supervisor was removed carry its state
	// in `session_info`; the loader must skip it and keep the rest.
	let s = serde_json::to_string(&json!({
		"type": "SUMMARY",
		"timestamp": 1_700_000_000u64,
		"session_info": {
			"name": "old-supervisor-state",
			"created_at": 1_700_000_000u64,
			"model": "test/model",
			"role": "",
			"input_tokens": 5u64,
			"output_tokens": 7u64,
			"cache_read_tokens": 0u64,
			"cache_write_tokens": 0u64,
			"total_cost": 0.0,
			"duration_seconds": 0u64,
			"layer_stats": [],
			"learning_stats": {"packs": 1},
			"verification_policy": "forbidden",
			"session_rules": ["never push to main"],
			"authorization": {"blocked": 3},
			"evidence": {"entries": []},
		},
	}))
	.unwrap();
	let m = serde_json::to_string(&msg("user", "hi")).unwrap();

	let file = write_session(&[&s, &m]);
	let session = load_session(&file.path().to_path_buf()).expect("load");

	assert_eq!(session.info.name, "old-supervisor-state");
	assert_eq!(session.info.input_tokens, 5);
}

#[test]
fn stats_newer_than_summary_update_only_upward() {
	let summary_ts = 1_700_000_500;
	let info = SessionInfo {
		name: "stats-upward".to_string(),
		created_at: 1_700_000_000,
		model: "test/model".to_string(),
		input_tokens: 100,
		output_tokens: 200,
		..Default::default()
	};
	let s = serde_json::to_string(&json!({
		"type": "SUMMARY",
		"timestamp": summary_ts,
		"session_info": info,
	}))
	.unwrap();
	// Newer STATS with LOWER input_tokens (e.g. cached-only) — must NOT decrement
	let newer_stats = serde_json::to_string(&json!({
		"type": "STATS",
		"timestamp": 1_700_000_600u64,
		"input_tokens": 5u64,
		"output_tokens": 500u64,
	}))
	.unwrap();
	let m = serde_json::to_string(&msg("user", "hi")).unwrap();

	let file = write_session(&[&s, &newer_stats, &m]);
	let session = load_session(&file.path().to_path_buf()).expect("load");

	assert_eq!(session.info.input_tokens, 100, "must not decrement");
	assert_eq!(session.info.output_tokens, 500, "must update upward");
}

#[test]
fn compression_point_without_snapshot_yields_empty_messages() {
	// Regression lock: this is what the OLD buggy code produced.
	// Verifies that our fix is meaningful — i.e. without the snapshot,
	// the parser WOULD return zero messages.
	let s = summary_line("bug-repro", 1_700_000_000);
	let m1 = serde_json::to_string(&msg("user", "before")).unwrap();
	let m2 = serde_json::to_string(&msg("assistant", "reply")).unwrap();
	let cp = compression_point_line("conversation", 2, 500, 1_700_000_100);

	// No snapshot after the marker — the buggy behavior.
	let file = write_session(&[&s, &m1, &m2, &cp]);
	let session = load_session(&file.path().to_path_buf()).expect("load");

	assert_eq!(
		session.messages.len(),
		0,
		"parser must wipe on COMPRESSION_POINT; this is exactly what the bug relied on"
	);
}

#[test]
fn restoration_point_then_compression_clears_restoration_messages() {
	let s = summary_line("rp-then-comp", 1_700_000_000);
	let rp = restoration_point_line("fresh", "ok", 1_700_000_050);
	let r1 = serde_json::to_string(&msg("user", "r1")).unwrap();
	let r2 = serde_json::to_string(&msg("assistant", "r2")).unwrap();
	let cp = compression_point_line("task", 1, 100, 1_700_000_100);
	let snap = serde_json::to_string(&msg("system", "[POST-RP-COMP]")).unwrap();
	let tail = serde_json::to_string(&msg("user", "tail")).unwrap();

	let file = write_session(&[&s, &rp, &r1, &r2, &cp, &snap, &tail]);
	let session = load_session(&file.path().to_path_buf()).expect("load");

	assert_eq!(session.messages.len(), 2);
	assert_eq!(session.messages[0].content, "[POST-RP-COMP]");
	assert_eq!(session.messages[1].content, "tail");
}

#[test]
fn stats_checkpoint_restores_all_token_categories_after_summary() {
	let info = SessionInfo {
		name: "token-checkpoint".into(),
		input_tokens: 100,
		cache_read_tokens: 500,
		cache_write_tokens: 20,
		output_tokens: 40,
		reasoning_tokens: 5,
		..Default::default()
	};
	let summary = serde_json::to_string(&json!({
		"type": "SUMMARY", "timestamp": 1, "session_info": SessionInfo::default()
	}))
	.unwrap();
	let message = serde_json::to_string(&msg("user", "hi")).unwrap();
	let file = write_session(&[&summary, &message]);
	crate::session::logger::log_stats_checkpoint(file.path(), &info).unwrap();
	let loaded = load_session(&file.path().to_path_buf()).unwrap();
	assert_eq!(loaded.info.input_tokens, 100);
	assert_eq!(loaded.info.cache_read_tokens, 500);
	assert_eq!(loaded.info.cache_write_tokens, 20);
	assert_eq!(loaded.info.output_tokens, 40);
	assert_eq!(loaded.info.reasoning_tokens, 5);
	assert_eq!(loaded.info.prompt_tokens(), 620);
	assert_eq!(loaded.info.total_tokens(), 665);
}

#[test]
fn same_timestamp_checkpoints_restore_main_cost_without_double_counting() {
	let file = NamedTempFile::new().unwrap();
	let path = file.path().to_path_buf();
	let info = SessionInfo {
		name: "accounting-checkpoints".into(),
		total_cost: 1.0,
		main_cost: Some(0.25),
		..Default::default()
	};
	append_to_session_file(
		&path,
		&json!({"type":"SUMMARY", "timestamp":100, "session_info":info}).to_string(),
	)
	.unwrap();
	append_to_session_file(
		&path,
		&json!({"type":"STATS", "timestamp":100, "total_cost":1.5, "main_cost":0.5}).to_string(),
	)
	.unwrap();
	// A stale summary must not overwrite a newer main-cost checkpoint.
	append_to_session_file(
		&path,
		&json!({"type":"SUMMARY", "timestamp":100, "session_info":info}).to_string(),
	)
	.unwrap();
	let loaded = load_session(&path).unwrap();
	assert_eq!(loaded.info.main_cost, Some(0.5));
	assert_eq!(loaded.info.total_cost, 1.5);
	loaded.save().unwrap();
	assert_eq!(load_session(&path).unwrap().info.main_cost, Some(0.5));
}

#[test]
fn old_tap_receipts_survive_compaction_and_leave_main_cost_unknown() {
	let file = NamedTempFile::new().unwrap();
	let path = file.path().to_path_buf();
	let mut info = serde_json::to_value(SessionInfo {
		name: "legacy-agents".into(),
		total_cost: 1.92857,
		..Default::default()
	})
	.unwrap();
	info.as_object_mut().unwrap().remove("main_cost");
	info.as_object_mut().unwrap().remove("agent_runs");
	append_to_session_file(
		&path,
		&json!({"type":"SUMMARY", "timestamp":100, "session_info":info}).to_string(),
	)
	.unwrap();
	let receipt = json!({"id":"tap-old-child", "role":"developer:general", "workdir":"/tmp/work", "message":"started"});
	let message = tool_response("call-1", "tap", &receipt.to_string());
	for _ in 0..2 {
		append_to_session_file(&path, &serde_json::to_string(&message).unwrap()).unwrap();
	}
	append_to_session_file(&path, &json!({"type":"RESTORATION_POINT", "timestamp":101}).to_string()).unwrap();
	append_to_session_file(
		&path,
		&json!({"type":"SUMMARY", "timestamp":102, "session_info":info}).to_string(),
	)
	.unwrap();
	let loaded = load_session(&path).unwrap();
	assert!(loaded.info.main_cost.is_none());
	assert_eq!(loaded.info.total_cost, 1.92857);
	assert_eq!(loaded.info.agent_runs.len(), 1);
	assert_eq!(loaded.info.agent_runs[0].id, "tap-old-child");
	loaded.save().unwrap();
	let reloaded = load_session(&path).unwrap();
	assert_eq!(reloaded.info.agent_runs.len(), 1);
	assert!(reloaded.info.main_cost.is_none());
}

#[test]
fn unknown_main_cost_checkpoint_cannot_be_replaced_with_a_partial_known_cost() {
	let file = NamedTempFile::new().unwrap();
	let path = file.path().to_path_buf();
	let info = SessionInfo {
		name: "unpriced-main".into(),
		main_cost: Some(0.5),
		..Default::default()
	};
	append_to_session_file(
		&path,
		&json!({"type":"SUMMARY", "timestamp":100, "session_info":info}).to_string(),
	)
	.unwrap();
	append_to_session_file(
		&path,
		&json!({"type":"STATS", "timestamp":101, "main_cost":null}).to_string(),
	)
	.unwrap();
	append_to_session_file(
		&path,
		&json!({"type":"SUMMARY", "timestamp":102, "session_info":info}).to_string(),
	)
	.unwrap();
	assert!(load_session(&path).unwrap().info.main_cost.is_none());
}

#[test]
fn entry_kind_reads_the_type_without_regard_to_key_order_or_other_fields() {
	assert_eq!(entry_kind(r#"{"type":"STATS","total_cost":1.5}"#), Some("STATS"));
	assert_eq!(
		entry_kind(r#"{"timestamp":5,"session_info":{"type":"nested"},"type":"SUMMARY"}"#),
		Some("SUMMARY")
	);
	let message = serde_json::to_string(&msg("user", "hi")).unwrap();
	assert_eq!(entry_kind(&message), None, "a message line has no type");
	assert_eq!(entry_kind("SUMMARY: {\"name\":\"legacy\"}"), None);
	assert_eq!(entry_kind("[1,2]"), None);
}

#[test]
fn snapshot_entry_with_type_not_first_still_loads() {
	let info = SessionInfo {
		name: "type-last".into(),
		total_cost: 2.0,
		..Default::default()
	};
	let summary = json!({"timestamp": 100, "session_info": info, "type": "SUMMARY"}).to_string();
	let stats = json!({"total_cost": 3.0, "timestamp": 101, "type": "STATS"}).to_string();
	let file = write_session(&[&summary, &stats]);
	let loaded = load_session(&file.path().to_path_buf()).expect("load");
	assert_eq!(loaded.info.name, "type-last");
	assert_eq!(loaded.info.total_cost, 3.0);
}

#[test]
fn one_load_yields_the_runtime_state_the_log_replay_computes() {
	let info = SessionInfo {
		name: "one-pass".into(),
		model: "ollama:saved".into(),
		role: "developer:general".into(),
		..Default::default()
	};
	let lines = [
		json!({"type": "SUMMARY", "timestamp": 100, "session_info": info}).to_string(),
		json!({"type": "COMMAND", "command": "/model ollama:latest"}).to_string(),
		json!({"type": "COMMAND", "command": "/role tester:general"}).to_string(),
		json!({"type": "RESTORATION_POINT", "timestamp": 101}).to_string(),
		json!({"type": "KNOWLEDGE_ENTRY", "content": "kept"}).to_string(),
		json!({"type": "COMMAND", "command": "/model ollama:after"}).to_string(),
		json!({"type": "COMMAND", "command": "/effort high"}).to_string(),
	];
	let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
	let file = write_session(&refs);
	let path = file.path().to_path_buf();

	let loaded = load_session_with_runtime_state(&path).expect("load");
	let replayed = extract_runtime_state_from_log(&path).expect("replay");

	assert_eq!(loaded.runtime_state.model, replayed.model);
	assert_eq!(loaded.runtime_state.role, replayed.role);
	assert_eq!(loaded.runtime_state.reasoning_effort, replayed.reasoning_effort);
	assert_eq!(loaded.runtime_state.critical_knowledge, replayed.critical_knowledge);
	assert_eq!(replayed.model.as_deref(), Some("ollama:after"));
	assert_eq!(replayed.role, None, "role predates the restoration point");
	assert_eq!(replayed.critical_knowledge, vec!["kept".to_string()]);
	assert_eq!(loaded.session.info.model, "ollama:after");
}

#[test]
fn snapshot_without_main_cost_leaves_a_known_main_cost_while_null_clears_it() {
	let mut info = serde_json::to_value(SessionInfo {
		name: "main-cost-presence".into(),
		main_cost: Some(0.5),
		..Default::default()
	})
	.unwrap();
	let known = json!({"type": "SUMMARY", "timestamp": 100, "session_info": info.clone()}).to_string();
	info.as_object_mut().unwrap().remove("main_cost");
	let absent = json!({"type": "SUMMARY", "timestamp": 101, "session_info": info}).to_string();
	let absent_stats = json!({"type": "STATS", "timestamp": 102, "total_cost": 0.1}).to_string();
	let file = write_session(&[&known, &absent, &absent_stats]);
	assert_eq!(
		load_session(&file.path().to_path_buf()).unwrap().info.main_cost,
		Some(0.5)
	);

	let null_stats = json!({"type": "STATS", "timestamp": 102, "main_cost": null}).to_string();
	let file = write_session(&[&known, &null_stats]);
	assert!(load_session(&file.path().to_path_buf())
		.unwrap()
		.info
		.main_cost
		.is_none());
}

fn snapshot_run(id: &str, status: crate::session::tap_runs::TapJobStatus) -> crate::session::tap_runs::TapJobInfo {
	crate::session::tap_runs::TapJobInfo {
		id: id.to_string(),
		role: "developer:general".to_string(),
		workdir: "/tmp".to_string(),
		started_at: std::time::UNIX_EPOCH,
		status,
		error: None,
		live: crate::session::tap_runs::TapLiveState {
			last_action: None,
			usage: None,
		},
	}
}

#[test]
fn full_and_changed_only_snapshots_fold_into_the_whole_run_history() {
	use crate::session::tap_runs::TapJobStatus::{Done, Running};
	// Binaries up to 0.72 wrote every run into every snapshot; later ones write only
	// the runs that changed. One log holds both, and a restoration point between
	// them must not drop the runs that only earlier snapshots name.
	let info = |runs| SessionInfo {
		name: "mixed-snapshots".into(),
		agent_runs: runs,
		..Default::default()
	};
	let lines = [
		json!({"type": "SUMMARY", "timestamp": 100, "session_info": info(vec![snapshot_run("a", Done), snapshot_run("b", Running)])})
			.to_string(),
		json!({"type": "STATS", "timestamp": 101, "agent_runs": [snapshot_run("a", Done), snapshot_run("b", Running), snapshot_run("c", Running)]})
			.to_string(),
		json!({"type": "RESTORATION_POINT", "timestamp": 102}).to_string(),
		json!({"type": "STATS", "timestamp": 103, "agent_runs": [snapshot_run("b", Done)]}).to_string(),
		json!({"type": "SUMMARY", "timestamp": 104, "session_info": info(vec![snapshot_run("d", Running)])}).to_string(),
		json!({"type": "SUMMARY", "timestamp": 105, "session_info": info(Vec::new())}).to_string(),
	];
	let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
	let file = write_session(&refs);

	let runs = load_session(&file.path().to_path_buf()).unwrap().info.agent_runs;

	let seen: Vec<(&str, crate::session::tap_runs::TapJobStatus)> =
		runs.iter().map(|run| (run.id.as_str(), run.status)).collect();
	assert_eq!(seen, vec![("a", Done), ("b", Done), ("c", Running), ("d", Running)]);
}

#[test]
fn stats_with_a_total_cost_that_is_not_a_number_fails_the_load() {
	let info = SessionInfo {
		name: "bad-total-cost".into(),
		..Default::default()
	};
	let summary = json!({"type": "SUMMARY", "timestamp": 100, "session_info": info}).to_string();
	let stats = json!({"type": "STATS", "timestamp": 101, "total_cost": "lots"}).to_string();
	let file = write_session(&[&summary, &stats]);

	assert!(load_session(&file.path().to_path_buf()).is_err());
}
