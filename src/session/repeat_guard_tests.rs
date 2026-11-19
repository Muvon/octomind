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
use serde_json::json;

struct Call {
	tool: &'static str,
	parameters: serde_json::Value,
	result: &'static str,
	is_error: bool,
	is_mutation: bool,
}

fn search(path: &'static str, result: &'static str) -> Call {
	Call {
		tool: "search",
		parameters: json!({"path": path}),
		result,
		is_error: false,
		is_mutation: false,
	}
}

fn edit(path: &'static str, result: &'static str) -> Call {
	Call {
		tool: "edit",
		parameters: json!({"path": path}),
		result,
		is_error: false,
		is_mutation: true,
	}
}

/// Feed one round and return the count the guard gives.
fn round(guard: &mut RepeatGuard, calls: &[Call]) -> usize {
	let round: Vec<RoundCall<'_>> = calls
		.iter()
		.map(|call| RoundCall {
			tool: call.tool,
			parameters: &call.parameters,
			result: call.result,
			is_error: call.is_error,
			is_mutation: call.is_mutation,
			free_form: false,
		})
		.collect();
	guard.note_round(&round)
}

/// Feed the rounds and collect the guard's counts.
fn run(guard: &mut RepeatGuard, rounds: &[Vec<Call>]) -> Vec<usize> {
	rounds.iter().map(|calls| round(guard, calls)).collect()
}

#[test]
fn one_round_repeated_counts_up() {
	let mut guard = RepeatGuard::default();
	let rounds: Vec<Vec<Call>> = (0..4).map(|_| vec![search("a.rs", "no matches")]).collect();
	assert_eq!(run(&mut guard, &rounds), vec![1, 2, 3, 4]);
}

#[test]
fn a_cycle_through_three_rounds_counts_as_repeating() {
	let mut guard = RepeatGuard::default();
	let rounds: Vec<Vec<Call>> = (0..3)
		.flat_map(|_| {
			[
				vec![search("a.rs", "one")],
				vec![search("b.rs", "two")],
				vec![search("c.rs", "three")],
			]
		})
		.collect();
	let counts = run(&mut guard, &rounds);
	assert!(counts.last().copied().unwrap() > 1, "{counts:?}");
}

#[test]
fn a_new_result_restarts_the_count() {
	let mut guard = RepeatGuard::default();
	let mut rounds: Vec<Vec<Call>> = (0..3).map(|_| vec![search("a.rs", "no matches")]).collect();
	rounds.push(vec![search("a.rs", "a hit")]);
	rounds.push(vec![search("a.rs", "no matches")]);
	let counts = run(&mut guard, &rounds);
	assert_eq!(counts[3], 1);
	assert!(counts[4] <= 2, "{counts:?}");
}

#[test]
fn a_parallel_batch_counts_once_whatever_its_order() {
	let mut guard = RepeatGuard::default();
	let batch = |first: bool| {
		let mut calls = vec![search("a.rs", "x"), search("b.rs", "y")];
		if !first {
			calls.reverse();
		}
		calls
	};
	let rounds = vec![batch(true), batch(false), batch(true)];
	assert_eq!(run(&mut guard, &rounds), vec![1, 2, 3]);
}

#[test]
fn a_write_that_repeats_its_output_still_restarts_the_count() {
	let mut guard = RepeatGuard::default();
	let rounds: Vec<Vec<Call>> = (0..3).map(|_| vec![edit("a.rs", "ok")]).collect();
	let counts = run(&mut guard, &rounds);
	assert!(counts.iter().all(|&count| count == 1), "{counts:?}");
}

#[test]
fn a_free_form_command_earns_no_write_credit() {
	let parameters = json!({"command": "make"});
	let call = |free_form| RoundCall {
		tool: "shell",
		parameters: &parameters,
		result: "ok",
		is_error: false,
		is_mutation: true,
		free_form,
	};
	let mut guard = RepeatGuard::default();
	guard.note_round(&[call(true)]);
	assert_eq!(guard.note_round(&[call(true)]), 2);
	let mut guard = RepeatGuard::default();
	guard.note_round(&[call(false)]);
	assert_eq!(guard.note_round(&[call(false)]), 1);
}

