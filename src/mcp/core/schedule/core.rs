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

//! Core schedule tool: global store, MCP handler, and session-loop helpers.

use super::storage::{parse_duration_secs, parse_when, ScheduleEntry, ScheduleStore};
use crate::mcp::{McpFunction, McpToolCall, McpToolResult};
use anyhow::Result;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex, OnceLock};

// ---------------------------------------------------------------------------
// Global store
// ---------------------------------------------------------------------------

static SCHEDULE_STORE: OnceLock<Arc<Mutex<ScheduleStore>>> = OnceLock::new();

/// Get schedule storage for the current context.
/// Returns session-scoped storage if in a session, otherwise CLI global.
fn get_store() -> Arc<Mutex<ScheduleStore>> {
	if let Some(session_id) = crate::session::context::current_session_id() {
		crate::session::context::get_schedule_storage(&session_id)
	} else {
		SCHEDULE_STORE
			.get_or_init(|| Arc::new(Mutex::new(ScheduleStore::new())))
			.clone()
	}
}

// ---------------------------------------------------------------------------
// Session-loop helpers (called from main_loop.rs)
// ---------------------------------------------------------------------------

/// Flush all due entries into the session inbox.  Call once per loop iteration
/// so the inbox is the single source of truth for all injected messages.
pub fn flush_due_to_inbox() {
	let store = get_store();
	while let Some(entry) = store.lock().unwrap().pop_due() {
		// If this is a repeating entry, re-add it before pushing to inbox.
		if entry.interval_secs.is_some() {
			let next = entry.reschedule();
			store.lock().unwrap().add(next);
			// Wake the monitor so it recalculates the next sleep duration.
			if let Some(sid) = crate::session::context::current_session_id() {
				crate::session::context::notify_schedule_change(&sid);
			}
		}
		crate::session::inbox::push_inbox_message(crate::session::inbox::InboxMessage {
			source: crate::session::inbox::InboxSource::Schedule {
				id: entry.id.clone(),
			},
			content: entry.message,
		});
	}
}

/// Returns true if there are any pending scheduled entries.
pub fn has_pending_schedules() -> bool {
	!get_store().lock().unwrap().is_empty()
}

/// Returns a future that resolves when the next scheduled entry is due.
/// Returns `futures::future::pending()` (never resolves) when the store is empty —
/// so the select! arm is a no-op when nothing is scheduled.
///
/// Also wakes up when schedules change (new/edited/removed) via schedule notify.
pub async fn next_schedule_sleep() {
	let session_id = crate::session::context::current_session_id();
	let duration = get_store().lock().unwrap().next_due_duration();

	match (duration, session_id) {
		(Some(d), Some(ref sid)) => {
			// Wait for timer OR schedule change notification
			let notify = crate::session::context::get_schedule_notify(sid);
			tokio::select! {
				_ = tokio::time::sleep(d) => {}
				_ = notify.notified() => {}
			}
		}
		(Some(d), None) => {
			tokio::time::sleep(d).await;
		}
		(None, Some(ref sid)) => {
			// No schedules - wait for notification that one was added
			let notify = crate::session::context::get_schedule_notify(sid);
			notify.notified().await;
		}
		(None, None) => {
			futures::future::pending::<()>().await;
		}
	}
}

// ---------------------------------------------------------------------------
// MCP tool definition
// ---------------------------------------------------------------------------

