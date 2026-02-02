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

pub struct EmacsWithShortcutHelp {
	emacs: Emacs,
	buffer_empty: Arc<AtomicBool>,
	reverse_search_active: Arc<AtomicBool>,
}

impl EmacsWithShortcutHelp {
	pub fn new(
		emacs: Emacs,
		buffer_empty: Arc<AtomicBool>,
		reverse_search_active: Arc<AtomicBool>,
	) -> Self {
		Self {
			emacs,
			buffer_empty,
			reverse_search_active,
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
