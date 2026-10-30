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
			error: None,
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

#[tokio::test]
async fn sent_prompts_are_bounded_ordered_and_closed_atomically() {
	let parent = "tap-prompt-queue".to_string();
	crate::session::context::with_session_id(parent.clone(), async {
		init_for_session();
		let (job, _cancel) = test_job("run", "developer:general", UNIX_EPOCH, TapJobStatus::Running);
		register_job(job);
		let mut input = TapRunInput {
			id: "run",
			prompts: open_prompt_channel("run").unwrap(),
		};
		for index in 0..MAX_PENDING_PROMPTS {
			send_prompt("run", index.to_string()).unwrap();
		}
		assert!(send_prompt("run", "overflow".into())
			.unwrap_err()
			.to_string()
			.contains("queued prompts"));
		for index in 0..MAX_PENDING_PROMPTS {
			assert_eq!(take_prompt_or_close(&mut input).unwrap(), Some(index.to_string()));
		}
		// Emptying the queue alone must not prevent sending during the next turn.
		send_prompt("run", "last".into()).unwrap();
		assert_eq!(take_prompt_or_close(&mut input).unwrap().as_deref(), Some("last"));
		assert!(take_prompt_or_close(&mut input).unwrap().is_none());
		assert!(send_prompt("run", "too late".into())
			.unwrap_err()
			.to_string()
			.contains("finishing"));
		clear_for_session(&parent);
	})
	.await;
}

