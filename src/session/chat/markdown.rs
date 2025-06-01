// Copyright 2025 Muvon Un Limited
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

// Markdown rendering module

use std::str::FromStr;
use termimad::MadSkin;
use anyhow::Result;
use regex::Regex;
use super::syntax::SyntaxHighlighter;

#[derive(Debug, Clone, PartialEq, Default)]
pub enum MarkdownTheme {
	#[default]
	Default,
	Dark,
	Light,
	Ocean,
	Solarized,
	Monokai,
}

impl FromStr for MarkdownTheme {
	type Err = String;

	fn from_str(s: &str) -> Result<Self, Self::Err> {
		match s.to_lowercase().as_str() {
			"default" => Ok(MarkdownTheme::Default),
			"dark" => Ok(MarkdownTheme::Dark),
			"light" => Ok(MarkdownTheme::Light),
			"ocean" => Ok(MarkdownTheme::Ocean),
			"solarized" => Ok(MarkdownTheme::Solarized),
			"monokai" => Ok(MarkdownTheme::Monokai),
			_ => Err(format!("Invalid theme: {}", s)),
		}
	}
}

impl MarkdownTheme {

	pub fn as_str(&self) -> &'static str {
		match self {
			MarkdownTheme::Default => "default",
			MarkdownTheme::Dark => "dark",
			MarkdownTheme::Light => "light",
			MarkdownTheme::Ocean => "ocean",
			MarkdownTheme::Solarized => "solarized",
			MarkdownTheme::Monokai => "monokai",
		}
	}

	pub fn all_themes() -> Vec<&'static str> {
		vec!["default", "dark", "light", "ocean", "solarized", "monokai"]
	}

	/// Get the corresponding syntax highlighting theme name for this markdown theme
	pub fn get_syntax_theme_name(&self) -> &'static str {
		match self {
			MarkdownTheme::Default => "base16-ocean.dark",
			MarkdownTheme::Dark => "base16-eighties.dark",
			MarkdownTheme::Light => "InspiredGitHub",
			MarkdownTheme::Ocean => "base16-ocean.dark",
			MarkdownTheme::Solarized => "Solarized (dark)",
			MarkdownTheme::Monokai => "base16-mocha.dark", // Closest match since Monokai Extended isn't available
		}
	}
}

pub struct MarkdownRenderer {
	skin: MadSkin,
	syntax_highlighter: SyntaxHighlighter,
	theme: MarkdownTheme,
}

impl MarkdownRenderer {
	pub fn new() -> Self {
		Self::with_theme(MarkdownTheme::Default)
	}

	pub fn with_theme(theme: MarkdownTheme) -> Self {
		let mut skin = MadSkin::default();
		Self::apply_theme(&mut skin, &theme);

		Self {
			skin,
			syntax_highlighter: SyntaxHighlighter::new(),
			theme,
		}
	}

