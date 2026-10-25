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
use serde_json::json;
use std::time::Duration;
use tokio::sync::mpsc;

/// Upper bound for any single await — a hang fails fast.
const WAIT: Duration = Duration::from_secs(2);

fn form(message: &str) -> ElicitRequestParams {
	ElicitRequestParams::FormElicitationParams {
		meta: None,
		message: message.to_string(),
		requested_schema: serde_json::from_value(json!({
			"type": "object",
			"properties": { "ok": { "type": "boolean" } }
		}))
		.expect("valid schema fixture"),
	}
}

/// A session answered by a client, with its notification channel.
fn client_session(session: &str) -> mpsc::UnboundedReceiver<ServerMessage> {
	let (frames_tx, frames) = mpsc::unbounded_channel();
	crate::session::context::set_notification_sender_for_session(&session.to_string(), frames_tx);
	set_answerer(session, Some(Answerer::Client));
	frames
}

fn forget(session: &str) {
	crate::session::context::clear_notification_sender_for_session(&session.to_string());
	clear_session(session);
}

async fn next_question(frames: &mut mpsc::UnboundedReceiver<ServerMessage>) -> ElicitationPayload {
	match tokio::time::timeout(WAIT, frames.recv()).await {
		Ok(Some(ServerMessage::Elicitation(question))) => question,
		other => panic!("expected an elicitation frame, got {other:?}"),
	}
}

#[tokio::test]
async fn without_a_session_or_answerer_the_question_is_declined() {
	let declined = ask("srv", None, None, form("anyone?"))
		.await
		.expect("declining is not an error");
	assert_eq!(declined.action, ElicitationAction::Decline);

	let declined = ask("srv", Some("elicitation-unanswered"), None, form("anyone?"))
		.await
		.expect("declining is not an error");
	assert_eq!(declined.action, ElicitationAction::Decline);
}

#[test]
fn set_answerer_none_forgets_the_answerer() {
	let session = "elicitation-forget";
	set_answerer(session, Some(Answerer::Terminal));
	assert_eq!(answerer(session), Some(Answerer::Terminal));
	set_answerer(session, None);
	assert_eq!(answerer(session), None);
}

/// The client receives MCP's params unchanged, answers once by id, and the
/// asker gets exactly that answer.
#[tokio::test]
async fn a_client_answer_reaches_the_asker_once() {
	let session = "elicitation-client-answer";
	let mut frames = client_session(session);
	let asker = tokio::spawn(async move { ask("srv", Some(session), Some("tool-1"), form("Refund?")).await });

	let question = next_question(&mut frames).await;
	assert_eq!(question.session_id, session);
	assert_eq!(question.server, "srv");
	assert_eq!(question.tool_id.as_deref(), Some("tool-1"));
	assert_eq!(question.params, form("Refund?"));

	let answer = ElicitResult::new(ElicitationAction::Accept).with_content(json!({ "ok": true }));
	assert!(resolve(session, &question.id, answer.clone()));
	assert!(
		!resolve(session, &question.id, answer.clone()),
		"a question is answered once"
	);
	let received = tokio::time::timeout(WAIT, asker)
		.await
		.expect("the asker must get its answer")
		.expect("asker task must not panic")
		.expect("a client answer is not an error");
	assert_eq!(received, answer);
	forget(session);
}

/// A client that binds after the question was asked still receives it.
#[tokio::test]
async fn unanswered_questions_are_re_sent_after_a_bind() {
	let session = "elicitation-resend";
	let mut frames = client_session(session);
	let asker = tokio::spawn(async move { ask("srv", Some(session), None, form("Still there?")).await });
	let question = next_question(&mut frames).await;

	resend_pending(session);
	let again = next_question(&mut frames).await;
	assert_eq!(
		again.id, question.id,
		"a re-send carries the same id, so clients dedupe"
	);

	assert!(resolve(
		session,
		&question.id,
		ElicitResult::new(ElicitationAction::Decline)
	));
	let received = tokio::time::timeout(WAIT, asker)
		.await
		.expect("the asker must get its answer")
		.expect("asker task must not panic")
		.expect("a client answer is not an error");
	assert_eq!(received.action, ElicitationAction::Decline);
	forget(session);
}

#[tokio::test]
async fn ending_the_session_cancels_its_open_questions() {
	let session = "elicitation-session-end";
	let mut frames = client_session(session);
	let asker = tokio::spawn(async move { ask("srv", Some(session), None, form("Still there?")).await });
	let question = next_question(&mut frames).await;

	forget(session);
	let received = tokio::time::timeout(WAIT, asker)
		.await
		.expect("the asker must not wait forever")
		.expect("asker task must not panic")
		.expect("a cancelled question is not an error");
	assert_eq!(received.action, ElicitationAction::Cancel);
	assert!(!resolve(
		session,
		&question.id,
		ElicitResult::new(ElicitationAction::Accept)
	));
}

/// An asker that gives up (its tool call ended) withdraws the question: it is
/// neither re-sent nor answerable.
#[tokio::test]
async fn a_withdrawn_question_is_forgotten() {
	let session = "elicitation-withdrawn";
	let mut frames = client_session(session);
	let asker = tokio::spawn(async move { ask("srv", Some(session), None, form("Still there?")).await });
	let question = next_question(&mut frames).await;

	asker.abort();
	assert!(asker.await.is_err(), "the asker was aborted");
	resend_pending(session);
	assert!(frames.try_recv().is_err(), "a withdrawn question is not re-sent");
	assert!(!resolve(
		session,
		&question.id,
		ElicitResult::new(ElicitationAction::Accept)
	));
	forget(session);
}
