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

//! Detectors — deterministic, free, every turn.
//!
//! Two free signals are fused before any model is woken:
//! 1. **Self-report** — the agent annotates each turn with a `<sup>state</sup>`
//!    token (it already knows whether it is exploring / stuck / done).
//! 2. **Novelty counters** — derived from a single primitive: did this action
//!    add *new information* to the agent's state? Loop = the same result repeats;
//!    no-progress = a window of actions with zero novelty.
//!
//! Agreement needs no model. Only a *conflict* (e.g. counter says "no progress"
//! while the agent reports `progressing`) is worth the rare model confirmation.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashSet, VecDeque};
use std::hash::{Hash, Hasher};

/// The agent's self-reported state for a turn, parsed from its `<sup>…</sup>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelfReport {
	Exploring,
	Progressing,
	Blocked,
	NeedInput,
	Done,
}

impl SelfReport {
	fn from_token(s: &str) -> Option<Self> {
		match s.trim().to_ascii_lowercase().as_str() {
			"exploring" => Some(Self::Exploring),
			"progressing" => Some(Self::Progressing),
			"blocked" => Some(Self::Blocked),
			"need_input" | "need-input" | "needinput" => Some(Self::NeedInput),
			"done" => Some(Self::Done),
			_ => None,
		}
	}
}

/// One-time system-side instruction that makes the agent self-annotate. Injected
/// out-of-band; the resulting tags are stripped before display.
pub const SELF_REPORT_INSTRUCTION: &str = "\
<supervisor>
End every response with a status tag on its own final line, in this exact form:
`<sup>STATE · brief reason</sup>`
Replace STATE with exactly one of these words — never write the literal text \"STATE\":
exploring, progressing, blocked, need_input, done. The reason is a few words.
- `done` only when the user's task is fully complete.
- `need_input` when you are asking the user a question and waiting on them.
- `blocked` when you are stuck and cannot proceed.
- `exploring` while still gathering context; `progressing` while actively making changes.
Examples: `<sup>progressing · wiring store registration</sup>` or `<sup>done · migration verified</sup>`
This tag is for the system and is hidden from the user. Always include exactly one.
</supervisor>";

/// One-time system-side instruction enabling evidence-bound claims. The agent
/// backs load-bearing factual claims about the codebase with a verbatim quote it
/// actually saw in a tool result; [`unverified_citations`] then deterministically
/// checks the quote really occurs in some tool output, catching fabricated
/// citations for free (no model call).
pub const EVIDENCE_INSTRUCTION: &str = "\
<supervisor>
When you assert a load-bearing fact about the code or repo (a path, signature, value, or concrete behavior), back it with text you actually saw in a tool result, quoted verbatim between guillemets, in this exact form:
[evidence: <locator> «exact text copied verbatim from the tool output»]
Quote verbatim — do not paraphrase inside « ». Tag only load-bearing factual claims; do NOT tag plans, reasoning, or general knowledge. A « » quote that is not found in any tool result you received will be flagged and you will be asked to re-ground it against real output or retract the claim.
</supervisor>";

/// Parse the *last* `<sup>…</sup>` token from a response. Returns the state and
/// an optional short reason. Tolerant of the `·` or `|` reason separator.
pub fn parse_self_report(text: &str) -> Option<(SelfReport, Option<String>)> {
	let end = text.rfind("</sup>")?;
	let start = text[..end].rfind("<sup>")? + "<sup>".len();
	let inner = text[start..end].trim();
	// Normal: the body leads with the state. Echo: a model copied the literal
	// `STATE` placeholder from the instruction, so the real state is the next
	// token (`<sup>STATE · done</sup>` → done). Robust to `·`, `|`, `:`, `-`, space.
	let lead = leading_state_token(inner);
	let (state, after) = match SelfReport::from_token(&lead) {
		Some(s) => (s, &inner[lead.len()..]),
		None if lead.eq_ignore_ascii_case("state") => {
			let rest = inner[lead.len()..].trim_start_matches([' ', '·', '|', ':', '-', '\t']);
			let next = leading_state_token(rest);
			(SelfReport::from_token(&next)?, &rest[next.len()..])
		}
		None => return None,
	};
	let reason = after
		.trim_start_matches([' ', '·', '|', ':', '-', '\t'])
		.trim();
	Some((state, (!reason.is_empty()).then(|| reason.to_string())))
}

/// The leading identifier run (`[A-Za-z_-]+`) of a `<sup>` body — the candidate
/// state token, separator-agnostic.
fn leading_state_token(inner: &str) -> String {
	inner
		.trim_start()
		.chars()
		.take_while(|c| c.is_ascii_alphabetic() || *c == '_' || *c == '-')
		.collect()
}

