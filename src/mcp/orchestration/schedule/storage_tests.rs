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

use super::*;
use chrono::{Datelike, Timelike};

/// Minimal time-mode entry with an explicit id and trigger time.
fn time_entry(id: &str, trigger_at: DateTime<Local>) -> ScheduleEntry {
	ScheduleEntry {
		id: id.to_string(),
		description: format!("desc-{id}"),
		message: format!("msg-{id}"),
		trigger_at,
		created_at: Local::now(),
		interval_secs: None,
		trigger_mode: TriggerMode::Time,
	}
}

#[test]
fn new_mints_short_unique_ids_and_defaults_to_time_mode() {
	let a = ScheduleEntry::new("a".into(), "m".into(), Local::now(), None);
	let b = ScheduleEntry::new("b".into(), "m".into(), Local::now(), Some(60));
	assert_eq!(a.id.len(), 8);
	assert_eq!(b.id.len(), 8);
	assert_ne!(a.id, b.id);
	assert_eq!(a.trigger_mode, TriggerMode::Time);
	assert_eq!(b.trigger_mode, TriggerMode::Time);
	assert_eq!(a.interval_secs, None);
	assert_eq!(b.interval_secs, Some(60));
	assert!(
		a.created_at
			.signed_duration_since(Local::now())
			.num_seconds()
			.abs() <= 5
	);
}

#[test]
fn new_idle_encodes_repeating_via_interval_flag() {
	let one_shot = ScheduleEntry::new_idle("d".into(), "m".into(), false);
	let repeating = ScheduleEntry::new_idle("d".into(), "m".into(), true);
	assert_eq!(one_shot.trigger_mode, TriggerMode::Idle);
	assert_eq!(repeating.trigger_mode, TriggerMode::Idle);
	assert_eq!(one_shot.interval_secs, None, "one-shot idle entry");
	assert_eq!(repeating.interval_secs, Some(0), "repeating idle entry");
}

#[test]
fn countdown_formats_by_magnitude() {
	let idle = ScheduleEntry::new_idle("d".into(), "m".into(), false);
	assert_eq!(idle.countdown(), "when idle");

	assert_eq!(
		time_entry("p", Local::now() - Duration::seconds(60)).countdown(),
		"now"
	);
	// countdown() samples Local::now() itself, so a moment elapses between
	// building the entry and formatting it — assert the format category with
	// ±2s of slack instead of the exact second count.
	let secs = time_entry("s", Local::now() + Duration::seconds(50)).countdown();
	assert!(
		secs.starts_with("in ") && secs.ends_with('s') && !secs.contains('m'),
		"seconds magnitude: {secs}"
	);
	let mins = time_entry("m", Local::now() + Duration::seconds(150)).countdown();
	assert!(
		mins.starts_with("in 2m ") && mins.ends_with('s'),
		"minutes magnitude: {mins}"
	);
	// 7300s = 2h 1m 40s; seconds are not rendered in the hours magnitude, so
	// ±2s of elapsed time cannot change the expected string.
	assert_eq!(
		time_entry("h", Local::now() + Duration::seconds(7300)).countdown(),
		"in 2h 1m"
	);
}

#[test]
fn reschedule_keeps_id_and_advances_exactly_one_interval() {
	let mut e = time_entry("abc12345", Local::now() + Duration::seconds(100));
	e.interval_secs = Some(3600);
	let before = e.trigger_at;
	let next = e.reschedule();
	assert_eq!(
		next.id, "abc12345",
		"remove <id> is the only stop handle — id must be stable"
	);
	assert_eq!(next.interval_secs, Some(3600));
	assert_eq!(next.trigger_mode, TriggerMode::Time);
	let advance = next.trigger_at.signed_duration_since(before).num_seconds();
	assert!((3595..=3605).contains(&advance), "advance was {advance}");
}

#[test]
fn reschedule_skips_catch_up_burst_when_overdue() {
	let mut e = time_entry("late0001", Local::now() - Duration::seconds(7200));
	e.interval_secs = Some(3600);
	let next = e.reschedule();
	// trigger_at + interval is still in the past → skip to one interval from now.
	let from_now = next
		.trigger_at
		.signed_duration_since(Local::now())
		.num_seconds();
	assert!((3590..=3610).contains(&from_now), "from_now was {from_now}");
}

#[test]
fn reschedule_idle_entry_resets_trigger_to_now() {
	let mut e = ScheduleEntry::new_idle("d".into(), "m".into(), true);
	e.trigger_at = Local::now() - Duration::hours(3);
	let next = e.reschedule();
	assert_eq!(next.id, e.id);
	assert_eq!(next.trigger_mode, TriggerMode::Idle);
	let drift = next
		.trigger_at
		.signed_duration_since(Local::now())
		.num_seconds()
		.abs();
	assert!(drift <= 5, "drift was {drift}");
}

