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

//! Human time and energy of agent work, estimated from session turns.
//!
//! Time is what the work occupied a person — the timesheet number. Energy is
//! in deep-hour equivalents (DHE, 1 = one hour of pre-AI deep coding) against a
//! daily budget: reviewing agent output drains more per hour than writing a
//! spec, waiting on an agent far less. Attention is single-threaded, so active
//! intervals that overlap across sessions split their time, and a run earns
//! wait credit only while the human's last input to it is recent. Model,
//! references and calibration: doc/usage/05-sessions.md ("Human Time and Energy").

use crate::config::TimingConfig;
use serde::{Deserialize, Serialize};

/// Prose reading speed, words/min (Brysbaert 2019, non-fiction silent reading).
const READ_WORDS_PER_MIN: f64 = 238.0;
/// Code review speed, lines/min — ~400 lines/h (Cisco / SmartBear).
const REVIEW_LINES_PER_MIN: f64 = 6.7;
/// Typing speed, words/min (Dhakal et al. 2018).
const TYPE_WORDS_PER_MIN: f64 = 52.0;
/// Shortest gap between active intervals that counts as a break, minutes
/// (Microsoft Human Factors Lab 2021).
pub const BREAK_MIN: f64 = 10.0;
/// Longest deep block before review quality drops, minutes (Cisco / SmartBear).
const MAX_BLOCK_MIN: f64 = 90.0;
/// Daily energy budget, DHE (Ericsson et al. 1993: ~4 deliberate hours a day).
pub const DAILY_BUDGET_DHE: f64 = 4.0;
/// Energy weight of spec and dialog turns — the unit the other weights scale.
const OTHER_WEIGHT: f64 = 1.0;
/// Review faster than this finds few defects, lines/hour (Cisco / SmartBear).
pub const MAX_REVIEW_LINES_PER_HOUR: f64 = 500.0;
/// A rubber-stamp needs a diff big enough to matter …
const RUBBER_STAMP_MIN_LINES: usize = 50;
/// … approved in less than this share of the time it takes to read.
const RUBBER_STAMP_READ_SHARE: f64 = 0.3;
const SECS_PER_MIN: f64 = 60.0;
pub const MIN_PER_HOUR: f64 = 60.0;

/// One human input and the agent run it started.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Turn {
	/// Unix seconds the input was submitted.
	pub input_at: u64,
	/// Unix seconds of the run's last agent activity (`input_at` when none).
	pub done_at: u64,
	pub words_in: usize,
	/// Words of assistant text shown to the human during the run.
	pub words_out: usize,
	/// Lines the agent's own edits wrote or removed during the run.
	pub diff_lines: usize,
}

