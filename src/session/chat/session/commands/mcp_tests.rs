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

//! Handler-level tests for the `/mcp` session command: subcommand dispatch,
//! JSON payload shapes, builtin-server enumeration, and the empty-config
//! branches. Dispatch-level smoke coverage lives in dispatch_tests.rs.

use super::*;

fn template_config() -> Config {
	let mut config: Config =
		toml::from_str(include_str!("../../../../../config-templates/default.toml"))
			.expect("parse default config template");
	config.build_role_map();
	config
}

/// The template's server registry emptied: every role's server_refs then
/// resolve to nothing, exercising the "No MCP servers configured" branches.
fn empty_servers_config() -> Config {
	let mut config = template_config();
	config.mcp.servers.clear();
	config
}

/// The handlers group tools by server through the global tool map; build it
/// from this config so grouping reflects the real builtin servers.
async fn init_tool_map(config: &Config) {
	crate::mcp::tool_map::initialize_tool_map(&config.get_merged_config_for_role("assistant"))
		.await
		.expect("init tool map");
}

/// Run one `/mcp` invocation and return `(mcp_command, data)`.
async fn run(config: &Config, params: &[&str]) -> (String, serde_json::Value) {
	let result = handle_mcp(config, "assistant", params)
		.await
		.unwrap_or_else(|e| panic!("mcp {params:?} errored: {e}"));
	let CommandResult::HandledWithOutput(output) = result else {
		panic!("expected typed output");
	};
	let CommandOutput::Mcp { mcp_command, data } = *output else {
		panic!("expected Mcp output");
	};
	(mcp_command, data)
}

#[tokio::test]
async fn test_bare_command_defaults_to_info() {
	let config = empty_servers_config();
	let (mcp_command, data) = run(&config, &[]).await;
	assert_eq!(mcp_command, "");
	assert_eq!(data["subcommand"], "info");
	assert_eq!(data["message"], "No MCP servers configured");
}

#[tokio::test]
async fn test_unknown_subcommand_is_rejected() {
	let config = empty_servers_config();
	let (mcp_command, data) = run(&config, &["frobnicate"]).await;
	assert_eq!(mcp_command, "invalid");
	assert_eq!(data["subcommand"], "invalid");
	assert_eq!(data["message"], "Invalid MCP subcommand");
}

#[tokio::test]
async fn test_info_with_no_servers() {
	let config = empty_servers_config();
	let (mcp_command, data) = run(&config, &["info"]).await;
	assert_eq!(mcp_command, "");
	assert_eq!(data["subcommand"], "info");
	assert_eq!(data["message"], "No MCP servers configured");
	assert_eq!(data["servers"].as_array().map(Vec::len), Some(0));
}

#[tokio::test]
async fn test_full_with_no_servers() {
	let config = empty_servers_config();
	let (mcp_command, data) = run(&config, &["full"]).await;
	assert_eq!(mcp_command, "full");
	assert_eq!(data["message"], "No MCP servers configured");
	assert_eq!(data["servers"].as_array().map(Vec::len), Some(0));
}

#[tokio::test]
async fn test_health_with_no_servers() {
	let config = empty_servers_config();
	let (mcp_command, data) = run(&config, &["health"]).await;
	assert_eq!(mcp_command, "health");
	assert_eq!(data["message"], "No MCP servers configured");
}

#[tokio::test]
async fn test_list_with_no_servers_reports_zero_tools() {
	let config = empty_servers_config();
	let (_, data) = run(&config, &["list"]).await;
	assert_eq!(data["subcommand"], "list");
	assert_eq!(data["total_tools"], 0);
	assert_eq!(
		data["servers"].as_object().map(serde_json::Map::len),
		Some(0)
	);
}

#[tokio::test]
async fn test_dump_with_no_servers_is_empty() {
	let config = empty_servers_config();
	let (mcp_command, data) = run(&config, &["dump"]).await;
	assert_eq!(mcp_command, "dump");
	assert_eq!(data["total_tools"], 0);
	assert_eq!(data["tools"].as_array().map(Vec::len), Some(0));
}

