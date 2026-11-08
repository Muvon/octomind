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
use crate::session::chat::conversation_compression::{
	collect_fold_job,
	tests::{fold_config, fold_ctx, parked_briefing_fold, prepared_decision},
};

const TEST_WAIT: Duration = Duration::from_secs(2);
const LOCK_WAIT_PROBE: Duration = Duration::from_millis(20);

fn pending_job() -> FoldJob {
	let (cancel, mut rx) = tokio::sync::watch::channel(false);
	FoldJob {
		handle: tokio::spawn(async move {
			rx.changed().await.expect_err("dropping the job cancels the request");
			Err(crate::session::cancellation::Cancelled.into())
		}),
		ctx: fold_ctx(0, 1, 42),
		prepared: prepared_decision(),
		cancel,
	}
}

fn write_request(journal: &Path) -> PathBuf {
	let sidecar = journal.with_added_extension("fold.json");
	let request = FoldRequest {
		prepared: prepared_decision(),
		ctx: fold_ctx(0, 1, 42),
		session_name: "detached-test".to_string(),
	};
	octolib::utils::config_file::atomic_write(&sidecar, &serde_json::to_vec(&request).unwrap(), None).unwrap();
	sidecar
}

fn lock_sidecar(sidecar: &Path) -> std::fs::File {
	let lock = std::fs::OpenOptions::new()
		.read(true)
		.write(true)
		.create(true)
		.truncate(false)
		.open(sidecar.with_added_extension("lock"))
		.unwrap();
	lock.lock().unwrap();
	lock
}

fn result() -> FoldResult {
	FoldResult {
		summary: CompressionSummary {
			should_compress: true,
			current_task: "write the briefing".to_string(),
			progress: "drafted two sections".to_string(),
			defer_reason: super::super::schema::DeferReason::MidDerivation,
			..Default::default()
		},
		usage: Some(TokenUsage {
			input_tokens: 100,
			output_tokens: 20,
			reasoning_tokens: 10,
			cache_read_tokens: 0,
			cache_write_tokens: 0,
			total_tokens: 130,
			cost: Some(0.25),
			request_time_ms: Some(400),
		}),
	}
}

#[tokio::test]
#[serial_test::serial]
async fn pending_fold_sidecar_round_trip_preserves_payload_and_governance() {
	let _data = crate::session::chat::test_support::DataDirGuard::new();
	let (session, ctx) = parked_briefing_fold("sidecar-round-trip").await;
	let request = FoldRequest {
		prepared: prepared_decision(),
		ctx,
		session_name: session.session.info.name.clone(),
	};
	let raw = serde_json::to_value(&request).unwrap();
	assert!(raw["ctx"].get("started").is_none());
	let restored: FoldRequest = serde_json::from_value(raw.clone()).unwrap();
	assert_eq!(serde_json::to_value(&restored).unwrap(), raw);
	assert!(!restored.ctx.detached);
	restored
		.ctx
		.pact
		.as_ref()
		.unwrap()
		.verify_governance(&session, restored.ctx.fold_point)
		.unwrap();

	let result = result();
	let raw = serde_json::to_value(&result).unwrap();
	assert_eq!(raw["summary"]["defer_reason"], result.summary.defer_reason.as_token());
	let restored: FoldResult = serde_json::from_value(raw.clone()).unwrap();
	assert_eq!(serde_json::to_value(restored).unwrap(), raw);
}

#[tokio::test]
async fn unfinished_exit_writes_request_before_spawn_and_cancels_without_waiting() {
	let dir = tempfile::tempdir().unwrap();
	let journal = dir.path().join("session.jsonl.zst");
	let mut session = ChatSession::for_tests(Vec::new());
	session.session.session_file = Some(journal.clone());
	let job = pending_job();
	let cancelled = job.cancel.subscribe();
	let handshake_cancel = cancelled.clone();
	session.fold_job = Some(job);
	let applied = tokio::time::timeout(
		TEST_WAIT,
		detach_fold_before_exit(&mut session, &fold_config(), |sidecar, session_file| async move {
			assert_eq!(session_file, journal);
			let request: FoldRequest = serde_json::from_slice(&std::fs::read(sidecar)?).unwrap();
			assert_eq!(request.prepared.system_content, prepared_decision().system_content);
			assert!(
				handshake_cancel.has_changed().is_ok(),
				"parent request lives through the handshake"
			);
			Ok(())
		}),
	)
	.await
	.expect("exit must not await the fold")
	.unwrap();
	assert!(!applied);
	assert!(session.fold_job.is_none());
	assert!(session
		.session
		.session_file
		.as_ref()
		.unwrap()
		.with_added_extension("fold.json")
		.exists());
	assert_eq!(session.fold_cooldown_until_call, 0);
	assert!(cancelled.has_changed().is_err());
}

#[tokio::test]
async fn corrupt_fold_sidecar_is_discarded_and_does_not_fail_restore() {
	let dir = tempfile::tempdir().unwrap();
	let journal = dir.path().join("session.jsonl.zst");
	let sidecar = journal.with_added_extension("fold.json");
	std::fs::write(&sidecar, b"{ not json").unwrap();
	std::fs::write(sidecar.with_added_extension("lock"), b"").unwrap();
	std::fs::write(sidecar.with_added_extension("result.json"), b"{}").unwrap();
	// A corrupt sidecar must not abort the session load: resume continues with no fold job.
	assert!(restore_fold(&journal).unwrap().is_none());
	assert!(!sidecar.exists());
	assert!(!sidecar.with_added_extension("lock").exists());
	assert!(!sidecar.with_added_extension("result.json").exists());
}