pub fn get_schedule_function() -> McpFunction {
	McpFunction {
		name: "schedule".to_string(),
		description: r#"Schedule a message to be automatically injected as a user message into the current session at a future time. The session keeps running until all scheduled messages have fired.

One-shot entries fire once and are removed. Repeating entries (set via 'every') re-schedule automatically after each firing until explicitly removed.

Commands:
- add: schedule a new message (requires 'when' and 'message'; 'description' recommended)
- list: show all pending scheduled entries with IDs, trigger times, and countdown
- remove: cancel a scheduled entry by 'id'
- edit: update an existing entry by 'id' (any of when, message, description, every)

'when' format (local timezone):
- Relative: 'in 5m', 'in 2h', 'in 1h30m', 'in 90s', 'in 2h 30m'
- Time today: '15:30', '3:30pm', '9am' (if already past, fires tomorrow)
- Exact datetime: '2026-03-22 15:30'

'every' format (optional, omit for one-shot):
- '10m', '1h', '30s', '1h30m' fires first at 'when', then every interval after
- To stop a repeating entry use remove, or clear interval with edit every='none'

'description' is what this task is about (shown in list, helps track intent).
'message' is the EXACT text injected verbatim as a user message when the timer fires. Write it as if a human typed it: include all context the AI will need to act on it, because the AI will see only this message at trigger time with no other hint about why it arrived."#.to_string(),
		parameters: json!({
			"type": "object",
			"properties": {
				"command": {
					"type": "string",
					"enum": ["add", "list", "remove", "edit"],
					"description": "Action to perform."
				},
				"when": {
					"type": "string",
					"description": "When to fire. Relative: 'in 5m', 'in 2h', 'in 1h30m', 'in 90s'. Time today: '15:30', '3:30pm'. Exact: '2026-03-22 15:30'. Required for add; optional for edit."
				},
				"message": {
					"type": "string",
					"description": "The exact text injected verbatim as a user message when the timer fires. Write it with full context — the AI will see only this text at trigger time. Required for add; optional for edit."
				},
				"description": {
					"type": "string",
					"description": "Human-readable description of what this scheduled task is about. Shown in list output. Recommended for add; optional for edit."
				},
				"id": {
					"type": "string",
					"description": "Entry ID (from list output). Required for remove and edit."
				},
				"every": {
					"type": "string",
					"description": "Repeat interval — entry re-schedules automatically after each firing. Format: '10m', '1h', '30s', '1h30m'. Omit for one-shot. Use 'none' or 'off' in edit to clear an existing interval."
				}
			},
			"required": ["command"]
		}),
	}
}

// ---------------------------------------------------------------------------
// MCP tool handler
// ---------------------------------------------------------------------------

pub async fn execute_schedule_tool(call: &McpToolCall) -> Result<McpToolResult> {
	let command = match call.parameters.get("command") {
		Some(Value::String(s)) if !s.trim().is_empty() => s.clone(),
		Some(_) => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"'command' must be a non-empty string".to_string(),
			))
		}
		None => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"missing required parameter 'command'".to_string(),
			))
		}
	};

	match command.as_str() {
		"add" => handle_add(call),
		"list" => handle_list(call),
		"remove" => handle_remove(call),
		"edit" => handle_edit(call),
		other => Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("unknown command '{}' — use: add, list, remove, edit", other),
		)),
	}
}

// ---------------------------------------------------------------------------
// Command handlers
// ---------------------------------------------------------------------------

/// Format interval_secs as a human-readable string, e.g. "1h 30m", "10m", "45s".
fn format_interval(secs: i64) -> String {
	let hours = secs / 3600;
	let mins = (secs % 3600) / 60;
	let s = secs % 60;
	match (hours, mins, s) {
		(h, m, 0) if h > 0 && m > 0 => format!("{}h {}m", h, m),
		(h, 0, 0) if h > 0 => format!("{}h", h),
		(0, m, s) if m > 0 && s > 0 => format!("{}m {}s", m, s),
		(0, m, 0) if m > 0 => format!("{}m", m),
		_ => format!("{}s", secs),
	}
}

