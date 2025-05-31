// Syntax highlighting for code blocks

use syntect::easy::HighlightLines;
use syntect::highlighting::{ThemeSet, Style};
use syntect::parsing::SyntaxSet;
use syntect::util::{as_24_bit_terminal_escaped, LinesWithEndings};
use anyhow::Result;

pub struct SyntaxHighlighter {
	pub syntax_set: SyntaxSet,
	pub theme_set: ThemeSet,
}

impl SyntaxHighlighter {
	pub fn new() -> Self {
		Self {
			syntax_set: SyntaxSet::load_defaults_newlines(),
			theme_set: ThemeSet::load_defaults(),
		}
	}

	pub fn highlight_code_with_theme(&self, code: &str, language: &str, theme_name: &str) -> Result<String> {
		// Try to find syntax definition for the language
		let syntax = self.syntax_set
			.find_syntax_by_token(language)
			.or_else(|| self.syntax_set.find_syntax_by_extension(language))
			.unwrap_or_else(|| self.syntax_set.find_syntax_plain_text());

		// Try to use the specified theme, fallback to a default if not found
		let theme = self.theme_set.themes.get(theme_name)
			.unwrap_or_else(|| {
				// Fallback order: try base16-ocean.dark, then any available theme
				self.theme_set.themes.get("base16-ocean.dark")
					.or_else(|| self.theme_set.themes.values().next())
					.expect("No syntax themes available")
			});

		let mut highlighter = HighlightLines::new(syntax, theme);
		let mut highlighted = String::new();

		for line in LinesWithEndings::from(code) {
			let ranges: Vec<(Style, &str)> = highlighter.highlight_line(line, &self.syntax_set)?;
			let escaped = as_24_bit_terminal_escaped(&ranges[..], false);
			highlighted.push_str(&escaped);
		}

		Ok(highlighted)
	}

	/// Get a list of available syntax themes
	#[allow(dead_code)]
	pub fn available_themes(&self) -> Vec<String> {
		self.theme_set.themes.keys().cloned().collect()
	}

	/// Get a list of supported languages for debugging
	#[allow(dead_code)]
	pub fn supported_languages(&self) -> Vec<String> {
		self.syntax_set
			.syntaxes()
			.iter()
			.flat_map(|syntax| syntax.file_extensions.iter())
			.map(|ext| ext.to_string())
			.collect()
	}
}

impl Default for SyntaxHighlighter {
	fn default() -> Self {
		Self::new()
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_syntax_highlighter_creation() {
		let highlighter = SyntaxHighlighter::new();
		assert!(!highlighter.syntax_set.syntaxes().is_empty());
		assert!(!highlighter.theme_set.themes.is_empty());
	}

	#[test]
	fn test_available_themes() {
		let highlighter = SyntaxHighlighter::new();
		let themes = highlighter.available_themes();
		println!("Available syntax themes: {:?}", themes);
		// Print them one by one for easier reading
		for theme in &themes {
			println!("Theme: {}", theme);
		}
		assert!(!themes.is_empty());
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
}
