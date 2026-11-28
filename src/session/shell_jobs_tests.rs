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
use rmcp::model::{CallToolResult, ContentBlock, Resource};

#[test]
fn extracts_resource_links_with_labels_and_ignores_plain_text() {
	let launched = CallToolResult::success(vec![
		ContentBlock::text("Started background job. Follow the linked resource."),
		ContentBlock::resource_link(Resource::new("octofs://jobs/1234-7", "shell: make reldebug")),
	]);
	assert_eq!(
		resource_links_in(&launched),
		vec![("octofs://jobs/1234-7".to_string(), "shell: make reldebug".to_string())]
	);

	// Recognition is not scheme-bound: any resource link is followed. An empty
	// name falls back to the URI as the label.
	let other = CallToolResult::success(vec![ContentBlock::resource_link(Resource::new(
		"custommcp://tasks/9",
		"",
	))]);
	assert_eq!(
		resource_links_in(&other),
		vec![("custommcp://tasks/9".to_string(), "custommcp://tasks/9".to_string())]
	);

	let plain = CallToolResult::success(vec![ContentBlock::text("just text, no resource")]);
	assert!(resource_links_in(&plain).is_empty());
}

#[test]
fn watch_complete_pending_and_labels_roundtrip() {
	let sid = "shell-jobs-unit-test-session";
	clear_for_session(sid);
	assert!(!has_pending_for_session(sid));

	let a = "octofs://jobs/a-1";
	let b = "octofs://jobs/a-2";
	register_for_session(sid, "octofs", a, "shell: make reldebug");
	register_for_session(sid, "octofs", b, "shell: run tests");
	assert!(has_pending_for_session(sid));
	let pending = pending_resources_for_session(sid);
	assert_eq!(pending.len(), 2);
	assert!(pending.iter().all(|job| job.server_name == "octofs"));
	assert!(pending.iter().all(|job| !job.delivering));
	assert_eq!(pending[0].uri, a);
	assert_eq!(pending[1].uri, b);
	assert!(is_watched_for_session(sid, a));
	assert!(!is_watched_for_session(sid, "octofs://jobs/never"));
	assert!(begin_delivery_for_session(sid, a));
	assert!(
		!begin_delivery_for_session(sid, a),
		"a duplicate update cannot start a second delivery"
	);
	assert!(
		has_pending_for_session(sid),
		"a resource remains pending until its inbox delivery exists"
	);

	assert!(complete_for_session(sid, a, "a complete"), "a was watched");
	assert!(!is_watched_for_session(sid, a));
	assert!(has_pending_for_session(sid), "b still keeps the session pending");
	assert!(
		!complete_for_session(sid, "octofs://jobs/unknown", "unknown"),
		"completing an unwatched uri reports not-watched"
	);

	assert!(complete_for_session(sid, b, "b complete"), "b was watched");
	assert!(
		!has_pending_for_session(sid),
		"the empty set is dropped, so the session is no longer pending"
	);
	assert!(
		!complete_for_session(sid, b, "duplicate"),
		"already-cleared uri reports not-watched"
	);
	clear_for_session(sid);
}

#[test]
fn completion_retains_sequence_label_and_body_until_taken() {
	let sid = "shell-jobs-completed-ledger-session";
	let uri = "octofs://jobs/completed-ledger";
	let label = "cargo test";
	let body = "status: exited with code 0\n\ntest result: ok";
	clear_for_session(sid);
	register_for_session(sid, "octofs", uri, label);
	attach_sequence_for_session(sid, uri, 26);

	assert!(complete_for_session(sid, uri, body));
	assert!(!has_pending_for_session(sid));
	let completed = take_completed_for_session(sid);
	assert_eq!(completed.len(), 1);
	assert_eq!(completed[0].sequence, Some(26));
	assert_eq!(completed[0].label, label);
	assert_eq!(completed[0].body, body);
	assert!(take_completed_for_session(sid).is_empty());
	clear_for_session(sid);
}

#[test]
fn sequence_attaches_after_a_fast_completion() {
	let sid = "shell-jobs-fast-completion-session";
	let uri = "octofs://jobs/fast-completion";
	clear_for_session(sid);
	register_for_session(sid, "octofs", uri, "cargo check");
	assert!(complete_for_session(sid, uri, "status: exited with code 0"));

	attach_sequence_for_session(sid, uri, 9);
	let completed = take_completed_for_session(sid);
	assert_eq!(completed.len(), 1);
	assert_eq!(completed[0].sequence, Some(9));
	clear_for_session(sid);
}

