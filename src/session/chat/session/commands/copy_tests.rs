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

//! Scope selection and payload rendering for `/copy`. The clipboard itself is
//! not touched here — `build_payload` is pure, and the one handler test drives
//! the error path, which returns before any clipboard access.

use super::*;
use crate::session::Message;

fn message(role: &str, content: &str) -> Message {
	Message {
		role: role.to_string(),
		content: content.to_string(),
		..Default::default()
	}
}

fn session_with(messages: Vec<Message>) -> ChatSession {
	ChatSession::for_tests(messages)
}

fn transcript() -> ChatSession {
	let mut session = session_with(vec![
		message("system", "system prompt"),
		message("user", "fix the parser"),
		message("assistant", "Looking at it."),
		message("tool", "file contents"),
		message("assistant", "Done — parser fixed."),
		message("user", "<system-note>\ninbox report\n</system-note>"),
		message("user", "now the tests"),
		message("assistant", "Tests pass."),
	]);
	session.last_response = "Tests pass.".to_string();
	session
}

#[test]
fn parses_every_scope_case_insensitively() {
	assert_eq!(CopyScope::parse("last"), Some(CopyScope::Last));
	assert_eq!(CopyScope::parse("ASSISTANT"), Some(CopyScope::Assistant));
	assert_eq!(CopyScope::parse("User"), Some(CopyScope::User));
	assert_eq!(CopyScope::parse("all"), Some(CopyScope::All));
	assert_eq!(CopyScope::parse("everything"), None);
	assert_eq!(CopyScope::parse(""), None);
}

#[test]
fn scope_names_round_trip() {
	for scope in CopyScope::ALL {
		assert_eq!(CopyScope::parse(scope.name()), Some(scope));
	}
}

#[test]
fn last_is_byte_identical_to_the_raw_response() {
	let session = transcript();
	assert_eq!(
		build_payload(&session, CopyScope::Last).as_deref(),
		Some("Tests pass.")
	);
}

#[test]
fn last_without_a_response_selects_nothing() {
	let session = session_with(vec![message("user", "hi")]);
	assert_eq!(build_payload(&session, CopyScope::Last), None);
}

#[test]
fn assistant_copies_prose_only_in_order() {
	let session = transcript();
	assert_eq!(
		build_payload(&session, CopyScope::Assistant).as_deref(),
		Some("**Assistant:**\n\nLooking at it.\n\n**Assistant:**\n\nDone — parser fixed.\n\n**Assistant:**\n\nTests pass.")
	);
}

#[test]
fn user_skips_system_managed_injections() {
	let session = transcript();
	assert_eq!(
		build_payload(&session, CopyScope::User).as_deref(),
		Some("**User:**\n\nfix the parser\n\n**User:**\n\nnow the tests")
	);
}

#[test]
fn all_interleaves_user_and_assistant_without_tools() {
	let session = transcript();
	assert_eq!(
		build_payload(&session, CopyScope::All).as_deref(),
		Some(
			"**User:**\n\nfix the parser\n\n**Assistant:**\n\nLooking at it.\n\n**Assistant:**\n\nDone — parser fixed.\n\n**User:**\n\nnow the tests\n\n**Assistant:**\n\nTests pass."
		)
	);
}

#[test]
fn empty_and_tool_only_messages_are_skipped() {
	let session = session_with(vec![
		message("assistant", "   "),
		message("assistant", ""),
		message("tool", "noise"),
		message("system", "noise"),
	]);
	assert_eq!(build_payload(&session, CopyScope::All), None);
}

#[test]
fn assistant_prose_riding_tool_calls_is_kept() {
	let mut message = message("assistant", "Checking the file.");
	message.tool_calls = Some(serde_json::json!([{"id": "1", "name": "read"}]));
	let session = session_with(vec![message]);
	assert_eq!(
		build_payload(&session, CopyScope::All).as_deref(),
		Some("**Assistant:**\n\nChecking the file.")
	);
}

#[tokio::test]
async fn unknown_scope_reports_the_valid_ones() {
	let session = transcript();
	let result =
		handle_copy(&session, &["everything"]).expect("unknown scope is handled, not an error");
	let CommandResult::HandledWithOutput(output) = result else {
		panic!("expected typed output");
	};
	let CommandOutput::Error { error, context } = *output else {
		panic!("expected an error output");
	};
	assert!(error.contains("everything"), "{error}");
	let scopes = context
		.and_then(|ctx| ctx.get("valid_scopes").cloned())
		.expect("valid_scopes listed");
	assert_eq!(
		scopes,
		serde_json::json!(["last", "assistant", "user", "all"])
	);
}
