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

//! Dynamic MCP Server Manager
//!
//! Provides runtime management of MCP servers that can be added/removed
//! during a session. These servers are separate from the config-defined servers
//! and are stored in memory only.

use crate::config::McpServerConfig;
use crate::mcp::{McpFunction, McpToolResult};
use anyhow::Result;
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

/// Dynamic server manager state
struct DynamicManagerState {
	/// Registered servers (name -> config) — registered but not necessarily enabled
	servers: HashMap<String, McpServerConfig>,
	/// Cached functions for each enabled server (name -> functions)
	functions: HashMap<String, Vec<McpFunction>>,
	/// Enabled status per server (name -> bool)
	enabled: HashMap<String, bool>,
}

/// Global dynamic server manager - initialized once
static DYNAMIC_MANAGER: OnceLock<Arc<RwLock<DynamicManagerState>>> = OnceLock::new();

fn get_manager() -> &'static Arc<RwLock<DynamicManagerState>> {
	DYNAMIC_MANAGER.get_or_init(|| {
		Arc::new(RwLock::new(DynamicManagerState {
			servers: HashMap::new(),
			functions: HashMap::new(),
			enabled: HashMap::new(),
		}))
	})
}

/// Register a new dynamic MCP server without connecting to it.
///
/// Stores the config in the registry with enabled=false.
/// Use `enable_server` to connect and activate it.
///
/// Session-aware: uses session-scoped registry when in a session context,
/// falls back to global singleton for CLI mode.
pub fn register_server(server: McpServerConfig) -> Result<()> {
	let server_name = server.name().to_string();

	if server_name.is_empty() {
		anyhow::bail!("server name cannot be empty");
	}

	// Validate server has required fields
	match &server {
		McpServerConfig::Stdin { command, .. } if command.is_empty() => {
			anyhow::bail!("stdin server requires a command");
		}
		McpServerConfig::Http { url, .. } if url.is_empty() => {
			anyhow::bail!("http server requires a url");
		}
		_ => {}
	}

	// Check if we're in a session context
	if let Some(session_id) = crate::session::context::current_session_id() {
		crate::session::context::register_dynamic_server_for_session(&session_id, server);
		return Ok(());
	}

	// Fall back to global singleton for CLI mode
	let manager = get_manager();
	let mut state = manager.write().unwrap();
	if state.servers.contains_key(&server_name) {
		anyhow::bail!(
			"Server '{}' already registered. Use 'remove' first.",
			server_name
		);
	}
	state.servers.insert(server_name.clone(), server);
	state.enabled.insert(server_name, false);
	Ok(())
}

/// Enable a registered server: connect, fetch tools, apply optional filter, mark enabled.
///
/// Returns the list of activated functions.
/// Also registers the tools in the global tool map.
///
/// Session-aware: uses session-scoped registry when in a session context,
/// falls back to global singleton for CLI mode.
pub async fn enable_server(
	name: &str,
	filter_tools: Option<Vec<String>>,
) -> Result<Vec<McpFunction>> {
	// Check if we're in a session context
	let session_id = crate::session::context::current_session_id();

	// Get the server config (from session or global)
	let server = if let Some(ref sid) = session_id {
		// Get from session registry - use get_dynamic_server_for_session which returns
		// the server config regardless of enabled status (add registers with enabled=false)
		crate::session::context::get_dynamic_server_for_session(sid, name)
			.map(|(config, _enabled)| config)
			.ok_or_else(|| anyhow::anyhow!("Server '{}' not registered. Use 'add' first.", name))?
	} else {
		// Fall back to global singleton for CLI mode
		let manager = get_manager();
		let state = manager.read().unwrap();
		state
			.servers
			.get(name)
			.cloned()
			.ok_or_else(|| anyhow::anyhow!("Server '{}' not registered. Use 'add' first.", name))?
	};

	let functions = crate::mcp::server::get_server_functions(&server)
		.await
		.map_err(|e| anyhow::anyhow!("Failed to connect to server '{}': {}", name, e))?;

	// Apply tool filter if provided
	let functions = if let Some(ref filter) = filter_tools {
		if filter.is_empty() {
			functions
		} else {
			crate::mcp::filter_tools_by_patterns(functions, filter)
		}
	} else {
		functions
	};

	crate::log_debug!(
		"Dynamic server '{}' enabled with {} functions",
		name,
		functions.len()
	);

	let tool_names: Vec<String> = functions.iter().map(|f| f.name.clone()).collect();

	// Update the registry (session or global)
	if let Some(ref sid) = session_id {
		crate::session::context::enable_dynamic_server_for_session(sid, name, functions.clone());
	} else {
		// Fall back to global singleton for CLI mode
		let manager = get_manager();
		let mut state = manager.write().unwrap();
		state.functions.insert(name.to_string(), functions.clone());
		state.enabled.insert(name.to_string(), true);
	}

	// Register tools in the global tool map
	crate::mcp::tool_map::register_dynamic_server_tools(name, &server, &tool_names);

	Ok(functions)
}

