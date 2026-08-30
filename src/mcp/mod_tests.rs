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

//! External unit tests for the MCP dispatch surface in `src/mcp/mod.rs`:
//! result helpers, tool-pattern filtering, the internal function cache,
//! function gathering, tool-map dispatch (including session ownership),
//! builtin routing arms, and role initialization. The inline `mod tests`
//! in mod.rs is left untouched; this module only adds coverage.

use super::*;
use crate::config::{Config, McpServerConfig};
use serde_json::json;
use serial_test::serial;

// ------------------------------------------------------------------
// Helpers
// ------------------------------------------------------------------

/// Parse the shipped default template and replace the MCP server list.
/// Builtin-only servers keep every test offline: no stdio spawn, no HTTP.
fn config_with_servers(servers: Vec<McpServerConfig>) -> Config {
	let mut config: Config = toml::from_str(include_str!("../../config-templates/default.toml"))
		.expect("parse default config template");
	config.mcp.servers = servers;
	config
}

/// The `core` builtin server with attention enabled so `recall` is advertised.
fn core_server_config() -> Config {
	let mut config = config_with_servers(vec![McpServerConfig::builtin("core", 30, vec![])]);
	config.compression.attention.enabled = true;
	config
}

/// `role_map` is `#[serde(skip)]` — a bare template parse leaves it empty and
/// `get_merged_config_for_role` panics on the empty map. Populate it the way
/// `Config::load` does so role-merging code paths work in tests.
fn with_role_map(mut config: Config) -> Config {
	for role in config.roles.clone() {
		config.role_map.insert(role.name.clone(), role);
	}
	config
}

fn tool_call(name: &str, params: serde_json::Value) -> McpToolCall {
	McpToolCall {
		tool_name: name.to_string(),
		parameters: params,
		tool_id: format!("id-{name}"),
	}
}

fn fn_def(name: &str) -> McpFunction {
	McpFunction {
		name: name.to_string(),
		description: format!("description for {name}"),
		parameters: json!({"type": "object"}),
	}
}

fn names_of(fns: &[McpFunction]) -> Vec<String> {
	fns.iter().map(|f| f.name.clone()).collect()
}

// ------------------------------------------------------------------
// McpToolResult helpers
// ------------------------------------------------------------------

#[test]
fn success_with_metadata_exposes_structured_content() {
	let result = McpToolResult::success_with_metadata(
		"tool".to_string(),
		"id-1".to_string(),
		"body".to_string(),
		json!({"count": 2}),
	);
	assert!(!result.is_error());
	let content = result.extract_content();
	assert!(content.starts_with("body"), "{content}");
	assert!(content.contains("[Metadata:"), "{content}");
	assert!(content.contains("\"count\": 2"), "{content}");
}

#[test]
fn extract_content_joins_text_blocks_and_skips_null_metadata() {
	let mut call_result = rmcp::model::CallToolResult::success(vec![
		rmcp::model::ContentBlock::text("one"),
		rmcp::model::ContentBlock::text("two"),
	]);
	call_result.structured_content = Some(serde_json::Value::Null);
	let result = McpToolResult {
		tool_name: "tool".to_string(),
		tool_id: "id-1".to_string(),
		result: call_result,
	};
	assert_eq!(result.extract_content(), "one\ntwo");
}

// ------------------------------------------------------------------
// Tool pattern filtering
// ------------------------------------------------------------------

#[test]
fn is_tool_allowed_by_patterns_empty_list_allows_everything() {
	assert!(is_tool_allowed_by_patterns("anything", &[]));
}

#[test]
fn is_tool_allowed_by_patterns_exact_and_wildcard_semantics() {
	let patterns = vec!["recall".to_string(), "sch*".to_string()];
	assert!(is_tool_allowed_by_patterns("recall", &patterns));
	assert!(is_tool_allowed_by_patterns("schedule", &patterns));
	assert!(is_tool_allowed_by_patterns("school", &patterns));
	assert!(!is_tool_allowed_by_patterns("tap", &patterns));
	// A bare prefix of an exact-pattern name is not a match.
	assert!(!is_tool_allowed_by_patterns("reca", &patterns));
}

#[test]
fn filter_tools_by_patterns_passes_all_when_empty_and_filters_otherwise() {
	let tools = vec![fn_def("alpha"), fn_def("beta_x"), fn_def("gamma")];
	assert_eq!(super::filter_tools_by_patterns(tools.clone(), &[]).len(), 3);
	let filtered = super::filter_tools_by_patterns(tools, &["beta_*".to_string()]);
	assert_eq!(names_of(&filtered), vec!["beta_x".to_string()]);
}

