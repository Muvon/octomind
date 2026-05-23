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
/// limit (keeping the most recent entries).
///
/// Replaces the old `<knowledge>` tag extractor — entries now arrive
/// pre-structured as `Vec<String>` from the schema response.
pub(super) fn fold_critical_knowledge(
	session: &mut ChatSession,
	config: &Config,
	entries: &[String],
) {
	let new_entries: Vec<&String> = entries.iter().filter(|e| !e.trim().is_empty()).collect();
	if new_entries.is_empty() {
		return;
	}

	let retention_limit = config.compression.knowledge_retention;
	let added = new_entries.len();

	for entry in new_entries {
		log_debug!("Extracted critical knowledge: {}", entry);
		session.critical_knowledge.push(entry.clone());
		let _ = crate::session::logger::log_knowledge_entry(&session.session.info.name, entry);
	}

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