/// One session's turns, in order.
#[derive(Debug, Clone)]
pub struct SessionTurns {
	pub name: String,
	/// Unix seconds the session opened; bounds the attention before its first
	/// input the way the previous run's end bounds every later one.
	pub opened_at: Option<u64>,
	pub turns: Vec<Turn>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct TurnTiming {
	/// Attention before the input — reading the previous output, deciding,
	/// typing. The last turn also carries the closing review of its output.
	pub active_min: f64,
	/// Attended waiting on this turn's run.
	pub wait_min: f64,
	pub energy_dhe: f64,
	/// A big diff approved faster than it could have been read.
	pub rubber_stamp: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SessionTiming {
	pub turns: Vec<TurnTiming>,
	pub active_min: f64,
	pub wait_min: f64,
	pub time_min: f64,
	pub energy_dhe: f64,
	/// Lines reviewed per hour of review time above what review can catch.
	pub fast_review: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Timing {
	pub sessions: Vec<SessionTiming>,
	pub time_min: f64,
	/// Session energies plus the cost of switching between sessions.
	pub energy_dhe: f64,
	pub budget_dhe: f64,
	pub switches: usize,
	/// Longest chain of active intervals without a break.
	pub longest_block_min: f64,
	pub long_block: bool,
	pub over_budget: bool,
}

/// An active interval (human attention) or an attended run (waiting).
struct Interval {
	session: usize,
	turn: usize,
	start: f64,
	end: f64,
	weight: f64,
	/// Diff lines read, when reading code dominates the interval.
	review_lines: Option<usize>,
}

impl Interval {
	fn covers(&self, t: f64) -> bool {
		self.start <= t && t < self.end
	}
}

fn minutes(unix_secs: u64) -> f64 {
	unix_secs as f64 / SECS_PER_MIN
}

fn review_min(lines: usize) -> f64 {
	lines as f64 / REVIEW_LINES_PER_MIN
}

/// Time and energy for a set of sessions whose turns may overlap in time.
pub fn compute(sessions: &[SessionTurns], config: &TimingConfig) -> Timing {
	let mut active: Vec<Interval> = Vec::new();
	let mut attended: Vec<Interval> = Vec::new();
	let mut result: Vec<SessionTiming> = sessions
		.iter()
		.map(|session| SessionTiming {
			turns: vec![TurnTiming::default(); session.turns.len()],
			..Default::default()
		})
		.collect();
	let mut review_lines = vec![0usize; sessions.len()];

	for (s, session) in sessions.iter().enumerate() {
		let turns = &session.turns;
		for (i, turn) in turns.iter().enumerate() {
			let prev = i.checked_sub(1).map(|p| &turns[p]);
			let (prev_words, prev_lines) = prev.map_or((0, 0), |p| (p.words_out, p.diff_lines));
			let estimate = prev_words as f64 / READ_WORDS_PER_MIN
				+ review_min(prev_lines)
				+ turn.words_in as f64 / TYPE_WORDS_PER_MIN
				+ config.think_overhead_min;
			let cap = config.deliberation_factor * estimate;
			// A long gap means the human was away or elsewhere, so it is capped;
			// a short one (pasted input) stays short.
			let since = prev.map(|p| p.done_at).or(session.opened_at);
			let gap = since.map(|since| (minutes(turn.input_at) - minutes(since)).max(0.0));
			let length = gap.map_or(cap, |gap| gap.min(cap));
			let review = review_min(prev_lines) >= config.review_share * estimate;
			result[s].turns[i].rubber_stamp = review
				&& prev_lines >= RUBBER_STAMP_MIN_LINES
				&& gap.is_some_and(|gap| gap < RUBBER_STAMP_READ_SHARE * review_min(prev_lines));
			if review {
				review_lines[s] += prev_lines;
			}
			let input_at = minutes(turn.input_at);
			active.push(Interval {
				session: s,
				turn: i,
				start: input_at - length,
				end: input_at,
				weight: if review {
					config.review_weight
				} else {
					OTHER_WEIGHT
				},
				review_lines: review.then_some(prev_lines),
			});
			attended.push(Interval {
				session: s,
				turn: i,
				start: input_at,
				end: minutes(turn.done_at).min(input_at + config.attention_window_min),
				weight: config.wait_weight,
				review_lines: None,
			});
		}
		if let Some(last) = turns.last() {
			// The human reads the last run's output after it finishes.
			let estimate = last.words_out as f64 / READ_WORDS_PER_MIN
				+ review_min(last.diff_lines)
				+ config.think_overhead_min;
			let review = review_min(last.diff_lines) >= config.review_share * estimate;
			if review {
				review_lines[s] += last.diff_lines;
			}
			let done_at = minutes(last.done_at);
			active.push(Interval {
				session: s,
				turn: turns.len() - 1,
				start: done_at,
				end: done_at + estimate,
				weight: if review {
					config.review_weight
				} else {
					OTHER_WEIGHT
				},
				review_lines: review.then_some(last.diff_lines),
			});
		}
	}

	let mut cuts: Vec<f64> = active
		.iter()
		.chain(&attended)
		.flat_map(|interval| [interval.start, interval.end])
		.collect();
	cuts.sort_by(f64::total_cmp);
	cuts.dedup();
	let mut review_minutes = vec![0.0; sessions.len()];
	// ponytail: O(segments × intervals); a sweep line if a day reaches thousands of turns.
	for window in cuts.windows(2) {
		let length = window[1] - window[0];
		let mid = (window[0] + window[1]) / 2.0;
		let covering: Vec<&Interval> = active.iter().filter(|a| a.covers(mid)).collect();
		let (covering, is_active) = if covering.is_empty() {
			(attended.iter().filter(|a| a.covers(mid)).collect(), false)
		} else {
			(covering, true)
		};
		let share = length / covering.len() as f64;
		for interval in covering {
			let turn = &mut result[interval.session].turns[interval.turn];
			if is_active {
				turn.active_min += share;
			} else {
				turn.wait_min += share;
			}
			turn.energy_dhe += share * interval.weight / MIN_PER_HOUR;
			if interval.review_lines.is_some() {
				review_minutes[interval.session] += share;
			}
		}
	}

	for (s, session) in result.iter_mut().enumerate() {
		session.active_min = session.turns.iter().map(|t| t.active_min).sum();
		session.wait_min = session.turns.iter().map(|t| t.wait_min).sum();
		session.time_min = session.active_min + session.wait_min;
		session.energy_dhe = session.turns.iter().map(|t| t.energy_dhe).sum();
		session.fast_review = review_minutes[s] > 0.0
			&& review_lines[s] as f64 / (review_minutes[s] / MIN_PER_HOUR)
				> MAX_REVIEW_LINES_PER_HOUR;
	}

	active.retain(|a| a.end > a.start);
	active.sort_by(|a, b| a.start.total_cmp(&b.start));
	let switches = active
		.windows(2)
		.filter(|pair| {
			pair[0].session != pair[1].session && pair[1].start - pair[0].end < BREAK_MIN
		})
		.count();
	let mut longest_block_min: f64 = 0.0;
	if let Some(first) = active.first() {
		let (mut block_start, mut block_end) = (first.start, first.end);
		for interval in &active[1..] {
			if interval.start - block_end < BREAK_MIN {
				block_end = block_end.max(interval.end);
			} else {
				longest_block_min = longest_block_min.max(block_end - block_start);
				(block_start, block_end) = (interval.start, interval.end);
			}
		}
		longest_block_min = longest_block_min.max(block_end - block_start);
	}

	let energy_dhe =
		result.iter().map(|s| s.energy_dhe).sum::<f64>() + switches as f64 * config.switch_cost_dhe;
	Timing {
		time_min: result.iter().map(|s| s.time_min).sum(),
		sessions: result,
		energy_dhe,
		budget_dhe: DAILY_BUDGET_DHE,
		switches,
		longest_block_min,
		long_block: longest_block_min > MAX_BLOCK_MIN,
		over_budget: energy_dhe > DAILY_BUDGET_DHE,
	}
}

/// Lines the agent wrote, from the rows a line-id editor returns for its edit:
/// `+12:ab|…` for a written line, `-12:ab (3 lines)` for a removed range.
/// Unified diffs in tool output (`git diff` in a shell) are the agent reading
/// existing changes, not writing them, so they count 0 — as do `- item` bullets.
pub fn diff_lines(content: &str) -> usize {
	content.lines().filter(|line| is_line_id_row(line)).count()
}

/// `+N:hh|…` (a written line) or `-N:hh (…` / `-N:hh..` (a removed range).
fn is_line_id_row(line: &str) -> bool {
	let Some((number, rest)) = line.get(1..).and_then(|rest| rest.split_once(':')) else {
		return false;
	};
	let (Some(hash), Some(tail)) = (rest.get(..2), rest.get(2..)) else {
		return false;
	};
	let shaped = match line.as_bytes()[0] {
		b'+' => tail.starts_with('|'),
		b'-' => tail.starts_with(" (") || tail.starts_with(".."),
		_ => false,
	};
	shaped
		&& !number.is_empty()
		&& number.bytes().all(|b| b.is_ascii_digit())
		&& hash.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
#[path = "timing_tests.rs"]
mod tests;
