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
