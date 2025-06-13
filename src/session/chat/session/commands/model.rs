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

// Model command handler

use super::super::core::ChatSession;
use crate::config::Config;
use anyhow::Result;
use colored::Colorize;

pub fn handle_model(session: &mut ChatSession, config: &Config, params: &[&str]) -> Result<bool> {
	// Handle model command
	if params.is_empty() {
		// Show current model and system default
		println!(
			"{}",
			format!("Current session model: {}", session.model).bright_cyan()
		);

		// Show the system default model
		let system_model = config.get_effective_model();
		println!(
			"{}",
			format!("System default model: {}", system_model).bright_blue()
		);

		println!();
		println!(
			"{}",
			"Note: Use '/model <model-name>' to change the model for this session only."
				.bright_yellow()
		);
		println!(
			"{}",
			"Model changes are runtime-only and won't be saved to config.".bright_yellow()
		);
		return Ok(false);
	}

	// Change to a new model (runtime only)
	let new_model = params.join(" ");
	let old_model = session.model.clone();

	// Log the command execution
	if let Some(session_file) = &session.session.session_file {
		if let Some(session_name) = session_file.file_stem().and_then(|s| s.to_str()) {
			let command_line = format!("/model {}", new_model);
			let _ = crate::session::logger::log_session_command(session_name, &command_line);
		}
	}

	// Update session model (runtime only - don't update config)
	session.model = new_model.clone();
	session.session.info.model = new_model.clone();

	println!(
		"{}",
		format!(
			"Model changed from {} to {} (runtime only)",
			old_model, new_model
		)
		.bright_green()
	);
	println!(
		"{}",
		"Note: This change only affects the current session and won't be saved to config."
			.bright_yellow()
	);

	// Save the session with the updated model info (but not config)
	if let Err(e) = session.save() {
		println!("{} {}", "Warning: Could not save session:".bright_red(), e);
	}

	Ok(false)
}
