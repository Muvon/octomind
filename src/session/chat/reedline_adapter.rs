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

// Reedline adapter for existing CommandCompleter logic

use crate::config::Config;
use crate::session::chat::session::commands::{age, matches_words, quote, Reply};
use nu_ansi_term::{Color, Style};
use reedline::{
	CommandLineSearch, Completer, CompletionResult, Highlighter, Hinter, History, SearchFilter,
	SearchQuery, Span, StyledText, Suggestion,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::Mutex;

/// Most lines the `>` picker lists at once; typing filters older lines in.
const QUOTE_PICKER_LIMIT: usize = 50;
/// Reedline adapter that reuses existing CommandCompleter logic
pub struct ReedlineAdapter {
	config: Arc<Config>,
	role: String,
	last_hint: String,
	buffer_empty: Arc<AtomicBool>,
	hint_available: Arc<AtomicBool>,
	line_state: Arc<Mutex<LineState>>,
	/// Assistant replies, newest first, listed by the `/reply` and `>` pickers.
	replies: Arc<Vec<Reply>>,
}

impl ReedlineAdapter {
	pub fn new(
		config: Arc<Config>,
		role: impl Into<String>,
		buffer_empty: Arc<AtomicBool>,
		hint_available: Arc<AtomicBool>,
		line_state: Arc<Mutex<LineState>>,
		replies: Arc<Vec<Reply>>,
	) -> Self {
		Self {
			config,
			role: role.into(),
			last_hint: String::new(),
			buffer_empty,
			hint_available,
			line_state,
			replies,
		}
	}

	/// Paint `remainder` — the text after the command token. The first argument
	/// is validated against the command's accepted set: green when it matches,
	/// red when it does not. The rest of the line stays plain.
	fn push_remainder(styled: &mut StyledText, command: &str, remainder: &str) {
		let Some(accepted) =
			crate::session::chat_helper::CommandCompleter::argument_candidates(command)
		else {
			styled.push((Style::new(), remainder.to_string()));
			return;
		};

		let trimmed = remainder.trim_start();
		let argument_end = trimmed.find(char::is_whitespace).unwrap_or(trimmed.len());
		let (argument, rest) = trimmed.split_at(argument_end);
		let leading = &remainder[..remainder.len() - trimmed.len()];

		if !leading.is_empty() {
			styled.push((Style::new(), leading.to_string()));
		}
		if !argument.is_empty() {
			let argument_lower = argument.to_lowercase();
			let argument_style = if accepted
				.iter()
				.any(|candidate| candidate.starts_with(&argument_lower))
			{
				Style::new().fg(Color::Green)
			} else {
				Style::new().fg(Color::Red)
			};
			styled.push((argument_style, argument.to_string()));
		}
		if !rest.is_empty() {
			styled.push((Style::new(), rest.to_string()));
		}
	}

	/// Suggestions for the reply pickers, or `None` when the cursor is in
	/// neither: `/reply <query>` lists replies by number, and a line starting
	/// with `>` lists reply lines to quote.
	fn reply_suggestions(&self, line: &str, pos: usize) -> Option<Vec<Suggestion>> {
		let pos = crate::utils::truncation::floor_char_boundary(line, pos.min(line.len()));
		if let Some(query) = line[..pos].strip_prefix("/reply ") {
			return Some(self.reply_number_suggestions(query, pos));
		}
		let line_start = line[..pos].rfind('\n').map_or(0, |index| index + 1);
		let query = line[line_start..pos].strip_prefix('>')?;
		Some(self.quote_line_suggestions(query, Span::new(line_start, pos)))
	}

	/// Replies matching `query` by words, or by number prefix when it is a
	/// number. Each row reads like the reply itself — when it was sent, how
	/// long it is, how it opens — so a person picks by content; the number
	/// only travels in the buffer as `/reply N`.
	fn reply_number_suggestions(&self, query: &str, pos: usize) -> Vec<Suggestion> {
		let numeric = query.trim().parse::<usize>().is_ok();
		let now = crate::utils::time::now_secs();
		self.replies
			.iter()
			.zip(1usize..)
			.filter(|(reply, number)| {
				if numeric {
					number.to_string().starts_with(query.trim())
				} else {
					matches_words(&reply.text, query)
				}
			})
			.map(|(reply, number)| Suggestion {
				value: number.to_string(),
				display_override: Some(format!(
					"{:<8} {:>9}  {}",
					age(reply.timestamp, now),
					reply.size(),
					reply.preview()
				)),
				..picker_row(Span::new(pos - query.len(), pos))
			})
			.collect()
	}

	/// Distinct reply lines containing every word of `query`, newest first,
	/// each led by its reply's age. Selecting one replaces the `>` line with
	/// the quoted line and moves to a fresh line, ready for another `>` or the
	/// user's answer.
	fn quote_line_suggestions(&self, query: &str, span: Span) -> Vec<Suggestion> {
		let now = crate::utils::time::now_secs();
		let mut seen = std::collections::HashSet::new();
		self.replies
			.iter()
			.flat_map(|reply| {
				let sent = age(reply.timestamp, now);
				reply
					.text
					.lines()
					.map(move |text| (text.trim(), sent.clone()))
			})
			// A lone code fence carries nothing worth quoting.
			.filter(|(text, _)| !text.is_empty() && !text.starts_with("```"))
			.filter(|(text, _)| matches_words(text, query) && seen.insert(*text))
			.take(QUOTE_PICKER_LIMIT)
			.map(|(text, sent)| Suggestion {
				value: format!("{}\n", quote(text)),
				display_override: Some(format!("{:<8}  {}", sent, text)),
				..picker_row(span)
			})
			.collect()
	}
}

/// Shared shape of a reply-picker row.
fn picker_row(span: Span) -> Suggestion {
	Suggestion {
		// Any description switches the menu to one row per suggestion; left
		// empty because the row text fills the width.
		description: Some(String::new()),
		// Default text, not the dim used for commands: these rows are prose
		// a person reads to choose.
		style: Some(Style::new()),
		span,
		append_whitespace: false,
		// Typed words are scattered through the row; reedline's fallback would
		// highlight a stray substring instead.
		match_indices: Some(Vec::new()),
		..Default::default()
	}
}

impl Completer for ReedlineAdapter {
	fn complete(&mut self, line: &str, pos: usize) -> CompletionResult {
		if let Some(suggestions) = self.reply_suggestions(line, pos) {
			return CompletionResult::fresh(suggestions);
		}
		let completer =
			crate::session::chat_helper::CommandCompleter::new(self.config.as_ref(), &self.role);
		let (start_pos, candidates) = completer.complete(line, pos);

		let span_start = start_pos.min(pos);
		let dim_style = Some(Style::new().dimmed());
		let suggestions: Vec<Suggestion> = candidates
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
			.collect();
		CompletionResult::fresh(suggestions)
	}
}

impl Highlighter for ReedlineAdapter {
	fn highlight(&self, line: &str, cursor: usize) -> StyledText {
		std::hint::black_box(cursor);
		if !line.starts_with('/') {
			// Quoted lines render dim so the user's own answer stands out.
			let mut styled = StyledText::new();
			for segment in line.split_inclusive('\n') {
				let style = if segment.starts_with('>') {
					Style::new().dimmed()
				} else {
					Style::new()
				};
				styled.push((style, segment.to_string()));
			}
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
		Self::push_remainder(&mut styled, command, remainder);
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
		} else if line.starts_with('>') {
			// A quote buffer: a past quoted message's tail would only cover the
			// `>` picker.
			String::new()
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

/// A blob captured from the clipboard via Ctrl+V while the user keeps typing.
/// Drained from `LineState` after a successful Reedline read and applied to
/// the active `ChatSession` (sets `pending_image` / `pending_video`).
#[derive(Debug, Clone)]
pub enum PendingClipboardItem {
	Image(crate::session::image::ImageAttachment),
	Video(crate::session::video::VideoAttachment),
}

#[derive(Debug, Default)]
pub struct LineState {
	pub buffer: String,
	pub cursor: usize,
	/// When true, signals that the user pressed Ctrl+G to add message without sending
	pub add_without_sending: bool,
	/// Clipboard blobs auto-attached via Ctrl+V; consumed by the input loop on submit.
	pub pending_clipboard: Vec<PendingClipboardItem>,
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

#[cfg(test)]
#[path = "reedline_adapter_tests.rs"]
mod tests;
