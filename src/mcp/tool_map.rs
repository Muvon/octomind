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

//! Tool Map Management - Application-level singleton for tool-to-server mapping
//!
//! This module provides a thread-safe, static tool map that is initialized once
//! at application startup and reused throughout the application lifetime.
//!
//! The tool map is built after MCP servers have been initialized and their
//! functions have been discovered. This eliminates the need to rebuild the
//! tool map on every tool execution or display operation.

use crate::config::{Config, McpServerConfig};
use crate::mcp::McpConnectionType;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

/// Global tool map singleton - initialized once at startup
static TOOL_MAP: OnceLock<Arc<RwLock<ToolMapState>>> = OnceLock::new();

#[derive(Debug, Clone, Default)]
struct ToolMapState {
	/// Tool name -> Server config mapping
	tool_to_server: HashMap<String, McpServerConfig>,
	/// Whether the tool map has been successfully initialized
	initialized: bool,
	/// Configuration hash used to detect if reinitialization is needed
	config_hash: u64,
}

/// Initialize the global tool map after MCP servers have been started
///
/// This function should be called AFTER `initialize_servers_for_role()` has completed
/// successfully. It builds the tool-to-server mapping by discovering functions from
/// all enabled servers.
///
/// # Arguments
/// * `config` - The merged configuration for the current role
///
/// # Returns
/// * `Ok(())` if initialization succeeded
/// * `Err(...)` if initialization failed (tool map remains uninitialized)
///
/// # Thread Safety
/// This function can be called multiple times safely. Subsequent calls will
/// only reinitialize if the configuration has changed.
pub async fn initialize_tool_map(config: &Config) -> Result<()> {
	let config_hash = calculate_config_hash(config);

	// Get or create the tool map state
	let tool_map_state = TOOL_MAP.get_or_init(|| Arc::new(RwLock::new(ToolMapState::default())));

	// Check if we need to (re)initialize
	{
		let state = tool_map_state.read().unwrap();
		if state.initialized && state.config_hash == config_hash {
			crate::log_debug!("Tool map already initialized with current config");
			return Ok(());
		}
	}

	crate::log_debug!("Building tool-to-server map...");

	// Build the tool map
	let tool_to_server = build_tool_server_map_impl(config).await?;

	// Update the state
	{
		let mut state = tool_map_state.write().unwrap();
		state.tool_to_server = tool_to_server;
		state.initialized = true;
		state.config_hash = config_hash;

		crate::log_debug!(
			"Tool map initialized with {} tools",
			state.tool_to_server.len()
		);
	}

	Ok(())
}

/// Get the server configuration for a specific tool
///
/// # Arguments
/// * `tool_name` - The name of the tool to look up
///
/// # Returns
/// * `Some(server_config)` if the tool is found
/// * `None` if the tool is not found or tool map is not initialized
///
/// # Fallback Behavior
/// If the tool map is not initialized, this function returns `None` and the
/// caller should fall back to the original `build_tool_server_map()` logic.
pub fn get_server_for_tool(tool_name: &str) -> Option<McpServerConfig> {
	let tool_map_state = TOOL_MAP.get()?;
	let state = tool_map_state.read().unwrap();

	if !state.initialized {
		crate::log_debug!("Tool map not initialized, falling back to original logic");
		return None;
	}

	state.tool_to_server.get(tool_name).cloned()
}

/// Get the server name for a specific tool (for display purposes)
///
/// # Arguments
/// * `tool_name` - The name of the tool to look up
///
/// # Returns
/// * Server name if found, "unknown" if not found or not initialized
///
/// # Fallback Behavior
/// If the tool map is not initialized, returns "unknown" and the caller
/// should use the async `get_tool_server_name_async()` fallback.
pub fn get_tool_server_name(tool_name: &str) -> Option<String> {
	get_server_for_tool(tool_name).map(|server| server.name().to_string())
}

/// Check if the tool map has been successfully initialized
///
/// # Returns
/// * `true` if the tool map is ready for use
/// * `false` if the tool map is not initialized (use fallback logic)
pub fn is_initialized() -> bool {
	TOOL_MAP
		.get()
		.map(|state| state.read().unwrap().initialized)
		.unwrap_or(false)
}

/// Get all available tools from the initialized tool map
///
/// # Returns
/// * Vector of tool names if initialized
/// * Empty vector if not initialized
pub fn get_all_tool_names() -> Vec<String> {
	let tool_map_state = match TOOL_MAP.get() {
		Some(state) => state,
		None => return Vec::new(),
	};

	let state = tool_map_state.read().unwrap();
	if !state.initialized {
		return Vec::new();
	}

	state.tool_to_server.keys().cloned().collect()
}

