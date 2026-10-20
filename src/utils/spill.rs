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

//! Spill-to-disk reference handles for over-cap tool output.
//!
//! Plain truncation is LOSSY: the tail is dropped and re-running the same call
//! returns the same capped output — the rest is simply unreachable, which is
//! what drives truncation-retry loops. Spilling instead writes the FULL body to
//! a session-scoped file and hands the model a path handle, turning truncation
//! into lossless just-in-time access: the model reads only the span it needs via
//! its normal file/grep tools. It hooks the single truncation choke point, so it
//! is tool-agnostic. (Cf. Anthropic "code execution with MCP" — lightweight
//! identifiers beat inlining large results into context.)

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

use crate::session::context::current_session_id;

/// Root spill directory for the current session, or `None` when there is no
/// session context (CLI/test paths never spill — they fall back to lossy
/// truncation, keeping those paths IO-free).
///
/// Durable, next to the session's own file — NOT the OS temp dir. The path is
/// handed to the model inside a truncation notice, and that notice lives in the
/// message history for as long as the session does: a resumed session, or one
/// picked up after a reboot, must still be able to read what the notice points
/// at. A temp-dir spill died with the run that wrote it and left every older
/// notice pointing at nothing.
fn session_spill_dir() -> Option<PathBuf> {
	let session_id = current_session_id()?;
	Some(
		crate::directories::get_sessions_dir()
			.ok()?
			.join("spill")
			.join(session_id),
	)
}

/// Write the full `content` to a session-scoped spill file and return its path,
/// or `None` when there is no session (CLI/tests) or the write fails — callers
/// then fall back to plain lossy truncation. The filename is keyed on
/// `(tool_name, content)` so re-running an identical call overwrites the same
/// file instead of accumulating duplicates (mirrors the truncation idempotency
/// guarantee: same args → same output → same handle).
pub fn write_spill(tool_name: &str, content: &str) -> Option<PathBuf> {
	let path = spill_path(tool_name, content)?;
	if std::fs::create_dir_all(path.parent()?).is_err() {
		return None;
	}
	match std::fs::write(&path, content) {
		Ok(()) => Some(path),
		Err(e) => {
			crate::log_debug!("spill write failed ({}), falling back to lossy truncation", e);
			None
		}
	}
}

/// The handle `write_spill` uses for `(tool_name, content)` in the current
/// session, whether or not it has been written yet.
fn spill_path(tool_name: &str, content: &str) -> Option<PathBuf> {
	let dir = session_spill_dir()?;
	let mut h = DefaultHasher::new();
	tool_name.hash(&mut h);
	content.hash(&mut h);
	let safe_tool: String = tool_name
		.chars()
		.map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
		.collect();
	Some(dir.join(format!("{safe_tool}-{:016x}.txt", h.finish())))
}

/// Whether this exact `(tool_name, content)` was already spilled in the
/// current session — the same output was cut once before.
pub fn spill_exists(tool_name: &str, content: &str) -> bool {
	spill_path(tool_name, content).is_some_and(|path| path.exists())
}

/// Whether tool arguments point into the current session's spill directory,
/// i.e. the call reads back output a notice told the model to recover. The
/// directory is compared in its JSON-escaped form so Windows separators match
/// the serialized arguments.
pub fn references_spill(arguments: &serde_json::Value) -> bool {
	let Some(dir) = session_spill_dir() else {
		return false;
	};
	let escaped = serde_json::Value::from(dir.to_string_lossy()).to_string();
	arguments.to_string().contains(escaped.trim_matches('"'))
}

/// Remove the current session's spill directory. Called when a session starts
/// FRESH (empty message list), which is the only moment a spill is certainly
/// unreachable: no notice in any surviving history can still refer to it.
pub fn clear_current_session() {
	if let Some(dir) = session_spill_dir() {
		let _ = std::fs::remove_dir_all(dir);
	}
}

/// Read every file in `dir` as text, up to `max_total` chars total (files are
/// appended in directory order until the cap is hit; a partially-read last file
/// is truncated to fit). Non-text or unreadable entries are skipped. Shared by
/// the spill and archive readers that re-ground supervisor evidence checks.
pub fn read_text_files(dir: &std::path::Path, max_total: usize) -> Vec<String> {
	let Ok(entries) = std::fs::read_dir(dir) else {
		return Vec::new();
	};
	let mut out = Vec::new();
	let mut remaining = max_total;
	for entry in entries.flatten() {
		if remaining == 0 {
			break;
		}
		let Ok(mut content) = std::fs::read_to_string(entry.path()) else {
			continue;
		};
		if content.len() > remaining {
			// Floor to a char boundary so the slice never splits a UTF-8 sequence.
			let mut end = remaining;
			while !content.is_char_boundary(end) {
				end -= 1;
			}
			content.truncate(end);
		}
		remaining -= content.len();
		out.push(content);
	}
	out
}

#[cfg(test)]
#[path = "spill_tests.rs"]
mod tests;
