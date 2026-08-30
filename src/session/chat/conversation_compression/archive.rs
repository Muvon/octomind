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
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ArchiveBlockEntry {
	pub id: String,
	pub kind: super::attention::PacketKind,
	pub provenance: super::attention::Provenance,
	pub dependencies: Vec<String>,
	#[serde(default)]
	pub linkage: super::attention::PacketLinkage,
	#[serde(default)]
	pub exact_spans: Vec<super::attention::SourceSpan>,
	pub content_digest: String,
	pub archive_line_start: usize,
	pub archive_line_end: usize,
	pub descriptor: String,
}

#[derive(Debug, Clone)]
pub(crate) struct ArchiveBundle {
	pub path: PathBuf,
	pub index_path: PathBuf,
	pub entries: Vec<ArchiveBlockEntry>,
}

#[derive(Debug, Clone)]
pub(crate) struct ArchivedBlockRef {
	pub provenance: super::attention::Provenance,
	pub archive_path: PathBuf,
	pub index_path: PathBuf,
	pub archive_line_start: usize,
	pub archive_line_end: usize,
	pub descriptor: String,
}

impl ArchiveBundle {
	pub fn entry(&self, id: &str) -> Option<&ArchiveBlockEntry> {
		self.entries.iter().find(|entry| entry.id == id)
	}
}

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

/// Archive a PACT drain transaction and its stable block sidecar before any
/// live message is removed. Unlike the legacy best-effort helper, failure is
/// returned to the caller so optional compaction can abort without data loss.
pub(crate) fn archive_messages_with_index(
	session_name: &str,
	compression_id: &str,
	messages: &[Message],
	packets: &[super::attention::EvidencePacket],
) -> Result<ArchiveBundle> {
	let dir = crate::directories::get_sessions_dir()?
		.join("archive")
		.join(session_name);
	write_archive_with_index_to(&dir, compression_id, messages, packets)
}

pub(super) fn write_archive_with_index_to(
	dir: &Path,
	compression_id: &str,
	messages: &[Message],
	packets: &[super::attention::EvidencePacket],
) -> Result<ArchiveBundle> {
	let path = write_archive_to(dir, compression_id, messages)?;
	let index_path = dir.join(format!("{compression_id}.blocks.jsonl"));
	let mut entries = Vec::with_capacity(packets.len());
	for packet in packets {
		let block = messages
			.get(packet.message_start..=packet.message_end)
			.ok_or_else(|| {
				anyhow::anyhow!("PACT packet {} points outside the archive range", packet.id)
			})?;
		entries.push(ArchiveBlockEntry {
			id: packet.id.clone(),
			kind: packet.kind,
			provenance: packet.provenance,
			dependencies: packet.depends_on.clone(),
			linkage: packet.linkage,
			exact_spans: packet.exact_spans.clone(),
			content_digest: block_digest(&packet.id, block),
			archive_line_start: packet.message_start + 1,
			archive_line_end: packet.message_end + 1,
			descriptor: packet.descriptor.clone(),
		});
	}
	let mut out = String::new();
	for entry in &entries {
		out.push_str(&serde_json::to_string(entry).context("failed to serialize PACT block")?);
		out.push('\n');
	}
	std::fs::write(&index_path, out).with_context(|| {
		format!(
			"failed to write archive block index: {}",
			index_path.display()
		)
	})?;
	Ok(ArchiveBundle {
		path,
		index_path,
		entries,
	})
}

