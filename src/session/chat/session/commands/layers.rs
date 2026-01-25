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

// Layers command handler - DEPRECATED: Use workflows instead
// This command is kept for backward compatibility but does nothing

use super::super::core::ChatSession;
use super::{CommandOutput, CommandResult};
use crate::config::Config;
use anyhow::Result;

pub async fn handle_layers(
	session: &mut ChatSession,
	config: &mut Config,
	role: &str,
) -> Result<CommandResult> {
	// DEPRECATED: Layers are now replaced by workflows
	// This command is kept for backward compatibility but returns a deprecation message

	let current_role = role;

	// Check if role has workflow configured
	let has_workflow = config
		.role_map
		.get(current_role)
		.and_then(|r| r.workflow.as_ref())
		.is_some();

	// Log the command execution
	if let Some(session_file) = &session.session.session_file {
		if let Some(session_name) = session_file.file_stem().and_then(|s| s.to_str()) {
			let command_line = "/layers (deprecated)";
			let _ = crate::session::logger::log_session_command(session_name, command_line);
		}
	}

	// Build output
	let (saved, save_error) = match session.save() {
		Ok(_) => (Some(true), None),
		Err(e) => (Some(false), Some(e.to_string())),
	};

	Ok(CommandResult::HandledWithOutput(CommandOutput::Layers {
		layers_enabled: has_workflow,
		role: current_role.to_string(),
		saved,
		save_error,
	}))
}
