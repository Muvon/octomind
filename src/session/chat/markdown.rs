// Markdown rendering module

use termimad::MadSkin;
use anyhow::Result;
use regex::Regex;
use super::syntax::SyntaxHighlighter;

pub struct MarkdownRenderer {
	skin: MadSkin,
	syntax_highlighter: SyntaxHighlighter,
}

impl MarkdownRenderer {
	pub fn new() -> Self {
		let mut skin = MadSkin::default();

		// Configure styles for better terminal output using termimad's Color enum
		use termimad::crossterm::style::Color;
		use termimad::crossterm::style::Attribute;

		// Headers with different colors (set separately, not chained)
		skin.headers[0].set_fg(Color::Yellow);
		skin.headers[0].add_attr(Attribute::Bold);
		skin.headers[1].set_fg(Color::Blue);
		skin.headers[1].add_attr(Attribute::Bold);
		skin.headers[2].set_fg(Color::Cyan);
		skin.headers[2].add_attr(Attribute::Bold);
		skin.headers[3].set_fg(Color::Green);
		skin.headers[3].add_attr(Attribute::Bold);
		skin.headers[4].set_fg(Color::Magenta);
		skin.headers[4].add_attr(Attribute::Bold);
		skin.headers[5].set_fg(Color::White);
		skin.headers[5].add_attr(Attribute::Bold);

		// Style for code blocks - we'll handle these manually with syntax highlighting
		skin.code_block.set_bg(Color::Rgb { r: 40, g: 40, b: 40 });
		skin.code_block.set_fg(Color::White);

		// Style for inline code
		skin.inline_code.set_bg(Color::Rgb { r: 60, g: 60, b: 60 });
		skin.inline_code.set_fg(Color::Yellow);

		// Style for emphasis
		skin.italic.set_fg(Color::Cyan);
		skin.bold.set_fg(Color::White);
		skin.bold.add_attr(Attribute::Bold);

		// Style for quotes
		skin.quote_mark.set_fg(Color::Blue);

		// Style for lists
		skin.bullet.set_fg(Color::Green);

		Self {
			skin,
			syntax_highlighter: SyntaxHighlighter::new(),
		}
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
			match self.syntax_highlighter.highlight_code(code, language) {
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
		let width = termimad::terminal_size().0.min(120).max(60);

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
			match self.syntax_highlighter.highlight_code(code, language) {
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
