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
use serial_test::serial;
use std::time::{Duration, UNIX_EPOCH};

fn test_job(id: &str, role: &str, started_at: SystemTime, status: TapJobStatus) -> (TapJob, watch::Receiver<bool>) {
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

#[test]
fn id_format_is_stable() {
	let id = generate_id("developer:general");
	assert!(id.starts_with("tap-developer-general-"));
	// 4 (tap-) + 17 (developer-general-) + 6 (hex) = 27
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
async fn cancel_job_cancels_a_running_job() {
	let session_id = "tap-runs-cancel-running".to_string();
	let (job, cancel_rx) = test_job("running", "developer:general", UNIX_EPOCH, TapJobStatus::Running);

	let status = crate::session::context::with_session_id(session_id.clone(), async {
		register_job(job);
		let status = cancel_job("running");
		assert!(!has_running_jobs());
		status
	})
	.await;
	clear_for_session(&session_id);

	assert_eq!(status, Some(TapJobStatus::Cancelled));
	assert!(*cancel_rx.borrow());
}

#[tokio::test]
#[serial]
async fn cancel_job_leaves_a_finished_job_unchanged() {
	let session_id = "tap-runs-cancel-finished".to_string();
	let (job, cancel_rx) = test_job("finished", "developer:general", UNIX_EPOCH, TapJobStatus::Done);

	let (status, cancelled) = crate::session::context::with_session_id(session_id.clone(), async {
		register_job(job);
		(cancel_job("finished"), *cancel_rx.borrow())
	})
	.await;
	clear_for_session(&session_id);

	assert_eq!(status, Some(TapJobStatus::Done));
	assert!(!cancelled);
}

#[tokio::test]
#[serial]
async fn cancel_job_returns_none_for_an_unknown_id() {
	let session_id = "tap-runs-cancel-unknown".to_string();
	let status = crate::session::context::with_session_id(session_id.clone(), async { cancel_job("missing") }).await;
	clear_for_session(&session_id);
	assert_eq!(status, None);
}

#[tokio::test]
#[serial]
async fn list_jobs_is_newest_first_and_empty_without_a_session() {
	assert!(list_jobs().is_empty());
	let session_id = "tap-runs-list-order".to_string();
	let older = UNIX_EPOCH + Duration::from_secs(1);
	let newer = UNIX_EPOCH + Duration::from_secs(2);

	let ids = crate::session::context::with_session_id(session_id.clone(), async {
		let (job, _) = test_job("older", "developer:general", older, TapJobStatus::Done);
		register_job(job);
		let (job, _) = test_job("newer", "reviewer:rust", newer, TapJobStatus::Running);
		register_job(job);
		list_jobs().into_iter().map(|job| job.id).collect::<Vec<_>>()
	})
	.await;
	clear_for_session(&session_id);
	assert_eq!(ids, ["newer", "older"]);
}

#[tokio::test]
#[serial]
async fn pending_labels_are_sorted_and_include_only_running_jobs() {
	let session_id = "tap-runs-pending-labels".to_string();
	let (labels, running) = crate::session::context::with_session_id(session_id.clone(), async {
		let (job, _) = test_job("z", "zeta", UNIX_EPOCH, TapJobStatus::Running);
		register_job(job);
		let (job, _) = test_job("done", "alpha", UNIX_EPOCH, TapJobStatus::Done);
		register_job(job);
		let (job, _) = test_job("a", "alpha", UNIX_EPOCH, TapJobStatus::Running);
		register_job(job);
		(pending_labels(), has_running_jobs())
	})
	.await;
	clear_for_session(&session_id);
	assert_eq!(labels, ["alpha (a)", "zeta (z)"]);
	assert!(running);
}

#[tokio::test]
#[serial]
async fn find_job_and_live_updates_return_a_snapshot() {
	let session_id = "tap-runs-live-snapshot".to_string();
	let snapshot = crate::session::context::with_session_id(session_id.clone(), async {
		let (job, _) = test_job("live", "developer:general", UNIX_EPOCH, TapJobStatus::Running);
		register_job(job);
		record_live_action("live", "editing src/main.rs".to_string());
		record_live_usage(
			"live",
			TapLiveUsage {
				input_tokens: 10,
				output_tokens: 5,
				cache_read_tokens: 2,
				cost: 0.25,
			},
		);
		record_live_action("missing", "ignored".to_string());
		find_job("live")
	})
	.await;
	clear_for_session(&session_id);

	let snapshot = snapshot.expect("registered job is found");
	assert_eq!(snapshot.role, "developer:general");
	assert_eq!(snapshot.workdir, "/tmp");
	assert_eq!(snapshot.live.last_action.as_deref(), Some("editing src/main.rs"));
	let usage = snapshot.live.usage.expect("usage was recorded");
	assert_eq!(usage.input_tokens, 10);
	assert_eq!(usage.output_tokens, 5);
	assert_eq!(usage.cache_read_tokens, 2);
	assert_eq!(usage.cost, 0.25);
}

#[tokio::test]
#[serial]
async fn reset_cancel_replaces_the_latched_receiver() {
	let session_id = "tap-runs-reset-cancel".to_string();
	let (job, old_rx) = test_job("resume", "developer:general", UNIX_EPOCH, TapJobStatus::Running);
	let new_rx_is_clear = crate::session::context::with_session_id(session_id.clone(), async {
		register_job(job);
		assert_eq!(cancel_job("resume"), Some(TapJobStatus::Cancelled));
		assert!(*old_rx.borrow());
		let new_rx = reset_cancel("resume").expect("registered job has a cancellation channel");
		let is_clear = !*new_rx.borrow();
		is_clear
	})
	.await;
	clear_for_session(&session_id);
	assert!(new_rx_is_clear);
}

#[tokio::test]
#[serial]
async fn registry_is_isolated_by_session() {
	let session_a = "tap-runs-session-a".to_string();
	let session_b = "tap-runs-session-b".to_string();
	let (job, _) = test_job("private", "developer:general", UNIX_EPOCH, TapJobStatus::Running);
	crate::session::context::with_session_id(session_a.clone(), async {
		register_job(job);
	})
	.await;

	let hidden = crate::session::context::with_session_id(session_b.clone(), async {
		(find_job("private"), list_jobs(), has_running_jobs(), pending_labels())
	})
	.await;
	let visible =
		crate::session::context::with_session_id(session_a.clone(), async { find_job("private").is_some() }).await;
	clear_for_session(&session_a);
	clear_for_session(&session_b);

	assert!(hidden.0.is_none());
	assert!(hidden.1.is_empty());
	assert!(!hidden.2);
	assert!(hidden.3.is_empty());
	assert!(visible);
}
