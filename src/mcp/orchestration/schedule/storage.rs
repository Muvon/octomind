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

//! Schedule storage: entries, store operations, and time parsing.

use anyhow::{bail, Result};
use chrono::{DateTime, Duration, Local, NaiveTime};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// How an entry decides to fire.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum TriggerMode {
	/// Fire at `trigger_at` (and repeat by `interval_secs` if set).
	#[default]
	Time,
	/// Fire when the session becomes idle. `trigger_at` is a placeholder.
	/// `interval_secs = Some(_)` means "fire every idle"; `None` means one-shot.
	Idle,
}

/// A single scheduled task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleEntry {
	/// Short unique ID (first 8 chars of UUID).
	pub id: String,
	/// Human-readable description of what this task is about.
	pub description: String,
	/// Exact text that will be injected verbatim as a user message when triggered.
	pub message: String,
	/// When to fire this entry. Meaningless when `trigger_mode == Idle`.
	pub trigger_at: DateTime<Local>,
	/// When this entry was created.
	pub created_at: DateTime<Local>,
	/// For time mode: repeat every this many seconds after firing.
	/// For idle mode: any `Some(_)` means "repeat on every idle"; `None` is one-shot.
	pub interval_secs: Option<i64>,
	/// How this entry fires. Defaults to `Time` for back-compat with old snapshots.
	#[serde(default)]
	pub trigger_mode: TriggerMode,
}

impl ScheduleEntry {
	pub fn new(
		description: String,
		message: String,
		trigger_at: DateTime<Local>,
		interval_secs: Option<i64>,
	) -> Self {
		let id = Uuid::new_v4().to_string()[..8].to_string();
		Self {
			id,
			description,
			message,
			trigger_at,
			created_at: Local::now(),
			interval_secs,
			trigger_mode: TriggerMode::Time,
		}
	}

	/// Build an idle-mode entry. `repeating = true` fires on every idle until removed.
	pub fn new_idle(description: String, message: String, repeating: bool) -> Self {
		let id = Uuid::new_v4().to_string()[..8].to_string();
		Self {
			id,
			description,
			message,
			trigger_at: Local::now(),
			created_at: Local::now(),
			interval_secs: if repeating { Some(0) } else { None },
			trigger_mode: TriggerMode::Idle,
		}
	}

	/// Create a rescheduled copy of this entry, keeping the same ID.
	/// The ID must stay stable: `remove <id>` is the only way to stop a repeating
	/// entry, and the only ID the model ever sees is the one handed out at `add`
	/// time (and echoed on each firing). Minting a fresh ID here made every
	/// repeating entry unstoppable.
	/// Only valid when interval_secs is Some — caller must check before calling.
	pub fn reschedule(&self) -> Self {
		let id = self.id.clone();
		let trigger_at = match self.trigger_mode {
			TriggerMode::Idle => Local::now(),
			TriggerMode::Time => {
				let secs = self
					.interval_secs
					.expect("reschedule called on non-repeating entry");
				let interval = Duration::seconds(secs);
				let now = Local::now();
				// Normally advance one interval from the last trigger. But if we
				// fell behind (session busy past several intervals), don't emit a
				// catch-up burst of identical messages — skip to one interval from
				// now. Also guards against DateTime overflow.
				self.trigger_at
					.checked_add_signed(interval)
					.filter(|next| *next > now)
					.unwrap_or_else(|| now + interval)
			}
		};
		Self {
			id,
			description: self.description.clone(),
			message: self.message.clone(),
			trigger_at,
			created_at: Local::now(),
			interval_secs: self.interval_secs,
			trigger_mode: self.trigger_mode,
		}
	}

	/// Human-friendly countdown string, e.g. "in 1h 23m" or "in 45s".
	/// Idle entries return "when idle".
	pub fn countdown(&self) -> String {
		if self.trigger_mode == TriggerMode::Idle {
			return "when idle".to_string();
		}
		let now = Local::now();
		let diff = self.trigger_at.signed_duration_since(now);
		if diff.num_seconds() <= 0 {
			return "now".to_string();
		}
		let total_secs = diff.num_seconds();
		let hours = total_secs / 3600;
		let mins = (total_secs % 3600) / 60;
		let secs = total_secs % 60;
		if hours > 0 {
			format!("in {}h {}m", hours, mins)
		} else if mins > 0 {
			format!("in {}m {}s", mins, secs)
		} else {
			format!("in {}s", secs)
		}
	}
}

/// In-memory store for scheduled entries. Sorted by trigger_at ascending.
#[derive(Default)]
pub struct ScheduleStore {
	entries: Vec<ScheduleEntry>,
}

impl ScheduleStore {
	pub fn new() -> Self {
		Self::default()
	}

	/// Add a new entry. Returns the entry ID.
	pub fn add(&mut self, entry: ScheduleEntry) -> String {
		let id = entry.id.clone();
		self.entries.push(entry);
		// Keep sorted by trigger time so pop_due and next_due are O(1).
		self.entries.sort_by_key(|e| e.trigger_at);
		id
	}

