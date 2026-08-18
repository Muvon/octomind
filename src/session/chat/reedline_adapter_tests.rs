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
