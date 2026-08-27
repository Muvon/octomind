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
		ContentBlock::resource_link(Resource::new(
			"octofs://jobs/1234-7",
			"shell: make reldebug",
		)),
	]);
	assert_eq!(
		resource_links_in(&launched),
		vec![(
			"octofs://jobs/1234-7".to_string(),
			"shell: make reldebug".to_string()
		)]
	);

	// Recognition is not scheme-bound: any resource link is followed. An empty
	// name falls back to the URI as the label.
	let other = CallToolResult::success(vec![ContentBlock::resource_link(Resource::new(
		"custommcp://tasks/9",
		"",
	))]);
	assert_eq!(
		resource_links_in(&other),
		vec![(
			"custommcp://tasks/9".to_string(),
			"custommcp://tasks/9".to_string()
		)]
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
	register_for_session(sid, a, "shell: make reldebug");
	register_for_session(sid, b, "shell: run tests");
	assert!(has_pending_for_session(sid));
	assert!(is_watched_for_session(sid, a));
	assert!(!is_watched_for_session(sid, "octofs://jobs/never"));

	assert!(complete_for_session(sid, a), "a was watched");
	assert!(!is_watched_for_session(sid, a));
	assert!(
		has_pending_for_session(sid),
		"b still keeps the session pending"
	);
	assert!(
		!complete_for_session(sid, "octofs://jobs/unknown"),
		"completing an unwatched uri reports not-watched"
	);

	assert!(complete_for_session(sid, b), "b was watched");
	assert!(
		!has_pending_for_session(sid),
		"the empty set is dropped, so the session is no longer pending"
	);
	assert!(
		!complete_for_session(sid, b),
		"already-cleared uri reports not-watched"
	);
	clear_for_session(sid);
}

/// Receive the next event for `session_id`, skipping events other parallel
/// tests publish for their own sessions (the channel is global).
async fn next_event_for(
	events: &mut tokio::sync::broadcast::Receiver<WatchEvent>,
	session_id: &str,
) -> WatchEvent {
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

#[tokio::test]
async fn completing_a_watched_resource_publishes_an_event() {
	let sid = "shell-jobs-events-complete-session";
	clear_for_session(sid);
	register_for_session(sid, "octofs://jobs/ev-1", "shell: build");
	let mut events = subscribe_events();

	assert!(complete_for_session(sid, "octofs://jobs/ev-1"));
	match next_event_for(&mut events, sid).await {
		WatchEvent::Completed { session_id, uri } => {
			assert_eq!(session_id, sid);
			assert_eq!(uri, "octofs://jobs/ev-1");
		}
		other => panic!("expected Completed event, got {other:?}"),
	}

	// Unwatched completions publish nothing.
	assert!(!complete_for_session(sid, "octofs://jobs/ev-1"));
	assert!(events.try_recv().is_err());
	clear_for_session(sid);
}

#[tokio::test]
async fn clearing_a_session_publishes_one_cleared_event() {
	let sid = "shell-jobs-events-clear-session";
	clear_for_session(sid);
	register_for_session(sid, "octofs://jobs/ev-2", "shell: test");
	let mut events = subscribe_events();

	clear_for_session(sid);
	match next_event_for(&mut events, sid).await {
		WatchEvent::Cleared { session_id } => assert_eq!(session_id, sid),
		other => panic!("expected Cleared event, got {other:?}"),
	}

	// Clearing an already-empty session publishes nothing.
	clear_for_session(sid);
	assert!(events.try_recv().is_err());
}
