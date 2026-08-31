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

//! Tests for the agent adapter behind `/status agents`:
//! the list view, the unknown-id detail error, and the stats aggregation
//! used by `/info`.

use super::*;
use crate::session::tap_runs::{TapJob, TapJobStatus, TapLiveState, TapLiveUsage};
use std::sync::{Arc, RwLock};
use std::time::SystemTime;
use tokio::sync::watch;

#[test]
fn test_agents_list_view() {
	// Registry contents depend on what other tests in this process ran —
	// only the output shape is stable: a list view with no detail card.
	let data = build_agents_status(&[]).expect("status");
	assert_eq!(data["view"], "agents");
	assert!(data["detail"].is_null());
}

#[test]
fn test_agents_unknown_id_is_an_error() {
	let data = build_agents_status(&["no-such-agent-id"]).expect("status");
	assert_eq!(data["view"], "error");
	assert!(data["message"]
		.as_str()
		.unwrap_or_default()
		.contains("no-such-agent-id"));
}

#[test]
fn test_agents_stats_shape_when_present() {
	// Other tests in this process may have recorded runs; when stats exist
	// they must carry the aggregate keys /info renders.
	if let Some(stats) = get_agents_stats() {
		assert!(stats.get("total").is_some(), "{stats}");
	}
}

fn job(id: &str, status: TapJobStatus, live: TapLiveState) -> TapJob {
	let (cancel_tx, _cancel_rx) = watch::channel(false);
	TapJob {
		id: id.to_string(),
		role: "developer:general".to_string(),
		workdir: "/tmp/project".to_string(),
		started_at: SystemTime::now(),
		status: Arc::new(RwLock::new(status)),
		cancel_tx,
		live: Arc::new(RwLock::new(live)),
	}
}

#[tokio::test]
#[serial_test::serial]
async fn list_detail_and_stats_cover_every_job_status() {
	let session_id = "agents-command-statuses".to_string();
	crate::session::context::with_session_id(session_id.clone(), async {
		crate::session::tap_runs::init_for_session();
		crate::session::tap_runs::register_job(job(
			"running-agent",
			TapJobStatus::Running,
			TapLiveState {
				last_action: Some("shell cargo test".to_string()),
				usage: Some(TapLiveUsage {
					input_tokens: 100,
					output_tokens: 20,
					cache_read_tokens: 50,
					cost: 0.25,
				}),
			},
		));
		crate::session::tap_runs::register_job(job(
			"done-agent",
			TapJobStatus::Done,
			TapLiveState::default(),
		));
		crate::session::tap_runs::register_job(job(
			"failed-agent",
			TapJobStatus::Failed,
			TapLiveState::default(),
		));
		crate::session::tap_runs::register_job(job(
			"cancelled-agent",
			TapJobStatus::Cancelled,
			TapLiveState::default(),
		));

		let data = build_agents_status(&[]).unwrap();
		let running = data["running"].as_array().expect("running");
		let finished = data["finished"].as_array().expect("finished");
		assert_eq!(data["total"], 4);
		assert_eq!(running.len(), 1);
		assert_eq!(finished.len(), 3);
		assert_eq!(running[0]["last_action"], "shell cargo test");
		assert_eq!(running[0]["tokens_input"], 100);

		let data = build_agents_status(&["running-agent"]).unwrap();
		let detail = &data["detail"];
		assert_eq!(data["total"], 1);
		assert_eq!(detail["status"], "running");
		assert_eq!(detail["tokens_cached"], 50);
		assert_eq!(detail["cost"], 0.25);

		let stats = get_agents_stats().expect("stats");
		assert_eq!(stats["total"], 4);
		assert_eq!(stats["running"], 1);
		assert_eq!(stats["done"], 1);
		assert_eq!(stats["failed"], 1);

		crate::session::tap_runs::clear_for_session(&session_id);
	})
	.await;
}

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

#[test]
fn truncate_flattens_and_caps() {
	assert_eq!(truncate("one line", 20), "one line");
	let long = "word ".repeat(30);
	let cut = truncate(&long, 20);
	// The cap is in characters; the ellipsis is multi-byte.
	assert!(cut.chars().count() <= 20, "{cut}");
	assert!(cut.ends_with('…'), "{cut}");
	let flattened = truncate("a\nb\nc", 20);
	assert_eq!(flattened, "a b c");
}

