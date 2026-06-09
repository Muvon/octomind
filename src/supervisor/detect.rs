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
//! 2. **Counters** — identical tool+args repeats, and turns without new info.
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
/// an optional short reason. Tolerant of the `·` or `-` reason separator.
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

/// Cheap markers of whether a turn actually advanced the task.
#[derive(Debug, Default)]
pub struct ProgressState {
	pub files_seen: HashSet<String>,
	pub last_error_sig: Option<u64>,
	pub edits_applied: usize,
}

/// Deterministic per-session detector state.
#[derive(Debug, Default)]
pub struct Detectors {
	/// Hashes of recent (tool, args) signatures — newest at back.
	window: VecDeque<u64>,
	pub progress: ProgressState,
	/// Consecutive turns observed without new information.
	no_progress_streak: usize,
}

/// What the deterministic layer concluded for a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectorSignal {
	/// Nothing notable.
	None,
	/// Same tool+args repeated `loop_threshold` times — unambiguous loop.
	Loop,
	/// `no_progress_window` turns elapsed with no new information.
	NoProgress,
}

fn sig(tool: &str, args: &str) -> u64 {
	let mut h = DefaultHasher::new();
	tool.hash(&mut h);
	args.hash(&mut h);
	h.finish()
}

impl Detectors {
	/// Record one tool action and return the deterministic signal. `made_progress`
	/// is true when this action surfaced new information (new file, changed error
	/// signature, applied edit) — the caller computes it from the tool result.
	pub fn record_action(
		&mut self,
		tool: &str,
		args: &str,
		made_progress: bool,
		loop_threshold: usize,
		no_progress_window: usize,
	) -> DetectorSignal {
		let s = sig(tool, args);
		self.window.push_back(s);
		while self.window.len() > loop_threshold.max(no_progress_window) {
			self.window.pop_front();
		}

		if made_progress {
			self.no_progress_streak = 0;
		} else {
			self.no_progress_streak += 1;
		}

		// Loop: the last `loop_threshold` signatures are all identical.
		if loop_threshold > 0 && self.window.len() >= loop_threshold {
			let tail = self.window.len() - loop_threshold;
			if self.window.iter().skip(tail).all(|&x| x == s) {
				return DetectorSignal::Loop;
			}
		}

		if no_progress_window > 0 && self.no_progress_streak >= no_progress_window {
			return DetectorSignal::NoProgress;
		}

		DetectorSignal::None
	}

	/// Reset the no-progress streak (e.g. after a steer note or new user turn).
	pub fn reset_streak(&mut self) {
		self.no_progress_streak = 0;
	}
}

/// Decide whether a fired signal warrants a steer note, fusing the deterministic
/// signal with the agent's free self-report (no model call). Loops always steer;
/// no-progress steers unless the agent is legitimately still exploring.
pub fn should_steer(signal: DetectorSignal, report: Option<SelfReport>) -> bool {
	match signal {
		DetectorSignal::Loop => true,
		DetectorSignal::NoProgress => !matches!(report, Some(SelfReport::Exploring)),
		DetectorSignal::None => false,
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
	fn loop_fires_on_third_identical() {
		let mut d = Detectors::default();
		assert_eq!(
			d.record_action("grep", "x", false, 3, 5),
			DetectorSignal::None
		);
		assert_eq!(
			d.record_action("grep", "x", false, 3, 5),
			DetectorSignal::None
		);
		assert_eq!(
			d.record_action("grep", "x", false, 3, 5),
			DetectorSignal::Loop
		);
	}

	#[test]
	fn progress_resets_streak() {
		let mut d = Detectors::default();
		d.record_action("a", "1", false, 9, 3);
		d.record_action("b", "2", true, 9, 3); // progress resets
		assert_eq!(d.record_action("c", "3", false, 9, 3), DetectorSignal::None);
	}
}
