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

// Log level command handler

use crate::config::{Config, LogLevel};
use anyhow::Result;
use colored::Colorize;

pub fn handle_loglevel(config: &mut Config, params: &[&str]) -> Result<bool> {
	// Handle log level command (runtime-only, does NOT save to disk)
	if params.is_empty() {
		// Show current log level - use system-wide getter
		let current_level = config.get_log_level();

		let level_str = match current_level {
			LogLevel::None => "none",
			LogLevel::Info => "info",
			LogLevel::Debug => "debug",
		};
		println!(
			"{}",
			format!("Current log level: {}", level_str).bright_cyan()
		);
		println!("{}", "Available levels: none, info, debug".bright_yellow());
		println!(
			"{}",
			"Note: Changes are runtime-only and do not persist to config file.".bright_blue()
		);
		return Ok(false);
	}

	// Parse the requested log level
	let new_level = match params[0].to_lowercase().as_str() {
		"none" => LogLevel::None,
		"info" => LogLevel::Info,
		"debug" => LogLevel::Debug,
		_ => {
			println!(
				"{}",
				"Invalid log level. Use: none, info, or debug".bright_red()
			);
			return Ok(false);
		}
	};

	// Update ONLY the runtime config, do NOT save to disk
	config.log_level = new_level.clone();

	// Show the new state
	match new_level {
		LogLevel::None => {
			println!(
				"{}",
				"Log level set to NONE (runtime only).".bright_yellow()
			);
			println!(
				"{}",
				"Only essential information will be displayed.".bright_blue()
			);
		}
		LogLevel::Info => {
			println!("{}", "Log level set to INFO (runtime only).".bright_green());
			println!("{}", "Moderate logging will be shown.".bright_yellow());
		}
		LogLevel::Debug => {
			println!(
				"{}",
				"Log level set to DEBUG (runtime only).".bright_green()
			);
			println!(
				"{}",
				"Detailed logging will be shown for API calls and tool executions.".bright_yellow()
			);
		}
	}
	println!(
		"{}",
		"Note: This change is runtime-only and will not persist after session ends.".bright_blue()
	);

	// Do NOT return Ok(true) - we don't want config reload
	Ok(false)
}
