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

/// A single scheduled task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleEntry {
	/// Short unique ID (first 8 chars of UUID).
	pub id: String,
	/// Human-readable description of what this task is about.
	pub description: String,
	/// Exact text that will be injected verbatim as a user message when triggered.
	pub message: String,
	/// When to fire this entry.
	pub trigger_at: DateTime<Local>,
	/// When this entry was created.
	pub created_at: DateTime<Local>,
	/// If set, the entry repeats every this many seconds after firing.
	pub interval_secs: Option<i64>,
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
		}
	}

	/// Create a rescheduled copy of this entry with a new ID and bumped trigger_at.
	/// Only valid when interval_secs is Some — caller must check before calling.
	pub fn reschedule(&self) -> Self {
		let secs = self
			.interval_secs
			.expect("reschedule called on non-repeating entry");
		let id = Uuid::new_v4().to_string()[..8].to_string();
		Self {
			id,
			description: self.description.clone(),
			message: self.message.clone(),
			trigger_at: self.trigger_at + Duration::seconds(secs),
			created_at: Local::now(),
			interval_secs: self.interval_secs,
		}
	}

	/// Human-friendly countdown string, e.g. "in 1h 23m" or "in 45s".
	pub fn countdown(&self) -> String {
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

	/// Pop the earliest entry that is due (trigger_at <= now). Returns None if nothing is due.
	pub fn pop_due(&mut self) -> Option<ScheduleEntry> {
		let now = Local::now();
		if self
			.entries
			.first()
			.map(|e| e.trigger_at <= now)
			.unwrap_or(false)
		{
			Some(self.entries.remove(0))
		} else {
			None
		}
	}

	/// Duration until the next entry fires. Returns None if the store is empty.
	pub fn next_due_duration(&self) -> Option<std::time::Duration> {
		let now = Local::now();
		self.entries.first().map(|e| {
			let diff = e.trigger_at.signed_duration_since(now);
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

	for ch in s.chars() {
		if ch.is_ascii_digit() {
			num_buf.push(ch);
		} else {
			let n: i64 = if num_buf.is_empty() {
				bail!("expected number before '{}'", ch)
			} else {
				num_buf.parse()?
			};
			num_buf.clear();
			match ch {
				'h' => total += n * 3600,
				'm' => total += n * 60,
				's' => total += n,
				_ => bail!("unknown unit '{}' — use h, m, or s", ch),
			}
		}
	}

	if !num_buf.is_empty() {
		bail!(
			"trailing number '{}' without unit (use h, m, or s)",
			num_buf
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
	// Strip am/pm suffix.
	let (s, pm) = if let Some(stripped) = s.strip_suffix("pm") {
		(stripped, true)
	} else if let Some(stripped) = s.strip_suffix("am") {
		(stripped, false)
	} else {
		(s, false)
	};

	let parts: Vec<&str> = s.split(':').collect();
	let mut hour: u32 = parts
		.first()
		.ok_or_else(|| anyhow::anyhow!("invalid time"))?
		.parse()?;
	let minute: u32 = parts.get(1).unwrap_or(&"0").parse()?;
	let second: u32 = parts.get(2).unwrap_or(&"0").parse()?;

	if pm && hour != 12 {
		hour += 12;
	} else if !pm && hour == 12 {
		// 12am = midnight
		hour = 0;
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
mod tests {
	use super::*;
	use chrono::{Datelike, Timelike};

	#[test]
	fn test_parse_now() {
		let t = parse_when("now").unwrap();
		let diff = t
			.signed_duration_since(Local::now())
			.num_milliseconds()
			.abs();
		assert!(diff < 100, "expected ~0ms, got {}", diff);
	}

	#[test]
	fn test_parse_now_case_insensitive() {
		assert!(parse_when("NOW").is_ok());
		assert!(parse_when("  Now  ").is_ok());
	}

	#[test]
	fn test_parse_relative_minutes() {
		let t = parse_when("in 5m").unwrap();
		let diff = t.signed_duration_since(Local::now()).num_seconds();
		assert!((295..=305).contains(&diff), "expected ~300s, got {}", diff);
	}

	#[test]
	fn test_parse_relative_hours() {
		let t = parse_when("in 2h").unwrap();
		let diff = t.signed_duration_since(Local::now()).num_seconds();
		assert!(
			(7195..=7205).contains(&diff),
			"expected ~7200s, got {}",
			diff
		);
	}

	#[test]
	fn test_parse_relative_combined() {
		let t = parse_when("in 1h30m").unwrap();
		let diff = t.signed_duration_since(Local::now()).num_seconds();
		assert!(
			(5395..=5405).contains(&diff),
			"expected ~5400s, got {}",
			diff
		);
	}

	#[test]
	fn test_parse_relative_with_spaces() {
		let t = parse_when("in 1h 30m 10s").unwrap();
		let diff = t.signed_duration_since(Local::now()).num_seconds();
		assert!(
			(5405..=5415).contains(&diff),
			"expected ~5410s, got {}",
			diff
		);
	}

	#[test]
	fn test_parse_relative_seconds() {
		let t = parse_when("in 90s").unwrap();
		let diff = t.signed_duration_since(Local::now()).num_seconds();
		assert!((88..=92).contains(&diff), "expected ~90s, got {}", diff);
	}

	#[test]
	fn test_parse_absolute_datetime() {
		let t = parse_when("2099-12-31 23:59").unwrap();
		assert_eq!(t.year(), 2099);
		assert_eq!(t.month(), 12);
		assert_eq!(t.day(), 31);
		assert_eq!(t.hour(), 23);
		assert_eq!(t.minute(), 59);
	}

	#[test]
	fn test_parse_invalid_relative() {
		assert!(parse_when("in 5x").is_err());
		assert!(parse_when("in ").is_err());
		assert!(parse_when("in 0m").is_err());
	}

	#[test]
	fn test_store_pop_due() {
		let mut store = ScheduleStore::new();
		let past = Local::now() - Duration::seconds(1);
		let entry = ScheduleEntry {
			id: "test0001".to_string(),
			description: "test".to_string(),
			message: "hello".to_string(),
			trigger_at: past,
			created_at: Local::now(),
			interval_secs: None,
		};
		store.add(entry);
		assert!(store.pop_due().is_some());
		assert!(store.is_empty());
	}

	#[test]
	fn test_store_not_due_yet() {
		let mut store = ScheduleStore::new();
		let future = Local::now() + Duration::seconds(3600);
		let entry = ScheduleEntry {
			id: "test0002".to_string(),
			description: "test".to_string(),
			message: "hello".to_string(),
			trigger_at: future,
			created_at: Local::now(),
			interval_secs: None,
		};
		store.add(entry);
		assert!(store.pop_due().is_none());
		assert!(!store.is_empty());
	}

	#[test]
	fn test_store_sorted_by_trigger() {
		let mut store = ScheduleStore::new();
		let later = Local::now() + Duration::seconds(7200);
		let sooner = Local::now() + Duration::seconds(3600);
		store.add(ScheduleEntry {
			id: "late0001".to_string(),
			description: "later".to_string(),
			message: "b".to_string(),
			trigger_at: later,
			created_at: Local::now(),
			interval_secs: None,
		});
		store.add(ScheduleEntry {
			id: "soon0001".to_string(),
			description: "sooner".to_string(),
			message: "a".to_string(),
			trigger_at: sooner,
			created_at: Local::now(),
			interval_secs: None,
		});
		// First entry should be the sooner one.
		assert_eq!(store.entries()[0].id, "soon0001");
	}
}