fn handle_add(call: &McpToolCall) -> Result<McpToolResult> {
	let when_str = match call.parameters.get("when") {
		Some(Value::String(s)) if !s.trim().is_empty() => s.clone(),
		Some(_) => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"'when' must be a non-empty string".to_string(),
			))
		}
		None => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"missing required parameter 'when' for add".to_string(),
			))
		}
	};

	let message = match call.parameters.get("message") {
		Some(Value::String(s)) if !s.trim().is_empty() => s.clone(),
		Some(_) => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"'message' must be a non-empty string".to_string(),
			))
		}
		None => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"missing required parameter 'message' for add".to_string(),
			))
		}
	};

	let description = match call.parameters.get("description") {
		Some(Value::String(s)) if !s.trim().is_empty() => s.clone(),
		_ => String::new(),
	};

	let trigger_at = match parse_when(&when_str) {
		Ok(t) => t,
		Err(e) => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				format!("invalid 'when' value: {}", e),
			))
		}
	};

	let interval_secs = match call.parameters.get("every") {
		Some(Value::String(s)) if !s.trim().is_empty() => match parse_duration_secs(s.trim()) {
			Ok(secs) if secs > 0 => Some(secs),
			Ok(_) => {
				return Ok(McpToolResult::error(
					call.tool_name.clone(),
					call.tool_id.clone(),
					"'every' duration must be greater than zero".to_string(),
				))
			}
			Err(e) => {
				return Ok(McpToolResult::error(
					call.tool_name.clone(),
					call.tool_id.clone(),
					format!("invalid 'every' value: {}", e),
				))
			}
		},
		_ => None,
	};

	let entry = ScheduleEntry::new(description.clone(), message, trigger_at, interval_secs);
	let id = entry.id.clone();
	let countdown = entry.countdown();
	let trigger_fmt = entry.trigger_at.format("%Y-%m-%d %H:%M:%S").to_string();

	get_store().lock().unwrap().add(entry);

	// Wake up the schedule monitor so it recalculates the next due time
	if let Some(sid) = crate::session::context::current_session_id() {
		crate::session::context::notify_schedule_change(&sid);
	}
	let desc_line = if description.is_empty() {
		String::new()
	} else {
		format!("\nDescription: {}", description)
	};
	let repeat_line = match interval_secs {
		Some(secs) => format!("\nRepeats: every {}", format_interval(secs)),
		None => String::new(),
	};

	Ok(McpToolResult::success(
		call.tool_name.clone(),
		call.tool_id.clone(),
		format!(
			"✅ Scheduled [{}] at {} ({}){}{}\n\nThe message will be injected as a user message when the timer fires.",
			id, trigger_fmt, countdown, desc_line, repeat_line
		),
	))
}

fn handle_list(call: &McpToolCall) -> Result<McpToolResult> {
	let store = get_store();
	let guard = store.lock().unwrap();
	let entries = guard.entries();

	if entries.is_empty() {
		return Ok(McpToolResult::success(
			call.tool_name.clone(),
			call.tool_id.clone(),
			"No scheduled entries.".to_string(),
		));
	}

	let mut lines = vec![format!("{} scheduled entries:\n", entries.len())];
	for entry in entries {
		let trigger_fmt = entry.trigger_at.format("%Y-%m-%d %H:%M:%S").to_string();
		let desc = if entry.description.is_empty() {
			"(no description)".to_string()
		} else {
			entry.description.clone()
		};
		// Truncate message preview to 80 chars.
		let preview = if entry.message.len() > 80 {
			format!("{}…", {
				let mut end = 80;
				while !entry.message.is_char_boundary(end) {
					end -= 1;
				}
				&entry.message[..end]
			})
		} else {
			entry.message.clone()
		};
		lines.push(format!(
			"[{}] {} ({}) — {}\n  Message: {}{}",
			entry.id,
			trigger_fmt,
			entry.countdown(),
			desc,
			preview,
			match entry.interval_secs {
				Some(secs) => format!("\n  🔁 Repeats every {}", format_interval(secs)),
				None => String::new(),
			},
		));
	}

	Ok(McpToolResult::success(
		call.tool_name.clone(),
		call.tool_id.clone(),
		lines.join("\n"),
	))
}