	fn apply_theme(skin: &mut MadSkin, theme: &MarkdownTheme) {
		use termimad::crossterm::style::Color;
		use termimad::crossterm::style::Attribute;

		match theme {
			MarkdownTheme::Default => {
				// Improved default theme with better contrast and readability
				skin.headers[0].set_fg(Color::Rgb { r: 255, g: 215, b: 0 });  // Gold
				skin.headers[0].add_attr(Attribute::Bold);
				skin.headers[1].set_fg(Color::Rgb { r: 100, g: 149, b: 237 }); // Cornflower blue
				skin.headers[1].add_attr(Attribute::Bold);
				skin.headers[2].set_fg(Color::Rgb { r: 72, g: 209, b: 204 });  // Medium turquoise
				skin.headers[2].add_attr(Attribute::Bold);
				skin.headers[3].set_fg(Color::Rgb { r: 144, g: 238, b: 144 }); // Light green
				skin.headers[3].add_attr(Attribute::Bold);
				skin.headers[4].set_fg(Color::Rgb { r: 221, g: 160, b: 221 }); // Plum
				skin.headers[4].add_attr(Attribute::Bold);
				skin.headers[5].set_fg(Color::Rgb { r: 240, g: 248, b: 255 }); // Alice blue
				skin.headers[5].add_attr(Attribute::Bold);

				// Improved code styling
				skin.code_block.set_bg(Color::Rgb { r: 45, g: 45, b: 50 });
				skin.code_block.set_fg(Color::Rgb { r: 248, g: 248, b: 242 });
				skin.inline_code.set_bg(Color::Rgb { r: 60, g: 60, b: 65 });
				skin.inline_code.set_fg(Color::Rgb { r: 230, g: 219, b: 116 });

				// Better text styling
				skin.italic.set_fg(Color::Rgb { r: 102, g: 217, b: 239 });
				skin.bold.set_fg(Color::White);
				skin.bold.add_attr(Attribute::Bold);
				skin.strikeout.set_fg(Color::Rgb { r: 128, g: 128, b: 128 });
				skin.strikeout.add_attr(Attribute::CrossedOut);

				// Quote and list styling
				skin.quote_mark.set_fg(Color::Rgb { r: 117, g: 113, b: 94 });
				skin.bullet.set_fg(Color::Rgb { r: 166, g: 226, b: 46 });
			},
			MarkdownTheme::Dark => {
				// Dark theme optimized for dark terminals
				skin.headers[0].set_fg(Color::Rgb { r: 255, g: 85, b: 85 });   // Light red
				skin.headers[0].add_attr(Attribute::Bold);
				skin.headers[1].set_fg(Color::Rgb { r: 255, g: 165, b: 0 });   // Orange
				skin.headers[1].add_attr(Attribute::Bold);
				skin.headers[2].set_fg(Color::Rgb { r: 255, g: 255, b: 0 });   // Yellow
				skin.headers[2].add_attr(Attribute::Bold);
				skin.headers[3].set_fg(Color::Rgb { r: 50, g: 205, b: 50 });   // Lime green
				skin.headers[3].add_attr(Attribute::Bold);
				skin.headers[4].set_fg(Color::Rgb { r: 135, g: 206, b: 235 }); // Sky blue
				skin.headers[4].add_attr(Attribute::Bold);
				skin.headers[5].set_fg(Color::Rgb { r: 238, g: 130, b: 238 }); // Violet
				skin.headers[5].add_attr(Attribute::Bold);

				skin.code_block.set_bg(Color::Rgb { r: 30, g: 30, b: 30 });
				skin.code_block.set_fg(Color::Rgb { r: 220, g: 220, b: 220 });
				skin.inline_code.set_bg(Color::Rgb { r: 50, g: 50, b: 50 });
				skin.inline_code.set_fg(Color::Rgb { r: 255, g: 215, b: 0 });

				skin.italic.set_fg(Color::Rgb { r: 176, g: 196, b: 222 });
				skin.bold.set_fg(Color::Rgb { r: 255, g: 255, b: 255 });
				skin.bold.add_attr(Attribute::Bold);

				skin.quote_mark.set_fg(Color::Rgb { r: 105, g: 105, b: 105 });
				skin.bullet.set_fg(Color::Rgb { r: 124, g: 252, b: 0 });
			},
			MarkdownTheme::Light => {
				// Light theme optimized for light terminals
				skin.headers[0].set_fg(Color::Rgb { r: 139, g: 0, b: 0 });     // Dark red
				skin.headers[0].add_attr(Attribute::Bold);
				skin.headers[1].set_fg(Color::Rgb { r: 255, g: 140, b: 0 });   // Dark orange
				skin.headers[1].add_attr(Attribute::Bold);
				skin.headers[2].set_fg(Color::Rgb { r: 184, g: 134, b: 11 });  // Dark golden rod
				skin.headers[2].add_attr(Attribute::Bold);
				skin.headers[3].set_fg(Color::Rgb { r: 0, g: 100, b: 0 });     // Dark green
				skin.headers[3].add_attr(Attribute::Bold);
				skin.headers[4].set_fg(Color::Rgb { r: 0, g: 0, b: 139 });     // Dark blue
				skin.headers[4].add_attr(Attribute::Bold);
				skin.headers[5].set_fg(Color::Rgb { r: 128, g: 0, b: 128 });   // Purple
				skin.headers[5].add_attr(Attribute::Bold);

				skin.code_block.set_bg(Color::Rgb { r: 245, g: 245, b: 245 });
				skin.code_block.set_fg(Color::Rgb { r: 51, g: 51, b: 51 });
				skin.inline_code.set_bg(Color::Rgb { r: 230, g: 230, b: 230 });
				skin.inline_code.set_fg(Color::Rgb { r: 139, g: 69, b: 19 });

				skin.italic.set_fg(Color::Rgb { r: 70, g: 130, b: 180 });
				skin.bold.set_fg(Color::Black);
				skin.bold.add_attr(Attribute::Bold);

				skin.quote_mark.set_fg(Color::Rgb { r: 105, g: 105, b: 105 });
				skin.bullet.set_fg(Color::Rgb { r: 34, g: 139, b: 34 });
			},
			MarkdownTheme::Ocean => {
				// Ocean-inspired theme with blue-green palette
				skin.headers[0].set_fg(Color::Rgb { r: 127, g: 255, b: 212 }); // Aquamarine
				skin.headers[0].add_attr(Attribute::Bold);
				skin.headers[1].set_fg(Color::Rgb { r: 64, g: 224, b: 208 });  // Turquoise
				skin.headers[1].add_attr(Attribute::Bold);
				skin.headers[2].set_fg(Color::Rgb { r: 0, g: 206, b: 209 });   // Dark turquoise
				skin.headers[2].add_attr(Attribute::Bold);
				skin.headers[3].set_fg(Color::Rgb { r: 72, g: 209, b: 204 });  // Medium turquoise
				skin.headers[3].add_attr(Attribute::Bold);
				skin.headers[4].set_fg(Color::Rgb { r: 95, g: 158, b: 160 });  // Cadet blue
				skin.headers[4].add_attr(Attribute::Bold);
				skin.headers[5].set_fg(Color::Rgb { r: 176, g: 224, b: 230 }); // Powder blue
				skin.headers[5].add_attr(Attribute::Bold);

				skin.code_block.set_bg(Color::Rgb { r: 25, g: 42, b: 50 });
				skin.code_block.set_fg(Color::Rgb { r: 171, g: 178, b: 191 });
				skin.inline_code.set_bg(Color::Rgb { r: 40, g: 55, b: 65 });
				skin.inline_code.set_fg(Color::Rgb { r: 128, g: 203, b: 196 });

				skin.italic.set_fg(Color::Rgb { r: 102, g: 217, b: 239 });
				skin.bold.set_fg(Color::Rgb { r: 192, g: 255, b: 238 });
				skin.bold.add_attr(Attribute::Bold);

				skin.quote_mark.set_fg(Color::Rgb { r: 107, g: 142, b: 135 });
				skin.bullet.set_fg(Color::Rgb { r: 64, g: 224, b: 208 });
			},
			MarkdownTheme::Solarized => {
				// Solarized-inspired theme
				skin.headers[0].set_fg(Color::Rgb { r: 220, g: 50, b: 47 });   // Red
				skin.headers[0].add_attr(Attribute::Bold);
				skin.headers[1].set_fg(Color::Rgb { r: 203, g: 75, b: 22 });   // Orange
				skin.headers[1].add_attr(Attribute::Bold);
				skin.headers[2].set_fg(Color::Rgb { r: 181, g: 137, b: 0 });   // Yellow
				skin.headers[2].add_attr(Attribute::Bold);
				skin.headers[3].set_fg(Color::Rgb { r: 133, g: 153, b: 0 });   // Green
				skin.headers[3].add_attr(Attribute::Bold);
				skin.headers[4].set_fg(Color::Rgb { r: 38, g: 139, b: 210 });  // Blue
				skin.headers[4].add_attr(Attribute::Bold);
				skin.headers[5].set_fg(Color::Rgb { r: 108, g: 113, b: 196 }); // Violet
				skin.headers[5].add_attr(Attribute::Bold);

				skin.code_block.set_bg(Color::Rgb { r: 0, g: 43, b: 54 });
				skin.code_block.set_fg(Color::Rgb { r: 131, g: 148, b: 150 });
				skin.inline_code.set_bg(Color::Rgb { r: 7, g: 54, b: 66 });
				skin.inline_code.set_fg(Color::Rgb { r: 42, g: 161, b: 152 });

				skin.italic.set_fg(Color::Rgb { r: 147, g: 161, b: 161 });
				skin.bold.set_fg(Color::Rgb { r: 253, g: 246, b: 227 });
				skin.bold.add_attr(Attribute::Bold);

				skin.quote_mark.set_fg(Color::Rgb { r: 88, g: 110, b: 117 });
				skin.bullet.set_fg(Color::Rgb { r: 133, g: 153, b: 0 });
			},
			MarkdownTheme::Monokai => {
				// Monokai-inspired theme
				skin.headers[0].set_fg(Color::Rgb { r: 249, g: 38, b: 114 });  // Pink
				skin.headers[0].add_attr(Attribute::Bold);
				skin.headers[1].set_fg(Color::Rgb { r: 253, g: 151, b: 31 });  // Orange
				skin.headers[1].add_attr(Attribute::Bold);
				skin.headers[2].set_fg(Color::Rgb { r: 230, g: 219, b: 116 }); // Yellow
				skin.headers[2].add_attr(Attribute::Bold);
				skin.headers[3].set_fg(Color::Rgb { r: 166, g: 226, b: 46 });  // Green
				skin.headers[3].add_attr(Attribute::Bold);
				skin.headers[4].set_fg(Color::Rgb { r: 102, g: 217, b: 239 }); // Cyan
				skin.headers[4].add_attr(Attribute::Bold);
				skin.headers[5].set_fg(Color::Rgb { r: 174, g: 129, b: 255 }); // Purple
				skin.headers[5].add_attr(Attribute::Bold);

				skin.code_block.set_bg(Color::Rgb { r: 39, g: 40, b: 34 });
				skin.code_block.set_fg(Color::Rgb { r: 248, g: 248, b: 242 });
				skin.inline_code.set_bg(Color::Rgb { r: 49, g: 50, b: 44 });
				skin.inline_code.set_fg(Color::Rgb { r: 230, g: 219, b: 116 });

				skin.italic.set_fg(Color::Rgb { r: 117, g: 113, b: 94 });
				skin.bold.set_fg(Color::Rgb { r: 248, g: 248, b: 242 });
				skin.bold.add_attr(Attribute::Bold);

				skin.quote_mark.set_fg(Color::Rgb { r: 117, g: 113, b: 94 });
				skin.bullet.set_fg(Color::Rgb { r: 166, g: 226, b: 46 });
			}
		}
	}

