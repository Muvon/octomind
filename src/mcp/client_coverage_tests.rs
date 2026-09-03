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

//! Coverage-focused client tests: real python MCP servers over stdio and
//! streamable HTTP (modern discover + legacy initialize fallback + OAuth
//! discovery/token), stale-process replacement, the absolute progress cap,
//! MRTR fulfillment rounds (elicitation / roots / sampling), task payloads,
//! the input-required round limit, and `subscriptions/listen` resource-link
//! watching. Complements the in-memory peer tests in `client_tests.rs` and
//! the transport-failure tests in `client_stdio_tests.rs`.

use super::*;
use futures::channel::mpsc;
use futures::StreamExt;
use rmcp::model::{
	CallToolResult, ClientRequest, ContentBlock, CreateTaskResult, DetailedTask,
	ElicitRequestParams, GetTaskResult, InputRequest, InputRequests, InputRequiredResult,
	JsonRpcMessage, Notification, ProgressNotificationParam, ReadResourceResult, Request,
	RequestId, Resource, ResourceContents, ResourceUpdatedNotificationParam, ServerNotification,
	ServerResult, SubscriptionFilter, SubscriptionsAcknowledgedNotificationParams, Task,
	TaskPayload, TaskStatus,
};
use rmcp::service::{RxJsonRpcMessage, TxJsonRpcMessage};
use serial_test::serial;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc as StdArc, Mutex as StdMutex};
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(5);

fn unique_server(tag: &str) -> String {
	format!("octomind-test-cov-{tag}")
}

fn stdin_config(
	name: &str,
	command: &str,
	args: Vec<String>,
	timeout_seconds: u64,
) -> McpServerConfig {
	McpServerConfig::Stdin {
		name: name.to_string(),
		command: command.to_string(),
		args,
		timeout_seconds,
		tools: vec![],
		env: HashMap::new(),
		cwd: None,
		auto_bind: None,
	}
}

fn tool_call(tool_id: &str) -> McpToolCall {
	McpToolCall {
		tool_name: "echo".to_string(),
		parameters: serde_json::json!({"text": "payload"}),
		tool_id: tool_id.to_string(),
	}
}

/// In-memory client service, same shape as the fake peers in the sibling
/// test modules (the helpers there are module-private).
struct InMemoryPeer {
	service: McpService,
	incoming: mpsc::UnboundedSender<RxJsonRpcMessage<RoleClient>>,
	outgoing: mpsc::UnboundedReceiver<TxJsonRpcMessage<RoleClient>>,
}

fn serve_in_memory(server_name: &str) -> InMemoryPeer {
	let (in_tx, in_rx) = mpsc::unbounded::<RxJsonRpcMessage<RoleClient>>();
	let (out_tx, out_rx) = mpsc::unbounded::<TxJsonRpcMessage<RoleClient>>();
	let service =
		rmcp::service::serve_directly(OctoClientHandler::new(server_name), (out_tx, in_rx), None);
	InMemoryPeer {
		service,
		incoming: in_tx,
		outgoing: out_rx,
	}
}

fn spawn_responder(
	mut outgoing: mpsc::UnboundedReceiver<TxJsonRpcMessage<RoleClient>>,
	incoming: mpsc::UnboundedSender<RxJsonRpcMessage<RoleClient>>,
	respond: impl Fn(ClientRequest) -> Option<ServerResult> + Send + 'static,
) -> tokio::task::JoinHandle<()> {
	tokio::spawn(async move {
		while let Some(message) = outgoing.next().await {
			if let JsonRpcMessage::Request(request) = message {
				if let Some(response) = respond(request.request) {
					incoming
						.unbounded_send(JsonRpcMessage::response(response, request.id))
						.expect("fake server channel must stay open");
				}
			}
		}
	})
}

// ---------------------------------------------------------------------------
// Fake MCP servers (python)
// ---------------------------------------------------------------------------

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
        return result(rid, {"tools": [{"name": "echo", "description": "cwd={} env={}".format(os.getcwd(), os.environ.get("OCTOMIND_TEST_ENV_MARKER", "unset")), "inputSchema": {"type": "object"}}]})
    if method == "tools/call":
        params = req.get("params") or {}
        payload = {"echo": True, "arguments": params.get("arguments"), "cwd": os.getcwd(), "env": os.environ.get("OCTOMIND_TEST_ENV_MARKER", "unset")}
        return result(rid, {"content": [{"type": "text", "text": json.dumps(payload)}], "isError": False})
    if method == "ping":
        return result(rid, {})
    if rid is not None:
        return json.dumps({"jsonrpc": "2.0", "id": rid, "error": {"code": -32601, "message": "method not found"}})
    return None

sys.stderr.write("fake-stdio-server-ready\n")
sys.stderr.flush()
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

const FAKE_HTTP_SERVER: &str = r#"
import json, os, socketserver
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
        return rpc_result(rid, {"tools": [{"name": "echo", "description": "echo over http", "inputSchema": {"type": "object"}}]})
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

class Server(ThreadingHTTPServer):
    def server_bind(self):
        # HTTPServer.server_bind calls getfqdn(), whose reverse-DNS lookup
        # stalls for >10s on macOS CI runners; bind without it.
        socketserver.TCPServer.server_bind(self)
        self.server_name, self.server_port = "127.0.0.1", self.server_address[1]

server = Server(("127.0.0.1", 0), Handler)
print("PORT={}".format(server.server_address[1]), flush=True)
server.serve_forever()
"#;

fn write_script(tag: &str, body: &str) -> std::path::PathBuf {
	let path =
		std::env::temp_dir().join(format!("octomind-test-cov-{tag}-{}.py", std::process::id()));
	std::fs::write(&path, body).expect("write fake server script");
	path
}

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
		tokio::time::timeout(Duration::from_secs(30), async {
			use tokio::io::AsyncBufReadExt;
			let mut reader = tokio::io::BufReader::new(&mut stdout);
			reader
				.read_line(&mut line)
				.await
				.expect("fake server must print its port");
		})
		.await
		.expect("fake http server startup within 30s");
		line.trim()
			.strip_prefix("PORT=")
			.and_then(|p| p.parse::<u16>().ok())
			.expect("PORT=<n> line from fake server")
	};
	(format!("http://127.0.0.1:{port}/mcp"), child)
}

// ---------------------------------------------------------------------------
// connect_stdio — real child process success paths
// ---------------------------------------------------------------------------

