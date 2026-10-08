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

//! Adapter tests: the reedline Completer/Hinter/Highlighter bridge over the
//! CommandCompleter, including the atomics the edit-mode keymap reads.

use super::*;

fn adapter() -> (
	ReedlineAdapter,
	Arc<AtomicBool>, // buffer_empty
	Arc<AtomicBool>, // hint_available
	Arc<Mutex<LineState>>,
) {
	adapter_with_replies(Vec::new())
}

fn adapter_with_replies(
	replies: Vec<Reply>,
) -> (
	ReedlineAdapter,
	Arc<AtomicBool>, // buffer_empty
	Arc<AtomicBool>, // hint_available
	Arc<Mutex<LineState>>,
) {
	let config: crate::config::Config =
		toml::from_str(include_str!("../../../config-templates/default.toml"))
			.expect("parse default config template");
	let buffer_empty = Arc::new(AtomicBool::new(true));
	let hint_available = Arc::new(AtomicBool::new(false));
	let line_state = Arc::new(Mutex::new(LineState::default()));
	let adapter = ReedlineAdapter::new(
		Arc::new(config),
		"assistant",
		buffer_empty.clone(),
		hint_available.clone(),
		line_state.clone(),
		Arc::new(replies),
	);
	(adapter, buffer_empty, hint_available, line_state)
}

#[test]
fn test_completer_bridges_suggestions() {
	let (mut adapter, ..) = adapter();
	let result = adapter.complete("/mcp li", 7);
	let suggestions = result.suggestions().to_vec();
	assert_eq!(suggestions.len(), 1);
	assert_eq!(suggestions[0].value, "list");
	// Replacement span starts after the command prefix
	assert_eq!(suggestions[0].span.start, 5);
	assert_eq!(suggestions[0].span.end, 7);
}

fn styled_to_string(styled: &reedline::StyledText) -> String {
	styled.buffer.iter().map(|(_, s)| s.as_str()).collect()
}

#[test]
fn test_highlighter_paints_commands_only() {
	let (adapter, ..) = adapter();
	// Non-command lines pass through as one plain segment
	let plain = adapter.highlight("just some text", 0);
	assert_eq!(plain.buffer.len(), 1);
	assert_eq!(styled_to_string(&plain), "just some text");

	// Valid commands get a styled command token + remainder; content is
	// preserved verbatim across the split
	let styled = adapter.highlight("/help me please", 0);
	assert!(styled.buffer.len() >= 2);
	assert_eq!(styled_to_string(&styled), "/help me please");
}

#[test]
fn test_hinter_updates_shared_state() {
	let (mut adapter, buffer_empty, hint_available, line_state) = adapter();
	let history = reedline::FileBackedHistory::new(10).expect("history");

	// A command prefix produces a completion hint and records line state
	let hint = adapter.handle("/he", 3, &history, false, "/tmp");
	assert!(!hint.is_empty(), "expected hint for /he, got {hint:?}");
	assert!(hint_available.load(std::sync::atomic::Ordering::SeqCst));
	assert!(!buffer_empty.load(std::sync::atomic::Ordering::SeqCst));
	{
		let state = line_state.lock().expect("line state");
		assert_eq!(state.buffer, "/he");
		assert_eq!(state.cursor, 3);
	}
	assert_eq!(adapter.complete_hint(), hint);

	// Empty line clears both flags
	let empty_hint = adapter.handle("", 0, &history, false, "/tmp");
	assert!(empty_hint.is_empty());
	assert!(buffer_empty.load(std::sync::atomic::Ordering::SeqCst));
	assert!(!hint_available.load(std::sync::atomic::Ordering::SeqCst));
}

#[test]
fn test_next_hint_token_splits_first_word() {
	let (mut adapter, _, _, _) = adapter();
	let history = reedline::FileBackedHistory::new(10).expect("history");
	adapter.handle("/mc", 3, &history, false, "/tmp");
	// Hint for /mc completes toward /mcp — the next token is a single word
	assert!(!adapter.next_hint_token().contains(' '));
}
/// Foreground color of the styled segment whose text is exactly `text`.
fn segment_color(styled: &reedline::StyledText, text: &str) -> Option<Color> {
	styled
		.buffer
		.iter()
		.find(|(_, segment)| segment == text)
		.unwrap_or_else(|| panic!("no segment {text:?} in {:?}", styled.buffer))
		.0
		.foreground
}

#[test]
fn test_highlighter_colors_first_argument() {
	let (adapter, ..) = adapter();

	// A valid scope is green, an invalid one red; the text is preserved verbatim
	let valid = adapter.highlight("/copy last", 0);
	assert_eq!(styled_to_string(&valid), "/copy last");
	assert_eq!(segment_color(&valid, "last"), Some(Color::Green));

	let invalid = adapter.highlight("/copy bogus", 0);
	assert_eq!(styled_to_string(&invalid), "/copy bogus");
	assert_eq!(segment_color(&invalid, "bogus"), Some(Color::Red));

	// Everything after the first argument stays plain
	let with_rest = adapter.highlight("/status agents abc", 0);
	assert_eq!(styled_to_string(&with_rest), "/status agents abc");
	assert_eq!(segment_color(&with_rest, "agents"), Some(Color::Green));
	assert_eq!(segment_color(&with_rest, " abc"), None);

	// Commands with free-form arguments are never validated
	let free_form = adapter.highlight("/run whatever", 0);
	assert_eq!(styled_to_string(&free_form), "/run whatever");
	assert_eq!(segment_color(&free_form, " whatever"), None);

	// A partial command name is not a known command — nothing is validated
	let partial = adapter.highlight("/co last", 0);
	assert_eq!(styled_to_string(&partial), "/co last");
	assert_eq!(segment_color(&partial, " last"), None);
}