	/// Remove an entry by ID. Returns true if found and removed.
	pub fn remove(&mut self, id: &str) -> bool {
		let before = self.entries.len();
		self.entries.retain(|e| e.id != id);
		self.entries.len() < before
	}

	/// Edit an existing entry. Only provided fields are updated.
	/// `interval_secs`: Some(Some(x)) = set interval, Some(None) = clear interval, None = no change.
	pub fn edit(
		&mut self,
		id: &str,
		description: Option<String>,
		message: Option<String>,
		trigger_at: Option<DateTime<Local>>,
		interval_secs: Option<Option<i64>>,
	) -> bool {
		let entry = self.entries.iter_mut().find(|e| e.id == id);
		match entry {
			None => false,
			Some(e) => {
				if let Some(d) = description {
					e.description = d;
				}
				if let Some(m) = message {
					e.message = m;
				}
				if let Some(t) = trigger_at {
					e.trigger_at = t;
				}
				if let Some(i) = interval_secs {
					e.interval_secs = i;
				}
				// Re-sort after potential time change.
				self.entries.sort_by_key(|e| e.trigger_at);
				true
			}
		}
	}

	/// Pop the earliest time-mode entry that is due (trigger_at <= now).
	/// Idle entries are ignored — drain them via `pop_idle`.
	pub fn pop_due(&mut self) -> Option<ScheduleEntry> {
		let now = Local::now();
		let idx = self
			.entries
			.iter()
			.position(|e| e.trigger_mode == TriggerMode::Time && e.trigger_at <= now)?;
		Some(self.entries.remove(idx))
	}

	/// Pop the next idle-mode entry. Caller decides when "idle" actually holds.
	pub fn pop_idle(&mut self) -> Option<ScheduleEntry> {
		let idx = self
			.entries
			.iter()
			.position(|e| e.trigger_mode == TriggerMode::Idle)?;
		Some(self.entries.remove(idx))
	}

	/// Returns true if any idle-mode entries are queued.
	pub fn has_idle(&self) -> bool {
		self.entries
			.iter()
			.any(|e| e.trigger_mode == TriggerMode::Idle)
	}

	/// Duration until the next time-mode entry fires. Returns None if no time entries exist.
	pub fn next_due_duration(&self) -> Option<std::time::Duration> {
		let now = Local::now();
		self.entries
			.iter()
			.filter(|e| e.trigger_mode == TriggerMode::Time)
			.map(|e| e.trigger_at)
			.min()
			.map(|t| {
				let diff = t.signed_duration_since(now);
				if diff.num_milliseconds() <= 0 {
					std::time::Duration::ZERO
				} else {
					std::time::Duration::from_millis(diff.num_milliseconds() as u64)
				}
			})
	}

	pub fn is_empty(&self) -> bool {
		self.entries.is_empty()
	}

	pub fn entries(&self) -> &[ScheduleEntry] {
		&self.entries
	}

	/// Replace all entries with the given set, keeping the trigger-time ordering invariant.
	/// Used by session restore to seed the store from a persisted snapshot.
	pub fn seed_entries(&mut self, mut entries: Vec<ScheduleEntry>) {
		entries.sort_by_key(|e| e.trigger_at);
		self.entries = entries;
	}
}

// ---------------------------------------------------------------------------
// Time parsing
// ---------------------------------------------------------------------------

/// Parse a human-readable time expression into an absolute `DateTime<Local>`.
///
/// Supported formats:
/// - `"now"` -- fires on the next scheduler tick (immediately)
/// - Relative: `"in 5m"`, `"in 2h"`, `"in 1h30m"`, `"in 90s"`, `"in 2h 30m 10s"`
/// - Absolute time today: `"15:30"`, `"3:30pm"`, `"9am"` (if past, schedules for tomorrow)
/// - Absolute datetime: `"2026-03-22 15:30"`, `"2026-03-22 15:30:00"`
pub fn parse_when(input: &str) -> Result<DateTime<Local>> {
	let s = input.trim().to_lowercase();

	if s == "now" {
		return Ok(Local::now());
	}

	if let Some(stripped) = s.strip_prefix("in ") {
		return parse_relative(stripped);
	}

	// Try absolute datetime first (contains a space between date and time parts with dashes).
	if s.contains('-') && s.contains(' ') {
		return parse_absolute_datetime(&s);
	}

	// Try absolute time-of-day.
	parse_time_of_day(&s)
}

/// Parse relative duration like `"5m"`, `"2h"`, `"1h30m"`, `"2h 30m 10s"`.
fn parse_relative(s: &str) -> Result<DateTime<Local>> {
	let total_secs = parse_duration_secs(s)?;
	if total_secs == 0 {
		bail!("duration must be greater than zero");
	}
	Ok(Local::now() + Duration::seconds(total_secs))
}