/// Does this `<sup>` body look like a self-report rather than legitimate
/// superscript (`2`, `th`, `®`)? True when it leads with a known state, with the
/// `STATE` placeholder a model may echo from the instruction, or carries the
/// reason separator (`·`/`|`) that real superscript never contains. This is the
/// safety net: an echoed or malformed report still never reaches the screen.
fn is_self_report_body(inner: &str) -> bool {
	let lead = leading_state_token(inner);
	SelfReport::from_token(&lead).is_some()
		|| lead.eq_ignore_ascii_case("state")
		|| inner.contains('·')
		|| inner.contains('|')
}

/// Remove `<sup>…</sup>` tokens that look like a self-report (see
/// [`is_self_report_body`]), leaving legitimate superscript markup untouched.
pub fn strip_self_report(text: &str) -> String {
	let mut out = String::with_capacity(text.len());
	let mut rest = text;
	while let Some(start) = rest.find("<sup>") {
		match rest[start..].find("</sup>") {
			Some(rel_end) => {
				let inner = &rest[start + "<sup>".len()..start + rel_end];
				if is_self_report_body(inner) {
					// Drop this token; keep text before it.
					out.push_str(&rest[..start]);
					rest = &rest[start + rel_end + "</sup>".len()..];
				} else {
					// Not ours — keep `<sup>…</sup>` verbatim and continue past it.
					let keep_to = start + rel_end + "</sup>".len();
					out.push_str(&rest[..keep_to]);
					rest = &rest[keep_to..];
				}
			}
			None => break,
		}
	}
	out.push_str(rest);
	out.trim_end().to_string()
}

/// Deterministic evidence check: return the verbatim `« »`-delimited quotes in
/// `response` that do NOT appear in any of `tool_outputs`. Whitespace is
/// normalized on both sides (models reflow quotes across lines), so the match
/// tolerates reformatting but not fabrication. Empty result = every cited quote
/// is grounded (or none were cited). No model call.
pub fn unverified_citations(response: &str, tool_outputs: &[String]) -> Vec<String> {
	let quotes = extract_quotes(response);
	if quotes.is_empty() {
		return Vec::new();
	}
	let haystack = tool_outputs
		.iter()
		.map(|o| normalize_ws(o))
		.collect::<Vec<_>>()
		.join("\n");
	quotes
		.into_iter()
		.filter(|q| {
			let n = normalize_ws(q);
			!n.is_empty() && !haystack.contains(&n)
		})
		.collect()
}

/// Extract the text inside each `«…»` pair (the evidence-tag quote delimiters).
fn extract_quotes(text: &str) -> Vec<String> {
	const OPEN: char = '«';
	const CLOSE: char = '»';
	let mut out = Vec::new();
	let mut rest = text;
	while let Some(s) = rest.find(OPEN) {
		let after = &rest[s + OPEN.len_utf8()..];
		match after.find(CLOSE) {
			Some(e) => {
				let q = after[..e].trim();
				if !q.is_empty() {
					out.push(q.to_string());
				}
				rest = &after[e + CLOSE.len_utf8()..];
			}
			None => break,
		}
	}
	out
}

