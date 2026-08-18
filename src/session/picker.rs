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

// Interactive session picker shown when `octomind` starts in a terminal
// without --resume/--name: a fuzzy-searchable list of recent sessions
// (newest first). Enter resumes the highlighted session, Esc starts fresh.

use anyhow::Result;
use crossterm::{
	cursor,
	event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
	execute, terminal,
};
use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use indicatif::{ProgressBar, ProgressStyle};
use std::io::Write;
use std::time::Duration;

struct PickerEntry {
	name: String,
	title: Option<String>,
	role: Option<String>,
	model: Option<String>,
	created_at: u64,
}

impl PickerEntry {
	/// One-line label used both for display and as the fuzzy-match target.
	fn label(&self) -> String {
		let created = chrono::DateTime::<chrono::Utc>::from_timestamp(self.created_at as i64, 0)
			.map(|dt| dt.naive_local().format("%Y-%m-%d %H:%M").to_string())
			.unwrap_or_default();
		let title = self.title.as_deref().unwrap_or("");
		let role = self.role.as_deref().unwrap_or("");
		let model_short = self
			.model
			.as_deref()
			.and_then(|m| m.split('/').next_back())
			.unwrap_or("");
		if title.is_empty() {
			format!("{}  {}  {}  {}", created, self.name, role, model_short)
		} else {
			format!(
				"{}  {} — {}  {}  {}",
				created, self.name, title, role, model_short
			)
		}
	}
}

/// Show the picker. Returns `Some(session_name)` to resume, or `None` when the
/// user chose to start a brand-new session. Returns `Ok(None)` silently when
/// there is nothing to pick from.
pub fn pick_session() -> Result<Option<String>> {
	// Listing sessions reads every session's metadata — show a spinner so the
	// terminal isn't blank until the picker renders.
	let spinner = ProgressBar::new_spinner();
	spinner.set_style(
		ProgressStyle::default_spinner()
			.template(" {spinner:.cyan} {msg:.cyan}")
			.unwrap()
			.tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧"),
	);
	spinner.set_message("Loading sessions...");
	spinner.enable_steady_tick(Duration::from_millis(80));

	let entries = crate::session::list_available_sessions().map(|sessions| {
		sessions
			.into_iter()
			.map(|(name, info)| {
				let meta = crate::session::titles::get_session_meta(&name);
				PickerEntry {
					name,
					title: meta.as_ref().and_then(|m| m.title.clone()),
					role: meta
						.as_ref()
						.and_then(|m| m.role.clone())
						.or(Some(info.role.clone())),
					model: meta
						.as_ref()
						.and_then(|m| m.model.clone())
						.or(Some(info.model.clone())),
					created_at: info.created_at,
				}
			})
			.collect::<Vec<PickerEntry>>()
	});
	spinner.finish_and_clear();
	print!("\x1B[2K\r");
	std::io::stdout().flush().ok();

	let entries = entries?;
	if entries.is_empty() {
		return Ok(None);
	}

	run_picker_loop(entries)
}

