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
<system-reminder>
Finish every response with one status line — the last line, nothing after it:
`<sup>STATE · brief reason</sup>`
Start the tag with one state word, then ` · `, then a few words of reason. Use exactly one of these words (write the word itself, not the placeholder STATE):
- `exploring` — still gathering context, reading code
- `progressing` — actively making changes
- `blocked` — stuck, cannot proceed
- `need_input` — asking the user a question and waiting on them
- `done` — the user's task is fully complete
Examples: `<sup>progressing · wiring store registration</sup>` or `<sup>done · migration verified</sup>`
This line is read by the system and hidden from the user. Emit exactly one, leading with the state word.
</system-reminder>";

/// One-time system-side instruction enabling evidence-bound claims. The agent
/// backs load-bearing factual claims about the codebase with a verbatim quote it
/// actually saw in a tool result; [`unverified_citations`] then deterministically
/// checks the quote really occurs in some tool output, catching fabricated
/// citations for free (no model call).
pub const EVIDENCE_INSTRUCTION: &str = "\
<system-reminder>
Before you assert a load-bearing fact about the code or repo (a path, signature, value, or concrete behavior), copy the supporting text you actually saw in a tool result, character-for-character, between guillemets, in this exact form:
[evidence: <locator> «exact text copied verbatim from the tool output»]
The text inside « » must be a literal copy that string-matches the tool output — rewording, summarizing, or trimming it breaks the match. Tag only load-bearing factual claims about the code, not plans, reasoning, or general knowledge. If you cannot find a line that supports a claim, say so and drop it — do not invent a quote. A « » quote not found in any tool result you received will be flagged to re-ground against real output or retract.
</system-reminder>";

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
	/// Single-tool-call rounds in a row. A round is a full AI turn's tool batch;
	/// when it carries exactly one call and the model could have batched independent
	/// calls, the streak grows. Reset by any multi-call (parallel) round.
	consecutive_singletons: usize,
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
	/// `sequential_threshold` single-tool-call ROUNDS in a row — the model is
	/// issuing one call per turn where independent calls could be batched into one
	/// parallel round. Round-level (not per-call); recorded once per turn via
	/// [`Detectors::record_round_arity`]. Off by default — single calls are often
	/// legitimate, so this is the softest, most conservative signal.
	Sequential,
}

impl DetectorSignal {
	/// Severity rank — higher wins when merging signals from a parallel batch.
	/// Mirrors the priority in `record_action`'s return cascade.
	fn priority(self) -> u8 {
		match self {
			Self::None => 0,
			Self::Sequential => 1,
			Self::Distraction => 2,
			Self::NoProgress => 3,
			Self::Loop => 4,
			Self::Truncation => 5,
			Self::Dedup => 6,
		}
	}

	/// Merge two signals from the same parallel batch — keep the higher-priority one.
	pub fn merge(self, other: Self) -> Self {
		if other.priority() > self.priority() {
			other
		} else {
			self
		}
	}
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

	/// Reset the rolling windows (e.g. on a new user turn).
	/// `unverified_mutation` is intentionally NOT reset — it is trajectory state
	/// that only a successful check clears, not a per-streak counter.
	pub fn reset_streak(&mut self) {
		self.novelty_window.clear();
		self.loop_window.clear();
		self.consecutive_truncations = 0;
		self.consecutive_dedups = 0;
		self.consecutive_drift = 0;
		self.consecutive_singletons = 0;
	}