// ------------------------------------------------------------------
// get_filtered_server_functions + INTERNAL_FUNCTION_CACHE
// ------------------------------------------------------------------

#[serial]
#[test]
fn filtered_server_functions_cache_hits_and_clear() {
	clear_function_cache();
	let first =
		get_filtered_server_functions("cache-probe", &[], || vec![fn_def("one"), fn_def("two")]);
	assert_eq!(names_of(&first), vec!["one".to_string(), "two".to_string()]);

	// Same cache key → the cached copy wins even when the producer changes.
	let cached = get_filtered_server_functions("cache-probe", &[], || vec![fn_def("three")]);
	assert_eq!(
		names_of(&cached),
		vec!["one".to_string(), "two".to_string()]
	);

	// A non-empty filter is a different cache key → producer runs again.
	let filtered = get_filtered_server_functions("cache-probe", &["tw*".to_string()], || {
		vec![fn_def("one"), fn_def("two")]
	});
	assert_eq!(names_of(&filtered), vec!["two".to_string()]);

	clear_function_cache();
	let after_clear = get_filtered_server_functions("cache-probe", &[], || vec![fn_def("three")]);
	assert_eq!(names_of(&after_clear), vec!["three".to_string()]);
	clear_function_cache();
}

// ------------------------------------------------------------------
// get_available_functions
// ------------------------------------------------------------------

#[tokio::test]
async fn available_functions_empty_when_no_servers_configured() {
	let config = config_with_servers(vec![]);
	assert!(get_available_functions(&config).await.is_empty());
}

#[serial]
#[tokio::test]
async fn available_functions_cover_each_builtin_server_arm() {
	clear_function_cache();

	let names = names_of(&get_available_functions(&core_server_config()).await);
	assert!(names.contains(&"recall".to_string()), "core: {names:?}");

	let runtime_cfg = config_with_servers(vec![McpServerConfig::builtin("runtime", 30, vec![])]);
	let names = names_of(&get_available_functions(&runtime_cfg).await);
	for expected in ["mcp", "agent", "skill", "capability"] {
		assert!(names.contains(&expected.to_string()), "runtime: {names:?}");
	}

	let orchestration_cfg =
		config_with_servers(vec![McpServerConfig::builtin("orchestration", 30, vec![])]);
	let names = names_of(&get_available_functions(&orchestration_cfg).await);
	for expected in ["tap", "schedule", "monitor"] {
		assert!(
			names.contains(&expected.to_string()),
			"orchestration: {names:?}"
		);
	}

	let mut agent_cfg = config_with_servers(vec![McpServerConfig::builtin("agent", 30, vec![])]);
	agent_cfg.agents.push(crate::config::agents::AgentConfig {
		name: "probe_agent".to_string(),
		description: "probe agent".to_string(),
		command: "echo".to_string(),
		workdir: ".".to_string(),
	});
	let names = names_of(&get_available_functions(&agent_cfg).await);
	assert!(
		names.contains(&"agent_probe_agent".to_string()),
		"agent: {names:?}"
	);

	clear_function_cache();
}

#[serial]
#[tokio::test]
async fn available_functions_honor_static_filters_and_unknown_builtins() {
	clear_function_cache();
	let mut filtered = core_server_config();
	filtered.mcp.servers[0] =
		McpServerConfig::builtin("core", 30, vec!["no_such_tool".to_string()]);
	let fns = get_available_functions(&filtered).await;
	assert!(
		!fns.iter().any(|f| f.name == "recall"),
		"filter must drop recall"
	);

	let unknown = config_with_servers(vec![McpServerConfig::builtin(
		"no-such-builtin",
		30,
		vec![],
	)]);
	let fns = get_available_functions(&unknown).await;
	assert!(!fns.iter().any(|f| f.name == "recall"));
	clear_function_cache();
}

#[serial]
#[tokio::test]
async fn available_functions_external_spawn_failure_yields_nothing() {
	// A stdio server whose command cannot exist: the cached-functions lookup
	// fails and the server contributes no functions (error is only logged).
	let config = config_with_servers(vec![McpServerConfig::stdin(
		"bogus-external",
		"definitely-not-a-real-command-xyz",
		vec![],
		1,
		vec![],
	)]);
	let fns = get_available_functions(&config).await;
	assert!(!fns.iter().any(|f| f.name.starts_with("bogus")));
}