/// Disable an enabled server: mark disabled and remove cached functions.
///
/// Works on BOTH dynamic and config-loaded servers:
/// - Dynamic server already in session registry → flipped to `enabled=false`.
/// - Config-loaded server (from `config.mcp.servers`, including auto-bound) →
///   registered into the session's dynamic registry as `enabled=false` so
///   subsequent `enable`/`persist` actions can find it. Its tools are stripped
///   from the global TOOL_MAP so the LLM no longer sees them.
///
/// The underlying process (if any) is NOT killed — re-enabling reuses it via
/// the cached function list.
///
/// Also unregisters the tools from the global tool map.
pub fn disable_server(name: &str, config: Option<&crate::config::Config>) -> Result<()> {
	// 1. Session-scoped dynamic registry (preferred path for servers already tracked there).
	if let Some(session_id) = crate::session::context::current_session_id() {
		if crate::session::context::disable_dynamic_server_for_session(&session_id, name) {
			// Server was in the dynamic registry — collect its cached tool names
			// BEFORE disable removed them. `disable_dynamic_server_for_session`
			// already cleared `functions`, so fall back to scanning the tool_map.
			let tool_names = crate::mcp::tool_map::get_tools_for_server(name);
			crate::mcp::tool_map::unregister_dynamic_server_tools(name, &tool_names);
			crate::mcp::server::clear_function_cache_for_server(name);
			return Ok(());
		}

		// 2. Not in dynamic registry — look it up in the merged role config.
		if let Some(server_config) =
			config.and_then(|c| c.mcp.servers.iter().find(|s| s.name() == name))
		{
			// Register a session-scoped "shadow" entry so persist/enable can see it.
			crate::session::context::register_dynamic_server_for_session(
				&session_id,
				server_config.clone(),
			);
			// Strip tools from the global tool_map for this session.
			let tool_names = crate::mcp::tool_map::get_tools_for_server(name);
			crate::mcp::tool_map::unregister_dynamic_server_tools(name, &tool_names);
			crate::mcp::server::clear_function_cache_for_server(name);
			return Ok(());
		}

		anyhow::bail!("Server '{}' not found", name);
	}

	// CLI / non-session mode — fall back to the global singleton.
	let manager = get_manager();
	let mut state = manager.write().unwrap();
	if state.servers.contains_key(name) {
		state.enabled.insert(name.to_string(), false);
		let tool_names: Vec<String> = state
			.functions
			.get(name)
			.map(|f| f.iter().map(|f| f.name.clone()).collect())
			.unwrap_or_default();
		state.functions.remove(name);
		drop(state);

		crate::mcp::tool_map::unregister_dynamic_server_tools(name, &tool_names);
		crate::mcp::server::clear_function_cache_for_server(name);
		return Ok(());
	}
	drop(state);

	// Not dynamic — try the config-loaded servers (CLI path).
	if let Some(server_config) =
		config.and_then(|c| c.mcp.servers.iter().find(|s| s.name() == name))
	{
		// Stuff it into the global singleton so enable/persist find it later.
		let manager = get_manager();
		let mut state = manager.write().unwrap();
		state
			.servers
			.insert(name.to_string(), server_config.clone());
		state.enabled.insert(name.to_string(), false);
		drop(state);

		let tool_names = crate::mcp::tool_map::get_tools_for_server(name);
		crate::mcp::tool_map::unregister_dynamic_server_tools(name, &tool_names);
		crate::mcp::server::clear_function_cache_for_server(name);
		return Ok(());
	}

	anyhow::bail!("Server '{}' not found", name)
}

