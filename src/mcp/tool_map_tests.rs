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

/// Single builtin server no branch in `build_tool_server_map_impl` recognizes —
/// initializes to a deterministic, empty tool map.
fn empty_map_config() -> Config {
	config_with_servers(vec![McpServerConfig::builtin(
		"no-such-builtin",
		30,
		vec![],
	)])
}

/// TOOL_MAP is a process-global OnceLock shared with every other test in this
/// binary; serial tests reset to a pristine state before and after running.
fn reset_tool_map() {
	let state = TOOL_MAP.get_or_init(|| Arc::new(RwLock::new(ToolMapState::default())));
	*state.write().unwrap() = ToolMapState::default();
}

fn sorted(mut names: Vec<String>) -> Vec<String> {
	names.sort();
	names
}

#[serial]
#[tokio::test]
async fn lookup_is_case_sensitive_and_rejects_lookalikes() {
	reset_tool_map();
	initialize_tool_map(&empty_map_config())
		.await
		.expect("init");

	register_dynamic_agent_tool("reviewer");
	assert!(get_server_for_tool("agent_reviewer").is_some());

	// Case variants, suffix lookalikes and the empty name never resolve.
	assert_eq!(get_server_for_tool("Agent_Reviewer"), None);
	assert_eq!(get_server_for_tool("AGENT_REVIEWER"), None);
	assert_eq!(get_server_for_tool("agent_reviewer2"), None);
	assert_eq!(get_server_for_tool(""), None);
	assert_eq!(get_tool_server_name("Agent_Reviewer"), None);

	reset_tool_map();
}

#[serial]
#[tokio::test]
async fn duplicate_dynamic_registration_keeps_single_entry() {
	reset_tool_map();
	initialize_tool_map(&empty_map_config())
		.await
		.expect("init");

	register_dynamic_agent_tool("reviewer");
	register_dynamic_agent_tool("reviewer");
	assert_eq!(
		get_tools_for_server("agent"),
		vec!["agent_reviewer".to_string()],
		"registering the same agent twice must not duplicate the mapping"
	);

	let server = McpServerConfig::builtin("dyn-local", 30, vec![]);
	register_dynamic_server_tools(
		"dyn-local",
		&server,
		&["alpha".to_string(), "alpha".to_string()],
	);
	assert_eq!(
		get_tools_for_server("dyn-local"),
		vec!["alpha".to_string()],
		"duplicate tool names in one registration must collapse"
	);

	reset_tool_map();
}

#[serial]
#[tokio::test]
async fn registration_while_uninitialized_is_invisible() {
	reset_tool_map();
	// TOOL_MAP exists but initialized == false: dynamic registration may write
	// state, yet every lookup must refuse to serve until initialization.
	register_dynamic_agent_tool("reviewer");
	assert!(!is_initialized());
	assert_eq!(get_server_for_tool("agent_reviewer"), None);
	assert!(get_all_tool_names().is_empty());
	assert!(get_all_server_names().is_empty());
	assert!(get_tools_for_server("agent").is_empty());

	reset_tool_map();
}

#[serial]
#[tokio::test]
async fn unregister_unknown_tools_is_a_noop() {
	reset_tool_map();
	initialize_tool_map(&empty_map_config())
		.await
		.expect("init");

	unregister_dynamic_agent_tool("ghost");
	unregister_dynamic_server_tools("ghost-server", &["never_existed".to_string()]);

	assert!(
		is_initialized(),
		"unregistering unknown tools must not deinitialize"
	);
	assert_eq!(get_server_for_tool("never_existed"), None);

	reset_tool_map();
}

#[serial]
#[tokio::test]
async fn dynamic_reregistration_overwrites_previous_server() {
	reset_tool_map();
	initialize_tool_map(&empty_map_config())
		.await
		.expect("init");

	let first = McpServerConfig::builtin("first-server", 30, vec![]);
	let second = McpServerConfig::http("second-server", "http://127.0.0.1:1/mcp", 30, vec![]);
	register_dynamic_server_tools("first-server", &first, &["shared".to_string()]);
	register_dynamic_server_tools("second-server", &second, &["shared".to_string()]);

	assert_eq!(
		get_server_for_tool("shared").map(|s| s.name().to_string()),
		Some("second-server".to_string()),
		"last registration must win for the same tool name"
	);
	assert!(get_tools_for_server("first-server").is_empty());
	assert_eq!(
		get_tools_for_server("second-server"),
		vec!["shared".to_string()]
	);

	reset_tool_map();
}

#[serial]
#[tokio::test]
async fn dynamic_servers_keep_tool_sets_isolated() {
	reset_tool_map();
	initialize_tool_map(&empty_map_config())
		.await
		.expect("init");

	let left = McpServerConfig::builtin("left-server", 30, vec![]);
	let right = McpServerConfig::builtin("right-server", 30, vec![]);
	register_dynamic_server_tools("left-server", &left, &["l1".to_string(), "l2".to_string()]);
	register_dynamic_server_tools("right-server", &right, &["r1".to_string()]);

	assert_eq!(
		sorted(get_tools_for_server("left-server")),
		sorted(vec!["l1".to_string(), "l2".to_string()])
	);
	assert_eq!(get_tools_for_server("right-server"), vec!["r1".to_string()]);
	for tool in ["l1", "l2", "r1"] {
		assert!(get_server_for_tool(tool).is_some(), "{tool} must resolve");
	}
	let servers = get_all_server_names();
	assert!(servers.contains("left-server") && servers.contains("right-server"));

	reset_tool_map();
}

/// `allowed_tools` filtering primitive shared by the tool map build: exact
/// names pass, wildcard patterns are plain prefix matches, empty means no
/// filtering. This is the matcher behind the `"<server>:*"` auto-append.
#[test]
fn allowed_tools_patterns_match_exact_and_wildcard() {
	use crate::mcp::is_tool_allowed_by_patterns as allowed;
	let patterns = |p: &[&str]| p.iter().map(|s| s.to_string()).collect::<Vec<_>>();

	assert!(allowed("anything", &[]), "empty allow-list must not filter");
	assert!(allowed("plan", &patterns(&["plan"])));
	assert!(
		!allowed("planet", &patterns(&["plan"])),
		"exact match must not be a prefix match"
	);
	assert!(allowed("core:plan", &patterns(&["core:*"])));
	assert!(!allowed("runtime:plan", &patterns(&["core:*"])));
	assert!(
		allowed("whatever", &patterns(&["*"])),
		"bare * matches everything"
	);
	assert!(
		allowed("plan", &patterns(&["plan", "other"])),
		"any single match suffices"
	);
}