/// Newest first, as `assistant_replies` returns them: sent 2½ and 15
/// minutes ago, far enough from the minute marks that the test's own clock
/// tick cannot change the label.
fn replies() -> Vec<Reply> {
	let now = crate::utils::time::now_secs();
	vec![
		Reply {
			text: "Tests pass.\n\nThe parser fix holds.".to_string(),
			timestamp: now - 150,
		},
		Reply {
			text: "Done — parser fixed.\n```rust\nfn parse() {}\n```\nTests pass.".to_string(),
			timestamp: now - 930,
		},
	]
}

#[test]
fn test_reply_argument_lists_replies_as_readable_rows() {
	let (mut adapter, ..) = adapter_with_replies(replies());

	let all = adapter.complete("/reply ", 7).suggestions().to_vec();
	let values: Vec<&str> = all.iter().map(|s| s.value.as_str()).collect();
	assert_eq!(values, ["1", "2"]);
	// Each row reads as age, length and the reply's opening prose; heading
	// marks, fences and blank lines drop out of the preview
	let rows: Vec<&str> = all
		.iter()
		.map(|s| s.display_override.as_deref().expect("row text"))
		.collect();
	assert_eq!(
		rows,
		[
			"2m ago     3 lines  Tests pass. The parser fix holds.",
			"15m ago    5 lines  Done — parser fixed. fn parse() {} Tests pass.",
		]
	);
	assert_eq!(all[0].style, Some(Style::new()));
	assert_eq!((all[0].span.start, all[0].span.end), (7, 7));

	// Words select by content, a number by its prefix
	let by_word = adapter.complete("/reply FIXED", 12).suggestions().to_vec();
	assert_eq!(by_word.len(), 1);
	assert_eq!(by_word[0].value, "2");
	assert_eq!((by_word[0].span.start, by_word[0].span.end), (7, 12));

	let by_number = adapter.complete("/reply 2", 8).suggestions().to_vec();
	assert_eq!(by_number.len(), 1);
	assert_eq!(by_number[0].value, "2");
}

#[test]
fn test_quote_picker_lists_distinct_lines_newest_first() {
	let (mut adapter, ..) = adapter_with_replies(replies());

	// `>` on a later line of the buffer: the span covers only that line
	let buffer = "first line\n>";
	let lines = adapter
		.complete(buffer, buffer.len())
		.suggestions()
		.to_vec();
	let values: Vec<&str> = lines.iter().map(|s| s.value.as_str()).collect();
	// Blank lines and lone fences are skipped; the repeated line appears once
	assert_eq!(
		values,
		[
			"> Tests pass.\n",
			"> The parser fix holds.\n",
			"> Done — parser fixed.\n",
			"> fn parse() {}\n",
		]
	);
	assert_eq!(
		lines[0].display_override.as_deref(),
		Some("2m ago    Tests pass.")
	);
	assert_eq!(
		lines[2].display_override.as_deref(),
		Some("15m ago   Done — parser fixed.")
	);
	assert_eq!((lines[0].span.start, lines[0].span.end), (11, buffer.len()));

	// Every typed word must appear in the line, in any case and order
	let filtered = adapter.complete("> FIX parser", 12).suggestions().to_vec();
	let values: Vec<&str> = filtered.iter().map(|s| s.value.as_str()).collect();
	assert_eq!(
		values,
		["> The parser fix holds.\n", "> Done — parser fixed.\n"]
	);
}

#[test]
fn test_quote_picker_needs_gt_at_line_start() {
	let (mut adapter, ..) = adapter_with_replies(replies());
	assert!(adapter.complete("a > b", 5).suggestions().is_empty());
	assert!(adapter.complete("text\nx>", 7).suggestions().is_empty());
}

#[test]
fn test_highlighter_dims_quoted_lines() {
	let (adapter, ..) = adapter();
	let buffer = "> quoted\nmy answer\n>";
	let styled = adapter.highlight(buffer, 0);
	assert_eq!(styled_to_string(&styled), buffer);
	let segments: Vec<(bool, &str)> = styled
		.buffer
		.iter()
		.map(|(style, text)| (style.is_dimmed, text.as_str()))
		.collect();
	assert_eq!(
		segments,
		[(true, "> quoted\n"), (false, "my answer\n"), (true, ">")]
	);
}

#[test]
fn test_quote_buffers_get_no_history_hint() {
	let (mut adapter, ..) = adapter();
	let mut history = reedline::FileBackedHistory::new(10).expect("history");
	history
		.save(reedline::HistoryItem::from_command_line(
			"> old quote\nanswer",
		))
		.expect("save");
	history
		.save(reedline::HistoryItem::from_command_line("hello world"))
		.expect("save");

	assert!(adapter.handle(">", 1, &history, false, "/tmp").is_empty());
	// Ordinary text still completes from history
	assert_eq!(
		adapter.handle("hello", 5, &history, false, "/tmp"),
		" world"
	);
}
