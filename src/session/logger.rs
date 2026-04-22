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

//! Session event logger — minimal persistence markers only.
//!
//! The session file is a JSONL stream where each line is either:
//! - A `Message` JSON (user/assistant/tool/system role) — the actual conversation
//! - A `SUMMARY` entry — session metadata
//! - A marker entry (`COMPRESSION_POINT`, `RESTORATION_POINT`, `KNOWLEDGE_ENTRY`, `COMMAND`)
//!
//! Messages are written directly by `messages.rs` / `message_handler.rs` via
//! `append_to_session_file`. This module only handles the marker entries that
//! the loader needs to reconstruct session state.

use crate::mcp::core::plan::storage::ExecutionPlan;
use crate::session::Message;
use anyhow::Result;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Get the session file path for a specific session.
pub fn get_session_log_file(session_name: &str) -> Result<PathBuf> {
	let sessions_dir = crate::directories::get_sessions_dir()?;
	Ok(sessions_dir.join(format!("{}.jsonl", session_name)))
}

/// Public alias for external callers.
pub fn get_session_log_path(session_name: &str) -> Result<PathBuf> {
	get_session_log_file(session_name)
}

/// Log restoration point for `/done` command — clears prior messages on reload.
pub fn log_restoration_point(
	session_name: &str,
	user_message: &str,
	assistant_response: &str,
) -> Result<()> {
	let log_file = get_session_log_file(session_name)?;
	let entry = serde_json::json!({
		"type": "RESTORATION_POINT",
		"timestamp": get_timestamp(),
		"user_message": user_message,
		"assistant_response": assistant_response,
	});
	append_to_log(&log_file, &serde_json::to_string(&entry)?)
}

/// Log compression point — clears prior messages on reload (messages were compressed).
///
/// CRITICAL: After writing the marker, this also snapshots the current in-memory
/// post-compression messages to the log. Without this, if no further activity occurs
/// before session exit, the parser on resume would wipe everything at the marker and
/// end up with zero messages (because compression mutates memory only — the compressed
/// summary + preserved tail aren't otherwise persisted until new messages arrive).
pub fn log_compression_point(
	session_name: &str,
	compression_type: &str,
	messages_removed: usize,
	tokens_saved: u64,
	post_compression_messages: &[Message],
) -> Result<()> {
	let log_file = get_session_log_file(session_name)?;
	let entry = serde_json::json!({
		"type": "COMPRESSION_POINT",
		"timestamp": get_timestamp(),
		"compression_type": compression_type,
		"messages_removed": messages_removed,
		"tokens_saved": tokens_saved,
	});
	append_to_log(&log_file, &serde_json::to_string(&entry)?)?;

	// Snapshot the post-compression message state so resume can reconstruct it.
	// The parser wipes `messages` at the marker and expects to re-read them below.
	for msg in post_compression_messages {
		let json = serde_json::to_string(msg)?;
		append_to_log(&log_file, &json)?;
	}

	Ok(())
}

/// Log a critical knowledge entry extracted during compression.
/// Re-injected as context on session resume.
pub fn log_knowledge_entry(session_name: &str, knowledge: &str) -> Result<()> {
	let log_file = get_session_log_file(session_name)?;
	let entry = serde_json::json!({
		"type": "KNOWLEDGE_ENTRY",
		"timestamp": get_timestamp(),
		"content": knowledge,
	});
	append_to_log(&log_file, &serde_json::to_string(&entry)?)
}

/// Log runtime-only commands (`/model`, `/role`, `/layers`, `/cache`) so they
/// can be replayed on resume to reconstruct runtime state.
pub fn log_session_command(session_name: &str, command_line: &str) -> Result<()> {
	let log_file = get_session_log_file(session_name)?;
	let entry = serde_json::json!({
		"type": "COMMAND",
		"timestamp": get_timestamp(),
		"command": command_line,
	});
	append_to_log(&log_file, &serde_json::to_string(&entry)?)
}

/// Log a snapshot of the active plan so it can be restored on session resume.
/// The full ExecutionPlan is serialized; the most recent snapshot wins on replay.
pub fn log_plan_snapshot(session_name: &str, plan: &ExecutionPlan) -> Result<()> {
	let log_file = get_session_log_file(session_name)?;
	let entry = serde_json::json!({
		"type": "PLAN_SNAPSHOT",
		"timestamp": get_timestamp(),
		"plan": plan,
	});
	append_to_log(&log_file, &serde_json::to_string(&entry)?)
}

/// Log that the active plan has been cleared (done/reset).
/// Invalidates any prior PLAN_SNAPSHOT on resume.
pub fn log_plan_cleared(session_name: &str) -> Result<()> {
	let log_file = get_session_log_file(session_name)?;
	let entry = serde_json::json!({
		"type": "PLAN_CLEARED",
		"timestamp": get_timestamp(),
	});
	append_to_log(&log_file, &serde_json::to_string(&entry)?)
}

fn get_timestamp() -> u64 {
	SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.unwrap_or_default()
		.as_secs()
}

/// Append a single-line entry to the session log file.
fn append_to_log(log_file: &PathBuf, content: &str) -> Result<()> {
	let mut file = OpenOptions::new()
		.create(true)
		.append(true)
		.open(log_file)?;

	// Ensure content is on a single line — replace any newlines with spaces
	let single_line = content.replace(['\n', '\r'], " ");
	writeln!(file, "{}", single_line)?;
	Ok(())
}
