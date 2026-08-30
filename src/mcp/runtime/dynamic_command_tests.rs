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

//! Tests for the `mcp` tool command surface (`execute_mcp_command`): the
//! add/list/remove lifecycle on the dynamic registry plus every parameter
//! validation arm. Connection-making actions (enable) are exercised only on
//! their error paths — nothing here spawns a real server process.

use super::*;
use serial_test::serial;

fn call(params: serde_json::Value) -> crate::mcp::McpToolCall {
	crate::mcp::McpToolCall {
		tool_name: "mcp".to_string(),
		parameters: params,
		tool_id: "t-dyn".to_string(),
	}
}

fn test_config() -> crate::config::Config {
	let mut config: crate::config::Config =
		toml::from_str(include_str!("../../../config-templates/default.toml"))
			.expect("parse default config template");
	config.build_role_map();
	config
}

fn text_of(result: &McpToolResult) -> String {
	result
		.result
		.content
		.iter()
		.filter_map(|block| match block {
			rmcp::model::ContentBlock::Text(t) => Some(t.text.clone()),
			_ => None,
		})
		.collect()
}

fn is_err(result: &McpToolResult) -> bool {
	result.result.is_error.unwrap_or(false)
}

#[tokio::test]
#[serial]
async fn test_missing_and_unknown_action() {
	clear_all();
	let config = test_config();

	let result = execute_mcp_command(&call(serde_json::json!({})), &config)
		.await
		.expect("dispatch");
	assert!(is_err(&result));
	assert!(text_of(&result).contains("action"));

	let result = execute_mcp_command(&call(serde_json::json!({"action": "explode"})), &config)
		.await
		.expect("dispatch");
	assert!(is_err(&result));
	assert!(text_of(&result).contains("Unknown action"));
}

#[tokio::test]
#[serial]
async fn test_add_validation_arms() {
	clear_all();
	let config = test_config();

	// Missing name
	let result = execute_mcp_command(&call(serde_json::json!({"action": "add"})), &config)
		.await
		.expect("dispatch");
	assert!(is_err(&result));
	assert!(text_of(&result).contains("name"));

	// Missing server_type
	let result = execute_mcp_command(
		&call(serde_json::json!({"action": "add", "name": "x"})),
		&config,
	)
	.await
	.expect("dispatch");
	assert!(is_err(&result));
	assert!(text_of(&result).contains("server_type"));

	// stdio without command
	let result = execute_mcp_command(
		&call(serde_json::json!({"action": "add", "name": "x", "server_type": "stdio"})),
		&config,
	)
	.await
	.expect("dispatch");
	assert!(is_err(&result));
	assert!(text_of(&result).contains("command"));
}

#[tokio::test]
#[serial]
async fn test_add_list_remove_lifecycle() {
	clear_all();
	let config = test_config();
	let name = "__cmdtest_srv";

	let result = execute_mcp_command(
		&call(serde_json::json!({
			"action": "add",
			"name": name,
			"server_type": "stdio",
			"command": "echo",
			"args": ["hello"],
			"tools": ["alpha"],
			"timeout_seconds": 5
		})),
		&config,
	)
	.await
	.expect("dispatch");
	assert!(!is_err(&result), "add failed: {}", text_of(&result));

	// Registered as a dynamic (not yet enabled) server — get_all_configs
	// returns only enabled servers, so it must NOT show up there yet
	assert!(is_dynamic(name));
	assert!(list_servers().iter().any(|(n, _, _)| n == name));
	assert!(!get_all_configs().iter().any(|s| s.name() == name));

	// list shows both the configured servers from the role config and ours
	let result = execute_mcp_command(&call(serde_json::json!({"action": "list"})), &config)
		.await
		.expect("dispatch");
	let listing = text_of(&result);
	assert!(listing.contains(name), "dynamic server missing:\n{listing}");

	// remove returns it and it disappears from the registry
	let result = execute_mcp_command(
		&call(serde_json::json!({"action": "remove", "name": name})),
		&config,
	)
	.await
	.expect("dispatch");
	assert!(!is_err(&result), "remove failed: {}", text_of(&result));
	assert!(!is_dynamic(name));

	clear_all();
}

#[tokio::test]
#[serial]
async fn test_enable_disable_persist_unknown_server() {
	clear_all();
	let config = test_config();

	for action in ["enable", "disable", "remove", "persist", "unpersist"] {
		let result = execute_mcp_command(
			&call(serde_json::json!({"action": action, "name": "__cmdtest_nope"})),
			&config,
		)
		.await
		.unwrap_or_else(|e| panic!("{action} dispatch errored: {e}"));
		assert!(
			is_err(&result),
			"{action} on unknown server must report an error, got: {}",
			text_of(&result)
		);
	}
}