	pub fn get_theme(&self) -> &MarkdownTheme {
		&self.theme
	}

	pub fn set_theme(&mut self, theme: MarkdownTheme) {
		self.theme = theme;
		Self::apply_theme(&mut self.skin, &self.theme);
	}

	fn preprocess_code_blocks(&self, markdown: &str) -> Result<String> {
		// Regex to match fenced code blocks with optional language specification
		let code_block_regex = Regex::new(r"```(\w+)?\n([\s\S]*?)\n```")?;

		let mut result = String::new();
		let mut last_end = 0;

		for cap in code_block_regex.captures_iter(markdown) {
			// Add content before this code block
			result.push_str(&markdown[last_end..cap.get(0).unwrap().start()]);

			let language = cap.get(1).map(|m| m.as_str()).unwrap_or("text");
			let code = cap.get(2).unwrap().as_str();

			// Try to highlight the code
			match self.syntax_highlighter.highlight_code_with_theme(code, language, self.theme.get_syntax_theme_name()) {
				Ok(highlighted) => {
					// Replace the code block with highlighted version
					// We'll use a simple format that termimad can handle
					result.push_str("```\n");
					result.push_str(&highlighted);
					result.push_str("```");
				}
				Err(_) => {
					// Fall back to original code block if highlighting fails
					result.push_str(cap.get(0).unwrap().as_str());
				}
			}

			last_end = cap.get(0).unwrap().end();
		}

		// Add remaining content after last code block
		result.push_str(&markdown[last_end..]);

		Ok(result)
	}

