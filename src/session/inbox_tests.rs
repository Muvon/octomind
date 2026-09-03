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

//! Inbox queue semantics inside a unique task-local session id.

use super::*;

fn schedule_msg(content: &str) -> InboxMessage {
	InboxMessage {
		source: InboxSource::Schedule {
			id: "sched1".to_string(),
		},
		content: content.to_string(),
	}
}

#[tokio::test]
async fn test_inbox_fifo_roundtrip() {
	crate::session::context::with_session_id("inbox-test-fifo".to_string(), async {
		init_inbox_for_session();
		assert!(!has_inbox_messages());
		assert!(try_pop_inbox_message().is_none());

		push_inbox_message(schedule_msg("first"));
		push_inbox_message(schedule_msg("second"));
		assert!(has_inbox_messages());
		assert!(peek_inbox_preview(&crate::session::context::expect_session_id()).is_some());

		let first = try_pop_inbox_message().expect("first message");
		assert_eq!(first.content, "first");
		let second = try_pop_inbox_message().expect("second message");
		assert_eq!(second.content, "second");
		assert!(!has_inbox_messages());

		clear_inbox_for_session(&crate::session::context::expect_session_id());
	})
	.await;
}

#[tokio::test]
async fn test_reinit_keeps_queued_messages() {
	// Resuming a live session re-runs the init sequence. A schedule that fired
	// in between must still be delivered — see the ws/acp resume path.
	crate::session::context::with_session_id("inbox-test-reinit".to_string(), async {
		init_inbox_for_session();
		push_inbox_message(schedule_msg("queued before resume"));

		init_inbox_for_session();

		let msg = try_pop_inbox_message().expect("message survives re-init");
		assert_eq!(msg.content, "queued before resume");

		clear_inbox_for_session(&crate::session::context::expect_session_id());
	})
	.await;
}

#[tokio::test]
async fn test_inbox_message_display_metadata() {
	let msg = schedule_msg("do the rounds");
	assert!(!msg.source.display_label().is_empty());
	assert!(!msg.source.display_kind().is_empty());
	assert!(!msg.source.display_icon().is_empty());
	// Schedule injections drive the next action but are system-managed
	// control-plane turns, not genuine user asks.
	assert!(msg.source.is_system_managed());

	display_injected_input(&msg);
}

#[tokio::test]
async fn test_mid_turn_pop_takes_results_and_leaves_user_injections() {
	// The tool loop delivers results a running turn may be waiting on, but a
	// human-shaped injection carries a new task and must start its own turn.
	crate::session::context::with_session_id("inbox-test-mid-turn".to_string(), async {
		init_inbox_for_session();
		push_inbox_message(InboxMessage {
			source: InboxSource::Inject,
			content: "user says something new".to_string(),
		});
		push_inbox_message(InboxMessage {
			source: InboxSource::BackgroundJob {
				id: "80551-17".to_string(),
			},
			content: "<background_job>exited with code 0</background_job>".to_string(),
		});

		let job = try_pop_system_managed_message().expect("job result delivered mid-turn");
		assert!(job.content.contains("exited with code 0"));
		assert!(
			try_pop_system_managed_message().is_none(),
			"a user injection is not delivered mid-turn"
		);

		let queued = try_pop_inbox_message().expect("user injection still queued");
		assert_eq!(queued.content, "user says something new");

		clear_inbox_for_session(&crate::session::context::expect_session_id());
	})
	.await;
}
