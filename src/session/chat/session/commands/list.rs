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

// List command handler

use super::super::core::ChatSession;
use super::utils::format_number;
use crate::config::Config;
use crate::session::chat::markdown::MarkdownRenderer;
use crate::session::list_available_sessions;
use anyhow::Result;
use chrono::{DateTime, Utc};
use colored::Colorize;

pub fn handle_list(session: &ChatSession, config: &Config, params: &[&str]) -> Result<bool> {
	// Parse optional page parameter
	let page = if !params.is_empty() {
		match params[0].parse::<usize>() {
			Ok(p) if p > 0 => p,
			_ => {
				println!(
					"{}: Page number must be a positive integer",
					"Error".bright_red()
				);
				return Ok(false);
			}
		}
	} else {
		1 // Default to page 1
	};

	match list_available_sessions() {
		Ok(sessions) => {
			if sessions.is_empty() {
				println!("{}", "No sessions found.".bright_yellow());
			} else {
				// Pagination settings
				const SESSIONS_PER_PAGE: usize = 15;
				let total_sessions = sessions.len();
				let total_pages = total_sessions.div_ceil(SESSIONS_PER_PAGE);

				if page > total_pages {
					println!(
						"{}: Page {} not found. Total pages: {}",
						"Error".bright_red(),
						page,
						total_pages
					);
					return Ok(false);
				}

				// Calculate pagination bounds
				let start_idx = (page - 1) * SESSIONS_PER_PAGE;
				let end_idx = std::cmp::min(start_idx + SESSIONS_PER_PAGE, total_sessions);
				let page_sessions = &sessions[start_idx..end_idx];

				// Create markdown table
				let mut markdown_content = String::new();

				// Add header with pagination info
				markdown_content.push_str(&format!(
					"# Available Sessions (Page {} of {})\n\n",
					page, total_pages
				));
				markdown_content.push_str(&format!(
					"Showing {} of {} sessions\n\n",
					page_sessions.len(),
					total_sessions
				));

				// Create table header
				markdown_content.push_str("| Name | Created | Model | Tokens | Cost |\n");
				markdown_content.push_str("|------|---------|-------|--------|------|\n");

				// Add table rows
				for (name, info) in page_sessions {
					// Format date from timestamp
					let created_time = DateTime::<Utc>::from_timestamp(info.created_at as i64, 0)
						.map(|dt| dt.naive_local().format("%Y-%m-%d %H:%M").to_string())
						.unwrap_or_default();

					// Determine if this is the current session
					let is_current = match &session.session.session_file {
						Some(path) => {
							path.file_stem().and_then(|s| s.to_str()).unwrap_or("") == name
						}
						None => false,
					};

					let name_display = if is_current {
						format!("**{}** *(current)*", name)
					} else {
						name.clone()
					};

					// Simplify model name - strip provider prefix if present
					let model_parts: Vec<&str> = info.model.split('/').collect();
					let model_name = if model_parts.len() > 1 {
						model_parts[1]
					} else {
						&info.model
					};

					// Calculate total tokens
					let total_tokens = info.input_tokens + info.output_tokens + info.cached_tokens;

					markdown_content.push_str(&format!(
						"| {} | {} | {} | {} | ${:.5} |\n",
						name_display,
						created_time,
						model_name,
						format_number(total_tokens),
						info.total_cost
					));
				}

				// Add navigation info
				markdown_content.push_str("\n## Navigation\n\n");
				if total_pages > 1 {
					if page > 1 {
						markdown_content.push_str(&format!("- Previous: `/list {}`\n", page - 1));
					}
					if page < total_pages {
						markdown_content.push_str(&format!("- Next: `/list {}`\n", page + 1));
					}
					markdown_content.push_str(&format!(
						"- Go to page: `/list <page>` (1-{})\n\n",
						total_pages
					));
				}

				markdown_content.push_str("## Session Management\n\n");
				markdown_content.push_str("- Switch to session: `/session <session_name>`\n");
				markdown_content.push_str("- Create new session: `/session`\n");

				// Render using markdown renderer if enabled
				if config.enable_markdown_rendering {
					let theme = config.markdown_theme.parse().unwrap_or_default();
					let renderer = MarkdownRenderer::with_theme(theme);
					match renderer.render_and_print(&markdown_content) {
						Ok(_) => {
							// Successfully rendered as markdown
						}
						Err(_) => {
							// Fallback to plain text if markdown rendering fails
							display_plain(&markdown_content);
						}
					}
				} else {
					// Use plain text rendering
					display_plain(&markdown_content);
				}
			}
		}
		Err(e) => {
			println!("{}: {}", "Failed to list sessions".bright_red(), e);
		}
	}
	Ok(false)
}

/// Display markdown as plain text (fallback)
fn display_plain(markdown_content: &str) {
	// Convert markdown to plain text for fallback
	let plain_text = markdown_content
		.replace("# ", "")
		.replace("## ", "")
		.replace("**", "")
		.replace("*", "");
	println!("{}", plain_text);
}
