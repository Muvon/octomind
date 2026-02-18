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

// Role switching command implementation

use super::super::core::ChatSession;
use super::{CommandOutput, CommandResult};
use crate::config::Config;
use anyhow::Result;

/// Handle /role command for runtime role switching
pub async fn handle_role(
	session: &mut ChatSession,
	config: &Config,
	params: &[&str],
) -> Result<CommandResult> {
	if params.is_empty() {
		// Show current role and available roles
		let available_roles: Vec<String> = config.roles.iter().map(|r| r.name.clone()).collect();

		return Ok(CommandResult::HandledWithOutput(CommandOutput::Role {
			old_role: None,
			new_role: session.role.clone(),
			current_role: Some(session.role.clone()),
			available_roles: Some(available_roles),
			changed: false,
			saved: None,
			save_error: None,
		}));
	}

	let new_role = params[0];

	// Validate role exists
	if !config.roles.iter().any(|r| r.name == new_role) {
		let available_roles: Vec<String> = config.roles.iter().map(|r| r.name.clone()).collect();

		return Ok(CommandResult::HandledWithOutput(CommandOutput::Error {
			error: format!("Invalid role: {}", new_role),
			context: Some(serde_json::json!({
				"available_roles": available_roles
			})),
		}));
	}

	// Don't switch if already using this role
	if session.role == new_role {
		return Ok(CommandResult::HandledWithOutput(CommandOutput::Role {
			old_role: None,
			new_role: new_role.to_string(),
			current_role: Some(new_role.to_string()),
			available_roles: None,
			changed: false,
			saved: None,
			save_error: None,
		}));
	}

	// Get new role configuration
	let (role_config, _, _, _, _) = config.get_role_config(new_role);

	// Update session role and related settings
	let old_role = session.role.clone();
	session.role = new_role.to_string();
	session.temperature = role_config.temperature;

	// Reinitialize the session for the new role (system prompt + MCP servers)
	if let Err(e) = session.reinitialize_for_role(new_role, config).await {
		// If reinitialization fails, revert the role change
		session.role = old_role.clone();
		session.temperature = config.get_role_config(&old_role).0.temperature;

		return Ok(CommandResult::HandledWithOutput(CommandOutput::Error {
			error: format!("Failed to switch role: {}", e),
			context: Some(serde_json::json!({
				"reverted": true
			})),
		}));
	}

	// Log the role change for session restoration
	if let Some(_session_file) = &session.session.session_file {
		let command_line = format!("/role {}", new_role);
		if let Err(e) =
			crate::session::logger::log_session_command(&session.session.info.name, &command_line)
		{
			crate::log_debug!("Warning: Failed to log role change: {}", e);
		}
	}

	// Save session with updated role
	let (saved, save_error) = match session.save() {
		Ok(_) => (Some(true), None),
		Err(e) => (Some(false), Some(e.to_string())),
	};

	Ok(CommandResult::HandledWithOutput(CommandOutput::Role {
		old_role: Some(old_role),
		new_role: new_role.to_string(),
		current_role: None,
		available_roles: None,
		changed: true,
		saved,
		save_error,
	}))
}
