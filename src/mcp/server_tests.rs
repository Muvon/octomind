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

//! Tests for the external-server provider layer: rmcp tool mapping, the
//! function cache, health-gated tool execution, and the status wrappers.
//! Everything here is offline — no server is ever started or contacted. The
//! cache-hit path is exercised by seeding FUNCTION_CACHE directly, and the
//! health gates by seeding SERVER_RESTART_INFO with unique server names.

use super::*;

fn rmcp_tool(name: &str, description: Option<&str>, read_only: Option<bool>) -> rmcp::model::Tool {
	let mut value = serde_json::json!({
		"name": name,
		"inputSchema": {"type": "object", "properties": {}, "required": []}
	});
	if let Some(d) = description {
		value["description"] = serde_json::json!(d);
	}
	if let Some(r) = read_only {
		value["annotations"] = serde_json::json!({"readOnlyHint": r});
	}
	serde_json::from_value(value).expect("deserialize rmcp Tool")
}

fn tool_call(name: &str) -> McpToolCall {
	McpToolCall {
		tool_name: name.to_string(),
		parameters: serde_json::json!({}),
		tool_id: "t-server".to_string(),
	}
}

fn seed_health(name: &str, health: process::ServerHealth) {
	process::SERVER_RESTART_INFO
		.write()
		.unwrap()
		.entry(name.to_string())
		.or_default()
		.health_status = health;
}

fn clear_health(name: &str) {
	process::SERVER_RESTART_INFO.write().unwrap().remove(name);
}

#[test]
fn test_tools_to_functions_maps_fields() {
	let tools = vec![
		rmcp_tool("alpha", Some("Does alpha things"), Some(true)),
		rmcp_tool("beta", None, None),
	];
	let functions = tools_to_functions(&tools);
	assert_eq!(functions.len(), 2);
	assert_eq!(functions[0].name, "alpha");
	assert_eq!(functions[0].description, "Does alpha things");
	assert!(functions[0].parameters.get("type").is_some());
	// Missing description maps to empty string, not a panic.
	assert_eq!(functions[1].name, "beta");
	assert_eq!(functions[1].description, "");
}

#[test]
fn test_tools_to_functions_empty() {
	assert!(tools_to_functions(&[]).is_empty());
}

#[tokio::test]
async fn test_get_server_functions_rejects_builtin() {
	let server = McpServerConfig::builtin("srvtest-builtin", 30, vec![]);
	let err = get_server_functions(&server).await.unwrap_err();
	assert!(
		err.to_string()
			.contains("Built-in servers should not use get_server_functions"),
		"got: {err}"
	);
}

#[tokio::test]
async fn test_cached_functions_returned_without_connecting() {
	const NAME: &str = "srvtest-cache";
	FUNCTION_CACHE.write().unwrap().insert(
		NAME.to_string(),
		vec![McpFunction {
			name: "cached_tool".to_string(),
			description: "from cache".to_string(),
			parameters: serde_json::json!({"type": "object"}),
		}],
	);
	// HTTP config whose URL is never contacted: the cache is checked first.
	let server = McpServerConfig::http(NAME, "http://127.0.0.1:9", 5, vec![]);

	let functions = get_server_functions_cached(&server)
		.await
		.expect("cache hit");
	assert_eq!(functions.len(), 1);
	assert_eq!(functions[0].name, "cached_tool");

	FUNCTION_CACHE.write().unwrap().remove(NAME);
}

#[tokio::test]
async fn test_cached_functions_skip_unavailable_servers() {
	// Stdio server with no live connection → empty, and NOT cached (a cached
	// empty list would freeze the server at zero tools forever).
	let server = McpServerConfig::stdin("srvtest-skip-stdio", "echo", vec![], 5, vec![]);
	let functions = get_server_functions_cached(&server)
		.await
		.expect("dispatch");
	assert!(functions.is_empty());
	assert!(!FUNCTION_CACHE
		.read()
		.unwrap()
		.contains_key("srvtest-skip-stdio"));

	// Builtin servers never fetch through this path.
	let server = McpServerConfig::builtin("srvtest-skip-builtin", 30, vec![]);
	let functions = get_server_functions_cached(&server)
		.await
		.expect("dispatch");
	assert!(functions.is_empty());
}

#[test]
fn test_clear_function_cache_scopes() {
	FUNCTION_CACHE.write().unwrap().insert(
		"srvtest-clear-a".to_string(),
		vec![McpFunction {
			name: "t".to_string(),
			description: String::new(),
			parameters: serde_json::json!({}),
		}],
	);
	FUNCTION_CACHE.write().unwrap().insert(
		"srvtest-clear-b".to_string(),
		vec![McpFunction {
			name: "t".to_string(),
			description: String::new(),
			parameters: serde_json::json!({}),
		}],
	);

	clear_function_cache_for_server("srvtest-clear-a");
	let cache = FUNCTION_CACHE.read().unwrap();
	assert!(!cache.contains_key("srvtest-clear-a"));
	assert!(cache.contains_key("srvtest-clear-b"));
	drop(cache);

	clear_all_function_cache();
	assert!(FUNCTION_CACHE.read().unwrap().is_empty());
}

