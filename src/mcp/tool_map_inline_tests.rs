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

use super::*;
use serial_test::serial;

/// Parse the shipped default template and replace the MCP server list.
/// Builtin-only servers keep every test offline: no stdio spawn, no HTTP.
fn config_with_servers(servers: Vec<McpServerConfig>) -> Config {
	let mut config: Config = toml::from_str(include_str!("../../config-templates/default.toml"))
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
