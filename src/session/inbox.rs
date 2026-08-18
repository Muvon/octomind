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
	/// A rate-limited batch from a long-running `monitor` script.
	Monitor { id: String, description: String },
	/// A background agent job that completed (success or failure).
	BackgroundAgent { name: String },
	/// A background tap-run launched via the `tap` core tool.
	TapRun { id: String, role: String },
	/// A `skill(use)` activation that needs its content injected.
	Skill { name: String },
	/// A skill validator failure that needs to be fed back to the AI.
	SkillValidator { name: String },
	/// An external injection via `octomind inject` CLI command.
	Inject,
	/// A webhook hook that received an HTTP request.
	Webhook { hook: String },
	/// A guardrail post-result hook script that exited non-zero.
	GuardrailHook { script: String },
	/// A `[[validator]]` script that flagged the end-of-turn state.
	GuardValidator { name: String },
}

/// A message waiting to be injected into the session as a user turn.
#[derive(Debug, Clone)]
pub struct InboxMessage {
	pub source: InboxSource,
	pub content: String,
}

impl InboxSource {
	/// Short human-readable label for CLI rendering of an injected message.
	/// Pairs with an icon in `display_injected_input` to mimic the regular
	/// user prompt so the user can see what triggered the AI's response.
	pub fn display_label(&self) -> String {
		match self {
			InboxSource::Schedule { id } => format!("schedule {id}"),
			InboxSource::Monitor { id, description } => {
				format!("monitor {id} ({description})")
			}
			InboxSource::BackgroundAgent { name } => format!("agent {name}"),
			InboxSource::TapRun { id, role } => format!("tap-run {id} ({role})"),
			InboxSource::Skill { name } => format!("skill {name}"),
			InboxSource::SkillValidator { name } => format!("skill-validator {name}"),
			InboxSource::Inject => "inject".to_string(),
			InboxSource::Webhook { hook } => format!("webhook {hook}"),
			InboxSource::GuardrailHook { script } => format!("guardrail-hook {script}"),
			InboxSource::GuardValidator { name } => format!("validator {name}"),
		}
	}

	/// Machine-readable source kind used by structured protocols (WebSocket, ACP).
	/// snake_case so it round-trips cleanly through JSON without quoting surprises.
	pub fn display_kind(&self) -> &'static str {
		match self {
			InboxSource::Schedule { .. } => "schedule",
			InboxSource::Monitor { .. } => "monitor",
			InboxSource::BackgroundAgent { .. } => "background_agent",
			InboxSource::TapRun { .. } => "tap_run",
			InboxSource::Skill { .. } => "skill",
			InboxSource::SkillValidator { .. } => "skill_validator",
			InboxSource::Inject => "inject",
			InboxSource::Webhook { .. } => "webhook",
			InboxSource::GuardrailHook { .. } => "guardrail_hook",
			InboxSource::GuardValidator { .. } => "guardrail_validator",
		}
	}

	/// Icon shown next to an injected message in the CLI.
	pub fn display_icon(&self) -> &'static str {
		match self {
			InboxSource::Schedule { .. } => "⏰",
			InboxSource::Monitor { .. } => "📡",
			InboxSource::BackgroundAgent { .. } => "🤖",
			InboxSource::TapRun { .. } => "🚰",
			InboxSource::Skill { .. } => "🧩",
			InboxSource::SkillValidator { .. } => "⚠️",
			InboxSource::Inject => "💬",
			InboxSource::Webhook { .. } => "🪝",
			InboxSource::GuardrailHook { .. } => "🛡️",
			InboxSource::GuardValidator { .. } => "🛡️",
		}
	}

	pub fn is_system_managed(&self) -> bool {
		matches!(
			self,
			InboxSource::Schedule { .. }
				| InboxSource::Monitor { .. }
				| InboxSource::BackgroundAgent { .. }
				| InboxSource::TapRun { .. }
				| InboxSource::Skill { .. }
				| InboxSource::SkillValidator { .. }
				| InboxSource::GuardrailHook { .. }
				| InboxSource::GuardValidator { .. }
		)
	}
}