/// Remove a dynamic MCP server by name
///
/// Returns the removed server config if it existed, or None if not found.
/// Also unregisters the tools from the global tool map.
///
/// Session-aware: uses session-scoped registry when in a session context,
/// falls back to global singleton for CLI mode.
pub fn remove_server(name: &str) -> Option<McpServerConfig> {
	// Check if we're in a session context
	if let Some(session_id) = crate::session::context::current_session_id() {
		// Get tool names before they're removed
		let tool_names: Vec<String> =
			crate::session::context::get_dynamic_server_functions_for_session(&session_id, name)
				.map(|funcs| funcs.iter().map(|f| f.name.clone()).collect())
				.unwrap_or_default();
		let removed = crate::session::context::remove_dynamic_server_for_session(&session_id, name);
		if removed.is_some() {
			// Unregister tools from the global tool map
			crate::mcp::tool_map::unregister_dynamic_server_tools(name, &tool_names);
			// Clear the global function cache so stale definitions don't linger
			crate::mcp::server::clear_function_cache_for_server(name);
		}
		return removed;
	}

	// Fall back to global singleton for CLI mode
	let manager = get_manager();
	let mut state = manager.write().unwrap();
	let tool_names: Vec<String> = state
		.functions
		.get(name)
		.map(|f| f.iter().map(|f| f.name.clone()).collect())
		.unwrap_or_default();
	state.functions.remove(name);
	state.enabled.remove(name);
	let removed = state.servers.remove(name);
	drop(state); // Release lock before calling tool_map

	// Unregister tools from the global tool map
	if removed.is_some() {
		crate::mcp::tool_map::unregister_dynamic_server_tools(name, &tool_names);
		// Clear the global function cache so stale definitions don't linger
		crate::mcp::server::clear_function_cache_for_server(name);
	}
	removed
}

/// List all registered dynamic servers with their tool filter and enabled status.
///
/// Session-aware: uses session-scoped registry when in a session context,
/// falls back to global singleton for CLI mode.
pub fn list_servers() -> Vec<(String, Vec<String>, bool)> {
	// Check if we're in a session context
	if let Some(session_id) = crate::session::context::current_session_id() {
		return crate::session::context::get_dynamic_servers_for_session(&session_id);
	}

	// Fall back to global singleton for CLI mode
	let manager = get_manager();
	let state = manager.read().unwrap();
	state
		.servers
		.iter()
		.map(|(name, config)| {
			let tools = config.tools().to_vec();
			let is_enabled = *state.enabled.get(name).unwrap_or(&false);
			(name.clone(), tools, is_enabled)
		})
		.collect()
}

/// Get functions for a specific dynamic server
///
/// Session-aware: uses session-scoped registry when in a session context,
/// falls back to global singleton for CLI mode.
pub fn get_functions(name: &str) -> Option<Vec<McpFunction>> {
	// Check if we're in a session context
	if let Some(session_id) = crate::session::context::current_session_id() {
		return crate::session::context::get_dynamic_server_functions_for_session(
			&session_id,
			name,
		);
	}

	// Fall back to global singleton for CLI mode
	let manager = get_manager();
	let state = manager.read().unwrap();
	state.functions.get(name).cloned()
}

/// Get all enabled dynamic server configs (for tool map building).
///
/// Session-aware: uses session-scoped registry when in a session context,
/// falls back to global singleton for CLI mode.
pub fn get_all_configs() -> Vec<McpServerConfig> {
	// Check if we're in a session context
	if let Some(session_id) = crate::session::context::current_session_id() {
		return crate::session::context::get_all_dynamic_server_configs_for_session(&session_id);
	}

	// Fall back to global singleton for CLI mode
	let manager = get_manager();
	let state = manager.read().unwrap();
	state
		.servers
		.iter()
		.filter(|(name, _)| *state.enabled.get(*name).unwrap_or(&false))
		.map(|(_, config)| config.clone())
		.collect()
}

/// Get all dynamic server functions (for tool map building)
///
/// Session-aware: uses session-scoped registry when in a session context,
/// falls back to global singleton for CLI mode.
pub fn get_all_functions() -> Vec<McpFunction> {
	// Check if we're in a session context
	if let Some(session_id) = crate::session::context::current_session_id() {
		return crate::session::context::get_all_dynamic_server_functions_for_session(&session_id);
	}

	// Fall back to global singleton for CLI mode
	let manager = get_manager();
	let state = manager.read().unwrap();
	state.functions.values().flatten().cloned().collect()
}