#[test]
fn test_is_server_already_running_with_config_builtin_tracks_health() {
	const NAME: &str = "srvtest-running-builtin";
	let server = McpServerConfig::builtin(NAME, 30, vec![]);
	assert!(is_server_already_running_with_config(&server));
	assert_eq!(
		process::SERVER_RESTART_INFO
			.read()
			.unwrap()
			.get(NAME)
			.map(|i| i.health_status),
		Some(process::ServerHealth::Running)
	);
	clear_health(NAME);
}

#[test]
fn test_is_server_already_running_with_config_http_without_process() {
	let server = McpServerConfig::http("srvtest-running-http", "http://127.0.0.1:9", 5, vec![]);
	assert!(!is_server_already_running_with_config(&server));
}

#[tokio::test]
async fn test_execute_tool_call_cancelled_before_start() {
	let (tx, rx) = tokio::sync::watch::channel(false);
	tx.send(true).expect("set cancel flag");
	let server = McpServerConfig::http("srvtest-cancel", "http://127.0.0.1:9", 5, vec![]);

	let err = execute_tool_call(&tool_call("cancelled_tool"), &server, Some(rx))
		.await
		.unwrap_err();
	assert!(err.to_string().contains("cancelled"), "got: {err}");
}

#[tokio::test]
async fn test_execute_tool_call_failed_health_gate() {
	const NAME: &str = "srvtest-failed";
	seed_health(NAME, process::ServerHealth::Failed);
	// HTTP config: skips the stdio liveness refresh that would reset health.
	let server = McpServerConfig::http(NAME, "http://127.0.0.1:9", 5, vec![]);

	let err = execute_tool_call(&tool_call("gated_tool"), &server, None)
		.await
		.unwrap_err();
	assert!(err.to_string().contains("in failed state"), "got: {err}");
	clear_health(NAME);
}

#[tokio::test]
async fn test_execute_tool_call_restarting_health_gate() {
	const NAME: &str = "srvtest-restarting";
	seed_health(NAME, process::ServerHealth::Restarting);
	let server = McpServerConfig::http(NAME, "http://127.0.0.1:9", 5, vec![]);

	let err = execute_tool_call(&tool_call("gated_tool"), &server, None)
		.await
		.unwrap_err();
	assert!(err.to_string().contains("currently starting"), "got: {err}");
	clear_health(NAME);
}

#[tokio::test]
async fn test_execute_tool_call_rejects_builtin() {
	const NAME: &str = "srvtest-exec-builtin";
	seed_health(NAME, process::ServerHealth::Running);
	let server = McpServerConfig::builtin(NAME, 30, vec![]);

	let err = execute_tool_call(&tool_call("builtin_tool"), &server, None)
		.await
		.unwrap_err();
	assert!(
		err.to_string()
			.contains("Built-in servers should not use execute_tool_call"),
		"got: {err}"
	);
	clear_health(NAME);
}

#[tokio::test]
async fn test_get_all_server_functions_empty_and_builtin_rejected() {
	let mut config: Config =
		toml::from_str(include_str!("../../config-templates/default.toml")).expect("parse config");
	config.build_role_map();

	// No servers → empty map, no error.
	config.mcp.servers.clear();
	let functions = get_all_server_functions(&config)
		.await
		.expect("empty config");
	assert!(functions.is_empty());

	// A builtin server propagates get_server_functions's rejection.
	config
		.mcp
		.servers
		.push(McpServerConfig::builtin("srvtest-all-builtin", 30, vec![]));
	let err = get_all_server_functions(&config).await.unwrap_err();
	assert!(err.to_string().contains("Built-in servers"), "got: {err}");
}

#[test]
fn test_cleanup_servers_succeeds() {
	cleanup_servers().expect("cleanup must be idempotent without running servers");
}

#[test]
fn test_status_report_wrappers_track_and_reset() {
	const NAME: &str = "srvtest-status";
	process::SERVER_RESTART_INFO
		.write()
		.unwrap()
		.entry(NAME.to_string())
		.or_default()
		.restart_count = 2;
	seed_health(NAME, process::ServerHealth::Restarting);

	assert_eq!(
		get_server_health_status(NAME),
		process::ServerHealth::Restarting
	);
	assert_eq!(get_server_restart_info(NAME).restart_count, 2);

	let report = get_server_status_report();
	let (health, info) = report.get(NAME).expect("server in report");
	assert_eq!(*health, process::ServerHealth::Restarting);
	assert_eq!(info.restart_count, 2);

	reset_server_failure_state(NAME).expect("reset tracked server");
	let info = get_server_restart_info(NAME);
	assert_eq!(info.health_status, process::ServerHealth::Dead);
	assert_eq!(info.restart_count, 0);
	assert_eq!(info.consecutive_failures, 0);

	clear_health(NAME);
}
