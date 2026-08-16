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

// Session titles store: a JSON map of session name → metadata (title, role,
// model) kept next to the session files for O(1) lookup. Surfaces: the
// interactive picker, `/rename`, `/list`, `/info`, and the terminal title.
// Missing entries are never a blocker — a session without a metadata row
// behaves exactly as before this store existed.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Hard cap on a session title (same spirit as a web page title).
pub const MAX_TITLE_CHARS: usize = 160;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionMeta {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub title: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub role: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub model: Option<String>,
}

fn titles_file() -> Result<PathBuf> {
	Ok(crate::directories::get_sessions_dir()?.join("titles.json"))
}

fn load_map() -> HashMap<String, SessionMeta> {
	let Ok(path) = titles_file() else {
		return HashMap::new();
	};
	let Ok(content) = std::fs::read_to_string(path) else {
		return HashMap::new();
	};
	// A corrupt store must not block session startup — fall back to empty.
	serde_json::from_str(&content).unwrap_or_default()
}

fn save_map(map: &HashMap<String, SessionMeta>) -> Result<()> {
	let path = titles_file()?;
	let content = serde_json::to_string_pretty(map)?;
	std::fs::write(path, content)?;
	Ok(())
}

/// Look up metadata for a session. O(1) on the parsed map; missing → default.
pub fn get_session_meta(session_name: &str) -> Option<SessionMeta> {
	load_map().get(session_name).cloned()
}

/// Set (or clear with None) the display title of a session, capped at
/// MAX_TITLE_CHARS. Also refreshes the stored role/model snapshot.
pub fn set_session_title(
	session_name: &str,
	title: Option<&str>,
	role: Option<&str>,
	model: Option<&str>,
) -> Result<Option<String>> {
	let mut map = load_map();
	let entry = map.entry(session_name.to_string()).or_default();
	let applied = title.map(|t| {
		let trimmed = t.trim();
		trimmed.chars().take(MAX_TITLE_CHARS).collect::<String>()
	});
	entry.title = applied.clone().filter(|t| !t.is_empty());
	if let Some(r) = role {
		entry.role = Some(r.to_string());
	}
	if let Some(m) = model {
		entry.model = Some(m.to_string());
	}
	save_map(&map)?;
	Ok(applied.filter(|t| !t.is_empty()))
}

/// Record the role/model a session ran with (called on session start/resume
/// so the picker can show and restore them even without an explicit title).
pub fn record_session_meta(session_name: &str, role: &str, model: &str) {
	let mut map = load_map();
	let entry = map.entry(session_name.to_string()).or_default();
	entry.role = Some(role.to_string());
	entry.model = Some(model.to_string());
	// Best-effort bookkeeping — never fail session startup over the sidecar.
	if let Err(e) = save_map(&map) {
		crate::log_debug!("Failed to update titles.json: {}", e);
	}
}
