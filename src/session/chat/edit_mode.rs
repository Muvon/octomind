// Copyright 2025 Muvon Un Limited
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

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use reedline::{EditMode, Emacs, PromptEditMode, ReedlineEvent, ReedlineRawEvent};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::Mutex;

pub struct EmacsWithShortcutHelp {
	emacs: Emacs,
	buffer_empty: Arc<AtomicBool>,
	reverse_search_active: Arc<AtomicBool>,
	hint_available: Arc<AtomicBool>,
	line_state: Arc<Mutex<crate::session::chat::reedline_adapter::LineState>>,
	meta_pending: bool,
}

impl EmacsWithShortcutHelp {
	pub fn new(
		emacs: Emacs,
		buffer_empty: Arc<AtomicBool>,
		reverse_search_active: Arc<AtomicBool>,
		hint_available: Arc<AtomicBool>,
		line_state: Arc<Mutex<crate::session::chat::reedline_adapter::LineState>>,
	) -> Self {
		Self {
			emacs,
			buffer_empty,
			reverse_search_active,
			hint_available,
			line_state,
			meta_pending: false,
		}
	}
}

impl EditMode for EmacsWithShortcutHelp {
	fn parse_event(&mut self, event: ReedlineRawEvent) -> ReedlineEvent {
		let event: Event = event.into();
		if let Event::Key(KeyEvent {
			code, modifiers, ..
		}) = event
		{
			if modifiers == KeyModifiers::NONE && code == KeyCode::Esc {
				self.meta_pending = true;
				return ReedlineEvent::None;
			}
			if self.meta_pending {
				self.meta_pending = false;
				match code {
					KeyCode::Char('h') | KeyCode::Char('H') | KeyCode::Backspace => {
						return ReedlineEvent::Edit(vec![reedline::EditCommand::BackspaceWord]);
					}
					KeyCode::Char('d') | KeyCode::Char('D') => {
						return ReedlineEvent::Edit(vec![reedline::EditCommand::CutWordRight]);
					}
					KeyCode::Char('b') | KeyCode::Char('B') => {
						return ReedlineEvent::Edit(vec![reedline::EditCommand::MoveWordLeft {
							select: false,
						}]);
					}
					KeyCode::Char('f') | KeyCode::Char('F') => {
						return ReedlineEvent::Edit(vec![reedline::EditCommand::MoveWordRight {
							select: false,
						}]);
					}
					_ => {}
				}
			}
			if modifiers.contains(KeyModifiers::ALT) && modifiers.contains(KeyModifiers::CONTROL) {
				match code {
					KeyCode::Char('h') | KeyCode::Char('H') | KeyCode::Backspace => {
						return ReedlineEvent::Edit(vec![reedline::EditCommand::BackspaceWord]);
					}
					KeyCode::Char('d') | KeyCode::Char('D') => {
						return ReedlineEvent::Edit(vec![reedline::EditCommand::CutWordRight]);
					}
					KeyCode::Char('b') | KeyCode::Char('B') => {
						return ReedlineEvent::Edit(vec![reedline::EditCommand::MoveWordLeft {
							select: false,
						}]);
					}
					KeyCode::Char('f') | KeyCode::Char('F') => {
						return ReedlineEvent::Edit(vec![reedline::EditCommand::MoveWordRight {
							select: false,
						}]);
					}
					_ => {}
				}
			}
			if code == KeyCode::Char('a') && modifiers == KeyModifiers::CONTROL {
				return ReedlineEvent::Edit(vec![reedline::EditCommand::MoveToLineStart {
					select: false,
				}]);
			}
			if code == KeyCode::Char('e') && modifiers == KeyModifiers::CONTROL {
				if self.hint_available.load(Ordering::SeqCst) {
					return ReedlineEvent::HistoryHintComplete;
				}
				return ReedlineEvent::Edit(vec![reedline::EditCommand::MoveToLineEnd {
					select: false,
				}]);
			}
			if code == KeyCode::Char('u') && modifiers == KeyModifiers::CONTROL {
				let state = self.line_state.lock().ok();
				if let Some(state) = state {
					let cursor = state.cursor.min(state.buffer.len());
					let line_start = state.buffer[..cursor]
						.rfind('\n')
						.map(|idx| idx + 1)
						.unwrap_or(0);
					if cursor == line_start && line_start > 0 {
						return ReedlineEvent::Edit(vec![reedline::EditCommand::Backspace]);
					}
				}
				return ReedlineEvent::Edit(vec![reedline::EditCommand::CutFromLineStart]);
			}
			if code == KeyCode::Char('?') && modifiers == KeyModifiers::NONE {
				if self.buffer_empty.load(Ordering::SeqCst) {
					return ReedlineEvent::ExecuteHostCommand("__show_shortcuts__".to_string());
				}
			}
			if code == KeyCode::Char('c') && modifiers == KeyModifiers::CONTROL {
				if self.reverse_search_active.load(Ordering::SeqCst) {
					return ReedlineEvent::Esc;
				}
				return ReedlineEvent::CtrlC;
			}
		}

		match ReedlineRawEvent::try_from(event) {
			Ok(raw_event) => self.emacs.parse_event(raw_event),
			Err(()) => ReedlineEvent::None,
		}
	}

	fn edit_mode(&self) -> PromptEditMode {
		self.emacs.edit_mode()
	}
}
