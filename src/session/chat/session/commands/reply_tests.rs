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

//! Reply selection and quoting for `/reply`.

use super::*;
use crate::session::Message;

fn message(role: &str, content: &str, timestamp: u64) -> Message {
	Message {
		role: role.to_string(),
		content: content.to_string(),
		timestamp,
		..Default::default()
	}
}

fn transcript() -> ChatSession {
	ChatSession::for_tests(vec![
		message("system", "system prompt", 100),
		message("user", "fix the parser", 110),
		message("assistant", "Looking at it.", 120),
		message("tool", "file contents", 130),
		message("assistant", "   ", 140),
		message(
			"assistant",
			"Done — parser fixed.\n\nRun the tests next?",
			150,
		),
		message("user", "yes", 160),
		message("assistant", "\nTests pass.\n", 170),
	])
}

fn reply_output(result: CommandResult) -> (usize, u64, usize, String) {
	let CommandResult::HandledWithOutput(output) = result else {
		panic!("expected typed output");
	};
	let CommandOutput::Reply {
		number,
		timestamp,
		lines,
		quote,
	} = *output
	else {
		panic!("expected a reply output");
	};
	(number, timestamp, lines, quote)
}

fn error_output(result: CommandResult) -> String {
	let CommandResult::HandledWithOutput(output) = result else {
		panic!("expected typed output");
	};
	let CommandOutput::Error { error, .. } = *output else {
		panic!("expected an error output");
	};
	error
}

#[test]
fn replies_are_trimmed_assistant_prose_newest_first() {
	let reply = |text: &str, timestamp: u64| Reply {
		text: text.to_string(),
		timestamp,
	};
	assert_eq!(
		assistant_replies(&transcript()),
		[
			reply("Tests pass.", 170),
			reply("Done — parser fixed.\n\nRun the tests next?", 150),
			reply("Looking at it.", 120),
		]
	);
}

#[test]
fn preview_is_one_line_of_prose() {
	let reply = Reply {
		text: "## Summary\n\nFixed it.\n```rust\nfn a() {}\n```\n- next step".to_string(),
		timestamp: 0,
	};
	assert_eq!(reply.preview(), "Summary Fixed it. fn a() {} - next step");
	assert_eq!(reply.size(), "7 lines");

	let long = Reply {
		text: "word ".repeat(100),
		timestamp: 0,
	};
	assert_eq!(long.preview().chars().count(), PREVIEW_CHARS);
	assert_eq!(long.size(), "1 line");
}

#[test]
fn age_reads_like_people_say_it() {
	assert_eq!(age(1_000, 1_002), "just now");
	assert_eq!(age(1_000, 1_000 + 4 * 60 + 10), "4m ago");
	// A clock that moved backwards never shows a negative age
	assert_eq!(age(1_000, 900), "just now");
}

#[test]
fn quote_prefixes_every_line_and_keeps_blank_lines_in_the_block() {
	assert_eq!(quote("one\n\ntwo"), "> one\n>\n> two");
	assert_eq!(quote("single"), "> single");
}

#[test]
fn words_match_in_any_order_and_case() {
	assert!(matches_words("Done — parser fixed.", "FIXED parser"));
	assert!(matches_words("anything", ""));
	assert!(!matches_words("Done — parser fixed.", "parser tests"));
}

#[test]
fn bare_reply_quotes_the_latest() {
	let result = handle_reply(&transcript(), &[]).expect("handled");
	assert_eq!(
		reply_output(result),
		(1, 170, 1, "> Tests pass.".to_string())
	);
}

#[test]
fn number_selects_the_nth_latest() {
	let result = handle_reply(&transcript(), &["2"]).expect("handled");
	assert_eq!(
		reply_output(result),
		(
			2,
			150,
			3,
			"> Done — parser fixed.\n>\n> Run the tests next?".to_string()
		)
	);
}

#[test]
fn words_select_the_latest_reply_containing_them() {
	let result = handle_reply(&transcript(), &["looking", "AT"]).expect("handled");
	assert_eq!(
		reply_output(result),
		(3, 120, 1, "> Looking at it.".to_string())
	);
}

#[test]
fn missing_replies_are_reported() {
	let session = transcript();
	let out_of_range = handle_reply(&session, &["4"]).expect("handled");
	assert_eq!(
		error_output(out_of_range),
		"No reply #4: this session has 3."
	);
	let zero = handle_reply(&session, &["0"]).expect("handled");
	assert_eq!(error_output(zero), "No reply #0: this session has 3.");
	let no_match = handle_reply(&session, &["deploy"]).expect("handled");
	assert_eq!(error_output(no_match), "No reply contains 'deploy'.");

	let empty = ChatSession::for_tests(vec![message("user", "hi", 100)]);
	let nothing = handle_reply(&empty, &[]).expect("handled");
	assert_eq!(error_output(nothing), "No assistant reply to quote yet.");
}
