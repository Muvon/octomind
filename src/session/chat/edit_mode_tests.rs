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
use reedline::EditCommand;

#[allow(clippy::type_complexity)]
fn mk() -> (
	EmacsWithShortcutHelp,
	Arc<Mutex<LineState>>,
	Arc<AtomicBool>, // buffer_empty
	Arc<AtomicBool>, // reverse_search_active
	Arc<AtomicBool>, // hint_available
) {
	let buffer_empty = Arc::new(AtomicBool::new(true));
	let reverse = Arc::new(AtomicBool::new(false));
	let hint = Arc::new(AtomicBool::new(false));
	let line_state = Arc::new(Mutex::new(LineState::default()));
	let helper = EmacsWithShortcutHelp::new(
		Emacs::new(reedline::default_emacs_keybindings()),
		buffer_empty.clone(),
		reverse.clone(),
		hint.clone(),
		line_state.clone(),
		ExternalPrinter::new(5),
	);
	(helper, line_state, buffer_empty, reverse, hint)
}

fn key(code: KeyCode, mods: KeyModifiers) -> ReedlineRawEvent {
	ReedlineRawEvent::try_from(Event::Key(KeyEvent::new(code, mods)))
		.expect("key event is convertible")
}

#[test]
fn test_esc_meta_sequences() {
	let (mut h, ..) = mk();

	// Esc arms the meta prefix and swallows the keypress
	assert_eq!(
		h.parse_event(key(KeyCode::Esc, KeyModifiers::NONE)),
		ReedlineEvent::None
	);
	// Esc+b → word left
	assert_eq!(
		h.parse_event(key(KeyCode::Char('b'), KeyModifiers::NONE)),
		ReedlineEvent::Edit(vec![EditCommand::MoveWordLeft { select: false }])
	);
	// Meta prefix is consumed: the next plain char inserts normally
	assert_eq!(
		h.parse_event(key(KeyCode::Char('b'), KeyModifiers::NONE)),
		ReedlineEvent::Edit(vec![EditCommand::InsertChar('b')])
	);

	// Esc+Backspace → backspace word
	h.parse_event(key(KeyCode::Esc, KeyModifiers::NONE));
	assert_eq!(
		h.parse_event(key(KeyCode::Backspace, KeyModifiers::NONE)),
		ReedlineEvent::Edit(vec![EditCommand::BackspaceWord])
	);

	// Esc+d → cut word right
	h.parse_event(key(KeyCode::Esc, KeyModifiers::NONE));
	assert_eq!(
		h.parse_event(key(KeyCode::Char('d'), KeyModifiers::NONE)),
		ReedlineEvent::Edit(vec![EditCommand::CutWordRight])
	);

	// Esc+f → word right
	h.parse_event(key(KeyCode::Esc, KeyModifiers::NONE));
	assert_eq!(
		h.parse_event(key(KeyCode::Char('f'), KeyModifiers::NONE)),
		ReedlineEvent::Edit(vec![EditCommand::MoveWordRight { select: false }])
	);
}

#[test]
fn test_ctrl_g_sets_add_without_sending() {
	let (mut h, state, ..) = mk();
	assert_eq!(
		h.parse_event(key(KeyCode::Char('g'), KeyModifiers::CONTROL)),
		ReedlineEvent::Submit
	);
	assert!(state.lock().expect("line state").add_without_sending);
}

#[test]
fn test_ctrl_a_moves_to_line_start() {
	let (mut h, ..) = mk();
	assert_eq!(
		h.parse_event(key(KeyCode::Char('a'), KeyModifiers::CONTROL)),
		ReedlineEvent::Edit(vec![EditCommand::MoveToLineStart { select: false }])
	);
}

#[test]
fn test_ctrl_e_priority() {
	// Default: move to line end
	let (mut h, ..) = mk();
	assert_eq!(
		h.parse_event(key(KeyCode::Char('e'), KeyModifiers::CONTROL)),
		ReedlineEvent::Edit(vec![EditCommand::MoveToLineEnd { select: false }])
	);

	// Hint available → accept the hint
	let (mut h, _, _, _, hint) = mk();
	hint.store(true, Ordering::SeqCst);
	assert_eq!(
		h.parse_event(key(KeyCode::Char('e'), KeyModifiers::CONTROL)),
		ReedlineEvent::HistoryHintComplete
	);

	// Reverse search wins over everything → Enter (accept)
	let (mut h, _, _, reverse, hint) = mk();
	reverse.store(true, Ordering::SeqCst);
	hint.store(true, Ordering::SeqCst);
	assert_eq!(
		h.parse_event(key(KeyCode::Char('e'), KeyModifiers::CONTROL)),
		ReedlineEvent::Enter
	);
}