/// A real stdio server connects via the modern discover handshake, lists its
/// tools, and answers tool calls; the spawn honors the configured cwd and env.
#[serial]
#[tokio::test]
async fn connect_stdio_real_server_round_trip_with_cwd_and_env() {
	let name = unique_server("stdio-real");
	let script = write_script("stdio-real", FAKE_STDIO_SERVER);
	let workdir = std::env::temp_dir().join(format!("octomind-test-cwd-{}", std::process::id()));
	std::fs::create_dir_all(&workdir).expect("create cwd dir");
	let server = McpServerConfig::Stdin {
		name: name.clone(),
		command: "python3".to_string(),
		args: vec![script.to_string_lossy().into_owned()],
		timeout_seconds: 15,
		tools: vec![],
		env: HashMap::from([(
			"OCTOMIND_TEST_ENV_MARKER".to_string(),
			"marker-abc-123".to_string(),
		)]),
		cwd: Some(workdir.to_string_lossy().into_owned()),
		auto_bind: None,
	};

	let service = tokio::time::timeout(WAIT, connect_stdio(&server))
		.await
		.expect("connect must not hang")
		.expect("fake stdio server must connect");
	assert!(is_connected(&name));

	let tools = tokio::time::timeout(WAIT, list_tools(&server))
		.await
		.expect("tools/list must not hang")
		.expect("tools/list must succeed");
	assert_eq!(tools.len(), 1);
	assert_eq!(tools[0].name, "echo");
	// The tool description is produced by the child, proving cwd + env.
	let desc = tools[0].description.clone().unwrap_or_default();
	assert!(
		desc.contains(workdir.to_str().expect("utf8 tmp path")),
		"description must embed the configured cwd: {desc}"
	);
	assert!(
		desc.contains("marker-abc-123"),
		"description must embed the configured env marker: {desc}"
	);

	let result = tokio::time::timeout(WAIT, call_tool(&server, &tool_call("stdio-real"), None))
		.await
		.expect("tools/call must not hang")
		.expect("echo must succeed");
	let text = result
		.content
		.iter()
		.filter_map(|b| match b {
			ContentBlock::Text(t) => Some(t.text.clone()),
			_ => None,
		})
		.collect::<Vec<_>>()
		.join("\n");
	assert!(text.contains("marker-abc-123"), "echo payload: {text}");
	assert!(text.contains("payload"), "echo payload: {text}");

	// The stderr drain task captures the child's diagnostics: the ready line
	// the fake server writes on startup must land in the per-server buffer.
	let stderr_buffer = super::super::process::stderr_buffer_for(&name);
	let mut saw_ready_line = false;
	for _ in 0..100 {
		if stderr_buffer
			.lock()
			.expect("stderr buffer lock")
			.iter()
			.any(|line| line.contains("fake-stdio-server-ready"))
		{
			saw_ready_line = true;
			break;
		}
		tokio::time::sleep(Duration::from_millis(20)).await;
	}
	assert!(
		saw_ready_line,
		"stderr drain must capture the server ready line"
	);

	drop(service);
	disconnect(&name);
	super::super::process::cleanup_server_process(&name).ok();
}

// ---------------------------------------------------------------------------
// connect_http — real HTTP transport
// ---------------------------------------------------------------------------

#[serial]
#[tokio::test]
async fn connect_http_real_server_round_trip() {
	let name = unique_server("http-real");
	let (url, mut child) = spawn_fake_http_server("http-real", "modern").await;
	let server = McpServerConfig::http(&name, &url, 15, vec![]);

	let _service = tokio::time::timeout(WAIT, connect_http(&server))
		.await
		.expect("connect must not hang")
		.expect("fake http server must accept the modern handshake");
	assert!(is_connected(&name));

	let result = tokio::time::timeout(WAIT, call_tool(&server, &tool_call("http-real"), None))
		.await
		.expect("tools/call must not hang")
		.expect("echo over http must succeed");
	assert_eq!(result.is_error, Some(false));

	let _ = child.kill().await;
	disconnect(&name);
}

/// A server that rejects `server/discover` falls back to the legacy
/// `initialize` handshake and still becomes usable.
#[serial]
#[tokio::test]
async fn connect_http_legacy_initialize_fallback() {
	let name = unique_server("http-legacy");
	let (url, mut child) = spawn_fake_http_server("http-legacy", "legacy").await;
	let server = McpServerConfig::http(&name, &url, 15, vec![]);

	let _service = tokio::time::timeout(WAIT, connect_http(&server))
		.await
		.expect("connect must not hang")
		.expect("legacy fallback must succeed against an initialize-only server");
	assert!(is_connected(&name));

	let result = tokio::time::timeout(WAIT, call_tool(&server, &tool_call("http-legacy"), None))
		.await
		.expect("tools/call must not hang")
		.expect("echo must succeed after legacy handshake");
	assert_eq!(result.is_error, Some(false));

	let _ = child.kill().await;
	disconnect(&name);
}

/// A TCP endpoint that accepts but never answers hits the connect timeout.
#[serial]
#[tokio::test]
async fn connect_http_times_out_on_hanging_endpoint() {
	let name = unique_server("http-hang");
	// Accept connections and hold them open without writing anything.
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
		.await
		.expect("bind hanging listener");
	let addr = listener.local_addr().expect("listener addr");
	tokio::spawn(async move {
		let mut held = Vec::new();
		while let Ok((sock, _)) = listener.accept().await {
			held.push(sock);
		}
	});

	// A static Authorization header pins the auth source to StaticHeader, so
	// connect_http skips OAuth discovery (whose 10s reqwest timeout against a
	// holding listener would outlast the connect timeout under test).
	let server = McpServerConfig::Http {
		name: name.clone(),
		url: format!("http://{addr}/mcp"),
		timeout_seconds: 1,
		tools: vec![],
		headers: HashMap::from([(
			"Authorization".to_string(),
			"Bearer static-test".to_string(),
		)]),
		auto_bind: None,
	};
	let error =
		match tokio::time::timeout(WAIT + Duration::from_secs(5), connect_http(&server)).await {
			Ok(Ok(_service)) => panic!("hanging endpoint must fail"),
			Ok(Err(e)) => e,
			Err(_) => panic!("connect must hit its timeout, not hang forever"),
		};
	assert!(
		error.to_string().contains("Timed out connecting"),
		"unexpected error: {}",
		error
	);
	assert!(!is_connected(&name));
}