	pub fn render(&self, markdown: &str) -> Result<String> {
		// First preprocess code blocks for syntax highlighting
		let processed_markdown = self.preprocess_code_blocks(markdown)?;

		// Get terminal width, fallback to 80 if unable to determine
		let width = termimad::terminal_size().0.clamp(60, 120);

		// Render the markdown
		let styled_content = self.skin.area_text(&processed_markdown, &termimad::Area::new(0, 0, width, 1000));

		// Convert to string
		Ok(styled_content.to_string())
	}

	pub fn render_and_print(&self, markdown: &str) -> Result<()> {
		// For printing, we'll handle code blocks manually for better control
		self.render_with_syntax_highlighting(markdown)?;
		Ok(())
	}

	fn render_with_syntax_highlighting(&self, markdown: &str) -> Result<()> {
		// Split markdown by code blocks and process each part separately
		let code_block_regex = Regex::new(r"```(\w+)?\n([\s\S]*?)\n```")?;

		let mut last_end = 0;

		for cap in code_block_regex.captures_iter(markdown) {
			// Render content before this code block with termimad
			let before_content = &markdown[last_end..cap.get(0).unwrap().start()];
			if !before_content.trim().is_empty() {
				self.skin.print_text(before_content);
			}

			let language = cap.get(1).map(|m| m.as_str()).unwrap_or("text");
			let code = cap.get(2).unwrap().as_str();

			// Print syntax-highlighted code block
			println!(); // Add some spacing
			match self.syntax_highlighter.highlight_code_with_theme(code, language, self.theme.get_syntax_theme_name()) {
				Ok(highlighted) => {
					// Print with a subtle border
					println!("┌─ {} ─", language);
					print!("{}", highlighted);
					if !highlighted.ends_with('\n') {
						println!();
					}
					println!("└─────");
				}
				Err(_) => {
					// Fall back to simple code block
					println!("┌─ {} ─", language);
					println!("{}", code);
					println!("└─────");
				}
			}
			println!(); // Add some spacing after

			last_end = cap.get(0).unwrap().end();
		}

		// Render remaining content after last code block
		let remaining_content = &markdown[last_end..];
		if !remaining_content.trim().is_empty() {
			self.skin.print_text(remaining_content);
		}

		Ok(())
	}
}

impl Default for MarkdownRenderer {
	fn default() -> Self {
		Self::new()
	}
}

// Helper function to check if content looks like markdown
pub fn is_markdown_content(content: &str) -> bool {
	// Simple heuristics to detect markdown content
	content.contains("```") ||
	content.contains("# ") ||
	content.contains("## ") ||
	content.contains("### ") ||
	content.contains("**") ||
	content.contains("*") ||
	content.contains("[") ||
	content.contains("|") ||
	content.contains("> ")
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_markdown_detection() {
		assert!(is_markdown_content("# Heading"));
		assert!(is_markdown_content("```rust\ncode\n```"));
		assert!(is_markdown_content("**bold text**"));
		assert!(is_markdown_content("[link](url)"));
		assert!(!is_markdown_content("plain text"));
	}

	#[test]
	fn test_renderer_creation() {
		let renderer = MarkdownRenderer::new();
		// Just test that it doesn't panic
		assert!(!renderer.skin.headers.is_empty());
	}
}