	/// Record the arity of a completed tool round (one AI turn's batch) and return
	/// the sequential-batching signal. `call_count` is how many tool calls the round
	/// carried; a round of exactly one grows the singleton streak, any parallel round
	/// (>= 2) resets it. Fires `Sequential` once the streak reaches `threshold`.
	/// `threshold == 0` disables the signal entirely (the default). Round-level, so
	/// it is recorded once per turn — separately from per-call [`record_action`].
	pub fn record_round_arity(&mut self, call_count: usize, threshold: usize) -> DetectorSignal {
		if call_count == 1 {
			self.consecutive_singletons += 1;
		} else {
			self.consecutive_singletons = 0;
		}
		if threshold > 0 && self.consecutive_singletons >= threshold {
			DetectorSignal::Sequential
		} else {
			DetectorSignal::None
		}
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
		// No-progress, distraction, and sequential-batching can be legitimate while
		// exploring; every other signal steers regardless of intent.
		Some(SelfReport::Exploring)
			if matches!(
				signal,
				DetectorSignal::NoProgress
					| DetectorSignal::Distraction
					| DetectorSignal::Sequential
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
		DetectorSignal::Sequential => {
			"single tool calls in a row — independent calls could be batched"
		}
		DetectorSignal::None => "",
	}
}

/// Shared persistent-failure frame: the model has been steered through the full
/// 0→1→2 ladder on a *stuck* signal and still has not broken out, so small tweaks
/// are clearly not working. Signal-agnostic and held on clamp. Only `Sequential`
/// (advisory, false-positive-prone) never reaches it.
const PERSISTENT_STEER: &str = "<system-reminder>\nYou have been steered several times here and have not broken out — small adjustments are not working. Stop iterating on the same approach: either take a fundamentally different path to the goal, or report `blocked` and name the single obstacle in your way.\n</system-reminder>";

/// Conflict framing: a no-progress signal while the agent self-reports
/// `progressing`. The counters and the self-assessment disagree — the canonical
/// reason the supervisor escalates at all — so name the contradiction directly
/// instead of the generic no-progress note. Same 0→1→2 escalation.
const CONFLICT_VARIANTS: &[&str] = &[
	"<system-reminder>\nYou reported you are making progress, but the last several actions added nothing new — your self-assessment and what the actions show disagree. Check which is right before continuing.\n</system-reminder>",
	"<system-reminder>\nYou report progressing, yet no new information has appeared. Name in one line the concrete result your recent steps produced. If you cannot, the work has stalled — take a single different step that visibly moves the goal, not another like the ones that yielded nothing.\n</system-reminder>",
	"<system-reminder>\nYour actions are not advancing the task despite a `progressing` report. Re-anchor: state the goal, what is actually done, and the one next step that moves it — then take it. If nothing does, report `blocked` with what is missing.\n</system-reminder>",
];

/// The advisory steer note for a fired signal. Out-of-band; the `<system-reminder>`
/// framing keeps it distinct from user content. Wording is positive-forward (the
/// concrete action to take, not a bare prohibition) and puts that action last, in
/// the recency slot — negation and buried directives are the empirically weakest
/// forms for instruction-following.
///
/// `attempt` rotates the *framing* when the same signal re-fires without the model
/// breaking out. Re-sending identical text loses salience (habituation), so each
/// retry reframes the same constraint from a different angle:
///   0 → diagnostic (what is happening; soft reconsider)
///   1 → directive  (a grounded one-line self-check + the concrete alternative)
///   2 → stop       (firm: a different approach now, or report `blocked`)
///  3+ → persistent ([`PERSISTENT_STEER`]: fundamentally different path or `blocked`)
/// Advance-then-clamp, not modulo: never soften once the model has proven it is
/// stuck — hold the firmest frame. `report` lets a no-progress signal switch to
/// [`CONFLICT_VARIANTS`] when the agent insists it is `progressing`.
pub fn steer_note(
	signal: DetectorSignal,
	report: Option<SelfReport>,
	attempt: usize,
) -> &'static str {
	let stuck = matches!(
		signal,
		DetectorSignal::Loop
			| DetectorSignal::NoProgress
			| DetectorSignal::Truncation
			| DetectorSignal::Dedup
			| DetectorSignal::Distraction
	);
	// Ladder exhausted on a stuck signal without breakout → hold the firmest frame.
	if stuck && attempt >= 3 {
		return PERSISTENT_STEER;
	}
	// Counters say no-progress while the agent reports progressing: name the conflict.
	if signal == DetectorSignal::NoProgress && report == Some(SelfReport::Progressing) {
		return CONFLICT_VARIANTS[attempt.min(CONFLICT_VARIANTS.len() - 1)];
	}
	let variants: &[&str] = match signal {
		DetectorSignal::Loop => &[
			"<system-reminder>\nThis result is identical to one already in your context — the last call added nothing, so the current approach has stalled. Reconsider what is actually blocking progress before the next call.\n</system-reminder>",
			"<system-reminder>\nSame result again — you are repeating a call that already failed to advance the task. In one sentence, name why it failed. Then change one concrete thing on the next call — a different tool, different arguments, or a different sub-goal — that approaches the goal a new way.\n</system-reminder>",
			"<system-reminder>\nThis is a loop: the same call keeps returning the same result. Make a different call that approaches the goal another way — a different tool, scope, or sub-goal — or report `blocked` with the one obstacle stopping you.\n</system-reminder>",
		],
		DetectorSignal::NoProgress => &[
			"<system-reminder>\nThe last few steps surfaced nothing new — this line of inquiry looks exhausted. Consider whether it can still reach what you need.\n</system-reminder>",
			"<system-reminder>\nStill nothing new. Name in one line what you still need but have not found, then take a single concrete step toward the goal using what you already know — a decision or an action, not another exploratory probe.\n</system-reminder>",
			"<system-reminder>\nThis exploration has stalled. Re-anchor on the user's actual request: state the goal in one line, what is done, and the one next step that delivers it — then take it. If no such step exists, report `blocked` with what is missing.\n</system-reminder>",
		],
		DetectorSignal::Truncation => &[
			"<system-reminder>\nYour recent tool results were truncated — the output is capped. Re-running the same broad call returns no new content, only more wasted context.\n</system-reminder>",
			"<system-reminder>\nThe output is capped — broadening the call adds nothing. First, what are you trying to find in it? Then narrow smart, not small — fewer, better-targeted calls:\n  • Prefer a specific tool over raw reads: signatures, structural search, semantic search, or grep.\n  • Need several parts? Request them in one parallel batch, not one chunk per turn.\n  • Need one part? Target it with the tool's parameters (line range, limit, offset, filter, query/pattern).\n</system-reminder>",
			"<system-reminder>\nThese broad calls keep truncating and will not return more. Switch now to a specific tool (signatures, structural/semantic search, grep) or target the exact span with parameters (line range, limit, offset, filter). If you cannot, report `blocked`.\n</system-reminder>",
		],
		DetectorSignal::Dedup => &[
			"<system-reminder>\nThese call(s) returned output you already received this session — the body was elided as a duplicate, so you already have it in context.\n</system-reminder>",
			"<system-reminder>\nThese calls keep returning output you already hold — re-fetching adds no new information. Ask yourself what you are still missing, then act on the result already in context, or change the tool or arguments to get something genuinely new.\n</system-reminder>",
			"<system-reminder>\nThis is a loop: the same call(s), the same output you already hold, no new information. Act on what is already in context, or switch to a different tool or arguments that returns something new. If neither moves the task, report `blocked`.\n</system-reminder>",
		],
		DetectorSignal::Distraction => &[
			"<system-reminder>\nYour recent results have drifted off the work you were pursuing — they no longer serve the current goal.\n</system-reminder>",
			"<system-reminder>\nPull back to the goal. In one line: what does the task actually need, and do your recent calls serve it? If not, make your next calls target exactly that — the specific files, symbols, or behavior the goal involves. If you deliberately moved on to a new sub-task, ignore this.\n</system-reminder>",
			"<system-reminder>\nRe-anchor now: state the goal in one line and the single next step it needs, then make your next calls hit exactly that — and nothing unrelated. If you cannot tie the next step to the goal, report `blocked`. If you deliberately moved on to a new sub-task, ignore this.\n</system-reminder>",
		],
		DetectorSignal::Sequential => &[
			"<system-reminder>\nYou have made several single-call turns in a row. For maximum efficiency, when your next operations are independent (none needs another's result), invoke them all in one parallel batch rather than one per turn — e.g. reading 3 files is 3 calls in one batch. It is faster and uses less context.\n</system-reminder>",
			"<system-reminder>\nYou keep issuing one tool call per turn. Name the calls you need next, then send every one that does not depend on a prior result together in a single parallel batch — three independent reads go out as three calls at once. Only chain calls whose arguments genuinely depend on an earlier result.\n</system-reminder>",
			"<system-reminder>\nStill one call per turn — stop serializing independent work. Name your next 2+ calls and send every independent one in a single parallel batch this turn. If each call truly depends on the previous result, serial is correct — keep it.\n</system-reminder>",
		],
		DetectorSignal::None => return "",
	};
	variants[attempt.min(variants.len() - 1)]
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

	#[test]
	fn conflict_framing_when_progressing_but_no_progress() {
		// No-progress signal while the agent insists it is progressing → conflict text.
		let conflict = steer_note(DetectorSignal::NoProgress, Some(SelfReport::Progressing), 0);
		assert!(conflict.contains("disagree"));
		// Without the progressing claim it stays the generic no-progress note.
		let generic = steer_note(DetectorSignal::NoProgress, None, 0);
		assert!(!generic.contains("disagree"));
	}

	#[test]
	fn persistent_frame_clamps_stuck_signals_past_the_ladder() {
		// A stuck signal re-firing past the 0→1→2 ladder holds the persistent frame.
		assert!(steer_note(DetectorSignal::Loop, None, 5).contains("not working"));
		// Advisory Sequential never escalates to the persistent frame.
		assert!(!steer_note(DetectorSignal::Sequential, None, 5).contains("not working"));
	}
}
