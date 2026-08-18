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

// Role-merge chain: how a role name becomes an effective Config (servers,
// tools, system prompt). Split from mod.rs to keep file sizes manageable.

use super::*;

impl Config {
	/// Get configuration for a specific role.
	/// Returns: (role_config, role_mcp_config, layers, commands, system_prompt)
	///
	/// When the role is missing, this used to `panic!` and tear down the
	/// long-running process. Now it logs a loud error and falls back to the
	/// first role in `role_map` so the session stays alive. Callers handling
	/// user-supplied role names should still validate with `has_role` and
	/// return a structured error — the fallback is a safety net, not the
	/// intended path. Truly broken configs (empty `role_map`) still panic
	/// because there is nothing sensible to fall back to.
	pub fn get_role_config(&self, role: &str) -> RoleConfigResult<'_> {
		if let Some(role_config) = self.role_map.get(role) {
			return (
				&role_config.config,
				&role_config.mcp,
				self.layers.as_ref(),
				self.commands.as_ref(),
				&role_config.config.system,
			);
		}

		// Fallback: pick the first role in the map. role_map is preserved as a
		// HashMap so "first" is stable-but-unspecified across runs — good enough
		// for a degraded path that already shouts about itself via log_error.
		let Some((fallback_name, fallback_role)) = self.role_map.iter().next() else {
			// Empty role_map = config loader produced no roles at all. This
			// is a load-time invariant violation, not a per-call problem.
			panic!(
				"CRITICAL CONFIG ERROR: role_map is empty — config loaded with no roles defined. \
				 At least one role must be present."
			);
		};
		let available: Vec<&str> = self.role_map.keys().map(|s| s.as_str()).collect();
		crate::log_error!(
			"Unknown role '{}' — falling back to '{}'. Available roles: {}. \
			 Define '{}' explicitly in your config to silence this warning.",
			role,
			fallback_name,
			available.join(", "),
			role
		);
		(
			&fallback_role.config,
			&fallback_role.mcp,
			self.layers.as_ref(),
			self.commands.as_ref(),
			&fallback_role.config.system,
		)
	}

	/// Get a merged config for a specific role (for backward compatibility)
	/// This creates a new Config with role-specific settings merged into system-wide settings
	pub fn get_merged_config_for_role(&self, mode: &str) -> Config {
		let (_role_config, role_mcp_config, _role_layers_config, commands, system_prompt) =
			self.get_role_config(mode);

		let mut merged = self.clone();

		// CRITICAL FIX: Create a legacy McpConfig for backward compatibility with existing code
		// Use the new runtime injection method to ensure core servers are ALWAYS available
		// Also includes servers that auto-bind to this role.
		let enabled_servers = self.get_enabled_servers_for_role(role_mcp_config, Some(mode));

		crate::log_debug!(
			"TRACE: Role '{}' server_refs: {:?}",
			mode,
			role_mcp_config.server_refs
		);
		crate::log_debug!(
			"TRACE: Found {} enabled servers for role",
			enabled_servers.len()
		);

		for server in &enabled_servers {
			crate::log_debug!("TRACE: Adding server '{}' to merged config", server.name());
		}

		// Auto-bind servers land in enabled_servers but are NOT in role_mcp_config.server_refs.
		// Downstream code reads server_refs in many places (layers, tool filtering, prompt, command executor).
		// To keep everything consistent we:
		//   1. add auto-bind names to server_refs
		//   2. add "<name>:*" patterns to allowed_tools (only when non-empty = restricted mode)
		// Both the returned McpConfig AND the role_map entry are patched so any reader sees the same truth.
		let explicit_refs: std::collections::HashSet<&str> = role_mcp_config
			.server_refs
			.iter()
			.map(|s| s.as_str())
			.collect();
		let auto_bind_names: Vec<String> = enabled_servers
			.iter()
			.map(|s| s.name().to_string())
			.filter(|name| !explicit_refs.contains(name.as_str()))
			.collect();

		let mut patched_server_refs = role_mcp_config.server_refs.clone();
		for name in &auto_bind_names {
			if !patched_server_refs.contains(name) {
				patched_server_refs.push(name.clone());
			}
		}

		let mut patched_allowed_tools = role_mcp_config.allowed_tools.clone();
		if !patched_allowed_tools.is_empty() {
			for name in &auto_bind_names {
				let wildcard = format!("{}:*", name);
				if !patched_allowed_tools.contains(&wildcard) {
					patched_allowed_tools.push(wildcard);
				}
			}
		}

		merged.mcp = McpConfig {
			servers: enabled_servers,
			allowed_tools: patched_allowed_tools.clone(),
		};

		// Patch the role entry in role_map so downstream readers of
		// config.role_map[role].mcp.server_refs see auto-bind servers.
		if let Some(role_entry) = merged.role_map.get_mut(mode) {
			role_entry.mcp.server_refs = patched_server_refs;
			role_entry.mcp.allowed_tools = patched_allowed_tools;
		}

		// Role-specific layers are now managed by workflows
		// Keep merged.layers as original registry for agent tools
		// let enabled_layers = self.get_enabled_layers_for_role(mode);

		merged.commands = commands.cloned();
		merged.system = Some(system_prompt.clone());

		merged
	}

	/// Build the role config used by an interactive CLI session.
	///
	/// Interactive sessions always expose the session-flow tools `schedule` and
	/// `monitor`. If the role already enables the orchestration server, its
	/// existing tool grant is preserved. Otherwise only these two tools are
	/// overlaid; the role does not implicitly gain `tap`.
	pub fn get_merged_config_for_interactive_role(&self, role: &str) -> Config {
		const SERVER: &str = "orchestration";
		const TOOLS: [&str; 2] = ["schedule", "monitor"];

		let mut merged = self.get_merged_config_for_role(role);
		// Builtins need no external configuration — if the registry lacks the
		// orchestration entry (e.g. a minimal user config), synthesize it so
		// interactive sessions always get schedule/monitor.
		let registry_server = self
			.mcp
			.servers
			.iter()
			.find(|server| server.name() == SERVER)
			.cloned()
			.unwrap_or_else(|| {
				McpServerConfig::builtin(SERVER, DEFAULT_MCP_TIMEOUT_SECONDS, Vec::new())
			});

		if let Some(server) = merged
			.mcp
			.servers
			.iter_mut()
			.find(|server| server.name() == SERVER)
		{
			// Empty means every tool on this server is already exposed. For a
			// concrete role filter, union in the two interactive session tools.
			if !server.tools().is_empty() {
				let tools = server.tools_mut();
				for tool in TOOLS {
					if !tools.iter().any(|existing| existing == tool) {
						tools.push(tool.to_string());
					}
				}
			}
		} else {
			let mut server = registry_server;
			*server.tools_mut() = TOOLS.into_iter().map(str::to_string).collect();
			merged.mcp.servers.push(server);
		}

		// Keep the compatibility fields aligned so re-merging this effective
		// config (for example during `/role`) retains the interactive overlay.
		if !merged.mcp.allowed_tools.is_empty() {
			for tool in TOOLS {
				let grant = format!("{SERVER}:{tool}");
				if !merged.mcp.allowed_tools.contains(&grant) {
					merged.mcp.allowed_tools.push(grant);
				}
			}
		}
		if let Some(role_entry) = merged.role_map.get_mut(role) {
			if !role_entry.mcp.server_refs.iter().any(|name| name == SERVER) {
				role_entry.mcp.server_refs.push(SERVER.to_string());
			}
			if !role_entry.mcp.allowed_tools.is_empty() {
				for tool in TOOLS {
					let grant = format!("{SERVER}:{tool}");
					if !role_entry.mcp.allowed_tools.contains(&grant) {
						role_entry.mcp.allowed_tools.push(grant);
					}
				}
			}
		}

		merged
	}
}

#[cfg(test)]
#[path = "merge_tests.rs"]
mod tests;
