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

use super::storage::{parse_duration_secs, parse_when, ScheduleEntry, ScheduleStore, TriggerMode};
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
	let mut mutated = false;
	// NOTE: must NOT use `while let Some(entry) = store.lock().unwrap().pop_due()`.
	// The MutexGuard temporary in a `while let` scrutinee lives for the entire
	// loop body, so re-locking inside the body (to reschedule) deadlocks.
	loop {
		let entry = match store.lock().unwrap().pop_due() {
			Some(e) => e,
			None => break,
		};
		mutated = true;

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
	if mutated {
		persist_schedule_snapshot();
	}
}

/// True when the session has no in-flight work: no running tap-runs and no
/// running background agent jobs. The main response loop having returned to
/// the input-waiting state is implicit at the only call site.
pub fn is_session_idle() -> bool {
	if crate::session::tap_runs::has_running_jobs() {
		return false;
	}
	let active_jobs = crate::mcp::agent::functions::get_job_manager()
		.map(|m| m.active_count())
		.unwrap_or(0);
	active_jobs == 0
}

/// Flush all idle-mode entries into the session inbox iff the session is
/// idle (no running taps, no running background jobs). One-shot entries are
/// consumed; repeating entries (`interval_secs = Some(_)`) are re-added.
pub fn flush_idle_to_inbox() {
	if !is_session_idle() {
		return;
	}
	let store = get_store();
	if !store.lock().unwrap().has_idle() {
		return;
	}
	let mut mutated = false;
	loop {
		let entry = match store.lock().unwrap().pop_idle() {
			Some(e) => e,
			None => break,
		};
		mutated = true;
		if entry.interval_secs.is_some() {
			let next = entry.reschedule();
			store.lock().unwrap().add(next);
		}
		crate::session::inbox::push_inbox_message(crate::session::inbox::InboxMessage {
			source: crate::session::inbox::InboxSource::Schedule {
				id: entry.id.clone(),
			},
			content: entry.message,
		});
	}
	if mutated {
		persist_schedule_snapshot();
	}
}

/// Returns true if there are any pending idle-mode entries.
pub fn has_pending_idle_schedules() -> bool {
	get_store().lock().unwrap().has_idle()
}

// ---------------------------------------------------------------------------
// Persistence: write snapshot after every mutation, replay last on resume
// ---------------------------------------------------------------------------

/// Append the current schedule store to the session log (best-effort, no-op outside a session).
/// Lock is held only long enough to clone the entries; file I/O happens after release.
fn persist_schedule_snapshot() {
	let Some(session_id) = crate::session::context::current_session_id() else {
		return;
	};
	let entries_snapshot: Vec<ScheduleEntry> = {
		let store = get_store();
		let guard = store.lock().unwrap();
		guard.entries().to_vec()
	};
	if let Err(e) = crate::session::logger::log_schedule_snapshot(&session_id, &entries_snapshot) {
		crate::log_debug!("Failed to log schedule snapshot: {}", e);
	}
}