/// OAuth discovery succeeds against the fake authorization server, the stored
/// token is attached as a Bearer header, and the server echoes it back.
#[serial]
#[tokio::test]
async fn connect_http_uses_stored_oauth_token_as_bearer() {
	let name = unique_server("http-oauth");
	let (url, mut child) = spawn_fake_http_server("http-oauth", "oauth").await;
	let server = McpServerConfig::http(&name, &url, 15, vec![]);

	crate::mcp::oauth::discovery::discover_oauth_from_mcp_server(&url, &name)
		.await
		.expect("fake discovery chain must succeed");
	let expires_at = std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.expect("clock after epoch")
		.as_secs()
		+ 3600;
	crate::mcp::oauth::token_store::save_token(
		&name,
		&crate::mcp::oauth::token_store::TokenMetadata {
			server_name: name.clone(),
			access_token: "test-token-xyz".to_string(),
			refresh_token: None,
			expires_at,
			scopes: vec![],
		},
	)
	.await
	.expect("seed token");

	let _service = tokio::time::timeout(WAIT, connect_http(&server))
		.await
		.expect("connect must not hang")
		.expect("oauth-configured server must connect");
	assert!(is_connected(&name));

	let result = tokio::time::timeout(WAIT, call_tool(&server, &tool_call("http-oauth"), None))
		.await
		.expect("tools/call must not hang")
		.expect("echo must succeed");
	let text = result
		.content
		.iter()
		.filter_map(|b| match b {
			ContentBlock::Text(t) => Some(t.text.clone()),
			_ => None,
		})
		.collect::<Vec<_>>()
		.join("\n");
	assert!(
		text.contains("Bearer test-token-xyz"),
		"stored token must ride the Authorization header: {text}"
	);

	// The stored token is still current → the connection is reusable.
	assert!(http_auth_token_still_current(&server).await);

	let _ = child.kill().await;
	disconnect(&name);
	crate::mcp::oauth::discovery::clear_discovered_oauth_cache(&name);
	let _ = crate::mcp::oauth::token_store::clear_token(&name, false, None, None, None).await;
}

/// with no stored token is also true (None == None); a static Authorization
/// header short-circuits to current without any discovery.
#[serial]
#[tokio::test]
async fn http_auth_token_currency_for_builtin_and_tokenless_http() {
	let builtin = McpServerConfig::builtin("cov-builtin", 30, vec![]);
	assert!(http_auth_token_still_current(&builtin).await);

	let static_auth = McpServerConfig::Http {
		name: "cov-static-auth".to_string(),
		url: "http://127.0.0.1:9/mcp".to_string(),
		timeout_seconds: 2,
		tools: vec![],
		headers: HashMap::from([("Authorization".to_string(), "Bearer static".to_string())]),
		auto_bind: None,
	};
	assert!(
		http_auth_token_still_current(&static_auth).await,
		"static header auth is always current — no discovery may run"
	);

	let name = unique_server("http-tokenless");
	let server = McpServerConfig::http(&name, "http://127.0.0.1:1/mcp", 2, vec![]);
	assert!(
		http_auth_token_still_current(&server).await,
		"no stored token and no discovery → None == None → current"
	);
}

// ---------------------------------------------------------------------------
// get_or_connect — stale stdio process replacement
// ---------------------------------------------------------------------------

/// A registered service whose OS process is gone is disconnected and respawned
/// from the config's real command.
#[serial]
#[cfg(unix)]
#[tokio::test]
async fn get_or_connect_replaces_stale_stdio_process() {
	let name = unique_server("stale-stdio");
	let script = write_script("stale-stdio", FAKE_STDIO_SERVER);
	let server = McpServerConfig::Stdin {
		name: name.clone(),
		command: "python3".to_string(),
		args: vec![script.to_string_lossy().into_owned()],
		timeout_seconds: 15,
		tools: vec![],
		env: HashMap::new(),
		cwd: None,
		auto_bind: None,
	};

	// Register a live in-memory service first (so `get` returns Some).
	let peer = serve_in_memory(&name);
	register(&name, peer.service);

	// Point the pgid registry at a process that has already exited.
	let short_lived = std::process::Command::new("true")
		.spawn()
		.expect("spawn true");
	let dead_pid = short_lived.id();
	let _ = short_lived.wait_with_output();
	// Give the OS a moment to reap the child so kill(pid, 0) fails.
	let mut reaped = false;
	for _ in 0..50 {
		if unsafe { libc::kill(dead_pid as i32, 0) } != 0 {
			reaped = true;
			break;
		}
		std::thread::sleep(std::time::Duration::from_millis(20));
	}
	assert!(
		reaped,
		"short-lived child must exit before the test continues"
	);
	super::super::process::register_pgid(&name, dead_pid);

	let service = tokio::time::timeout(WAIT, get_or_connect(&server))
		.await
		.expect("get_or_connect must not hang")
		.expect("stale process must be replaced by a real spawn");
	assert!(is_connected(&name));

	// The replacement is a real stdio server: echo works end-to-end.
	let result = tokio::time::timeout(WAIT, call_tool(&server, &tool_call("stale"), None))
		.await
		.expect("tools/call must not hang")
		.expect("respawned server must answer");
	assert_eq!(result.is_error, Some(false));

	drop(service);
	disconnect(&name);
	super::super::process::cleanup_server_process(&name).ok();
}

// ---------------------------------------------------------------------------
// call_tool — absolute progress cap, MRTR fulfillment, round limit
// ---------------------------------------------------------------------------

/// Endless progress notifications keep resetting the idle timeout, so the
/// absolute cap (20 × idle timeout) must fire and cancel the call.
#[serial]
#[tokio::test]
async fn call_tool_hits_absolute_progress_cap() {
	let name = unique_server("progress-cap");
	let peer = serve_in_memory(&name);
	register(&name, peer.service);
	// idle timeout 1s → absolute cap 20s; the test runs in real time.
	let server = stdin_config(&name, "unused", Vec::new(), 1);

	let mut outgoing = peer.outgoing;
	let incoming = peer.incoming;
	let responder = tokio::spawn(async move {
		use rmcp::model::GetMeta;
		while let Some(message) = outgoing.next().await {
			if let JsonRpcMessage::Request(request) = message {
				// rmcp 3.1.4 carries the progress token in the request
				// wrapper's extensions-backed meta (serialized to `_meta` on
				// the wire), not in `params.meta` — read it from the enum.
				let token = request.request.get_meta().get_progress_token();
				if matches!(&request.request, ClientRequest::CallToolRequest(_)) {
					if let Some(token) = token {
						loop {
							incoming
								.unbounded_send(JsonRpcMessage::notification(
									ServerNotification::ProgressNotification(Notification::new(
										ProgressNotificationParam::new(token.clone(), 1.0),
									)),
								))
								.expect("fake server channel must stay open");
							// 200ms: well under the 1s idle timeout, but only
							// ~100 notifications across the 20s cap (each one
							// is buffered process-globally while no CLI
							// notification sender is installed).
							tokio::time::sleep(Duration::from_millis(200)).await;
						}
					}
				}
			}
		}
	});

	let error = match tokio::time::timeout(
		Duration::from_secs(30),
		call_tool(&server, &tool_call("cap"), None),
	)
	.await
	{
		Ok(Ok(result)) => panic!("runaway progress must be capped, got {result:?}"),
		Ok(Err(e)) => e,
		Err(_) => panic!("absolute cap must fire within 30s"),
	};
	assert!(
		error.to_string().contains("exceeded PT20S total"),
		"unexpected error: {error}"
	);

	responder.abort();
	disconnect(&name);
}