#[tokio::test]
async fn test_validate_with_no_servers_is_trivially_valid() {
	let config = empty_servers_config();
	let (mcp_command, data) = run(&config, &["validate"]).await;
	assert_eq!(mcp_command, "validate");
	assert_eq!(data["all_valid"], true);
	assert_eq!(data["tools"].as_array().map(Vec::len), Some(0));
}

#[tokio::test]
async fn test_list_enumerates_builtin_servers_and_tools() {
	let config = template_config();
	init_tool_map(&config).await;
	let (_, data) = run(&config, &["list"]).await;

	let servers = data["servers"].as_object().expect("servers object");
	let core_tools = servers
		.get("core")
		.and_then(|v| v.as_array())
		.expect("core server listed");
	assert!(!core_tools.is_empty(), "core exposes no tools: {data}");
	assert!(
		core_tools.iter().any(|t| t == "recall"),
		"core must expose `recall`: {data}"
	);
	assert!(
		data["total_tools"].as_u64().unwrap_or_default() > 0,
		"no tools enumerated: {data}"
	);
}

#[tokio::test]
async fn test_info_reports_builtin_servers_as_running() {
	let config = template_config();
	init_tool_map(&config).await;
	let (_, data) = run(&config, &["info"]).await;

	let servers = data["servers"].as_array().expect("servers array");
	let names: Vec<&str> = servers
		.iter()
		.filter_map(|s| s.get("name").and_then(|v| v.as_str()))
		.collect();
	for expected in ["core", "runtime", "agent", "orchestration"] {
		assert!(
			names.contains(&expected),
			"missing server {expected}: {data}"
		);
	}
	for server in servers {
		if server["connection_type"] == "builtin" {
			assert_eq!(
				server["health"], "running",
				"builtin must be running: {server}"
			);
		}
	}
	assert!(
		data["total_tools"].as_u64().unwrap_or_default() > 0,
		"no tools enumerated: {data}"
	);
	assert!(
		!data["tools"].as_object().expect("tools object").is_empty(),
		"no tool groups: {data}"
	);
}

#[tokio::test]
async fn test_full_includes_parameter_schemas() {
	let config = template_config();
	init_tool_map(&config).await;
	let (mcp_command, data) = run(&config, &["full"]).await;
	assert_eq!(mcp_command, "full");

	let tools = data["tools"].as_object().expect("tools object");
	assert!(!tools.is_empty(), "no tool groups: {data}");
	for (_server, entries) in tools {
		for entry in entries.as_array().expect("tool entries") {
			assert!(entry.get("parameters").is_some(), "missing schema: {entry}");
			assert!(
				entry.get("description").is_some(),
				"missing description: {entry}"
			);
		}
	}
}

#[tokio::test]
async fn test_validate_builtin_schemas_are_well_formed() {
	let config = template_config();
	init_tool_map(&config).await;
	let (_, data) = run(&config, &["validate"]).await;

	let tools = data["tools"].as_array().expect("tools array");
	assert!(!tools.is_empty(), "no tools validated: {data}");
	for tool in tools {
		let valid = tool["valid"].as_bool().expect("valid flag");
		let issues = tool["issues"].as_array().expect("issues array");
		assert_eq!(
			valid,
			issues.is_empty(),
			"valid flag disagrees with issues: {tool}"
		);
	}
	assert_eq!(data["all_valid"], true, "builtin schemas regressed: {data}");
}

#[tokio::test]
async fn test_dump_totals_match_tool_list() {
	let config = template_config();
	init_tool_map(&config).await;
	let (_, data) = run(&config, &["dump"]).await;

	let tools = data["tools"].as_array().expect("tools array");
	let total = data["total_tools"].as_u64().expect("total_tools");
	assert_eq!(tools.len() as u64, total, "dump count mismatch: {data}");
	assert!(total > 0, "dump enumerated nothing: {data}");
}