/// Check if a server name is dynamically managed
///
/// Session-aware: uses session-scoped registry when in a session context,
/// falls back to global singleton for CLI mode.
pub fn is_dynamic(name: &str) -> bool {
	// Check if we're in a session context
	if let Some(session_id) = crate::session::context::current_session_id() {
		return crate::session::context::has_dynamic_server(&session_id, name);
	}

	// Fall back to global singleton for CLI mode
	let manager = get_manager();
	let state = manager.read().unwrap();
	state.servers.contains_key(name)
}

/// Check if a tool belongs to any dynamic server (by tool name)
///
/// Session-aware: uses session-scoped registry when in a session context,
/// falls back to global singleton for CLI mode.
pub fn is_dynamic_by_tool(tool_name: &str) -> bool {
	// Check if we're in a session context
	if let Some(session_id) = crate::session::context::current_session_id() {
		return crate::session::context::is_dynamic_server_tool(&session_id, tool_name);
	}

	// Fall back to global singleton for CLI mode
	let manager = get_manager();
	let state = manager.read().unwrap();
	state
		.functions
		.values()
		.any(|funcs| funcs.iter().any(|f| f.name == tool_name))
}

/// Get the dynamic server name for a specific tool (by tool name)
///
/// Session-aware: uses session-scoped registry when in a session context,
/// falls back to global singleton for CLI mode.
pub fn get_dynamic_server_name_by_tool(tool_name: &str) -> Option<String> {
	// Check if we're in a session context
	if let Some(session_id) = crate::session::context::current_session_id() {
		return crate::session::context::get_dynamic_server_name_by_tool(&session_id, tool_name);
	}

	// Fall back to global singleton for CLI mode
	let manager = get_manager();
	let state = manager.read().unwrap();
	for (server_name, funcs) in &state.functions {
		if funcs.iter().any(|f| f.name == tool_name) {
			return Some(server_name.clone());
		}
	}
	None
}

/// Get the persist file path for a server name
fn persist_file_path(name: &str) -> Result<std::path::PathBuf> {
	let config_dir = crate::directories::get_config_dir()?;
	Ok(config_dir.join(format!("mcp-{}.toml", name)))
}

/// Check if a server has been persisted to a config file
pub fn is_persisted(name: &str) -> bool {
	persist_file_path(name).map(|p| p.exists()).unwrap_or(false)
}

/// Result of a persist operation — contains all info needed for the response message.
pub struct PersistResult {
	pub path: std::path::PathBuf,
	/// The auto_bind roles that were written, or None if cleared.
	pub auto_bind: Option<Vec<String>>,
}

/// Persist a registered dynamic server to a TOML config file.
///
/// Writes `<config_dir>/mcp-<name>.toml` with `[[mcp.servers]]` format
/// so it gets auto-loaded and merged on next startup.
///
/// Works on BOTH dynamic and config-loaded servers:
/// - Dynamic server → uses its `enabled` flag from the session/global registry.
/// - Config-loaded server (not in the dynamic registry) → treated as enabled,
///   since config servers that reach this point are actively serving tools.
///   (A `disable` call registers the server into the dynamic registry with
///   `enabled=false`, so the dynamic lookup will pick it up as disabled.)
///
/// `auto_bind` rule:
/// - enabled → `auto_bind = [current_role]` (next session auto-activates it)
/// - disabled → `auto_bind = None` (file persists but won't auto-load)
pub fn persist_server(name: &str, config: Option<&crate::config::Config>) -> Result<PersistResult> {
	// Lookup order: dynamic registry (session or global) first → config servers.
	let (server, is_enabled) =
		if let Some(session_id) = crate::session::context::current_session_id() {
			if let Some(found) =
				crate::session::context::get_dynamic_server_for_session(&session_id, name)
			{
				found
			} else if let Some(server_config) =
				config.and_then(|c| c.mcp.servers.iter().find(|s| s.name() == name))
			{
				// Config-loaded server that was never disabled — treat as enabled.
				(server_config.clone(), true)
			} else {
				anyhow::bail!("Server '{}' not found", name);
			}
		} else {
			let manager = get_manager();
			let state = manager.read().unwrap();
			if let Some(server) = state.servers.get(name).cloned() {
				let is_enabled = *state.enabled.get(name).unwrap_or(&false);
				(server, is_enabled)
			} else {
				drop(state);
				if let Some(server_config) =
					config.and_then(|c| c.mcp.servers.iter().find(|s| s.name() == name))
				{
					(server_config.clone(), true)
				} else {
					anyhow::bail!("Server '{}' not found", name);
				}
			}
		};

	// Determine auto_bind based on enabled state and current role
	let auto_bind = if is_enabled {
		crate::config::get_thread_role().map(|role| vec![role])
	} else {
		None
	};

	// Apply auto_bind change
	let server = server.with_auto_bind(auto_bind.clone());

	// Wrap in the config structure so it serializes as [[mcp.servers]]
	#[derive(serde::Serialize)]
	struct PersistWrapper {
		mcp: PersistMcp,
	}
	#[derive(serde::Serialize)]
	struct PersistMcp {
		servers: Vec<crate::config::McpServerConfig>,
	}

	let wrapper = PersistWrapper {
		mcp: PersistMcp {
			servers: vec![server],
		},
	};

	let toml_str = toml::to_string_pretty(&wrapper)
		.map_err(|e| anyhow::anyhow!("Failed to serialize server config: {}", e))?;

	let path = persist_file_path(name)?;
	std::fs::write(&path, toml_str)
		.map_err(|e| anyhow::anyhow!("Failed to write {}: {}", path.display(), e))?;

	Ok(PersistResult { path, auto_bind })
}

