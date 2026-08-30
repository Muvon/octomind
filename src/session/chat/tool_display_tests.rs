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
use serde_json::json;

#[test]
fn key_width_is_the_longest_key_clamped_to_twenty() {
	assert_eq!(key_width(Vec::<&str>::new()), 0);
	assert_eq!(key_width(["a", "bbb", "cc"]), 3);
	assert_eq!(key_width(["x".repeat(50)]), 20);
	// Counted in chars, not bytes, so alignment survives non-ASCII keys.
	assert_eq!(key_width(["日本語"]), 3);
}

#[test]
fn smart_value_quotes_short_strings_and_marks_empty_ones() {
	assert_eq!(format_parameter_value_smart(&json!("ls -la")), "\"ls -la\"");
	assert!(format_parameter_value_smart(&json!("")).contains("\"\""));
}

#[test]
fn smart_value_truncates_long_strings_by_chars() {
	let long = "x".repeat(500);
	let out = format_parameter_value_smart(&json!(long));
	// 97 kept + "..." inside quotes.
	assert_eq!(out.chars().count(), 102);
	assert!(out.ends_with("...\""));

	// Multi-byte input must not panic or split a char.
	let cjk = "語".repeat(500);
	let out = format_parameter_value_smart(&json!(cjk));
	assert_eq!(out.chars().count(), 102);
}

#[test]
fn smart_value_summarises_multiline_strings() {
	let out = format_parameter_value_smart(&json!("first\nsecond\nthird"));
	assert_eq!(out, "\"first\" [+2 lines]");

	// A single line with a trailing newline reports no extra lines.
	assert_eq!(format_parameter_value_smart(&json!("only\n")), "\"only\"");
}

#[test]
fn smart_value_collapses_big_arrays_and_objects() {
	assert_eq!(format_parameter_value_smart(&json!([])), "[]");
	assert_eq!(format_parameter_value_smart(&json!([1, 2, 3])), "[1, 2, 3]");
	assert_eq!(
		format_parameter_value_smart(&json!([1, 2, 3, 4])),
		"[4 items]"
	);
	assert_eq!(format_parameter_value_smart(&json!({})), "{}");

	let big = json!({ "k": "v".repeat(200) });
	assert_eq!(format_parameter_value_smart(&big), "{...} (1 keys)");
}

#[test]
fn smart_value_shortens_long_strings_inside_small_arrays() {
	let out = format_parameter_value_smart(&json!(["y".repeat(50), "short"]));
	assert!(out.starts_with("[\"yyy"));
	assert!(out.contains("...\""));
	assert!(out.ends_with("\"short\"]"));
}

#[test]
fn full_value_does_not_truncate() {
	let long = "z".repeat(500);
	let out = format_parameter_value_full(&json!(long));
	assert_eq!(out.chars().count(), 502);
	assert!(!out.contains("..."));
}
