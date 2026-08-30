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

//! Session-loop helper tests for the schedule core: idle detection, idle and
//! due flushing against the real session inbox, snapshot persistence/restore
//! through the zstd session log, every `next_schedule_sleep` arm, and the
//! add/edit validation arms the tool tests do not reach.

use super::*;
use serial_test::serial;

const WAIT: std::time::Duration = std::time::Duration::from_secs(2);

fn call(params: serde_json::Value) -> McpToolCall {
	McpToolCall {
		tool_name: "schedule".to_string(),
		parameters: params,
		tool_id: "sched-session-test".to_string(),
	}
}

fn extract_id(text: &str) -> String {
	let start = text.find('[').expect("id bracket in add response") + 1;
	let end = text[start..].find(']').expect("closing bracket") + start;
	text[start..end].to_string()
}

/// Point `OCTOMIND_DATA_DIR` at a fresh temp dir for the lifetime of the guard,
/// restoring the previous value on drop. Sandboxes the session log directory.
struct TempDataDir {
	previous: Option<std::ffi::OsString>,
	_dir: tempfile::TempDir,
}

impl TempDataDir {
	fn new() -> Self {
		let previous = std::env::var_os("OCTOMIND_DATA_DIR");
		let dir = tempfile::tempdir().expect("create temp data dir");
		std::env::set_var("OCTOMIND_DATA_DIR", dir.path());
		Self {
			previous,
			_dir: dir,
		}
	}
}

impl Drop for TempDataDir {
	fn drop(&mut self) {
		match self.previous.take() {
			Some(old) => std::env::set_var("OCTOMIND_DATA_DIR", old),
			None => std::env::remove_var("OCTOMIND_DATA_DIR"),
		}
	}
}

/// Drain every entry (time and idle mode) from the CLI-global store so
/// serial global-store tests cannot leak state into each other.
fn drain_global_store() {
	let store = get_store();
	loop {
		let popped = {
			let mut guard = store.lock().unwrap();
			guard.pop_due().or_else(|| guard.pop_idle())
		};
		if popped.is_none() {
			return;
		}
	}
}

fn register_running_tap_job(id: &str, running: bool) {
	let (cancel_tx, _keep_alive) = tokio::sync::watch::channel(false);
	crate::session::tap_runs::register_job(crate::session::tap_runs::TapJob {
		id: id.to_string(),
		role: "developer:general".to_string(),
		workdir: "/tmp".to_string(),
		started_at: std::time::SystemTime::UNIX_EPOCH,
		status: Arc::new(std::sync::RwLock::new(if running {
			crate::session::tap_runs::TapJobStatus::Running
		} else {
			crate::session::tap_runs::TapJobStatus::Done
		})),
		cancel_tx,
		live: Arc::new(std::sync::RwLock::new(
			crate::session::tap_runs::TapLiveState::default(),
		)),
	});
}

// ---------------------------------------------------------------------------
// is_session_idle
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_is_session_idle_walks_every_busy_source() {
	crate::session::context::with_session_id("sched-idle-probe".to_string(), async {
		crate::session::inbox::init_inbox_for_session();
		crate::session::tap_runs::init_for_session();
		crate::session::context::init_job_manager_for_session(&"sched-idle-probe".to_string());

		assert!(is_session_idle(), "fresh session must be idle");

		// Running tap-run keeps the session busy.
		register_running_tap_job("sched-idle-tap", true);
		assert!(!is_session_idle());
		crate::session::tap_runs::clear_for_session(&"sched-idle-probe".to_string());
		assert!(is_session_idle(), "clearing taps restores idle");

		// Pending watched shell job keeps the session busy.
		crate::session::shell_jobs::register_for_session(
			"sched-idle-probe",
			"file:///tmp/sched-idle-job",
			"job",
		);
		assert!(!is_session_idle());
		crate::session::shell_jobs::clear_for_session("sched-idle-probe");
		assert!(is_session_idle(), "clearing watched jobs restores idle");

		// Active background agent job keeps the session busy.
		let manager = crate::session::context::get_job_manager_for_session()
			.expect("job manager initialized for session");
		manager
			.try_acquire()
			.expect("first slot must be acquirable");
		assert!(!is_session_idle());
		manager.release(crate::session::background_jobs::CompletedJob {
			agent_name: "sched-idle-agent".to_string(),
			output: "done".to_string(),
		});
		assert!(is_session_idle(), "released job restores idle");

		crate::session::context::clear_job_manager_for_session(&"sched-idle-probe".to_string());
		crate::session::inbox::clear_inbox_for_session(&"sched-idle-probe".to_string());
		crate::session::context::clear_schedule_storage(&"sched-idle-probe".to_string());
	})
	.await;
}

