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
At the very end of every response, on its own final line, emit a status tag:
`<sup>STATE</sup>` where STATE is exactly one of: exploring, progressing, blocked, need_input, done.
You may append a short reason: `<sup>STATE · brief reason</sup>`.
- `done` only when the user's task is fully complete.
- `need_input` when you are asking the user a question and waiting on them.
- `blocked` when you are stuck and cannot proceed.
- `exploring` while still gathering context; `progressing` while actively making changes.
This tag is for the system and is hidden from the user. Always include exactly one.
</supervisor>";

/// Parse the *last* `<sup>…</sup>` token from a response. Returns the state and
/// an optional short reason. Tolerant of the `·` or `|` reason separator.
pub fn parse_self_report(text: &str) -> Option<(SelfReport, Option<String>)> {
	let close = "</sup>";
	let open = "<sup>";
	let end = text.rfind(close)?;
	let start = text[..end].rfind(open)? + open.len();
	let inner = text[start..end].trim();
	let (state_part, reason) = match inner.split_once(['·', '|']) {
		Some((s, r)) => (s, Some(r.trim().to_string()).filter(|r| !r.is_empty())),
		None => (inner, None),
	};
	SelfReport::from_token(state_part).map(|s| (s, reason))
}

/// Remove only `<sup>…</sup>` tokens whose body parses as a known state, so
/// legitimate superscript markup the agent might emit is left untouched.
pub fn strip_self_report(text: &str) -> String {
	let mut out = String::with_capacity(text.len());
	let mut rest = text;
	while let Some(start) = rest.find("<sup>") {
		match rest[start..].find("</sup>") {
			Some(rel_end) => {
				let inner = &rest[start + "<sup>".len()..start + rel_end];
				let state_part = inner.split(['·', '|']).next().unwrap_or("").trim();
				if SelfReport::from_token(state_part).is_some() {
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
	pub fn record_action(
		&mut self,
		tool: &str,
		result: &str,
		is_error: bool,
		is_mutation: bool,
		loop_threshold: usize,
		no_progress_window: usize,
	) -> DetectorSignal {
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

		if looping {
			DetectorSignal::Loop
		} else if stalled {
			DetectorSignal::NoProgress
		} else {
			DetectorSignal::None
		}
	}

	/// Reset the rolling windows (e.g. after a steer note or new user turn).
	pub fn reset_streak(&mut self) {
		self.novelty_window.clear();
		self.loop_window.clear();
	}
}

/// Fuse the deterministic signal with the agent's free self-report (no model
/// call). The decision table:
/// - any `done`                          → defer to the verify-gate (no steer)
/// - no-progress while `exploring`       → wait (legitimate exploration)
/// - loop, or no-progress otherwise      → steer
pub fn should_steer(signal: DetectorSignal, report: Option<SelfReport>) -> bool {
	if signal == DetectorSignal::None {
		return false;
	}
	match report {
		Some(SelfReport::Done) => false,
		Some(SelfReport::Exploring) if signal == DetectorSignal::NoProgress => false,
		_ => true,
	}
}

/// The advisory steer note for a fired signal. Out-of-band; the `<supervisor>`
/// framing keeps it distinct from user content.
pub fn steer_note(signal: DetectorSignal) -> &'static str {
	match signal {
		DetectorSignal::Loop => "<supervisor>\nYou have repeated the same action without new results. Stop and try a different approach. If you cannot proceed, report `blocked`.\n</supervisor>",
		DetectorSignal::NoProgress => "<supervisor>\nSeveral steps have passed without new progress. Re-anchor on the user's actual request: restate the goal, what is done, and the next concrete step — or report `blocked`.\n</supervisor>",
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
	fn loop_fires_on_repeated_result() {
		let mut d = Detectors::default();
		assert_eq!(
			d.record_action("grep", "same", false, false, 3, 9),
			DetectorSignal::None
		);
		assert_eq!(
			d.record_action("grep", "same", false, false, 3, 9),
			DetectorSignal::None
		);
		// Third identical RESULT → loop.
		assert_eq!(
			d.record_action("grep", "same", false, false, 3, 9),
			DetectorSignal::Loop
		);
	}

	#[test]
	fn no_progress_fires_on_zero_novelty_window() {
		let mut d = Detectors::default();
		d.record_action("a", "r", false, false, 9, 3); // first "r" → novel
		d.record_action("a", "r", false, false, 9, 3); // seen → not novel
		d.record_action("a", "r", false, false, 9, 3); // not novel
		assert_eq!(
			d.record_action("a", "r", false, false, 9, 3),
			DetectorSignal::NoProgress
		);
	}

	#[test]
	fn mutation_counts_as_progress() {
		let mut d = Detectors::default();
		d.record_action("read", "same", false, false, 9, 2);
		d.record_action("read", "same", false, false, 9, 2);
		// An edit always advances state → breaks the stall.
		assert_eq!(
			d.record_action("edit", "ok", false, true, 9, 2),
			DetectorSignal::None
		);
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
}