/// Remove a persisted server config file.
///
/// Deletes `<config_dir>/mcp-<name>.toml` if it exists.
pub fn unpersist_server(name: &str) -> Result<()> {
	let path = persist_file_path(name)?;
	if path.exists() {
		std::fs::remove_file(&path)
			.map_err(|e| anyhow::anyhow!("Failed to remove {}: {}", path.display(), e))?;
	}
	Ok(())
}

/// Clear all dynamic servers (useful for testing).
#[cfg(test)]
pub fn clear_all() {
	let manager = get_manager();
	let mut state = manager.write().unwrap();
	state.servers.clear();
	state.functions.clear();
	state.enabled.clear();
}

/// Get the mcp tool definition for managing dynamic servers.
pub fn get_mcp_tool_function() -> McpFunction {
	McpFunction {
		name: "mcp".to_string(),
		description: "Manage MCP servers at runtime without editing config. Use when:\n- You need a tool that's available in an MCP server but not currently configured\n- You want to test a server temporarily before adding to config\n- You need different servers for different tasks\n\nActions:\n- list: Show all MCP servers (configured + dynamic) with status and persistence info\n- add: Register a new MCP server config (does NOT connect yet)\n- enable: Connect to a registered server and activate its tools\n- disable: Deactivate a server's tools (config stays registered)\n- remove: Unregister a server entirely\n- persist: Save a registered server to config dir. If enabled, auto-binds to current role. If disabled, clears auto_bind.\n- unpersist: Remove a persisted server config file".to_string(),
		parameters: json!({
			"type": "object",
			"properties": {
				"action": {
					"type": "string",
					"description": "Action to perform",
					"enum": ["add", "remove", "enable", "disable", "list", "persist", "unpersist"]
				},
				"name": {
					"type": "string",
					"description": "Unique name to identify this server instance"
				},
				"server_type": {
					"type": "string",
					"description": "How to connect to the server",
					"enum": ["stdio", "http"]
				},
				"command": {
					"type": "string",
					"description": "Executable to run (e.g., 'npx', 'uvx', './server'). Required for stdio type."
				},
				"args": {
					"type": "array",
					"items": { "type": "string" },
					"description": "Arguments passed to command (e.g., ['-m', 'mcp-server-github']). Required for stdio type."
				},
				"url": {
					"type": "string",
					"description": "HTTP endpoint of MCP server (e.g., 'http://localhost:3000'). Required for http type."
				},
				"timeout_seconds": {
					"type": "number",
					"description": "How long to wait for server response (default: 60)"
				},
				"tools": {
					"type": "array",
					"items": { "type": "string" },
					"description": "Which tools to expose from this server. Empty = all tools. Supports wildcards (e.g., ['github_*'])"
				}
			},
			"required": ["action"],
			"additionalProperties": false
		}),
	}
}