// ---------------------------------------------------------------------------
// Parameter-validation arms, persistence round trip (sandboxed config dir),
// session-scoped registry branches, and tool-name lookups.
// ---------------------------------------------------------------------------

struct EnvGuard(Vec<(&'static str, Option<std::ffi::OsString>)>);

impl EnvGuard {
	fn new(keys: &[&'static str]) -> Self {
		Self(keys.iter().map(|k| (*k, std::env::var_os(k))).collect())
	}
}

impl Drop for EnvGuard {
	fn drop(&mut self) {
		for (key, saved) in &self.0 {
			match saved {
				Some(v) => std::env::set_var(key, v),
				None => std::env::remove_var(key),
			}
		}
	}
}

#[tokio::test]
#[serial]
async fn test_add_parameter_validation_arms() {
	clear_all();
	let config = test_config();

	// name: wrong type, empty, missing
	for params in [
		serde_json::json!({"action": "add", "name": 42, "server_type": "stdio", "command": "echo"}),
		serde_json::json!({"action": "add", "name": "   ", "server_type": "stdio", "command": "echo"}),
		serde_json::json!({"action": "add", "server_type": "stdio", "command": "echo"}),
	] {
		let result = execute_mcp_command(&call(params), &config)
			.await
			.expect("dispatch");
		assert!(is_err(&result), "must reject: {}", text_of(&result));
		assert!(text_of(&result).contains("name"));
	}

	// server_type missing / invalid
	let result = execute_mcp_command(
		&call(serde_json::json!({"action": "add", "name": "__dyntest_v"})),
		&config,
	)
	.await
	.expect("dispatch");
	assert!(is_err(&result));
	assert!(text_of(&result).contains("server_type"));

	let result = execute_mcp_command(
		&call(serde_json::json!({"action": "add", "name": "__dyntest_v", "server_type": "grpc"})),
		&config,
	)
	.await
	.expect("dispatch");
	assert!(is_err(&result));
	assert!(text_of(&result).contains("Invalid server_type"));

	// stdio without command
	let result = execute_mcp_command(
		&call(serde_json::json!({"action": "add", "name": "__dyntest_v", "server_type": "stdio"})),
		&config,
	)
	.await
	.expect("dispatch");
	assert!(is_err(&result));
	assert!(text_of(&result).contains("requires 'command'"));

	// http without url
	let result = execute_mcp_command(
		&call(serde_json::json!({"action": "add", "name": "__dyntest_v", "server_type": "http"})),
		&config,
	)
	.await
	.expect("dispatch");
	assert!(is_err(&result));
	assert!(text_of(&result).contains("requires 'url'"));

	clear_all();
}

#[tokio::test]
#[serial]
async fn test_add_duplicate_server_rejected() {
	clear_all();
	let config = test_config();
	let params = serde_json::json!({
		"action": "add",
		"name": "__dyntest_dup",
		"server_type": "stdio",
		"command": "echo"
	});

	let first = execute_mcp_command(&call(params.clone()), &config)
		.await
		.expect("dispatch");
	assert!(!is_err(&first), "first add failed: {}", text_of(&first));

	let second = execute_mcp_command(&call(params), &config)
		.await
		.expect("dispatch");
	assert!(is_err(&second));
	assert!(text_of(&second).contains("already registered"));

	clear_all();
}

#[tokio::test]
#[serial]
async fn test_disable_via_command_flips_enabled_and_empties_functions() {
	clear_all();
	let config = test_config();

	let result = execute_mcp_command(
		&call(serde_json::json!({
			"action": "add",
			"name": "__dyntest_disable",
			"server_type": "stdio",
			"command": "echo",
			"tools": ["alpha"]
		})),
		&config,
	)
	.await
	.expect("dispatch");
	assert!(!is_err(&result), "add failed: {}", text_of(&result));

	// Seed an enabled state with functions, as `enable` would have produced.
	get_manager()
		.write()
		.unwrap()
		.enabled
		.insert("__dyntest_disable".to_string(), true);
	get_manager().write().unwrap().functions.insert(
		"__dyntest_disable".to_string(),
		vec![McpFunction {
			name: "__dyntest_disable_tool".to_string(),
			description: String::new(),
			parameters: serde_json::json!({}),
		}],
	);
	assert!(get_all_configs()
		.iter()
		.any(|s| s.name() == "__dyntest_disable"));

	let result = execute_mcp_command(
		&call(serde_json::json!({"action": "disable", "name": "__dyntest_disable"})),
		&config,
	)
	.await
	.expect("dispatch");
	assert!(!is_err(&result), "disable failed: {}", text_of(&result));
	assert!(text_of(&result).contains("disabled"));

	// Still registered, but disabled: no configs, no functions.
	assert!(is_dynamic("__dyntest_disable"));
	assert!(list_servers()
		.iter()
		.any(|(n, _, e)| n == "__dyntest_disable" && !e));
	assert!(!get_all_configs()
		.iter()
		.any(|s| s.name() == "__dyntest_disable"));
	assert!(get_functions("__dyntest_disable").is_none());

	clear_all();
}

#[tokio::test]
#[serial]
async fn test_persist_unpersist_round_trip_via_command() {
	let env = EnvGuard::new(&["OCTOMIND_DATA_DIR"]);
	let dir = std::env::temp_dir().join(format!("octomind-dyn-persist-{}", std::process::id()));
	if dir.exists() {
		std::fs::remove_dir_all(&dir).expect("clear stale sandbox");
	}
	std::fs::create_dir_all(&dir).expect("create sandbox");
	std::env::set_var("OCTOMIND_DATA_DIR", &dir);

	clear_all();
	crate::config::set_thread_role("developer");
	let config = test_config();

	// Enabled server: persist writes auto_bind = [current role].
	let result = execute_mcp_command(
		&call(serde_json::json!({
			"action": "add",
			"name": "__dyntest_persist",
			"server_type": "stdio",
			"command": "echo"
		})),
		&config,
	)
	.await
	.expect("dispatch");
	assert!(!is_err(&result), "add failed: {}", text_of(&result));
	get_manager()
		.write()
		.unwrap()
		.enabled
		.insert("__dyntest_persist".to_string(), true);

	let result = execute_mcp_command(
		&call(serde_json::json!({"action": "persist", "name": "__dyntest_persist"})),
		&config,
	)
	.await
	.expect("dispatch");
	assert!(!is_err(&result), "persist failed: {}", text_of(&result));
	let msg = text_of(&result);
	assert!(msg.contains("persisted to"), "got: {msg}");
	assert!(
		msg.contains("Auto-bind set to role 'developer'"),
		"got: {msg}"
	);
	assert!(is_persisted("__dyntest_persist"));
	assert!(dir
		.join("config")
		.join("mcp-__dyntest_persist.toml")
		.exists());

	// list marks persisted servers.
	let result = execute_mcp_command(&call(serde_json::json!({"action": "list"})), &config)
		.await
		.expect("dispatch");
	assert!(text_of(&result).contains("💾"));

	let result = execute_mcp_command(
		&call(serde_json::json!({"action": "unpersist", "name": "__dyntest_persist"})),
		&config,
	)
	.await
	.expect("dispatch");
	assert!(!is_err(&result), "unpersist failed: {}", text_of(&result));
	assert!(text_of(&result).contains("unpersisted"));
	assert!(!is_persisted("__dyntest_persist"));

	// Disabled server: persist clears auto_bind.
	let result = execute_mcp_command(
		&call(serde_json::json!({
			"action": "add",
			"name": "__dyntest_persist2",
			"server_type": "stdio",
			"command": "echo"
		})),
		&config,
	)
	.await
	.expect("dispatch");
	assert!(!is_err(&result));
	let result = execute_mcp_command(
		&call(serde_json::json!({"action": "persist", "name": "__dyntest_persist2"})),
		&config,
	)
	.await
	.expect("dispatch");
	assert!(!is_err(&result));
	assert!(text_of(&result).contains("Auto-bind cleared (server disabled)"));
	let _ = execute_mcp_command(
		&call(serde_json::json!({"action": "unpersist", "name": "__dyntest_persist2"})),
		&config,
	)
	.await;

	clear_all();
	drop(env);
	let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
#[serial]
async fn test_session_scoped_registry_branches() {
	let sid = "__dyntest_session".to_string();
	crate::session::context::with_session_id(sid.clone(), async {
		let config = test_config();

		// add lands in the session registry (global stays untouched).
		let result = execute_mcp_command(
			&call(serde_json::json!({
				"action": "add",
				"name": "__dyntest_sess_srv",
				"server_type": "stdio",
				"command": "echo"
			})),
			&config,
		)
		.await
		.expect("dispatch");
		assert!(!is_err(&result), "add failed: {}", text_of(&result));
		assert!(is_dynamic("__dyntest_sess_srv"));
		assert!(list_servers()
			.iter()
			.any(|(n, _, e)| n == "__dyntest_sess_srv" && !e));
		// Registered but not enabled: no functions, no configs, no tool hits.
		assert!(get_functions("__dyntest_sess_srv").is_none());
		assert!(get_all_functions().is_empty());
		assert!(!is_dynamic_by_tool("__dyntest_sess_tool"));
		assert!(get_dynamic_server_name_by_tool("__dyntest_sess_tool").is_none());

		// enable of an unregistered name errors through the session branch.
		let result = execute_mcp_command(
			&call(serde_json::json!({"action": "enable", "name": "__dyntest_sess_nope"})),
			&config,
		)
		.await
		.expect("dispatch");
		assert!(is_err(&result));
		assert!(text_of(&result).contains("not registered"));

		// remove through the session branch.
		let result = execute_mcp_command(
			&call(serde_json::json!({"action": "remove", "name": "__dyntest_sess_srv"})),
			&config,
		)
		.await
		.expect("dispatch");
		assert!(!is_err(&result), "remove failed: {}", text_of(&result));
		assert!(!is_dynamic("__dyntest_sess_srv"));
	})
	.await;
	crate::session::context::cleanup_session(&sid);
}

#[tokio::test]
#[serial]
async fn test_tool_name_lookups_in_cli_mode() {
	clear_all();
	register_server(McpServerConfig::stdin(
		"__dyntest_lookup",
		"echo",
		vec![],
		5,
		vec![],
	))
	.expect("register");
	get_manager().write().unwrap().functions.insert(
		"__dyntest_lookup".to_string(),
		vec![McpFunction {
			name: "__dyntest_lookup_tool".to_string(),
			description: String::new(),
			parameters: serde_json::json!({}),
		}],
	);

	assert!(is_dynamic_by_tool("__dyntest_lookup_tool"));
	assert_eq!(
		get_dynamic_server_name_by_tool("__dyntest_lookup_tool").as_deref(),
		Some("__dyntest_lookup")
	);

	remove_server("__dyntest_lookup");
	assert!(!is_dynamic_by_tool("__dyntest_lookup_tool"));
	assert!(get_dynamic_server_name_by_tool("__dyntest_lookup_tool").is_none());

	clear_all();
}

#[tokio::test]
#[serial]
async fn test_list_output_sections() {
	let env = EnvGuard::new(&["OCTOMIND_DATA_DIR"]);
	let dir = std::env::temp_dir().join(format!("octomind-dyn-list-{}", std::process::id()));
	if dir.exists() {
		std::fs::remove_dir_all(&dir).expect("clear stale sandbox");
	}
	std::fs::create_dir_all(&dir).expect("create sandbox");
	std::env::set_var("OCTOMIND_DATA_DIR", &dir);

	clear_all();

	// Empty: neither configured nor dynamic.
	let mut config = test_config();
	config.mcp.servers.clear();
	let result = execute_mcp_command(&call(serde_json::json!({"action": "list"})), &config)
		.await
		.expect("dispatch");
	assert!(text_of(&result).contains("No MCP servers configured or registered."));

	// Configured section: type, status, tool list.
	let mut config = test_config();
	config.mcp.servers.push(McpServerConfig::builtin(
		"__dyntest_cfg_srv",
		30,
		vec!["tool_one".to_string()],
	));
	let result = execute_mcp_command(&call(serde_json::json!({"action": "list"})), &config)
		.await
		.expect("dispatch");
	let msg = text_of(&result);
	assert!(msg.contains("Configured servers:"), "got: {msg}");
	assert!(
		msg.contains("__dyntest_cfg_srv [builtin] ✓ active"),
		"got: {msg}"
	);
	assert!(msg.contains("tool_one"), "got: {msg}");

	// Dynamic section: disabled marker, all-tools hint.
	let result = execute_mcp_command(
		&call(serde_json::json!({
			"action": "add",
			"name": "__dyntest_extra",
			"server_type": "stdio",
			"command": "echo"
		})),
		&config,
	)
	.await
	.expect("dispatch");
	assert!(!is_err(&result));
	let result = execute_mcp_command(&call(serde_json::json!({"action": "list"})), &config)
		.await
		.expect("dispatch");
	let msg = text_of(&result);
	assert!(msg.contains("Dynamic servers:"), "got: {msg}");
	assert!(msg.contains("__dyntest_extra ✗ disabled"), "got: {msg}");
	assert!(msg.contains("(all tools)"), "got: {msg}");

	clear_all();
	drop(env);
	let _ = std::fs::remove_dir_all(&dir);
}