// ---------------------------------------------------------------------------
// flush_idle_to_inbox / flush_due_to_inbox against the real inbox
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_flush_idle_waits_for_idle_then_consumes_and_reschedules() {
	crate::session::context::with_session_id("sched-idle-flush".to_string(), async {
		crate::session::inbox::init_inbox_for_session();
		crate::session::tap_runs::init_for_session();

		// One-shot idle entry + repeating idle entry.
		let one_shot = execute_schedule_tool(&call(serde_json::json!({
			"command": "add",
			"message": "one shot at idle"
		})))
		.await
		.expect("add one-shot idle");
		assert!(!one_shot.is_error());
		let repeating = execute_schedule_tool(&call(serde_json::json!({
			"command": "add",
			"message": "every idle",
			"every": "idle"
		})))
		.await
		.expect("add repeating idle");
		assert!(!repeating.is_error());
		assert!(repeating.extract_content().contains("Repeats: every idle"));

		// Busy session: flush is a no-op, entries stay queued, inbox stays empty.
		register_running_tap_job("sched-idle-flush-tap", true);
		flush_idle_to_inbox();
		assert!(has_pending_idle_schedules(), "busy session must not flush");
		assert!(!crate::session::inbox::has_inbox_messages());
		crate::session::tap_runs::clear_for_session(&"sched-idle-flush".to_string());

		// Idle session: both entries fire; the repeating one is re-added.
		flush_idle_to_inbox();
		assert!(
			has_pending_idle_schedules(),
			"repeating idle entry must be re-added after firing"
		);
		let first = crate::session::inbox::try_pop_inbox_message().expect("first message");
		assert!(matches!(
			first.source,
			crate::session::inbox::InboxSource::Schedule { .. }
		));
		assert_eq!(first.content, "one shot at idle");
		let second = crate::session::inbox::try_pop_inbox_message().expect("second message");
		assert_eq!(second.content, "every idle");

		// Nothing left to flush — early return on has_idle.
		flush_idle_to_inbox();
		assert!(!crate::session::inbox::has_inbox_messages());

		// The surviving entry is the repeating one; remove it to clean up.
		let listing = render_pending_entries().expect("repeating entry listed");
		assert!(listing.contains("🔁 Repeats every idle"), "{listing}");
		let id = extract_id(&listing);
		let removed = execute_schedule_tool(&call(serde_json::json!({
			"command": "remove",
			"id": id
		})))
		.await
		.expect("remove");
		assert!(!removed.is_error());

		crate::session::inbox::clear_inbox_for_session(&"sched-idle-flush".to_string());
		crate::session::context::clear_schedule_storage(&"sched-idle-flush".to_string());
	})
	.await;
}