fn block_digest(id: &str, messages: &[Message]) -> String {
	let mut hasher = Sha256::new();
	hasher.update(b"octomind-pact-archive-v1\0");
	hasher.update(id.as_bytes());
	for message in messages {
		hasher.update([0]);
		hasher.update(serde_json::to_vec(message).expect("session messages are serializable"));
	}
	hasher
		.finalize()
		.iter()
		.map(|byte| format!("{byte:02x}"))
		.collect()
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

/// Registry for IDs emitted by prior PACT compactions. This lets a
/// recompression preserve transitive raw-source references and their exact
/// archive coordinates rather than forcing every unit to cite only the
/// immediately preceding summary packet.
pub(crate) fn read_session_block_registry(
	session_name: &str,
) -> BTreeMap<String, ArchivedBlockRef> {
	let dir = match crate::directories::get_sessions_dir() {
		Ok(d) => d.join("archive").join(session_name),
		Err(_) => return BTreeMap::new(),
	};
	let Ok(entries) = std::fs::read_dir(dir) else {
		return BTreeMap::new();
	};
	let mut paths: Vec<PathBuf> = entries
		.flatten()
		.map(|entry| entry.path())
		.filter(|path| path.to_string_lossy().ends_with(".blocks.jsonl"))
		.collect();
	paths.sort();
	let mut registry = BTreeMap::new();
	for path in paths {
		let Ok(content) = std::fs::read_to_string(&path) else {
			continue;
		};
		let Some(archive_path) = path
			.file_name()
			.and_then(|name| name.to_str())
			.and_then(|name| name.strip_suffix(".blocks.jsonl"))
			.map(|stem| path.with_file_name(format!("{stem}.jsonl")))
		else {
			continue;
		};
		for entry in content
			.lines()
			.filter_map(|line| serde_json::from_str::<ArchiveBlockEntry>(line).ok())
		{
			registry.insert(
				entry.id,
				ArchivedBlockRef {
					provenance: entry.provenance,
					archive_path: archive_path.clone(),
					index_path: path.clone(),
					archive_line_start: entry.archive_line_start,
					archive_line_end: entry.archive_line_end,
					descriptor: entry.descriptor,
				},
			);
		}
	}
	registry
}

/// Exact stable-ID dereference used by validation and future bounded recall.
/// Returned messages preserve archive order and are deduplicated when several
/// requested packets share a dependency range.
pub(crate) fn read_blocks(index_path: &Path, ids: &[String]) -> Result<Vec<Message>> {
	let content = std::fs::read_to_string(index_path)
		.with_context(|| format!("failed to read block index: {}", index_path.display()))?;
	let wanted: HashSet<&str> = ids.iter().map(String::as_str).collect();
	let entries: Vec<ArchiveBlockEntry> = content
		.lines()
		.filter_map(|line| serde_json::from_str::<ArchiveBlockEntry>(line).ok())
		.filter(|entry| wanted.contains(entry.id.as_str()))
		.collect();
	let found: HashSet<&str> = entries.iter().map(|entry| entry.id.as_str()).collect();
	if found != wanted {
		return Err(anyhow::anyhow!("one or more PACT block IDs were not found"));
	}
	let archive_path = index_path.with_file_name(
		index_path
			.file_name()
			.and_then(|name| name.to_str())
			.and_then(|name| name.strip_suffix(".blocks.jsonl"))
			.map(|stem| format!("{stem}.jsonl"))
			.ok_or_else(|| anyhow::anyhow!("invalid PACT sidecar name"))?,
	);
	let archive = std::fs::read_to_string(&archive_path)
		.with_context(|| format!("failed to read archive: {}", archive_path.display()))?;
	let lines: Vec<&str> = archive.lines().collect();
	let mut selected_lines = BTreeMap::new();
	for entry in entries {
		let block: Vec<Message> = (entry.archive_line_start..=entry.archive_line_end)
			.map(|line| {
				let raw = lines
					.get(line - 1)
					.ok_or_else(|| anyhow::anyhow!("archive line {line} is missing"))?;
				serde_json::from_str::<Message>(raw).context("invalid archived message")
			})
			.collect::<Result<_>>()?;
		if block_digest(&entry.id, &block) != entry.content_digest {
			return Err(anyhow::anyhow!(
				"PACT block {} failed content-address verification",
				entry.id
			));
		}
		for line in entry.archive_line_start..=entry.archive_line_end {
			let raw = lines
				.get(line - 1)
				.ok_or_else(|| anyhow::anyhow!("archive line {line} is missing"))?;
			selected_lines.insert(line, *raw);
		}
	}
	selected_lines
		.into_values()
		.map(|line| serde_json::from_str::<Message>(line).context("invalid archived message"))
		.collect()
}

/// Decode JSONL archive chunks into per-message verbatim contents. Lines that
/// fail to parse (e.g. the last line of a cap-truncated chunk) are skipped.
#[cfg(test)]
fn decode_jsonl_contents(chunks: &[String]) -> Vec<String> {
	chunks
		.iter()
		.flat_map(|chunk| chunk.lines())
		.filter_map(|line| serde_json::from_str::<Message>(line).ok())
		.map(|m| m.content)
		.collect()
}

/// Render the `<archive>` pointer embedded into a compressed summary.
///
/// The pointer tells the model the full raw transcript of the replaced
/// messages exists on disk and how to recall it. Kept terse — it is re-fed
/// into every subsequent compression cycle as part of the summary.
pub(crate) fn archive_pointer(path: &Path) -> String {
	format!(
		"<archive path=\"{}\">\nFull raw transcript of the messages this summary replaced (JSONL, one message per line). If you need exact code, error text, or tool output the summary elided, do not guess: prefer the `recall` tool with the block IDs cited in <folded_state> refs; otherwise read or search this file.\n</archive>",
		path.display()
	)
}

#[cfg(test)]
#[path = "archive_module_tests.rs"]
mod archive_tests;

#[cfg(test)]
#[path = "archive_tests.rs"]
mod tests;
