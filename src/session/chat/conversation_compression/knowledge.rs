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

// XML wrapper around the rendered summary + folding of critical knowledge
// into the session.
//
// The XML/regex parsers that used to live here for `<knowledge>` and
// `<context>` tags are gone — the model now returns a typed JSON object
// (`schema::CompressionSummary`) and we render it deterministically as XML.
//
// Why XML for the wrapper too: Claude is tuned to attend to XML-delimited
// sections. A summary is the largest *re-fed* block in subsequent
// compressions, so structuring it as XML (instead of `## H2 markdown`)
// makes the model's section detection more reliable across paraphrase
// cycles.

use crate::config::Config;
use crate::session::chat::session::ChatSession;
use crate::{log_debug, log_info};

/// Open tag of the conversation-summary wrapper. Used as the prior-summary
/// sentinel when re-feeding a prior summary into the next compression
/// transcript (`prompt.rs`) and as the file-context strip boundary below.
pub(super) const SUMMARY_TAG_OPEN_PREFIX: &str = "<conversation_summary";

pub(super) fn format_compressed_entry_with_context(
	body: &str,
	file_context: &str,
	compression_id: String,
	archive_path: Option<&std::path::Path>,
) -> String {
	let mut sections = String::new();

	if !body.is_empty() {
		sections.push_str(body);
		sections.push('\n');
	}

	if !file_context.is_empty() {
		sections.push_str("<file_context>\n");
		sections.push_str(file_context);
		if !file_context.ends_with('\n') {
			sections.push('\n');
		}
		sections.push_str("</file_context>\n");
	}

	// Addressable-recall pointer: the raw drained transcript lives on disk.
	// Embedded inside the summary so every future compression cycle (and the
	// model itself) knows the elided detail is recoverable, not lost.
	if let Some(path) = archive_path {
		sections.push_str(&super::archive::archive_pointer(path));
		sections.push('\n');
	}

	format!(
		"<conversation_summary id=\"{}\">\n{}</conversation_summary>",
		compression_id, sections
	)
}

/// Strip the `<file_context>` block from a prior compressed summary before
/// re-feeding it to the next compression pass. When a summary is
/// re-compressed, the embedded file bytes are stale and bloat the prompt —
/// the AI will re-request whatever it still needs via the structured
/// `file_context` field of the new summary.
pub(super) fn strip_file_context_from_summary(summary: &str) -> String {
	const OPEN: &str = "<file_context>";
	const CLOSE: &str = "</file_context>";
	let bytes = summary.as_bytes();
	if let Some(open) = summary.find(OPEN) {
		// Locate the matching close tag; if absent, drop everything from the
		// open onward (defensive — a malformed summary should still
		// strip cleanly rather than re-embed half the file dump).
		let close_end = summary[open + OPEN.len()..]
			.find(CLOSE)
			.map(|i| open + OPEN.len() + i + CLOSE.len())
			.unwrap_or(bytes.len());
		let mut head = summary[..open].trim_end().to_string();
		let tail = summary[close_end..].trim_start().to_string();
		if !tail.is_empty() {
			head.push('\n');
			head.push_str(&tail);
		}
		head.trim().to_string()
	} else {
		summary.trim().to_string()
	}
}

/// Persist `critical_knowledge` entries from the typed summary onto the
/// session and log them. Trims to the configured `knowledge_retention`
/// limit (keeping the most recent entries); `0` disables trimming entirely,
/// which is what a session that must never forget should be configured with.
///
/// Entries are deduped on the way in — the model re-emits carried-forward
/// knowledge every cycle, so without this the store fills with copies and a
/// retention limit then evicts genuinely distinct entries.
///
/// Replaces the old `<knowledge>` tag extractor — entries now arrive
/// pre-structured as `Vec<String>` from the schema response.
pub(super) fn fold_critical_knowledge(
	session: &mut ChatSession,
	config: &Config,
	entries: &[String],
) {
	let mut added = 0usize;
	for entry in entries.iter().filter(|e| !e.trim().is_empty()) {
		if session.critical_knowledge.iter().any(|k| k == entry) {
			continue;
		}
		log_debug!("Extracted critical knowledge: {}", entry);
		session.critical_knowledge.push(entry.clone());
		let _ = crate::session::logger::log_knowledge_entry(&session.session.info.name, entry);
		added += 1;
	}
	if added == 0 {
		return;
	}

	let retention_limit = config.compression.knowledge_retention;
	if retention_limit > 0 && session.critical_knowledge.len() > retention_limit {
		let drain_count = session.critical_knowledge.len() - retention_limit;
		session.critical_knowledge.drain(..drain_count);
		log_debug!(
			"Trimmed critical knowledge to {} entries (retention limit)",
			retention_limit
		);
	}

	log_info!(
		"Stored {} new critical knowledge entries ({} total)",
		added,
		session.critical_knowledge.len()
	);
}

/// Accumulate `analysis_findings` across compactions and return the full set.
///
/// The schema and prompt both ask the model to "carry forward all prior
/// entries, append new ones" — measured over 19 compactions it does not: the
/// list churned 6→8→5→…→10→…→4, and between two cycles all 9 prior findings
/// were dropped and 0 retained, deleting the root cause the agent had already
/// established, which it then re-derived 37 times. Carry-forward is therefore
/// enforced HERE, in code, and the model's output is treated as "what I
/// learned this cycle" rather than as the authoritative list.
///
/// UNBOUNDED and deliberately not configurable: findings are scoped to one
/// task, so the only correct time to drop them is when the task itself
/// changes — which `ChatSession::add_user_message` does on a genuine new user
/// turn, alongside the detector reset. A count-based cap would evict the
/// earliest conclusions, and those are exactly the load-bearing ones (the root
/// cause is usually found early and re-confirmed late). Dedupe is exact-match;
/// order is oldest-first so earlier conclusions stay stable across renders.
pub(super) fn fold_analysis_findings(session: &mut ChatSession, entries: &[String]) -> Vec<String> {
	for entry in entries.iter().filter(|e| !e.trim().is_empty()) {
		if !session.analysis_findings.iter().any(|f| f == entry) {
			session.analysis_findings.push(entry.clone());
		}
	}
	session.analysis_findings.clone()
}