// ------------------------------------------------------------------
// build_tool_server_map
// ------------------------------------------------------------------

#[serial]
#[tokio::test]
async fn tool_server_map_maps_core_tools_to_core_server() {
	clear_function_cache();
	let config = core_server_config();
	let map = build_tool_server_map(&config).await;
	assert_eq!(
		map.get("recall").map(|s| s.name().to_string()),
		Some("core".to_string())
	);
	clear_function_cache();
}

// ------------------------------------------------------------------
// execute_tool_call — guards and dispatch
// ------------------------------------------------------------------

#[tokio::test]
async fn execute_tool_call_requires_configured_servers() {
	let config = config_with_servers(vec![]);
	let err = execute_tool_call(&tool_call("recall", json!({})), &config, None)
		.await
		.unwrap_err();
	assert!(err.to_string().contains("no servers configured"), "{err}");
}

#[tokio::test]
async fn execute_tool_call_rejects_precancelled_token() {
	let config = config_with_servers(vec![McpServerConfig::builtin("runtime", 30, vec![])]);
	let (_tx, rx) = tokio::sync::watch::channel(true);
	let err = execute_tool_call(&tool_call("mcp", json!({})), &config, Some(rx))
		.await
		.unwrap_err();
	assert!(err.to_string().contains("cancelled"), "{err}");
}

#[serial]
#[tokio::test]
async fn execute_tool_call_routes_core_tool_through_tool_map() {
	clear_function_cache();
	let config = core_server_config();
	tool_map::initialize_tool_map(&config)
		.await
		.expect("init tool map");

	// `recall` without ids → soft error from the tool, but routing succeeded.
	let (result, _elapsed_ms) = execute_tool_call(&tool_call("recall", json!({})), &config, None)
		.await
		.expect("routing should succeed");
	assert_eq!(result.tool_name, "recall");
	assert_eq!(result.tool_id, "id-recall");
	assert!(result.is_error());
	assert!(
		result.extract_content().to_lowercase().contains("recall"),
		"{}",
		result.extract_content()
	);

	// Unknown tool → hard error listing the available tools.
	let err = execute_tool_call(&tool_call("no_such_tool", json!({})), &config, None)
		.await
		.unwrap_err();
	assert!(
		err.to_string()
			.contains("not found in any configured MCP server"),
		"{err}"
	);
	assert!(err.to_string().contains("recall"), "{err}");
	clear_function_cache();
}

#[serial]
#[tokio::test]
async fn session_ownership_rejects_tools_from_foreign_servers() {
	clear_function_cache();
	let config = core_server_config();
	tool_map::initialize_tool_map(&config)
		.await
		.expect("init tool map");

	// A config that does NOT include the `core` server the tool map routes to.
	let other = config_with_servers(vec![McpServerConfig::builtin("runtime", 30, vec![])]);
	crate::session::context::with_session_id(
		format!("ownership-{}", uuid::Uuid::new_v4()),
		async {
			let (result, _) = execute_tool_call(&tool_call("recall", json!({})), &other, None)
				.await
				.expect("ownership rejection is a soft error");
			assert!(result.is_error());
			assert!(result
				.extract_content()
				.contains("belongs to another session"));
		},
	)
	.await;

	// Same session context, but the config DOES define the server → allowed.
	crate::session::context::with_session_id(
		format!("ownership-ok-{}", uuid::Uuid::new_v4()),
		async {
			let (result, _) = execute_tool_call(&tool_call("recall", json!({})), &config, None)
				.await
				.expect("config-defined server must route");
			assert!(result.is_error()); // recall validation error, not ownership
			assert!(!result.extract_content().contains("another session"));
		},
	)
	.await;
	clear_function_cache();
}

#[serial]
#[tokio::test]
async fn session_ownership_rejects_foreign_dynamic_agent_tools() {
	clear_function_cache();
	let config = core_server_config();
	tool_map::initialize_tool_map(&config)
		.await
		.expect("init tool map");
	tool_map::register_dynamic_agent_tool("foreign_probe_agent");

	crate::session::context::with_session_id(
		format!("ownership-agent-{}", uuid::Uuid::new_v4()),
		async {
			let (result, _) = execute_tool_call(
				&tool_call("agent_foreign_probe_agent", json!({"task": "x"})),
				&config, // config defines no such agent
				None,
			)
			.await
			.expect("ownership rejection is a soft error");
			assert!(result.is_error());
			assert!(result
				.extract_content()
				.contains("belongs to another session"));
		},
	)
	.await;

	tool_map::unregister_dynamic_agent_tool("foreign_probe_agent");
	clear_function_cache();
}

