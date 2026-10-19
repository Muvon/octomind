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
use crate::session::tap_runs;
use serial_test::serial;
use std::time::Duration;

fn job(
	id: &str,
	role: &str,
	started_at: SystemTime,
	status: TapJobStatus,
) -> (TapJob, watch::Receiver<bool>) {
	let (cancel_tx, cancel_rx) = watch::channel(false);
	(
		TapJob {
			id: id.to_string(),
			role: role.to_string(),
			workdir: "/tmp".to_string(),
			started_at,
			status: Arc::new(RwLock::new(status)),
			cancel_tx,
			live: Arc::new(RwLock::new(TapLiveState::default())),
		},
		cancel_rx,
	)
}

fn register(id: &str, role: &str, second: u64, status: TapJobStatus) -> watch::Receiver<bool> {
	let (job, cancel_rx) = job(
		id,
		role,
		SystemTime::UNIX_EPOCH + Duration::from_secs(second),
		status,
	);
	tap_runs::register_job(job);
	cancel_rx
}

#[test]
fn id_format_is_stable() {
	let id = generate_id("developer:general");
	assert!(id.starts_with("tap-developer-general-"));
	assert_eq!(id.len(), "tap-developer-general-".len() + 6);
}

#[test]
fn status_strings_are_stable() {
	assert_eq!(TapJobStatus::Running.as_str(), "running");
	assert_eq!(TapJobStatus::Done.as_str(), "done");
	assert_eq!(TapJobStatus::Failed.as_str(), "failed");
	assert_eq!(TapJobStatus::Cancelled.as_str(), "cancelled");
}

#[tokio::test]
#[serial]
async fn cancel_job_cancels_running_jobs_and_signals_the_receiver() {
	let session = "tap-runs-cancel-running";
	crate::session::context::with_session_id(session.to_string(), async {
		let mut cancel_rx = register("running", "writer", 1, TapJobStatus::Running);
		assert_eq!(
			tap_runs::cancel_job("running"),
			Some(TapJobStatus::Cancelled)
		);
		assert!(*cancel_rx.borrow_and_update());
		assert_eq!(
			tap_runs::find_job("running").unwrap().status,
			TapJobStatus::Cancelled
		);
	})
	.await;
	tap_runs::clear_for_session(&session.to_string());
}

#[tokio::test]
#[serial]
async fn cancel_job_leaves_finished_jobs_unchanged_and_unknown_ids_return_none() {
	let session = "tap-runs-cancel-finished";
	crate::session::context::with_session_id(session.to_string(), async {
		let mut cancel_rx = register("done", "writer", 1, TapJobStatus::Done);
		assert_eq!(tap_runs::cancel_job("done"), Some(TapJobStatus::Done));
		assert!(!*cancel_rx.borrow_and_update());
		assert_eq!(tap_runs::cancel_job("missing"), None);
	})
	.await;
	tap_runs::clear_for_session(&session.to_string());
}

#[tokio::test]
#[serial]
async fn list_jobs_returns_newest_first_and_empty_without_a_session() {
	let session = "tap-runs-list-order";
	assert!(tap_runs::list_jobs().is_empty());
	crate::session::context::with_session_id(session.to_string(), async {
		register("old", "writer", 1, TapJobStatus::Done);
		register("new", "reviewer", 3, TapJobStatus::Running);
		register("middle", "tester", 2, TapJobStatus::Failed);
		let jobs = tap_runs::list_jobs();
		assert_eq!(
			jobs.iter().map(|job| job.id.as_str()).collect::<Vec<_>>(),
			["new", "middle", "old"]
		);
	})
	.await;
	tap_runs::clear_for_session(&session.to_string());
}

#[tokio::test]
#[serial]
async fn pending_labels_include_only_running_jobs_in_sorted_order() {
	let session = "tap-runs-pending-labels";
	crate::session::context::with_session_id(session.to_string(), async {
		register("z", "writer", 1, TapJobStatus::Running);
		register("done", "archivist", 2, TapJobStatus::Done);
		register("a", "reviewer", 3, TapJobStatus::Running);
		assert_eq!(
			tap_runs::pending_labels(),
			["reviewer (a)".to_string(), "writer (z)".to_string()]
		);
	})
	.await;
	tap_runs::clear_for_session(&session.to_string());
}

#[tokio::test]
#[serial]
async fn running_job_helpers_find_jobs_and_track_live_updates() {
	let session = "tap-runs-live-state";
	crate::session::context::with_session_id(session.to_string(), async {
		register("running", "writer", 1, TapJobStatus::Running);
		register("done", "reviewer", 2, TapJobStatus::Done);
		assert!(tap_runs::has_running_jobs());
		assert!(tap_runs::find_job("missing").is_none());

		tap_runs::record_live_action("running", "edited src/lib.rs".to_string());
		tap_runs::record_live_usage(
			"running",
			TapLiveUsage {
				input_tokens: 10,
				output_tokens: 20,
				cache_read_tokens: 30,
				cost: 0.4,
			},
		);
		tap_runs::record_live_action("missing", "ignored".to_string());
		tap_runs::record_live_usage(
			"missing",
			TapLiveUsage {
				input_tokens: 1,
				output_tokens: 2,
				cache_read_tokens: 3,
				cost: 0.1,
			},
		);

		let live = tap_runs::find_job("running").unwrap().live;
		assert_eq!(live.last_action.as_deref(), Some("edited src/lib.rs"));
		let usage = live.usage.unwrap();
		assert_eq!(usage.input_tokens, 10);
		assert_eq!(usage.output_tokens, 20);
		assert_eq!(usage.cache_read_tokens, 30);
		assert!((usage.cost - 0.4).abs() < f64::EPSILON);
		assert_eq!(tap_runs::find_job("done").unwrap().live.last_action, None);
	})
	.await;
	tap_runs::clear_for_session(&session.to_string());
}

#[tokio::test]
#[serial]
async fn reset_cancel_replaces_a_latched_cancellation_with_false() {
	let session = "tap-runs-reset-cancel";
	crate::session::context::with_session_id(session.to_string(), async {
		let _old_rx = register("running", "writer", 1, TapJobStatus::Running);
		assert_eq!(
			tap_runs::cancel_job("running"),
			Some(TapJobStatus::Cancelled)
		);
		let new_rx = tap_runs::reset_cancel("running").expect("job exists");
		assert!(!*new_rx.borrow());
		assert!(tap_runs::reset_cancel("missing").is_none());
	})
	.await;
	tap_runs::clear_for_session(&session.to_string());
}

#[tokio::test]
#[serial]
async fn jobs_are_isolated_between_sessions() {
	let session_a = "tap-runs-isolation-a";
	let session_b = "tap-runs-isolation-b";
	crate::session::context::with_session_id(session_a.to_string(), async {
		register("only-a", "writer", 1, TapJobStatus::Running);
	})
	.await;
	crate::session::context::with_session_id(session_b.to_string(), async {
		assert!(tap_runs::list_jobs().is_empty());
		assert!(tap_runs::find_job("only-a").is_none());
		assert!(!tap_runs::has_running_jobs());
		register("only-b", "reviewer", 2, TapJobStatus::Done);
		assert_eq!(tap_runs::list_jobs().len(), 1);
	})
	.await;
	tap_runs::clear_for_session(&session_a.to_string());
	tap_runs::clear_for_session(&session_b.to_string());
}