/// Get all tool names that belong to a given server in the initialized tool map.
///
/// Used by the `mcp` tool's `disable` action to temporarily strip a
/// config-loaded server's tools from the session's tool map.
///
/// # Returns
/// * Vector of tool names belonging to `server_name`.
/// * Empty vector if not initialized or the server has no tools registered.
pub fn get_tools_for_server(server_name: &str) -> Vec<String> {
	let tool_map_state = match TOOL_MAP.get() {
		Some(state) => state,
		None => return Vec::new(),
	};

	let state = tool_map_state.read().unwrap();
	if !state.initialized {
		return Vec::new();
	}

	state
		.tool_to_server
		.iter()
		.filter_map(|(tool, server)| {
			if server.name() == server_name {
				Some(tool.clone())
			} else {
				None
			}
		})
		.collect()
}

/// Get all unique server names from the initialized tool map
///
/// # Returns
/// * Set of server names if initialized
/// * Empty set if not initialized
pub fn get_all_server_names() -> std::collections::HashSet<String> {
	let tool_map_state = match TOOL_MAP.get() {
		Some(state) => state,
		None => return std::collections::HashSet::new(),
	};

	let state = tool_map_state.read().unwrap();
	if !state.initialized {
		return std::collections::HashSet::new();
	}

	state
		.tool_to_server
		.values()
		.map(|server| server.name().to_string())
		.collect()
}

/// Register a dynamic agent tool in the tool map
///
/// Call this when an agent is enabled to make its tool available.
pub fn register_dynamic_agent_tool(agent_name: &str) {
	let tool_map_state = match TOOL_MAP.get() {
		Some(state) => state,
		None => {
			crate::log_debug!("Tool map not initialized, cannot register dynamic agent");
			return;
		}
	};

	let tool_name = format!("agent_{}", agent_name);
	let agent_server = McpServerConfig::Builtin {
		name: "agent".to_string(),
		timeout_seconds: 300,
		tools: vec![tool_name.clone()],
		auto_bind: None,
	};

	let mut state = tool_map_state.write().unwrap();
	state.tool_to_server.insert(tool_name.clone(), agent_server);
	crate::log_debug!("Registered dynamic agent tool: {}", tool_name);
}

/// Unregister a dynamic agent tool from the tool map
///
/// Call this when an agent is disabled or removed.
pub fn unregister_dynamic_agent_tool(agent_name: &str) {
	let tool_map_state = match TOOL_MAP.get() {
		Some(state) => state,
		None => {
			crate::log_debug!("Tool map not initialized, cannot unregister dynamic agent");
			return;
		}
	};

	let tool_name = format!("agent_{}", agent_name);
	let mut state = tool_map_state.write().unwrap();
	state.tool_to_server.remove(&tool_name);
	crate::log_debug!("Unregistered dynamic agent tool: {}", tool_name);
}

/// Register all tools from a dynamic MCP server in the tool map
///
/// Call this when a server is enabled to make its tools available.
pub fn register_dynamic_server_tools(
	server_name: &str,
	server_config: &McpServerConfig,
	tool_names: &[String],
) {
	let tool_map_state = match TOOL_MAP.get() {
		Some(state) => state,
		None => {
			crate::log_debug!("Tool map not initialized, cannot register dynamic server");
			return;
		}
	};

	let mut state = tool_map_state.write().unwrap();
	for tool_name in tool_names {
		state
			.tool_to_server
			.insert(tool_name.clone(), server_config.clone());
		crate::log_debug!("Registered dynamic server tool: {}", tool_name);
	}
	crate::log_debug!(
		"Registered {} tools from dynamic server '{}'",
		tool_names.len(),
		server_name
	);
}

