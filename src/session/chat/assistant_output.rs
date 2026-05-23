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

// Assistant response output and formatting

use crate::config::Config;
use crate::providers::ThinkingBlock;
use crate::session::chat::markdown::{is_markdown_content, MarkdownRenderer};
use colored::Colorize;
use regex::Regex;

/// Strip `<system>...</system>` blocks (and surrounding blank lines) from
/// content intended for user-facing display. Used for welcome messages that
/// embed AI-only context inside `<system>` tags.
pub fn strip_system_tags(content: &str) -> String {
	// Match <system>...</system> (multiline, lazy) including any trailing newline
	let re = Regex::new(r"(?is)\s*<system>.*?</system>\s*").unwrap();
	re.replace_all(content, "\n").trim().to_string()
}

/// Check if assistant content was already displayed in thinking block
/// Returns the content to display (either full content or trimmed content)
pub fn get_content_to_display(content: &str, thinking: &Option<ThinkingBlock>) -> String {
	if let Some(ref thinking_block) = thinking {
		// Check if thinking content is a prefix of the full content
		// If so, skip the thinking portion and only show the response portion
		if content.starts_with(&thinking_block.content) {
			let after_thinking = &content[thinking_block.content.len()..];
			// Skip leading whitespace/newlines
			let trimmed = after_thinking.trim_start().to_string();
			if trimmed.is_empty() {
				// All content was thinking, nothing to display
				return String::new();
			}
			return trimmed;
		}
	}
	content.to_string()
}

// Helper function to print content with optional markdown rendering
pub fn print_assistant_response(
	content: &str,
	config: &Config,
	_role: &str,
	thinking: &Option<ThinkingBlock>,
) {
	let content_to_display = get_content_to_display(content, thinking);

	if content_to_display.is_empty() {
		return;
	}

	// Frame the assistant block with a dim horizontal rule above and below so
	// the reply is visually distinct from system notes (`·`-prefixed) and from
	// the prompt status line (`▍`-prefixed). Per-line markers would fight the
	// markdown renderer (code blocks, tables); a top/bottom rule does not.
	let rule = "─────".bright_black();
	println!("{}", rule);

	if config.enable_markdown_rendering && is_markdown_content(&content_to_display) {
		// Use markdown rendering with theme from config.
		// The renderer suspends the spinner internally around each termimad
		// `skin.print_text` call (those bypass our shadowed print macros).
		// All other prints inside the renderer go through our shadowed
		// macros, which suspend per-line on their own.
		let theme = config.markdown_theme.parse().unwrap_or_default();
		let renderer = MarkdownRenderer::with_theme(theme);
		match renderer.render_and_print(&content_to_display) {
			Ok(_) => {
				// Successfully rendered as markdown
			}
			Err(e) => {
				// Fallback to plain text if markdown rendering fails
				crate::log_debug!("{}: {}", "Warning: Markdown rendering failed".yellow(), e);
				println!("{}", content_to_display.bright_green());
			}
		}
	} else {
		// Use plain text with color
		println!("{}", content_to_display.bright_green());
	}

	println!("{}", rule);
}
