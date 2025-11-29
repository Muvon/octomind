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
use super::{CommandOutput, CommandResult};
use crate::config::Config;
use anyhow::Result;

pub fn handle_model(
	session: &mut ChatSession,
	config: &Config,
	params: &[&str],
) -> Result<CommandResult> {
	// Handle model command
	if params.is_empty() {
		// Show current model and system default
		let _system_model = config.get_effective_model();

		// Build JSON output
		return Ok(CommandResult::HandledWithOutput(CommandOutput::Model {
			old_model: None,
			new_model: session.model.clone(),
			changed: false,
			saved: None,
			save_error: None,
		}));
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

	// Build JSON output
	let saved = match session.save() {
		Ok(_) => Some(true),
		Err(e) => {
			return Ok(CommandResult::HandledWithOutput(CommandOutput::Model {
				old_model: Some(old_model),
				new_model,
				changed: true,
				saved: Some(false),
				save_error: Some(e.to_string()),
			}));
		}
	};

	Ok(CommandResult::HandledWithOutput(CommandOutput::Model {
		old_model: Some(old_model),
		new_model,
		changed: true,
		saved,
		save_error: None,
	}))
}
