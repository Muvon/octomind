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

// Compression-summary formatting and <knowledge> tag handling.
//
// Extracted from the main compression module to keep this cluster of pure
// text-manipulation helpers in one focused place. Visibility is `pub(super)`
// so both the parent `conversation_compression` module and its `tests`
// submodule can reach these helpers without exposing them crate-wide.

use crate::config::Config;
use crate::session::chat::session::ChatSession;
use crate::{log_debug, log_info};

pub(super) fn format_compressed_entry_with_context(
	context: &str,
	file_context: &str,
	compression_id: String,
) -> String {
	let mut sections = Vec::new();

	if !context.is_empty() {
		sections.push(context.to_string());
	}

	// Add file context if provided (automatically expanded from AI's <context> tags)
	if !file_context.is_empty() {
		sections.push(format!(
			"**FILE CONTEXT** (auto-expanded):\n{}",
			file_context
		));
	}

	format!(
		"## Conversation Summary [COMPRESSED: {}]\n\n{}",
		compression_id,
		sections.join("\n\n"),
	)
}

/// Strip the FILE CONTEXT section from a prior compressed summary before re-feeding it
/// to the next compression pass.
///
/// When a summary is re-compressed, the embedded file bytes are stale and bloat the
/// prompt. The AI will re-request any files it still needs via <context> tags.
/// Returns the summary text with the FILE CONTEXT block removed, trimmed.
pub(super) fn strip_file_context_from_summary(summary: &str) -> String {
	const SENTINEL: &str = "\n\n**FILE CONTEXT** (auto-expanded):";
	if let Some(pos) = summary.find(SENTINEL) {
		summary[..pos].trim().to_string()
	} else {
		summary.trim().to_string()
	}
}

/// Extract <knowledge> tags from AI compression response, store in session, and log.
/// Trims to the configured knowledge_retention limit (keeps most recent entries).
pub(super) fn extract_and_store_knowledge(
	session: &mut ChatSession,
	config: &Config,
	content: &str,
) {
	let knowledge_entries = parse_knowledge_tags(content);
	if knowledge_entries.is_empty() {
		return;
	}

	let retention_limit = config.compression.knowledge_retention;
	for entry in &knowledge_entries {
		log_debug!("Extracted critical knowledge: {}", entry);
		session.critical_knowledge.push(entry.clone());

		// Persist to session log
		let _ = crate::session::logger::log_knowledge_entry(&session.session.info.name, entry);
	}

	// Trim to retention limit (keep most recent)
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
		knowledge_entries.len(),
		session.critical_knowledge.len()
	);
}

/// Parse all <knowledge>...</knowledge> tags from text.
/// Returns the trimmed content of each tag.
pub(super) fn parse_knowledge_tags(content: &str) -> Vec<String> {
	let mut entries = Vec::new();
	let mut search_from = 0;
	while let Some(start) = content[search_from..].find("<knowledge>") {
		let abs_start = search_from + start + "<knowledge>".len();
		if let Some(end) = content[abs_start..].find("</knowledge>") {
			let abs_end = abs_start + end;
			let entry = content[abs_start..abs_end].trim().to_string();
			if !entry.is_empty() {
				entries.push(entry);
			}
			search_from = abs_end + "</knowledge>".len();
		} else {
			break;
		}
	}
	entries
}

/// Strip <knowledge>...</knowledge> tags from summary text.
/// The knowledge is already extracted and stored separately — no need to keep it in the summary.
pub(super) fn strip_knowledge_tags(content: &str) -> String {
	let mut result = content.to_string();
	while let Some(start) = result.find("<knowledge>") {
		if let Some(end) = result[start..].find("</knowledge>") {
			let abs_end = start + end + "</knowledge>".len();
			result = format!("{}{}", &result[..start], &result[abs_end..]);
		} else {
			break;
		}
	}
	result.trim().to_string()
}