#[tokio::test]
async fn test_flush_due_pushes_into_session_inbox_and_persists_snapshot() {
	crate::session::context::with_session_id("sched-due-flush".to_string(), async {
		crate::session::inbox::init_inbox_for_session();

		let added = execute_schedule_tool(&call(serde_json::json!({
			"command": "add",
			"message": "due now",
			"when": "now"
		})))
		.await
		.expect("add due entry");
		assert!(!added.is_error());

		flush_due_to_inbox();
		assert!(!has_pending_schedules(), "one-shot due entry is consumed");
		let msg = crate::session::inbox::try_pop_inbox_message().expect("due message injected");
		assert_eq!(msg.content, "due now");
		assert!(matches!(
			msg.source,
			crate::session::inbox::InboxSource::Schedule { .. }
		));

		crate::session::inbox::clear_inbox_for_session(&"sched-due-flush".to_string());
		crate::session::context::clear_schedule_storage(&"sched-due-flush".to_string());
	})
	.await;
}

/// Outside a session the global store is used; inbox pushes are dropped and
/// persistence is a no-op, but due entries are still consumed.
#[serial]
#[tokio::test]
async fn test_flush_due_to_inbox_outside_session_uses_global_store() {
	drain_global_store();
	let store = get_store();
	let entry = ScheduleEntry::new(
		"global one-shot".to_string(),
		"fire and forget".to_string(),
		chrono::Local::now(),
		None,
	);
	store.lock().unwrap().add(entry);
	assert!(has_pending_schedules());

	flush_due_to_inbox();
	assert!(
		!has_pending_schedules(),
		"global due entry must be consumed"
	);
	drain_global_store();
}

// ---------------------------------------------------------------------------
// next_schedule_sleep — all four (duration, session) arms
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_next_schedule_sleep_timer_and_notify_arms_in_session() {
	crate::session::context::with_session_id("sched-sleep".to_string(), async {
		// (Some(d), Some(sid)): a due entry resolves via the timer arm.
		let store = crate::session::context::get_schedule_storage(&"sched-sleep".to_string());
		store.lock().unwrap().add(ScheduleEntry::new(
			String::new(),
			"due".to_string(),
			chrono::Local::now(),
			None,
		));
		tokio::time::timeout(WAIT, next_schedule_sleep())
			.await
			.expect("due entry must resolve the sleep");

		// (None, Some(sid)): empty store waits for a schedule-change notify.
		// notify_one stores a permit, so the notified() future returns at once.
		crate::session::context::get_schedule_notify(&"sched-sleep".to_string()).notify_one();
		tokio::time::timeout(WAIT, next_schedule_sleep())
			.await
			.expect("stored notify permit must resolve the sleep");

		crate::session::context::clear_schedule_storage(&"sched-sleep".to_string());
		crate::session::context::clear_schedule_notify(&"sched-sleep".to_string());
	})
	.await;
}

/// The two no-session arms share the process-global store, so they are serial
/// and drain it before and after.
#[serial]
#[tokio::test]
async fn test_next_schedule_sleep_no_session_arms() {
	drain_global_store();

	// (Some(d), None): a due global entry resolves via plain sleep.
	get_store().lock().unwrap().add(ScheduleEntry::new(
		String::new(),
		"global due".to_string(),
		chrono::Local::now(),
		None,
	));
	tokio::time::timeout(WAIT, next_schedule_sleep())
		.await
		.expect("global due entry must resolve the sleep");
	drain_global_store();

	// (None, None): empty store + no session — the future must never resolve.
	let elapsed = tokio::time::timeout(std::time::Duration::from_millis(100), async {
		next_schedule_sleep().await;
	})
	.await;
	assert!(elapsed.is_err(), "empty global store must stay pending");
}

// ---------------------------------------------------------------------------
// Snapshot restore
// ---------------------------------------------------------------------------