/// Collapse every run of whitespace to a single space and trim — the normal form
/// both sides of the evidence check are compared in.
fn normalize_ws(s: &str) -> String {
	s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Heuristic: does this tool change state, so a success is inherently progress?
/// (Reads/searches only count as progress when they surface *new* content.)
pub fn is_mutation_tool(tool: &str) -> bool {
	let t = tool.to_ascii_lowercase();
	[
		"write",
		"edit",
		"create",
		"str_replace",
		"apply",
		"insert",
		"delete",
		"remove",
		"patch",
		"mkdir",
		"rename",
		"move",
	]
	.iter()
	.any(|k| t.contains(k))
}

const SEEN_CAP: usize = 128;

/// Calls used to seed the working-set centroid before drift scoring begins —
/// too few and the first off-task call would define the baseline.
const CENTROID_WARMUP: usize = 3;
/// EMA weight for folding a new on-task call into the centroid. Higher tracks
/// topic shifts faster but is noisier; lower is stabler but lags.
const CENTROID_ALPHA: f32 = 0.3;

/// Deterministic per-session detector state, built on a single novelty primitive.
#[derive(Debug, Default)]
pub struct Detectors {
	/// Recent result hashes (loop detection), newest at back.
	loop_window: VecDeque<u64>,
	/// Recent novelty flags (no-progress detection), newest at back.
	novelty_window: VecDeque<bool>,
	/// Result hashes seen recently — for novelty. Bounded by `SEEN_CAP`.
	seen: HashSet<u64>,
	seen_order: VecDeque<u64>,
	/// Truncated tool results in a row. Reset by any non-truncated result.
	consecutive_truncations: usize,
	/// Deduplicated results in a row. Reset by any non-dedup result.
	consecutive_dedups: usize,
	/// Off-task results in a row (drift). Reset by an on-task result or new task.
	consecutive_drift: usize,
	/// EMA centroid of recent ON-TASK result embeddings — the "working set". A
	/// result far from it (low cosine) is drift. Empty until the first one seeds it.
	centroid: Vec<f32>,
	/// Calls folded into `centroid` since the last reset (cold-start gate + EMA).
	centroid_count: usize,
	/// Hash of the user task the `centroid` belongs to; a change resets it so the
	/// working set never carries across turns (see [`Detectors::note_task`]).
	task_hash: Option<u64>,
	/// A code change was made with no successful non-mutation check since — the
	/// free pre-gate signal for a premature `done`. Trajectory state, NOT a
	/// streak: it persists across turns until a clean check clears it, so
	/// [`Detectors::reset_streak`] deliberately leaves it untouched.
	unverified_mutation: bool,
}

/// What the deterministic layer concluded for an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectorSignal {
	/// Nothing notable.
	None,
	/// The same result repeated `loop_threshold` times — even across reworded
	/// args (keyed on result, so near-duplicate calls are caught too).
	Loop,
	/// `no_progress_window` actions elapsed with zero new information.
	NoProgress,
	/// `truncation_threshold` truncated results in a row — the model is ignoring
	/// the truncation notice and re-querying without narrowing. Tool-agnostic:
	/// the model varies args so each truncated chunk differs (defeating Loop) and
	/// reads as fresh (defeating NoProgress); the only invariant is truncation.
	Truncation,
	/// `dedup_threshold` deduplicated results in a row — the model is re-issuing
	/// calls whose output it already received this session (the body was elided to
	/// an error placeholder). The most precise loop there is: the exact same call.
	Dedup,
	/// `distraction_threshold` off-task RESULTS in a row — results whose cosine to
	/// the working-set centroid (recent on-task results) fell below `drift_floor`.
	/// Self-referential, so no task anchor is needed (robust to abstract requests).
	/// We score the result, not the call: short call strings are format-dominated
	/// and don't separate by topic (measured); results carry real content and do.
	Distraction,
}

fn hash2(a: &str, b: &str) -> u64 {
	let mut h = DefaultHasher::new();
	a.hash(&mut h);
	b.hash(&mut h);
	h.finish()
}