#[test]
fn clear_drops_banked_completions() {
	let sid = "shell-jobs-clear-completed-session";
	let uri = "octofs://jobs/clear-completed";
	clear_for_session(sid);
	register_for_session(sid, "octofs", uri, "cargo test");
	assert!(complete_for_session(sid, uri, "status: exited with code 0"));

	clear_for_session(sid);
	assert!(take_completed_for_session(sid).is_empty());
}

#[test]
fn a_task_is_pending_under_its_call_key_and_banks_its_sequence() {
	let sid = "shell-jobs-task-session";
	let key = task_key("toolu_1");
	assert_eq!(key, "mcp-task:toolu_1");
	clear_for_session(sid);
	let (_stop, _) = register_task_for_session(sid, "octofs", &key, "shell: cargo test");
	register_for_session(sid, "octofs", "octofs://jobs/resource", "cargo check");

	let pending = pending_resources_for_session(sid);
	assert_eq!(pending.len(), 2);
	assert_eq!(pending[0].uri, key);
	assert!(pending[0].task, "a task has no resource to read");
	assert!(!pending[1].task, "a resource link stays a resource");

	attach_sequence_for_session(sid, &key, 4);
	assert!(complete_for_session(sid, &key, "built"));
	let completed = take_completed_for_session(sid);
	assert_eq!(completed.len(), 1);
	assert_eq!(completed[0].sequence, Some(4));
	assert_eq!(completed[0].label, "shell: cargo test");
	assert_eq!(completed[0].body, "built");
	clear_for_session(sid);
}

#[test]
fn a_task_records_its_status_and_is_stopped_through_its_entry() {
	let sid = "shell-jobs-task-stop-session";
	let key = task_key("toolu_stop");
	let resource = "octofs://jobs/not-a-task";
	clear_for_session(sid);
	let (mut stop, claim) = register_task_for_session(sid, "octofs", &key, "shell: cargo test");
	register_for_session(sid, "octofs", resource, "cargo check");

	assert!(set_status_for_claim(sid, &key, claim, "shell: cargo test · running 5s"));
	let pending = pending_resources_for_session(sid);
	assert_eq!(pending[0].status.as_deref(), Some("shell: cargo test · running 5s"));
	assert_eq!(pending[1].status, None);

	assert!(!stop_task_for_session(sid, resource), "a resource is not a task");
	assert!(!stop_task_for_session(sid, "mcp-task:unknown"));
	assert!(!*stop.borrow());
	assert!(stop_task_for_session(sid, &key));
	assert!(*stop.borrow_and_update(), "the poller sees the stop request");

	// Teardown asks for the stop and drops the entry; the signal closes on it.
	clear_for_session(sid);
	assert!(stop.has_changed().is_err(), "the stop signal closes on teardown");
	assert!(*stop.borrow(), "and carries the stop request");
}

/// Receive the next event for `session_id`, skipping events other parallel
/// tests publish for their own sessions (the channel is global).
async fn next_event_for(events: &mut tokio::sync::broadcast::Receiver<WatchEvent>, session_id: &str) -> WatchEvent {
	loop {
		let event = tokio::time::timeout(std::time::Duration::from_secs(5), events.recv())
			.await
			.expect("event within timeout")
			.expect("channel open");
		let foreign = match &event {
			WatchEvent::Completed { session_id: s, .. } => s != session_id,
			WatchEvent::Cleared { session_id: s } => s != session_id,
		};
		if !foreign {
			return event;
		}
	}
}

/// Assert no event for `session_id` is pending. Own-session events are
/// published synchronously by the mutating call, so anything queued for us is
/// already here; foreign events from parallel tests are drained and ignored.
fn assert_no_event_for(events: &mut tokio::sync::broadcast::Receiver<WatchEvent>, session_id: &str) {
	loop {
		match events.try_recv() {
			Ok(event) => {
				let mine = match &event {
					WatchEvent::Completed { session_id: s, .. } => s == session_id,
					WatchEvent::Cleared { session_id: s } => s == session_id,
				};
				assert!(!mine, "expected no event for {session_id}, got {event:?}");
			}
			// Lagged: foreign traffic overflowed the buffer. Keep draining.
			Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => continue,
			Err(_) => return,
		}
	}
}