#[serial]
#[tokio::test]
async fn test_restore_schedule_round_trip_through_session_log() {
	let _data_dir = TempDataDir::new();
	let session = "sched-restore-roundtrip".to_string();

	let entries = vec![
		ScheduleEntry::new(
			"poll ci".to_string(),
			"check the build".to_string(),
			chrono::Local::now() + chrono::Duration::seconds(300),
			Some(600),
		),
		ScheduleEntry::new_idle("wrap up".to_string(), "finish the task".to_string(), false),
	];
	crate::session::logger::log_schedule_snapshot(&session, &entries).expect("write snapshot");

	restore_schedule_for_session(&session);

	let storage = crate::session::context::get_schedule_storage(&session);
	let restored = storage.lock().unwrap().entries().to_vec();
	assert_eq!(restored.len(), 2, "both entries must be restored");
	assert!(restored
		.iter()
		.any(|e| e.description == "poll ci" && e.interval_secs == Some(600)));
	assert!(restored
		.iter()
		.any(|e| e.description == "wrap up" && e.trigger_mode == TriggerMode::Idle));

	crate::session::context::clear_schedule_storage(&session);
}

#[serial]
#[tokio::test]
async fn test_restore_skips_malformed_lines_and_takes_latest_snapshot() {
	let _data_dir = TempDataDir::new();
	let session = "sched-restore-malformed".to_string();

	// Hand-craft a zstd session log with every degenerate line shape.
	let path = crate::session::logger::get_session_log_path(&session).expect("log path");
	let file = std::fs::File::create(&path).expect("create log file");
	let mut encoder = zstd::stream::write::Encoder::new(file, 0).expect("zstd encoder");
	let good_entry = ScheduleEntry::new(
		"latest".to_string(),
		"only this one survives".to_string(),
		chrono::Local::now() + chrono::Duration::seconds(60),
		None,
	);
	for line in [
		"this line is not json at all".to_string(),
		serde_json::json!({"type": "PLAN_CLEARED"}).to_string(),
		serde_json::json!({"type": "SCHEDULE_SNAPSHOT"}).to_string(),
		serde_json::json!({"type": "SCHEDULE_SNAPSHOT", "entries": "not-an-array"}).to_string(),
		serde_json::json!({"type": "SCHEDULE_SNAPSHOT", "entries": []}).to_string(),
		serde_json::json!({"type": "SCHEDULE_SNAPSHOT", "entries": [good_entry]}).to_string(),
	] {
		use std::io::Write;
		writeln!(encoder, "{line}").expect("write log line");
	}
	encoder.finish().expect("finish zstd encoder");

	restore_schedule_for_session(&session);

	let storage = crate::session::context::get_schedule_storage(&session);
	let restored = storage.lock().unwrap().entries().to_vec();
	assert_eq!(restored.len(), 1, "latest valid snapshot must win");
	assert_eq!(restored[0].description, "latest");

	crate::session::context::clear_schedule_storage(&session);
}

#[serial]
#[tokio::test]
async fn test_restore_is_noop_without_log_file() {
	let _data_dir = TempDataDir::new();
	let session = "sched-restore-absent".to_string();
	restore_schedule_for_session(&session);
	let storage = crate::session::context::get_schedule_storage(&session);
	assert!(
		storage.lock().unwrap().is_empty(),
		"no log file must leave the store empty"
	);
	crate::session::context::clear_schedule_storage(&session);
}

