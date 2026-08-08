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

const ANALYSIS_OPEN: &str = "<analysis_findings>";
const ANALYSIS_CLOSE: &str = "</analysis_findings>";
const FINDING_OPEN: &str = "<finding>";
const FINDING_CLOSE: &str = "</finding>";

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

pub(super) fn format_compressed_entry_with_pact(
	body: &str,
	file_context: &str,
	compression_id: String,
	archive: Option<&super::archive::ArchiveBundle>,
	pact: &super::attention::PactContext,
) -> String {
	let (pinned, frontier_and_recall) = pact.render_live_bands(archive);
	let mut sections = pinned;
	sections.push('\n');
	if !body.is_empty() {
		sections.push_str(body);
		sections.push('\n');
	}
	if !frontier_and_recall.is_empty() {
		sections.push_str(&frontier_and_recall);
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
	if let Some(bundle) = archive {
		sections.push_str(&super::archive::archive_pointer(&bundle.path));
		sections.push('\n');
	}
	format!(
		"<conversation_summary id=\"{}\" controller=\"pact-v{}\">\n{}</conversation_summary>",
		compression_id,
		super::attention::CONTROLLER_VERSION,
		sections
	)
}

/// Strip the blocks that the session re-attaches itself from a prior
/// compressed summary, before re-feeding it to the next compression pass.
///
/// `<file_context>` — the embedded file bytes are stale; the AI re-requests
/// whatever it still needs via the structured `file_context` field.
///
/// `<analysis_findings>` — the accumulated union is re-attached by
/// `fold_analysis_findings` at render time, so showing it back to the
/// compressor only invites it to restate every entry in new words (which is
/// exactly what the old "carry forward all prior entries" instruction asked
/// for). Left in place it recurses the same way file bytes did: one measured
/// session re-fed 220 KB of findings into every compression call.
///
/// `<recall_index>` — this is runtime-generated navigation metadata, not
/// evidence. Re-feeding it makes every historical ID look newly referenced,
/// causing the live index to grow monotonically across compactions. IDs cited
/// by pinned, folded, or active state remain in the retained text and are
/// therefore carried forward with exact coordinates.
pub(super) fn strip_regrown_sections(summary: &str) -> String {
	let stripped = strip_block(summary, "<file_context>", "</file_context>");
	let stripped = strip_block(&stripped, ANALYSIS_OPEN, ANALYSIS_CLOSE);
	strip_block(
		&stripped,
		"<recall_index format=\"json\">",
		"</recall_index>",
	)
}

fn strip_block(summary: &str, open_tag: &str, close_tag: &str) -> String {
	if let Some(open) = summary.find(open_tag) {
		// Locate the matching close tag; if absent, drop everything from the
		// open onward (defensive — a malformed summary should still
		// strip cleanly rather than re-embed half the file dump).
		let close_end = summary[open + open_tag.len()..]
			.find(close_tag)
			.map(|i| open + open_tag.len() + i + close_tag.len())
			.unwrap_or(summary.len());
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

/// Recover the authoritative finding store from the newest rendered summary.
///
/// The store is runtime-only, while the rendered summary is persisted with the
/// session. Resumed sessions therefore rebuild the store from the latest
/// summary before recompression. Stop at the newest summary even when it has no
/// findings: an older summary is not allowed to resurrect entries that a newer
/// compaction intentionally evicted.
pub(super) fn latest_analysis_findings(messages: &[crate::session::Message]) -> Vec<String> {
	messages
		.iter()
		.rev()
		.find(|message| {
			message.role == "assistant"
				&& message
					.content
					.trim_start()
					.starts_with(SUMMARY_TAG_OPEN_PREFIX)
		})
		.map(|message| analysis_findings_from_summary(&message.content))
		.unwrap_or_default()
}

fn analysis_findings_from_summary(summary: &str) -> Vec<String> {
	let Some(open) = summary.find(ANALYSIS_OPEN) else {
		return Vec::new();
	};
	let body_start = open + ANALYSIS_OPEN.len();
	let Some(close) = summary[body_start..].find(ANALYSIS_CLOSE) else {
		return Vec::new();
	};
	let mut body = &summary[body_start..body_start + close];
	let mut findings = Vec::new();
	while let Some(item_open) = body.find(FINDING_OPEN) {
		let item_start = item_open + FINDING_OPEN.len();
		let Some(item_close) = body[item_start..].find(FINDING_CLOSE) else {
			break;
		};
		let finding = body[item_start..item_start + item_close].trim();
		if !finding.is_empty() {
			findings.push(finding.to_string());
		}
		body = &body[item_start + item_close + FINDING_CLOSE.len()..];
	}
	findings
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

/// Fold newly extracted findings into the session under a hard token budget.
///
/// Semantic similarity is deliberately NOT used as an equivalence test: a
/// correction and the stale claim it replaces are often close in embedding
/// space. We only collapse normalized exact duplicates. When everything does
/// not fit, cosine drives query-focused maximal-marginal-relevance selection:
/// keep findings relevant to the live task while penalizing redundant coverage.
/// A small recency term makes newer wording win ties, including corrections.
///
/// The token budget is the actual growth bound. Embedding failure falls back to
/// newest-first selection under the same budget, so optional semantic ranking
/// can never turn into unbounded retention.
pub(super) async fn fold_analysis_findings(
	session: &mut ChatSession,
	config: &Config,
	entries: &[String],
	focus: &str,
) -> Vec<String> {
	let budget = config.compression.analysis_findings_max_tokens;
	let findings = merge_latest_exact(&session.analysis_findings, entries);
	let before = findings.len();
	if budget == 0 || findings.is_empty() {
		session.analysis_findings.clear();
		return Vec::new();
	}

	let mut embedding_inputs: Vec<String> = findings.iter().map(|s| embedding_input(s)).collect();
	let focus_index = if focus.trim().is_empty() {
		None
	} else {
		embedding_inputs.push(embedding_input(focus));
		Some(embedding_inputs.len() - 1)
	};

	let selected = match crate::embeddings::embed_many(&embedding_inputs).await {
		Ok(vectors) if vectors.len() == embedding_inputs.len() => select_findings_with_vectors(
			&findings,
			&vectors[..findings.len()],
			focus_index.map(|i| vectors[i].as_slice()),
			budget,
		),
		Ok(_) => select_newest_with_budget(&findings, budget),
		Err(e) => {
			log_debug!("Findings ranking fell back to recency: {}", e);
			select_newest_with_budget(&findings, budget)
		}
	};

	session.analysis_findings = selected;
	log_debug!(
		"Retained {} of {} analysis findings within {} tokens",
		session.analysis_findings.len(),
		before,
		budget
	);
	session.analysis_findings.clone()
}

fn merge_latest_exact(held: &[String], entries: &[String]) -> Vec<String> {
	let mut findings: Vec<String> = Vec::new();
	let mut keys: Vec<String> = Vec::new();
	for finding in held.iter().chain(entries) {
		let trimmed = finding.trim();
		if trimmed.is_empty() {
			continue;
		}
		let key = trimmed
			.split_whitespace()
			.collect::<Vec<_>>()
			.join(" ")
			.to_lowercase();
		if let Some(index) = keys.iter().position(|existing| existing == &key) {
			findings[index] = trimmed.to_string();
		} else {
			keys.push(key);
			findings.push(trimmed.to_string());
		}
	}
	findings
}

fn embedding_input(text: &str) -> String {
	crate::embeddings::chunk_to_token_limit(text, crate::embeddings::EMBED_MAX_INPUT_TOKENS)
		.into_iter()
		.next()
		.unwrap_or_default()
}

pub(super) fn analysis_findings_tokens(findings: &[String]) -> usize {
	if findings.is_empty() {
		return 0;
	}
	let mut rendered = String::from(ANALYSIS_OPEN);
	rendered.push('\n');
	for finding in findings {
		rendered.push_str(FINDING_OPEN);
		rendered.push_str(finding.trim());
		rendered.push_str(FINDING_CLOSE);
		rendered.push('\n');
	}
	rendered.push_str(ANALYSIS_CLOSE);
	crate::session::estimate_tokens(&rendered)
}

fn fits_budget(findings: &[String], selected: &[usize], candidate: usize, budget: usize) -> bool {
	let mut indices = selected.to_vec();
	indices.push(candidate);
	indices.sort_unstable();
	let values: Vec<String> = indices.iter().map(|&i| findings[i].clone()).collect();
	analysis_findings_tokens(&values) <= budget
}

pub(super) fn select_findings_with_vectors(
	findings: &[String],
	vectors: &[Vec<f32>],
	focus: Option<&[f32]>,
	budget: usize,
) -> Vec<String> {
	if findings.len() != vectors.len() || budget == 0 {
		return select_newest_with_budget(findings, budget);
	}

	let mut selected: Vec<usize> = Vec::new();
	loop {
		let mut best: Option<(usize, f32)> = None;
		for i in 0..findings.len() {
			if selected.contains(&i) || !fits_budget(findings, &selected, i, budget) {
				continue;
			}
			let relevance = focus
				.map(|query| crate::embeddings::cosine(&vectors[i], query))
				.unwrap_or(0.0)
				.clamp(0.0, 1.0);
			let redundancy = selected
				.iter()
				.map(|&j| crate::embeddings::cosine(&vectors[i], &vectors[j]))
				.fold(0.0_f32, f32::max)
				.clamp(0.0, 1.0);
			let recency = (i + 1) as f32 / findings.len().max(1) as f32;
			let score = 0.65 * relevance + 0.10 * recency - 0.35 * redundancy;
			if best.is_none_or(|(best_i, best_score)| {
				score > best_score || (score == best_score && i > best_i)
			}) {
				best = Some((i, score));
			}
		}
		let Some((index, _)) = best else {
			break;
		};
		selected.push(index);
	}
	selected.sort_unstable();
	selected.into_iter().map(|i| findings[i].clone()).collect()
}

pub(super) fn select_newest_with_budget(findings: &[String], budget: usize) -> Vec<String> {
	let mut selected = Vec::new();
	for i in (0..findings.len()).rev() {
		if fits_budget(findings, &selected, i, budget) {
			selected.push(i);
		}
	}
	selected.sort_unstable();
	selected.into_iter().map(|i| findings[i].clone()).collect()
}