#[tokio::test]
async fn completing_a_watched_resource_publishes_an_event() {
	let sid = "shell-jobs-events-complete-session";
	clear_for_session(sid);
	register_for_session(sid, "octofs", "octofs://jobs/ev-1", "shell: build");
	let mut events = subscribe_events();

	assert!(complete_for_session(
		sid,
		"octofs://jobs/ev-1",
		"status: exited with code 0"
	));
	match next_event_for(&mut events, sid).await {
		WatchEvent::Completed { session_id, uri } => {
			assert_eq!(session_id, sid);
			assert_eq!(uri, "octofs://jobs/ev-1");
		}
		other => panic!("expected Completed event, got {other:?}"),
	}

	// Unwatched completions publish nothing.
	assert!(!complete_for_session(sid, "octofs://jobs/ev-1", "duplicate"));
	assert_no_event_for(&mut events, sid);
	clear_for_session(sid);
}

#[tokio::test]
async fn clearing_a_session_publishes_one_cleared_event() {
	let sid = "shell-jobs-events-clear-session";
	clear_for_session(sid);
	register_for_session(sid, "octofs", "octofs://jobs/ev-2", "shell: test");
	let mut events = subscribe_events();

	clear_for_session(sid);
	match next_event_for(&mut events, sid).await {
		WatchEvent::Cleared { session_id } => assert_eq!(session_id, sid),
		other => panic!("expected Cleared event, got {other:?}"),
	}

	// Clearing an already-empty session publishes nothing.
	clear_for_session(sid);
	assert_no_event_for(&mut events, sid);
}

/// Losing track is per job and only on a sustained outage: the job whose
/// budget ran out gets one terminal result — an error delivery under its URI,
/// banked under its label, the `Completed` event a real completion publishes —
/// while a job that merely started long ago in the same session is tracked as
/// ever, and neither other sessions nor banked completions are touched.
#[tokio::test]
async fn losing_track_of_a_job_drops_only_that_job() {
	let sid = "shell-jobs-outage-session";
	let other = "shell-jobs-outage-other-session";
	let lost = "octofs://jobs/lost";
	let old = "octofs://jobs/old";
	clear_for_session(sid);
	clear_for_session(other);
	register_for_session(sid, "octofs", lost, "shell: cargo build");
	register_for_session(sid, "octofs", old, "shell: cargo test");
	register_for_session(other, "octofs", "octofs://jobs/elsewhere", "shell: other session");
	register_for_session(sid, "octofs", "octofs://jobs/banked", "shell: cargo clippy");
	assert!(complete_for_session(sid, "octofs://jobs/banked", "built"));
	// Four hours of work without a word is no outage: nothing fails to observe it.
	let old_claim = registration_for_session(sid, old).expect("tracked");
	let lost_claim = registration_for_session(sid, lost).expect("tracked");
	age_for_tests(sid, old);
	assert_eq!(observation_failed_for_session(sid, old, old_claim), Some(false));
	assert_eq!(
		observation_failed_for_session(sid, lost, lost_claim),
		Some(false),
		"the first failure starts the budget"
	);
	assert!(exhaust_outage_for_tests(sid, lost));
	assert_eq!(
		observation_failed_for_session(sid, lost, lost_claim),
		Some(true),
		"run out without a success in between"
	);
	let mut events = subscribe_events();

	crate::session::context::with_session_id(sid.to_string(), async {
		crate::session::inbox::init_inbox_for_session();
		let claim = claim_delivery_for_session(sid, "octofs", lost).expect("watched");
		assert!(tracking_lost_for_session(
			sid,
			lost,
			false,
			claim,
			&TrackingLoss::Unreachable("every status check failed".to_string())
		));
		assert!(!is_watched_for_session(sid, lost));
		assert!(
			is_watched_for_session(sid, old),
			"a job started long ago is still tracked"
		);
		assert!(has_pending_for_session(other), "other sessions are untouched");
		let banked = take_completed_for_session(sid);
		assert_eq!(banked.len(), 2, "banked completions stay; the lost job joins them");
		assert_eq!(
			(banked[1].label.as_str(), banked[1].uri.as_str()),
			("shell: cargo build", lost)
		);
		assert!(
			banked[1].body.contains("lost track") && banked[1].body.contains("outcome is unknown"),
			"{}",
			banked[1].body
		);
		// One terminal result, attributed to the lost job alone: its URI in the
		// tag, the error attribute marking it, nothing for the old one.
		let delivered = crate::session::inbox::drain_inbox_batch();
		assert_eq!(delivered.len(), 1, "{delivered:?}");
		assert!(matches!(
			&delivered[0].source,
			crate::session::inbox::InboxSource::BackgroundJob { id } if id.as_str() == lost
		));
		assert_eq!(
			delivered[0].content,
			delivery_message(
				&format!("<background_job resource=\"{lost}\"{DELIVERY_ERROR_ATTR}>"),
				&banked[1].body
			)
		);
	})
	.await;

	match next_event_for(&mut events, sid).await {
		WatchEvent::Completed { session_id, uri } => {
			assert_eq!(session_id, sid);
			assert_eq!(uri, lost);
		}
		other => panic!("expected a Completed event, got {other:?}"),
	}
	// Not a teardown: the session was not wiped, so no Cleared event follows.
	assert_no_event_for(&mut events, sid);
	clear_for_session(sid);
	clear_for_session(other);
}

