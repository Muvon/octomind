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

// Log level command handler

use super::{CommandOutput, CommandResult};
use crate::config::{Config, LogLevel};
use anyhow::Result;

pub fn handle_loglevel(config: &mut Config, params: &[&str]) -> Result<CommandResult> {
	// Handle log level command (runtime-only, does NOT save to disk)
	if params.is_empty() {
		// Show current log level - use system-wide getter
		let current_level = config.get_log_level();

		return Ok(CommandResult::HandledWithOutput(Box::new(
			CommandOutput::Loglevel {
				old_level: None,
				new_level: None,
				current_level: Some(current_level.display_name().to_string()),
				available_levels: vec!["none".to_string(), "info".to_string(), "debug".to_string()],
				changed: false,
			},
		)));
	}

	// Parse the requested log level
	let new_level = match LogLevel::from_display_name(params[0]) {
		Some(level) => level,
		None => {
			return Ok(CommandResult::HandledWithOutput(Box::new(
				CommandOutput::Error {
					error: "Invalid log level. Use: none, info, or debug".to_string(),
					context: Some(serde_json::json!({
						"available_levels": ["none", "info", "debug"]
					})),
				},
			)));
		}
	};

	// Read the level in effect before switching so the renderer can show "from"
	let old_level = config.get_log_level();
	let changed = old_level != new_level;

	// Update ONLY the runtime config, do NOT save to disk
	config.log_level = new_level.clone();

	// Propagate the change to thread-local storage so logging macros use the new level
	crate::config::set_thread_config(config);

	Ok(CommandResult::HandledWithOutput(Box::new(
		CommandOutput::Loglevel {
			old_level: Some(old_level.display_name().to_string()),
			new_level: Some(new_level.display_name().to_string()),
			current_level: None,
			available_levels: vec!["none".to_string(), "info".to_string(), "debug".to_string()],
			changed,
		},
	)))
}
