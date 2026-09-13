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

// Syntax highlighting for code blocks

use anyhow::Result;
use syntect::easy::HighlightLines;
use syntect::highlighting::{Style, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::{as_24_bit_terminal_escaped, LinesWithEndings};

use std::sync::LazyLock;

// syntect's bundled dumps decompress to several MB of syntax definitions and
// themes. A renderer is built per printed assistant message / report, so
// loading them per instance allocated and freed that whole set every turn.
// They are immutable — load once per process and share.
static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);
static THEME_SET: LazyLock<ThemeSet> = LazyLock::new(ThemeSet::load_defaults);

pub struct SyntaxHighlighter {
	pub syntax_set: &'static SyntaxSet,
	pub theme_set: &'static ThemeSet,
}

impl SyntaxHighlighter {
	pub fn new() -> Self {
		Self {
			syntax_set: &SYNTAX_SET,
			theme_set: &THEME_SET,
		}
	}

	pub fn highlight_code_with_theme(
		&self,
		code: &str,
		language: &str,
		theme_name: &str,
	) -> Result<String> {
		// Try to find syntax definition for the language
		let syntax = self
			.syntax_set
			.find_syntax_by_token(language)
			.or_else(|| self.syntax_set.find_syntax_by_extension(language))
			.unwrap_or_else(|| self.syntax_set.find_syntax_plain_text());

		// Try to use the specified theme, fallback to a default if not found
		let theme = self.theme_set.themes.get(theme_name).unwrap_or_else(|| {
			// Fallback order: try base16-ocean.dark, then any available theme
			self.theme_set
				.themes
				.get("base16-ocean.dark")
				.or_else(|| self.theme_set.themes.values().next())
				.expect("No syntax themes available")
		});

		let mut highlighter = HighlightLines::new(syntax, theme);
		let mut highlighted = String::new();

		for line in LinesWithEndings::from(code) {
			let ranges: Vec<(Style, &str)> = highlighter.highlight_line(line, self.syntax_set)?;
			let escaped = as_24_bit_terminal_escaped(&ranges[..], false);
			highlighted.push_str(&escaped);
		}

		Ok(highlighted)
	}
}

impl Default for SyntaxHighlighter {
	fn default() -> Self {
		Self::new()
	}
}

#[cfg(test)]
#[path = "syntax_tests.rs"]
mod tests;
