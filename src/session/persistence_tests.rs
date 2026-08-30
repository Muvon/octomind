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
	let file = write_session(&[
		&s,
		&pre1,
		&pre2,
		&pre3,
		&pre4,
		&cp,
		&post_summary,
		&post_tail,
	]);
	let session = load_session(&file.path().to_path_buf()).expect("load");

	// Must NOT be empty — this was the bug.
	assert!(
		!session.messages.is_empty(),
		"resume lost post-compression snapshot"
	);
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
	let assistant =
		serde_json::to_string(&assistant_with_tool_calls("", tool_calls_value)).unwrap();
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
	assert!(session.info.evidence.render().is_empty());
}

#[test]
fn summary_with_evidence_restores_ledger_on_load() {
	// The still-open turn's recorded actions must survive a restart: a
	// SUMMARY carrying ledger state restores it instead of default().
	let mut info = SessionInfo {
		name: "evidence-restore".to_string(),
		created_at: 1_700_000_000,
		model: "test/model".to_string(),
		..Default::default()
	};
	info.evidence.record(
		"knowledge",
		&json!({"source":"https://docs.x.ai/developers/pricing"}),
		false,
		false,
		1024,
	);
	let s = serde_json::to_string(&json!({
		"type": "SUMMARY",
		"timestamp": 1_700_000_000u64,
		"session_info": info,
	}))
	.unwrap();
	let m = serde_json::to_string(&msg("user", "hi")).unwrap();

	let file = write_session(&[&s, &m]);
	let session = load_session(&file.path().to_path_buf()).expect("load");

	assert_eq!(session.info.evidence.render(), info.evidence.render());
	assert!(session.info.evidence.render().contains("docs.x.ai"));
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
