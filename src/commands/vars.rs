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

use crate::session::helper_functions::get_all_placeholders;
use anyhow::Result;
use clap::Args;
use colored::*;
use octomind::config::Config;
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

	println!("{}", "Available placeholders:".bright_blue().bold());
	println!();

	// Sort placeholders by name for consistent output
	let mut sorted_placeholders: Vec<_> = placeholders.iter().collect();
	sorted_placeholders.sort_by_key(|(name, _)| *name);

	for (placeholder, value) in sorted_placeholders {
		print!("{}", placeholder.bright_green().bold());

		if args.expand || args.preview {
			println!(":");
			if value.trim().is_empty() {
				println!("  {}", "(empty)".dimmed());
			} else if args.expand {
				// Show full content
				println!("  {}", value.trim());
			} else {
				// Show preview (current behavior)
				let lines: Vec<&str> = value.lines().collect();
				let tokens = crate::session::estimate_tokens(value);
				if lines.len() <= 5 && tokens <= 200 {
					// Short value, show inline
					println!("  {}", value.trim());
				} else {
					// Long value, show truncated with meaningful preview
					println!(
						"  {}",
						format!("({} lines, {} tokens)", lines.len(), tokens).dimmed()
					);

					// Show first 3 non-empty lines as preview
					let mut preview_lines = Vec::new();
					for line in lines.iter().take(10) {
						// Look at first 10 lines to find 3 non-empty ones
						let trimmed = line.trim();
						if !trimmed.is_empty() && preview_lines.len() < 3 {
							preview_lines.push(trimmed);
						}
						if preview_lines.len() >= 3 {
							break;
						}
					}

					if !preview_lines.is_empty() {
						println!("  {} ", "Preview:".dimmed());
						for line in preview_lines.iter() {
							let display_line = if line.chars().count() > 100 {
								let truncated: String = line.chars().take(97).collect();
								format!("{}...", truncated)
							} else {
								line.to_string()
							};
							println!("    {}", display_line);
						}
						if lines.len() > preview_lines.len() {
							println!("    {}", "...".dimmed());
						}
					}
				}
			}
			println!();
		} else {
			// Show just a brief description
			let description = match placeholder.as_str() {
				"%{DATE}" => "Current date and time with timezone",
				"%{SHELL}" => "Current shell name and version",
				"%{OS}" => "Operating system information",
				"%{BINARIES}" => "Available development tools and their versions",
				"%{CWD}" => "Current working directory",
				"%{SYSTEM}" => "Complete system information (date, shell, OS, binaries, CWD)",
				"%{CONTEXT}" => "Project context information (README, git status, git tree)",
				"%{GIT_STATUS}" => "Git repository status",
				"%{GIT_TREE}" => "Git file tree",
				"%{README}" => "Project README content",
				_ => "Project context variable",
			};
			println!(" - {}", description.dimmed());
		}
	}

	if !args.expand && !args.preview {
		println!();
		println!(
			"{}",
			"Use --preview (-p) to see preview values or --expand (-e) to see full values."
				.yellow()
		);
	}

	Ok(())
}