// ---------------------------------------------------------------------------
// add/edit validation arms and list rendering not covered by tool tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_add_rejects_idle_mix_zero_and_invalid_every() {
	crate::session::context::with_session_id("sched-add-validation".to_string(), async {
		for (params, expect) in [
			(
				serde_json::json!({"command": "add", "message": "x", "when": "in 5m", "every": "idle"}),
				"cannot combine a time-based 'when' with idle scheduling",
			),
			(
				serde_json::json!({"command": "add", "message": "x", "when": "idle", "every": "10m"}),
				"cannot combine time-based 'every' with idle scheduling",
			),
			(
				serde_json::json!({"command": "add", "message": "x", "every": "10m"}),
				"missing required parameter 'when'",
			),
			(
				serde_json::json!({"command": "add", "message": "x", "when": "in 5m", "every": "0s"}),
				"must be greater than zero",
			),
			(
				serde_json::json!({"command": "add", "message": "x", "when": "in 5m", "every": "5x"}),
				"invalid 'every' value",
			),
			(
				serde_json::json!({"command": 5}),
				"'command' must be a non-empty string",
			),
			(
				serde_json::json!({"command": "add", "message": "x", "when": 7}),
				"'when' must be a non-empty string",
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
		assert!(
			!has_pending_schedules(),
			"no entry may be created by errors"
		);

		crate::session::context::clear_schedule_storage(&"sched-add-validation".to_string());
	})
	.await;
}

#[tokio::test]
async fn test_edit_when_and_every_clearing_and_validation() {
	crate::session::context::with_session_id("sched-edit-validation".to_string(), async {
		let added = execute_schedule_tool(&call(serde_json::json!({
			"command": "add",
			"message": "original",
			"when": "in 10m",
			"every": "5m"
		})))
		.await
		.expect("add repeating");
		let id = extract_id(&added.extract_content());

		// Invalid `when` and invalid/zero `every` are rejected up front.
		for (params, expect) in [
			(
				serde_json::json!({"command": "edit", "id": id, "when": "in potato"}),
				"invalid 'when' value",
			),
			(
				serde_json::json!({"command": "edit", "id": id, "every": "0s"}),
				"must be greater than zero",
			),
			(
				serde_json::json!({"command": "edit", "id": id, "every": "nope"}),
				"invalid 'every' value",
			),
			(
				serde_json::json!({"command": "edit", "id": 3, "message": "x"}),
				"'id' must be a non-empty string",
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

		// Editing `when` moves the trigger time.
		let edited = execute_schedule_tool(&call(serde_json::json!({
			"command": "edit",
			"id": id,
			"when": "in 1h"
		})))
		.await
		.expect("edit when");
		assert!(!edited.is_error(), "{}", edited.extract_content());

		// `every=none` clears the repeat interval.
		let cleared = execute_schedule_tool(&call(serde_json::json!({
			"command": "edit",
			"id": id,
			"every": "none"
		})))
		.await
		.expect("edit every none");
		assert!(!cleared.is_error());
		let listing = render_pending_entries().expect("entry listed");
		assert!(!listing.contains("Repeats"), "interval cleared: {listing}");

		crate::session::context::clear_schedule_storage(&"sched-edit-validation".to_string());
	})
	.await;
}

#[tokio::test]
async fn test_render_pending_entries_formats_idle_truncation_and_repeats() {
	crate::session::context::with_session_id("sched-render".to_string(), async {
		// Idle entry without description, with an over-long message whose
		// 80-byte cut lands mid-multibyte-char (79 ASCII bytes + ééé).
		let long_message = "x".repeat(79) + "ééé";
		let added = execute_schedule_tool(&call(serde_json::json!({
			"command": "add",
			"message": long_message,
			"every": "idle"
		})))
		.await
		.expect("add idle repeating");
		assert!(!added.is_error());

		let listing = render_pending_entries().expect("entries rendered");
		assert!(listing.contains("(no description)"), "{listing}");
		assert!(listing.contains("(idle)"), "idle trigger format: {listing}");
		assert!(
			listing.contains("…"),
			"long message preview truncated: {listing}"
		);
		assert!(listing.contains("🔁 Repeats every idle"), "{listing}");
		// The preview must cut on a char boundary: no replacement bytes rendered.
		assert!(!listing.contains('\u{FFFD}'));

		// Time-mode repeat suffix.
		let time_entry = execute_schedule_tool(&call(serde_json::json!({
			"command": "add",
			"message": "tick",
			"when": "in 30m",
			"every": "1h 30m"
		})))
		.await
		.expect("add time repeating");
		assert!(
			time_entry
				.extract_content()
				.contains("Repeats: every 1h 30m"),
			"{}",
			time_entry.extract_content()
		);
		let listing = render_pending_entries().expect("entries rendered");
		assert!(listing.contains("🔁 Repeats every 1h 30m"), "{listing}");

		crate::session::context::clear_schedule_storage(&"sched-render".to_string());
	})
	.await;
}
