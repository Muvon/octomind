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

// Role switching command implementation.
//
// Supports two kinds of role identifiers (identical to `octomind run <TAG>`):
//   - Plain role name (e.g. `developer`)            → must exist in config.roles
//   - Tap agent tag   (e.g. `developer:general`)    → fetched from tap registry,
//     INPUT/ENV resolved, dep scripts run, manifest merged into the active config
//
// Both paths end with `reinitialize_for_role` which restarts MCP servers and
// rebuilds the system prompt for the new role.

use super::super::core::ChatSession;
use super::{CommandOutput, CommandResult};
use crate::agent::resolver;
use crate::config::Config;
use anyhow::Result;

/// Handle /role command for runtime role switching.
///
/// Accepts either a plain role name defined in `config.roles` or a tap agent
/// tag (`domain:spec`) that is resolved via the same path as `octomind run`.
pub async fn handle_role(
	session: &mut ChatSession,
	config: &mut Config,
	params: &[&str],
) -> Result<CommandResult> {
	if params.is_empty() {
		// Show current role and available roles
		let available_roles: Vec<String> = config.roles.iter().map(|r| r.name.clone()).collect();

		return Ok(CommandResult::HandledWithOutput(Box::new(
			CommandOutput::Role {
				old_role: None,
				new_role: session.role.clone(),
				current_role: Some(session.role.clone()),
				available_roles: Some(available_roles),
				changed: false,
				saved: None,
				save_error: None,
			},
		)));
	}

	let new_role_arg = params[0];
	let is_tap_tag = new_role_arg.contains(':');

	// Plain role: validate up-front against the active config.
	// Tap tag: no pre-validation — resolver will fail loudly if the manifest
	// cannot be fetched or merged.
	if !is_tap_tag && !config.roles.iter().any(|r| r.name == new_role_arg) {
		let available_roles: Vec<String> = config.roles.iter().map(|r| r.name.clone()).collect();

		return Ok(CommandResult::HandledWithOutput(Box::new(
			CommandOutput::Error {
				error: format!("Invalid role: {}", new_role_arg),
				context: Some(serde_json::json!({
					"available_roles": available_roles
				})),
			},
		)));
	}

	// Don't switch if already using this role
	if session.role == new_role_arg {
		return Ok(CommandResult::HandledWithOutput(Box::new(
			CommandOutput::Role {
				old_role: None,
				new_role: new_role_arg.to_string(),
				current_role: Some(new_role_arg.to_string()),
				available_roles: None,
				changed: false,
				saved: None,
				save_error: None,
			},
		)));
	}

	// Snapshot for revert on failure
	let old_role = session.role.clone();
	let old_model = session.model.clone();
	let old_temperature = session.temperature;
	let old_config = config.clone();

	// Resolve the target role. For tap tags this fetches the manifest, resolves
	// INPUT/ENV placeholders, runs dep scripts, and returns a merged config
	// that contains the new [[roles]] entry. For plain roles this is a no-op
	// clone of the current config.
	let resolve_input: Option<&str> = if is_tap_tag { Some(new_role_arg) } else { None };
	let (resolved_config, resolved_role) =
		match resolver::resolve_config_and_role(resolve_input, config, None).await {
			Ok(v) => v,
			Err(e) => {
				return Ok(CommandResult::HandledWithOutput(Box::new(
					CommandOutput::Error {
						error: format!("Failed to resolve role '{}': {}", new_role_arg, e),
						context: None,
					},
				)));
			}
		};

	// For the plain-role path, resolver returns the tag string from config.default
	// as the role — ignore it and use the user's explicit arg. For tap path, it
	// returns the injected role name (identical to the tag).
	let target_role: String = if is_tap_tag {
		resolved_role
	} else {
		new_role_arg.to_string()
	};

	// Commit the merged config into the live session config BEFORE reinit so
	// that downstream code (MCP init, system-prompt build, thread-local config)
	// sees the new servers/roles/settings.
	//
	// We commit the *merged-for-new-role* view (filtered `mcp.servers`,
	// patched `role_map`) — not the raw base config returned by the resolver.
	// Reason: the chat loop's `current_config` (`main_loop.rs:395`) is
	// initialized from `get_merged_config_for_role(...)` at startup, and the
	// MCP routing ownership check in `src/mcp/mod.rs:769-783` inspects
	// `config.mcp.servers` to decide whether a tool's server is owned by the
	// current session. If we wrote the base config here, the new role's
	// merged server list would silently regress to the full disk list,
	// breaking that invariant and producing spurious "belongs to another
	// session" errors after a role swap.
	*config = resolved_config.get_merged_config_for_role(&target_role);

	// Apply role-level settings (temperature, optional model override)
	let (role_config, _, _, _, _) = config.get_role_config(&target_role);
	session.role = target_role.clone();
	session.temperature = role_config.temperature;
	crate::config::set_thread_role(&session.role);
	if let Some(role_model) = &role_config.model {
		session.model = role_model.clone();
	}

	// Reinitialize for the new role: restart MCP servers, rebuild system prompt.
	if let Err(e) = session.reinitialize_for_role(&target_role, config).await {
		// Revert everything
		session.role = old_role.clone();
		session.model = old_model;
		session.temperature = old_temperature;
		crate::config::set_thread_role(&session.role);
		*config = old_config;

		// Best-effort restore of MCP servers for the old role
		if let Err(restore_err) = crate::mcp::initialize_mcp_for_role(&old_role, config).await {
			crate::log_debug!(
				"Failed to restore MCP servers for old role '{}': {}",
				old_role,
				restore_err
			);
		}

		return Ok(CommandResult::HandledWithOutput(Box::new(
			CommandOutput::Error {
				error: format!("Failed to switch role: {}", e),
				context: Some(serde_json::json!({
					"reverted": true
				})),
			},
		)));
	}

	// Log the role change for session restoration
	if let Some(_session_file) = &session.session.session_file {
		let command_line = format!("/role {}", new_role_arg);
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

	Ok(CommandResult::HandledWithOutput(Box::new(
		CommandOutput::Role {
			old_role: Some(old_role),
			new_role: target_role,
			current_role: None,
			available_roles: None,
			changed: true,
			saved,
			save_error,
		},
	)))
}
