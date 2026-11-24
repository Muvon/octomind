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
use indicatif::{ProgressBar, ProgressStyle};
use octolib::terminal::{pick_index, Picked};
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
			.map(|dt| dt.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
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
			format!("{}  {} — {}  {}  {}", created, self.name, title, role, model_short)
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
					role: meta.as_ref().and_then(|m| m.role.clone()).or(Some(info.role.clone())),
					model: meta.as_ref().and_then(|m| m.model.clone()).or(Some(info.model.clone())),
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

	let labels: Vec<String> = entries.iter().map(PickerEntry::label).collect();
	let picked = pick_index(
		"Resume session (type to filter, ↑/↓ to move, Enter to resume, Esc for new session)",
		&labels,
		0,
	)?;
	Ok(match picked {
		Picked::Index(i) => Some(entries[i].name.clone()),
		Picked::Back | Picked::Quit => None,
	})
}

#[cfg(test)]
#[path = "picker_tests.rs"]
mod tests;
