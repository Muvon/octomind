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

//! Observation masking — stale tool results age out of the live context.
//!
//! Once a tool round lands, tool results older than the most recent
//! [`KEEP_RECENT_ROUNDS`] rounds are replaced IN MEMORY with a short
//! fingerprint placeholder. The reasoning and tool-call structure of the
//! trajectory stays intact — only aged observation bodies go. Simple masking
//! matches or beats LLM summarization on solve rate at roughly half the cost
//! (arXiv 2508.21433, replicated on SWE-bench Verified), and costs no model
//! call.
//!
//! In-memory only: the session file keeps the original bodies (report/share/
//! resume replay them; a resumed session simply re-masks on its next round).
//! Masked originals are retained in a session-keyed store so the claim-check
//! can still verify «quotes» that cite an aged-out output, and the dedup state
//! is cleared whenever masking fires — a re-run of a masked call must return
//! the real bytes again, not a "you already have it" placeholder pointing at
//! content that is no longer visible.

use crate::session::Message;
use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

/// Tool rounds whose results stay verbatim. Beyond this recency window the
/// model acts on its own reasoning about an output, not the raw bytes; the
/// masking paper's ablations hold accuracy with a window this small.
const KEEP_RECENT_ROUNDS: usize = 3;

/// Results shorter than this are never masked — same rationale as dedup's
/// floor: short outputs are verdicts the model must keep seeing, and the
/// placeholder would save nothing. Also makes the pass idempotent (every
/// placeholder is far below this).
const MIN_MASK_CONTENT_LEN: usize = 500;

/// Max chars of the original's first/last line quoted in the placeholder.
const SNIPPET_CHARS: usize = 120;

/// Sentinel embedded in every mask placeholder, tool-agnostically — mirrors
/// `DEDUP_NOTICE_TAG` so future detectors can key on it.
pub const MASK_NOTICE_TAG: &str = "stale tool result masked";

type GlobalMap = HashMap<String, Vec<String>>;

static MASKED_ORIGINALS: OnceLock<RwLock<GlobalMap>> = OnceLock::new();

fn state() -> &'static RwLock<GlobalMap> {
	MASKED_ORIGINALS.get_or_init(|| RwLock::new(HashMap::new()))
}

fn session_key() -> String {
	crate::session::context::current_session_id().unwrap_or_else(|| "_global_".to_string())
}

/// Original bodies of every result masked this session — the claim-check
/// includes these so a verbatim quote from an aged-out output still verifies.
pub fn masked_originals() -> Vec<String> {
	state()
		.read()
		.unwrap()
		.get(&session_key())
		.cloned()
		.unwrap_or_default()
}

/// Drop the mask store for the current session. Called from the compaction
/// path alongside the dedup reset: once messages are drained, quotes against
/// them are already unverifiable, so retaining originals would only grow.
pub fn clear_current_session() {
	state().write().unwrap().remove(&session_key());
}

/// First `SNIPPET_CHARS` chars of a line, char-boundary safe.
fn snippet(line: &str) -> String {
	let line = line.trim();
	if line.chars().count() > SNIPPET_CHARS {
		let cut: String = line.chars().take(SNIPPET_CHARS).collect();
		format!("{cut}…")
	} else {
		line.to_string()
	}
}

fn placeholder(tool_name: &str, content: &str) -> String {
	let first = content
		.lines()
		.find(|l| !l.trim().is_empty())
		.map(snippet)
		.unwrap_or_default();
	let last = content
		.lines()
		.rev()
		.find(|l| !l.trim().is_empty())
		.map(snippet)
		.unwrap_or_default();
	let fingerprint = if first == last {
		format!("it began: {first}")
	} else {
		format!("it began: {first} — and ended: {last}")
	};
	let size = if content.len() >= 1024 {
		format!("{:.1}k", content.len() as f64 / 1024.0)
	} else {
		format!("{}b", content.len())
	};
	format!(
		"[{MASK_NOTICE_TAG} — `{tool_name}` ({size}): {fingerprint}. Aged out of the recent window; act on what you already concluded from it, or re-run the call if you genuinely need the full output again.]"
	)
}

