// Copyright 2025 Muvon Un Limited
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

//! Session inbox — unified queue for all injected user messages.
//!
//! Every source that needs to inject a message into the session loop
//! (scheduled timers, completed background agents, skill activations, …)
//! pushes an [`InboxMessage`] here.  The session loop drains the inbox at
//! the right moment — either immediately when idle, or after the current
//! API round-trip finishes.
//!
//! This replaces three separate ad-hoc mechanisms:
//!   - `ChatSession.pending_prompt`  (single-slot, schedule + job injection)
//!   - `ChatSession.job_rx`          (mpsc channel for background agents)
//!   - `PENDING_SKILL_INJECTIONS`    (static map in context.rs)

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, RwLock};

use tokio::sync::Notify;

use crate::session::context::SessionId;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Where the injected message came from.  Used for logging / debugging only.
#[derive(Debug, Clone)]
pub enum InboxSource {
	/// A `schedule` tool entry that fired at its configured time.
	Schedule { id: String },
	/// A background agent job that completed (success or failure).
	BackgroundAgent { name: String },
	/// A `skill(use)` activation that needs its content injected.
	Skill { name: String },
	/// An external injection via `octomind inject` CLI command.
	Inject,
	/// A webhook hook that received an HTTP request.
	Webhook { hook: String },
}

/// A message waiting to be injected into the session as a user turn.
#[derive(Debug, Clone)]
pub struct InboxMessage {
	pub source: InboxSource,
	pub content: String,
}

// ---------------------------------------------------------------------------
// Internal registry
// ---------------------------------------------------------------------------

/// Per-session inbox: a queue of pending messages plus a Notify for wakeup.
struct InboxQueue {
	messages: VecDeque<InboxMessage>,
	/// Notified whenever a message is pushed.  The session loop awaits this
	/// to wake up from the `select!` arm without busy-polling.
	notify: Arc<Notify>,
}

static INBOX: RwLock<Option<HashMap<SessionId, InboxQueue>>> = RwLock::new(None);

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

/// Create an empty inbox for a session.  Call once, right after
/// `with_session_id` establishes the session context.
pub fn init_inbox_for_session() {
	let session_id = match crate::session::context::current_session_id() {
		Some(id) => id,
		None => return,
	};
	let mut guard = INBOX.write().unwrap();
	let registry = guard.get_or_insert_with(HashMap::new);
	registry.insert(
		session_id,
		InboxQueue {
			messages: VecDeque::new(),
			notify: Arc::new(Notify::new()),
		},
	);
}

/// Destroy the inbox for a session.  Called from `cleanup_session`.
pub fn clear_inbox_for_session(session_id: &SessionId) {
	if let Ok(mut guard) = INBOX.write() {
		if let Some(registry) = guard.as_mut() {
			registry.remove(session_id);
		}
	}
}

// ---------------------------------------------------------------------------
// Producer API
// ---------------------------------------------------------------------------

/// Push a message into the current session's inbox and wake the loop.
///
/// Resolves the session ID from the task-local context automatically.
/// Safe to call from any thread / async context.  If the session inbox does
/// not exist (session already cleaned up) the message is silently dropped.
pub fn push_inbox_message(msg: InboxMessage) {
	let session_id = match crate::session::context::current_session_id() {
		Some(id) => id,
		None => return,
	};
	let mut guard = INBOX.write().unwrap();
	if let Some(registry) = guard.as_mut() {
		if let Some(q) = registry.get_mut(&session_id) {
			q.messages.push_back(msg);
			q.notify.notify_one();
		}
	}
}

/// Push a message into a specific session's inbox by explicit session ID.
///
/// Use this when the caller is NOT running inside a session context
/// (e.g. a `tokio::spawn`-ed task that doesn't inherit the task-local).
pub fn push_inbox_message_for_session(session_id: &str, msg: InboxMessage) {
	let mut guard = INBOX.write().unwrap();
	if let Some(registry) = guard.as_mut() {
		if let Some(q) = registry.get_mut(session_id) {
			q.messages.push_back(msg);
			q.notify.notify_one();
		}
	}
}

// ---------------------------------------------------------------------------
// Consumer API
// ---------------------------------------------------------------------------

/// Pop the next pending message for the current session, or `None` if empty.
pub fn try_pop_inbox_message() -> Option<InboxMessage> {
	let session_id = crate::session::context::current_session_id()?;
	let mut guard = INBOX.write().unwrap();
	let registry = guard.as_mut()?;
	let queue = registry.get_mut(&session_id)?;
	queue.messages.pop_front()
}

/// Returns `true` if there is at least one message waiting for the current session.
pub fn has_inbox_messages() -> bool {
	let session_id = match crate::session::context::current_session_id() {
		Some(id) => id,
		None => return false,
	};
	let guard = INBOX.read().unwrap();
	guard
		.as_ref()
		.and_then(|r| r.get(&session_id))
		.map(|q| !q.messages.is_empty())
		.unwrap_or(false)
}

/// Peek at the first inbox message for a specific session without consuming it.
/// Returns a short preview (source + truncated content) suitable for display.
/// Takes an explicit session_id so it works from any thread.
pub fn peek_inbox_preview(session_id: &str) -> Option<String> {
	let guard = INBOX.read().unwrap();
	let msg = guard
		.as_ref()
		.and_then(|r| r.get(session_id))?
		.messages
		.front()?;
	let source = match &msg.source {
		InboxSource::Schedule { .. } => "scheduled message",
		InboxSource::BackgroundAgent { name } => {
			return Some(format!("background agent '{name}'"));
		}
		InboxSource::Skill { name } => {
			return Some(format!("skill '{name}'"));
		}
		InboxSource::Inject => "external inject",
		InboxSource::Webhook { hook } => {
			return Some(format!("webhook '{hook}'"));
		}
	};
	// Truncate content preview to first line, max 80 chars
	let preview: String = msg
		.content
		.lines()
		.next()
		.unwrap_or("")
		.chars()
		.take(80)
		.collect();
	let ellipsis = if preview.len() < msg.content.len() {
		"…"
	} else {
		""
	};
	Some(format!("{source}: {preview}{ellipsis}"))
}

/// Returns the `Arc<Notify>` for the current session's inbox, or `None`.
///
/// The session loop holds this across `select!` iterations so it can
/// `.await` the notify and be woken the moment a producer pushes a message.
pub fn get_inbox_notify() -> Option<Arc<Notify>> {
	let session_id = crate::session::context::current_session_id()?;
	let guard = INBOX.read().unwrap();
	guard
		.as_ref()
		.and_then(|r| r.get(&session_id))
		.map(|q| Arc::clone(&q.notify))
}
