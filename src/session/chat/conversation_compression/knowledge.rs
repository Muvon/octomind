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

/// Robust z-score at which a candidate finding counts as a restatement.
/// Dimensionless — the conventional 3-sigma bound, applied to the session's
/// own similarity distribution rather than to a raw cosine value.
const RESTATEMENT_Z: f32 = 3.0;

/// Normal-consistency constant relating MAD to sigma (sigma ~= 1.4826 * MAD).
const MAD_TO_SIGMA: f32 = 1.4826;

/// Fewer held findings than this and the nearest-neighbour distribution is too
/// small to estimate an outlier bound from.
const MIN_HELD_FOR_DISTRIBUTION: usize = 5;

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
pub(super) fn strip_regrown_sections(summary: &str) -> String {
	let stripped = strip_block(summary, "<file_context>", "</file_context>");
	strip_block(&stripped, "<analysis_findings>", "</analysis_findings>")
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
/// No count cap: findings are scoped to one task, so the only correct time to
/// drop them all is when the task itself changes — which
/// `ChatSession::add_user_message` does on a genuine new user turn, alongside
/// the detector reset. A count-based cap evicts the earliest conclusions, and
/// those are exactly the load-bearing ones (the root cause is usually found
/// early and re-confirmed late).
///
/// Growth is bounded by restatement detection instead. Exact-match alone was
/// not enough: the model restates the same conclusion in new words, so every
/// restatement landed as a new entry — one measured session reached 888
/// findings (220 KB, ~55K tokens per prompt) over 135 compactions.
///
/// The test is adaptive, never an absolute cosine cutoff. Cosine is not
/// calibrated across models or domains: findings from one dense investigation
/// share vocabulary and sit high, findings spanning subsystems sit low, so a
/// fixed number means "restatement" in one session and "unrelated" in the
/// next. The held findings are ones already accepted as distinct from each
/// other, so they are the null model — take each held entry's similarity to
/// its nearest held neighbour and a candidate is a restatement only when it
/// beats that distribution as an outlier (see `restatement_bound`).
///
/// Order stays oldest-first so earlier conclusions are stable across renders,
/// and the held phrasing wins — the first statement of a conclusion is kept,
/// later paraphrases discarded.
pub(super) async fn fold_analysis_findings(
	session: &mut ChatSession,
	entries: &[String],
) -> Vec<String> {
	let candidates: Vec<String> = entries
		.iter()
		.filter(|e| !e.trim().is_empty())
		.filter(|e| !session.analysis_findings.iter().any(|f| f == *e))
		.cloned()
		.collect();
	if candidates.is_empty() {
		return session.analysis_findings.clone();
	}

	// Too few held findings to estimate the null distribution — exact match
	// is all we can honestly claim.
	if session.analysis_findings.len() < MIN_HELD_FOR_DISTRIBUTION {
		session.analysis_findings.extend(candidates);
		return session.analysis_findings.clone();
	}

	// One batched embed for held entries and candidates together; the cache
	// makes held entries free on every cycle after the one that added them.
	let mut texts = session.analysis_findings.clone();
	let held = texts.len();
	texts.extend(candidates.iter().cloned());
	let vectors = match crate::embeddings::embed_many(&texts).await {
		Ok(v) => v,
		Err(e) => {
			// Embeddings are an external dependency (model load can fail);
			// losing dedupe is recoverable, losing the compaction is not.
			log_debug!("Findings dedupe fell back to exact-match: {}", e);
			session.analysis_findings.extend(candidates);
			return session.analysis_findings.clone();
		}
	};

	let bound = restatement_bound(&vectors[..held]);
	// Grows as candidates are accepted, so two paraphrases arriving in the
	// same cycle collapse against each other, not only against held entries.
	let mut kept: Vec<&[f32]> = vectors[..held].iter().map(|v| v.as_slice()).collect();
	let mut added = 0usize;
	for (i, entry) in candidates.iter().enumerate() {
		let vec = &vectors[held + i];
		let nearest = kept
			.iter()
			.map(|k| crate::embeddings::cosine(k, vec))
			.fold(f32::MIN, f32::max);
		if nearest > bound {
			log_debug!("Dropped restated finding (sim {:.3}): {}", nearest, entry);
			continue;
		}
		kept.push(vec.as_slice());
		session.analysis_findings.push(entry.clone());
		added += 1;
	}
	log_debug!(
		"Folded {} of {} findings, bound {:.3} ({} total)",
		added,
		candidates.len(),
		bound,
		session.analysis_findings.len()
	);
	session.analysis_findings.clone()
}

/// Similarity above which a candidate is a restatement rather than a new
/// finding, derived from the held findings themselves.
///
/// Builds the null distribution — each held finding's cosine to its nearest
/// other held finding, i.e. how close two *distinct* findings get in this
/// session — and returns a robust upper outlier bound on it. MAD rather than
/// standard deviation because it has a 50% breakdown point: if restatements
/// have already leaked into the held set they cannot inflate the bound and
/// hide the next ones. `MAD_TO_SIGMA` is the usual normal-consistency
/// constant, so `RESTATEMENT_Z` is an ordinary dimensionless z-score and
/// carries no assumption about the embedding model or the subject matter.
/// The bound is computed under the Fisher transform because cosine is a
/// bounded correlation, not an unbounded quantity.
///
/// A degenerate spread (every neighbour equidistant) would collapse the bound
/// onto the median and start discarding real findings, so it falls back to the
/// closest observed distinct pair: to be a restatement you must then be nearer
/// than any two distinct findings have ever been here.
pub(super) fn restatement_bound(held: &[Vec<f32>]) -> f32 {
	let nearest: Vec<f32> = held
		.iter()
		.enumerate()
		.map(|(i, a)| {
			held.iter()
				.enumerate()
				.filter(|(j, _)| *j != i)
				.map(|(_, b)| crate::embeddings::cosine(a, b))
				.fold(f32::MIN, f32::max)
		})
		.collect();
	// Fisher z. Cosine is a correlation: bounded in [-1, 1], and its variance
	// shrinks as it approaches the ends. A median + k*MAD bound computed on raw
	// cosines routinely lands above 1.0, where nothing can ever exceed it and
	// the test silently never fires (measured: bound 1.437). atanh maps the
	// bounded scale onto the whole real line, where an additive robust bound is
	// meaningful; tanh maps it back, always below 1.
	let mut z: Vec<f32> = nearest.iter().copied().map(fisher_z).collect();
	let center = median(&mut z);
	let mut deviations: Vec<f32> = z.iter().map(|v| (v - center).abs()).collect();
	let mad = median(&mut deviations);
	if mad > f32::EPSILON {
		(center + RESTATEMENT_Z * MAD_TO_SIGMA * mad).tanh()
	} else {
		// Zero spread means the sample carries no information about what
		// "unusually similar" looks like here, so fail open: keep everything
		// rather than guess a cutoff. Dropping a real finding costs more than
		// carrying a duplicate for one more cycle.
		1.0
	}
}

/// Fisher transform, clamped short of the poles where `atanh` is infinite.
fn fisher_z(r: f32) -> f32 {
	const LIMIT: f32 = 0.999_999;
	r.clamp(-LIMIT, LIMIT).atanh()
}

/// Median of `values`, sorting in place. Callers pass scratch vectors.
fn median(values: &mut [f32]) -> f32 {
	values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
	let n = values.len();
	if n == 0 {
		return 0.0;
	}
	if n % 2 == 1 {
		values[n / 2]
	} else {
		(values[n / 2 - 1] + values[n / 2]) / 2.0
	}
}