/// Unregister all tools from a dynamic MCP server from the tool map
///
/// Call this when a server is disabled or removed.
pub fn unregister_dynamic_server_tools(server_name: &str, tool_names: &[String]) {
	let tool_map_state = match TOOL_MAP.get() {
		Some(state) => state,
		None => {
			crate::log_debug!("Tool map not initialized, cannot unregister dynamic server");
			return;
		}
	};

	let mut state = tool_map_state.write().unwrap();
	for tool_name in tool_names {
		state.tool_to_server.remove(tool_name);
		crate::log_debug!("Unregistered dynamic server tool: {}", tool_name);
	}
	crate::log_debug!(
		"Unregistered {} tools from dynamic server '{}'",
		tool_names.len(),
		server_name
	);
}
/// Build the tool-to-server mapping
///
/// Creates a mapping from tool names to their server configurations.
async fn build_tool_server_map_impl(config: &Config) -> Result<HashMap<String, McpServerConfig>> {
	let mut tool_map = HashMap::new();
	let enabled_servers: Vec<McpServerConfig> = config.mcp.servers.to_vec();
	let session_id = crate::session::context::current_session_id();

	for server in enabled_servers {
		// Skip config servers that are disabled in the dynamic registry
		if let Some(ref sid) = session_id {
			if let Some((_, enabled)) =
				crate::session::context::get_dynamic_server_for_session(sid, server.name())
			{
				if !enabled {
					continue;
				}
			}
		}

		// Get all functions this server provides
		let server_functions = match server.connection_type() {
			McpConnectionType::Builtin => {
				match server.name() {
					"core" => {
						// Developer server only has shell and other dev tools
						crate::mcp::get_filtered_server_functions(
							"core",
							server.tools(),
							crate::mcp::core::get_all_functions,
						)
					}
					"runtime" => crate::mcp::get_filtered_server_functions(
						"runtime",
						server.tools(),
						crate::mcp::runtime::get_all_functions,
					),
					"agent" => {
						// For agent server, get all agent functions based on config
						// Don't cache agent functions since they depend on config
						let server_functions = crate::mcp::agent::get_all_functions(config);
						crate::mcp::filter_tools_by_patterns(server_functions, server.tools())
					}

					_ => {
						crate::log_debug!("Unknown builtin server: {}", server.name());
						Vec::new()
					}
				}
			}
			McpConnectionType::Http | McpConnectionType::Stdin => {
				// For external servers, get their actual functions
				match crate::mcp::server::get_server_functions_cached(&server).await {
					Ok(functions) => {
						crate::mcp::filter_tools_by_patterns(functions, server.tools())
					}
					Err(e) => {
						crate::log_error!(
							"Server '{}' is not available: {}. Verify the server is running at the configured URL.",
							server.name(),
							e
						);
						Vec::new()
					}
				}
			}
		};

		// Map each function name to this server
		for function in server_functions {
			// CONFIGURATION ORDER PRIORITY: First server wins for each tool
			tool_map
				.entry(function.name)
				.or_insert_with(|| server.clone());
		}
	}

	// Also include dynamically added servers
	for server in crate::mcp::core::dynamic::get_all_configs() {
		if let Some(functions) = crate::mcp::core::dynamic::get_functions(server.name()) {
			for function in functions {
				// Dynamic servers have lower priority than config servers
				tool_map
					.entry(function.name)
					.or_insert_with(|| server.clone());
			}
		}
	}

	// Also include dynamically added agents
	for agent_config in crate::mcp::core::dynamic_agents::get_all_configs() {
		let tool_name = format!("agent_{}", agent_config.name);
		let agent_server = McpServerConfig::Builtin {
			name: "agent".to_string(),
			timeout_seconds: 300,
			tools: vec![tool_name.clone()],
			auto_bind: None,
		};
		tool_map.entry(tool_name).or_insert_with(|| agent_server);
	}

	// Project-local tools — `<workdir>/.agents/tools/<name>` shebang scripts.
	// Lowest priority: `or_insert` keeps config/dynamic winners on collision,
	// so a script can never shadow a real tool by accident.
	for func in crate::mcp::core::local_tool::get_all_functions() {
		let local_server = McpServerConfig::Builtin {
			name: crate::mcp::core::local_tool::SERVER_NAME.to_string(),
			timeout_seconds: 300,
			tools: vec![func.name.clone()],
			auto_bind: None,
		};
		tool_map.entry(func.name).or_insert(local_server);
	}

	Ok(tool_map)
}

/// Calculate a hash of the configuration to detect changes
///
/// This is used to determine if the tool map needs to be rebuilt when
/// the configuration changes.
fn calculate_config_hash(config: &Config) -> u64 {
	use std::collections::hash_map::DefaultHasher;
	use std::hash::{Hash, Hasher};

	let mut hasher = DefaultHasher::new();

	// Hash the MCP server configuration
	for server in &config.mcp.servers {
		server.name().hash(&mut hasher);
		server.connection_type().hash(&mut hasher);
		server.tools().hash(&mut hasher);
	}

	hasher.finish()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_tool_map_not_initialized() {
		// Before initialization, should return None
		assert_eq!(get_server_for_tool("test_tool"), None);
		assert_eq!(get_tool_server_name("test_tool"), None);
		assert!(!is_initialized());
		assert!(get_all_tool_names().is_empty());
	}
}
