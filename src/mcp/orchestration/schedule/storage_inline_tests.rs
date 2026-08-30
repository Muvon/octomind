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
		trigger_mode: TriggerMode::Time,
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
		trigger_mode: TriggerMode::Time,
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
		trigger_mode: TriggerMode::Time,
	});
	store.add(ScheduleEntry {
		id: "soon0001".to_string(),
		description: "sooner".to_string(),
		message: "a".to_string(),
		trigger_at: sooner,
		created_at: Local::now(),
		interval_secs: None,
		trigger_mode: TriggerMode::Time,
	});
	// First entry should be the sooner one.
	assert_eq!(store.entries()[0].id, "soon0001");
}
