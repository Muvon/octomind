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

// Lossless archive for compacted messages (addressable recall).
//
// Both compression paths (conversation-level `apply_compression` and
// task-level `compress_completed_task`) drain messages from the session and
// replace them with a lossy summary. Before this module, the raw transcript
// was gone for good — any detail the summarizer dropped was unrecoverable
// (rate–distortion principle P1: never discard irreversibly what you cannot
// re-derive cheaply).
//
// Now every drained range is first written verbatim to
// `<sessions_dir>/archive/<session_name>/<compression_id>.jsonl` (one JSON
// message per line) and the summary embeds an `<archive path="…">` pointer.
// The model can `view` / `structural_search` / `knowledge(match)` that file
// on demand to recall exact code, errors, or tool output the summary elided.
// Compaction becomes reversible: lossy in-context, lossless on disk.

use crate::session::Message;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Archive the messages about to be drained by a compression cycle.
///
/// Best-effort at the call site: returns `Some(path)` on success, `None`
/// (with a loud error log) on failure. Archiving must never abort the
/// compression itself — at the hard context ceiling, failing to compress
/// because of a disk hiccup would stall the session entirely.
pub(crate) fn archive_messages(
	session_name: &str,
	compression_id: &str,
	messages: &[Message],
) -> Option<PathBuf> {
	let dir = match crate::directories::get_sessions_dir() {
		Ok(d) => d.join("archive").join(session_name),
		Err(e) => {
			crate::log_error!(
				"Compression archive: cannot resolve sessions dir: {} — drained messages will NOT be recoverable",
				e
			);
			return None;
		}
	};
	match write_archive_to(&dir, compression_id, messages) {
		Ok(path) => {
			crate::log_debug!(
				"Compression archive: {} messages written to {}",
				messages.len(),
				path.display()
			);
			Some(path)
		}
		Err(e) => {
			crate::log_error!(
				"Compression archive: failed to write archive: {} — drained messages will NOT be recoverable",
				e
			);
			None
		}
	}
}

/// Write one JSON line per message to `<dir>/<compression_id>.jsonl`.
/// Split from `archive_messages` so tests can point at a temp dir.
fn write_archive_to(dir: &Path, compression_id: &str, messages: &[Message]) -> Result<PathBuf> {
	std::fs::create_dir_all(dir)
		.with_context(|| format!("failed to create archive dir: {}", dir.display()))?;
	let path = dir.join(format!("{compression_id}.jsonl"));
	let mut out = String::new();
	for msg in messages {
		let line = serde_json::to_string(msg).context("failed to serialize archived message")?;
		out.push_str(&line);
		out.push('\n');
	}
	std::fs::write(&path, out)
		.with_context(|| format!("failed to write archive file: {}", path.display()))?;
	Ok(path)
}

/// Read every archive file of `session_name`, up to `max_total` chars total.
/// Used to re-ground supervisor evidence checks after a mid-turn compression
/// drained the tool outputs those checks match against — the archive is the
/// verbatim record of everything drained. Empty when no archives exist.
pub(crate) fn read_session_archives(session_name: &str, max_total: usize) -> Vec<String> {
	let dir = match crate::directories::get_sessions_dir() {
		Ok(d) => d.join("archive").join(session_name),
		Err(_) => return Vec::new(),
	};
	crate::utils::spill::read_text_files(&dir, max_total)
}

/// Render the `<archive>` pointer embedded into a compressed summary.
///
/// The pointer tells the model the full raw transcript of the replaced
/// messages exists on disk and how to recall it. Kept terse — it is re-fed
/// into every subsequent compression cycle as part of the summary.
pub(crate) fn archive_pointer(path: &Path) -> String {
	format!(
		"<archive path=\"{}\">\nFull raw transcript of the messages this summary replaced (JSONL, one message per line). If you need exact code, error text, or tool output the summary elided, read or search this file — do not guess.\n</archive>",
		path.display()
	)
}

#[cfg(test)]
mod archive_tests {
	use super::*;

	fn msg(role: &str, content: &str) -> Message {
		Message {
			role: role.to_string(),
			content: content.to_string(),
			..Default::default()
		}
	}

	#[test]
	fn archive_roundtrip_preserves_every_message_verbatim() {
		let dir =
			std::env::temp_dir().join(format!("octomind-archive-test-{}", std::process::id()));
		let messages = vec![
			msg("user", "fix the parser"),
			msg("assistant", "looking at src/parser.rs"),
			msg("tool", "error: unexpected token at line 42"),
		];

		let path = write_archive_to(&dir, "test-id-1", &messages).expect("write succeeds");
		let content = std::fs::read_to_string(&path).expect("archive readable");
		let lines: Vec<&str> = content.lines().collect();
		assert_eq!(lines.len(), 3);

		// Every line deserializes back to the exact original message.
		for (line, original) in lines.iter().zip(messages.iter()) {
			let restored: Message = serde_json::from_str(line).expect("valid JSON line");
			assert_eq!(restored.role, original.role);
			assert_eq!(restored.content, original.content);
		}

		let _ = std::fs::remove_dir_all(&dir);
	}

	#[test]
	fn archive_pointer_names_the_path_and_recall_guidance() {
		let pointer = archive_pointer(Path::new("/tmp/sessions/archive/s1/abc.jsonl"));
		assert!(pointer.contains("path=\"/tmp/sessions/archive/s1/abc.jsonl\""));
		assert!(pointer.contains("do not guess"));
	}
}
