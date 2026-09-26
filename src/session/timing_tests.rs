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

use super::*;

fn template_timing() -> TimingConfig {
	let config: crate::config::Config =
		toml::from_str(include_str!("../../config-templates/default.toml"))
			.expect("parse default config template");
	config.timing
}

/// Unix seconds of a clock time on day zero.
fn at(hour: u64, minute: u64) -> u64 {
	(hour * 60 + minute) * 60
}

fn turn(input_at: u64, done_at: u64, words_in: usize, words_out: usize, diff_lines: usize) -> Turn {
	Turn {
		input_at,
		done_at,
		words_in,
		words_out,
		diff_lines,
	}
}

/// A session with no known opening time: its first input gets the full cap.
fn session(turns: Vec<Turn>) -> SessionTurns {
	SessionTurns {
		name: String::new(),
		opened_at: None,
		turns,
	}
}

fn close(actual: f64, expected: f64) {
	assert!(
		(actual - expected).abs() < 1e-6,
		"expected {expected}, got {actual}"
	);
}

#[test]
fn two_overlapping_sessions_split_attention_wait_and_energy() {
	// Session A is a spec task with one large review, session B a feature
	// task started while A's run was going.
	let a = vec![
		turn(at(9, 13), at(9, 25), 260, 476, 0),
		turn(at(9, 30), at(10, 5), 52, 238, 134),
		turn(at(11, 5), at(11, 10), 26, 238, 0),
	];
	let b = vec![
		turn(at(9, 35), at(9, 50), 104, 238, 67),
		turn(at(10, 2), at(10, 3), 26, 119, 0),
	];
	let timing = compute(&[session(a), session(b)], &template_timing());

	let a = &timing.sessions[0];
	let active: Vec<f64> = a.turns.iter().map(|t| t.active_min).collect();
	let wait: Vec<f64> = a.turns.iter().map(|t| t.wait_min).collect();
	// A2 shares 09:28–09:30 with B1; the last turn carries the closing review.
	for (actual, expected) in active.into_iter().zip([13.0, 4.0, 46.0 + 2.5]) {
		close(actual, expected);
	}
	for (actual, expected) in wait.into_iter().zip([12.0, 7.5, 5.0]) {
		close(actual, expected);
	}
	close(a.active_min, 65.5);
	close(a.wait_min, 24.5);
	close(a.time_min, 90.0);
	// A3's 46 min cover reading A2's text and replying (6 min at weight 1) and
	// A2's 134 lines (the other 40 min at weight 2).
	close(a.code_min, 40.0);
	close(a.energy_dhe, 111.625 / 60.0);
	assert_eq!(a.turns[1].read_share, Some(1.0));

	let b = &timing.sessions[1];
	close(b.active_min, 20.0);
	close(b.wait_min, 8.5);
	// B2's 12 min: 6 for B1's text and the reply, 6 of the 10 B1's lines need.
	close(b.code_min, 6.0);
	close(b.energy_dhe, 28.125 / 60.0);
	close(b.turns[0].read_share.expect("B2 followed B1"), 0.6);

	close(timing.time_min, 118.5);
	close(timing.code_min, 46.0);
	assert_eq!(timing.lines, 201);
	close(timing.lines_read, 134.0 + 67.0 * 0.6);
	assert_eq!(timing.switches, 1);
	close(timing.energy_dhe, (111.625 + 28.125) / 60.0 + 0.05);
	// A3 (10:19–11:05) chains with the closing read 5 minutes later.
	close(timing.longest_block_min, 53.5);
	assert!(!timing.long_block);
	assert!(!timing.over_budget);
	assert!(timing
		.sessions
		.iter()
		.all(|s| s.turns.iter().all(|t| !t.rubber_stamp)));
}

#[test]
fn time_before_the_next_input_tells_a_code_review_from_a_behavior_check() {
	// A run changes 200 lines; the reply comes `secs` after it finished.
	let reply_after = |secs: u64| {
		let turns = vec![
			turn(at(10, 0), at(10, 10), 30, 0, 200),
			turn(at(10, 10) + secs, at(10, 10) + secs + 60, 2, 10, 0),
		];
		compute(&[session(turns)], &template_timing())
			.sessions
			.remove(0)
	};

	let reviewed = reply_after(40 * 60);
	assert_eq!(reviewed.turns[0].read_share, Some(1.0));
	assert!(reviewed.code_min > 30.0);

	// A minute covers reading the reply's context and typing it, not 200 lines.
	let checked = reply_after(60);
	assert_eq!(checked.turns[0].read_share, Some(0.0));
	assert!(!checked.turns[0].rubber_stamp);
	close(checked.code_min, 0.0);

	// Ten seconds is not enough to read even the agent's text.
	let stamped = reply_after(10);
	assert!(stamped.turns[0].rubber_stamp);

	// Without a next input the lines stay unread, and the closing read covers
	// only the agent's text.
	let open = compute(
		&[session(vec![turn(at(10, 0), at(10, 10), 30, 0, 200)])],
		&template_timing(),
	);
	assert_eq!(open.sessions[0].turns[0].read_share, None);
	close(open.code_min, 0.0);
	assert_eq!(open.lines, 200);
}

#[test]
fn the_first_input_is_bounded_by_when_the_session_opened() {
	// Opened, a 6-word request 3 s later, a 52 s run ending in a 672-word brief.
	let brief = SessionTurns {
		name: String::new(),
		opened_at: Some(at(17, 31) + 6),
		turns: vec![turn(at(17, 31) + 9, at(17, 32) + 1, 6, 672, 0)],
	};
	let timing = compute(&[brief], &template_timing());
	let turn = &timing.sessions[0].turns[0];
	// 3 s of typing, then reading the brief: 672 / 238 words/min + 1.5 min.
	close(turn.active_min, 3.0 / 60.0 + 672.0 / 238.0 + 1.5);
	close(turn.wait_min, 52.0 / 60.0);
}

#[test]
fn diff_lines_counts_only_rows_of_the_agents_own_edits() {
	let edit = "...\n-10:87 (1 line)\n-12:ab..14:cd (3 lines)\n+10:af|version = 20\n11:c5|\n";
	assert_eq!(diff_lines(edit), 3);
	// A `git diff` the agent ran to read existing changes is not its writing.
	let unified = "--- a/x.rs\n+++ b/x.rs\n@@ -1,2 +1,2 @@\n-old\n+new\n+12:30 standup\n context";
	assert_eq!(diff_lines(unified), 0);
	assert_eq!(diff_lines("- item\n- other\n+ plus"), 0);
}