fn handle_remove(call: &McpToolCall) -> Result<McpToolResult> {
	let id = match call.parameters.get("id") {
		Some(Value::String(s)) if !s.trim().is_empty() => s.clone(),
		Some(_) => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"'id' must be a non-empty string".to_string(),
			))
		}
		None => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"missing required parameter 'id' for remove".to_string(),
			))
		}
	};

	let removed = get_store().lock().unwrap().remove(&id);
	if removed {
		// Wake up the schedule monitor so it recalculates the next due time
		if let Some(sid) = crate::session::context::current_session_id() {
			crate::session::context::notify_schedule_change(&sid);
		}
		Ok(McpToolResult::success(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("✅ Removed scheduled entry [{}].", id),
		))
	} else {
		Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("No scheduled entry found with id '{}'.", id),
		))
	}
}

fn handle_edit(call: &McpToolCall) -> Result<McpToolResult> {
	let id = match call.parameters.get("id") {
		Some(Value::String(s)) if !s.trim().is_empty() => s.clone(),
		Some(_) => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"'id' must be a non-empty string".to_string(),
			))
		}
		None => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"missing required parameter 'id' for edit".to_string(),
			))
		}
	};

	let new_when = match call.parameters.get("when") {
		Some(Value::String(s)) if !s.trim().is_empty() => match parse_when(s) {
			Ok(t) => Some(t),
			Err(e) => {
				return Ok(McpToolResult::error(
					call.tool_name.clone(),
					call.tool_id.clone(),
					format!("invalid 'when' value: {}", e),
				))
			}
		},
		_ => None,
	};

	let new_message = match call.parameters.get("message") {
		Some(Value::String(s)) if !s.trim().is_empty() => Some(s.clone()),
		_ => None,
	};

	let new_description = match call.parameters.get("description") {
		Some(Value::String(s)) if !s.trim().is_empty() => Some(s.clone()),
		_ => None,
	};

	// Some(Some(secs)) = set interval, Some(None) = clear interval, None = no change.
	// Pass `every = ""` or omit to leave unchanged; pass `every = "0"` is rejected.
	let new_interval: Option<Option<i64>> = match call.parameters.get("every") {
		Some(Value::String(s)) if s.trim() == "none" || s.trim() == "off" => Some(None),
		Some(Value::String(s)) if !s.trim().is_empty() => match parse_duration_secs(s.trim()) {
			Ok(secs) if secs > 0 => Some(Some(secs)),
			Ok(_) => {
				return Ok(McpToolResult::error(
					call.tool_name.clone(),
					call.tool_id.clone(),
					"'every' duration must be greater than zero (use 'none' to clear)".to_string(),
				))
			}
			Err(e) => {
				return Ok(McpToolResult::error(
					call.tool_name.clone(),
					call.tool_id.clone(),
					format!("invalid 'every' value: {}", e),
				))
			}
		},
		_ => None,
	};

	if new_when.is_none()
		&& new_message.is_none()
		&& new_description.is_none()
		&& new_interval.is_none()
	{
		return Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			"edit requires at least one of: when, message, description, every".to_string(),
		));
	}
	let store = get_store();
	let updated =
		store
			.lock()
			.unwrap()
			.edit(&id, new_description, new_message, new_when, new_interval);

	if updated {
		// Wake up the schedule monitor so it recalculates the next due time
		if let Some(sid) = crate::session::context::current_session_id() {
			crate::session::context::notify_schedule_change(&sid);
		}

		// Read back the updated entry for confirmation.
		let store = get_store();
		let guard = store.lock().unwrap();
		let entry = guard.entries().iter().find(|e| e.id == id);
		let summary = entry
			.map(|e| {
				let repeat = match e.interval_secs {
					Some(secs) => format!(", repeats every {}", format_interval(secs)),
					None => String::new(),
				};
				format!(
					" → fires at {} ({}){}",
					e.trigger_at.format("%Y-%m-%d %H:%M:%S"),
					e.countdown(),
					repeat,
				)
			})
			.unwrap_or_default();

		Ok(McpToolResult::success(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("✅ Updated scheduled entry [{}]{}.", id, summary),
		))
	} else {
		Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("No scheduled entry found with id '{}'.", id),
		))
	}
}