fn elicitation_request() -> InputRequest {
	InputRequest::Elicitation(Request::new(ElicitRequestParams::UrlElicitationParams {
		meta: None,
		message: "Confirm to continue".to_string(),
		url: "https://example.com/confirm".to_string(),
		elicitation_id: "elicit-round-1".to_string(),
	}))
}

/// An input_required round with an elicitation request is fulfilled (declined
/// headlessly), echoed back as inputResponses, and the retried call completes.
#[serial]
#[tokio::test]
async fn call_tool_fulfills_elicitation_round() {
	let name = unique_server("elicit-round");
	let peer = serve_in_memory(&name);
	register(&name, peer.service);
	let server = stdin_config(&name, "unused", Vec::new(), 5);

	let requests: StdArc<StdMutex<Vec<serde_json::Value>>> = StdArc::new(StdMutex::new(Vec::new()));
	let seen = StdArc::clone(&requests);
	let mut outgoing = peer.outgoing;
	let incoming = peer.incoming;
	let responder = tokio::spawn(async move {
		while let Some(message) = outgoing.next().await {
			if let JsonRpcMessage::Request(request) = message {
				if let ClientRequest::CallToolRequest(call) = &request.request {
					seen.lock()
						.expect("requests lock")
						.push(serde_json::to_value(&call.params).expect("serialize params"));
				}
				let response = match &request.request {
					ClientRequest::CallToolRequest(call) => {
						if call.params.input_responses.is_some() {
							ServerResult::CallToolResult(CallToolResult::success(vec![
								ContentBlock::text("after-input"),
							]))
						} else {
							let mut rounds = InputRequests::new();
							rounds.insert("k1".to_string(), elicitation_request());
							ServerResult::InputRequiredResult(
								InputRequiredResult::from_input_requests(rounds),
							)
						}
					}
					_ => continue,
				};
				incoming
					.unbounded_send(JsonRpcMessage::response(response, request.id))
					.expect("fake server channel must stay open");
			}
		}
	});

	let result = tokio::time::timeout(WAIT, call_tool(&server, &tool_call("elicit"), None))
		.await
		.expect("elicitation round must not hang")
		.expect("fulfilled round must complete");
	let text = result
		.content
		.iter()
		.filter_map(|b| match b {
			ContentBlock::Text(t) => Some(t.text.clone()),
			_ => None,
		})
		.collect::<Vec<_>>()
		.join("\n");
	assert_eq!(text, "after-input");

	let requests = requests.lock().expect("requests lock");
	assert_eq!(requests.len(), 2, "exactly one retry after fulfillment");
	let retry = &requests[1];
	let responses = retry
		.get("inputResponses")
		.and_then(|v| v.as_object())
		.expect("retry must carry inputResponses");
	let elicitation_response = responses
		.get("k1")
		.expect("response keyed by the request id");
	assert_eq!(
		elicitation_response.get("action").and_then(|a| a.as_str()),
		Some("decline"),
		"headless client declines elicitation: {elicitation_response}"
	);

	responder.abort();
	disconnect(&name);
}

/// roots/list input requests are rejected: Octomind does not advertise roots.
#[serial]
#[tokio::test]
async fn call_tool_rejects_roots_list_input_request() {
	let name = unique_server("roots-round");
	let peer = serve_in_memory(&name);
	register(&name, peer.service);
	let server = stdin_config(&name, "unused", Vec::new(), 5);

	let mut outgoing = peer.outgoing;
	let incoming = peer.incoming;
	let responder = tokio::spawn(async move {
		while let Some(message) = outgoing.next().await {
			if let JsonRpcMessage::Request(request) = message {
				let response = match request.request {
					ClientRequest::CallToolRequest(_) => {
						let roots: InputRequest = serde_json::from_value(serde_json::json!({
							"method": "roots/list",
							"params": {}
						}))
						.expect("deserialize roots/list request");
						let mut rounds = InputRequests::new();
						rounds.insert("r1".to_string(), roots);
						ServerResult::InputRequiredResult(InputRequiredResult::from_input_requests(
							rounds,
						))
					}
					_ => continue,
				};
				incoming
					.unbounded_send(JsonRpcMessage::response(response, request.id))
					.expect("fake server channel must stay open");
			}
		}
	});

	let error = tokio::time::timeout(WAIT, call_tool(&server, &tool_call("roots"), None))
		.await
		.expect("rejection must not hang")
		.expect_err("roots/list must be rejected");
	assert!(
		error.to_string().contains("deprecated roots/list"),
		"unexpected error: {error}"
	);

	responder.abort();
	disconnect(&name);
}

/// sampling/createMessage input requests are rejected: no sampling advertised.
#[serial]
#[tokio::test]
async fn call_tool_rejects_sampling_input_request() {
	let name = unique_server("sampling-round");
	let peer = serve_in_memory(&name);
	register(&name, peer.service);
	let server = stdin_config(&name, "unused", Vec::new(), 5);

	let mut outgoing = peer.outgoing;
	let incoming = peer.incoming;
	let responder = tokio::spawn(async move {
		while let Some(message) = outgoing.next().await {
			if let JsonRpcMessage::Request(request) = message {
				let response = match request.request {
					ClientRequest::CallToolRequest(_) => {
						let sampling: InputRequest = serde_json::from_value(serde_json::json!({
							"method": "sampling/createMessage",
							"params": {
								"messages": [{"role": "user", "content": {"type": "text", "text": "hi"}}],
								"maxTokens": 16
							}
						}))
						.expect("deserialize sampling request");
						let mut rounds = InputRequests::new();
						rounds.insert("s1".to_string(), sampling);
						ServerResult::InputRequiredResult(InputRequiredResult::from_input_requests(
							rounds,
						))
					}
					_ => continue,
				};
				incoming
					.unbounded_send(JsonRpcMessage::response(response, request.id))
					.expect("fake server channel must stay open");
			}
		}
	});

	let error = tokio::time::timeout(WAIT, call_tool(&server, &tool_call("sampling"), None))
		.await
		.expect("rejection must not hang")
		.expect_err("sampling must be rejected");
	assert!(
		error.to_string().contains("sampling/createMessage"),
		"unexpected error: {error}"
	);

	responder.abort();
	disconnect(&name);
}