#[serial]
#[tokio::test]
async fn execute_tool_calls_collects_results_in_order() {
	clear_function_cache();
	let config = core_server_config();
	tool_map::initialize_tool_map(&config)
		.await
		.expect("init tool map");
	let calls = vec![
		tool_call("recall", json!({})),
		tool_call("no_such_tool", json!({})),
	];
	let results = execute_tool_calls(&calls, &config).await;
	assert_eq!(results.len(), 2);
	assert!(results[0].is_ok());
	assert!(results[1].is_err());
	clear_function_cache();
}

#[tokio::test]
async fn layer_tool_call_delegates_to_execute_tool_call() {
	let config = config_with_servers(vec![]);
	assert!(
		execute_layer_tool_call(&tool_call("x", json!({})), &config, None)
			.await
			.is_err()
	);
}

// ------------------------------------------------------------------
// route_builtin_tool — direct arm coverage
// ------------------------------------------------------------------

fn route_config() -> Config {
	config_with_servers(vec![McpServerConfig::builtin("core", 30, vec![])])
}

#[tokio::test]
async fn route_core_unknown_tool_is_hard_error() {
	let err =
		super::route_builtin_tool(&tool_call("nope", json!({})), "core", &route_config(), None)
			.await
			.unwrap_err();
	assert!(
		err.to_string().contains("not implemented in core server"),
		"{err}"
	);
}

#[tokio::test]
async fn route_core_recall_maps_failure_to_soft_error() {
	let result = super::route_builtin_tool(
		&tool_call("recall", json!({})),
		"core",
		&route_config(),
		None,
	)
	.await
	.expect("recall failures are soft errors");
	assert_eq!(result.tool_id, "id-recall");
	assert!(result.is_error());
	assert!(
		result.extract_content().to_lowercase().contains("recall"),
		"{}",
		result.extract_content()
	);
}

#[tokio::test]
async fn route_orchestration_unknown_tool_is_hard_error() {
	let err = super::route_builtin_tool(
		&tool_call("nope", json!({})),
		"orchestration",
		&route_config(),
		None,
	)
	.await
	.unwrap_err();
	assert!(
		err.to_string()
			.contains("not implemented in orchestration server"),
		"{err}"
	);
}

#[tokio::test]
async fn route_orchestration_tools_map_failures_to_soft_errors() {
	let config = route_config();
	// Missing/unknown actions are soft errors from the tool implementations —
	// routing must return them as Ok(McpToolResult) with the tool id attached.
	for (tool, params) in [
		("tap", json!({})),
		("schedule", json!({})),
		("monitor", json!({"action": "bogus"})),
	] {
		let result =
			super::route_builtin_tool(&tool_call(tool, params), "orchestration", &config, None)
				.await
				.unwrap_or_else(|_| panic!("{tool} failures must be soft errors, not Err"));
		assert_eq!(result.tool_id, format!("id-{tool}"));
		assert!(result.is_error(), "{tool} should report a parameter error");
	}
}

#[tokio::test]
async fn route_runtime_unknown_tool_is_soft_error() {
	let result = super::route_builtin_tool(
		&tool_call("nope", json!({})),
		"runtime",
		&route_config(),
		None,
	)
	.await
	.expect("runtime maps unknown tools to soft errors");
	assert!(result.is_error());
	assert!(result
		.extract_content()
		.contains("not implemented in runtime server"));
	assert_eq!(result.tool_id, "id-nope");
}

#[tokio::test]
async fn route_agent_rejects_non_agent_prefixed_names() {
	let err =
		super::route_builtin_tool(&tool_call("tap", json!({})), "agent", &route_config(), None)
			.await
			.unwrap_err();
	assert!(
		err.to_string().contains("not implemented in agent server"),
		"{err}"
	);
}

#[tokio::test]
async fn route_agent_unconfigured_agent_is_soft_error() {
	let result = super::route_builtin_tool(
		&tool_call("agent_ghost", json!({"task": "do something"})),
		"agent",
		&route_config(),
		None,
	)
	.await
	.expect("unconfigured agent is a soft error");
	assert!(result.is_error());
	assert!(result
		.extract_content()
		.contains("not configured or not enabled"));
	assert_eq!(result.tool_id, "id-agent_ghost");
}