impl Detectors {
	/// Record one tool action and return the deterministic signal. Novelty is
	/// computed internally: a mutation always advances state; a read/search only
	/// advances when its (non-error) result is one we have not seen recently.
	#[allow(clippy::too_many_arguments)]
	pub fn record_action(
		&mut self,
		tool: &str,
		result: &str,
		is_error: bool,
		is_mutation: bool,
		is_truncated: bool,
		is_dedup: bool,
		is_drift: bool,
		loop_threshold: usize,
		no_progress_window: usize,
		truncation_threshold: usize,
		dedup_threshold: usize,
		distraction_threshold: usize,
	) -> DetectorSignal {
		// Truncation streak: count truncated results in a row, reset on a clean
		// one (the model narrowed and got a full result). Checked first because it
		// is the most specific, most actionable diagnosis — the model keeps hitting
		// a capped output by tweaking args, which slips past Loop and NoProgress.
		if is_truncated {
			self.consecutive_truncations += 1;
		} else {
			self.consecutive_truncations = 0;
		}

		// Dedup streak: count deduplicated results in a row, reset on any
		// non-dedup result. A dedup is the model re-issuing a call whose output it
		// already has — the most precise loop there is, so it is diagnosed
		// separately and fires on its own (low) threshold.
		if is_dedup {
			self.consecutive_dedups += 1;
		} else {
			self.consecutive_dedups = 0;
		}

		// Drift streak: count off-task results in a row, reset on any on-task one.
		// Drift is scored upstream against the working-set centroid (async embedding,
		// see `note_result`); here we only count the boolean. The softest signal —
		// context quality, not an active loop — so it is ranked last and stays silent
		// while the agent is exploring.
		if is_drift {
			self.consecutive_drift += 1;
		} else {
			self.consecutive_drift = 0;
		}

		// Mutation-verification tracking for the free pre-gate (premature `done`):
		// a successful change sets the flag; a later successful non-mutation action
		// (a check / build / test / read) clears it. Errors never clear it.
		// Conservative by design — clearing on any clean non-mutation action
		// under-fires rather than over-fires, so a false positive costs at most one
		// extra turn. Tool-agnostic: we never name which tool verifies.
		if !is_error {
			self.unverified_mutation = is_mutation;
		}

		// Identity of this action's RESULT, keyed on tool+result so the same
		// output from differently-worded calls still reads as a repeat.
		let rhash = hash2(tool, result);

		// Novelty: fresh = result content not seen in the recent window.
		let fresh = self.seen.insert(rhash);
		if fresh {
			self.seen_order.push_back(rhash);
			if self.seen_order.len() > SEEN_CAP {
				if let Some(old) = self.seen_order.pop_front() {
					self.seen.remove(&old);
				}
			}
		}
		let novel = is_mutation || (!is_error && fresh);

		// Loop window: identical result repeated.
		self.loop_window.push_back(rhash);
		while self.loop_window.len() > loop_threshold.max(1) {
			self.loop_window.pop_front();
		}
		let looping = loop_threshold > 0
			&& self.loop_window.len() >= loop_threshold
			&& self.loop_window.iter().all(|&h| h == rhash);

		// Novelty window: actions without any new information.
		self.novelty_window.push_back(novel);
		while self.novelty_window.len() > no_progress_window.max(1) {
			self.novelty_window.pop_front();
		}
		let stalled = no_progress_window > 0
			&& self.novelty_window.len() >= no_progress_window
			&& self.novelty_window.iter().all(|&n| !n);

		let truncating =
			truncation_threshold > 0 && self.consecutive_truncations >= truncation_threshold;
		let deduping = dedup_threshold > 0 && self.consecutive_dedups >= dedup_threshold;
		let distracting =
			distraction_threshold > 0 && self.consecutive_drift >= distraction_threshold;

		if deduping {
			DetectorSignal::Dedup
		} else if truncating {
			DetectorSignal::Truncation
		} else if looping {
			DetectorSignal::Loop
		} else if stalled {
			DetectorSignal::NoProgress
		} else if distracting {
			DetectorSignal::Distraction
		} else {
			DetectorSignal::None
		}
	}

	/// Reset the rolling windows (e.g. after a steer note or new user turn).
	/// `unverified_mutation` is intentionally NOT reset — it is trajectory state
	/// that only a successful check clears, not a per-streak counter.
	pub fn reset_streak(&mut self) {
		self.novelty_window.clear();
		self.loop_window.clear();
		self.consecutive_truncations = 0;
		self.consecutive_dedups = 0;
		self.consecutive_drift = 0;
	}

	/// Update the working-set centroid with this result's embedding and return
	/// whether the result drifted off it (cosine below `floor`). Self-referential:
	/// it scores the result against what the agent has recently worked with, so it
	/// needs no task anchor and is robust to abstract requests. Only NON-drift
	/// results are folded in, so a sustained wander can't pull the centroid with it;
	/// the first `CENTROID_WARMUP` results seed it and never count as drift.
	pub fn note_result(&mut self, emb: &[f32], floor: f32) -> bool {
		if self.centroid.len() != emb.len() {
			// First result since a reset (or a dimension change): seed, never drift.
			self.centroid = emb.to_vec();
			self.centroid_count = 1;
			return false;
		}
		let sim = crate::embeddings::cosine(&self.centroid, emb);
		let warming = self.centroid_count < CENTROID_WARMUP;
		let is_drift = !warming && sim < floor;
		if !is_drift {
			for (c, &e) in self.centroid.iter_mut().zip(emb) {
				*c = (1.0 - CENTROID_ALPHA) * *c + CENTROID_ALPHA * e;
			}
			self.centroid_count = self.centroid_count.saturating_add(1);
		}
		is_drift
	}

	/// Reset the working-set centroid when the user's task changes (a new turn):
	/// the centroid is the CURRENT task's calls and must not carry across. `h` is a
	/// cheap hash of the latest real user message — NOT an embedding, so this never
	/// hits the abstract-request problem the scoring side avoids by construction.
	pub fn note_task(&mut self, h: u64) {
		if self.task_hash != Some(h) {
			self.task_hash = Some(h);
			self.reset_working_set();
		}
	}

	/// Drop the working set so the next calls re-seed it. Called on a task change
	/// and after a Distraction steer — a legit pivot then re-seeds instead of being
	/// flagged forever (drift results are never folded into the stale centroid).
	pub fn reset_working_set(&mut self) {
		self.centroid.clear();
		self.centroid_count = 0;
		self.consecutive_drift = 0;
	}

