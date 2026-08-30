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
fn strip_ansi_removes_csi_sequences() {
	assert_eq!(strip_ansi("\x1b[31mred\x1b[0m plain"), "red plain");
	assert_eq!(strip_ansi("\x1b[1;32mx\x1b[0m"), "x");
	assert_eq!(strip_ansi("no escapes"), "no escapes");
	assert_eq!(strip_ansi(""), "");
	// A lone ESC that never terminates is swallowed entirely
	assert_eq!(strip_ansi("a\x1b[12"), "a");
}

#[test]
fn display_cols_counts_chars_after_stripping() {
	assert_eq!(display_cols("\x1b[31mabc\x1b[0m"), 3);
	assert_eq!(display_cols("héllo"), 5);
}

#[test]
fn rendered_rows_wraps_by_terminal_width() {
	// Unknown terminal width → caller should skip redraw
	assert_eq!(rendered_rows("▍", "〉", " ", "hello", 0), 0);
	// Short single line with a 2-cell prefix → one row
	assert_eq!(rendered_rows("▍", "〉", " ", "hello", 10), 1);
	// 25 visible chars, no prefix, width 10 → 3 rows
	assert_eq!(rendered_rows("", "", "", &"x".repeat(25), 10), 3);
	// Multiline input: each logical line contributes at least one row
	assert_eq!(rendered_rows("", "", "", "a\nb", 10), 2);
	// Empty line still occupies one row (max(1) guard)
	assert_eq!(rendered_rows("", "", "", "", 10), 1);
}

#[test]
fn highlight_submitted_input_bails_safely_without_tty() {
	// Empty input is a no-op; non-empty input bails at terminal-size lookup
	// on a headless runner (or redraws harmlessly under a real tty).
	highlight_submitted_input("▍", "〉", " ", "");
	highlight_submitted_input("▍", "〉", " ", "hello world");
	highlight_submitted_input("▍", "〉", " ", "multi\nline\ninput");
}

#[tokio::test]
async fn calculate_current_context_tokens_counts_tools_and_messages() {
	let config: Config = toml::from_str(include_str!("../../../config-templates/default.toml"))
		.expect("parse default config template");
	let empty = calculate_current_context_tokens(&[], &config, "assistant").await;
	assert!(
		empty > 0,
		"system prompt + tool schemas must count: {empty}"
	);

	let messages = vec![crate::session::Session::build_message(
		"user",
		"hello world",
	)];
	let with_message = calculate_current_context_tokens(&messages, &config, "assistant").await;
	assert!(with_message > empty);
}