fn run_picker_loop(entries: Vec<PickerEntry>) -> Result<Option<String>> {
	let matcher = SkimMatcherV2::default();
	let mut query: Vec<char> = Vec::new();
	let mut cursor_pos: usize = 0; // char index into query, 0..=len
	let mut selected: usize = 0;
	let mut stdout = std::io::stdout();

	terminal::enable_raw_mode()?;
	let result = (|| -> Result<Option<String>> {
		execute!(stdout, terminal::EnterAlternateScreen, cursor::Hide)?;
		loop {
			let query_str: String = query.iter().collect();
			// Rank entries against the current query; empty query keeps the
			// natural order (already newest-first from list_available_sessions).
			let filtered: Vec<&PickerEntry> = if query.is_empty() {
				entries.iter().collect()
			} else {
				let mut scored: Vec<(i64, &PickerEntry)> = entries
					.iter()
					.filter_map(|e| matcher.fuzzy_match(&e.label(), &query_str).map(|s| (s, e)))
					.collect();
				scored.sort_by_key(|b| std::cmp::Reverse(b.0));
				scored.into_iter().map(|(_, e)| e).collect()
			};
			if selected >= filtered.len() {
				selected = filtered.len().saturating_sub(1);
			}

			execute!(
				stdout,
				terminal::Clear(terminal::ClearType::All),
				cursor::MoveTo(0, 0)
			)?;
			write!(stdout, "Resume session (type to filter, ↑/↓ to move, Enter to resume, Esc for new session)\r\n")?;
			write!(stdout, "> {}\r\n\r\n", query_str)?;

			let rows = terminal::size().map(|(_, h)| h as usize).unwrap_or(24);
			let max_visible = rows.saturating_sub(5);
			// Keep the selection inside the visible window.
			let offset = if selected >= max_visible {
				selected + 1 - max_visible
			} else {
				0
			};
			for (i, entry) in filtered.iter().skip(offset).take(max_visible).enumerate() {
				let idx = offset + i;
				if idx == selected {
					write!(stdout, "\x1b[7m> {}\x1b[0m\r\n", entry.label())?;
				} else {
					write!(stdout, "  {}\r\n", entry.label())?;
				}
			}
			if filtered.is_empty() {
				write!(stdout, "  (no matches — Enter starts a new session)\r\n")?;
			}
			// Place the cursor in the query line at its edit position.
			execute!(
				stdout,
				cursor::Show,
				cursor::MoveTo(2 + cursor_pos as u16, 1)
			)?;
			stdout.flush()?;

			if let Event::Key(KeyEvent {
				code, modifiers, ..
			}) = event::read()?
			{
				let ctrl = modifiers.contains(KeyModifiers::CONTROL);
				match code {
					KeyCode::Esc => return Ok(None),
					KeyCode::Enter => {
						return Ok(filtered.get(selected).map(|e| e.name.clone()));
					}
					KeyCode::Up => selected = selected.saturating_sub(1),
					KeyCode::Down => {
						if selected + 1 < filtered.len() {
							selected += 1;
						}
					}
					// ── readline-style editing ─────────────────────────
					KeyCode::Char('c') if ctrl => return Ok(None),
					KeyCode::Char('d') if ctrl => return Ok(None), // exit, not resume
					KeyCode::Char('n') if ctrl => {
						if selected + 1 < filtered.len() {
							selected += 1;
						}
					}
					KeyCode::Char('p') if ctrl => selected = selected.saturating_sub(1),
					KeyCode::Char('a') if ctrl => cursor_pos = 0,
					KeyCode::Char('e') if ctrl => cursor_pos = query.len(),
					KeyCode::Char('u') if ctrl => {
						query.drain(..cursor_pos);
						cursor_pos = 0;
						selected = 0;
					}
					KeyCode::Char('k') if ctrl => {
						query.truncate(cursor_pos);
						selected = 0;
					}
					KeyCode::Char('w') if ctrl => {
						// kill word before cursor: trailing spaces, then the word
						let mut start = cursor_pos;
						while start > 0 && query[start - 1] == ' ' {
							start -= 1;
						}
						while start > 0 && query[start - 1] != ' ' {
							start -= 1;
						}
						query.drain(start..cursor_pos);
						cursor_pos = start;
						selected = 0;
					}
					KeyCode::Left => cursor_pos = cursor_pos.saturating_sub(1),
					KeyCode::Right => cursor_pos = (cursor_pos + 1).min(query.len()),
					KeyCode::Home => cursor_pos = 0,
					KeyCode::End => cursor_pos = query.len(),
					KeyCode::Backspace => {
						if cursor_pos > 0 {
							query.remove(cursor_pos - 1);
							cursor_pos -= 1;
							selected = 0;
						}
					}
					KeyCode::Delete => {
						if cursor_pos < query.len() {
							query.remove(cursor_pos);
							selected = 0;
						}
					}
					KeyCode::Char(c) if !ctrl => {
						query.insert(cursor_pos, c);
						cursor_pos += 1;
						selected = 0;
					}
					_ => {}
				}
			}
		}
	})();
	execute!(stdout, cursor::Show, terminal::LeaveAlternateScreen)?;
	terminal::disable_raw_mode()?;
	result
}