/// A script that names a tool able to write is marked a mutation whether or not
/// it wrote, so the same script with the same output counts up like any repeat.
#[test]
fn a_script_that_can_write_earns_no_write_credit() {
	let parameters = json!({"script": "print(await octofs.shell({command: \"sed -n '2343,2400p' a.js\"}))"});
	let call = RoundCall {
		tool: crate::mcp::code::TOOL_NAME,
		parameters: &parameters,
		result: "the same lines",
		is_error: false,
		is_mutation: true,
		free_form: false,
	};
	let mut guard = RepeatGuard::default();
	let counts: Vec<usize> = (0..4).map(|_| guard.note_round(&[call])).collect();
	assert_eq!(counts, vec![1, 2, 3, 4]);
}

#[test]
fn a_tree_move_restarts_the_count() {
	let mut guard = RepeatGuard::default();
	let rounds: Vec<Vec<Call>> = (0..3).map(|_| vec![search("a.rs", "no matches")]).collect();
	run(&mut guard, &rounds);
	guard.note_tree_moved();
	assert_eq!(guard.identical_rounds(), 1);
	assert_eq!(run(&mut guard, &[vec![search("a.rs", "no matches")]]), vec![2]);
}

#[test]
fn a_reset_clears_the_count_but_not_what_was_seen() {
	let mut guard = RepeatGuard::default();
	let rounds: Vec<Vec<Call>> = (0..3).map(|_| vec![search("a.rs", "no matches")]).collect();
	run(&mut guard, &rounds);
	guard.reset_streak();
	assert_eq!(guard.identical_rounds(), 0);
	run(
		&mut guard,
		&[vec![search("a.rs", "no matches")], vec![search("a.rs", "no matches")]],
	);
}