/// A server that never completes (state-only input_required every round)
/// exhausts the MRTR round limit.
#[serial]
#[tokio::test]
async fn call_tool_exhausts_input_required_round_limit() {
	let name = unique_server("round-limit");
	let peer = serve_in_memory(&name);
	register(&name, peer.service);
	let server = stdin_config(&name, "unused", Vec::new(), 5);

	let responder = spawn_responder(
		peer.outgoing,
		peer.incoming.clone(),
		|request| match request {
			ClientRequest::CallToolRequest(_) => Some(ServerResult::InputRequiredResult(
				InputRequiredResult::from_request_state("perpetually-busy"),
			)),
			_ => None,
		},
	);

	// Ten state-only rounds with 50–250ms backoff ≈ 2.1s in real time.
	let error = match tokio::time::timeout(
		Duration::from_secs(15),
		call_tool(&server, &tool_call("limit"), None),
	)
	.await
	{
		Ok(Ok(result)) => {
			panic!("perpetual input_required must hit the round limit, got {result:?}")
		}
		Ok(Err(e)) => e,
		Err(_) => panic!("round limit must be reached within 15s"),
	};
	assert!(
		error
			.to_string()
			.contains("exceeded the MCP input-required round limit"),
		"unexpected error: {error}"
	);

	responder.abort();
	disconnect(&name);
}

// ---------------------------------------------------------------------------
// drive_task — input_required mid-task, failed and cancelled payloads
// ---------------------------------------------------------------------------

/// A task that pauses with an elicitation mid-flight gets it fulfilled, the
/// update is sent, and the task then completes.
#[serial]
#[tokio::test]
async fn task_input_required_round_updates_and_completes() {
	let name = unique_server("task-input");
	let peer = serve_in_memory(&name);
	register(&name, peer.service);
	let server = stdin_config(&name, "unused", Vec::new(), 5);

	let updated = StdArc::new(AtomicBool::new(false));
	let updated_flag = StdArc::clone(&updated);
	let mut outgoing = peer.outgoing;
	let incoming = peer.incoming;
	let responder = tokio::spawn(async move {
		let mut get_count = 0usize;
		while let Some(message) = outgoing.next().await {
			if let JsonRpcMessage::Request(request) = message {
				let response = match request.request {
					ClientRequest::CallToolRequest(_) => {
						ServerResult::CreateTaskResult(CreateTaskResult::new(Task::new(
							"task-in",
							TaskStatus::Working,
							"2026-01-01T00:00:00Z",
							"2026-01-01T00:00:00Z",
						)))
					}
					ClientRequest::GetTaskRequest(_) => {
						get_count += 1;
						let task = if get_count == 1 {
							DetailedTask::new(
								Task::new(
									"task-in",
									TaskStatus::InputRequired,
									"2026-01-01T00:00:00Z",
									"2026-01-01T00:00:00Z",
								),
								TaskPayload::InputRequired {
									input_requests: {
										let mut rounds = InputRequests::new();
										rounds.insert("t1".to_string(), elicitation_request());
										rounds
									},
								},
							)
						} else {
							let completed = serde_json::to_value(CallToolResult::success(vec![
								ContentBlock::text("task-done"),
							]))
							.expect("serialize completed result");
							DetailedTask::new(
								Task::new(
									"task-in",
									TaskStatus::Completed,
									"2026-01-01T00:00:00Z",
									"2026-01-01T00:00:01Z",
								),
								TaskPayload::Completed {
									result: completed.as_object().expect("object").clone(),
								},
							)
						};
						ServerResult::GetTaskResult(GetTaskResult::new(task))
					}
					ClientRequest::UpdateTaskRequest(_) => {
						updated_flag.store(true, Ordering::SeqCst);
						ServerResult::task_ack(())
					}
					_ => continue,
				};
				incoming
					.unbounded_send(JsonRpcMessage::response(response, request.id))
					.expect("fake server channel must stay open");
			}
		}
	});

	let result = tokio::time::timeout(WAIT, call_tool(&server, &tool_call("task-in"), None))
		.await
		.expect("task round must not hang")
		.expect("task must complete after the update");
	let text = result
		.content
		.iter()
		.filter_map(|b| match b {
			ContentBlock::Text(t) => Some(t.text.clone()),
			_ => None,
		})
		.collect::<Vec<_>>()
		.join("\n");
	assert!(text.contains("task-done"), "task result: {text}");
	assert!(
		updated.load(Ordering::SeqCst),
		"tasks/update must be sent after fulfilling the elicitation"
	);

	responder.abort();
	disconnect(&name);
}

/// Failed and Cancelled task payloads surface as errors.
#[serial]
#[tokio::test]
async fn task_failed_and_cancelled_payloads_surface_as_errors() {
	for (tag, payload, expected) in [
		(
			"task-failed",
			TaskPayload::Failed {
				error: serde_json::json!({"code": -32000, "message": "boom"})
					.as_object()
					.expect("object")
					.clone(),
			},
			"failed",
		),
		("task-cancelled", TaskPayload::Cancelled, "cancel"),
	] {
		let name = unique_server(tag);
		let peer = serve_in_memory(&name);
		register(&name, peer.service);
		let server = stdin_config(&name, "unused", Vec::new(), 5);

		let responder =
			spawn_responder(peer.outgoing, peer.incoming, move |request| match request {
				ClientRequest::CallToolRequest(_) => Some(ServerResult::CreateTaskResult(
					CreateTaskResult::new(Task::new(
						tag,
						TaskStatus::Working,
						"2026-01-01T00:00:00Z",
						"2026-01-01T00:00:00Z",
					)),
				)),
				ClientRequest::GetTaskRequest(_) => {
					let task = DetailedTask::new(
						Task::new(
							tag,
							payload.status(),
							"2026-01-01T00:00:00Z",
							"2026-01-01T00:00:01Z",
						),
						payload.clone(),
					);
					Some(ServerResult::GetTaskResult(GetTaskResult::new(task)))
				}
				_ => None,
			});

		let error = tokio::time::timeout(WAIT, call_tool(&server, &tool_call(tag), None))
			.await
			.expect("terminal task states must not hang")
			.expect_err("terminal payload must fail the call");
		assert!(
			error.to_string().to_lowercase().contains(expected),
			"unexpected error for {tag}: {error}"
		);

		responder.abort();
		disconnect(&name);
	}
}

// ---------------------------------------------------------------------------
// watch_resource_links — subscriptions/listen delivery paths
// ---------------------------------------------------------------------------

fn result_with_resource_link(uri: &str) -> CallToolResult {
	CallToolResult::success(vec![ContentBlock::resource_link(Resource::new(uri, uri))])
}

#[tokio::test]
async fn read_resource_text_uses_the_owning_server_without_uri_assumptions() {
	let name = unique_server("resource-read");
	let peer = serve_in_memory(&name);
	register(&name, peer.service);
	let responder = spawn_responder(peer.outgoing, peer.incoming, |request| match request {
		ClientRequest::ReadResourceRequest(_) => Some(ServerResult::ReadResourceResult(
			ReadResourceResult::new(vec![ResourceContents::TextResourceContents {
				uri: "custommcp://background/42".to_string(),
				mime_type: Some("text/plain".to_string()),
				text: "status: running\ncurrent output".to_string(),
				meta: None,
			}]),
		)),
		_ => None,
	});

	let text = read_resource_text(&name, "custommcp://background/42")
		.await
		.expect("generic resource read");
	assert_eq!(text, "status: running\ncurrent output");

	responder.abort();
	disconnect(&name);
}

