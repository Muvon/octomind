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

// Layers command handler

use super::super::core::ChatSession;
use super::{CommandOutput, CommandResult};
use crate::config::Config;
use anyhow::Result;

pub async fn handle_layers(
	session: &mut ChatSession,
	config: &mut Config,
	role: &str,
) -> Result<CommandResult> {
	// Toggle layered processing (RUNTIME ONLY - no config file changes)
	let current_role = role; // Use the passed role parameter

	// Toggle the setting for the appropriate role in the runtime config
	if let Some(role) = config.role_map.get_mut(current_role) {
		role.config.enable_layers = !role.config.enable_layers;
	}

	// Get the current state from the updated config
	let is_enabled = config
		.role_map
		.get(current_role)
		.map(|r| r.config.enable_layers)
		.unwrap_or(false);

	// Log the command execution with the actual resulting state
	if let Some(session_file) = &session.session.session_file {
		if let Some(session_name) = session_file.file_stem().and_then(|s| s.to_str()) {
			let command_line = format!(
				"/layers {}",
				if is_enabled { "enabled" } else { "disabled" }
			);
			let _ = crate::session::logger::log_session_command(session_name, &command_line);
		}
	}

	// Build output
	let (saved, save_error) = match session.save() {
		Ok(_) => (Some(true), None),
		Err(e) => (Some(false), Some(e.to_string())),
	};

	Ok(CommandResult::HandledWithOutput(CommandOutput::Layers {
		layers_enabled: is_enabled,
		role: current_role.to_string(),
		saved,
		save_error,
	}))
}