#[tokio::test]
async fn a_job_is_never_dropped_by_age_alone() {
	let sid = "shell-jobs-age-session";
	let fresh = "octofs://jobs/fresh";
	let key = task_key("toolu_fresh");
	clear_for_session(sid);
	let (stop, claim) = register_task_for_session(sid, "octofs", &key, "shell: cargo test");
	register_for_session(sid, "octofs", fresh, "shell: cargo check");
	let fresh_claim = registration_for_session(sid, fresh).expect("registered");
	age_for_tests(sid, &key);
	age_for_tests(sid, fresh);
	let mut events = subscribe_events();

	// No probe has failed, so there is no budget to run out; one that fails now
	// only starts one.
	assert!(!exhaust_outage_for_tests(sid, &key));
	assert_eq!(observation_failed_for_session(sid, fresh, fresh_claim), Some(false));
	assert_eq!(observation_failed_for_session(sid, &key, claim), Some(false));
	assert!(is_watched_for_session(sid, fresh));
	assert!(!*stop.borrow(), "a task nobody failed to observe is not stopped");
	assert!(has_pending_for_session(sid));
	assert_no_event_for(&mut events, sid);
	clear_for_session(sid);
}

/// Losing track of a task drops its entry the way completion does — its stop
/// signal closes without a stop request, so the poller ends without cancelling
/// the task — and its terminal result is delivered under the task's key, like
/// the poller's own.
#[tokio::test]
async fn losing_track_of_the_last_task_releases_its_poller_without_a_stop() {
	let sid = "shell-jobs-outage-last-task";
	let key = task_key("toolu_lost");
	clear_for_session(sid);
	let (mut stop, claim) = register_task_for_session(sid, "octofs", &key, "shell: cargo test");
	assert!(!*stop.borrow_and_update());
	assert_eq!(observation_failed_for_session(sid, &key, claim), Some(false));
	assert!(exhaust_outage_for_tests(sid, &key));
	let mut events = subscribe_events();

	crate::session::context::with_session_id(sid.to_string(), async {
		crate::session::inbox::init_inbox_for_session();
		assert_eq!(observation_failed_for_session(sid, &key, claim), Some(true));
		let claim = claim_delivery_for_session(sid, "octofs", &key).expect("watched");
		assert!(tracking_lost_for_session(
			sid,
			&key,
			true,
			claim,
			&TrackingLoss::Unreachable("no status check succeeded".to_string())
		));
		assert!(!has_pending_for_session(sid));
		assert!(stop.has_changed().is_err(), "the stop signal closes");
		assert!(!*stop.borrow(), "but nothing asked to stop the task");
		let delivered = crate::session::inbox::drain_inbox_batch();
		assert_eq!(delivered.len(), 1, "{delivered:?}");
		let open_tag = format!("<background_job task=\"{key}\"{DELIVERY_ERROR_ATTR}>");
		assert_eq!(
			delivered[0].content,
			delivery_message(&open_tag, &take_completed_for_session(sid)[0].body)
		);
	})
	.await;
	match next_event_for(&mut events, sid).await {
		WatchEvent::Completed { uri, .. } => assert_eq!(uri, key),
		other => panic!("expected a Completed event, got {other:?}"),
	}
	assert_no_event_for(&mut events, sid);
	clear_for_session(sid);
}