/// Early returns: no links in the result, and links without a session scope.
#[serial]
#[tokio::test]
async fn watch_resource_links_returns_early_without_links_or_session() {
	let name = unique_server("watch-early");
	let peer = serve_in_memory(&name);
	let service = register(&name, peer.service);

	// No links → immediate return, nothing is sent to the server.
	let plain = CallToolResult::success(vec![ContentBlock::text("no links")]);
	tokio::time::timeout(WAIT, watch_resource_links(&service, &name, &plain))
		.await
		.expect("no-links result must return immediately");

	// Links but no session context → immediate return before any listen.
	let linked = result_with_resource_link("job://early-1");
	tokio::time::timeout(WAIT, watch_resource_links(&service, &name, &linked))
		.await
		.expect("sessionless result must return immediately");

	// No subscriptions/listen request was issued in either case.
	let mut outgoing = peer.outgoing;
	let saw_listen = tokio::time::timeout(Duration::from_millis(200), async {
		loop {
			match outgoing.next().await {
				Some(JsonRpcMessage::Request(r)) => {
					if matches!(r.request, ClientRequest::SubscriptionsListenRequest(_)) {
						return true;
					}
				}
				Some(_) => continue,
				None => return false,
			}
		}
	})
	.await;
	assert!(
		!saw_listen.unwrap_or(false),
		"early returns must not open a subscription"
	);

	disconnect(&name);
}

/// A listen request that fails (wrong response type) is skipped without
/// aborting the loop.
#[serial]
#[tokio::test]
async fn watch_resource_links_continues_when_listen_fails() {
	let name = unique_server("watch-fail");
	let peer = serve_in_memory(&name);
	let service = register(&name, peer.service);
	let linked = result_with_resource_link("job://fail-1");

	crate::session::context::with_session_id(
		format!("cov-watch-fail-{}", uuid::Uuid::new_v4()),
		async {
			crate::session::shell_jobs::note_watched_from_result(&name, &linked);
			// Answer the listen request with an unrelated result type → the
			// client treats the stream as misbehaving and returns Err.
			let responder =
				spawn_responder(peer.outgoing, peer.incoming, |request| match request {
					ClientRequest::SubscriptionsListenRequest(_) => Some(
						ServerResult::CallToolResult(CallToolResult::success(vec![])),
					),
					_ => None,
				});
			tokio::time::timeout(WAIT, watch_resource_links(&service, &name, &linked))
				.await
				.expect("listen failure must not hang the loop");
			responder.abort();
		},
	)
	.await;

	disconnect(&name);
}

/// An acknowledgment that does not include the requested URI cancels the
/// subscription instead of spawning a watcher.
#[serial]
#[tokio::test]
async fn watch_resource_links_cancels_unacknowledged_subscription() {
	let name = unique_server("watch-unack");
	let peer = serve_in_memory(&name);
	let service = register(&name, peer.service);
	let linked = result_with_resource_link("job://unack-1");

	crate::session::context::with_session_id(
		format!("cov-watch-unack-{}", uuid::Uuid::new_v4()),
		async {
			crate::session::shell_jobs::note_watched_from_result(&name, &linked);

			let mut outgoing = peer.outgoing;
			let incoming = peer.incoming;
			let responder = tokio::spawn(async move {
				use rmcp::model::GetMeta;
				while let Some(message) = outgoing.next().await {
					if let JsonRpcMessage::Request(request) = message {
						if let ClientRequest::SubscriptionsListenRequest(_) = request.request {
							// Acknowledge an EMPTY filter: the requested uri
							// is not included.
							let mut ack = ServerNotification::SubscriptionsAcknowledgedNotification(
								Notification::new(
									SubscriptionsAcknowledgedNotificationParams::new(
										SubscriptionFilter::new(),
									),
								),
							);
							ack.get_meta_mut().set_subscription_id(request.id.clone());
							incoming
								.unbounded_send(JsonRpcMessage::notification(ack))
								.expect("fake server channel must stay open");
						}
					}
				}
			});

			tokio::time::timeout(WAIT, watch_resource_links(&service, &name, &linked))
				.await
				.expect("unacknowledged subscription must not hang");
			responder.abort();
		},
	)
	.await;

	disconnect(&name);
}

/// An acknowledged subscription for a URI nobody watches is cancelled.
#[serial]
#[tokio::test]
async fn watch_resource_links_cancels_unwatched_subscription() {
	let name = unique_server("watch-unwatched");
	let peer = serve_in_memory(&name);
	let service = register(&name, peer.service);
	let linked = result_with_resource_link("job://unwatched-1");

	crate::session::context::with_session_id(
		format!("cov-watch-unwatched-{}", uuid::Uuid::new_v4()),
		async {
			// Deliberately do NOT note the link as watched.
			let mut outgoing = peer.outgoing;
			let incoming = peer.incoming;
			let uri = "job://unwatched-1".to_string();
			let responder = tokio::spawn(async move {
				use rmcp::model::GetMeta;
				while let Some(message) = outgoing.next().await {
					if let JsonRpcMessage::Request(request) = message {
						if let ClientRequest::SubscriptionsListenRequest(_) = request.request {
							let mut filter = SubscriptionFilter::new();
							filter.resource_subscriptions = Some(vec![uri.clone()]);
							let mut ack = ServerNotification::SubscriptionsAcknowledgedNotification(
								Notification::new(
									SubscriptionsAcknowledgedNotificationParams::new(filter),
								),
							);
							ack.get_meta_mut().set_subscription_id(request.id.clone());
							incoming
								.unbounded_send(JsonRpcMessage::notification(ack))
								.expect("fake server channel must stay open");
						}
					}
				}
			});

			tokio::time::timeout(WAIT, watch_resource_links(&service, &name, &linked))
				.await
				.expect("unwatched subscription must not hang");
			responder.abort();
		},
	)
	.await;

	disconnect(&name);
}