/// Parse a duration string into total seconds.
/// Accepts: `"5m"`, `"2h"`, `"90s"`, `"1h30m"`, `"2h 30m 10s"` (spaces optional).
pub(crate) fn parse_duration_secs(s: &str) -> Result<i64> {
	// Remove spaces so "1h 30m" and "1h30m" both work.
	let s = s.replace(' ', "");
	if s.is_empty() {
		bail!("empty duration");
	}

	let mut total: i64 = 0;
	let mut num_buf = String::new();

	// Cap at ~10 years. Bigger values are always mistakes and, unchecked, would
	// overflow i64 / chrono's Duration and panic the scheduler on a model-supplied
	// string like "999999999999h".
	const MAX_DURATION_SECS: i64 = 10 * 365 * 24 * 3600;
	const FORMAT_HINT: &str =
		"units are single-letter h/m/s — e.g. \"35m\", \"2h\", \"90s\", \"1h30m\" (not \"min\"/\"sec\"/\"hr\")";

	for ch in s.chars() {
		if ch.is_ascii_digit() {
			num_buf.push(ch);
		} else {
			let n: i64 = if num_buf.is_empty() {
				bail!("unexpected '{}' in duration '{}': {}", ch, s, FORMAT_HINT)
			} else {
				num_buf.parse()?
			};
			num_buf.clear();
			let add = match ch {
				'h' => n.checked_mul(3600),
				'm' => n.checked_mul(60),
				's' => Some(n),
				_ => bail!("unknown unit '{}' in duration '{}': {}", ch, s, FORMAT_HINT),
			};
			total = add
				.and_then(|a| total.checked_add(a))
				.filter(|t| *t <= MAX_DURATION_SECS)
				.ok_or_else(|| anyhow::anyhow!("duration '{}' too large (max 10 years)", s))?;
		}
	}

	if !num_buf.is_empty() {
		bail!(
			"trailing number '{}' without unit in duration '{}': {}",
			num_buf,
			s,
			FORMAT_HINT
		);
	}

	Ok(total)
}

/// Parse `"15:30"`, `"3:30pm"`, `"9am"` into today's date at that time.
/// If the time is already past, schedules for tomorrow.
fn parse_time_of_day(s: &str) -> Result<DateTime<Local>> {
	let naive_time = parse_naive_time(s)?;
	let now = Local::now();
	let today = now.date_naive();
	let candidate = today
		.and_time(naive_time)
		.and_local_timezone(Local)
		.single()
		.ok_or_else(|| anyhow::anyhow!("ambiguous local time"))?;

	// If already past, schedule for tomorrow.
	if candidate <= now {
		let tomorrow = today
			.succ_opt()
			.ok_or_else(|| anyhow::anyhow!("date overflow"))?;
		let next = tomorrow
			.and_time(naive_time)
			.and_local_timezone(Local)
			.single()
			.ok_or_else(|| anyhow::anyhow!("ambiguous local time"))?;
		Ok(next)
	} else {
		Ok(candidate)
	}
}

/// Parse time strings: `"15:30"`, `"15:30:00"`, `"3:30pm"`, `"9am"`.
fn parse_naive_time(s: &str) -> Result<NaiveTime> {
	// Strip am/pm suffix. Bare input is 24-hour time — the 12am/12pm special
	// cases must only apply when a meridiem was actually written, or "12:43"
	// silently becomes 00:43.
	let (s, meridiem) = if let Some(stripped) = s.strip_suffix("pm") {
		(stripped, Some(true))
	} else if let Some(stripped) = s.strip_suffix("am") {
		(stripped, Some(false))
	} else {
		(s, None)
	};

	let parts: Vec<&str> = s.split(':').collect();
	let mut hour: u32 = parts
		.first()
		.ok_or_else(|| anyhow::anyhow!("invalid time"))?
		.parse()?;
	let minute: u32 = parts.get(1).unwrap_or(&"0").parse()?;
	let second: u32 = parts.get(2).unwrap_or(&"0").parse()?;

	match meridiem {
		Some(true) if hour != 12 => hour += 12,
		// 12am = midnight
		Some(false) if hour == 12 => hour = 0,
		_ => {}
	}

	NaiveTime::from_hms_opt(hour, minute, second)
		.ok_or_else(|| anyhow::anyhow!("invalid time {}:{}:{}", hour, minute, second))
}

/// Parse `"2026-03-22 15:30"` or `"2026-03-22 15:30:00"`.
fn parse_absolute_datetime(s: &str) -> Result<DateTime<Local>> {
	// Try with seconds first, then without.
	if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S") {
		return dt
			.and_local_timezone(Local)
			.single()
			.ok_or_else(|| anyhow::anyhow!("ambiguous local datetime"));
	}
	if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M") {
		return dt
			.and_local_timezone(Local)
			.single()
			.ok_or_else(|| anyhow::anyhow!("ambiguous local datetime"));
	}
	bail!(
		"could not parse datetime '{}' — expected format: YYYY-MM-DD HH:MM",
		s
	)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "storage_inline_tests.rs"]
mod inline_tests;

#[cfg(test)]
#[path = "storage_tests.rs"]
mod storage_tests;
