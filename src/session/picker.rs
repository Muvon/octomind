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
use std::io::Write;

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
	let sessions = crate::session::list_available_sessions()?;
	if sessions.is_empty() {
		return Ok(None);
	}

	let entries: Vec<PickerEntry> = sessions
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
		.collect();

	run_picker_loop(entries)
}

fn run_picker_loop(entries: Vec<PickerEntry>) -> Result<Option<String>> {
	let matcher = SkimMatcherV2::default();
	let mut query = String::new();
	let mut selected: usize = 0;
	let mut stdout = std::io::stdout();

	terminal::enable_raw_mode()?;
	let result = (|| -> Result<Option<String>> {
		execute!(stdout, terminal::EnterAlternateScreen, cursor::Hide)?;
		loop {
			// Rank entries against the current query; empty query keeps the
			// natural order (already newest-first from list_available_sessions).
			let filtered: Vec<&PickerEntry> = if query.is_empty() {
				entries.iter().collect()
			} else {
				let mut scored: Vec<(i64, &PickerEntry)> = entries
					.iter()
					.filter_map(|e| matcher.fuzzy_match(&e.label(), &query).map(|s| (s, e)))
					.collect();
				scored.sort_by(|a, b| b.0.cmp(&a.0));
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
			write!(stdout, "> {}\r\n\r\n", query)?;

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
			stdout.flush()?;

			if let Event::Key(KeyEvent {
				code, modifiers, ..
			}) = event::read()?
			{
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
					KeyCode::Backspace => {
						query.pop();
						selected = 0;
					}
					KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
						return Ok(None)
					}
					KeyCode::Char(c) => {
						query.push(c);
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
