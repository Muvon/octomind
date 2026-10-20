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

#[test]
fn truncate_short_string_unchanged() {
	assert_eq!(truncate("short", 10), "short");
	assert_eq!(truncate("exact len!", 10), "exact len!");
}

#[test]
fn truncate_long_string_caps_at_max_ending_with_ellipsis() {
	let long = "a".repeat(20);
	let cut = truncate(&long, 10);
	assert_eq!(cut.chars().count(), 10);
	assert!(cut.ends_with('…'));
	assert_eq!(cut, "aaaaaaaaa…");
}

#[test]
fn truncate_replaces_newlines_and_crlf_with_spaces() {
	assert_eq!(truncate("a\nb\nc", 10), "a b c");
	assert_eq!(truncate("a\r\nb\r\nc", 10), "a  b  c");
	assert_eq!(truncate("line1\rline2", 15), "line1 line2");
}

#[test]
fn truncate_counts_multibyte_and_emoji_as_single_chars() {
	// Accented character
	let text = "café";
	assert_eq!(text.chars().count(), 4);
	assert_eq!(truncate(text, 10), "café");

	// Emoji
	let emoji_text = "hello 🦀 world";
	let cut = truncate(emoji_text, 8);
	assert_eq!(cut.chars().count(), 8);
	assert!(cut.ends_with('…'));
	assert_eq!(cut, "hello 🦀…");

	// Multi-byte truncation boundary
	let unicode = "é".repeat(10);
	let cut = truncate(&unicode, 5);
	assert_eq!(cut.chars().count(), 5);
	assert!(cut.ends_with('…'));
	assert_eq!(cut, "éééé…");
}

#[test]
fn truncate_zero_cap_gives_ellipsis_only() {
	assert_eq!(truncate("anything", 0), "…");
}
