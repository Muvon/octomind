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

//! Who answers an MCP server's elicitation in each session, and the questions
//! still waiting on a WebSocket or ACP client's answer.
//!
//! Every channel carries MCP's own `elicitation/create` params and result
//! unchanged: the terminal prompts for them, a client receives them as an
//! `elicitation` frame on the session's notification channel (ACP translates
//! that frame into its own `elicitation/create`) and answers through
//! [`resolve`].

use crate::websocket::protocol::ElicitationPayload;
use crate::websocket::ServerMessage;
use rmcp::model::{ElicitRequestParams, ElicitResult, ElicitationAction};
use rmcp::ErrorData;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex};
use tokio::sync::oneshot;

/// Who answers a session's elicitations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answerer {
	/// The person at the interactive CLI terminal.
	Terminal,
	/// A WebSocket or ACP client, reached through the session's notification channel.
	Client,
}

/// Set by the entry points that attach answerers to their sessions. Legacy
/// servers learn the client's capabilities once per shared connection, so
/// they are offered elicitation exactly when some session here may answer.
static ACCEPTS_ELICITATIONS: AtomicBool = AtomicBool::new(false);

static ANSWERERS: LazyLock<Mutex<HashMap<String, Answerer>>> = LazyLock::new(Default::default);

/// Unanswered client questions by (session, id).
static PENDING: LazyLock<Mutex<HashMap<(String, String), Pending>>> = LazyLock::new(Default::default);

struct Pending {
	/// Kept to re-send after a client binds.
	frame: ServerMessage,
	reply: oneshot::Sender<ElicitResult>,
}

/// Called by the interactive CLI, `octomind server` and `octomind acp`.
pub fn accept_elicitations() {
	ACCEPTS_ELICITATIONS.store(true, Ordering::Relaxed);
}

pub(crate) fn accepts_elicitations() -> bool {
	ACCEPTS_ELICITATIONS.load(Ordering::Relaxed)
}

/// Who answers this session's elicitations; `None` = nobody (they are declined).
pub fn set_answerer(session_id: &str, answerer: Option<Answerer>) {
	let mut answerers = ANSWERERS.lock().unwrap();
	match answerer {
		Some(answerer) => answerers.insert(session_id.to_string(), answerer),
		None => answerers.remove(session_id),
	};
}

pub(crate) fn answerer(session_id: &str) -> Option<Answerer> {
	ANSWERERS.lock().unwrap().get(session_id).copied()
}

/// Ask whoever answers for `session_id`; declines when nobody does.
pub(crate) async fn ask(
	server: &str,
	session_id: Option<&str>,
	tool_id: Option<&str>,
	params: ElicitRequestParams,
) -> Result<ElicitResult, ErrorData> {
	let Some(session_id) = session_id else {
		return Ok(decline(server, None, &params, "it belongs to no session"));
	};
	match answerer(session_id) {
		Some(Answerer::Terminal) => crate::session::chat::elicitation::ask(server, params).await,
		Some(Answerer::Client) => Ok(ask_client(server, session_id, tool_id, params).await),
		None => Ok(decline(
			server,
			Some(session_id),
			&params,
			"nobody in its session can answer",
		)),
	}
}

/// Decline without asking anyone. Observers still see the request as an
/// `elicitation/requested` notification.
pub(crate) fn decline(
	server: &str,
	session_id: Option<&str>,
	params: &ElicitRequestParams,
	reason: &str,
) -> ElicitResult {
	crate::log_debug!("Elicitation from '{}' declined: {}", server, reason);
	let value = serde_json::to_value(params).expect("elicitation params serialize");
	super::process::emit_notification(server, "elicitation/requested", &value, session_id, None);
	ElicitResult::new(ElicitationAction::Decline)
}

async fn ask_client(
	server: &str,
	session_id: &str,
	tool_id: Option<&str>,
	params: ElicitRequestParams,
) -> ElicitResult {
	let id = uuid::Uuid::new_v4().to_string();
	let frame = ServerMessage::Elicitation(ElicitationPayload {
		session_id: session_id.to_string(),
		id: id.clone(),
		server: server.to_string(),
		tool_id: tool_id.map(str::to_string),
		params,
	});
	let (reply, answer) = oneshot::channel();
	let _pending = PendingQuestion::register(session_id, &id, frame.clone(), reply);
	super::process::send_notification_message_for(frame, Some(&session_id.to_string()));
	match answer.await {
		Ok(result) => result,
		// The session ended before anyone answered.
		Err(_) => ElicitResult::new(ElicitationAction::Cancel),
	}
}

/// Removes its question from `PENDING` however the asking future ends, so a
/// withdrawn question is never re-sent or answered.
struct PendingQuestion((String, String));

impl PendingQuestion {
	fn register(session_id: &str, id: &str, frame: ServerMessage, reply: oneshot::Sender<ElicitResult>) -> Self {
		let key = (session_id.to_string(), id.to_string());
		PENDING.lock().unwrap().insert(key.clone(), Pending { frame, reply });
		Self(key)
	}
}

impl Drop for PendingQuestion {
	fn drop(&mut self) {
		PENDING.lock().unwrap().remove(&self.0);
	}
}

/// Deliver the user's answer to a pending question. `false` = no such pending question.
pub fn resolve(session_id: &str, id: &str, result: ElicitResult) -> bool {
	let pending = PENDING
		.lock()
		.unwrap()
		.remove(&(session_id.to_string(), id.to_string()));
	match pending {
		Some(pending) => {
			// A send error means the asker gave up meanwhile; the answer is moot.
			let _ = pending.reply.send(result);
			true
		}
		None => false,
	}
}

/// Re-send every unanswered `elicitation` frame on the session's notification channel.
pub fn resend_pending(session_id: &str) {
	let frames: Vec<ServerMessage> = PENDING
		.lock()
		.unwrap()
		.iter()
		.filter(|((pending_session, _), _)| pending_session == session_id)
		.map(|(_, pending)| pending.frame.clone())
		.collect();
	let session_id = session_id.to_string();
	for frame in frames {
		super::process::send_notification_message_for(frame, Some(&session_id));
	}
}

/// Forget a session's answerer; its unanswered questions resolve as cancel.
pub(crate) fn clear_session(session_id: &str) {
	set_answerer(session_id, None);
	PENDING
		.lock()
		.unwrap()
		.retain(|(pending_session, _), _| pending_session != session_id);
}

#[cfg(test)]
#[path = "elicitation_tests.rs"]
mod tests;