/// A stdio server whose binary cannot exist — health checks must classify it
/// without ever spawning a process.
fn stdio_server(name: &str) -> crate::config::McpServerConfig {
	crate::config::McpServerConfig::Stdin {
		name: name.to_string(),
		command: "/nonexistent/cov-mcp-server".to_string(),
		args: Vec::new(),
		timeout_seconds: 30,
		tools: Vec::new(),
		env: Default::default(),
		cwd: None,
		auto_bind: None,
	}
}

/// Template config whose server registry holds ONLY the given servers, with
/// the assistant role's server_refs pointed at exactly those names — the
/// role's refs (not the raw registry) decide what the merged config exposes.
fn config_with_only_servers(servers: Vec<crate::config::McpServerConfig>) -> Config {
	let mut config = template_config();
	let names: Vec<String> = servers.iter().map(|s| s.name().to_string()).collect();
	config.mcp.servers = servers;
	if let Some(role) = config.role_map.get_mut("assistant") {
		role.mcp.server_refs = names;
		// Non-empty allowed_tools silently DROPS servers that match no
		// pattern (config/mcp.rs get_enabled_servers) — clear it so the
		// fixture servers are not filtered out of the merged config.
		role.mcp.allowed_tools = Vec::new();
	}
	config
}

fn server_entry<'a>(data: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
	data["servers"]
		.as_array()
		.expect("servers array")
		.iter()
		.find(|s| s["name"] == name)
		.unwrap_or_else(|| panic!("server {name} missing: {data}"))
}

#[tokio::test]
#[serial_test::serial]
async fn test_info_marks_unstarted_stdio_server_dead() {
	let config = config_with_only_servers(vec![stdio_server("cov-stdio-dead")]);
	let (_, data) = run(&config, &["info"]).await;

	let entry = server_entry(&data, "cov-stdio-dead");
	assert_eq!(entry["health"], "dead", "unstarted stdio server: {entry}");
	assert_eq!(entry["connection_type"], "stdin");

	// The on-demand probe writes restart-info side effects; undo them.
	crate::mcp::process::SERVER_RESTART_INFO
		.write()
		.unwrap()
		.remove("cov-stdio-dead");
}

#[tokio::test]
#[serial_test::serial]
async fn test_health_reports_dead_and_failed_states() {
	use crate::mcp::process::{ServerHealth, ServerRestartInfo, SERVER_RESTART_INFO};

	let config = config_with_only_servers(vec![
		stdio_server("cov-health-dead"),
		stdio_server("cov-health-failed"),
	]);

	// Seed the global restart registry:
	// - "dead": a recent restart attempt puts it inside the 30s cooldown, so
	//   the health check records Dead without trying to spawn the binary.
	// - "failed": terminal state — the check must leave it untouched.
	{
		let mut guard = SERVER_RESTART_INFO.write().unwrap();
		guard.insert(
			"cov-health-dead".to_string(),
			ServerRestartInfo {
				last_restart_time: Some(std::time::SystemTime::now()),
				..Default::default()
			},
		);
		guard.insert(
			"cov-health-failed".to_string(),
			ServerRestartInfo {
				health_status: ServerHealth::Failed,
				restart_count: 5,
				consecutive_failures: 3,
				..Default::default()
			},
		);
	}

	let (_, data) = run(&config, &["health"]).await;
	assert!(data["monitor_running"].is_boolean(), "{data}");

	let dead = server_entry(&data, "cov-health-dead");
	assert_eq!(dead["health"], "dead");
	assert_eq!(dead["restart_count"], 0);
	assert!(
		dead["last_checked_secs_ago"].is_u64(),
		"probe must stamp last_checked: {dead}"
	);

	let failed = server_entry(&data, "cov-health-failed");
	assert_eq!(failed["health"], "failed");
	assert_eq!(failed["restart_count"], 5);
	assert_eq!(failed["consecutive_failures"], 3);

	let mut guard = SERVER_RESTART_INFO.write().unwrap();
	guard.remove("cov-health-dead");
	guard.remove("cov-health-failed");
}

