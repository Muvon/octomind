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