#[test]
fn serde_roundtrip_preserves_every_field() {
	let mut e = time_entry("roundtrip", Local::now() + Duration::minutes(30));
	e.interval_secs = Some(120);
	let json = serde_json::to_string(&e).expect("serialize");
	let back: ScheduleEntry = serde_json::from_str(&json).expect("deserialize");
	assert_eq!(back.id, e.id);
	assert_eq!(back.description, e.description);
	assert_eq!(back.message, e.message);
	assert_eq!(back.trigger_at, e.trigger_at);
	assert_eq!(back.created_at, e.created_at);
	assert_eq!(back.interval_secs, e.interval_secs);
	assert_eq!(back.trigger_mode, e.trigger_mode);
}

#[test]
fn trigger_mode_serde_is_lowercase_and_defaults_to_time() {
	assert_eq!(
		serde_json::to_value(TriggerMode::Time).unwrap(),
		serde_json::json!("time")
	);
	assert_eq!(
		serde_json::to_value(TriggerMode::Idle).unwrap(),
		serde_json::json!("idle")
	);

	// Back-compat: snapshots written before trigger_mode existed load as Time.
	let now = Local::now().to_rfc3339();
	let legacy = format!(
		r#"{{"id":"legacy01","description":"d","message":"m","trigger_at":"{now}","created_at":"{now}","interval_secs":null}}"#
	);
	let entry: ScheduleEntry = serde_json::from_str(&legacy).expect("legacy snapshot must parse");
	assert_eq!(entry.trigger_mode, TriggerMode::Time);
}

#[test]
fn store_add_remove_roundtrip() {
	let mut store = ScheduleStore::new();
	assert!(store.is_empty());

	let id = store.add(time_entry(
		"addremove",
		Local::now() + Duration::minutes(10),
	));
	assert_eq!(id, "addremove");
	assert_eq!(store.entries().len(), 1);

	assert!(store.remove("addremove"));
	assert!(
		!store.remove("addremove"),
		"second remove must report false"
	);
	assert!(!store.remove("never-existed"));
	assert!(store.is_empty());
}

#[test]
fn store_edit_updates_only_provided_fields() {
	let mut store = ScheduleStore::new();
	store.add(time_entry(
		"edit00001",
		Local::now() + Duration::minutes(10),
	));

	assert!(store.edit("edit00001", Some("new desc".into()), None, None, None));
	let e = &store.entries()[0];
	assert_eq!(e.description, "new desc");
	assert_eq!(
		e.message, "msg-edit00001",
		"unprovided fields stay untouched"
	);
	assert_eq!(e.interval_secs, None);

	assert!(store.edit(
		"edit00001",
		None,
		Some("new msg".into()),
		None,
		Some(Some(300))
	));
	let e = &store.entries()[0];
	assert_eq!(e.description, "new desc");
	assert_eq!(e.message, "new msg");
	assert_eq!(e.interval_secs, Some(300));

	assert!(store.edit("edit00001", None, None, None, Some(None)));
	assert_eq!(
		store.entries()[0].interval_secs,
		None,
		"Some(None) clears the interval"
	);

	assert!(!store.edit("missing-id", Some("x".into()), None, None, None));
}

#[test]
fn store_edit_retriggers_sorting_by_new_time() {
	let mut store = ScheduleStore::new();
	store.add(time_entry("early000", Local::now() + Duration::minutes(5)));
	store.add(time_entry("later000", Local::now() + Duration::minutes(30)));
	assert_eq!(store.entries()[0].id, "early000");

	let pushed = Local::now() + Duration::minutes(60);
	assert!(store.edit("early000", None, None, Some(pushed), None));
	assert_eq!(store.entries()[0].id, "later000");
	assert_eq!(store.entries()[1].id, "early000");
}

#[test]
fn pop_due_ignores_idle_and_pop_idle_drains_idle_only() {
	let mut store = ScheduleStore::new();
	store.add(ScheduleEntry::new_idle(
		"d".into(),
		"idle msg".into(),
		false,
	));
	store.add(time_entry("future00", Local::now() + Duration::hours(1)));

	assert!(store.pop_due().is_none(), "idle entries are never time-due");
	assert!(store.has_idle());

	let idle = store.pop_idle().expect("idle entry must pop");
	assert_eq!(idle.trigger_mode, TriggerMode::Idle);
	assert_eq!(idle.message, "idle msg");
	assert!(!store.has_idle());
	assert_eq!(store.entries().len(), 1, "time entry must survive pop_idle");
	assert_eq!(store.entries()[0].id, "future00");
	assert!(store.pop_idle().is_none());
}

#[test]
fn next_due_duration_tracks_earliest_time_entry() {
	let mut store = ScheduleStore::new();
	assert_eq!(store.next_due_duration(), None);

	store.add(ScheduleEntry::new_idle("d".into(), "m".into(), true));
	assert_eq!(
		store.next_due_duration(),
		None,
		"idle entries carry no due duration"
	);

	store.add(time_entry("far000000", Local::now() + Duration::hours(2)));
	store.add(time_entry(
		"near00000",
		Local::now() + Duration::minutes(10),
	));
	let d = store
		.next_due_duration()
		.expect("time entries must produce a duration");
	assert!(d >= std::time::Duration::from_secs(590), "got {d:?}");
	assert!(d <= std::time::Duration::from_secs(610), "got {d:?}");

	store.add(time_entry("due000000", Local::now() - Duration::minutes(1)));
	assert_eq!(store.next_due_duration(), Some(std::time::Duration::ZERO));
}