/// The budget belongs to a continuous outage: it starts at the first failed
/// observation, a success starts it over, and each job keeps its own.
#[test]
fn an_outage_budget_runs_from_the_first_failure_and_resets_on_success() {
	let sid = "shell-jobs-outage-budget";
	let job = "octofs://jobs/flaky";
	let quiet = "octofs://jobs/quiet";
	clear_for_session(sid);
	register_for_session(sid, "octofs", job, "shell: cargo build");
	let claim = registration_for_session(sid, job).expect("registered");
	register_for_session(sid, "octofs", quiet, "shell: cargo test");

	assert_eq!(
		observation_failed_for_session(sid, job, claim),
		Some(false),
		"the first failure only starts the budget"
	);
	assert!(exhaust_outage_for_tests(sid, job));
	assert_eq!(
		observation_failed_for_session(sid, job, claim),
		Some(true),
		"run out without a success in between"
	);
	assert!(!exhaust_outage_for_tests(sid, quiet), "the other job has no outage");
	assert_eq!(
		observation_failed_for_session(sid, "octofs://jobs/unknown", claim),
		None,
		"nobody watches it"
	);

	assert!(observation_recovered_for_session(sid, job, claim));
	assert!(!exhaust_outage_for_tests(sid, job), "a success cleared the outage");
	assert_eq!(
		observation_failed_for_session(sid, job, claim),
		Some(false),
		"the next failure starts a fresh budget"
	);
	assert!(is_watched_for_session(sid, job), "and the job is still tracked");
	clear_for_session(sid);
}

/// Cancellation has one owner: an explicit stop or teardown sends it; a
/// dropped entry — a completion here — only closes the signal, which ends the
/// poller without cancelling the task.
#[test]
fn teardown_requests_a_stop_but_a_dropped_entry_only_closes_the_signal() {
	let sid = "shell-jobs-stop-ownership";
	let key = task_key("toolu_owner");
	clear_for_session(sid);
	let (stop, _) = register_task_for_session(sid, "octofs", &key, "shell: cargo test");
	assert!(complete_for_session(sid, &key, "built"));
	assert!(
		stop.has_changed().is_err(),
		"completion drops the entry and closes the signal"
	);
	assert!(!*stop.borrow(), "but never asks to stop the task");

	let (stop, _) = register_task_for_session(sid, "octofs", &key, "shell: cargo test");
	clear_for_session(sid);
	assert!(stop.has_changed().is_err());
	assert!(
		*stop.borrow(),
		"teardown is a deliberate end: the task is stopped on the server"
	);
	clear_for_session(sid);
}

/// A delivery still in flight when its job's outage budget runs out adds no
/// second outcome, whichever side finalizes first: the real delivery takes the
/// job and the lost-tracking result finds nothing, or the lost-tracking result
/// is delivered and the stale claim delivers nothing — not to the inbox, not
/// to the ledger.
#[tokio::test]
async fn losing_track_of_a_job_mid_delivery_leaves_one_outcome() {
	let sid = "shell-jobs-outage-mid-delivery";
	let first = "octofs://jobs/first";
	let job = "octofs://jobs/slow-read";
	let open_tag = |uri: &str| format!("<background_job resource=\"{uri}\">");
	clear_for_session(sid);
	register_for_session(sid, "octofs", first, "shell: cargo check");
	register_for_session(sid, "octofs", job, "shell: cargo build");
	let first_claim = claim_delivery_for_session(sid, "octofs", first).expect("a read is in flight");
	let job_claim = claim_delivery_for_session(sid, "octofs", job).expect("a read is in flight");

	crate::session::context::with_session_id(sid.to_string(), async {
		crate::session::inbox::init_inbox_for_session();
		// The first read returned while its outage was running out — the real
		// result is the outcome. It is queued, and the lost-tracking result then
		// finds nothing to take: no second outcome.
		assert_eq!(observation_failed_for_session(sid, first, first_claim), Some(false));
		assert!(exhaust_outage_for_tests(sid, first));
		assert!(deliver_for_session(
			sid,
			first,
			"status: exited with code 0",
			first_claim,
			&open_tag(first)
		));
		assert_eq!(
			observation_failed_for_session(sid, first, first_claim),
			None,
			"the entry is gone"
		);
		assert!(
			claim_delivery_for_session(sid, "octofs", first).is_none(),
			"nothing left to take for the first"
		);
		let delivered = crate::session::inbox::drain_inbox_batch();
		assert_eq!(delivered.len(), 1, "{delivered:?}");
		assert!(delivered[0].content.contains("status: exited with code 0"));
		assert_eq!(
			take_completed_for_session(sid).len(),
			1,
			"only the first job's own outcome"
		);

		// The second read is still out when the budget runs out: the reader
		// delivers the lost-tracking result under its own claim; the late
		// completion then lands nothing.
		assert_eq!(observation_failed_for_session(sid, job, job_claim), Some(false));
		assert!(exhaust_outage_for_tests(sid, job));
		assert_eq!(observation_failed_for_session(sid, job, job_claim), Some(true));
		assert!(tracking_lost_for_session(
			sid,
			job,
			false,
			job_claim,
			&TrackingLoss::Unreachable("reading it failed: boom".to_string())
		));
		assert!(
			!deliver_for_session(sid, job, "the real output", job_claim, &open_tag(job)),
			"a second delivery lands nothing"
		);
		let delivered = crate::session::inbox::drain_inbox_batch();
		assert_eq!(delivered.len(), 1, "only the terminal result: {delivered:?}");
		assert!(delivered[0].content.contains("lost track"));
		let banked = take_completed_for_session(sid);
		assert_eq!(banked.len(), 1);
		assert!(banked[0].body.contains("lost track"), "the late output is not banked");
	})
	.await;
	clear_for_session(sid);
}

