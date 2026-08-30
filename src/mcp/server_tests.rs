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
use serial_test::serial;

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

// ---------------------------------------------------------------------------
// Live-server coverage: a python fake MCP server over real transports.
// ---------------------------------------------------------------------------

/// Streamable-HTTP JSON-RPC MCP server. Answers `server/discover` (modern),
/// `initialize` (legacy fallback when FAKE_MODE=legacy), `tools/list` with
/// `echo` + `extra_tool`, and `tools/call` echoing arguments plus the
/// Authorization header. Also serves the RFC 9728 / RFC 8414 OAuth discovery
/// documents and a DCR registration endpoint on the same port.
const FAKE_HTTP_SERVER: &str = r#"
import json, os
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

MODE = os.environ.get("FAKE_MODE", "modern")

def rpc_result(rid, res):
    return {"jsonrpc": "2.0", "id": rid, "result": res}

def rpc_error(rid, code, message):
    return {"jsonrpc": "2.0", "id": rid, "error": {"code": code, "message": message}}

def handle_rpc(req, auth):
    method = req.get("method")
    rid = req.get("id")
    if method == "server/discover":
        if MODE == "legacy":
            return rpc_error(rid, -32601, "discover not supported")
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
        payload = {"echo": True, "arguments": params.get("arguments"), "auth": auth}
        return rpc_result(rid, {"content": [{"type": "text", "text": json.dumps(payload)}], "isError": False})
    if method == "ping":
        return rpc_result(rid, {})
    if rid is not None:
        return rpc_error(rid, -32601, "method not found")
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
        if self.path == "/mcp":
            length = int(self.headers.get("Content-Length") or 0)
            req = json.loads(self.rfile.read(length) or b"{}")
            resp = handle_rpc(req, self.headers.get("Authorization"))
            if resp is None:
                self._send_empty(202)
            else:
                self._send_json(200, resp)
        elif self.path == "/register":
            self._send_json(201, {"client_id": "dcr-client-123", "client_secret": "s3cret", "client_name": "octomind", "redirect_uris": ["http://localhost:34567/oauth/callback"]})
        elif self.path == "/token":
            self._send_json(200, {"access_token": "test-token-xyz", "token_type": "Bearer", "expires_in": 3600, "refresh_token": "rt-1", "scope": ""})
        else:
            self._send_json(404, {"error": "not found"})

    def do_GET(self):
        base = "http://127.0.0.1:{}".format(self.server.server_address[1])
        if self.path == "/mcp":
            self._send_empty(405)
            return
        if ".well-known" in self.path:
            # RFC 9728 metadata exists only in "oauth" mode. Without it
            # discovery fails fast and the client connects unauthenticated;
            # with it, a tokenless connect would launch the interactive
            # browser flow (minutes-long callback wait) instead.
            if MODE != "oauth":
                self._send_json(404, {"error": "not found"})
                return
            # Discovery probes {mcp_url}/.well-known/... (pre-discovery
            # derives the path from the MCP URL), so serve the documents
            # under both the root and the /mcp prefix.
            if self.path.endswith("/.well-known/oauth-protected-resource"):
                self._send_json(200, {"resource": base, "authorization_servers": [base]})
            elif self.path.endswith("/.well-known/oauth-authorization-server"):
                self._send_json(200, {"issuer": base, "authorization_endpoint": base + "/authorize", "token_endpoint": base + "/token", "registration_endpoint": base + "/register", "response_types_supported": ["code"], "code_challenge_methods_supported": ["S256"], "grant_types_supported": ["authorization_code", "refresh_token"]})
            else:
                self._send_json(404, {"error": "not found"})
            return
        self._send_json(404, {"error": "not found"})

    def do_DELETE(self):
        self._send_empty(200)

server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
print("PORT={}".format(server.server_address[1]), flush=True)
server.serve_forever()
"#;

/// Newline-delimited JSON-RPC MCP server over stdio (discover + initialize +
/// tools/list + echo).
const FAKE_STDIO_SERVER: &str = r#"
import json, os, sys

def result(rid, res):
    return json.dumps({"jsonrpc": "2.0", "id": rid, "result": res})

