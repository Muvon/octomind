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

#[test]
#[serial_test::serial]
fn take_handback_without_session_context_returns_zero() {
	// Outside `with_session_id` there is no key to tally under.
	assert_eq!(take_handback(), (0, 0));
}

#[tokio::test]
#[serial_test::serial]
async fn note_and_take_report_verified_run() {
	crate::session::context::with_session_id("delegate-test-verified".to_string(), async {
		note_handback(true);
		assert_eq!(take_handback(), (1, 1));
	})
	.await;
}

#[tokio::test]
#[serial_test::serial]
async fn note_false_counts_as_unverified() {
	crate::session::context::with_session_id("delegate-test-unverified".to_string(), async {
		note_handback(false);
		assert_eq!(take_handback(), (1, 0));
	})
	.await;
}

#[tokio::test]
#[serial_test::serial]
async fn multiple_notes_accumulate_until_taken() {
	crate::session::context::with_session_id("delegate-test-accumulate".to_string(), async {
		note_handback(true);
		note_handback(false);
		note_handback(true);
		assert_eq!(take_handback(), (3, 2));
	})
	.await;
}

#[tokio::test]
#[serial_test::serial]
async fn take_drains_the_tally() {
	crate::session::context::with_session_id("delegate-test-drain".to_string(), async {
		note_handback(true);
		assert_eq!(take_handback(), (1, 1));
		assert_eq!(take_handback(), (0, 0));
	})
	.await;
}

#[tokio::test]
#[serial_test::serial]
async fn sessions_are_isolated_and_clearable() {
	let a = "delegate-test-isolation-a".to_string();
	let b = "delegate-test-isolation-b".to_string();

	crate::session::context::with_session_id(a.clone(), async {
		note_handback(true);
	})
	.await;

	crate::session::context::with_session_id(b, async {
		// A's tally must not leak into B's window.
		assert_eq!(take_handback(), (0, 0));
	})
	.await;

	crate::session::context::with_session_id(a.clone(), async {
		assert_eq!(take_handback(), (1, 1));
		note_handback(false);
		// Session-end cleanup drops whatever is left untaken.
		clear_handback_for_session(&a);
		assert_eq!(take_handback(), (0, 0));
	})
	.await;
}
