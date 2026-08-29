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

//! Rendering, condition evaluation, persistence, and resume coverage for the
//! runtime-owned plan core. Every test scopes plan storage through a unique
//! task-local session; tests that persist snapshots to the session log pin
//! OCTOMIND_DATA_DIR to a temp dir under ENV_LOCK.

use super::*;
use crate::mcp::core::plan::storage::{ExecutionPlan, PlanStatus, PlanTask};

fn directive(title: &str) -> crate::supervisor::plan::PlanTaskDirective {
	crate::supervisor::plan::PlanTaskDirective {
		title: title.to_string(),
		done_when: format!("{title} is verified"),
	}
}

fn task(title: &str, valid_if: Option<&str>) -> TaskData {
	TaskData::new(
		title.to_string(),
		format!("Done when: {title}"),
		None,
		valid_if.map(str::to_string),
	)
}

fn unique_session(label: &str) -> String {
	format!("plan-core-{label}-{}", std::process::id())
}

fn plan_fixture(title: &str, tasks: usize, current: usize) -> ExecutionPlan {
	ExecutionPlan {
		title: title.to_string(),
		tasks: (0..tasks)
			.map(|i| PlanTask {
				title: format!("t{i}"),
				description: format!("Done when: t{i}"),
				details: String::new(),
				summary: None,
				status: if i < current {
					TaskStatus::Completed
				} else {
					TaskStatus::InProgress
				},
				completed_at: None,
				message_range: None,
				phase: None,
				valid_if: None,
			})
			.collect(),
		current_task_index: current,
		created_at: chrono::Utc::now(),
		status: PlanStatus::Active,
	}
}

// ---------------------------------------------------------------------------
// Rendering surfaces
// ---------------------------------------------------------------------------

#[tokio::test]
async fn display_json_details_and_conditions_error_without_a_plan() {
	crate::session::context::with_session_id(unique_session("noplan"), async {
		let err = get_current_plan_display().await.expect_err("no plan");
		assert!(err.to_string().contains("No active plan"), "err: {err}");
		let err = get_current_plan_json().await.expect_err("no plan");
		assert!(err.to_string().contains("No active plan"), "err: {err}");
		assert!(render_plan_details().is_none());
		assert!(broken_plan_conditions().is_empty());
	})
	.await;
}

#[tokio::test]
async fn display_renders_status_icons_and_the_current_task() {
	crate::session::context::with_session_id(unique_session("display"), async {
		let storage = get_storage();
		storage
			.lock()
			.unwrap()
			.create_plan(
				"Ship it".to_string(),
				vec![task("alpha", None), task("beta", None), task("gamma", None)],
			)
			.unwrap();
		storage
			.lock()
			.unwrap()
			.complete_current_task("alpha done".into())
			.unwrap();

		let display = get_current_plan_display().await.unwrap();
		assert!(display.starts_with("PLAN: Ship it"), "display: {display}");
		assert!(display.contains("✅ 1. alpha"), "display: {display}");
		assert!(
			display.contains("🔄 2. beta (IN PROGRESS)"),
			"display: {display}"
		);
		assert!(display.contains("⏳ 3. gamma"), "display: {display}");
		assert!(
			display.contains("CURRENT: Task 2/3 - beta"),
			"display: {display}"
		);
		assert!(display.contains("📝 Done when: beta"), "display: {display}");

		let json = get_current_plan_json().await.unwrap();
		assert_eq!(json["plan_title"], "Ship it");
		assert_eq!(json["current_task"], 2);
		assert_eq!(json["total_tasks"], 3);
		assert_eq!(json["current_task_title"], "beta");
		assert_eq!(json["current_task_description"], "Done when: beta");
		assert_eq!(json["tasks"][0]["status"], "completed");
		assert_eq!(json["tasks"][1]["status"], "in_progress");
		assert_eq!(json["tasks"][2]["status"], "in_progress");
	})
	.await;
}

#[tokio::test]
async fn details_expand_every_phase_state() {
	crate::session::context::with_session_id(unique_session("details"), async {
		let storage = get_storage();
		storage
			.lock()
			.unwrap()
			.create_plan(
				"Review".to_string(),
				vec![
					task("inspect", None),
					task("synthesize", None),
					task("deliver", None),
				],
			)
			.unwrap();
		storage
			.lock()
			.unwrap()
			.complete_current_task("inspected".into())
			.unwrap();

		let details = render_plan_details().expect("active plan renders");
		assert!(details.starts_with("Plan: Review"), "details: {details}");
		assert!(details.contains("1. [completed] inspect — Done when: inspect"));
		assert!(details.contains("2. [current] synthesize — Done when: synthesize"));
		assert!(details.contains("3. [pending] deliver — Done when: deliver"));
	})
	.await;
}