#[tokio::test]
async fn sent_prompts_require_a_running_owned_specialist() {
	let parent = "tap-prompt-owner".to_string();
	crate::session::context::with_session_id(parent.clone(), async {
		init_for_session();
		for status in [TapJobStatus::Done, TapJobStatus::Failed, TapJobStatus::Cancelled] {
			let id = status.as_str();
			let (job, _cancel) = test_job(id, "developer:general", UNIX_EPOCH, status);
			register_job(job);
			assert!(send_prompt(id, "hello".into())
				.unwrap_err()
				.to_string()
				.contains("use run to resume"));
		}
		let (job, _cancel) = test_job("workflow", "workflow:digest", UNIX_EPOCH, TapJobStatus::Running);
		register_job(job);
		assert!(send_prompt("workflow", "hello".into())
			.unwrap_err()
			.to_string()
			.contains("does not accept prompts"));
		let (job, _cancel) = test_job("active", "developer:general", UNIX_EPOCH, TapJobStatus::Running);
		register_job(job);
		let mut rx = open_prompt_channel("active").unwrap();
		crate::session::context::with_session_id("other-parent".to_string(), async {
			assert!(send_prompt("active", "wrong owner".into())
				.unwrap_err()
				.to_string()
				.contains("in this session"));
		})
		.await;
		assert!(rx.try_recv().is_err());
		send_prompt("active", "owned".into()).unwrap();
		assert_eq!(rx.recv().await.as_deref(), Some("owned"));
		clear_for_session(&parent);
		assert!(rx.recv().await.is_none());
	})
	.await;
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

	let (status, still_running) = crate::session::context::with_session_id(session_id.clone(), async {
		register_job(job);
		(cancel_job("running"), has_running_jobs())
	})
	.await;
	clear_for_session(&session_id);

	assert_eq!(status, Some(TapJobStatus::Cancelled));
	assert!(!still_running);
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
	let (snapshot, missing, job_count) = crate::session::context::with_session_id(session_id.clone(), async {
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
		// Updates for an unknown id must neither create a job nor touch "live".
		record_live_action("missing", "ignored".to_string());
		record_live_usage(
			"missing",
			TapLiveUsage {
				input_tokens: 99,
				output_tokens: 99,
				cache_read_tokens: 99,
				cost: 9.0,
			},
		);
		(find_job("live"), find_job("missing"), list_jobs().len())
	})
	.await;
	clear_for_session(&session_id);

	assert!(missing.is_none());
	assert_eq!(job_count, 1);
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
async fn finish_run_keeps_error_in_snapshots_and_tap_list_until_resume() {
	use crate::mcp::{orchestration::execute_tap_command, McpToolCall};
	use serde_json::json;
	let parent = "tap-runs-terminal-error".to_string();
	let id = "tap-developer-general-abcdef";
	crate::session::context::with_session_id(parent.clone(), async {
		init_for_session();
		let (job, _) = test_job(id, "developer:general", UNIX_EPOCH, TapJobStatus::Running);
		let status = job.status.clone();
		register_job(job);
		finish_run(id, &status, TapJobStatus::Failed, Some("child failed to start".into())).unwrap();
		let found = find_job(id).unwrap();
		assert_eq!(found.status, TapJobStatus::Failed);
		assert_eq!(found.error.as_deref(), Some("child failed to start"));
		assert_eq!(list_jobs()[0].error, found.error);

		let config = toml::from_str(include_str!("../../config-templates/default.toml")).unwrap();
		let call = McpToolCall {
			tool_name: "tap".into(),
			tool_id: "list-errors".into(),
			parameters: json!({"action": "list"}),
		};
		let result = execute_tap_command(&call, &config).await.unwrap();
		assert!(!result.is_error());
		let listed: serde_json::Value = serde_json::from_str(&result.extract_content()).unwrap();
		assert_eq!(listed["runs"][0]["status"], "failed");
		assert_eq!(listed["runs"][0]["error"], "child failed to start");

		let claim = resume_run(id).unwrap();
		assert_eq!(find_job(id).unwrap().error, None);
		finish_run(id, &claim.status, TapJobStatus::Done, None).unwrap();
		let result = execute_tap_command(&call, &config).await.unwrap();
		let listed: serde_json::Value = serde_json::from_str(&result.extract_content()).unwrap();
		assert!(listed["runs"][0].get("error").is_none());
	})
	.await;
	clear_for_session(&parent);
}

#[tokio::test]
#[serial]
async fn resume_run_replaces_the_latched_receiver() {
	let session_id = "tap-runs-reset-cancel".to_string();
	let (job, old_rx) = test_job("resume", "developer:general", UNIX_EPOCH, TapJobStatus::Running);
	// Read the new receiver inside the session: `clear_for_session` sends
	// `true` on the job's current sender, which is the replacement.
	let (status, new_rx_is_clear) = crate::session::context::with_session_id(session_id.clone(), async {
		register_job(job);
		let status = cancel_job("resume");
		let new_rx = resume_run("resume").unwrap().cancel_rx;
		let is_clear = !*new_rx.borrow();
		(status, is_clear)
	})
	.await;
	clear_for_session(&session_id);

	assert_eq!(status, Some(TapJobStatus::Cancelled));
	assert!(*old_rx.borrow(), "cancel latches the old channel");
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tap_run_snapshot_sequence_is_atomic_with_concurrent_emission() {
	use crate::websocket::protocol::{RunEvent, RunLogMessage};
	let parent = "tap-atomic-snapshot".to_string();
	let id = "tap-developer-cto-abcdef";
	crate::session::context::with_session_id(parent.clone(), async {
		init_for_session();
		enable_run_events(None).unwrap();
		let (job, _) = test_job(id, "developer:cto", UNIX_EPOCH, TapJobStatus::Running);
		register_job(job);
		with_run_events(|events, parent| events.begin(parent, id, "developer:cto"))
			.unwrap()
			.unwrap();
	})
	.await;
	let request = RunLogMessage {
		session_id: parent.clone(),
		run_id: id.into(),
		request_id: None,
		summary: false,
	};
	let emitter = tokio::spawn(crate::session::context::with_session_id(parent.clone(), async move {
		for _ in 0..256 {
			with_run_events(|events, parent| events.emit(parent, id, RunEvent::Assistant { content: "x".into() }))
				.unwrap()
				.unwrap();
			tokio::task::yield_now().await;
		}
	}));
	for _ in 0..256 {
		let snapshot = snapshot_run_log(&request).unwrap();
		assert_eq!(snapshot.source, "live");
		let text_bytes: usize = snapshot
			.events
			.iter()
			.map(|event| match event {
				RunEvent::Assistant { content } => content.len(),
				_ => 0,
			})
			.sum();
		assert_eq!(
			text_bytes as u64,
			snapshot.seq - 1,
			"snapshot content and sequence must cover exactly the same emissions"
		);
		tokio::task::yield_now().await;
	}
	emitter.await.unwrap();
	assert_eq!(snapshot_run_log(&request).unwrap().seq, 257);
	clear_for_session(&parent);
}

#[tokio::test]
async fn tap_run_terminal_status_releases_buffer_and_resume_continues_sequence() {
	use crate::websocket::protocol::{RunEvent, RunLogMessage};
	let parent = "tap-terminal-replay".to_string();
	let id = "tap-developer-cto-abcdef";
	crate::session::context::with_session_id(parent.clone(), async {
		init_for_session();
		enable_run_events(None).unwrap();
		let (job, _) = test_job(id, "developer:cto", UNIX_EPOCH, TapJobStatus::Running);
		let status = job.status.clone();
		register_job(job);
		let request = RunLogMessage {
			session_id: parent.clone(),
			run_id: id.into(),
			request_id: None,
			summary: false,
		};
		for (turn, terminal) in [TapJobStatus::Done, TapJobStatus::Failed, TapJobStatus::Cancelled]
			.into_iter()
			.enumerate()
		{
			*status.write().unwrap() = TapJobStatus::Running;
			with_run_events(|events, parent| {
				events.begin(parent, id, "developer:cto")?;
				events.emit(
					parent,
					id,
					RunEvent::Prompt {
						content: format!("turn {turn}"),
					},
				)
			})
			.unwrap()
			.unwrap();
			let live = snapshot_run_log(&request).unwrap();
			assert_eq!(live.source, "live");
			assert_eq!(live.events.len(), 2, "a resume gets a fresh buffer");
			assert_eq!(live.seq, (turn as u64) * 3 + 2);
			finish_run(
				id,
				&status,
				terminal,
				(terminal == TapJobStatus::Failed).then(|| "failure".into()),
			)
			.unwrap();
			let disk = snapshot_run_log(&request).unwrap();
			assert_eq!(disk.source, "disk");
			assert_eq!(disk.status, terminal.as_str());
			assert_eq!(disk.seq, live.seq + 1);
			assert_eq!(
				disk.events,
				vec![RunEvent::Status {
					status: terminal,
					error: (terminal == TapJobStatus::Failed).then(|| "failure".into()),
				}]
			);
			assert!(with_run_events(|events, _| Ok(events.snapshot(&request)))
				.unwrap()
				.unwrap()
				.is_none());
		}
	})
	.await;
	clear_for_session(&parent);
}

#[tokio::test]
async fn websocket_tap_cancel_keeps_the_stream_running_until_child_exit() {
	let parent = "tap-cancel-stream-owner".to_string();
	let id = "tap-developer-cto-abcdef";
	crate::session::context::with_session_id(parent.clone(), async {
		init_for_session();
		enable_run_events(None).unwrap();
		let (job, cancelled) = test_job(id, "developer:cto", UNIX_EPOCH, TapJobStatus::Running);
		let status = job.status.clone();
		register_job(job);
		with_run_events(|events, parent| events.begin(parent, id, "developer:cto"))
			.unwrap()
			.unwrap();
		assert_eq!(cancel_job(id), Some(TapJobStatus::Cancelled));
		assert!(*cancelled.borrow());
		assert_eq!(find_job(id).unwrap().status, TapJobStatus::Running);
		finish_run(id, &status, TapJobStatus::Cancelled, None).unwrap();
		assert_eq!(find_job(id).unwrap().status, TapJobStatus::Cancelled);
		assert!(!with_run_events(|events, _| Ok(events.is_running(id))).unwrap().unwrap());
	})
	.await;
	clear_for_session(&parent);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_same_id_resume_claims_exactly_one_run() {
	use crate::session::context::with_session_id;
	use crate::websocket::protocol::{RunEvent, RunLogMessage};
	let parent = "tap-concurrent-resume".to_string();
	let id = "tap-developer-cto-abcdef";
	with_session_id(parent.clone(), async {
		init_for_session();
		enable_run_events(None).unwrap();
		let (job, _) = test_job(id, "developer:cto", UNIX_EPOCH, TapJobStatus::Done);
		register_job(job);
	})
	.await;
	let barrier = Arc::new(tokio::sync::Barrier::new(3));
	let spawn = || {
		let barrier = barrier.clone();
		tokio::spawn(with_session_id(parent.clone(), async move {
			barrier.wait().await;
			resume_run(id)
		}))
	};
	let first = spawn();
	let second = spawn();
	barrier.wait().await;
	let (claim, error) = match (first.await.unwrap(), second.await.unwrap()) {
		(Ok(claim), Err(error)) | (Err(error), Ok(claim)) => (claim, error),
		other => panic!("exactly one resume must succeed: {other:?}"),
	};
	assert!(error.to_string().starts_with(&format!("run {id} is already running")));
	assert!(!*claim.cancel_rx.borrow());
	with_session_id(parent.clone(), async {
		let request = RunLogMessage {
			session_id: parent.clone(),
			run_id: id.into(),
			request_id: None,
			summary: false,
		};
		let snapshot = snapshot_run_log(&request).unwrap();
		assert_eq!(snapshot.seq, 1, "only the winner may begin the stream");
		assert_eq!(snapshot.events.len(), 1);
		with_run_events(|events, parent| {
			events.emit(
				parent,
				id,
				RunEvent::Assistant {
					content: "winner".into(),
				},
			)
		})
		.unwrap()
		.unwrap();
		assert_eq!(cancel_job(id), Some(TapJobStatus::Cancelled));
		assert!(
			*claim.cancel_rx.borrow(),
			"the losing resume must not replace cancellation"
		);
		finish_run(id, &claim.status, TapJobStatus::Cancelled, None).unwrap();
		let snapshot = snapshot_run_log(&request).unwrap();
		assert_eq!(snapshot.seq, 3);
		assert_eq!(snapshot.status, "cancelled");
	})
	.await;
	clear_for_session(&parent);
}