#[test]
fn a_tap_relaunch_with_a_new_run_id_is_the_same_call() {
	let parameters = json!({"agent": "x"});
	let mut guard = RepeatGuard::default();
	guard.note_round(&[tap_launch(&parameters, r#"{"id":"tap-1"}"#)]);
	assert_eq!(guard.note_round(&[tap_launch(&parameters, r#"{"id":"tap-2"}"#)]), 2);
}

fn tap_launch<'a>(parameters: &'a serde_json::Value, result: &'a str) -> RoundCall<'a> {
	RoundCall {
		tool: "tap",
		parameters,
		result,
		is_error: false,
		is_mutation: false,
		free_form: false,
	}
}

#[test]
fn loop_stop_note_names_the_repeated_call_and_its_result() {
	let call = crate::mcp::McpToolCall {
		tool_name: "shell".to_string(),
		parameters: json!({"command": "test -e compat/lib.so"}),
		tool_id: "c1".to_string(),
	};
	let result = crate::mcp::McpToolResult::success("shell".to_string(), "c1".to_string(), "False".to_string());
	let note = loop_stop_note(10, &[call], &[result]);
	assert!(note.starts_with("Stopped by the supervisor: the last 10 tool rounds"));
	assert!(note.contains("Repeated: `shell` {\"command\":\"test -e compat/lib.so\"}"));
	assert!(note.contains("Result: False"));
	assert!(note.ends_with("continuing needs a different approach."));

	let long = crate::mcp::McpToolCall {
		tool_name: "shell".to_string(),
		parameters: json!({"command": "x".repeat(1000)}),
		tool_id: "c2".to_string(),
	};
	let note = loop_stop_note(10, &[long], &[]);
	let repeated = note
		.lines()
		.find(|line| line.starts_with("Repeated:"))
		.expect("call line");
	assert!(
		repeated.chars().count() < 400,
		"call preview is capped: {}",
		repeated.len()
	);
}

/// One finished call of the same search, as the agent saw it and as the tool
/// returned it.
fn seen_and_returned<'a>(
	parameters: &'a serde_json::Value,
	shown: (&'a str, bool),
	returned: (&'a str, bool),
) -> (RoundCall<'a>, RoundCall<'a>) {
	let call = |(result, is_error)| RoundCall {
		tool: "search",
		parameters,
		result,
		is_error,
		is_mutation: false,
		free_form: false,
	};
	(call(shown), call(returned))
}

/// One round: what the agent was shown and what the tool returned, each as
/// its text and whether it is an error.
type Round<'a> = ((&'a str, bool), (&'a str, bool));

/// Feed rounds of one call each and collect the guard's counts.
fn run_pairs(guard: &mut RepeatGuard, parameters: &serde_json::Value, rounds: &[Round<'_>]) -> Vec<usize> {
	rounds
		.iter()
		.map(|(shown, returned)| {
			let (shown, returned) = seen_and_returned(parameters, *shown, *returned);
			guard.note_round_returned(&[shown], &[returned])
		})
		.collect()
}

#[test]
fn rounds_that_became_repeat_notes_count_as_repeats_of_the_real_result() {
	let parameters = json!({"path": "a.rs"});
	let real = ("no matches", false);
	// The tool says the same again; the agent is told it is a repeat, as an error.
	let note = ("duplicate tool call", true);
	let rounds = [(real, real), (note, real), (note, real), (note, real)];

	let counts = run_pairs(&mut RepeatGuard::default(), &parameters, &rounds);

	assert_eq!(counts, vec![1, 2, 3, 4]);
}

#[test]
fn the_same_returned_text_shown_differently_each_round_is_still_a_repeat() {
	let parameters = json!({"path": "a.rs"});
	let real = ("no matches", false);
	let rounds = [
		(("summary one", false), real),
		(("summary two", false), real),
		(("summary three", false), real),
	];

	let counts = run_pairs(&mut RepeatGuard::default(), &parameters, &rounds);

	assert_eq!(counts, vec![1, 2, 3]);
}

#[test]
fn the_same_shown_text_over_different_returned_texts_is_a_repeat() {
	let parameters = json!({"path": "a.rs"});
	let cut = "first lines of the output [cut]";
	// The size cap cuts every output down to the same start.
	let rounds = [
		((cut, false), ("output ending in one", false)),
		((cut, false), ("output ending in two", false)),
		((cut, false), ("output ending in three", false)),
	];

	let counts = run_pairs(&mut RepeatGuard::default(), &parameters, &rounds);

	assert_eq!(counts, vec![1, 2, 3]);
}

#[test]
fn different_output_stays_different_whatever_the_agent_was_shown() {
	let parameters = json!({"path": "a.rs"});
	let rounds = [
		(("one", false), ("output one", false)),
		(("two", false), ("output two", false)),
		(("duplicate tool call", true), ("output three", false)),
	];

	let counts = run_pairs(&mut RepeatGuard::default(), &parameters, &rounds);

	assert_eq!(counts, vec![1, 1, 1]);
}

#[test]
fn a_successful_write_shown_as_a_repeat_note_is_still_progress() {
	let parameters = json!({"path": "a.rs"});
	let call = |result, is_error, returned_error| {
		(
			RoundCall {
				tool: "edit",
				parameters: &parameters,
				result,
				is_error,
				is_mutation: true,
				free_form: false,
			},
			RoundCall {
				tool: "edit",
				parameters: &parameters,
				result: "ok",
				is_error: returned_error,
				is_mutation: true,
				free_form: false,
			},
		)
	};
	let mut guard = RepeatGuard::default();
	for _ in 0..3 {
		let (shown, returned) = call("duplicate tool call", true, false);
		assert_eq!(guard.note_round_returned(&[shown], &[returned]), 1);
	}
}
