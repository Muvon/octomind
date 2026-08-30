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

fn entry(
	name: &str,
	title: Option<&str>,
	role: Option<&str>,
	model: Option<&str>,
	created_at: u64,
) -> PickerEntry {
	PickerEntry {
		name: name.to_string(),
		title: title.map(|t| t.to_string()),
		role: role.map(|r| r.to_string()),
		model: model.map(|m| m.to_string()),
		created_at,
	}
}

/// The label starts with a `%Y-%m-%d %H:%M` timestamp. The wall-clock value is
/// timezone-dependent (`naive_local`), so assert the shape, not the exact date.
fn has_timestamp_prefix(label: &str) -> bool {
	let b = label.as_bytes();
	b.len() >= 16
		&& b[..4].iter().all(|c| c.is_ascii_digit())
		&& b[4] == b'-'
		&& b[5..7].iter().all(|c| c.is_ascii_digit())
		&& b[7] == b'-'
		&& b[8..10].iter().all(|c| c.is_ascii_digit())
		&& b[10] == b' '
		&& b[11..13].iter().all(|c| c.is_ascii_digit())
		&& b[13] == b':'
		&& b[14..16].iter().all(|c| c.is_ascii_digit())
}

#[test]
fn label_with_title_uses_em_dash_and_all_fields() {
	let e = entry(
		"abc123",
		Some("Fix login flow"),
		Some("developer"),
		Some("anthropic/claude-opus-4-7"),
		1_700_000_000,
	);
	let label = e.label();
	assert!(has_timestamp_prefix(&label), "missing date prefix: {label}");
	assert!(
		label.contains("abc123 — Fix login flow  developer  claude-opus-4-7"),
		"unexpected label layout: {label}"
	);
}

#[test]
fn label_without_title_omits_em_dash_separator() {
	let e = entry(
		"abc123",
		None,
		Some("developer"),
		Some("claude-opus-4-7"),
		1_700_000_000,
	);
	let label = e.label();
	assert!(
		!label.contains('—'),
		"no title must mean no em dash: {label}"
	);
	assert!(
		label.contains("abc123  developer  claude-opus-4-7"),
		"unexpected label layout: {label}"
	);
}

#[test]
fn label_with_empty_title_string_is_treated_as_missing() {
	let e = entry("abc123", Some(""), Some("developer"), None, 1_700_000_000);
	let label = e.label();
	assert!(
		!label.contains('—'),
		"empty title must not render em dash: {label}"
	);
	assert!(
		label.contains("abc123  developer"),
		"unexpected label layout: {label}"
	);
}

#[test]
fn label_shortens_model_to_last_slash_segment() {
	let e = entry(
		"s1",
		None,
		None,
		Some("openrouter/google/gemini-3-pro-preview"),
		1_700_000_000,
	);
	let label = e.label();
	assert!(
		label.contains("gemini-3-pro-preview"),
		"model short name missing: {label}"
	);
	assert!(
		!label.contains("openrouter/google"),
		"full model path must be shortened: {label}"
	);
}

#[test]
fn label_without_model_and_role_leaves_only_name_after_date() {
	let e = entry("abc123", None, None, None, 1_700_000_000);
	let label = e.label();
	assert!(has_timestamp_prefix(&label), "missing date prefix: {label}");
	let after_name = label
		.split_once("abc123")
		.expect("label must contain the session name")
		.1;
	assert!(
		after_name.trim().is_empty() && after_name.len() >= 4,
		"only separator whitespace may follow the name: {label:?}"
	);
}

#[test]
fn label_out_of_range_timestamp_renders_empty_date() {
	// i64::MAX seconds is far beyond chrono's representable range —
	// from_timestamp returns None, the label falls back to an empty date,
	// so the label starts with the two-space separator.
	let e = entry("abc123", Some("t"), None, None, i64::MAX as u64);
	let label = e.label();
	assert!(
		label.starts_with("  abc123"),
		"invalid timestamp must yield an empty date field: {label}"
	);
	assert!(!has_timestamp_prefix(&label), "no date expected: {label}");
}

#[test]
fn label_epoch_timestamp_still_formats() {
	let e = entry("abc123", None, None, None, 0);
	let label = e.label();
	assert!(
		has_timestamp_prefix(&label),
		"epoch must format as YYYY-MM-DD HH:MM (local): {label}"
	);
}