/// Full delivery path: acknowledged subscription over a watched resource, the
/// server pushes resources/updated, the watcher reads the resource and
/// completes the job (WatchEvent::Completed).
#[serial]
#[tokio::test]
async fn watch_resource_links_delivers_update_and_completes() {
	let name = unique_server("watch-full");
	let peer = serve_in_memory(&name);
	let service = register(&name, peer.service);
	let linked = result_with_resource_link("job://full-1");
	let uri = "job://full-1".to_string();

	crate::session::context::with_session_id(
		format!("cov-watch-full-{}", uuid::Uuid::new_v4()),
		async {
			crate::session::shell_jobs::note_watched_from_result(&name, &linked);
			let mut events = crate::session::shell_jobs::subscribe_events();

			let mut outgoing = peer.outgoing;
			let incoming = peer.incoming;
			let push_update = incoming.clone();
			let listen_id: StdArc<StdMutex<Option<RequestId>>> = StdArc::new(StdMutex::new(None));
			let seen_id = StdArc::clone(&listen_id);
			let watched_uri = uri.clone();
			let responder = tokio::spawn(async move {
				use rmcp::model::GetMeta;
				while let Some(message) = outgoing.next().await {
					if let JsonRpcMessage::Request(request) = message {
						match request.request {
							ClientRequest::SubscriptionsListenRequest(_) => {
								let mut filter = SubscriptionFilter::new();
								filter.resource_subscriptions = Some(vec![watched_uri.clone()]);
								let mut ack =
									ServerNotification::SubscriptionsAcknowledgedNotification(
										Notification::new(
											SubscriptionsAcknowledgedNotificationParams::new(
												filter,
											),
										),
									);
								ack.get_meta_mut().set_subscription_id(request.id.clone());
								seen_id
									.lock()
									.expect("listen id lock")
									.replace(request.id.clone());
								incoming
									.unbounded_send(JsonRpcMessage::notification(ack))
									.expect("fake server channel must stay open");
							}
							ClientRequest::ReadResourceRequest(_) => {
								incoming
									.unbounded_send(JsonRpcMessage::response(
										ServerResult::ReadResourceResult(ReadResourceResult::new(
											vec![ResourceContents::TextResourceContents {
												uri: watched_uri.clone(),
												mime_type: Some("text/plain".to_string()),
												text: "job output body".to_string(),
												meta: None,
											}],
										)),
										request.id,
									))
									.expect("fake server channel must stay open");
							}
							_ => {}
						}
					}
				}
			});

			// Establish the watcher.
			tokio::time::timeout(WAIT, watch_resource_links(&service, &name, &linked))
				.await
				.expect("watch setup must not hang");

			// Server pushes the resource update on the open stream. The
			// notification must carry the subscription id of the listen
			// request — rmcp routes stream notifications by that id and
			// rejects mismatches as an abrupt stream end.
			let subscription_id = {
				let mut waited = 0u32;
				loop {
					if let Some(id) = listen_id.lock().expect("listen id lock").clone() {
						break id;
					}
					assert!(waited < 250, "listen request id must arrive");
					waited += 1;
					tokio::time::sleep(Duration::from_millis(4)).await;
				}
			};
			use rmcp::model::GetMeta;
			let mut update = ServerNotification::ResourceUpdatedNotification(Notification::new(
				ResourceUpdatedNotificationParam::new(uri.clone()),
			));
			update.get_meta_mut().set_subscription_id(subscription_id);
			push_update
				.unbounded_send(JsonRpcMessage::notification(update))
				.expect("fake server channel must stay open");

			let session_id = crate::session::context::current_session_id()
				.expect("session id")
				.to_string();
			let event = tokio::time::timeout(WAIT, events.recv())
				.await
				.expect("update must be delivered within the wait")
				.expect("watch event channel must stay open");
			match event {
				crate::session::shell_jobs::WatchEvent::Completed {
					session_id: s,
					uri: u,
				} => {
					assert_eq!(s, session_id);
					assert_eq!(u, uri);
				}
				other => panic!("expected Completed, got {other:?}"),
			}

			responder.abort();
		},
	)
	.await;

	disconnect(&name);
}

// ---------------------------------------------------------------------------
// Remaining uncovered branches: spawn failure, tokenless discovery, token
// rotation, static-header auth, tools/list timeout, task cancellation
// ---------------------------------------------------------------------------

fn text_of(result: &CallToolResult) -> String {
	result
		.content
		.iter()
		.filter_map(|b| match b {
			ContentBlock::Text(t) => Some(t.text.clone()),
			_ => None,
		})
		.collect::<Vec<_>>()
		.join("\n")
}

/// A command that cannot spawn fails both the modern and the legacy handshake
/// attempt; the combined error names the server and both attempts.
#[serial]
#[tokio::test]
async fn connect_stdio_spawn_failure_combines_modern_and_legacy_errors() {
	let name = unique_server("stdio-missing");
	let server = stdin_config(&name, "octomind-definitely-missing-binary", Vec::new(), 5);

	let error = match tokio::time::timeout(WAIT, connect_stdio(&server)).await {
		Ok(Ok(_service)) => panic!("missing binary must fail to connect"),
		Ok(Err(e)) => e,
		Err(_) => panic!("spawn failure must surface quickly"),
	};
	let message = error.to_string();
	assert!(
		message.contains(&format!("Failed to initialize MCP server '{name}'")),
		"unexpected error: {message}"
	);
	assert!(message.contains("modern:"), "unexpected error: {message}");
	assert!(message.contains("legacy:"), "unexpected error: {message}");
	assert!(!is_connected(&name));

	disconnect(&name);
}

/// A server with no OAuth metadata (well-known 404, probe answered 200)
/// fails discovery; the failure is tolerated and the connection proceeds
/// unauthenticated, so the server observes no Authorization header.
#[serial]
#[tokio::test]
async fn connect_http_without_oauth_support_connects_unauthenticated() {
	let name = unique_server("http-noauth");
	let (url, mut child) = spawn_fake_http_server("http-noauth", "no-oauth").await;
	let server = McpServerConfig::http(&name, &url, 15, vec![]);

	let discovery = crate::mcp::oauth::discovery::discover_oauth_from_mcp_server(&url, &name).await;
	assert!(
		discovery.is_err(),
		"server without OAuth metadata must fail discovery"
	);

	let _service = tokio::time::timeout(WAIT, connect_http(&server))
		.await
		.expect("connect must not hang")
		.expect("failed discovery must still connect unauthenticated");
	assert!(is_connected(&name));

	let result = tokio::time::timeout(WAIT, call_tool(&server, &tool_call("noauth"), None))
		.await
		.expect("tools/call must not hang")
		.expect("echo must succeed without auth");
	let text = text_of(&result);
	assert!(
		text.contains("\"auth\": null"),
		"no bearer must be sent when discovery found nothing: {text}"
	);

	let _ = child.kill().await;
	disconnect(&name);
	crate::mcp::oauth::discovery::clear_discovered_oauth_cache(&name);
}