#[tokio::test]
async fn route_local_missing_tool_is_soft_error() {
	let result = super::route_builtin_tool(
		&tool_call("no_such_local_tool", json!({})),
		"local",
		&route_config(),
		None,
	)
	.await
	.expect("local arm always returns Ok");
	assert!(result.is_error());
	assert!(
		result
			.extract_content()
			.contains("local tool 'no_such_local_tool' failed"),
		"{}",
		result.extract_content()
	);
}

#[tokio::test]
async fn route_unknown_builtin_server_is_hard_error() {
	let err = super::route_builtin_tool(
		&tool_call("x", json!({})),
		"no-such-server",
		&route_config(),
		None,
	)
	.await
	.unwrap_err();
	assert!(err.to_string().contains("Unknown builtin server"), "{err}");
}

// ------------------------------------------------------------------
// Initialization
// ------------------------------------------------------------------

#[tokio::test]
async fn initialize_servers_with_no_servers_is_ok() {
	let config = config_with_servers(vec![]);
	initialize_servers_for_role(&config)
		.await
		.expect("no servers → Ok");
}

#[serial]
#[tokio::test]
async fn initialize_servers_builtin_only_starts_nothing_external() {
	let config = config_with_servers(vec![
		McpServerConfig::builtin("core", 30, vec![]),
		McpServerConfig::builtin("runtime", 30, vec![]),
	]);
	let started: std::sync::Mutex<Vec<Vec<String>>> = std::sync::Mutex::new(Vec::new());
	let completed: std::sync::Mutex<usize> = std::sync::Mutex::new(0);
	initialize_servers_for_role_with_callback(
		&config,
		Some(&|progress| match progress {
			McpInitProgress::Starting { servers } => started.lock().unwrap().push(servers),
			McpInitProgress::Completed { .. } => *completed.lock().unwrap() += 1,
		}),
	)
	.await
	.expect("builtin-only init must succeed");
	assert_eq!(*started.lock().unwrap(), vec![Vec::<String>::new()]);
	assert_eq!(*completed.lock().unwrap(), 0);
}

#[serial]
#[tokio::test]
async fn initialize_servers_skips_servers_with_unset_env_placeholders() {
	let mut server = McpServerConfig::stdin("env-gated", "some-command", vec![], 1, vec![]);
	if let McpServerConfig::Stdin { env, .. } = &mut server {
		env.insert(
			"TOKEN".to_string(),
			"{{ENV:OCTOMIND_TEST_UNSET_VAR_XYZ}}".to_string(),
		);
	}
	let config = config_with_servers(vec![server]);
	let started: std::sync::Mutex<Vec<Vec<String>>> = std::sync::Mutex::new(Vec::new());
	initialize_servers_for_role_with_callback(
		&config,
		Some(&|p| {
			if let McpInitProgress::Starting { servers } = p {
				started.lock().unwrap().push(servers);
			}
		}),
	)
	.await
	.expect("skipped server must not fail init");
	assert_eq!(
		*started.lock().unwrap(),
		vec![Vec::<String>::new()],
		"env-gated server must be filtered out before Starting"
	);
}

#[serial]
#[tokio::test]
async fn initialize_servers_reports_failed_external_server() {
	let config = config_with_servers(vec![McpServerConfig::stdin(
		"broken-external",
		"definitely-not-a-real-command-xyz",
		vec![],
		1,
		vec![],
	)]);
	let events: std::sync::Mutex<Vec<(String, bool, usize)>> = std::sync::Mutex::new(Vec::new());
	initialize_servers_for_role_with_callback(
		&config,
		Some(&|p| {
			if let McpInitProgress::Completed {
				server,
				success,
				function_count,
			} = p
			{
				events
					.lock()
					.unwrap()
					.push((server, success, function_count));
			}
		}),
	)
	.await
	.expect("failed external server is logged, not fatal");
	let events = events.lock().unwrap();
	assert_eq!(events.len(), 1);
	assert_eq!(events[0], ("broken-external".to_string(), false, 0));
	// The health monitor task spawned for the external server dies with this
	// test's runtime — reset its flag so later tests see a clean slate.
	health_monitor::stop_health_monitor();
}

#[serial]
#[tokio::test]
async fn initialize_mcp_for_role_initializes_servers_and_tool_map() {
	let config = with_role_map(config_with_servers(vec![]));
	initialize_mcp_for_role("assistant", &config)
		.await
		.expect("role init must succeed");
	assert!(tool_map::is_initialized());
}