#[tokio::test]
async fn restored_fold_waits_for_lock_then_yields_summary_and_usage() {
	let dir = tempfile::tempdir().unwrap();
	let journal = dir.path().join("session.jsonl.zst");
	let sidecar = write_request(&journal);
	let lock = lock_sidecar(&sidecar);
	let mut job = restore_fold(&journal).unwrap().unwrap();
	assert!(job.ctx.detached);
	assert!(tokio::time::timeout(LOCK_WAIT_PROBE, &mut job.handle).await.is_err());
	let expected = result();
	octolib::utils::config_file::atomic_write(
		&sidecar.with_added_extension("result.json"),
		&serde_json::to_vec(&expected).unwrap(),
		None,
	)
	.unwrap();
	drop(lock);
	let (summary, usage) = tokio::time::timeout(TEST_WAIT, job.handle)
		.await
		.unwrap()
		.unwrap()
		.unwrap();
	assert_eq!(summary.current_task, expected.summary.current_task);
	assert_eq!(usage.unwrap().cost, Some(0.25));
	assert!(!sidecar.exists());
	assert!(!sidecar.with_added_extension("result.json").exists());
	assert!(!sidecar.with_added_extension("lock").exists());
}

#[tokio::test]
async fn restored_fold_without_a_result_errors_and_cleans_up() {
	let dir = tempfile::tempdir().unwrap();
	let journal = dir.path().join("session.jsonl.zst");
	let sidecar = write_request(&journal);
	let lock = lock_sidecar(&sidecar);
	let job = restore_fold(&journal).unwrap().unwrap();
	drop(lock);
	let error = tokio::time::timeout(TEST_WAIT, job.handle)
		.await
		.unwrap()
		.unwrap()
		.unwrap_err();
	assert_eq!(error.to_string(), "detached fold produced no result");
	assert!(!sidecar.exists());
	assert!(!sidecar.with_added_extension("result.json").exists());
	assert!(!sidecar.with_added_extension("lock").exists());
}

#[tokio::test]
async fn restored_fold_at_exit_leaves_child_files_alone() {
	let dir = tempfile::tempdir().unwrap();
	let journal = dir.path().join("session.jsonl.zst");
	let sidecar = write_request(&journal);
	let lock = lock_sidecar(&sidecar);
	let raw = std::fs::read(&sidecar).unwrap();
	let result_path = sidecar.with_added_extension("result.json");
	let result_raw = serde_json::to_vec(&result()).unwrap();
	octolib::utils::config_file::atomic_write(&result_path, &result_raw, None).unwrap();
	let mut session = ChatSession::for_tests(Vec::new());
	session.session.session_file = Some(journal.clone());
	let job = restore_fold(&journal).unwrap().unwrap();
	session.fold_job = Some(job);
	assert!(!tokio::time::timeout(
		TEST_WAIT,
		detach_fold_before_exit(&mut session, &fold_config(), spawn_compact_process)
	)
	.await
	.unwrap()
	.unwrap());
	assert!(session.fold_job.is_none());
	assert_eq!(std::fs::read(&sidecar).unwrap(), raw);
	assert_eq!(std::fs::read(&result_path).unwrap(), result_raw);
	assert!(sidecar.with_added_extension("lock").exists());
	drop(lock);
	let next = restore_fold(&journal).unwrap().unwrap();
	let (summary, _) = tokio::time::timeout(TEST_WAIT, next.handle)
		.await
		.unwrap()
		.unwrap()
		.unwrap();
	assert_eq!(summary.current_task, "write the briefing");
}

#[tokio::test]
#[serial_test::serial]
async fn detached_spend_is_not_billed_twice() {
	let _data = crate::session::chat::test_support::DataDirGuard::new();
	let dir = tempfile::tempdir().unwrap();
	let journal = dir.path().join("session.jsonl.zst");
	let sidecar = journal.with_added_extension("fold.json");
	let (mut session, ctx) = parked_briefing_fold("restored-fold-accounting").await;
	let request = FoldRequest {
		prepared: prepared_decision(),
		ctx,
		session_name: session.session.info.name.clone(),
	};
	octolib::utils::config_file::atomic_write(&sidecar, &serde_json::to_vec(&request).unwrap(), None).unwrap();
	let lock = lock_sidecar(&sidecar);
	let result = result();
	octolib::utils::config_file::atomic_write(
		&sidecar.with_added_extension("result.json"),
		&serde_json::to_vec(&result).unwrap(),
		None,
	)
	.unwrap();
	let job = restore_fold(&journal).unwrap().unwrap();
	drop(lock);
	let mut config = fold_config();
	config.compression.attention.enabled = false;
	config.compression.attention.telemetry = false;
	config.supervisor.learning.enabled = false;
	session.session.info.total_cost = 0.25;
	assert!(collect_fold_job(&mut session, &config, job, false, false)
		.await
		.unwrap());
	assert_eq!(session.session.info.total_cost, 0.25);
	assert_eq!(session.session.info.compression_stats.cost, 0.0);
	assert!(session
		.session
		.messages
		.iter()
		.any(|message| { message.name.as_deref() == Some(super::super::apply::COMPRESSION_MESSAGE_NAME) }));
}