/// Rotating the stored token invalidates the live HTTP connection: the next
/// call detects the change, disconnects, and reconnects unauthenticated.
#[serial]
#[tokio::test]
async fn get_or_connect_reconnects_http_after_token_rotation() {
	let name = unique_server("http-rotate");
	let (url, mut child) = spawn_fake_http_server("http-rotate", "oauth").await;
	let server = McpServerConfig::http(&name, &url, 15, vec![]);

	crate::mcp::oauth::discovery::discover_oauth_from_mcp_server(&url, &name)
		.await
		.expect("fake discovery chain must succeed");
	let expires_at = std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.expect("clock after epoch")
		.as_secs()
		+ 3600;
	crate::mcp::oauth::token_store::save_token(
		&name,
		&crate::mcp::oauth::token_store::TokenMetadata {
			server_name: name.clone(),
			access_token: "test-token-xyz".to_string(),
			refresh_token: None,
			expires_at,
			scopes: vec![],
		},
	)
	.await
	.expect("seed token");

	let first = tokio::time::timeout(WAIT, call_tool(&server, &tool_call("rotate-1"), None))
		.await
		.expect("first call must not hang")
		.expect("first call must succeed");
	let text = text_of(&first);
	assert!(
		text.contains("Bearer test-token-xyz"),
		"seeded token must ride the first request: {text}"
	);

	// Rotate: a fresh token replaces the seeded one. The live connection was
	// built with the old bearer, so the next call must detect the change,
	// disconnect, and reconnect with the new token.
	crate::mcp::oauth::token_store::save_token(
		&name,
		&crate::mcp::oauth::token_store::TokenMetadata {
			server_name: name.clone(),
			access_token: "test-token-rotated".to_string(),
			refresh_token: None,
			expires_at,
			scopes: vec![],
		},
	)
	.await
	.expect("rotate token");

	let second = tokio::time::timeout(WAIT, call_tool(&server, &tool_call("rotate-2"), None))
		.await
		.expect("reconnect must not hang")
		.expect("reconnect after rotation must succeed");
	let text = text_of(&second);
	assert!(
		text.contains("Bearer test-token-rotated"),
		"reconnected request must carry the rotated token: {text}"
	);

	let _ = child.kill().await;
	disconnect(&name);
	crate::mcp::oauth::discovery::clear_discovered_oauth_cache(&name);
	let _ = crate::mcp::oauth::token_store::clear_token(&name, false, None, None, None).await;
}

/// A server that never answers tools/list hits the per-server timeout.
#[serial]
#[tokio::test]
async fn list_tools_times_out_against_silent_server() {
	let name = unique_server("list-silent");
	let peer = serve_in_memory(&name);
	register(&name, peer.service);
	let server = stdin_config(&name, "unused", Vec::new(), 1);

	// Swallow every request: no response ever arrives.
	let responder = spawn_responder(peer.outgoing, peer.incoming, |_| None);

	let error = tokio::time::timeout(WAIT + Duration::from_secs(5), list_tools(&server))
		.await
		.expect("tools/list must hit its timeout, not hang forever")
		.expect_err("silent server must fail tools/list");
	assert!(
		error.to_string().contains("tools/list timed out"),
		"unexpected error: {error}"
	);

	responder.abort();
	disconnect(&name);
}

/// Cancelling before the first poll and cancelling mid-sleep both send
/// tasks/cancel and fail the call with the cancellation sentinel.
#[serial]
#[tokio::test]
async fn task_cancellation_sends_cancel_and_fails() {
	let name = unique_server("task-cancel");
	let peer = serve_in_memory(&name);
	register(&name, peer.service);
	let server = stdin_config(&name, "unused", Vec::new(), 5);

	let saw_cancel = StdArc::new(AtomicBool::new(false));
	let flag = StdArc::clone(&saw_cancel);
	let responder = spawn_responder(peer.outgoing, peer.incoming, move |request| match request {
		ClientRequest::CallToolRequest(_) => Some(ServerResult::CreateTaskResult(
			CreateTaskResult::new(Task::new(
				"task-cancel",
				TaskStatus::Working,
				"2026-01-01T00:00:00Z",
				"2026-01-01T00:00:00Z",
			)),
		)),
		ClientRequest::CancelTaskRequest(_) => {
			flag.store(true, Ordering::SeqCst);
			Some(ServerResult::task_ack(()))
		}
		_ => None,
	});

	// Cancelled before the call: the pre-poll check fires.
	let (tx, rx) = tokio::sync::watch::channel(false);
	tx.send(true).expect("pre-cancel");
	let error = tokio::time::timeout(WAIT, call_tool(&server, &tool_call("tc-pre"), Some(rx)))
		.await
		.expect("pre-cancelled task must not hang")
		.expect_err("pre-cancelled task must fail");
	assert!(
		crate::session::cancellation::is_cancelled(&error),
		"unexpected error: {error}"
	);

	// Cancelled mid-sleep: the sleep_or_cancel branch fires.
	saw_cancel.store(false, Ordering::SeqCst);
	let (tx, rx) = tokio::sync::watch::channel(false);
	let canceller = tokio::spawn(async move {
		tokio::time::sleep(Duration::from_millis(30)).await;
		let _ = tx.send(true);
	});
	let error = tokio::time::timeout(WAIT, call_tool(&server, &tool_call("tc-mid"), Some(rx)))
		.await
		.expect("mid-sleep cancellation must not hang")
		.expect_err("mid-sleep cancelled task must fail");
	assert!(
		crate::session::cancellation::is_cancelled(&error),
		"unexpected error: {error}"
	);
	assert!(
		saw_cancel.load(Ordering::SeqCst),
		"tasks/cancel must be sent for the mid-sleep cancellation"
	);

	canceller.abort();
	responder.abort();
	disconnect(&name);
}

/// A tasks/get answer of the wrong type surfaces as a tasks/get failure.
#[serial]
#[tokio::test]
async fn task_get_wrong_response_type_fails_the_call() {
	let name = unique_server("task-get-bad");
	let peer = serve_in_memory(&name);
	register(&name, peer.service);
	let server = stdin_config(&name, "unused", Vec::new(), 5);

	let responder = spawn_responder(peer.outgoing, peer.incoming, |request| match request {
		ClientRequest::CallToolRequest(_) => Some(ServerResult::CreateTaskResult(
			CreateTaskResult::new(Task::new(
				"task-get-bad",
				TaskStatus::Working,
				"2026-01-01T00:00:00Z",
				"2026-01-01T00:00:00Z",
			)),
		)),
		ClientRequest::GetTaskRequest(_) => Some(ServerResult::CallToolResult(
			CallToolResult::success(vec![]),
		)),
		_ => None,
	});

	let error = tokio::time::timeout(WAIT, call_tool(&server, &tool_call("get-bad"), None))
		.await
		.expect("bad tasks/get answer must not hang")
		.expect_err("wrong-type tasks/get must fail the call");
	assert!(
		error.to_string().contains("tasks/get failed"),
		"unexpected error: {error}"
	);

	responder.abort();
	disconnect(&name);
}