#[serial]
#[tokio::test]
async fn initialize_mcp_for_role_with_callback_forwards_progress() {
	let config = with_role_map(config_with_servers(vec![McpServerConfig::builtin(
		"core",
		30,
		vec![],
	)]));
	let saw_start = std::sync::atomic::AtomicBool::new(false);
	initialize_mcp_for_role_with_callback(
		"assistant",
		&config,
		Some(&|p| {
			if matches!(p, McpInitProgress::Starting { .. }) {
				saw_start.store(true, std::sync::atomic::Ordering::Relaxed);
			}
		}),
	)
	.await
	.expect("role init with callback must succeed");
	assert!(
		saw_start.load(std::sync::atomic::Ordering::Relaxed),
		"callback should observe Starting"
	);
}

// ---------------------------------------------------------------------------
// Live external server (python fake over HTTP) + dispatch/log coverage.
// ---------------------------------------------------------------------------

/// Streamable-HTTP JSON-RPC MCP server: `server/discover`, `initialize`,
/// `tools/list` (echo + extra_tool), `tools/call` echo. See server_tests.rs
/// for the OAuth-capable variant; this copy keeps the module self-contained.
const FAKE_HTTP_SERVER: &str = r#"
import json, os
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

def rpc_result(rid, res):
    return {"jsonrpc": "2.0", "id": rid, "result": res}

def handle_rpc(req):
    method = req.get("method")
    rid = req.get("id")
    if method == "server/discover":
        return rpc_result(rid, {"resultType": "complete", "supportedVersions": ["2026-07-28"], "capabilities": {}, "ttlMs": 0, "cacheScope": "private"})
    if method == "initialize":
        return rpc_result(rid, {"protocolVersion": "2025-03-26", "capabilities": {}, "serverInfo": {"name": "octomind-fake-http", "version": "1.0"}, "instructions": "fake"})
    if method == "tools/list":
        return rpc_result(rid, {"tools": [
            {"name": "echo", "description": "echo over http", "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}}},
            {"name": "extra_tool", "description": "overlay probe", "inputSchema": {"type": "object"}},
        ]})
    if method == "tools/call":
        params = req.get("params") or {}
        return rpc_result(rid, {"content": [{"type": "text", "text": json.dumps({"echo": True, "arguments": params.get("arguments")})}], "isError": False})
    if method == "ping":
        return rpc_result(rid, {})
    if rid is not None:
        return {"jsonrpc": "2.0", "id": rid, "error": {"code": -32601, "message": "method not found"}}
    return None

class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *args):
        pass

    def _send_json(self, code, obj):
        body = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _send_empty(self, code):
        self.send_response(code)
        self.send_header("Content-Length", "0")
        self.end_headers()

    def do_POST(self):
        length = int(self.headers.get("Content-Length") or 0)
        req = json.loads(self.rfile.read(length) or b"{}")
        resp = handle_rpc(req)
        if resp is None:
            self._send_empty(202)
        else:
            self._send_json(200, resp)

    def do_GET(self):
        self._send_empty(405)

    def do_DELETE(self):
        self._send_empty(200)

server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
print("PORT={}".format(server.server_address[1]), flush=True)
server.serve_forever()
"#;

async fn spawn_fake_http_server(tag: &str) -> (String, tokio::process::Child) {
	let path =
		std::env::temp_dir().join(format!("octomind-test-mod-{tag}-{}.py", std::process::id()));
	std::fs::write(&path, FAKE_HTTP_SERVER).expect("write fake server script");
	let mut child = tokio::process::Command::new("python3")
		.arg(&path)
		.stdout(std::process::Stdio::piped())
		.stderr(std::process::Stdio::null())
		.spawn()
		.expect("spawn fake http server");
	let port = {
		let mut stdout = child.stdout.take().expect("piped stdout");
		let mut line = String::new();
		tokio::time::timeout(std::time::Duration::from_secs(10), async {
			use tokio::io::AsyncBufReadExt;
			let mut reader = tokio::io::BufReader::new(&mut stdout);
			reader
				.read_line(&mut line)
				.await
				.expect("fake server must print its port");
		})
		.await
		.expect("fake http server startup within 10s");
		line.trim()
			.strip_prefix("PORT=")
			.and_then(|p| p.parse::<u16>().ok())
			.expect("PORT=<n> line from fake server")
	};
	(format!("http://127.0.0.1:{port}/mcp"), child)
}

fn enable_debug_logging() {
	let mut config: Config = toml::from_str(include_str!("../../config-templates/default.toml"))
		.expect("parse default config template");
	config.log_level = crate::config::LogLevel::Debug;
	crate::config::set_thread_config(&config);
}