/// A claim outlives its registration at the claimant's peril, not the job's:
/// when the job lost tracking and was launched again under the same key before
/// the read returned, the stale claim delivers nothing and the fresh
/// registration delivers its own result.
#[tokio::test]
async fn a_stale_claim_cannot_deliver_for_a_replaced_registration() {
	let sid = "shell-jobs-outage-replaced";
	let job = "octofs://jobs/rerun";
	let open_tag = format!("<background_job resource=\"{job}\">");
	clear_for_session(sid);
	register_for_session(sid, "octofs", job, "shell: cargo build");
	let stale = claim_delivery_for_session(sid, "octofs", job).expect("the first read is in flight");
	assert_eq!(observation_failed_for_session(sid, job, stale), Some(false));
	assert!(exhaust_outage_for_tests(sid, job));

	crate::session::context::with_session_id(sid.to_string(), async {
		crate::session::inbox::init_inbox_for_session();
		assert_eq!(observation_failed_for_session(sid, job, stale), Some(true));
		assert!(
			tracking_lost_for_session(
				sid,
				job,
				false,
				stale,
				&TrackingLoss::Unreachable("every status check failed".to_string())
			),
			"the first registration lost tracking"
		);
		register_for_session(sid, "octofs", job, "shell: cargo build");
		let fresh =
			claim_delivery_for_session(sid, "octofs", job).expect("a second read claims the fresh registration");
		assert!(!deliver_for_session(
			sid,
			job,
			"the first run's late output",
			stale,
			&open_tag
		));
		assert!(
			is_watched_for_session(sid, job),
			"the fresh registration is still pending"
		);
		assert_eq!(
			observation_failed_for_session(sid, job, fresh),
			Some(false),
			"and starts with no outage of its own"
		);
		assert_eq!(
			observation_failed_for_session(sid, job, stale),
			None,
			"the replaced one is gone"
		);
		assert!(deliver_for_session(
			sid,
			job,
			"the second run's output",
			fresh,
			&open_tag
		));
		let delivered = crate::session::inbox::drain_inbox_batch();
		assert_eq!(
			delivered.len(),
			2,
			"the lost-tracking result and the fresh result, nothing else: {delivered:?}"
		);
		assert!(delivered
			.iter()
			.all(|m| !m.content.contains("the first run's late output")));
		let banked = take_completed_for_session(sid);
		assert_eq!(banked.len(), 2);
		assert!(banked.iter().all(|c| !c.body.contains("the first run's late output")));
	})
	.await;
	clear_for_session(sid);
}

