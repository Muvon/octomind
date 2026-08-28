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
