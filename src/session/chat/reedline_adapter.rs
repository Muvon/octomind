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

// Reedline adapter for existing CommandCompleter logic

use crate::config::Config;
use nu_ansi_term::{Color, Style};
use reedline::{
	CommandLineSearch, Completer, Highlighter, Hinter, History, SearchFilter, SearchQuery, Span,
	StyledText, Suggestion,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::Mutex;

/// Reedline adapter that reuses existing CommandCompleter logic
pub struct ReedlineAdapter {
	config: Arc<Config>,
	role: String,
	last_hint: String,
	buffer_empty: Arc<AtomicBool>,
	hint_available: Arc<AtomicBool>,
	line_state: Arc<Mutex<LineState>>,
}

impl ReedlineAdapter {
	pub fn new(
		config: Arc<Config>,
		role: impl Into<String>,
		buffer_empty: Arc<AtomicBool>,
		hint_available: Arc<AtomicBool>,
		line_state: Arc<Mutex<LineState>>,
	) -> Self {
		Self {
			config,
			role: role.into(),
			last_hint: String::new(),
			buffer_empty,
			hint_available,
			line_state,
		}
	}
}

impl Completer for ReedlineAdapter {
	fn complete(&mut self, line: &str, pos: usize) -> Vec<Suggestion> {
		let completer =
			crate::session::chat_helper::CommandCompleter::new(self.config.as_ref(), &self.role);
		let (start_pos, candidates) = completer.complete(line, pos);

		let span_start = start_pos.min(pos);
		let dim_style = Some(Style::new().dimmed());
		candidates
			.into_iter()
			.map(|pair| {
				let replacement = pair.replacement;
				let display = pair.display;
				let description = if display.is_empty() || display == replacement {
					None
				} else {
					Some(display)
				};
				Suggestion {
					value: replacement,
					description,
					style: dim_style,
					span: Span::new(span_start, pos),
					append_whitespace: false,
					..Default::default()
				}
			})
			.collect()
	}
}

impl Highlighter for ReedlineAdapter {
	fn highlight(&self, line: &str, cursor: usize) -> StyledText {
		std::hint::black_box(cursor);
		if !line.starts_with('/') {
			let mut styled = StyledText::new();
			styled.push((Style::new(), line.to_string()));
			return styled;
		}

		let mut styled = StyledText::new();
		let command_end = line.find(char::is_whitespace).unwrap_or(line.len());
		let command = &line[..command_end];
		let remainder = &line[command_end..];

		let is_valid_command = crate::session::chat::COMMANDS
			.iter()
			.any(|cmd| *cmd == command || cmd.starts_with(command));
		let command_style = if is_valid_command {
			Style::new().fg(Color::Green)
		} else {
			Style::new()
		};

		styled.push((command_style, command.to_string()));
		if !remainder.is_empty() {
			styled.push((Style::new(), remainder.to_string()));
		}
		styled
	}
}

impl Hinter for ReedlineAdapter {
	fn handle(
		&mut self,
		line: &str,
		pos: usize,
		history: &dyn History,
		use_ansi_coloring: bool,
		cwd: &str,
	) -> String {
		if let Ok(mut state) = self.line_state.lock() {
			state.buffer = line.to_string();
			state.cursor = pos;
		}
		std::hint::black_box(history);
		std::hint::black_box(cwd);
		let hint = if line.starts_with('/') {
			let completer = crate::session::chat_helper::CommandCompleter::new(
				self.config.as_ref(),
				&self.role,
			);
			completer.hint(line).unwrap_or_default()
		} else {
			self.history_hint(line, history)
		};
		self.last_hint = hint.clone();
		self.buffer_empty.store(line.is_empty(), Ordering::SeqCst);
		self.hint_available
			.store(!hint.is_empty(), Ordering::SeqCst);
		if use_ansi_coloring && !hint.is_empty() {
			Style::new().dimmed().paint(hint).to_string()
		} else {
			hint
		}
	}

	fn complete_hint(&self) -> String {
		self.last_hint.clone()
	}

	fn next_hint_token(&self) -> String {
		self.last_hint
			.split_whitespace()
			.next()
			.unwrap_or("")
			.to_string()
	}
}

#[derive(Debug, Default)]
pub struct LineState {
	pub buffer: String,
	pub cursor: usize,
	/// When true, signals that the user pressed Ctrl+G to add message without sending
	pub add_without_sending: bool,
}
impl ReedlineAdapter {
	fn history_hint(&self, line: &str, history: &dyn History) -> String {
		if line.is_empty() {
			return String::new();
		}
		let filter =
			SearchFilter::from_text_search(CommandLineSearch::Prefix(line.to_string()), None);
		let query = SearchQuery::last_with_search(filter);
		let Ok(results) = history.search(query) else {
			return String::new();
		};
		let Some(item) = results.first() else {
			return String::new();
		};
		let command = &item.command_line;
		command
			.strip_prefix(line)
			.map(str::to_string)
			.unwrap_or_default()
	}
}