/// Execute the mcp tool command
pub async fn execute_mcp_command(
	call: &crate::mcp::McpToolCall,
	config: &crate::config::Config,
) -> Result<McpToolResult> {
	let params = &call.parameters;

	// Extract action
	let action = match params.get("action").and_then(|v| v.as_str()) {
		Some(a) => a,
		None => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Missing required parameter 'action'".to_string(),
			));
		}
	};

	match action {
		"list" => handle_list(call, config).await,
		"add" => handle_add(call).await,
		"enable" => handle_enable(call).await,
		"disable" => handle_disable(call, config).await,
		"remove" => handle_remove(call).await,
		"persist" => handle_persist(call, config).await,
		"unpersist" => handle_unpersist(call).await,
		_ => Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!(
				"Unknown action '{}'. Use: add, enable, disable, remove, list, persist, unpersist",
				action
			),
		)),
	}
}

async fn handle_list(
	call: &crate::mcp::McpToolCall,
	config: &crate::config::Config,
) -> Result<McpToolResult> {
	let mut lines = Vec::new();

	// Configured servers from the current role's config (passed directly)
	let configured_servers: Vec<(String, String, Vec<String>)> = config
		.mcp
		.servers
		.iter()
		.map(|s| {
			let type_str = match s.connection_type() {
				crate::config::McpConnectionType::Builtin => "builtin",
				crate::config::McpConnectionType::Http => "http",
				crate::config::McpConnectionType::Stdin => "stdio",
			};
			(
				s.name().to_string(),
				type_str.to_string(),
				s.tools().to_vec(),
			)
		})
		.collect();

	// Dynamic servers (runtime-added, may or may not be enabled)
	let dynamic_servers = list_servers();

	if !configured_servers.is_empty() {
		lines.push("Configured servers:".to_string());
		lines.push("".to_string());
		for (name, type_str, tools) in &configured_servers {
			// Configured servers are always active in the current role
			let status = "✓ active";
			let persisted = if is_persisted(name) { " 💾" } else { "" };
			let tools_str = if tools.is_empty() {
				"(all tools)".to_string()
			} else {
				tools.join(", ")
			};
			lines.push(format!(
				"  {name} [{type_str}] {status}{persisted} → {tools_str}"
			));
		}
	}

	// Only show dynamic servers not already listed as configured
	let configured_names: std::collections::HashSet<&str> = configured_servers
		.iter()
		.map(|(n, _, _)| n.as_str())
		.collect();

	let extra_dynamic: Vec<_> = dynamic_servers
		.iter()
		.filter(|(name, _, _)| !configured_names.contains(name.as_str()))
		.collect();

	if !extra_dynamic.is_empty() {
		if !lines.is_empty() {
			lines.push("".to_string());
		}
		lines.push("Dynamic servers:".to_string());
		lines.push("".to_string());
		for (name, tools, is_enabled) in extra_dynamic {
			let status = if *is_enabled {
				"✓ enabled"
			} else {
				"✗ disabled"
			};
			let persisted = if is_persisted(name) { " 💾" } else { "" };
			let tools_str = if tools.is_empty() {
				"(all tools)".to_string()
			} else {
				tools.join(", ")
			};
			lines.push(format!("  {name} {status}{persisted} → {tools_str}"));
		}
	}

	if lines.is_empty() {
		lines.push("No MCP servers configured or registered.".to_string());
	}

	Ok(McpToolResult::success(
		call.tool_name.clone(),
		call.tool_id.clone(),
		lines.join("\n"),
	))
}