/// Mask tool results older than the last [`KEEP_RECENT_ROUNDS`] tool rounds.
/// A round starts at an assistant message carrying tool_calls. Returns the
/// number of results masked this pass. Skips short results (verdicts), results
/// with media attachments, and everything inside the recency window; already-
/// masked results fall under the length floor, so the pass is idempotent.
pub fn mask_stale_tool_results(messages: &mut [Message]) -> usize {
	let round_starts: Vec<usize> = messages
		.iter()
		.enumerate()
		.filter(|(_, m)| {
			m.role == "assistant" && m.tool_calls.as_ref().is_some_and(|tc| !tc.is_null())
		})
		.map(|(i, _)| i)
		.collect();
	if round_starts.len() <= KEEP_RECENT_ROUNDS {
		return 0;
	}
	let frontier = round_starts[round_starts.len() - KEEP_RECENT_ROUNDS];
	let mut originals = Vec::new();
	for msg in &mut messages[..frontier] {
		if msg.role != "tool"
			|| msg.content.len() < MIN_MASK_CONTENT_LEN
			|| msg.images.is_some()
			|| msg.videos.is_some()
		{
			continue;
		}
		let tool = msg.name.as_deref().unwrap_or("tool");
		let masked = placeholder(tool, &msg.content);
		originals.push(std::mem::replace(&mut msg.content, masked));
	}
	let masked_count = originals.len();
	if masked_count > 0 {
		state()
			.write()
			.unwrap()
			.entry(session_key())
			.or_default()
			.append(&mut originals);
		// Masked bodies are no longer in the model's context, so an identical
		// re-run must be answered with real bytes — a dedup placeholder saying
		// "you already have it" would starve the model into a retry loop.
		crate::session::dedup::clear_current_session();
	}
	masked_count
}

#[cfg(test)]
mod tests {
	use super::*;

	fn assistant_with_calls() -> Message {
		Message {
			role: "assistant".to_string(),
			content: "calling".to_string(),
			tool_calls: Some(serde_json::json!([{"name": "view"}])),
			..Default::default()
		}
	}

	fn tool_result(content: &str) -> Message {
		Message {
			role: "tool".to_string(),
			content: content.to_string(),
			tool_call_id: Some("id".to_string()),
			name: Some("view".to_string()),
			..Default::default()
		}
	}

	fn rounds(n: usize, body: &str) -> Vec<Message> {
		let mut v = Vec::new();
		for _ in 0..n {
			v.push(assistant_with_calls());
			v.push(tool_result(body));
		}
		v
	}

	#[test]
	fn keeps_recent_rounds_verbatim() {
		let body = "line one\n".repeat(100);
		let mut msgs = rounds(KEEP_RECENT_ROUNDS, &body);
		assert_eq!(mask_stale_tool_results(&mut msgs), 0);
		assert!(msgs.iter().all(|m| !m.content.contains(MASK_NOTICE_TAG)));
		clear_current_session();
	}

	#[test]
	fn masks_only_beyond_window_and_is_idempotent() {
		let body = format!("first line\n{}last line", "x\n".repeat(300));
		let mut msgs = rounds(KEEP_RECENT_ROUNDS + 2, &body);
		assert_eq!(mask_stale_tool_results(&mut msgs), 2);
		let masked: Vec<&Message> = msgs
			.iter()
			.filter(|m| m.content.contains(MASK_NOTICE_TAG))
			.collect();
		assert_eq!(masked.len(), 2);
		// Placeholder fingerprints the original and names the tool.
		assert!(masked[0].content.contains("first line"));
		assert!(masked[0].content.contains("last line"));
		assert!(masked[0].content.contains("view"));
		// Second pass with no new rounds changes nothing.
		assert_eq!(mask_stale_tool_results(&mut msgs), 0);
		clear_current_session();
	}

	#[test]
	fn short_results_never_masked() {
		let mut msgs = rounds(KEEP_RECENT_ROUNDS + 2, "[OK] No errors");
		assert_eq!(mask_stale_tool_results(&mut msgs), 0);
		clear_current_session();
	}

	#[test]
	fn originals_retained_for_claim_check() {
		clear_current_session();
		let body = format!("needle-quote-42\n{}", "x\n".repeat(300));
		let mut msgs = rounds(KEEP_RECENT_ROUNDS + 1, &body);
		assert_eq!(mask_stale_tool_results(&mut msgs), 1);
		let originals = masked_originals();
		assert_eq!(originals.len(), 1);
		assert!(originals[0].contains("needle-quote-42"));
		clear_current_session();
		assert!(masked_originals().is_empty());
	}

	#[test]
	fn non_tool_roles_untouched() {
		let body = "y\n".repeat(300);
		let mut msgs = rounds(KEEP_RECENT_ROUNDS + 1, &body);
		// A long user message older than the frontier must stay verbatim.
		msgs.insert(
			0,
			Message {
				role: "user".to_string(),
				content: "z\n".repeat(300),
				..Default::default()
			},
		);
		mask_stale_tool_results(&mut msgs);
		assert!(!msgs[0].content.contains(MASK_NOTICE_TAG));
		clear_current_session();
	}
}