/// The budget is each registration's own: a job that completed and was launched
/// again under the same key starts with no outage, whatever its predecessor's.
#[tokio::test]
async fn a_relaunched_job_starts_with_no_outage_of_its_own() {
	let sid = "shell-jobs-outage-relaunched";
	let job = "octofs://jobs/relaunched";
	clear_for_session(sid);
	register_for_session(sid, "octofs", job, "shell: cargo build");
	let spent = registration_for_session(sid, job).expect("registered");
	assert_eq!(observation_failed_for_session(sid, job, spent), Some(false));
	assert!(exhaust_outage_for_tests(sid, job));
	assert!(complete_for_session(sid, job, "status: exited with code 0"));
	register_for_session(sid, "octofs", job, "shell: cargo build");
	let fresh = registration_for_session(sid, job).expect("registered");

	crate::session::context::with_session_id(sid.to_string(), async {
		crate::session::inbox::init_inbox_for_session();
		assert_eq!(
			observation_failed_for_session(sid, job, fresh),
			Some(false),
			"the fresh registration has no outage yet"
		);
		assert_eq!(
			observation_failed_for_session(sid, job, spent),
			None,
			"the completed one is gone"
		);
		assert!(is_watched_for_session(sid, job));
		assert!(
			crate::session::inbox::drain_inbox_batch().is_empty(),
			"no lost-tracking result lands for it"
		);
	})
	.await;
	clear_for_session(sid);
}

/// An observer's claim is its own registration's: once that registration is
/// replaced under the same key, every registry call it makes is refused — its
/// failures and recoveries never touch the replacement's budget or status — and
/// it is told so (`None`, `false`) instead of being answered for the new job.
#[tokio::test]
async fn a_stale_observer_cannot_touch_a_replaced_registration() {
	let sid = "shell-jobs-stale-observer";
	let job = "octofs://jobs/replaced";
	clear_for_session(sid);
	register_for_session(sid, "octofs", job, "shell: cargo build");
	let first = registration_for_session(sid, job).expect("registered");
	assert_eq!(observation_failed_for_session(sid, job, first), Some(false));
	register_for_session(sid, "octofs", job, "shell: cargo build");
	let second = registration_for_session(sid, job).expect("tracked");

	assert_eq!(
		observation_failed_for_session(sid, job, first),
		None,
		"its registration is gone"
	);
	assert!(!observation_recovered_for_session(sid, job, first));
	assert!(!set_status_for_claim(sid, job, first, "from the stale observer"));
	assert!(!observer_is_current(sid, job, first));
	// The replacement starts clean: neither the predecessor's outage nor its status.
	assert!(!exhaust_outage_for_tests(sid, job), "no outage of the predecessor's");
	assert_eq!(pending_resources_for_session(sid)[0].status, None);
	assert!(observer_is_current(sid, job, second));
	assert_eq!(observation_failed_for_session(sid, job, second), Some(false));
	assert!(set_status_for_claim(sid, job, second, "from its own observer"));
	clear_for_session(sid);
}

/// A task's poller gets its claim with its stop signal, from the one insertion:
/// the claim names the registration that is there, and stops naming it once
/// that registration is gone.
#[test]
fn a_task_registration_comes_with_its_own_claim() {
	let sid = "shell-jobs-task-claim";
	let key = task_key("toolu_claim");
	clear_for_session(sid);
	let (_stop, claim) = register_task_for_session(sid, "octofs", &key, "shell: cargo test");

	assert!(observer_is_current(sid, &key, claim));
	let (name, config) = server_for_session(sid, &key, claim).expect("the claim names the registration");
	assert_eq!(&*name, "octofs");
	assert!(
		config.is_none(),
		"a task registered by name alone has no server to reconnect to"
	);
	assert!(complete_for_session(sid, &key, "built"));
	assert!(!observer_is_current(sid, &key, claim));
	assert_eq!(observation_failed_for_session(sid, &key, claim), None);
	clear_for_session(sid);
}

#[test]
fn delivery_round_trips_the_mcp_output_verbatim() {
	// The job's own output may quote the closing tag; the last one is ours.
	let output = "status: exited with code 1\n</background_job> quoted by the job\nerror: boom";
	let open_tag = "<background_job resource=\"octofs://jobs/7\">";
	let content = delivery_message(open_tag, output);
	assert_eq!(split_delivery(&content), Some((open_tag, output)));
}

#[test]
fn foreign_content_is_not_a_delivery() {
	assert_eq!(split_delivery("job done"), None);
	assert_eq!(split_delivery("<background_job resource=\"x\">\nno closing tag"), None);
}