def handle(req):
    method = req.get("method")
    rid = req.get("id")
    if method == "server/discover":
        return result(rid, {"resultType": "complete", "supportedVersions": ["2026-07-28"], "capabilities": {}, "ttlMs": 0, "cacheScope": "private"})
    if method == "initialize":
        return result(rid, {"protocolVersion": "2025-03-26", "capabilities": {}, "serverInfo": {"name": "octomind-fake-stdio", "version": "1.0"}, "instructions": "fake"})
    if method == "tools/list":
        return result(rid, {"tools": [{"name": "echo", "description": "echo tool", "inputSchema": {"type": "object"}}]})
    if method == "tools/call":
        params = req.get("params") or {}
        payload = {"echo": True, "arguments": params.get("arguments"), "cwd": os.getcwd(), "env": os.environ.get("OCTOMIND_TEST_ENV_MARKER", "unset")}
        return result(rid, {"content": [{"type": "text", "text": json.dumps(payload)}], "isError": False})
    if method == "ping":
        return result(rid, {})
    if rid is not None:
        return json.dumps({"jsonrpc": "2.0", "id": rid, "error": {"code": -32601, "message": "method not found"}})
    return None

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    try:
        req = json.loads(line)
    except ValueError:
        continue
    out = handle(req)
    if out is not None:
        sys.stdout.write(out + "\n")
        sys.stdout.flush()
"#;

fn write_script(tag: &str, body: &str) -> std::path::PathBuf {
	let path =
		std::env::temp_dir().join(format!("octomind-test-srv-{tag}-{}.py", std::process::id()));
	std::fs::write(&path, body).expect("write fake server script");
	path
}