/// Restore the schedule store (if any) from the session log into session-scoped storage.
/// Called at session startup (all entry points) right after init_session_services.
/// Safe no-op when the log file doesn't exist or contains no snapshot.
pub fn restore_schedule_for_session(session_name: &str) {
	let log_file = match crate::session::logger::get_session_log_path(session_name) {
		Ok(p) => p,
		Err(e) => {
			crate::log_debug!(
				"restore_schedule_for_session: cannot resolve log file: {}",
				e
			);
			return;
		}
	};
	if !log_file.exists() {
		return;
	}

	let file = match std::fs::File::open(&log_file) {
		Ok(f) => f,
		Err(e) => {
			crate::log_debug!("restore_schedule_for_session: open failed: {}", e);
			return;
		}
	};

	use std::io::{BufRead, BufReader};
	let reader = BufReader::new(file);
	let mut latest_entries: Option<Vec<ScheduleEntry>> = None;

	for line in reader.lines().map_while(Result::ok) {
		let Ok(val) = serde_json::from_str::<serde_json::Value>(&line) else {
			continue;
		};
		let Some(t) = val.get("type").and_then(|t| t.as_str()) else {
			continue;
		};
		if t != "SCHEDULE_SNAPSHOT" {
			continue;
		}
		let Some(entries_val) = val.get("entries") else {
			continue;
		};
		match serde_json::from_value::<Vec<ScheduleEntry>>(entries_val.clone()) {
			Ok(entries) => latest_entries = Some(entries),
			Err(e) => crate::log_debug!("restore_schedule_for_session: deserialize failed: {}", e),
		}
	}

	let Some(entries) = latest_entries else {
		return;
	};
	let count = entries.len();
	let session_id = session_name.to_string();
	let storage = crate::session::context::get_schedule_storage(&session_id);
	storage.lock().unwrap().seed_entries(entries);
	crate::log_debug!(
		"Restored {} scheduled entries for session '{}'",
		count,
		session_name
	);
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
		description: r#"Schedule a message to be automatically injected as a user message into the current session at a future time or when the session becomes idle. The session keeps running until all scheduled messages have fired.

One-shot entries fire once and are removed. Repeating entries (set via 'every') re-schedule automatically after each firing until explicitly removed.

Commands:
- add: schedule a new message ('message' required; 'when' and 'every' both optional — defaults to when=\"idle\")
- list: show all pending scheduled entries with IDs, trigger times, and countdown
- remove: cancel a scheduled entry by 'id'
- edit: update an existing entry by 'id' (any of when, message, description, every)

'when' format (local timezone):
- 'idle' — fires the next time the session is idle (no running taps, no background jobs)
- 'now' — fires on the next scheduler tick (immediately)
- Relative: 'in 5m', 'in 2h', 'in 1h30m', 'in 90s', 'in 2h 30m'
- Time today: '15:30', '3:30pm', '9am' (if already past, fires tomorrow)
- Exact datetime: '2026-03-22 15:30'

'every' format (optional, omit for one-shot):
- 'idle' — fires every time the session becomes idle (pairs with when=\"idle\" or omitted)
- '10m', '1h', '30s', '1h30m' fires first at 'when', then every interval after
- To stop a repeating entry use remove, or clear interval with edit every='none'

If neither 'when' nor 'every' is provided, the entry defaults to when=\"idle\" (one-shot at next idle).

'description' is what this task is about (shown in list, helps track intent).
'message' is the EXACT text injected verbatim as a user message when the entry fires. Write it as if a human typed it: include all context the AI will need to act on it, because the AI will see only this message at trigger time with no other hint about why it arrived."#.to_string(),
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
					"description": "When to fire. 'idle' fires on the next session idle (no running taps/jobs). 'now' fires immediately. Relative: 'in 5m', 'in 1h30m'. Time today: '15:30', '3:30pm'. Exact: '2026-03-22 15:30'. Optional for add (defaults to 'idle' if both when and every are omitted)."
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
					"description": "Repeat interval. 'idle' fires on every session idle. Time formats: '10m', '1h', '30s', '1h30m'. Omit for one-shot. Use 'none' or 'off' in edit to clear an existing interval."
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

	// Raw `when` / `every` strings — both optional. Empty/omitted `when` defaults to "idle".
	let when_raw = match call.parameters.get("when") {
		Some(Value::String(s)) if !s.trim().is_empty() => Some(s.trim().to_string()),
		Some(_) => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"'when' must be a non-empty string".to_string(),
			))
		}
		None => None,
	};
	let every_raw = match call.parameters.get("every") {
		Some(Value::String(s)) if !s.trim().is_empty() => Some(s.trim().to_string()),
		_ => None,
	};

	let when_is_idle = when_raw.as_deref().map(str::to_lowercase).as_deref() == Some("idle");
	let every_is_idle = every_raw.as_deref().map(str::to_lowercase).as_deref() == Some("idle");

	// Idle mode: `when="idle"`, `every="idle"`, or neither specified (default).
	let idle_mode = when_is_idle || every_is_idle || (when_raw.is_none() && every_raw.is_none());

	if idle_mode {
		// Reject inconsistent mixes: a real time alongside "idle" makes no sense.
		if when_raw.is_some() && !when_is_idle {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"cannot combine a time-based 'when' with idle scheduling — use when=\"idle\" or omit when".to_string(),
			));
		}
		if every_raw.is_some() && !every_is_idle {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"cannot combine time-based 'every' with idle scheduling — use every=\"idle\" or omit every".to_string(),
			));
		}

		let repeating = every_is_idle;
		let entry = ScheduleEntry::new_idle(description.clone(), message, repeating);
		let id = entry.id.clone();
		get_store().lock().unwrap().add(entry);
		persist_schedule_snapshot();
		if let Some(sid) = crate::session::context::current_session_id() {
			crate::session::context::notify_schedule_change(&sid);
		}
		let desc_line = if description.is_empty() {
			String::new()
		} else {
			format!("\nDescription: {}", description)
		};
		let repeat_line = if repeating {
			"\nRepeats: every idle".to_string()
		} else {
			String::new()
		};
		return Ok(McpToolResult::success(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!(
				"✅ Scheduled [{}] for next idle{}{}\n\nThe message will be injected when the session is idle (no running taps or background jobs).",
				id, desc_line, repeat_line
			),
		));
	}

	// Time mode — `when` is required at this point.
	let when_str = match when_raw {
		Some(s) => s,
		None => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"missing required parameter 'when' for add".to_string(),
			))
		}
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

	let interval_secs = match every_raw.as_deref() {
		Some(s) => match parse_duration_secs(s) {
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
		None => None,
	};

	let entry = ScheduleEntry::new(description.clone(), message, trigger_at, interval_secs);
	let id = entry.id.clone();
	let countdown = entry.countdown();
	let trigger_fmt = entry.trigger_at.format("%Y-%m-%d %H:%M:%S").to_string();

	get_store().lock().unwrap().add(entry);
	persist_schedule_snapshot();

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
		let trigger_fmt = if entry.trigger_mode == TriggerMode::Idle {
			"idle".to_string()
		} else {
			entry.trigger_at.format("%Y-%m-%d %H:%M:%S").to_string()
		};
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
		let repeat_suffix = match (entry.trigger_mode, entry.interval_secs) {
			(TriggerMode::Idle, Some(_)) => "\n  🔁 Repeats every idle".to_string(),
			(TriggerMode::Time, Some(secs)) => {
				format!("\n  🔁 Repeats every {}", format_interval(secs))
			}
			_ => String::new(),
		};
		lines.push(format!(
			"[{}] {} ({}) — {}\n  Message: {}{}",
			entry.id,
			trigger_fmt,
			entry.countdown(),
			desc,
			preview,
			repeat_suffix,
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
		persist_schedule_snapshot();
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
		persist_schedule_snapshot();
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
