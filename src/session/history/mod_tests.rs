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
fn escape_leaves_plain_text_untouched() {
	assert_eq!(escape_history_line("hello"), "hello");
	assert_eq!(escape_history_line(""), "");
}

#[test]
fn escape_doubles_backslashes() {
	assert_eq!(escape_history_line("a\\b"), "a\\\\b");
}

#[test]
fn escape_encodes_newlines() {
	assert_eq!(escape_history_line("a\nb"), "a\\nb");
}

#[test]
fn escape_backslash_then_newline_stays_unambiguous() {
	// A literal backslash directly before a newline must survive as `\\` + `n`
	// so the decoder can never misread it as a single escaped newline.
	assert_eq!(escape_history_line("\\\n"), "\\\\\\n");
}

#[test]
fn unescape_leaves_plain_text_untouched() {
	assert_eq!(unescape_history_line("hello"), "hello");
}

#[test]
fn unescape_collapses_doubled_backslashes() {
	assert_eq!(unescape_history_line("a\\\\b"), "a\\b");
}

#[test]
fn unescape_decodes_newline_escapes() {
	assert_eq!(unescape_history_line("a\\nb"), "a\nb");
}

#[test]
fn unescape_keeps_trailing_lonely_backslash() {
	// A dangling `\` at end of line has no pair — kept verbatim.
	assert_eq!(unescape_history_line("a\\"), "a\\");
}

#[test]
fn unescape_preserves_unknown_escape_sequences() {
	// `\t` is not part of this format — both characters survive as-is.
	assert_eq!(unescape_history_line("a\\tb"), "a\\tb");
}

#[test]
fn escape_unescape_round_trips() {
	let cases = [
		"",
		"hello",
		"a\\b",
		"a\nb",
		"multi\nline\nentry",
		"back\\slash\nand newline",
		"\\\\\n\\n",
		"trailing backslash \\\nend",
	];
	for case in cases {
		assert_eq!(
			unescape_history_line(&escape_history_line(case)),
			case,
			"round-trip failed for {case:?}"
		);
	}
}
