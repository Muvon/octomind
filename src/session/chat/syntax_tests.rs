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
fn test_default_matches_new() {
	let via_new = SyntaxHighlighter::new();
	let via_default = SyntaxHighlighter::default();
	assert!(!via_default.syntax_set.syntaxes().is_empty());
	assert!(!via_default.theme_set.themes.is_empty());
	assert_eq!(
		via_new.syntax_set.syntaxes().len(),
		via_default.syntax_set.syntaxes().len()
	);
	assert_eq!(
		via_new.theme_set.themes.len(),
		via_default.theme_set.themes.len()
	);
}

#[test]
fn test_highlight_empty_code() {
	let highlighter = SyntaxHighlighter::new();
	let result = highlighter.highlight_code_with_theme("", "rust", "base16-ocean.dark");
	assert!(result.is_ok());
	assert_eq!(result.unwrap(), "");
}

#[test]
fn test_highlight_unknown_language_falls_back_to_plain_text() {
	let highlighter = SyntaxHighlighter::new();
	let result = highlighter.highlight_code_with_theme(
		"just some words",
		"not-a-real-language",
		"base16-ocean.dark",
	);
	assert!(result.is_ok());
	// Plain-text fallback still emits styled output containing the source text
	let highlighted = result.unwrap();
	assert!(!highlighted.is_empty());
	assert!(highlighted.contains("just some words"));
}

#[test]
fn test_highlight_unknown_theme_falls_back_to_default() {
	let highlighter = SyntaxHighlighter::new();
	let result = highlighter.highlight_code_with_theme("fn main() {}", "rust", "no-such-theme");
	assert!(result.is_ok());
	// Fallback theme still produces ANSI escape codes
	assert!(result.unwrap().contains("\x1b["));
}

#[test]
fn test_highlight_language_by_extension() {
	let highlighter = SyntaxHighlighter::new();
	let result = highlighter.highlight_code_with_theme("print('hi')", "py", "base16-ocean.dark");
	assert!(result.is_ok());
	let highlighted = result.unwrap();
	assert!(highlighted.contains("\x1b["));
	assert!(highlighted.contains("print"));
}

#[test]
fn test_highlight_multiline_code_preserves_line_count() {
	let highlighter = SyntaxHighlighter::new();
	let code = "fn a() {}\nfn b() {}\n";
	let result = highlighter.highlight_code_with_theme(code, "rust", "base16-ocean.dark");
	assert!(result.is_ok());
	assert_eq!(result.unwrap().matches('\n').count(), 2);
}

#[test]
fn test_syntax_highlighter_creation() {
	let highlighter = SyntaxHighlighter::new();
	assert!(!highlighter.syntax_set.syntaxes().is_empty());
	assert!(!highlighter.theme_set.themes.is_empty());
}

#[test]
fn test_rust_highlighting() {
	let highlighter = SyntaxHighlighter::new();
	let code = "fn main() {\n    println!(\"Hello, world!\");\n}";
	let result = highlighter.highlight_code_with_theme(code, "rust", "base16-ocean.dark");
	assert!(result.is_ok());
	// The result should contain ANSI escape codes for coloring
	assert!(result.unwrap().contains("\x1b["));
}