#[tokio::test]
async fn checklist_expands_valid_if_for_open_tasks_only() {
	crate::session::context::with_session_id(unique_session("checklist"), async {
		let storage = get_storage();
		storage
			.lock()
			.unwrap()
			.create_plan(
				"Conditions".to_string(),
				vec![
					task("done", Some("file_absent: Cargo.toml")),
					task("open", Some("file_exists: Cargo.toml")),
					task("plain", None),
				],
			)
			.unwrap();
		storage
			.lock()
			.unwrap()
			.complete_current_task("done".into())
			.unwrap();

		let checklist = render_plan_checklist().expect("active plan renders");
		assert!(
			checklist.contains("Live plan (1/3 done)"),
			"checklist: {checklist}"
		);
		assert!(checklist.contains("✅ done"), "checklist: {checklist}");
		assert!(
			checklist.contains("🔄 open ← current"),
			"checklist: {checklist}"
		);
		assert!(
			checklist.contains("   Done when: open"),
			"checklist: {checklist}"
		);
		assert!(
			checklist.contains("⤷ task 2 valid if: file_exists: Cargo.toml"),
			"checklist: {checklist}"
		);
		assert!(
			!checklist.contains("⤷ task 1"),
			"completed tasks drop their condition"
		);
	})
	.await;
}

#[tokio::test]
async fn broken_conditions_report_only_broken_open_ones() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let present = tmp.path().join("present.txt");
	std::fs::write(&present, "x").expect("write fixture");

	crate::session::context::with_session_id(unique_session("broken"), async {
		let storage = get_storage();
		storage
			.lock()
			.unwrap()
			.create_plan(
				"Mixed".to_string(),
				vec![
					// Completed: broken condition is irrelevant — never reported.
					task(
						"finished",
						Some(&format!("file_absent: {}", present.display())),
					),
					// Open + deterministically broken → reported.
					task(
						"wrongpath",
						Some(&format!("file_absent: {}", present.display())),
					),
					// Open + holds → not reported.
					task(
						"rightpath",
						Some(&format!("file_exists: {}", present.display())),
					),
					// Open + not machine-checkable → ignored.
					task("prose", Some("the API still returns 200")),
				],
			)
			.unwrap();
		storage
			.lock()
			.unwrap()
			.complete_current_task("finished".into())
			.unwrap();

		let broken = broken_plan_conditions();
		assert_eq!(broken.len(), 1, "broken: {broken:?}");
		assert_eq!(broken[0].0, 2);
		assert_eq!(broken[0].1, "wrongpath");
		assert!(broken[0].2.starts_with("file_absent:"));
	})
	.await;
}

// ---------------------------------------------------------------------------
// Sidecar transitions
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial_test::serial]
async fn advance_rejects_when_every_task_is_already_complete() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let data_dir = tempfile::tempdir().expect("temp data dir");
	std::env::set_var("OCTOMIND_DATA_DIR", data_dir.path());

	let session = unique_session("advance-exhausted");
	crate::session::context::with_session_id(session, async {
		sidecar_start("Exhaust", &[directive("a"), directive("b")]).expect("start");
		sidecar_advance("a done").expect("first advance");
		sidecar_advance("b done").expect("second advance");
		let err = sidecar_advance("one too many").expect_err("plan is exhausted");
		assert!(err.to_string().contains("already complete"), "err: {err}");
	})
	.await;
	std::env::remove_var("OCTOMIND_DATA_DIR");
}

#[tokio::test]
#[serial_test::serial]
async fn revise_validates_input_and_preserves_completed_history() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let data_dir = tempfile::tempdir().expect("temp data dir");
	std::env::set_var("OCTOMIND_DATA_DIR", data_dir.path());

	let session = unique_session("revise");
	crate::session::context::with_session_id(session, async {
		let err = sidecar_revise("   ", &[directive("x")]).expect_err("empty reason");
		assert!(err.to_string().contains("non-empty"), "err: {err}");
		let err = sidecar_revise("why", &[directive("x")]).expect_err("no plan to revise");
		assert!(err.to_string().contains("No active plan"), "err: {err}");

		sidecar_start("Revise me", &[directive("old-a"), directive("old-b")]).expect("start");
		sidecar_advance("old-a done").expect("advance");
		sidecar_revise("route changed", &[directive("fresh")]).expect("revise");

		let json = get_current_plan_json().await.unwrap();
		assert_eq!(json["total_tasks"], 2);
		assert_eq!(json["tasks"][0]["title"], "old-a");
		assert_eq!(json["tasks"][0]["status"], "completed");
		assert_eq!(json["tasks"][1]["title"], "fresh");
		assert_eq!(json["current_task"], 2);
	})
	.await;
	std::env::remove_var("OCTOMIND_DATA_DIR");
}

#[test]
#[serial_test::serial]
fn cli_fallbacks_apply_outside_a_session_context() {
	// Outside a session the plan storage and the task start index fall back
	// to the CLI globals; persistence is a no-op (no session to log to).
	assert!(!has_active_plan());
	sidecar_start("CLI plan", &[directive("a"), directive("b")]).expect("start");
	assert!(has_active_plan());
	assert!(render_plan_checklist().is_some());

	set_current_task_start_index(7);
	assert_eq!(get_current_task_start_index(), Some(7));
	clear_task_start_index();
	assert_eq!(get_current_task_start_index(), None);

	sidecar_finish("wrapped").expect("finish");
	assert!(!has_active_plan());
}

