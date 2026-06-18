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
/// truncation, keeping those paths IO-free). Lives under the OS temp dir:
/// spills are transient by nature (a within-session read-through), so we let
/// the OS reclaim them and best-effort clear on fresh-session start.
fn session_spill_dir() -> Option<PathBuf> {
	let session_id = current_session_id()?;
	Some(std::env::temp_dir().join("octomind-spill").join(session_id))
}

/// Write the full `content` to a session-scoped spill file and return its path,
/// or `None` when there is no session (CLI/tests) or the write fails — callers
/// then fall back to plain lossy truncation. The filename is keyed on
/// `(tool_name, content)` so re-running an identical call overwrites the same
/// file instead of accumulating duplicates (mirrors the truncation idempotency
/// guarantee: same args → same output → same handle).
pub fn write_spill(tool_name: &str, content: &str) -> Option<PathBuf> {
	let dir = session_spill_dir()?;
	if std::fs::create_dir_all(&dir).is_err() {
		return None;
	}
	let mut h = DefaultHasher::new();
	tool_name.hash(&mut h);
	content.hash(&mut h);
	let safe_tool: String = tool_name
		.chars()
		.map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
		.collect();
	let path = dir.join(format!("{safe_tool}-{:016x}.txt", h.finish()));
	match std::fs::write(&path, content) {
		Ok(()) => Some(path),
		Err(e) => {
			crate::log_debug!(
				"spill write failed ({}), falling back to lossy truncation",
				e
			);
			None
		}
	}
}

/// Remove the current session's spill directory. Called from the same
/// session-end/reset point as the dedup cache clear.
pub fn clear_current_session() {
	if let Some(dir) = session_spill_dir() {
		let _ = std::fs::remove_dir_all(dir);
	}
}