#[test]
fn seed_entries_restores_trigger_ordering() {
	let mut store = ScheduleStore::new();
	store.seed_entries(vec![
		time_entry("z-late", Local::now() + Duration::hours(3)),
		time_entry("a-soon", Local::now() + Duration::minutes(1)),
		time_entry("m-mid", Local::now() + Duration::hours(1)),
	]);
	let ids: Vec<&str> = store.entries().iter().map(|e| e.id.as_str()).collect();
	assert_eq!(ids, vec!["a-soon", "m-mid", "z-late"]);
}

#[test]
fn parse_duration_secs_edge_cases() {
	assert_eq!(parse_duration_secs("0s").unwrap(), 0);
	assert_eq!(
		parse_duration_secs("1h 30m").unwrap(),
		5400,
		"spaces are optional"
	);
	assert_eq!(
		parse_duration_secs("2h30m10s").unwrap(),
		2 * 3600 + 30 * 60 + 10
	);
	assert!(parse_duration_secs("").is_err());
	assert!(
		parse_duration_secs("10").is_err(),
		"trailing number without unit"
	);
	assert!(
		parse_duration_secs("5min").is_err(),
		"multi-letter units are rejected"
	);
	assert!(
		parse_duration_secs("999999999999h").is_err(),
		"must hit the 10-year cap, not overflow"
	);
}

#[test]
fn parse_when_accepts_am_pm_seconds_and_midnight_forms() {
	let pm = parse_when("3:30pm").unwrap();
	assert_eq!((pm.hour(), pm.minute()), (15, 30));
	assert!(pm > Local::now());

	assert_eq!(parse_when("9am").unwrap().hour(), 9);

	let midnight = parse_when("12am").unwrap();
	assert_eq!(midnight.hour(), 0, "12am is midnight");

	let with_secs = parse_when("15:30:45").unwrap();
	assert_eq!(
		(with_secs.hour(), with_secs.minute(), with_secs.second()),
		(15, 30, 45)
	);

	// Bare 24-hour input in the 12:xx hour must stay noon — the 12am rule
	// applies only when a meridiem was written (regression: "12:43" → 00:43).
	let noonish = parse_when("12:43").unwrap();
	assert_eq!((noonish.hour(), noonish.minute()), (12, 43));
	assert_eq!(parse_when("12pm").unwrap().hour(), 12, "12pm is noon");
}

#[test]
fn parse_when_absolute_datetime_with_seconds() {
	let t = parse_when("2099-06-15 08:45:30").unwrap();
	assert_eq!((t.year(), t.month(), t.day()), (2099, 6, 15));
	assert_eq!((t.hour(), t.minute(), t.second()), (8, 45, 30));
}

#[test]
fn parse_when_past_time_of_day_rolls_to_tomorrow() {
	// Today's midnight is always past (or exactly now) by the time the parser
	// runs, so "00:00" must land on tomorrow's midnight — never in the past.
	let t = parse_when("00:00").unwrap();
	assert!(t > Local::now(), "must never schedule in the past");
	let ahead = t.signed_duration_since(Local::now());
	assert!(
		ahead.num_seconds() > 0 && ahead.num_hours() <= 24,
		"ahead was {ahead}"
	);
	assert_eq!((t.hour(), t.minute()), (0, 0));
}

#[test]
fn parse_when_rejects_malformed_input() {
	assert!(parse_when("25:00").is_err(), "hour out of range");
	assert!(parse_when("12:99").is_err(), "minute out of range");
	assert!(parse_when("not a time").is_err());
	assert!(
		parse_when("2026-13-01 10:00").is_err(),
		"month out of range"
	);
	assert!(
		parse_when("2026-03-22").is_err(),
		"date without a time part"
	);
}

// --- parse_time_of_day: future-today branch ---------------------------------

#[test]
fn parse_time_of_day_returns_today_when_the_time_is_still_ahead() {
	use chrono::{Duration, Local, Timelike};

	// Pick a time-of-day ten minutes from now. If that crosses midnight the
	// function must answer tomorrow instead — assert whichever is expected at
	// run time so the test is deterministic at any hour.
	let target = Local::now() + Duration::minutes(10);
	let hhmm = format!("{:02}:{:02}", target.hour(), target.minute());
	let parsed = parse_time_of_day(&hhmm).expect("valid HH:MM parses");

	let expected_date = if target.date_naive() == Local::now().date_naive() {
		Local::now().date_naive()
	} else {
		Local::now()
			.date_naive()
			.succ_opt()
			.expect("tomorrow exists")
	};
	assert_eq!(
		parsed.date_naive(),
		expected_date,
		"parsed {hhmm} at {:?} → {:?}",
		Local::now(),
		parsed
	);
	assert_eq!(
		(parsed.hour(), parsed.minute()),
		(target.hour(), target.minute()),
		"time-of-day preserved"
	);
}