	/// Free pre-gate signal: code was changed and no successful check has run
	/// since. See [`Detectors::unverified_mutation`].
	pub fn needs_verification(&self) -> bool {
		self.unverified_mutation
	}
}

/// Fuse the deterministic signal with the agent's free self-report (no model
/// call). The decision table:
/// - any `done`                          → defer to the verify-gate (no steer)
/// - no-progress / distraction while `exploring` → wait (legitimate exploration)
/// - dedup, truncation, loop, no-progress, distraction → steer
///
/// Truncation steers even while `exploring`: re-hitting a capped output is waste
/// regardless of intent, unlike a no-progress window which can be legitimate
/// exploration.
pub fn should_steer(signal: DetectorSignal, report: Option<SelfReport>) -> bool {
	if signal == DetectorSignal::None {
		return false;
	}
	match report {
		Some(SelfReport::Done) => false,
		// No-progress and distraction can be legitimate while exploring; every
		// other signal steers regardless of intent.
		Some(SelfReport::Exploring)
			if matches!(
				signal,
				DetectorSignal::NoProgress | DetectorSignal::Distraction
			) =>
		{
			false
		}
		_ => true,
	}
}

/// Short human description of a fired signal — for the user-facing
/// `· Supervisor: steering — …` notice.
pub fn signal_description(signal: DetectorSignal) -> &'static str {
	match signal {
		DetectorSignal::Loop => "repeated action without new results",
		DetectorSignal::NoProgress => "no new information in recent steps",
		DetectorSignal::Truncation => "repeated truncated results — narrowing args ignored",
		DetectorSignal::Dedup => "repeated identical results — re-fetching output already received",
		DetectorSignal::Distraction => "off-task results — drifting from the current line of work",
		DetectorSignal::None => "",
	}
}