/// Render an injected inbox message to stdout so the user sees what the AI
/// is about to respond to. Mirrors the format of a user-typed prompt line
/// (source-tagged) with the message content below if multi-line.
pub fn display_injected_input(msg: &InboxMessage) {
	use colored::Colorize;

	let icon = msg.source.display_icon();
	let label = msg.source.display_label();
	let first_line = msg.content.lines().next().unwrap_or("");
	let rest = msg.content.lines().skip(1).collect::<Vec<_>>();

	println!(
		"{} {} {}",
		icon,
		format!("[{label}]").bright_black(),
		first_line
	);
	for line in rest {
		println!("   {}", line);
	}
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

/// Push a bounded monitor delivery, coalescing it with an already-pending
/// delivery from the same monitor. A slow AI turn therefore cannot build an
/// unbounded queue of periodic batches from a noisy script.
pub fn push_monitor_message_for_session(
	session_id: &str,
	id: &str,
	description: &str,
	content: String,
	max_batch_bytes: usize,
) {
	let mut guard = INBOX.write().unwrap();
	let Some(queue) = guard
		.as_mut()
		.and_then(|registry| registry.get_mut(session_id))
	else {
		return;
	};
	if let Some(pending) = queue.messages.iter_mut().find(|message| {
		matches!(&message.source, InboxSource::Monitor { id: pending_id, .. } if pending_id == id)
	}) {
		append_monitor_content(&mut pending.content, &content, max_batch_bytes);
	} else {
		let mut content = content;
		bound_monitor_content(&mut content, max_batch_bytes);
		queue.messages.push_back(InboxMessage {
			source: InboxSource::Monitor {
				id: id.to_string(),
				description: description.to_string(),
			},
			content,
		});
	}
	queue.notify.notify_one();
}

fn append_monitor_content(existing: &mut String, addition: &str, max_batch_bytes: usize) {
	const MARKER: &str = "\n[additional monitor output omitted while this delivery was pending]";
	let limit = max_batch_bytes.saturating_add(2048);
	if existing.ends_with(MARKER) {
		return;
	}
	let separator = "\n\n";
	if existing.len() + separator.len() + addition.len() <= limit {
		existing.push_str(separator);
		existing.push_str(addition);
		return;
	}

	let content_limit = limit.saturating_sub(MARKER.len());
	if existing.len() < content_limit {
		let separator = if content_limit.saturating_sub(existing.len()) >= separator.len() {
			separator
		} else {
			""
		};
		existing.push_str(separator);
		let remaining = content_limit.saturating_sub(existing.len());
		let mut end = remaining.min(addition.len());
		while end > 0 && !addition.is_char_boundary(end) {
			end -= 1;
		}
		existing.push_str(&addition[..end]);
	} else {
		let mut end = content_limit.min(existing.len());
		while end > 0 && !existing.is_char_boundary(end) {
			end -= 1;
		}
		existing.truncate(end);
	}
	existing.push_str(MARKER);
}

fn bound_monitor_content(content: &mut String, max_batch_bytes: usize) {
	const MARKER: &str = "\n[monitor delivery truncated to its configured pending-message limit]";
	let limit = max_batch_bytes.saturating_add(2048);
	if content.len() <= limit {
		return;
	}
	let mut end = limit.saturating_sub(MARKER.len()).min(content.len());
	while end > 0 && !content.is_char_boundary(end) {
		end -= 1;
	}
	content.truncate(end);
	content.push_str(MARKER);
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
		InboxSource::Monitor { id, description } => {
			return Some(format!("monitor {id} ({description})"));
		}
		InboxSource::BackgroundAgent { name } => {
			return Some(format!("background agent '{name}'"));
		}
		InboxSource::TapRun { id, role } => {
			return Some(format!("tap-run {id} ({role})"));
		}
		InboxSource::Skill { name } => {
			return Some(format!("skill '{name}'"));
		}
		InboxSource::SkillValidator { name } => {
			return Some(format!("skill validator '{name}' failed"));
		}
		InboxSource::Inject => "external inject",
		InboxSource::Webhook { hook } => {
			return Some(format!("webhook '{hook}'"));
		}
		InboxSource::GuardrailHook { script } => {
			return Some(format!("guardrail hook '{script}'"));
		}
		InboxSource::GuardValidator { name } => {
			return Some(format!("validator '{name}' failed"));
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

#[cfg(test)]
mod tests {
	use super::*;

	fn all_sources() -> Vec<InboxSource> {
		vec![
			InboxSource::Schedule { id: "s1".into() },
			InboxSource::Monitor {
				id: "m1".into(),
				description: "build".into(),
			},
			InboxSource::BackgroundAgent {
				name: "reviewer".into(),
			},
			InboxSource::TapRun {
				id: "t1".into(),
				role: "developer".into(),
			},
			InboxSource::Skill {
				name: "deploy".into(),
			},
			InboxSource::SkillValidator {
				name: "deploy".into(),
			},
			InboxSource::Inject,
			InboxSource::Webhook { hook: "ci".into() },
			InboxSource::GuardrailHook {
				script: "check.sh".into(),
			},
			InboxSource::GuardValidator {
				name: "tests".into(),
			},
		]
	}

	#[test]
	fn every_source_kind_is_distinct_snake_case() {
		let kinds: Vec<&str> = all_sources().iter().map(|s| s.display_kind()).collect();
		let unique: std::collections::HashSet<&&str> = kinds.iter().collect();
		assert_eq!(unique.len(), kinds.len(), "duplicate kind in {kinds:?}");
		for kind in kinds {
			assert!(
				kind.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
				"'{kind}' is not snake_case — structured clients key on it"
			);
		}
	}

	#[test]
	fn every_source_renders_a_label_and_an_icon() {
		for source in all_sources() {
			assert!(!source.display_label().is_empty());
			assert!(!source.display_icon().is_empty());
		}
	}

	#[test]
	fn only_user_initiated_sources_are_not_system_managed() {
		// A schedule is user-authored configuration, but each firing is runtime
		// control input: it must not replace the underlying task during compression.
		assert!(InboxSource::Schedule { id: "s".into() }.is_system_managed());
		assert!(!InboxSource::Inject.is_system_managed());
		assert!(!InboxSource::Webhook { hook: "h".into() }.is_system_managed());
		for source in all_sources() {
			let expected = !matches!(source, InboxSource::Inject | InboxSource::Webhook { .. });
			assert_eq!(
				source.is_system_managed(),
				expected,
				"{} classified wrong",
				source.display_kind()
			);
		}
	}

	#[test]
	fn coalesced_monitor_content_remains_bounded() {
		let mut content = "first".to_string();
		append_monitor_content(&mut content, &"x".repeat(10_000), 1024);
		assert!(content.len() <= 1024 + 2048);
		assert!(content.contains("additional monitor output omitted"));
		bound_monitor_content(&mut content, 1024);
		assert!(content.len() <= 1024 + 2048);
	}
}

#[cfg(test)]
#[path = "inbox_tests.rs"]
mod queue_tests;
