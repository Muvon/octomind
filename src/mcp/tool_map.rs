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
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock, RwLock};

/// Global tool map singleton - initialized once at startup
static TOOL_MAP: OnceLock<Arc<RwLock<ToolMapState>>> = OnceLock::new();

#[derive(Debug, Clone, Default)]
struct ToolMapState {
	/// Tool name -> Server config mapping
	tool_to_server: HashMap<String, McpServerConfig>,
	/// Tools that originated from the static role config. Capability deactivation
	/// must never evict these — they belong to the role, not to any capability.
	static_tools: HashSet<String>,
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
		state.static_tools = tool_to_server.keys().cloned().collect();
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
		// Never evict tools owned by the static role config — they belong to the
		// role, not to the capability being deactivated. Without this guard,
		// disabling a capability that was enabled on a server already in the
		// static config (e.g. octofs with tools=[]) would silently break dispatch
		// for every tool that server exposes.
		if state.static_tools.contains(tool_name) {
			crate::log_debug!(
				"Skipping unregister of static tool '{}' (capability deactivation must not evict role-owned tools)",
				tool_name
			);
			continue;
		}
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
						// Uncached (like `agent`): the core list depends on config.
						let server_functions = crate::mcp::core::get_all_functions(config);
						crate::mcp::filter_tools_by_patterns(server_functions, server.tools())
					}
					"runtime" => crate::mcp::get_filtered_server_functions(
						"runtime",
						server.tools(),
						crate::mcp::runtime::get_all_functions,
					),
					"orchestration" => crate::mcp::get_filtered_server_functions(
						"orchestration",
						server.tools(),
						crate::mcp::orchestration::get_all_functions,
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
	for server in crate::mcp::runtime::dynamic::get_all_configs() {
		if let Some(functions) = crate::mcp::runtime::dynamic::get_functions(server.name()) {
			for function in functions {
				// Dynamic servers have lower priority than config servers
				tool_map
					.entry(function.name)
					.or_insert_with(|| server.clone());
			}
		}
	}

	// Also include dynamically added agents
	for agent_config in crate::mcp::runtime::dynamic_agents::get_all_configs() {
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
	use serial_test::serial;

	/// Parse the shipped default template and replace the MCP server list.
	/// Builtin-only servers keep every test offline: no stdio spawn, no HTTP.
	fn config_with_servers(servers: Vec<McpServerConfig>) -> Config {
		let mut config: Config =
			toml::from_str(include_str!("../../config-templates/default.toml"))
				.expect("parse default config template");
		config.mcp.servers = servers;
		config
	}

	/// Single builtin server whose name no branch in `build_tool_server_map_impl`
	/// recognizes — initializes to a deterministic, empty tool map.
	fn empty_map_config() -> Config {
		config_with_servers(vec![McpServerConfig::builtin(
			"no-such-builtin",
			30,
			vec![],
		)])
	}

	/// The `core` builtin server with attention enabled, so
	/// `core::get_all_functions` advertises at least the `recall` tool.
	fn core_server_config() -> Config {
		let mut config = config_with_servers(vec![McpServerConfig::builtin("core", 30, vec![])]);
		config.compression.attention.enabled = true;
		config
	}

	/// TOOL_MAP is a process-global OnceLock shared with every other test in
	/// this binary (api_executor_tests, dispatch_tests also initialize it).
	/// Serial tests reset to a pristine uninitialized state before and after
	/// running so they neither observe nor leave foreign state behind.
	fn reset_tool_map() {
		let state = TOOL_MAP.get_or_init(|| Arc::new(RwLock::new(ToolMapState::default())));
		*state.write().unwrap() = ToolMapState::default();
	}

	fn sorted(mut names: Vec<String>) -> Vec<String> {
		names.sort();
		names
	}

	#[serial]
	#[test]
	fn test_tool_map_not_initialized() {
		reset_tool_map();
		// Before initialization, should return None
		assert_eq!(get_server_for_tool("test_tool"), None);
		assert_eq!(get_tool_server_name("test_tool"), None);
		assert!(!is_initialized());
		assert!(get_all_tool_names().is_empty());
	}

	#[serial]
	#[tokio::test]
	async fn initialize_with_unknown_builtin_server_yields_empty_map() {
		reset_tool_map();
		initialize_tool_map(&empty_map_config())
			.await
			.expect("unknown builtin must initialize cleanly");

		assert!(is_initialized());
		assert!(get_all_tool_names().is_empty());
		assert!(get_all_server_names().is_empty());
		assert_eq!(get_server_for_tool("plan"), None);
		assert_eq!(get_tool_server_name("recall"), None);
		assert!(get_tools_for_server("no-such-builtin").is_empty());

		reset_tool_map();
	}

	#[serial]
	#[tokio::test]
	async fn initialize_with_core_server_maps_core_tools() {
		reset_tool_map();
		initialize_tool_map(&core_server_config())
			.await
			.expect("core builtin must initialize cleanly");

		assert!(is_initialized());
		let names = get_all_tool_names();
		assert!(
			names.contains(&"recall".to_string()),
			"attention-gated core tools must be advertised, got: {names:?}"
		);
		for name in &names {
			let server = get_server_for_tool(name).expect("every listed tool must resolve");
			assert_eq!(server.name(), "core");
		}
		assert_eq!(sorted(get_tools_for_server("core")), sorted(names.clone()));
		assert!(get_all_server_names().contains("core"));

		reset_tool_map();
	}

	#[serial]
	#[tokio::test]
	async fn reinit_with_unchanged_config_keeps_dynamically_registered_tools() {
		reset_tool_map();
		let config = empty_map_config();
		initialize_tool_map(&config).await.expect("initial init");
		register_dynamic_agent_tool("reviewer");
		assert!(get_server_for_tool("agent_reviewer").is_some());

		// Same config hash → short-circuit; the map must not be rebuilt.
		initialize_tool_map(&config).await.expect("re-init");
		assert_eq!(
			get_server_for_tool("agent_reviewer").map(|s| s.name().to_string()),
			Some("agent".to_string()),
			"unchanged config must not wipe dynamically registered tools"
		);

		reset_tool_map();
	}

	#[serial]
	#[tokio::test]
	async fn reinit_with_changed_config_rebuilds_and_drops_dynamic_tools() {
		reset_tool_map();
		initialize_tool_map(&core_server_config())
			.await
			.expect("init with core config");
		register_dynamic_agent_tool("reviewer");
		assert!(get_server_for_tool("agent_reviewer").is_some());

		initialize_tool_map(&empty_map_config())
			.await
			.expect("re-init with different config");

		assert!(is_initialized());
		assert_eq!(
			get_server_for_tool("agent_reviewer"),
			None,
			"rebuild must drop tools registered outside the config"
		);
		assert!(!get_all_tool_names().contains(&"recall".to_string()));

		reset_tool_map();
	}

	#[serial]
	#[tokio::test]
	async fn register_dynamic_agent_tool_maps_agent_prefixed_name() {
		reset_tool_map();
		initialize_tool_map(&empty_map_config())
			.await
			.expect("init");

		register_dynamic_agent_tool("reviewer");

		let server = get_server_for_tool("agent_reviewer").expect("agent tool must resolve");
		assert_eq!(server.name(), "agent");
		assert_eq!(server.tools().to_vec(), vec!["agent_reviewer".to_string()]);
		assert_eq!(server.connection_type(), McpConnectionType::Builtin);
		assert_eq!(
			get_tool_server_name("agent_reviewer").as_deref(),
			Some("agent")
		);
		assert_eq!(
			get_tools_for_server("agent"),
			vec!["agent_reviewer".to_string()]
		);
		assert!(get_all_server_names().contains("agent"));
		// Other agent names never resolve.
		assert_eq!(get_server_for_tool("agent_unknown"), None);

		reset_tool_map();
	}

	#[serial]
	#[tokio::test]
	async fn unregister_dynamic_agent_tool_removes_mapping() {
		reset_tool_map();
		initialize_tool_map(&empty_map_config())
			.await
			.expect("init");
		register_dynamic_agent_tool("reviewer");
		assert!(get_server_for_tool("agent_reviewer").is_some());

		unregister_dynamic_agent_tool("reviewer");

		assert_eq!(get_server_for_tool("agent_reviewer"), None);
		assert!(get_tools_for_server("agent").is_empty());

		reset_tool_map();
	}

	#[serial]
	#[tokio::test]
	async fn register_dynamic_server_tools_maps_each_tool() {
		reset_tool_map();
		initialize_tool_map(&empty_map_config())
			.await
			.expect("init");
		let server = McpServerConfig::http("dyn-remote", "http://127.0.0.1:1/mcp", 30, vec![]);

		register_dynamic_server_tools(
			"dyn-remote",
			&server,
			&["alpha".to_string(), "beta".to_string()],
		);

		for tool in ["alpha", "beta"] {
			let resolved = get_server_for_tool(tool).expect("dynamic tool must resolve");
			assert_eq!(resolved.name(), "dyn-remote");
		}
		assert_eq!(
			sorted(get_tools_for_server("dyn-remote")),
			sorted(vec!["alpha".to_string(), "beta".to_string()])
		);
		// Registering an empty tool list is a no-op.
		register_dynamic_server_tools("dyn-remote", &server, &[]);
		assert_eq!(get_all_tool_names().len(), 2);

		reset_tool_map();
	}

	#[serial]
	#[tokio::test]
	async fn unregister_dynamic_server_tools_removes_only_listed_tools() {
		reset_tool_map();
		initialize_tool_map(&empty_map_config())
			.await
			.expect("init");
		let server = McpServerConfig::builtin("dyn-local", 30, vec![]);
		register_dynamic_server_tools(
			"dyn-local",
			&server,
			&["alpha".to_string(), "beta".to_string()],
		);
		register_dynamic_agent_tool("reviewer");

		unregister_dynamic_server_tools("dyn-local", &["alpha".to_string()]);

		assert_eq!(get_server_for_tool("alpha"), None);
		assert!(get_server_for_tool("beta").is_some());
		assert!(get_server_for_tool("agent_reviewer").is_some());

		reset_tool_map();
	}

	#[serial]
	#[tokio::test]
	async fn unregister_dynamic_server_tools_never_evicts_static_tools() {
		reset_tool_map();
		initialize_tool_map(&core_server_config())
			.await
			.expect("init");
		let static_tool = get_all_tool_names()
			.first()
			.expect("core config must expose at least one static tool")
			.clone();

		// Capability-deactivation path: the tool belongs to the static role
		// config, so unregistering it as "dynamic" must be refused.
		unregister_dynamic_server_tools("core", std::slice::from_ref(&static_tool));

		let server = get_server_for_tool(&static_tool)
			.expect("static role-owned tool must survive dynamic unregister");
		assert_eq!(server.name(), "core");

		reset_tool_map();
	}

	#[test]
	fn config_hash_is_stable_for_identical_configs() {
		assert_eq!(
			calculate_config_hash(&empty_map_config()),
			calculate_config_hash(&empty_map_config())
		);
	}

	#[test]
	fn config_hash_tracks_server_name_tools_and_connection_type() {
		let base = config_with_servers(vec![McpServerConfig::builtin("core", 30, vec![])]);
		let renamed = config_with_servers(vec![McpServerConfig::builtin("runtime", 30, vec![])]);
		let with_tools = config_with_servers(vec![McpServerConfig::builtin(
			"core",
			30,
			vec!["plan".to_string()],
		)]);
		let http_variant = config_with_servers(vec![McpServerConfig::http(
			"core",
			"http://127.0.0.1:1/mcp",
			30,
			vec![],
		)]);
		let no_servers = config_with_servers(vec![]);

		let base_hash = calculate_config_hash(&base);
		assert_ne!(
			base_hash,
			calculate_config_hash(&renamed),
			"server name must change the hash"
		);
		assert_ne!(
			base_hash,
			calculate_config_hash(&with_tools),
			"tool list must change the hash"
		);
		assert_ne!(
			base_hash,
			calculate_config_hash(&http_variant),
			"connection type must change the hash"
		);
		assert_ne!(
			base_hash,
			calculate_config_hash(&no_servers),
			"server count must change the hash"
		);
	}
}