/// The advisory steer note for a fired signal. Out-of-band; the `<supervisor>`
/// framing keeps it distinct from user content.
pub fn steer_note(signal: DetectorSignal) -> &'static str {
	match signal {
		DetectorSignal::Loop => "<supervisor>\nYou have repeated the same action without new results. Stop and try a different approach. If you cannot proceed, report `blocked`.\n</supervisor>",
		DetectorSignal::NoProgress => "<supervisor>\nSeveral steps have passed without new progress. Re-anchor on the user's actual request: restate the goal, what is done, and the next concrete step — or report `blocked`.\n</supervisor>",
		DetectorSignal::Truncation => "<supervisor>\nYour recent tool results were truncated — the output is capped, so re-running the same kind of broad call returns no more, just more wasted context.\nWork efficiently: read only what you actually need, not whole files or full listings. Each call costs context and attention, so spending it on output you won't use makes the rest of the task harder. Before each call, decide the smallest slice that answers your current question and target it with the tool's own parameters — a line range, limit, offset, filter, or a more specific query/pattern. Read broadly only when you genuinely need most of the content.\n</supervisor>",
		DetectorSignal::Dedup => "<supervisor>\nSTOP — you are repeating the same call(s) and getting the same output you already have. This is a loop; it adds nothing and wastes context. Do NOT repeat them. Act on the results already in context, or change the args/tool to get something new. If you truly cannot proceed, report `blocked`.\n</supervisor>",
		DetectorSignal::Distraction => "<supervisor>\nYour recent work has drifted away from the line you were pursuing — you are pulling in content unrelated to what you have been doing, which crowds out what matters. Refocus on the current goal: make your next calls target exactly what it needs (the specific files, symbols, or behavior involved). If you have deliberately moved on to a new sub-task, ignore this.\n</supervisor>",
		DetectorSignal::None => "",
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn parses_state_only() {
		assert_eq!(
			parse_self_report("work\n<sup>done</sup>"),
			Some((SelfReport::Done, None))
		);
	}

	#[test]
	fn parses_state_with_reason() {
		let r = parse_self_report("x <sup>progressing · editing api</sup> y");
		assert_eq!(
			r,
			Some((SelfReport::Progressing, Some("editing api".into())))
		);
	}

	#[test]
	fn need_input_variants() {
		assert_eq!(
			parse_self_report("<sup>need_input</sup>").map(|(s, _)| s),
			Some(SelfReport::NeedInput)
		);
	}

	#[test]
	fn strips_token_and_trailing_blank() {
		assert_eq!(strip_self_report("answer\n\n<sup>done</sup>"), "answer");
	}

	#[test]
	fn strips_and_parses_real_multiword_reason() {
		let s = "answer\n<sup>need_input · Phase 1 complete, awaiting user direction</sup>";
		assert_eq!(strip_self_report(s), "answer");
		let (st, reason) = parse_self_report(s).unwrap();
		assert_eq!(st, SelfReport::NeedInput);
		assert_eq!(
			reason.as_deref(),
			Some("Phase 1 complete, awaiting user direction")
		);
	}

	#[test]
	fn handles_non_dot_separators() {
		assert_eq!(strip_self_report("x <sup>done: all good</sup>"), "x");
		assert_eq!(
			strip_self_report("x <sup>blocked - cannot proceed</sup>"),
			"x"
		);
		assert_eq!(
			parse_self_report("<sup>done: all good</sup>").map(|(s, _)| s),
			Some(SelfReport::Done)
		);
	}

	#[test]
	fn keeps_legitimate_superscript() {
		// `<sup>2</sup>` (x squared) is not a state token — keep it verbatim.
		assert_eq!(strip_self_report("x<sup>2</sup> + 1"), "x<sup>2</sup> + 1");
		// A short non-state superscript with no separator stays too.
		assert_eq!(
			strip_self_report("the 5<sup>th</sup>"),
			"the 5<sup>th</sup>"
		);
	}

	#[test]
	fn strips_echoed_state_placeholder() {
		// The reported leak: a model copies the literal `STATE` placeholder.
		// It must never reach the screen, and we recover the intended state.
		assert_eq!(
			strip_self_report("answer\n<sup>STATE · done</sup>"),
			"answer"
		);
		assert_eq!(
			parse_self_report("answer\n<sup>STATE · done</sup>"),
			Some((SelfReport::Done, None))
		);
		// Bare echoed placeholder (no reason) is stripped as well.
		assert_eq!(strip_self_report("answer <sup>STATE</sup>"), "answer");
	}

	#[test]
	fn strips_report_with_unknown_lead_but_separator() {
		// Even a malformed state word can't leak once the `·` separator is present.
		assert_eq!(
			strip_self_report("ok\n<sup>finished · all good</sup>"),
			"ok"
		);
	}

	#[test]
	fn loop_fires_on_repeated_result() {
		let mut d = Detectors::default();
		assert_eq!(
			d.record_action("grep", "same", false, false, false, false, false, 3, 9, 0, 0, 0),
			DetectorSignal::None
		);
		assert_eq!(
			d.record_action("grep", "same", false, false, false, false, false, 3, 9, 0, 0, 0),
			DetectorSignal::None
		);
		// Third identical RESULT → loop.
		assert_eq!(
			d.record_action("grep", "same", false, false, false, false, false, 3, 9, 0, 0, 0),
			DetectorSignal::Loop
		);
	}

	#[test]
	fn no_progress_fires_on_zero_novelty_window() {
		let mut d = Detectors::default();
		d.record_action("a", "r", false, false, false, false, false, 9, 3, 0, 0, 0); // first "r" → novel
		d.record_action("a", "r", false, false, false, false, false, 9, 3, 0, 0, 0); // seen → not novel
		d.record_action("a", "r", false, false, false, false, false, 9, 3, 0, 0, 0); // not novel
		assert_eq!(
			d.record_action("a", "r", false, false, false, false, false, 9, 3, 0, 0, 0),
			DetectorSignal::NoProgress
		);
	}

	#[test]
	fn mutation_counts_as_progress() {
		let mut d = Detectors::default();
		d.record_action(
			"read", "same", false, false, false, false, false, 9, 2, 0, 0, 0,
		);
		d.record_action(
			"read", "same", false, false, false, false, false, 9, 2, 0, 0, 0,
		);
		// An edit always advances state → breaks the stall.
		assert_eq!(
			d.record_action("edit", "ok", false, true, false, false, false, 9, 2, 0, 0, 0),
			DetectorSignal::None
		);
	}

	#[test]
	fn truncation_fires_on_consecutive_truncated_results() {
		let mut d = Detectors::default();
		// Different content each time (model tweaks args) — defeats Loop/NoProgress,
		// but both carry the truncation flag. Threshold 2 → fires on the second.
		assert_eq!(
			d.record_action("view", "chunk A", false, false, true, false, false, 9, 9, 2, 0, 0),
			DetectorSignal::None
		);
		assert_eq!(
			d.record_action("view", "chunk B", false, false, true, false, false, 9, 9, 2, 0, 0),
			DetectorSignal::Truncation
		);
	}

	#[test]
	fn clean_result_resets_truncation_streak() {
		let mut d = Detectors::default();
		d.record_action(
			"view", "chunk A", false, false, true, false, false, 9, 9, 2, 0, 0,
		);
		// Model narrowed and got a full result → streak resets.
		assert_eq!(
			d.record_action("view", "full", false, false, false, false, false, 9, 9, 2, 0, 0),
			DetectorSignal::None
		);
		// One truncation again is not yet at threshold.
		assert_eq!(
			d.record_action("view", "chunk B", false, false, true, false, false, 9, 9, 2, 0, 0),
			DetectorSignal::None
		);
	}

	#[test]
	fn truncation_outranks_loop() {
		let mut d = Detectors::default();
		// Identical truncated result repeated: both Loop and Truncation conditions
		// hold; the more actionable Truncation signal wins.
		d.record_action(
			"view", "same", false, false, true, false, false, 2, 9, 2, 0, 0,
		);
		assert_eq!(
			d.record_action("view", "same", false, false, true, false, false, 2, 9, 2, 0, 0),
			DetectorSignal::Truncation
		);
	}

	#[test]
	fn needs_verification_after_mutation_until_clean_check() {
		let mut d = Detectors::default();
		assert!(!d.needs_verification());
		// A successful edit → unverified.
		d.record_action(
			"edit", "ok", false, true, false, false, false, 9, 9, 0, 0, 0,
		);
		assert!(d.needs_verification());
		// A failed check does NOT clear it.
		d.record_action(
			"shell", "error", true, false, false, false, false, 9, 9, 0, 0, 0,
		);
		assert!(d.needs_verification());
		// A successful non-mutation check clears it.
		d.record_action(
			"shell",
			"tests pass",
			false,
			false,
			false,
			false,
			false,
			9,
			9,
			0,
			0,
			0,
		);
		assert!(!d.needs_verification());
	}

	#[test]
	fn reset_streak_keeps_unverified_mutation() {
		let mut d = Detectors::default();
		d.record_action(
			"edit", "ok", false, true, false, false, false, 9, 9, 0, 0, 0,
		);
		assert!(d.needs_verification());
		// reset_streak is for the rolling windows — trajectory state survives.
		d.reset_streak();
		assert!(d.needs_verification());
	}

	#[test]
	fn evidence_grounded_quote_passes() {
		let outputs = vec!["274:\t\tif (!in_array($deal_data['status'], ...))".to_string()];
		// Quote is a contiguous (whitespace-normalized) substring of the output.
		let resp =
			"The guard is here [evidence: PayoutTaskService.php «in_array($deal_data['status']»].";
		assert!(unverified_citations(resp, &outputs).is_empty());
	}

	#[test]
	fn evidence_fabricated_quote_flagged() {
		let outputs = vec!["fn record_action(&mut self) -> DetectorSignal".to_string()];
		let resp = "It does [evidence: detect.rs «fn totally_made_up_symbol()»].";
		let bad = unverified_citations(resp, &outputs);
		assert_eq!(bad, vec!["fn totally_made_up_symbol()"]);
	}

	#[test]
	fn evidence_tolerates_reflowed_whitespace() {
		let outputs = vec!["let signal = detectors.record_action(tool, result);".to_string()];
		// Model reflowed the quote across lines — normalization makes it match.
		let resp = "see [evidence: x «let signal =\n   detectors.record_action(tool, result);»]";
		assert!(unverified_citations(resp, &outputs).is_empty());
	}

	#[test]
	fn no_citations_is_clean() {
		assert!(unverified_citations("plain answer, no tags", &["x".to_string()]).is_empty());
	}

	#[test]
	fn truncation_steers_even_while_exploring() {
		assert!(should_steer(
			DetectorSignal::Truncation,
			Some(SelfReport::Exploring)
		));
		// But still defers to the gate on done.
		assert!(!should_steer(
			DetectorSignal::Truncation,
			Some(SelfReport::Done)
		));
	}

	#[test]
	fn steer_defers_to_gate_on_done() {
		assert!(!should_steer(
			DetectorSignal::NoProgress,
			Some(SelfReport::Done)
		));
		assert!(!should_steer(DetectorSignal::Loop, Some(SelfReport::Done)));
	}

	#[test]
	fn steer_waits_while_exploring_but_fires_on_loop() {
		assert!(!should_steer(
			DetectorSignal::NoProgress,
			Some(SelfReport::Exploring)
		));
		assert!(should_steer(
			DetectorSignal::Loop,
			Some(SelfReport::Exploring)
		));
		assert!(should_steer(
			DetectorSignal::NoProgress,
			Some(SelfReport::Progressing)
		));
	}

	#[test]
	fn dedup_fires_on_consecutive_dedup_results() {
		let mut d = Detectors::default();
		// is_dedup=true, threshold 2 → fires on the second in a row.
		assert_eq!(
			d.record_action("view", "[dup A]", true, false, false, true, false, 9, 9, 0, 2, 0),
			DetectorSignal::None
		);
		assert_eq!(
			d.record_action("view", "[dup B]", true, false, false, true, false, 9, 9, 0, 2, 0),
			DetectorSignal::Dedup
		);
	}

	#[test]
	fn clean_result_resets_dedup_streak() {
		let mut d = Detectors::default();
		d.record_action(
			"view", "[dup A]", true, false, false, true, false, 9, 9, 0, 2, 0,
		);
		// A fresh, non-dedup result breaks the streak.
		assert_eq!(
			d.record_action("view", "full", false, false, false, false, false, 9, 9, 0, 2, 0),
			DetectorSignal::None
		);
		assert_eq!(
			d.record_action("view", "[dup B]", true, false, false, true, false, 9, 9, 0, 2, 0),
			DetectorSignal::None
		);
	}

	#[test]
	fn dedup_outranks_loop() {
		let mut d = Detectors::default();
		// Identical dedup placeholder repeated satisfies both Loop and Dedup;
		// the more precise Dedup signal wins.
		d.record_action(
			"view", "[dup]", true, false, false, true, false, 2, 9, 0, 2, 0,
		);
		assert_eq!(
			d.record_action("view", "[dup]", true, false, false, true, false, 2, 9, 0, 2, 0),
			DetectorSignal::Dedup
		);
	}

	#[test]
	fn dedup_steers_even_while_exploring() {
		assert!(should_steer(
			DetectorSignal::Dedup,
			Some(SelfReport::Exploring)
		));
		assert!(!should_steer(DetectorSignal::Dedup, Some(SelfReport::Done)));
	}

	#[test]
	fn distraction_fires_on_consecutive_drift() {
		let mut d = Detectors::default();
		// is_drift=true, threshold 2 → fires on the second in a row.
		assert_eq!(
			d.record_action("view", "off", false, false, false, false, true, 9, 9, 0, 0, 2),
			DetectorSignal::None
		);
		assert_eq!(
			d.record_action("view", "off2", false, false, false, false, true, 9, 9, 0, 0, 2),
			DetectorSignal::Distraction
		);
	}

	#[test]
	fn on_task_result_resets_distraction_streak() {
		let mut d = Detectors::default();
		d.record_action(
			"view", "off", false, false, false, false, true, 9, 9, 0, 0, 2,
		);
		// An on-task call breaks the streak.
		assert_eq!(
			d.record_action("view", "rel", false, false, false, false, false, 9, 9, 0, 0, 2),
			DetectorSignal::None
		);
		assert_eq!(
			d.record_action("view", "off2", false, false, false, false, true, 9, 9, 0, 0, 2),
			DetectorSignal::None
		);
	}

	#[test]
	fn distraction_waits_while_exploring_but_fires_when_progressing() {
		assert!(!should_steer(
			DetectorSignal::Distraction,
			Some(SelfReport::Exploring)
		));
		assert!(should_steer(
			DetectorSignal::Distraction,
			Some(SelfReport::Progressing)
		));
	}

	#[test]
	fn note_result_seeds_then_flags_drift() {
		let mut d = Detectors::default();
		let a = vec![1.0, 0.0, 0.0];
		let b = vec![0.9, 0.1, 0.0];
		// Warmup seeds the centroid with similar results — never drift.
		assert!(!d.note_result(&a, 0.5));
		assert!(!d.note_result(&b, 0.5));
		assert!(!d.note_result(&b, 0.5));
		// A call orthogonal to the working set is drift; an aligned one is not.
		let off = vec![0.0, 0.0, 1.0];
		assert!(d.note_result(&off, 0.5));
		assert!(!d.note_result(&a, 0.5));
	}

	#[test]
	fn drift_calls_do_not_pull_the_centroid() {
		let mut d = Detectors::default();
		let on = vec![1.0, 0.0, 0.0];
		for _ in 0..3 {
			d.note_result(&on, 0.5);
		}
		// Repeated drift is never folded in, so it keeps reading as drift.
		let off = vec![0.0, 0.0, 1.0];
		assert!(d.note_result(&off, 0.5));
		assert!(d.note_result(&off, 0.5));
		// And an on-task call is still recognised.
		assert!(!d.note_result(&on, 0.5));
	}

	#[test]
	fn note_task_change_resets_working_set() {
		let mut d = Detectors::default();
		let on = vec![1.0, 0.0, 0.0];
		for _ in 0..3 {
			d.note_result(&on, 0.5);
		}
		// New task → working set cleared → next call re-seeds (never drift).
		d.note_task(42);
		let off = vec![0.0, 0.0, 1.0];
		assert!(!d.note_result(&off, 0.5));
	}
}