/// Non-text content blocks are skipped by extract_content — only text blocks
/// contribute, in order.
#[test]
fn extract_content_skips_non_text_blocks() {
	let call_result: rmcp::model::CallToolResult = serde_json::from_value(json!({
		"content": [
			{"type": "resource_link", "uri": "job://1", "name": "job"},
			{"type": "text", "text": "only-text"},
			{"type": "resource_link", "uri": "job://2", "name": "job2"}
		]
	}))
	.expect("deserialize mixed content");
	let result = McpToolResult {
		tool_name: "tool".to_string(),
		tool_id: "id-1".to_string(),
		result: call_result,
	};
	assert_eq!(result.extract_content(), "only-text");
}

/// External server initialization succeeds end-to-end against a live HTTP
/// server and reports the real function count via the progress callback.
#[serial]
#[tokio::test]
async fn initialize_servers_external_http_reports_success() {
	enable_debug_logging();
	clear_function_cache();
	const NAME: &str = "mod-live-http-init";
	let (url, mut child) = spawn_fake_http_server("init").await;
	let config = config_with_servers(vec![
		McpServerConfig::builtin("core", 30, vec![]),
		McpServerConfig::http(NAME, &url, 10, vec![]),
	]);

	let events: std::sync::Mutex<Vec<(String, bool, usize)>> = std::sync::Mutex::new(Vec::new());
	initialize_servers_for_role_with_callback(
		&config,
		Some(&|p| {
			if let McpInitProgress::Completed {
				server,
				success,
				function_count,
			} = p
			{
				events
					.lock()
					.unwrap()
					.push((server, success, function_count));
			}
		}),
	)
	.await
	.expect("live server init must succeed");

	let events = events.lock().unwrap();
	let external = events
		.iter()
		.find(|(server, _, _)| server == NAME)
		.expect("external server must report completion");
	assert_eq!(external.1, true, "live server must initialize successfully");
	assert_eq!(external.2, 2, "echo + extra_tool must be counted");

	let _ = child.kill().await;
	server::clear_function_cache_for_server(NAME);
	crate::mcp::client::disconnect(NAME);
	health_monitor::stop_health_monitor();
	clear_function_cache();
}

/// Capability overlay extras extend a restrictive static filter at function
/// gathering time.
#[serial]
#[tokio::test]
async fn available_functions_merge_capability_overlay_extras() {
	clear_function_cache();
	const NAME: &str = "mod-overlay-http";
	let (url, mut child) = spawn_fake_http_server("overlay").await;
	// Restrictive filter: only `echo` is statically allowed.
	let config = config_with_servers(vec![McpServerConfig::http(
		NAME,
		&url,
		10,
		vec!["echo".to_string()],
	)]);

	let mut extras = std::collections::HashMap::new();
	extras.insert(NAME.to_string(), vec!["extra_tool".to_string()]);
	crate::config::runtime_overlay::set_capability_extras("mod-overlay-cap", extras);

	let names = names_of(&get_available_functions(&config).await);
	assert!(
		names.contains(&"echo".to_string()),
		"static filter must keep echo: {names:?}"
	);
	assert!(
		names.contains(&"extra_tool".to_string()),
		"overlay extra must survive the filter: {names:?}"
	);

	crate::config::runtime_overlay::clear_capability_extras("mod-overlay-cap");
	let _ = child.kill().await;
	server::clear_function_cache_for_server(NAME);
	crate::mcp::client::disconnect(NAME);
	clear_function_cache();
}

/// A config server disabled in the session's dynamic registry contributes no
/// functions, even though the server is reachable.
#[serial]
#[tokio::test]
async fn available_functions_skip_disabled_dynamic_servers() {
	clear_function_cache();
	const NAME: &str = "mod-disabled-http";
	let (url, mut child) = spawn_fake_http_server("disabled").await;
	let server = McpServerConfig::http(NAME, &url, 10, vec![]);
	let config = config_with_servers(vec![server.clone()]);

	crate::session::context::with_session_id(
		format!("mod-disabled-{}", uuid::Uuid::new_v4()),
		async {
			crate::session::context::register_dynamic_server_for_session(
				&crate::session::context::current_session_id().expect("session id"),
				server,
			);
			let names = names_of(&get_available_functions(&config).await);
			assert!(
				names.is_empty(),
				"disabled dynamic server must contribute nothing: {names:?}"
			);
		},
	)
	.await;

	let _ = child.kill().await;
	server::clear_function_cache_for_server(NAME);
	crate::mcp::client::disconnect(NAME);
	clear_function_cache();
}

