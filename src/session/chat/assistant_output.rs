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

// Assistant response output and formatting

use crate::config::Config;
use crate::session::chat::markdown::{is_markdown_content, MarkdownRenderer};
use colored::Colorize;

// Helper function to print content with optional markdown rendering
pub fn print_assistant_response(content: &str, config: &Config, _role: &str) {
	if config.enable_markdown_rendering && is_markdown_content(content) {
		// Use markdown rendering with theme from config
		let theme = config.markdown_theme.parse().unwrap_or_default();
		let renderer = MarkdownRenderer::with_theme(theme);
		match renderer.render_and_print(content) {
			Ok(_) => {
				// Successfully rendered as markdown
			}
			Err(e) => {
				// Fallback to plain text if markdown rendering fails
				if config.get_log_level().is_debug_enabled() {
					println!("{}: {}", "Warning: Markdown rendering failed".yellow(), e);
				}
				println!("{}", content.bright_green());
			}
		}
	} else {
		// Use plain text with color
		println!("{}", content.bright_green());
	}
}