// ---------------------------------------------------------------------------
// Persistence and resume
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial_test::serial]
async fn restore_replays_the_latest_snapshot_into_session_storage() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let data_dir = tempfile::tempdir().expect("temp data dir");
	std::env::set_var("OCTOMIND_DATA_DIR", data_dir.path());

	let session = unique_session("restore-latest");
	crate::session::logger::log_plan_snapshot(&session, &plan_fixture("older", 3, 0))
		.expect("first snapshot");
	// A malformed plan payload must be skipped, not abort the replay.
	crate::session::persistence::append_to_session_file(
		&crate::session::logger::get_session_log_file(&session).unwrap(),
		r#"{"type":"PLAN_SNAPSHOT","plan":{"title":123}}"#,
	)
	.expect("malformed entry appends");
	crate::session::logger::log_plan_snapshot(&session, &plan_fixture("newest", 3, 1))
		.expect("second snapshot");

	crate::session::context::with_session_id(session.clone(), async {
		restore_plan_for_session(&session);
		assert!(has_active_plan());
		let json = get_current_plan_json().await.unwrap();
		assert_eq!(json["plan_title"], "newest");
		assert_eq!(json["current_task"], 2);
		assert_eq!(json["total_tasks"], 3);
		assert_eq!(json["tasks"][0]["status"], "completed");
	})
	.await;
	std::env::remove_var("OCTOMIND_DATA_DIR");
	crate::session::context::clear_plan_storage(&session);
}

#[tokio::test]
#[serial_test::serial]
async fn restore_honors_a_trailing_clear_marker() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let data_dir = tempfile::tempdir().expect("temp data dir");
	std::env::set_var("OCTOMIND_DATA_DIR", data_dir.path());

	let session = unique_session("restore-cleared");
	crate::session::logger::log_plan_snapshot(&session, &plan_fixture("gone", 2, 0))
		.expect("snapshot");
	crate::session::logger::log_plan_cleared(&session).expect("clear marker");

	crate::session::context::with_session_id(session.clone(), async {
		restore_plan_for_session(&session);
		assert!(!has_active_plan());
	})
	.await;
	std::env::remove_var("OCTOMIND_DATA_DIR");
	crate::session::context::clear_plan_storage(&session);
}

#[tokio::test]
#[serial_test::serial]
async fn restore_tolerates_missing_corrupt_and_foreign_logs() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let data_dir = tempfile::tempdir().expect("temp data dir");
	std::env::set_var("OCTOMIND_DATA_DIR", data_dir.path());

	// Missing log file: plain no-op.
	let session = unique_session("restore-missing");
	crate::session::context::with_session_id(session.clone(), async {
		restore_plan_for_session(&session);
		assert!(!has_active_plan());
	})
	.await;

	// Corrupt (non-zstd) log: the decoder fails, restore gives up quietly.
	let session = unique_session("restore-corrupt");
	let log = crate::session::logger::get_session_log_file(&session).unwrap();
	std::fs::write(&log, b"definitely not zstd").expect("write corrupt log");
	crate::session::context::with_session_id(session.clone(), async {
		restore_plan_for_session(&session);
		assert!(!has_active_plan());
	})
	.await;

	// Valid zstd, but only unparseable / foreign / keyless entries.
	let session = unique_session("restore-foreign");
	let log = crate::session::logger::get_session_log_file(&session).unwrap();
	for line in [
		"{not json",
		r#"{"type":"OTHER","plan":null}"#,
		r#"{"type":"PLAN_SNAPSHOT"}"#,
	] {
		crate::session::persistence::append_to_session_file(&log, line).expect("raw entry appends");
	}
	crate::session::context::with_session_id(session.clone(), async {
		restore_plan_for_session(&session);
		assert!(!has_active_plan());
	})
	.await;

	std::env::remove_var("OCTOMIND_DATA_DIR");
}

#[tokio::test]
#[serial_test::serial]
async fn sidecar_transitions_persist_and_resume_as_a_cleared_plan() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let data_dir = tempfile::tempdir().expect("temp data dir");
	std::env::set_var("OCTOMIND_DATA_DIR", data_dir.path());

	let session = unique_session("sidecar-persist");
	crate::session::context::with_session_id(session.clone(), async {
		sidecar_start("Persisted", &[directive("a"), directive("b")]).expect("start");
		let log = crate::session::logger::get_session_log_file(&session).unwrap();
		assert!(log.is_file(), "snapshot must land on disk");
		sidecar_advance("a done").expect("advance");
		sidecar_finish("done").expect("finish");
	})
	.await;

	// A fresh process resumes from the log: the trailing PLAN_CLEARED must
	// win over the earlier snapshots.
	crate::session::context::with_session_id(session.clone(), async {
		restore_plan_for_session(&session);
		assert!(!has_active_plan());
	})
	.await;
	std::env::remove_var("OCTOMIND_DATA_DIR");
	crate::session::context::clear_plan_storage(&session);
}