/// A tool-map-routed external tool executes over the real transport and
/// returns its result with the tool id attached.
#[serial]
#[tokio::test]
async fn external_tool_routes_through_tool_map_and_executes() {
	enable_debug_logging();
	clear_function_cache();
	const NAME: &str = "mod-route-http";
	let (url, mut child) = spawn_fake_http_server("route").await;
	let config = config_with_servers(vec![McpServerConfig::http(NAME, &url, 10, vec![])]);
	tool_map::initialize_tool_map(&config)
		.await
		.expect("init tool map");

	let (result, _elapsed) =
		execute_tool_call(&tool_call("echo", json!({"text": "routed"})), &config, None)
			.await
			.expect("external routing must succeed");
	assert!(!result.is_error(), "{}", result.extract_content());
	assert_eq!(result.tool_id, "id-echo");
	assert!(result.extract_content().contains("routed"));

	let _ = child.kill().await;
	crate::mcp::client::disconnect(NAME);
	server::clear_function_cache_for_server(NAME);
	clear_function_cache();
}

/// Dynamic-server tools execute inside the owning session and bump the
/// capability LRU (last_used) on success.
#[serial]
#[tokio::test]
async fn dynamic_server_tool_execution_succeeds_in_owning_session() {
	clear_function_cache();
	const NAME: &str = "mod-dynamic-http";
	let (url, mut child) = spawn_fake_http_server("dynamic").await;
	let server = McpServerConfig::http(NAME, &url, 10, vec![]);
	let config = config_with_servers(vec![server.clone()]);
	tool_map::initialize_tool_map(&config)
		.await
		.expect("init tool map");

	crate::session::context::with_session_id(
		format!("mod-dynamic-{}", uuid::Uuid::new_v4()),
		async {
			let sid = crate::session::context::current_session_id().expect("session id");
			crate::session::context::register_dynamic_server_for_session(&sid, server);
			let enabled = crate::session::context::enable_dynamic_server_for_session(
				&sid,
				NAME,
				vec![fn_def("echo")],
			);
			assert!(enabled, "enabling a registered dynamic server must succeed");

			let (result, _) = execute_tool_call(
				&tool_call("echo", json!({"text": "dynamic"})),
				&config,
				None,
			)
			.await
			.expect("dynamic tool must execute");
			assert!(!result.is_error(), "{}", result.extract_content());
			assert!(result.extract_content().contains("dynamic"));
		},
	)
	.await;

	let _ = child.kill().await;
	crate::mcp::client::disconnect(NAME);
	server::clear_function_cache_for_server(NAME);
	clear_function_cache();
}

/// The internal dispatcher rejects empty configs and pre-cancelled tokens
/// before touching any tool.
#[tokio::test]
async fn try_execute_tool_call_guards_empty_config_and_cancellation() {
	let err = try_execute_tool_call(
		&tool_call("x", json!({})),
		&config_with_servers(vec![]),
		None,
	)
	.await
	.expect_err("empty config must be rejected");
	assert!(err.to_string().contains("no servers configured"), "{err}");

	let config = config_with_servers(vec![McpServerConfig::builtin("core", 30, vec![])]);
	let (_tx, rx) = tokio::sync::watch::channel(true);
	let err = try_execute_tool_call(&tool_call("recall", json!({})), &config, Some(rx))
		.await
		.expect_err("pre-cancelled token must be rejected");
	assert!(err.to_string().contains("cancelled"), "{err}");
}

/// Dispatch logging evaluates its format args under debug logging, including
/// the >200-token truncation branch.
#[serial]
#[tokio::test]
async fn dispatch_log_covers_small_and_truncated_params() {
	enable_debug_logging();
	clear_function_cache();
	let config = core_server_config();
	tool_map::initialize_tool_map(&config)
		.await
		.expect("init tool map");

	// Small params → plain serialization branch.
	let (small, _) =
		execute_tool_call(&tool_call("recall", json!({"ids": ["b:1"]})), &config, None)
			.await
			.expect("small-params dispatch must route");
	assert!(small.is_error(), "recall without an archive errors softly");

	// >200 tokens of params → truncation branch.
	let big = "x".repeat(4000);
	let (large, _) = execute_tool_call(&tool_call("recall", json!({"blob": big})), &config, None)
		.await
		.expect("large-params dispatch must route");
	assert!(large.is_error());

	clear_function_cache();
}