/// Spawn the fake HTTP MCP server; returns its `/mcp` endpoint URL. The child
/// must be killed by the caller.
async fn spawn_fake_http_server(tag: &str, mode: &str) -> (String, tokio::process::Child) {
	let script = write_script(tag, FAKE_HTTP_SERVER);
	let mut child = tokio::process::Command::new("python3")
		.arg(&script)
		.env("FAKE_MODE", mode)
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

fn fake_stdio_config(name: &str, tag: &str) -> McpServerConfig {
	let script = write_script(tag, FAKE_STDIO_SERVER);
	McpServerConfig::Stdin {
		name: name.to_string(),
		command: "python3".to_string(),
		args: vec![script.to_string_lossy().into_owned()],
		timeout_seconds: 10,
		tools: vec![],
		env: std::collections::HashMap::new(),
		cwd: None,
		auto_bind: None,
	}
}

/// `get_server_functions` against a live HTTP server: real tools/list round
/// trip mapped to McpFunction definitions.
#[serial]
#[tokio::test]
async fn test_get_server_functions_lists_tools_from_live_http_server() {
	const NAME: &str = "srv-live-http-list";
	let (url, mut child) = spawn_fake_http_server("list", "modern").await;
	let server = McpServerConfig::http(NAME, &url, 10, vec![]);

	let functions = get_server_functions(&server)
		.await
		.expect("live tools/list must succeed");
	let names: Vec<&str> = functions.iter().map(|f| f.name.as_str()).collect();
	assert!(names.contains(&"echo"), "functions: {names:?}");
	assert!(names.contains(&"extra_tool"), "functions: {names:?}");
	let echo = functions
		.iter()
		.find(|f| f.name == "echo")
		.expect("echo fn");
	assert_eq!(echo.description, "echo over http");

	let _ = child.kill().await;
	crate::mcp::client::disconnect(NAME);
}

/// The cached variant fetches over HTTP on first call and serves from
/// FUNCTION_CACHE afterwards — proven by killing the server before the
/// second call.
#[serial]
#[tokio::test]
async fn test_cached_functions_fetch_once_then_serve_from_cache() {
	const NAME: &str = "srv-live-http-cache";
	let (url, mut child) = spawn_fake_http_server("cache", "modern").await;
	let server = McpServerConfig::http(NAME, &url, 10, vec![]);

	let first = get_server_functions_cached(&server)
		.await
		.expect("first fetch must succeed");
	assert!(first.iter().any(|f| f.name == "echo"));

	// Server gone → the second call must still succeed from the cache.
	let _ = child.kill().await;
	let second = get_server_functions_cached(&server)
		.await
		.expect("cached copy must survive server death");
	assert_eq!(first.len(), second.len());

	crate::mcp::client::disconnect(NAME);
	clear_function_cache_for_server(NAME);
}

/// A fetch failure returns an EMPTY list without caching it — the next call
/// must retry the fetch (cache stays unset).
#[tokio::test]
async fn test_cached_functions_http_failure_returns_empty_uncached() {
	const NAME: &str = "srv-http-fail-uncached";
	let server = McpServerConfig::http(NAME, "http://127.0.0.1:1/mcp", 2, vec![]);

	let functions = get_server_functions_cached(&server)
		.await
		.expect("fetch failure is mapped to an empty Ok list");
	assert!(functions.is_empty());
	assert!(
		FUNCTION_CACHE.read().unwrap().get(NAME).is_none(),
		"transient failure must not poison the cache"
	);
}

/// An OAuth-discovered server with no stored token yields no functions and
/// never contacts the MCP endpoint (no OAuth flow is triggered).
#[serial]
#[tokio::test]
async fn test_oauth_discovered_server_without_token_yields_no_functions() {
	const NAME: &str = "srv-oauth-no-token";
	let (url, mut child) = spawn_fake_http_server("oauth-none", "oauth").await;
	let server = McpServerConfig::http(NAME, &url, 10, vec![]);

	crate::mcp::oauth::discovery::discover_oauth_from_mcp_server(&url, NAME)
		.await
		.expect("fake discovery documents must satisfy the chain");

	let functions = get_server_functions_cached(&server)
		.await
		.expect("missing token is an Ok(empty), not an error");
	assert!(functions.is_empty(), "no token → no fetch → no functions");

	let _ = child.kill().await;
	crate::mcp::oauth::discovery::clear_discovered_oauth_cache(NAME);
}

/// An OAuth-discovered server WITH a valid stored token proceeds to the real
/// fetch and caches the functions.
#[serial]
#[tokio::test]
async fn test_oauth_discovered_server_with_token_fetches_functions() {
	const NAME: &str = "srv-oauth-with-token";
	let (url, mut child) = spawn_fake_http_server("oauth-token", "oauth").await;
	let server = McpServerConfig::http(NAME, &url, 10, vec![]);

	crate::mcp::oauth::discovery::discover_oauth_from_mcp_server(&url, NAME)
		.await
		.expect("fake discovery documents must satisfy the chain");
	let expires_at = std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.expect("clock after epoch")
		.as_secs()
		+ 3600;
	token_store::save_token(
		NAME,
		&token_store::TokenMetadata {
			server_name: NAME.to_string(),
			access_token: "test-token-xyz".to_string(),
			refresh_token: None,
			expires_at,
			scopes: vec![],
		},
	)
	.await
	.expect("seed token");

	let functions = get_server_functions_cached(&server)
		.await
		.expect("valid token must allow the fetch");
	assert!(
		functions.iter().any(|f| f.name == "echo"),
		"functions: {:?}",
		functions.iter().map(|f| f.name.clone()).collect::<Vec<_>>()
	);

	let _ = child.kill().await;
	crate::mcp::client::disconnect(NAME);
	clear_function_cache_for_server(NAME);
	crate::mcp::oauth::discovery::clear_discovered_oauth_cache(NAME);
	let _ = token_store::clear_token(NAME, false, None, None, None).await;
}

/// A Dead stdio server is restarted in place and the tool call then succeeds
/// end-to-end over the real transport.
#[serial]
#[tokio::test]
async fn test_execute_tool_call_restarts_dead_stdio_server_and_executes() {
	const NAME: &str = "srv-dead-stdio-restart";
	let server = fake_stdio_config(NAME, "exec-restart");
	seed_health(NAME, process::ServerHealth::Dead);

	let call = McpToolCall {
		tool_name: "echo".to_string(),
		parameters: serde_json::json!({"text": "round-trip"}),
		tool_id: "t-echo".to_string(),
	};
	let result = execute_tool_call(&call, &server, None)
		.await
		.expect("restart + execute must succeed");
	assert!(!result.is_error(), "{}", result.extract_content());
	assert!(result.extract_content().contains("round-trip"));

	assert_eq!(
		process::get_server_health(NAME),
		process::ServerHealth::Running
	);
	crate::mcp::client::disconnect(NAME);
	process::cleanup_server_process(NAME).ok();
	clear_health(NAME);
}

/// A Dead HTTP server is allowed to proceed (fresh connection on demand); when
/// the endpoint is unreachable the failure surfaces as a soft error result.
#[tokio::test]
async fn test_execute_tool_call_dead_http_proceeds_then_reports_soft_error() {
	const NAME: &str = "srv-dead-http";
	let server = McpServerConfig::http(NAME, "http://127.0.0.1:1/mcp", 2, vec![]);
	seed_health(NAME, process::ServerHealth::Dead);

	let result = execute_tool_call(&tool_call("echo"), &server, None)
		.await
		.expect("execution failures are soft errors, not Err");
	assert!(result.is_error());
	assert!(
		result.extract_content().contains("Error executing tool"),
		"{}",
		result.extract_content()
	);
	clear_health(NAME);
}

/// An Unreachable (auth-failed) HTTP server is also allowed to proceed.
#[tokio::test]
async fn test_execute_tool_call_unreachable_http_proceeds() {
	const NAME: &str = "srv-unreachable-http";
	let server = McpServerConfig::http(NAME, "http://127.0.0.1:1/mcp", 2, vec![]);
	seed_health(NAME, process::ServerHealth::Unreachable);

	let result = execute_tool_call(&tool_call("echo"), &server, None)
		.await
		.expect("unreachable gate must let execution proceed");
	assert!(result.is_error());
	clear_health(NAME);
}

/// A Dead stdio server whose binary cannot start fails the restart loudly.
#[serial]
#[tokio::test]
async fn test_execute_tool_call_dead_stdio_restart_failure_is_err() {
	const NAME: &str = "srv-dead-stdio-fail";
	let server = McpServerConfig::stdin(NAME, "definitely-not-a-real-binary", vec![], 2, vec![]);
	seed_health(NAME, process::ServerHealth::Dead);

	let err = execute_tool_call(&tool_call("echo"), &server, None)
		.await
		.expect_err("unstartable server must fail the call");
	assert!(err.to_string().contains("failed to restart"), "{err}");
	clear_health(NAME);
}

/// The inner cancellation entry rejects an already-cancelled token.
#[tokio::test]
async fn test_execute_tool_with_cancellation_rejects_precancelled_token() {
	let server = McpServerConfig::http("srv-precancel", "http://127.0.0.1:1/mcp", 2, vec![]);
	let (_tx, rx) = tokio::sync::watch::channel(true);
	let err = execute_tool_with_cancellation(&tool_call("echo"), &server, Some(rx))
		.await
		.expect_err("pre-cancelled token must abort before any I/O");
	assert!(err.to_string().contains("cancelled"), "{err}");
}

/// `get_all_server_functions` includes live external servers with their
/// resolved configs.
#[serial]
#[tokio::test]
async fn test_get_all_server_functions_includes_live_http_server() {
	const NAME: &str = "srv-all-functions";
	let (url, mut child) = spawn_fake_http_server("allfns", "modern").await;
	let mut config: Config = toml::from_str(include_str!("../../config-templates/default.toml"))
		.expect("parse default config template");
	config.mcp.servers = vec![McpServerConfig::http(NAME, &url, 10, vec![])];

	let functions = get_all_server_functions(&config)
		.await
		.expect("live server must contribute functions");
	let (echo_fn, owner) = functions
		.get("echo")
		.expect("live server's echo tool must be in the map");
	assert_eq!(owner.name(), NAME);
	assert_eq!(echo_fn.description, "echo over http");

	let _ = child.kill().await;
	crate::mcp::client::disconnect(NAME);
	clear_function_cache_for_server(NAME);
}

/// `perform_health_check_all_servers` reports a connected HTTP server as
/// Running.
#[serial]
#[tokio::test]
async fn test_perform_health_check_reports_connected_http_running() {
	const NAME: &str = "srv-health-check-live";
	let (url, mut child) = spawn_fake_http_server("health", "modern").await;
	let server = McpServerConfig::http(NAME, &url, 10, vec![]);
	crate::mcp::client::connect_http(&server)
		.await
		.expect("fake http server must accept the MCP handshake");

	let report = perform_health_check_all_servers().await;
	assert_eq!(
		report.get(NAME).copied(),
		Some(process::ServerHealth::Running),
		"a connected client must classify as Running"
	);

	let _ = child.kill().await;
	crate::mcp::client::disconnect(NAME);
	clear_health(NAME);
}