#[tokio::test]
#[serial_test::serial]
async fn test_dynamic_servers_appear_in_info_and_full() {
	let session_id = format!("mcp-cmd-dyn-{}", std::process::id());
	crate::session::context::with_session_id(session_id.clone(), async {
		// /mcp info lists a dynamic server's CONFIG-declared tools (not the
		// enabled function list), so register the server carrying its tool.
		let mut dyn_server = stdio_server("cov-dyn-server");
		if let crate::config::McpServerConfig::Stdin { tools, .. } = &mut dyn_server {
			*tools = vec!["cov_dyn_tool".to_string()];
		}
		crate::session::context::register_dynamic_server_for_session(&session_id, dyn_server);
		crate::session::context::enable_dynamic_server_for_session(
			&session_id,
			"cov-dyn-server",
			vec![crate::mcp::McpFunction {
				name: "cov_dyn_tool".to_string(),
				description: "Coverage dynamic tool".to_string(),
				parameters: serde_json::json!({"type": "object"}),
			}],
		);

		let config = template_config();
		let (_, info) = run(&config, &["info"]).await;
		let entry = server_entry(&info, "cov-dyn-server");
		assert_eq!(entry["connection_type"], "dynamic");
		assert_eq!(entry["health"], "running");
		assert_eq!(entry["tools"], serde_json::json!(["cov_dyn_tool"]));
		assert!(
			info["total_tools"].as_u64().unwrap_or_default() >= 1,
			"dynamic tool not enumerated: {info}"
		);

		let (_, full) = run(&config, &["full"]).await;
		let entry = server_entry(&full, "cov-dyn-server");
		assert_eq!(entry["connection_type"], "dynamic");
		assert_eq!(entry["tools"], serde_json::json!(["cov_dyn_tool"]));

		crate::session::context::cleanup_session(&session_id);
	})
	.await;
}

#[cfg(unix)]
#[tokio::test]
#[serial_test::serial]
async fn test_local_tools_surface_as_local_server() {
	use std::os::unix::fs::PermissionsExt;

	let session_id = format!("mcp-cmd-local-{}", std::process::id());
	let workdir = std::env::temp_dir().join(format!("octomind-mcp-local-{}", std::process::id()));
	let _ = std::fs::remove_dir_all(&workdir);
	let tools_dir = workdir.join(".agents").join("tools");
	std::fs::create_dir_all(&tools_dir).expect("create tools dir");
	let script = tools_dir.join("cov-local-tool");
	std::fs::write(
		&script,
		"#!/bin/sh\n# @description Coverage local tool\n# @param input The input text\nexit 0\n",
	)
	.expect("write local tool");
	std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
		.expect("make tool executable");

	crate::session::context::with_session_id(session_id.clone(), async {
		crate::mcp::workdir::set_session_working_directory(workdir.clone());
		// The tool map picks up `.agents/tools/*` at init time from the
		// session workdir; /mcp groups tools by that map.
		// initialize_tool_map skips rebuilds when the config hash is
		// unchanged; a uniquely-named dummy server forces the rebuild so
		// local-tool discovery re-runs against THIS session's workdir.
		let dummy = format!("cov-local-force-{}", std::process::id());
		let mut config = template_config();
		config
			.mcp
			.servers
			.push(crate::config::McpServerConfig::builtin(&dummy, 30, vec![]));
		if let Some(role) = config.role_map.get_mut("assistant") {
			role.mcp.server_refs.push(dummy);
			role.mcp.allowed_tools = Vec::new();
		}
		init_tool_map(&config).await;

		let (_, data) = run(&config, &["info"]).await;
		let entry = server_entry(&data, "local");
		assert_eq!(entry["connection_type"], "builtin");
		assert_eq!(entry["health"], "running");
		assert_eq!(entry["tools"], serde_json::json!(["cov-local-tool"]));

		crate::session::context::cleanup_session(&session_id);
	})
	.await;
}
