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

use anyhow::Result;
use clap::Args;
use colored::*;
use octomind::config::Config;
use octomind::session::chat::{
	block_close_ok, block_line, block_open, block_row, block_row_text, block_section,
	block_section_with,
};
use octomind::session::helper_functions::get_all_placeholders;
use std::env;

#[derive(Args)]
pub struct VarsArgs {
	/// Show preview of placeholder values (3 lines)
	#[arg(short, long)]
	pub preview: bool,

	/// Show full expanded values for placeholders
	#[arg(short, long)]
	pub expand: bool,
}

pub async fn execute(args: &VarsArgs, _config: &Config) -> Result<()> {
	let current_dir = env::current_dir()?;
	let placeholders = get_all_placeholders(&current_dir).await;

	let mode = if args.expand {
		"expand"
	} else if args.preview {
		"preview"
	} else {
		"list"
	};
	block_open("vars", Some(mode));

	let mut sorted_placeholders: Vec<_> = placeholders.iter().collect();
	sorted_placeholders.sort_by_key(|(name, _)| *name);

	let name_width = sorted_placeholders
		.iter()
		.map(|(n, _)| n.len())
		.max()
		.unwrap_or(0)
		.min(24);

	if !args.expand && !args.preview && !sorted_placeholders.is_empty() {
		block_section("placeholders");
	}

	for (placeholder, value) in &sorted_placeholders {
		if args.expand || args.preview {
			let lines: Vec<&str> = value.lines().collect();
			let tokens = octomind::session::estimate_tokens(value);

			if value.trim().is_empty() {
				block_section_with(placeholder, "empty");
			} else if args.expand || (lines.len() <= 5 && tokens <= 200) {
				block_section_with(
					placeholder,
					&format!("{} line(s), {} tokens", lines.len(), tokens),
				);
				for line in value.lines() {
					block_row_text(line);
				}
			} else {
				block_section_with(
					placeholder,
					&format!("{} line(s), {} tokens", lines.len(), tokens),
				);
				let mut preview_lines = Vec::new();
				for line in lines.iter().take(10) {
					let trimmed = line.trim();
					if !trimmed.is_empty() && preview_lines.len() < 3 {
						preview_lines.push(trimmed);
					}
					if preview_lines.len() >= 3 {
						break;
					}
				}
				for line in &preview_lines {
					let display_line = if line.chars().count() > 100 {
						let truncated: String = line.chars().take(97).collect();
						format!("{}…", truncated)
					} else {
						line.to_string()
					};
					block_row_text(&display_line.dimmed().to_string());
				}
				if lines.len() > preview_lines.len() {
					block_row_text(&"…".dimmed().to_string());
				}
			}
		} else {
			let description = match placeholder.as_str() {
				"{{DATE}}" => "Current date and time with timezone",
				"{{SHELL}}" => "Current shell name and version",
				"{{OS}}" => "Operating system information",
				"{{BINARIES}}" => "Available development tools and their versions",
				"{{CWD}}" => "Current working directory",
				"{{ROLE}}" => "Current session role (developer, assistant, etc.)",
				"{{SYSTEM}}" => "Complete system information (date, shell, OS, binaries, CWD)",
				"{{CONTEXT}}" => "Project context information (README, git status, git tree)",
				"{{GIT_STATUS}}" => "Git repository status",
				"{{GIT_TREE}}" => "Git file tree",
				"{{README}}" => "Project README content",
				_ => "Project context variable",
			};
			block_row(placeholder, &description.dimmed().to_string(), name_width);
		}
	}

	if !args.expand && !args.preview {
		block_line(
			&"Use --preview (-p) for preview values or --expand (-e) for full values."
				.dimmed()
				.to_string(),
		);
	}

	block_close_ok(
		"vars",
		Some(&format!("{} placeholder(s)", sorted_placeholders.len())),
	);
	println!();
	Ok(())
}