async fn handle_add(call: &crate::mcp::McpToolCall) -> Result<McpToolResult> {
	let params = &call.parameters;

	// Extract required fields
	let name = match params.get("name").and_then(|v| v.as_str()) {
		Some(n) if !n.trim().is_empty() => n.trim().to_string(),
		_ => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Missing required parameter 'name'".to_string(),
			));
		}
	};

	let server_name = name.clone();

	let server_type = match params.get("server_type").and_then(|v| v.as_str()) {
		Some(t) => t,
		None => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Missing required parameter 'server_type' (stdio or http)".to_string(),
			));
		}
	};

	// Get optional fields
	let timeout_seconds = params
		.get("timeout_seconds")
		.and_then(|v| v.as_u64())
		.unwrap_or(60);

	let tools: Vec<String> = params
		.get("tools")
		.and_then(|v| v.as_array())
		.map(|arr| {
			arr.iter()
				.filter_map(|v| v.as_str().map(String::from))
				.collect()
		})
		.unwrap_or_default();

	// Build server config based on type
	let server_config = match server_type {
		"stdio" => {
			let command = match params.get("command").and_then(|v| v.as_str()) {
				Some(c) if !c.trim().is_empty() => c.trim().to_string(),
				_ => {
					return Ok(McpToolResult::error(
						call.tool_name.clone(),
						call.tool_id.clone(),
						"stdio server requires 'command' parameter".to_string(),
					));
				}
			};

			let args: Vec<String> = params
				.get("args")
				.and_then(|v| v.as_array())
				.map(|arr| {
					arr.iter()
						.filter_map(|v| v.as_str().map(String::from))
						.collect()
				})
				.unwrap_or_default();

			McpServerConfig::Stdin {
				name,
				command,
				args,
				timeout_seconds,
				tools,
				auto_bind: None,
			}
		}
		"http" => {
			let url = match params.get("url").and_then(|v| v.as_str()) {
				Some(u) if !u.trim().is_empty() => u.trim().to_string(),
				_ => {
					return Ok(McpToolResult::error(
						call.tool_name.clone(),
						call.tool_id.clone(),
						"http server requires 'url' parameter".to_string(),
					));
				}
			};

			McpServerConfig::Http {
				name,
				url,
				timeout_seconds,
				tools,
				auto_bind: None,
			}
		}
		_ => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Invalid server_type. Use: stdio or http".to_string(),
			));
		}
	};

	match register_server(server_config) {
		Ok(()) => Ok(McpToolResult::success(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!(
				"Server '{}' registered. Use 'enable' to activate it.",
				server_name
			),
		)),
		Err(e) => Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("Failed to register server: {}", e),
		)),
	}
}

async fn handle_enable(call: &crate::mcp::McpToolCall) -> Result<McpToolResult> {
	let params = &call.parameters;

	let name = match params.get("name").and_then(|v| v.as_str()) {
		Some(n) if !n.trim().is_empty() => n.trim().to_string(),
		_ => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Missing required parameter 'name'".to_string(),
			));
		}
	};

	let filter_tools: Option<Vec<String>> =
		params.get("tools").and_then(|v| v.as_array()).map(|arr| {
			arr.iter()
				.filter_map(|v| v.as_str().map(String::from))
				.collect()
		});

	match enable_server(&name, filter_tools).await {
		Ok(functions) => {
			let func_names: Vec<&str> = functions.iter().map(|f| f.name.as_str()).collect();
			Ok(McpToolResult::success(
				call.tool_name.clone(),
				call.tool_id.clone(),
				format!(
					"Server '{}' enabled with {} tools:\n{}",
					name,
					func_names.len(),
					func_names.join(", ")
				),
			))
		}
		Err(e) => Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("Failed to enable server: {}", e),
		)),
	}
}

async fn handle_disable(
	call: &crate::mcp::McpToolCall,
	config: &crate::config::Config,
) -> Result<McpToolResult> {
	let params = &call.parameters;

	let name = match params.get("name").and_then(|v| v.as_str()) {
		Some(n) if !n.trim().is_empty() => n.trim().to_string(),
		_ => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Missing required parameter 'name'".to_string(),
			));
		}
	};

	match disable_server(&name, Some(config)) {
		Ok(()) => Ok(McpToolResult::success(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("Server '{}' disabled.", name),
		)),
		Err(e) => Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("Failed to disable server: {}", e),
		)),
	}
}

async fn handle_remove(call: &crate::mcp::McpToolCall) -> Result<McpToolResult> {
	let params = &call.parameters;

	let name = match params.get("name").and_then(|v| v.as_str()) {
		Some(n) if !n.trim().is_empty() => n.trim().to_string(),
		_ => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Missing required parameter 'name'".to_string(),
			));
		}
	};

	if let Some(_removed) = remove_server(&name) {
		Ok(McpToolResult::success(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("Removed server '{}'", name),
		))
	} else {
		Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("Server '{}' not found in dynamic servers", name),
		))
	}
}