#[test]
fn summarize_tool_picks_hint_from_preferred_keys() {
	// String-encoded arguments (the wire format) with a preferred key.
	let args = serde_json::json!(r#"{"file_path":"src/main.rs","other":"x"}"#);
	assert_eq!(summarize_tool("view", Some(&args)), "view src/main.rs");
	// Object arguments work too.
	let args = serde_json::json!({"command":"cargo test"});
	assert_eq!(summarize_tool("shell", Some(&args)), "shell cargo test");
	// No recognizable key → bare tool name.
	let args = serde_json::json!({"unrelated": 1});
	assert_eq!(summarize_tool("shell", Some(&args)), "shell");
	// Missing arguments entirely.
	assert_eq!(summarize_tool("plan", None), "plan");
}

#[test]
fn last_action_from_message_variants() {
	let msg: crate::session::Message = serde_json::from_value(serde_json::json!({
		"role": "user",
		"content": "hello",
		"timestamp": 1,
	}))
	.expect("user message");
	assert_eq!(last_action_from_message(&msg), None);

	let msg: crate::session::Message = serde_json::from_value(serde_json::json!({
		"role": "assistant",
		"content": "",
		"timestamp": 2,
	}))
	.expect("empty assistant");
	assert_eq!(last_action_from_message(&msg), None);

	let msg: crate::session::Message = serde_json::from_value(serde_json::json!({
		"role": "assistant",
		"content": "Final answer",
		"timestamp": 3,
	}))
	.expect("text assistant");
	assert_eq!(
		last_action_from_message(&msg),
		Some("Final answer".to_string())
	);

	let msg: crate::session::Message = serde_json::from_value(serde_json::json!({
		"role": "assistant",
		"content": "",
		"timestamp": 4,
		"tool_calls": [
			{"function": {"name": "view", "arguments": "{\"file_path\":\"src/a.rs\"}"}}
		]
	}))
	.expect("tool assistant");
	assert_eq!(
		last_action_from_message(&msg),
		Some("view src/a.rs".to_string())
	);
}

#[test]
fn elapsed_secs_saturates_on_future_clock() {
	assert!(elapsed_secs(SystemTime::now()) <= 1);
	// A start time in the future (clock skew) must not panic or wrap.
	assert_eq!(
		elapsed_secs(SystemTime::now() + std::time::Duration::from_secs(600)),
		0
	);
	// The epoch is far in the past — just needs to be large, not exact.
	assert!(elapsed_secs(std::time::UNIX_EPOCH) > 1_000_000_000);
}

// ---------------------------------------------------------------------------
// Snapshot reading (sandboxed data dir)
// ---------------------------------------------------------------------------

const DATA_DIR_KEY: &str = "OCTOMIND_DATA_DIR";

/// Snapshot env vars and restore them on drop.
struct EnvGuard(Vec<(&'static str, Option<std::ffi::OsString>)>);

impl EnvGuard {
	fn new(keys: &[&'static str]) -> Self {
		Self(keys.iter().map(|k| (*k, std::env::var_os(k))).collect())
	}
}

impl Drop for EnvGuard {
	fn drop(&mut self) {
		for (key, saved) in &self.0 {
			match saved {
				Some(v) => std::env::set_var(key, v),
				None => std::env::remove_var(key),
			}
		}
	}
}

fn sandbox(tag: &str) -> std::path::PathBuf {
	let dir = std::env::temp_dir().join(format!("octomind-agents-{tag}-{}", std::process::id()));
	if dir.exists() {
		std::fs::remove_dir_all(&dir).expect("clear stale sandbox data dir");
	}
	std::fs::create_dir_all(&dir).expect("create sandbox data dir");
	dir
}

/// Write a zstd-framed session log with one JSON line per entry.
fn write_snapshot(path: &std::path::Path, lines: &[String]) {
	let mut body = String::new();
	for line in lines {
		body.push_str(line);
		body.push('\n');
	}
	let compressed =
		zstd::encode_all(std::io::Cursor::new(body.as_bytes()), 3).expect("encode zst");
	std::fs::write(path, compressed).expect("write snapshot");
}

fn summary_line() -> String {
	serde_json::json!({
		"type": "SUMMARY",
		"session_info": {
			"name": "cov-agents",
			"created_at": 1,
			"model": "cov-model",
			"role": "developer",
			"input_tokens": 11,
			"output_tokens": 7,
			"cache_read_tokens": 5,
			"cache_write_tokens": 3,
			"total_cost": 0.5,
			"duration_seconds": 9,
			"layer_stats": [],
			"tool_calls": 4
		}
	})
	.to_string()
}

#[test]
#[serial_test::serial]
fn read_agent_snapshot_parses_summary_and_last_action() {
	let _env = EnvGuard::new(&[DATA_DIR_KEY]);
	let data_dir = sandbox("snapshot");
	std::env::set_var(DATA_DIR_KEY, &data_dir);
	let sessions_dir = crate::directories::get_sessions_dir().expect("sessions dir");
	std::fs::create_dir_all(&sessions_dir).expect("create sessions dir");

	write_snapshot(
		&sessions_dir.join("cov-agents-snap.jsonl.zst"),
		&[
			summary_line(),
			// Tool-call step: last_action becomes "view src/a.rs".
			r#"{"role":"assistant","content":"","timestamp":2,"tool_calls":[{"function":{"name":"view","arguments":"{\"file_path\":\"src/a.rs\"}"}}]}"#.to_string(),
			// Restoration point invalidates prior actions.
			r#"{"type":"RESTORATION_POINT"}"#.to_string(),
			// Malformed and blank lines are skipped tolerantly.
			"not json".to_string(),
			String::new(),
			// Non-assistant messages never set last_action.
			r#"{"role":"user","content":"hi","timestamp":3}"#.to_string(),
			// Final assistant text wins.
			r#"{"role":"assistant","content":"Final answer","timestamp":4}"#.to_string(),
		],
	);

	let snap = read_agent_snapshot("cov-agents-snap");
	let info = snap.info.expect("summary parsed");
	assert_eq!(info.model, "cov-model");
	assert_eq!(info.input_tokens, 11);
	assert_eq!(info.output_tokens, 7);
	assert_eq!(info.cache_read_tokens, 5);
	assert_eq!(info.total_cost, 0.5);
	assert_eq!(info.tool_calls, 4);
	assert_eq!(snap.last_action.as_deref(), Some("Final answer"));

	// Missing file → empty snapshot, and ago_secs reports None.
	let missing = read_agent_snapshot("cov-agents-no-file");
	assert!(missing.info.is_none());
	assert!(missing.last_action.is_none());
	assert_eq!(ago_secs("cov-agents-no-file"), None);
	let ago = ago_secs("cov-agents-snap").expect("ago for existing file");
	assert!(ago <= 2, "freshly written file reports {ago}s ago");
}

#[tokio::test]
#[serial_test::serial]
async fn detail_card_uses_snapshot_for_finished_job() {
	let _env = EnvGuard::new(&[DATA_DIR_KEY]);
	let data_dir = sandbox("detail");
	std::env::set_var(DATA_DIR_KEY, &data_dir);
	let sessions_dir = crate::directories::get_sessions_dir().expect("sessions dir");
	std::fs::create_dir_all(&sessions_dir).expect("create sessions dir");
	write_snapshot(
		&sessions_dir.join("cov-agents-detail.jsonl.zst"),
		&[
			summary_line(),
			r#"{"role":"assistant","content":"Final answer","timestamp":4}"#.to_string(),
		],
	);

	let session_id = "agents-command-detail".to_string();
	crate::session::context::with_session_id(session_id.clone(), async {
		crate::session::tap_runs::init_for_session();
		crate::session::tap_runs::register_job(job(
			"cov-agents-detail",
			TapJobStatus::Done,
			TapLiveState::default(),
		));

		// Detail: finished job → snapshot is authoritative.
		let data = build_agents_status(&["cov-agents-detail"]).unwrap();
		let detail = &data["detail"];
		assert_eq!(data["total"], 1);
		assert_eq!(detail["status"], "done");
		assert_eq!(detail["model"], "cov-model");
		assert_eq!(detail["tokens_input"], 11);
		assert_eq!(detail["tokens_output"], 7);
		assert_eq!(detail["tokens_cached"], 5);
		assert_eq!(detail["cost"], 0.5);
		assert_eq!(detail["tool_calls"], 4);
		assert_eq!(detail["last_action"], "Final answer");

		// List: the finished row carries ago_secs from the file's mtime.
		let data = build_agents_status(&[]).unwrap();
		let finished = data["finished"].as_array().expect("finished");
		let row = finished
			.iter()
			.find(|r| r["id"] == "cov-agents-detail")
			.unwrap_or_else(|| panic!("job missing from finished list: {finished:?}"));
		assert_eq!(row["status"], "done");
		assert_eq!(row["model"], "cov-model");
		assert!(row["ago_secs"].is_u64(), "{row}");

		crate::session::tap_runs::clear_for_session(&session_id);
	})
	.await;
}