#[test]
fn test_ctrl_u_multiline() {
	let (mut h, state, ..) = mk();

	// Cursor at the start of a continuation line → join with previous line
	{
		let mut s = state.lock().expect("line state");
		s.buffer = "ab\ncd".to_string();
		s.cursor = 3;
	}
	assert_eq!(
		h.parse_event(key(KeyCode::Char('u'), KeyModifiers::CONTROL)),
		ReedlineEvent::Edit(vec![EditCommand::Backspace])
	);

	// Mid-line → cut from line start
	{
		let mut s = state.lock().expect("line state");
		s.cursor = 5;
	}
	assert_eq!(
		h.parse_event(key(KeyCode::Char('u'), KeyModifiers::CONTROL)),
		ReedlineEvent::Edit(vec![EditCommand::CutFromLineStart])
	);

	// Start of the first line → cut from line start (nothing to join)
	{
		let mut s = state.lock().expect("line state");
		s.cursor = 0;
	}
	assert_eq!(
		h.parse_event(key(KeyCode::Char('u'), KeyModifiers::CONTROL)),
		ReedlineEvent::Edit(vec![EditCommand::CutFromLineStart])
	);
}

#[test]
fn test_ctrl_c_and_reverse_search() {
	let (mut h, ..) = mk();
	assert_eq!(
		h.parse_event(key(KeyCode::Char('c'), KeyModifiers::CONTROL)),
		ReedlineEvent::CtrlC
	);

	let (mut h, _, _, reverse, _) = mk();
	reverse.store(true, Ordering::SeqCst);
	assert_eq!(
		h.parse_event(key(KeyCode::Char('c'), KeyModifiers::CONTROL)),
		ReedlineEvent::Esc
	);
}

#[test]
fn test_question_mark_shortcut_help() {
	// Empty buffer → shortcut help
	let (mut h, _, buffer_empty, _, _) = mk();
	buffer_empty.store(true, Ordering::SeqCst);
	assert_eq!(
		h.parse_event(key(KeyCode::Char('?'), KeyModifiers::NONE)),
		ReedlineEvent::ExecuteHostCommand("__show_shortcuts__".to_string())
	);

	// Non-empty buffer → plain insert
	let (mut h, _, buffer_empty, _, _) = mk();
	buffer_empty.store(false, Ordering::SeqCst);
	assert_eq!(
		h.parse_event(key(KeyCode::Char('?'), KeyModifiers::NONE)),
		ReedlineEvent::Edit(vec![EditCommand::InsertChar('?')])
	);
}

/// Runs on the headless dev box where no clipboard blob can intercept the
/// paste probe, so the multiline-wrap branch is deterministic.
#[test]
fn test_paste_wrapping() {
	let (mut h, ..) = mk();

	// 3+ lines → wrapped in <log> tags
	let raw = ReedlineRawEvent::try_from(Event::Paste("a\nb\nc".to_string()))
		.expect("paste event is convertible");
	assert_eq!(
		h.parse_event(raw),
		ReedlineEvent::Edit(vec![EditCommand::InsertString(
			"<log>\na\nb\nc\n</log>".to_string()
		)])
	);

	// Short paste falls through unwrapped
	let raw = ReedlineRawEvent::try_from(Event::Paste("a\nb".to_string()))
		.expect("paste event is convertible");
	match h.parse_event(raw) {
		ReedlineEvent::Edit(cmds) => {
			assert_eq!(cmds, vec![EditCommand::InsertString("a\nb".to_string())]);
		}
		other => panic!("expected Edit(InsertString), got {:?}", other),
	}
}

#[test]
fn test_format_size() {
	assert_eq!(format_size(512 * 1024), "512 KB");
	assert_eq!(format_size(1536 * 1024), "1.5 MB");
	assert_eq!(format_size(0), "0 KB");
}