async fn handle_persist(
	call: &crate::mcp::McpToolCall,
	config: &crate::config::Config,
) -> Result<McpToolResult> {
	let params = &call.parameters;

	let name = match params.get("name").and_then(|v| v.as_str()) {
		Some(n) if !n.trim().is_empty() => n.trim().to_string(),
		_ => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Missing required parameter 'name'".to_string(),
			));
		}
	};

	match persist_server(&name, Some(config)) {
		Ok(result) => {
			let msg = match &result.auto_bind {
				Some(roles) => {
					format!(
						"Server '{}' persisted to {}. Auto-bind set to role '{}'.",
						name,
						result.path.display(),
						roles.join(", ")
					)
				}
				None => {
					format!(
						"Server '{}' persisted to {}. Auto-bind cleared (server disabled).",
						name,
						result.path.display()
					)
				}
			};
			Ok(McpToolResult::success(
				call.tool_name.clone(),
				call.tool_id.clone(),
				msg,
			))
		}
		Err(e) => Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("Failed to persist server: {}", e),
		)),
	}
}

async fn handle_unpersist(call: &crate::mcp::McpToolCall) -> Result<McpToolResult> {
	let params = &call.parameters;

	let name = match params.get("name").and_then(|v| v.as_str()) {
		Some(n) if !n.trim().is_empty() => n.trim().to_string(),
		_ => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Missing required parameter 'name'".to_string(),
			));
		}
	};

	if !is_persisted(&name) {
		return Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("Server '{}' is not persisted", name),
		));
	}

	match unpersist_server(&name) {
		Ok(()) => Ok(McpToolResult::success(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!(
				"Server '{}' unpersisted. It will no longer auto-load on startup.",
				name
			),
		)),
		Err(e) => Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("Failed to unpersist server: {}", e),
		)),
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use serial_test::serial;

	#[tokio::test]
	#[serial]
	async fn test_list_empty() {
		clear_all();
		let servers = list_servers();
		assert!(servers.is_empty());
	}

	#[test]
	fn test_mcp_function_definition() {
		let func = get_mcp_tool_function();
		assert_eq!(func.name, "mcp");
		assert!(func.parameters.get("properties").is_some());
	}

	#[tokio::test]
	#[serial]
	async fn test_persist_enabled_server_stores_auto_bind() {
		clear_all();

		// Set the current role (uses global RwLock, survives across threads)
		crate::config::set_thread_role("developer");

		// Register a stdio server
		let server = crate::config::McpServerConfig::stdin(
			"__test_persist_autobind",
			"echo",
			vec!["hello".to_string()],
			60,
			vec![],
		);
		register_server(server).unwrap();

		// Manually mark it as enabled (skip actual connection)
		{
			let manager = get_manager();
			let mut state = manager.write().unwrap();
			state
				.enabled
				.insert("__test_persist_autobind".to_string(), true);
		}

		// Persist — should include auto_bind = ["developer"]
		let result = persist_server("__test_persist_autobind", None).unwrap();

		// Verify the PersistResult
		assert_eq!(
			result.auto_bind,
			Some(vec!["developer".to_string()]),
			"auto_bind should be set to current role"
		);

		// Verify the actual file content
		let content = std::fs::read_to_string(&result.path).unwrap();
		assert!(
			content.contains("auto_bind"),
			"TOML file must contain auto_bind field, got:\n{}",
			content
		);
		assert!(
			content.contains("developer"),
			"TOML file must contain the role name, got:\n{}",
			content
		);

		// Cleanup
		let _ = std::fs::remove_file(&result.path);
		clear_all();
	}

	#[tokio::test]
	#[serial]
	async fn test_persist_disabled_server_clears_auto_bind() {
		clear_all();

		crate::config::set_thread_role("developer");

		// Register but do NOT enable
		let server = crate::config::McpServerConfig::stdin(
			"__test_persist_disabled",
			"echo",
			vec![],
			60,
			vec![],
		);
		register_server(server).unwrap();

		// Persist while disabled — auto_bind should be None
		let result = persist_server("__test_persist_disabled", None).unwrap();

		assert_eq!(
			result.auto_bind, None,
			"auto_bind should be None for disabled server"
		);

		// Verify the file does NOT contain auto_bind
		let content = std::fs::read_to_string(&result.path).unwrap();
		assert!(
			!content.contains("auto_bind"),
			"TOML file must NOT contain auto_bind for disabled server, got:\n{}",
			content
		);

		// Cleanup
		let _ = std::fs::remove_file(&result.path);
		clear_all();
	}
}
