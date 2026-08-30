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
fn test_strip_ansi() {
	assert_eq!(strip_ansi("plain"), "plain");
	assert_eq!(strip_ansi("\x1b[94m▍\x1b[0m x"), "▍ x");
	assert_eq!(strip_ansi("a\x1b[2Kb"), "ab");
	// Unterminated escape swallows the rest — never panics
	assert_eq!(strip_ansi("a\x1b[12;34"), "a");
}

#[test]
fn test_display_cols() {
	assert_eq!(display_cols(""), 0);
	assert_eq!(display_cols("abc"), 3);
	// ANSI sequences contribute zero width
	assert_eq!(display_cols("\x1b[94m▍ 〉\x1b[0m"), 3);
}

#[test]
fn test_rendered_rows() {
	// Unknown terminal width → 0 (callers skip redraw)
	assert_eq!(rendered_rows("", "▍ 〉", "    ", "hello", 0), 0);

	// prefix (3) + text fits in one row
	assert_eq!(rendered_rows("", "▍ 〉", "    ", "hello", 80), 1);

	// Exact width stays one row; one char over wraps to two
	let fits = "x".repeat(77); // 3 prefix + 77 = 80
	assert_eq!(rendered_rows("", "▍ 〉", "    ", &fits, 80), 1);
	let wraps = "x".repeat(78);
	assert_eq!(rendered_rows("", "▍ 〉", "    ", &wraps, 80), 2);

	// Continuation lines use the multiline prefix (4), first line the
	// prompt prefix (3): "ab" row + 76-char line +4 prefix = 80 → 1 row
	let second = "y".repeat(76);
	assert_eq!(
		rendered_rows("", "▍ 〉", "    ", &format!("ab\n{}", second), 80),
		2
	);
	// One char more on the continuation line wraps it
	let second_wraps = "y".repeat(77);
	assert_eq!(
		rendered_rows("", "▍ 〉", "    ", &format!("ab\n{}", second_wraps), 80),
		3
	);

	// Empty lines still occupy a row
	assert_eq!(rendered_rows("", "▍ 〉", "    ", "a\n\nb", 80), 3);
}

#[test]
fn test_strip_ansi_boundaries() {
	// Lone ESC is swallowed
	assert_eq!(strip_ansi("\x1b"), "");
	// Multibyte content passes through untouched
	assert_eq!(strip_ansi("日本語"), "日本語");
	assert_eq!(strip_ansi("日本語\x1b[0m"), "日本語");
	// Long interleaved input never panics or drops plain runs
	let mixed = "plain \x1b[31mred\x1b[0m ".repeat(200);
	assert_eq!(strip_ansi(&mixed), "plain red ".repeat(200));
	// ESC followed by digits-only sequence keeps swallowing until a letter
	assert_eq!(strip_ansi("a\x1b[2Kb"), "ab");
}

#[test]
fn test_display_cols_multibyte() {
	assert_eq!(display_cols("日本語"), 3);
	assert_eq!(display_cols("héllo"), 5);
	assert_eq!(display_cols("\x1b[94m日本語\x1b[0m"), 3);
}

#[tokio::test]
async fn test_calculate_current_context_tokens_grows_with_messages() {
	let config = toml::from_str::<crate::config::Config>(include_str!(
		"../../../config-templates/default.toml"
	))
	.expect("parse default config template");

	let empty = calculate_current_context_tokens(&[], &config, "assistant").await;
	let messages = vec![crate::session::Message {
		role: "user".to_string(),
		content: "please review the authentication module end to end".to_string(),
		..Default::default()
	}];
	let one = calculate_current_context_tokens(&messages, &config, "assistant").await;

	assert!(one > empty, "one={one} empty={empty}");
}

#[test]
fn test_display_shortcuts_help_smoke() {
	// Pure display function: the value is exercising every println path.
	display_shortcuts_help();
}

#[test]
fn test_highlight_submitted_input_bails_safely() {
	// Empty input: immediate no-op
	highlight_submitted_input("", "▍ 〉", "    ", "");
	// Non-empty input: without a TTY, terminal::size() fails and the function
	// returns before emitting anything — must not panic either way.
	highlight_submitted_input("", "▍ 〉", "    ", "hello world");
	highlight_submitted_input("", "▍ 〉", "    ", "multi\nline\ninput");
}

#[test]
fn test_add_completion_menu_keybindings_smoke() {
	let mut keybindings = Keybindings::new();
	add_completion_menu_keybindings(&mut keybindings);
}

#[test]
fn test_display_shortcuts_help_prints_all_rows_without_panicking() {
	// Pure println box — smoke-cover every row of the shortcuts help.
	display_shortcuts_help();
}
