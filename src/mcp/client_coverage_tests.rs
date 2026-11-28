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
	CallToolResult, ClientRequest, ContentBlock, CreateTaskResult, DetailedTask, ElicitRequestParams, GetTaskResult,
	InputRequest, InputRequests, InputRequiredResult, JsonRpcMessage, Notification, ProgressNotificationParam,
	ReadResourceResult, Request, RequestId, Resource, ResourceContents, ResourceUpdatedNotificationParam,
	ServerNotification, ServerResult, SubscriptionFilter, SubscriptionsAcknowledgedNotificationParams, Task,
	TaskPayload, TaskStatus,
};
use rmcp::service::{RxJsonRpcMessage, TxJsonRpcMessage};
use serial_test::serial;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc as StdArc, Mutex as StdMutex};
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(5);

fn unique_server(tag: &str) -> String {
	format!("octomind-test-cov-{tag}")
}

fn stdin_config(name: &str, command: &str, args: Vec<String>, timeout_seconds: u64) -> McpServerConfig {
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
	let service = rmcp::service::serve_directly(
		OctoClientHandler::new(&stdin_config(server_name, "unused-in-memory", Vec::new(), 5)),
		(out_tx, in_rx),
		None,
	);
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
            # Discovery probes the well-known URLs with and without the
            # MCP path (path-specific, then root), so match by suffix.
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
	let path = std::env::temp_dir().join(format!("octomind-test-cov-{tag}-{}.py", std::process::id()));
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
		env: HashMap::from([("OCTOMIND_TEST_ENV_MARKER".to_string(), "marker-abc-123".to_string())]),
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
		.expect("tools/list must succeed")
		.tools;
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
	assert!(saw_ready_line, "stderr drain must capture the server ready line");

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
		headers: HashMap::from([("Authorization".to_string(), "Bearer static-test".to_string())]),
		auto_bind: None,
	};
	let error = match tokio::time::timeout(WAIT + Duration::from_secs(5), connect_http(&server)).await {
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

/// Outside an interactive session nobody can sign in, so an OAuth server with
/// no stored token fails with the typed `SignInUnavailable` before any tokenless
/// connect, and the startup paths can stay quiet until the session signs in.
#[serial]
#[tokio::test]
async fn connect_http_without_token_outside_session_reports_sign_in_unavailable() {
	let name = unique_server("http-oauth-nosession");
	let (url, mut child) = spawn_fake_http_server("http-oauth-nosession", "oauth").await;
	let server = McpServerConfig::http(&name, &url, 15, vec![]);

	let error = match tokio::time::timeout(WAIT, connect_http(&server))
		.await
		.expect("connect must not hang")
	{
		Ok(_service) => panic!("a tokenless OAuth server must not connect outside a session"),
		Err(e) => e,
	};
	assert!(
		crate::mcp::oauth::is_sign_in_unavailable(&error),
		"unexpected error: {error:#}"
	);
	assert!(!is_connected(&name));

	let _ = child.kill().await;
	crate::mcp::oauth::discovery::clear_discovered_oauth_cache(&name);
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
	let short_lived = std::process::Command::new("true").spawn().expect("spawn true");
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
	assert!(reaped, "short-lived child must exit before the test continues");
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
								.unbounded_send(JsonRpcMessage::notification(ServerNotification::ProgressNotification(
									Notification::new(ProgressNotificationParam::new(token.clone(), 1.0)),
								)))
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

	let error = match tokio::time::timeout(Duration::from_secs(30), call_tool(&server, &tool_call("cap"), None)).await {
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
							ServerResult::CallToolResult(CallToolResult::success(vec![ContentBlock::text(
								"after-input",
							)]))
						} else {
							let mut rounds = InputRequests::new();
							rounds.insert("k1".to_string(), elicitation_request());
							ServerResult::InputRequiredResult(InputRequiredResult::from_input_requests(rounds))
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
	let elicitation_response = responses.get("k1").expect("response keyed by the request id");
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
						ServerResult::InputRequiredResult(InputRequiredResult::from_input_requests(rounds))
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
						ServerResult::InputRequiredResult(InputRequiredResult::from_input_requests(rounds))
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

	let responder = spawn_responder(peer.outgoing, peer.incoming.clone(), |request| match request {
		ClientRequest::CallToolRequest(_) => Some(ServerResult::InputRequiredResult(
			InputRequiredResult::from_request_state("perpetually-busy"),
		)),
		_ => None,
	});

	// Ten state-only rounds with 50–250ms backoff ≈ 2.1s in real time.
	let error = match tokio::time::timeout(Duration::from_secs(15), call_tool(&server, &tool_call("limit"), None)).await
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
							let completed =
								serde_json::to_value(CallToolResult::success(vec![ContentBlock::text("task-done")]))
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

		let responder = spawn_responder(peer.outgoing, peer.incoming, move |request| match request {
			ClientRequest::CallToolRequest(_) => Some(ServerResult::CreateTaskResult(CreateTaskResult::new(
				Task::new(tag, TaskStatus::Working, "2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z"),
			))),
			ClientRequest::GetTaskRequest(_) => {
				let task = DetailedTask::new(
					Task::new(tag, payload.status(), "2026-01-01T00:00:00Z", "2026-01-01T00:00:01Z"),
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
		ClientRequest::ReadResourceRequest(_) => Some(ServerResult::ReadResourceResult(ReadResourceResult::new(vec![
			ResourceContents::TextResourceContents {
				uri: "custommcp://background/42".to_string(),
				mime_type: Some("text/plain".to_string()),
				text: "status: running\ncurrent output".to_string(),
				meta: None,
			},
		]))),
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
	register(&name, peer.service);
	let server = stdin_config(&name, "unused", Vec::new(), 5);

	// No links → immediate return, nothing is sent to the server.
	let plain = CallToolResult::success(vec![ContentBlock::text("no links")]);
	tokio::time::timeout(WAIT, watch_resource_links(&server, &plain))
		.await
		.expect("no-links result must return immediately");

	// Links but no session context → immediate return before any listen.
	let linked = result_with_resource_link("job://early-1");
	tokio::time::timeout(WAIT, watch_resource_links(&server, &linked))
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
	register(&name, peer.service);
	let server = stdin_config(&name, "unused", Vec::new(), 5);
	let linked = result_with_resource_link("job://fail-1");

	crate::session::context::with_session_id(format!("cov-watch-fail-{}", uuid::Uuid::new_v4()), async {
		crate::session::shell_jobs::note_watched_from_server_config(&server, &linked);
		// Answer the listen request with an unrelated result type → the
		// client treats the stream as misbehaving and returns Err.
		let responder = spawn_responder(peer.outgoing, peer.incoming, |request| match request {
			ClientRequest::SubscriptionsListenRequest(_) => {
				Some(ServerResult::CallToolResult(CallToolResult::success(vec![])))
			}
			_ => None,
		});
		tokio::time::timeout(WAIT, watch_resource_links(&server, &linked))
			.await
			.expect("listen failure must not hang the loop");
		responder.abort();
	})
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
	register(&name, peer.service);
	let server = stdin_config(&name, "unused", Vec::new(), 5);
	let linked = result_with_resource_link("job://unack-1");

	crate::session::context::with_session_id(format!("cov-watch-unack-{}", uuid::Uuid::new_v4()), async {
		crate::session::shell_jobs::note_watched_from_server_config(&server, &linked);

		let mut outgoing = peer.outgoing;
		let incoming = peer.incoming;
		let responder = tokio::spawn(async move {
			use rmcp::model::GetMeta;
			while let Some(message) = outgoing.next().await {
				if let JsonRpcMessage::Request(request) = message {
					if let ClientRequest::SubscriptionsListenRequest(_) = request.request {
						// Acknowledge an EMPTY filter: the requested uri
						// is not included.
						let mut ack = ServerNotification::SubscriptionsAcknowledgedNotification(Notification::new(
							SubscriptionsAcknowledgedNotificationParams::new(SubscriptionFilter::new()),
						));
						ack.get_meta_mut().set_subscription_id(request.id.clone());
						incoming
							.unbounded_send(JsonRpcMessage::notification(ack))
							.expect("fake server channel must stay open");
					}
				}
			}
		});

		tokio::time::timeout(WAIT, watch_resource_links(&server, &linked))
			.await
			.expect("unacknowledged subscription must not hang");
		responder.abort();
	})
	.await;

	disconnect(&name);
}

/// No subscription is opened for a URI nobody watches: a watcher acts for a
/// registration, and there is none.
#[serial]
#[tokio::test]
async fn watch_resource_links_opens_no_subscription_for_an_unwatched_resource() {
	let name = unique_server("watch-unwatched");
	let peer = serve_in_memory(&name);
	register(&name, peer.service);
	let server = stdin_config(&name, "unused", Vec::new(), 5);
	let linked = result_with_resource_link("job://unwatched-1");

	crate::session::context::with_session_id(format!("cov-watch-unwatched-{}", uuid::Uuid::new_v4()), async {
		// Deliberately do NOT note the link as watched.
		let mut outgoing = peer.outgoing;
		tokio::time::timeout(WAIT, watch_resource_links(&server, &linked))
			.await
			.expect("an unwatched link must not hang");
		let heard = tokio::time::timeout(Duration::from_millis(200), async { outgoing.next().await.is_some() }).await;
		assert!(heard.is_err(), "nothing reaches the server for an unwatched resource");
	})
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
	register(&name, peer.service);
	let server = stdin_config(&name, "unused", Vec::new(), 5);
	let linked = result_with_resource_link("job://full-1");
	let uri = "job://full-1".to_string();

	crate::session::context::with_session_id(format!("cov-watch-full-{}", uuid::Uuid::new_v4()), async {
		crate::session::shell_jobs::note_watched_from_server_config(&server, &linked);
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
							let mut ack = ServerNotification::SubscriptionsAcknowledgedNotification(Notification::new(
								SubscriptionsAcknowledgedNotificationParams::new(filter),
							));
							ack.get_meta_mut().set_subscription_id(request.id.clone());
							seen_id.lock().expect("listen id lock").replace(request.id.clone());
							incoming
								.unbounded_send(JsonRpcMessage::notification(ack))
								.expect("fake server channel must stay open");
						}
						ClientRequest::ReadResourceRequest(_) => {
							incoming
								.unbounded_send(JsonRpcMessage::response(
									ServerResult::ReadResourceResult(ReadResourceResult::new(vec![
										ResourceContents::TextResourceContents {
											uri: watched_uri.clone(),
											mime_type: Some("text/plain".to_string()),
											text: "job output body".to_string(),
											meta: None,
										},
									])),
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
		tokio::time::timeout(WAIT, watch_resource_links(&server, &linked))
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
			crate::session::shell_jobs::WatchEvent::Completed { session_id: s, uri: u } => {
				assert_eq!(s, session_id);
				assert_eq!(u, uri);
			}
			other => panic!("expected Completed, got {other:?}"),
		}

		responder.abort();
	})
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
	assert!(discovery.is_err(), "server without OAuth metadata must fail discovery");

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
		ClientRequest::CallToolRequest(_) => Some(ServerResult::CreateTaskResult(CreateTaskResult::new(Task::new(
			"task-cancel",
			TaskStatus::Working,
			"2026-01-01T00:00:00Z",
			"2026-01-01T00:00:00Z",
		)))),
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
		ClientRequest::CallToolRequest(_) => Some(ServerResult::CreateTaskResult(CreateTaskResult::new(Task::new(
			"task-get-bad",
			TaskStatus::Working,
			"2026-01-01T00:00:00Z",
			"2026-01-01T00:00:00Z",
		)))),
		ClientRequest::GetTaskRequest(_) => Some(ServerResult::CallToolResult(CallToolResult::success(vec![]))),
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

fn working_task(id: &str) -> Task {
	Task::new(id, TaskStatus::Working, "2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z")
		.with_status_message("shell: cargo test")
		.with_poll_interval_ms(50)
}

async fn next_inbox_batch() -> Vec<crate::session::inbox::InboxMessage> {
	tokio::time::timeout(WAIT, async {
		loop {
			let batch = crate::session::inbox::drain_inbox_batch();
			if !batch.is_empty() {
				return batch;
			}
			tokio::time::sleep(Duration::from_millis(20)).await;
		}
	})
	.await
	.expect("the task outcome must reach the inbox")
}

/// In a session a task does not hold the turn: the call returns at once, the
/// task stays pending, and its result arrives in the inbox when it completes.
#[serial]
#[tokio::test]
async fn session_task_runs_in_the_background_and_reaches_the_inbox() {
	let name = unique_server("task-background");
	let peer = serve_in_memory(&name);
	register(&name, peer.service);
	let server = stdin_config(&name, "unused", Vec::new(), 5);

	let polls = StdArc::new(AtomicUsize::new(0));
	let counter = StdArc::clone(&polls);
	let responder = spawn_responder(peer.outgoing, peer.incoming, move |request| match request {
		ClientRequest::CallToolRequest(_) => Some(ServerResult::CreateTaskResult(CreateTaskResult::new(working_task(
			"task-bg",
		)))),
		ClientRequest::GetTaskRequest(_) => {
			let payload = if counter.fetch_add(1, Ordering::SeqCst) < 3 {
				TaskPayload::Working
			} else {
				let result = serde_json::to_value(CallToolResult::success(vec![ContentBlock::text("built")]))
					.expect("serialize completed result");
				TaskPayload::Completed {
					result: result.as_object().expect("object").clone(),
				}
			};
			Some(ServerResult::GetTaskResult(GetTaskResult::new(DetailedTask::new(
				working_task("task-bg"),
				payload,
			))))
		}
		_ => None,
	});

	let session_id = format!("cov-task-bg-{}", uuid::Uuid::new_v4());
	crate::session::context::with_session_id(session_id.clone(), async {
		crate::session::inbox::init_inbox_for_session();
		let result = tokio::time::timeout(WAIT, call_tool(&server, &tool_call("bg-1"), None))
			.await
			.expect("a session task must not hold the call")
			.expect("the call must hand back a notice");
		let notice = result.content[0].as_text().expect("text notice").text.clone();
		assert!(notice.contains("mcp-task:bg-1"), "{notice}");
		assert!(notice.contains("shell: cargo test"), "{notice}");
		assert!(crate::session::shell_jobs::has_pending_for_session(&session_id));

		let batch = next_inbox_batch().await;
		assert!(
			batch[0]
				.content
				.starts_with("<background_job task=\"mcp-task:bg-1\">\nbuilt"),
			"{}",
			batch[0].content
		);
		assert!(!crate::session::shell_jobs::has_pending_for_session(&session_id));
		let completed = crate::session::shell_jobs::take_completed_for_session(&session_id);
		assert_eq!(completed.len(), 1);
		assert_eq!(completed[0].body, "built");
	})
	.await;

	responder.abort();
	disconnect(&name);
}

/// Tearing the session down stops a background task on the server, and its
/// outcome is not delivered to the gone session.
#[serial]
#[tokio::test]
async fn session_teardown_cancels_a_background_task() {
	let name = unique_server("task-teardown");
	let peer = serve_in_memory(&name);
	register(&name, peer.service);
	let server = stdin_config(&name, "unused", Vec::new(), 5);

	let saw_cancel = StdArc::new(AtomicBool::new(false));
	let flag = StdArc::clone(&saw_cancel);
	let responder = spawn_responder(peer.outgoing, peer.incoming, move |request| match request {
		ClientRequest::CallToolRequest(_) => Some(ServerResult::CreateTaskResult(CreateTaskResult::new(working_task(
			"task-teardown",
		)))),
		ClientRequest::GetTaskRequest(_) => Some(ServerResult::GetTaskResult(GetTaskResult::new(DetailedTask::new(
			working_task("task-teardown"),
			TaskPayload::Working,
		)))),
		ClientRequest::CancelTaskRequest(_) => {
			flag.store(true, Ordering::SeqCst);
			Some(ServerResult::task_ack(()))
		}
		_ => None,
	});

	let session_id = format!("cov-task-teardown-{}", uuid::Uuid::new_v4());
	crate::session::context::with_session_id(session_id.clone(), async {
		crate::session::inbox::init_inbox_for_session();
		tokio::time::timeout(WAIT, call_tool(&server, &tool_call("teardown-1"), None))
			.await
			.expect("a session task must not hold the call")
			.expect("the call must hand back a notice");

		crate::session::shell_jobs::clear_for_session(&session_id);
		tokio::time::timeout(WAIT, async {
			while !saw_cancel.load(Ordering::SeqCst) {
				tokio::time::sleep(Duration::from_millis(20)).await;
			}
		})
		.await
		.expect("teardown must send tasks/cancel");
		tokio::time::sleep(Duration::from_millis(100)).await;
		assert!(crate::session::inbox::drain_inbox_batch().is_empty());
	})
	.await;

	responder.abort();
	disconnect(&name);
}

/// A failed outcome still ends the job, flagged so the model reads it as a
/// failure: a completed result with `isError`, or a task the server failed.
#[serial]
#[tokio::test]
async fn session_task_errors_reach_the_inbox_flagged() {
	let name = unique_server("task-errors");
	let peer = serve_in_memory(&name);
	register(&name, peer.service);
	let server = stdin_config(&name, "unused", Vec::new(), 5);

	let calls = StdArc::new(AtomicUsize::new(0));
	let counter = StdArc::clone(&calls);
	let responder = spawn_responder(peer.outgoing, peer.incoming, move |request| match request {
		ClientRequest::CallToolRequest(_) => {
			let id = if counter.fetch_add(1, Ordering::SeqCst) == 0 {
				"task-err"
			} else {
				"task-fail"
			};
			Some(ServerResult::CreateTaskResult(CreateTaskResult::new(working_task(id))))
		}
		ClientRequest::GetTaskRequest(request) => {
			let id = request.params.task_id;
			let payload = if id == "task-err" {
				let result = serde_json::to_value(CallToolResult::error(vec![ContentBlock::text(
					"Command failed with exit code 3",
				)]))
				.expect("serialize error result");
				TaskPayload::Completed {
					result: result.as_object().expect("object").clone(),
				}
			} else {
				let error = serde_json::json!({"code": -32603, "message": "boom"});
				TaskPayload::Failed {
					error: error.as_object().expect("object").clone(),
				}
			};
			Some(ServerResult::GetTaskResult(GetTaskResult::new(DetailedTask::new(
				working_task(&id),
				payload,
			))))
		}
		_ => None,
	});

	let session_id = format!("cov-task-errors-{}", uuid::Uuid::new_v4());
	crate::session::context::with_session_id(session_id.clone(), async {
		crate::session::inbox::init_inbox_for_session();

		tokio::time::timeout(WAIT, call_tool(&server, &tool_call("err-1"), None))
			.await
			.expect("a session task must not hold the call")
			.expect("the call must hand back a notice");
		let batch = next_inbox_batch().await;
		assert!(
			batch[0].content.starts_with(
				"<background_job task=\"mcp-task:err-1\" is_error=\"true\">\n\
				 Command failed with exit code 3"
			),
			"{}",
			batch[0].content
		);

		tokio::time::timeout(WAIT, call_tool(&server, &tool_call("fail-1"), None))
			.await
			.expect("a session task must not hold the call")
			.expect("the call must hand back a notice");
		let batch = next_inbox_batch().await;
		assert!(
			batch[0]
				.content
				.starts_with("<background_job task=\"mcp-task:fail-1\" is_error=\"true\">"),
			"{}",
			batch[0].content
		);
		assert!(batch[0].content.contains("boom"), "{}", batch[0].content);
		assert!(!crate::session::shell_jobs::has_pending_for_session(&session_id));
		crate::session::shell_jobs::clear_for_session(&session_id);
	})
	.await;

	responder.abort();
	disconnect(&name);
}

/// The poller records each polled status message, and a stop request (what
/// `monitor` stop sends) cancels the task on the server and still ends the job
/// with a flagged outcome.
#[serial]
#[tokio::test]
async fn stopping_a_background_task_cancels_it_and_reports_the_stop() {
	let name = unique_server("task-stop");
	let peer = serve_in_memory(&name);
	register(&name, peer.service);
	let server = stdin_config(&name, "unused", Vec::new(), 5);

	let cancels = StdArc::new(AtomicUsize::new(0));
	let cancel_count = StdArc::clone(&cancels);
	let responder = spawn_responder(peer.outgoing, peer.incoming, move |request| match request {
		ClientRequest::CallToolRequest(_) => Some(ServerResult::CreateTaskResult(CreateTaskResult::new(working_task(
			"task-stop",
		)))),
		ClientRequest::GetTaskRequest(_) => Some(ServerResult::GetTaskResult(GetTaskResult::new(DetailedTask::new(
			working_task("task-stop").with_status_message("shell: cargo test · running 3s"),
			TaskPayload::Working,
		)))),
		ClientRequest::CancelTaskRequest(_) => {
			cancel_count.fetch_add(1, Ordering::SeqCst);
			Some(ServerResult::task_ack(()))
		}
		_ => None,
	});

	let session_id = format!("cov-task-stop-{}", uuid::Uuid::new_v4());
	crate::session::context::with_session_id(session_id.clone(), async {
		crate::session::inbox::init_inbox_for_session();
		let result = tokio::time::timeout(WAIT, call_tool(&server, &tool_call("stop-1"), None))
			.await
			.expect("a session task must not hold the call")
			.expect("the call must hand back a notice");
		let notice = result.content[0].as_text().expect("text notice").text.clone();
		assert!(
			notice.contains("`monitor` with action `stop` and id `mcp-task:stop-1`"),
			"{notice}"
		);

		let key = crate::session::shell_jobs::task_key("stop-1");
		tokio::time::timeout(WAIT, async {
			loop {
				let status = crate::session::shell_jobs::pending_resources_for_session(&session_id)
					.into_iter()
					.find(|job| job.uri == key)
					.and_then(|job| job.status);
				if status.as_deref() == Some("shell: cargo test · running 3s") {
					break;
				}
				tokio::time::sleep(Duration::from_millis(20)).await;
			}
		})
		.await
		.expect("the polled status message must be recorded");

		assert!(crate::session::shell_jobs::stop_task_for_session(&session_id, &key));
		let batch = next_inbox_batch().await;
		assert!(
			batch[0].content.starts_with(
				"<background_job task=\"mcp-task:stop-1\" is_error=\"true\">\n\
				 stopped on request"
			),
			"{}",
			batch[0].content
		);
		assert_eq!(cancels.load(Ordering::SeqCst), 1, "tasks/cancel is sent once");
		assert!(!crate::session::shell_jobs::has_pending_for_session(&session_id));
		crate::session::shell_jobs::clear_for_session(&session_id);
	})
	.await;

	responder.abort();
	disconnect(&name);
}

fn completed_task(id: &str, text: &str) -> ServerResult {
	let result = serde_json::to_value(CallToolResult::success(vec![ContentBlock::text(text)]))
		.expect("serialize completed result");
	ServerResult::GetTaskResult(GetTaskResult::new(DetailedTask::new(
		working_task(id),
		TaskPayload::Completed {
			result: result.as_object().expect("object").clone(),
		},
	)))
}

/// A `tasks/get` that times out says nothing about the task: polling goes on
/// and the next answer completes the call.
#[serial]
#[tokio::test]
async fn task_poll_keeps_going_after_a_timed_out_tasks_get() {
	let name = unique_server("task-poll-timeout");
	let peer = serve_in_memory(&name);
	register(&name, peer.service);
	// A 1s request timeout so the unanswered poll expires quickly.
	let server = stdin_config(&name, "unused", Vec::new(), 1);

	let polls = StdArc::new(AtomicUsize::new(0));
	let counter = StdArc::clone(&polls);
	let responder = spawn_responder(peer.outgoing, peer.incoming, move |request| match request {
		ClientRequest::CallToolRequest(_) => Some(ServerResult::CreateTaskResult(CreateTaskResult::new(working_task(
			"task-slow",
		)))),
		// The first poll is never answered.
		ClientRequest::GetTaskRequest(_) if counter.fetch_add(1, Ordering::SeqCst) == 0 => None,
		ClientRequest::GetTaskRequest(_) => Some(completed_task("task-slow", "done")),
		_ => None,
	});

	let result = tokio::time::timeout(WAIT, call_tool(&server, &tool_call("slow-1"), None))
		.await
		.expect("polling must not hang")
		.expect("a timed-out poll must not fail the task");
	assert_eq!(result.content[0].as_text().expect("text").text, "done");
	assert!(polls.load(Ordering::SeqCst) >= 2);

	responder.abort();
	disconnect(&name);
}

/// Start a task on one in-memory connection, wait for its first poll, then
/// replace the connection with a second one answered by `respond` and close
/// the first, as a reconnect after a dropped transport would.
async fn poll_across_a_reconnect(
	tag: &str,
	respond: impl Fn(rmcp::model::RequestId) -> RxJsonRpcMessage<RoleClient> + Send + 'static,
) -> Result<CallToolResult> {
	let name = unique_server(tag);
	let first = serve_in_memory(&name);
	register(&name, first.service);
	let server = stdin_config(&name, "unused", Vec::new(), 5);

	let polled = StdArc::new(AtomicBool::new(false));
	let flag = StdArc::clone(&polled);
	let first_responder = spawn_responder(first.outgoing, first.incoming, move |request| match request {
		ClientRequest::CallToolRequest(_) => Some(ServerResult::CreateTaskResult(CreateTaskResult::new(working_task(
			"task-moved",
		)))),
		ClientRequest::GetTaskRequest(_) => {
			flag.store(true, Ordering::SeqCst);
			Some(ServerResult::GetTaskResult(GetTaskResult::new(DetailedTask::new(
				working_task("task-moved"),
				TaskPayload::Working,
			))))
		}
		_ => None,
	});
	let call = tokio::spawn(async move { call_tool(&server, &tool_call("moved-1"), None).await });
	tokio::time::timeout(WAIT, async {
		while !polled.load(Ordering::SeqCst) {
			tokio::time::sleep(Duration::from_millis(10)).await;
		}
	})
	.await
	.expect("the task must be polled on the first connection");

	let second = serve_in_memory(&name);
	register(&name, second.service);
	let mut outgoing = second.outgoing;
	let incoming = second.incoming;
	let second_responder = tokio::spawn(async move {
		while let Some(message) = outgoing.next().await {
			if let JsonRpcMessage::Request(request) = message {
				if matches!(request.request, ClientRequest::GetTaskRequest(_)) {
					incoming
						.unbounded_send(respond(request.id))
						.expect("fake server channel must stay open");
				}
			}
		}
	});
	// Dropping the first responder drops the first connection's transport.
	first_responder.abort();

	let outcome = tokio::time::timeout(WAIT, call)
		.await
		.expect("polling must not hang across a reconnect")
		.expect("call task must not panic");
	second_responder.abort();
	disconnect(&name);
	outcome
}

/// A closed connection is not the task's end: the poller reconnects and the
/// server that still knows the task completes it.
#[serial]
#[tokio::test]
async fn task_poll_reconnects_after_the_connection_closes() {
	let result = poll_across_a_reconnect("task-reconnect", |id| {
		JsonRpcMessage::response(completed_task("task-moved", "done"), id)
	})
	.await
	.expect("the reconnected poll must complete the task");
	assert_eq!(result.content[0].as_text().expect("text").text, "done");
}

/// After a reconnect to a server that no longer knows the task (a restarted
/// process), the task is reported lost instead of polled forever.
#[serial]
#[tokio::test]
async fn task_unknown_after_a_reconnect_is_reported_lost() {
	let error = poll_across_a_reconnect("task-lost", |id| {
		JsonRpcMessage::error(
			rmcp::ErrorData::invalid_params("unknown task: task-moved", None),
			Some(id),
		)
	})
	.await
	.expect_err("an unknown task must end the call");
	assert!(
		error
			.to_string()
			.contains("the server no longer knows MCP task 'task-moved'"),
		"unexpected error: {error}"
	);
}

/// A job is tracked for as long as it keeps answering, however long that is:
/// one four hours old that only says "working" is never abandoned, restarted or
/// cancelled, nothing wakes the model for its age or its unchanged status, and
/// its completion arrives once, when the server says so.
#[serial]
#[tokio::test]
async fn a_background_task_is_tracked_for_as_long_as_it_keeps_answering() {
	let name = unique_server("task-long-running");
	let peer = serve_in_memory(&name);
	register(&name, peer.service);
	let server = stdin_config(&name, "unused", Vec::new(), 5);

	let polls = StdArc::new(AtomicUsize::new(0));
	let counter = StdArc::clone(&polls);
	let cancels = StdArc::new(AtomicUsize::new(0));
	let cancel_count = StdArc::clone(&cancels);
	let responder = spawn_responder(peer.outgoing, peer.incoming, move |request| match request {
		ClientRequest::CallToolRequest(_) => Some(ServerResult::CreateTaskResult(CreateTaskResult::new(working_task(
			"task-long",
		)))),
		ClientRequest::GetTaskRequest(_) => {
			let payload = if counter.fetch_add(1, Ordering::SeqCst) < 6 {
				TaskPayload::Working
			} else {
				let result =
					serde_json::to_value(CallToolResult::success(vec![ContentBlock::text("built after hours")]))
						.expect("serialize completed result");
				TaskPayload::Completed {
					result: result.as_object().expect("object").clone(),
				}
			};
			Some(ServerResult::GetTaskResult(GetTaskResult::new(DetailedTask::new(
				working_task("task-long"),
				payload,
			))))
		}
		ClientRequest::CancelTaskRequest(_) => {
			cancel_count.fetch_add(1, Ordering::SeqCst);
			Some(ServerResult::task_ack(()))
		}
		_ => None,
	});

	let session_id = format!("cov-task-long-{}", uuid::Uuid::new_v4());
	crate::session::context::with_session_id(session_id.clone(), async {
		crate::session::inbox::init_inbox_for_session();
		tokio::time::timeout(WAIT, call_tool(&server, &tool_call("long-1"), None))
			.await
			.expect("a session task must not hold the call")
			.expect("the call must hand back a notice");
		let key = crate::session::shell_jobs::task_key("long-1");
		// Four hours in and silent: still tracked, nothing delivered, nobody woken.
		crate::session::shell_jobs::age_for_tests(&session_id, &key);
		tokio::time::sleep(Duration::from_millis(200)).await;
		assert!(polls.load(Ordering::SeqCst) >= 2, "the poller is at work");
		assert!(
			crate::session::shell_jobs::has_pending_for_session(&session_id),
			"a silent job four hours old is still tracked"
		);
		assert!(
			crate::session::inbox::drain_inbox_batch().is_empty(),
			"nothing wakes the model for elapsed time or an unchanged status"
		);

		let batch = next_inbox_batch().await;
		assert!(
			batch[0]
				.content
				.starts_with("<background_job task=\"mcp-task:long-1\">\nbuilt after hours"),
			"{}",
			batch[0].content
		);
		assert_eq!(
			cancels.load(Ordering::SeqCst),
			0,
			"a job is never cancelled for its age"
		);
		assert!(!crate::session::shell_jobs::has_pending_for_session(&session_id));
		assert_eq!(
			crate::session::shell_jobs::take_completed_for_session(&session_id).len(),
			1
		);
	})
	.await;

	responder.abort();
	disconnect(&name);
}

/// A dropped connection is not the task's end: the poller reconnects and asks
/// for the same task id again — one launch, no cancel — and the reconnected
/// poll delivers the result once.
#[serial]
#[tokio::test]
async fn a_dropped_connection_then_the_same_task_id_completes_once() {
	let name = unique_server("task-drop-background");
	let first = serve_in_memory(&name);
	register(&name, first.service);
	let server = stdin_config(&name, "unused", Vec::new(), 5);

	let launches = StdArc::new(AtomicUsize::new(0));
	let cancels = StdArc::new(AtomicUsize::new(0));
	let polled = StdArc::new(AtomicBool::new(false));
	let (launch_count, cancel_count, polled_flag) = (
		StdArc::clone(&launches),
		StdArc::clone(&cancels),
		StdArc::clone(&polled),
	);
	let first_responder = spawn_responder(first.outgoing, first.incoming, move |request| match request {
		ClientRequest::CallToolRequest(_) => {
			launch_count.fetch_add(1, Ordering::SeqCst);
			Some(ServerResult::CreateTaskResult(CreateTaskResult::new(working_task(
				"task-drop",
			))))
		}
		ClientRequest::GetTaskRequest(request) => {
			assert_eq!(request.params.task_id, "task-drop");
			polled_flag.store(true, Ordering::SeqCst);
			Some(ServerResult::GetTaskResult(GetTaskResult::new(DetailedTask::new(
				working_task("task-drop"),
				TaskPayload::Working,
			))))
		}
		ClientRequest::CancelTaskRequest(_) => {
			cancel_count.fetch_add(1, Ordering::SeqCst);
			Some(ServerResult::task_ack(()))
		}
		_ => None,
	});

	let session_id = format!("cov-task-drop-{}", uuid::Uuid::new_v4());
	crate::session::context::with_session_id(session_id.clone(), async {
		crate::session::inbox::init_inbox_for_session();
		let call = tokio::spawn(crate::session::context::with_session_id(
			session_id.clone(),
			async move { call_tool(&server, &tool_call("drop-1"), None).await },
		));
		tokio::time::timeout(WAIT, async {
			while !polled.load(Ordering::SeqCst) {
				tokio::time::sleep(Duration::from_millis(10)).await;
			}
		})
		.await
		.expect("the task must be polled on the first connection");

		// The connection drops; a fresh one to the same server still knows the task.
		let second = serve_in_memory(&name);
		register(&name, second.service);
		let second_responder = spawn_responder(second.outgoing, second.incoming, |request| match request {
			ClientRequest::GetTaskRequest(request) => {
				assert_eq!(
					request.params.task_id, "task-drop",
					"the same task id, never a resubmission"
				);
				Some(completed_task("task-drop", "done after the drop"))
			}
			request => panic!("unexpected request {request:?}"),
		});
		// Dropping the first responder drops the first connection's transport.
		first_responder.abort();

		let batch = next_inbox_batch().await;
		assert_eq!(batch.len(), 1, "{batch:?}");
		assert!(
			batch[0]
				.content
				.starts_with("<background_job task=\"mcp-task:drop-1\">\ndone after the drop"),
			"{}",
			batch[0].content
		);
		assert_eq!(launches.load(Ordering::SeqCst), 1, "one launch");
		assert_eq!(
			cancels.load(Ordering::SeqCst),
			0,
			"a dropped connection cancels nothing"
		);
		tokio::time::timeout(WAIT, call)
			.await
			.expect("the call returned at once")
			.expect("the call task must not panic")
			.expect("the call must hand back a notice");
		second_responder.abort();
	})
	.await;

	disconnect(&name);
}

/// A server nobody could reach for the whole outage budget is a failed
/// observation, not the task's outcome: the poller keeps asking for the same id
/// — never a second launch, never a cancel — and only an outage lasting the
/// whole budget ends the tracking, with one result saying the outcome is
/// unknown. Unanswered polls time out after the server's 1s request timeout.
#[serial]
#[tokio::test]
async fn a_task_nobody_could_observe_for_the_whole_budget_is_reported_lost_once() {
	let name = unique_server("task-lost-background");
	let peer = serve_in_memory(&name);
	register(&name, peer.service);
	let server = stdin_config(&name, "unused", Vec::new(), 1);

	let launches = StdArc::new(AtomicUsize::new(0));
	let polls = StdArc::new(AtomicUsize::new(0));
	let cancels = StdArc::new(AtomicUsize::new(0));
	let (launch_count, poll_count, cancel_count) =
		(StdArc::clone(&launches), StdArc::clone(&polls), StdArc::clone(&cancels));
	let mut outgoing = peer.outgoing;
	let incoming = peer.incoming;
	let responder = tokio::spawn(async move {
		while let Some(message) = outgoing.next().await {
			if let JsonRpcMessage::Request(request) = message {
				let response = match request.request {
					ClientRequest::CallToolRequest(_) => {
						launch_count.fetch_add(1, Ordering::SeqCst);
						JsonRpcMessage::response(
							ServerResult::CreateTaskResult(CreateTaskResult::new(working_task("task-gone"))),
							request.id,
						)
					}
					ClientRequest::GetTaskRequest(poll) => {
						assert_eq!(poll.params.task_id, "task-gone");
						poll_count.fetch_add(1, Ordering::SeqCst);
						// Never answered: the poll times out and counts as a miss.
						continue;
					}
					ClientRequest::CancelTaskRequest(_) => {
						cancel_count.fetch_add(1, Ordering::SeqCst);
						JsonRpcMessage::response(ServerResult::task_ack(()), request.id)
					}
					other => panic!("unexpected request {other:?}"),
				};
				incoming
					.unbounded_send(response)
					.expect("fake server channel must stay open");
			}
		}
	});

	let session_id = format!("cov-task-lost-{}", uuid::Uuid::new_v4());
	crate::session::context::with_session_id(session_id.clone(), async {
		crate::session::inbox::init_inbox_for_session();
		tokio::time::timeout(WAIT, call_tool(&server, &tool_call("gone-1"), None))
			.await
			.expect("a session task must not hold the call")
			.expect("the call must hand back a notice");
		let key = crate::session::shell_jobs::task_key("gone-1");
		// The first miss starts the budget; mark it as run out, as two hours of
		// misses would.
		tokio::time::timeout(WAIT, async {
			while !crate::session::shell_jobs::exhaust_outage_for_tests(&session_id, &key) {
				tokio::time::sleep(Duration::from_millis(10)).await;
			}
		})
		.await
		.expect("the poller records the failed observation");

		let batch = next_inbox_batch().await;
		let content = &batch[0].content;
		assert!(
			content.starts_with(&format!(
				"<background_job task=\"{key}\"{}>",
				crate::session::shell_jobs::DELIVERY_ERROR_ATTR
			)),
			"{content}"
		);
		assert!(
			content.contains("lost track")
				&& content.contains("outcome is unknown")
				&& content.contains("outage budget")
				&& content.contains("Nothing was cancelled"),
			"{content}"
		);
		assert!(!crate::session::shell_jobs::has_pending_for_session(&session_id));
		assert_eq!(cancels.load(Ordering::SeqCst), 0, "nothing is cancelled");
		assert_eq!(launches.load(Ordering::SeqCst), 1, "never resubmitted");
		assert!(polls.load(Ordering::SeqCst) >= 2, "the same id was asked for again");
		tokio::time::sleep(Duration::from_millis(150)).await;
		assert!(
			crate::session::inbox::drain_inbox_batch().is_empty(),
			"one result, not more"
		);
	})
	.await;

	responder.abort();
	disconnect(&name);
}

/// A success in between starts the budget over: a task whose polls briefly went
/// unanswered and then answered again is tracked to its real completion,
/// however long the outage before it had run.
#[serial]
#[tokio::test]
async fn a_recovered_task_clears_its_outage_and_still_completes() {
	let name = unique_server("task-blip");
	let peer = serve_in_memory(&name);
	register(&name, peer.service);
	let server = stdin_config(&name, "unused", Vec::new(), 1);

	let polls = StdArc::new(AtomicUsize::new(0));
	let poll_count = StdArc::clone(&polls);
	let mut outgoing = peer.outgoing;
	let incoming = peer.incoming;
	let responder = tokio::spawn(async move {
		while let Some(message) = outgoing.next().await {
			if let JsonRpcMessage::Request(request) = message {
				match request.request {
					ClientRequest::CallToolRequest(_) => incoming
						.unbounded_send(JsonRpcMessage::response(
							ServerResult::CreateTaskResult(CreateTaskResult::new(working_task("task-blip"))),
							request.id,
						))
						.expect("fake server channel must stay open"),
					ClientRequest::GetTaskRequest(poll) => {
						assert_eq!(poll.params.task_id, "task-blip");
						let response = match poll_count.fetch_add(1, Ordering::SeqCst) {
							// Never answered: the poll times out and counts as a miss.
							0 | 2 => continue,
							// Held back long enough for the test to mark the first
							// outage as run out before this success clears it.
							1 => {
								tokio::time::sleep(Duration::from_millis(300)).await;
								JsonRpcMessage::response(
									ServerResult::GetTaskResult(GetTaskResult::new(DetailedTask::new(
										working_task("task-blip"),
										TaskPayload::Working,
									))),
									request.id,
								)
							}
							_ => JsonRpcMessage::response(completed_task("task-blip", "done after a blip"), request.id),
						};
						incoming
							.unbounded_send(response)
							.expect("fake server channel must stay open");
					}
					other => panic!("nothing else is expected, got {other:?}"),
				}
			}
		}
	});

	let session_id = format!("cov-task-blip-{}", uuid::Uuid::new_v4());
	crate::session::context::with_session_id(session_id.clone(), async {
		crate::session::inbox::init_inbox_for_session();
		tokio::time::timeout(WAIT, call_tool(&server, &tool_call("blip-1"), None))
			.await
			.expect("a session task must not hold the call")
			.expect("the call must hand back a notice");
		let key = crate::session::shell_jobs::task_key("blip-1");
		tokio::time::timeout(WAIT, async {
			while !crate::session::shell_jobs::exhaust_outage_for_tests(&session_id, &key) {
				tokio::time::sleep(Duration::from_millis(10)).await;
			}
		})
		.await
		.expect("the first miss is recorded");

		// The success clears the outage; the next miss starts a fresh budget that
		// has not run out, so the task completes for real.
		let batch = next_inbox_batch().await;
		assert_eq!(batch.len(), 1, "{batch:?}");
		assert!(
			batch[0]
				.content
				.starts_with("<background_job task=\"mcp-task:blip-1\">\ndone after a blip"),
			"{}",
			batch[0].content
		);
		assert!(polls.load(Ordering::SeqCst) >= 4);
		assert_eq!(
			crate::session::shell_jobs::take_completed_for_session(&session_id)[0].body,
			"done after a blip"
		);
	})
	.await;

	responder.abort();
	disconnect(&name);
}

/// A server that answers `tasks/get` with "invalid params" has forgotten the
/// task. The tracking ends at once — no outage budget to wait out — with one
/// result saying the outcome is unknown, and nothing is cancelled.
#[serial]
#[tokio::test]
async fn a_background_task_its_server_forgot_is_reported_lost_at_once() {
	let name = unique_server("task-forgotten");
	let peer = serve_in_memory(&name);
	register(&name, peer.service);
	let server = stdin_config(&name, "unused", Vec::new(), 5);

	let cancels = StdArc::new(AtomicUsize::new(0));
	let cancel_count = StdArc::clone(&cancels);
	let mut outgoing = peer.outgoing;
	let incoming = peer.incoming;
	let responder = tokio::spawn(async move {
		while let Some(message) = outgoing.next().await {
			if let JsonRpcMessage::Request(request) = message {
				let response = match request.request {
					ClientRequest::CallToolRequest(_) => JsonRpcMessage::response(
						ServerResult::CreateTaskResult(CreateTaskResult::new(working_task("task-forgotten"))),
						request.id,
					),
					ClientRequest::GetTaskRequest(_) => JsonRpcMessage::error(
						rmcp::ErrorData::invalid_params("unknown task: task-forgotten", None),
						Some(request.id),
					),
					ClientRequest::CancelTaskRequest(_) => {
						cancel_count.fetch_add(1, Ordering::SeqCst);
						JsonRpcMessage::response(ServerResult::task_ack(()), request.id)
					}
					other => panic!("unexpected request {other:?}"),
				};
				incoming
					.unbounded_send(response)
					.expect("fake server channel must stay open");
			}
		}
	});

	let session_id = format!("cov-task-forgotten-{}", uuid::Uuid::new_v4());
	crate::session::context::with_session_id(session_id.clone(), async {
		crate::session::inbox::init_inbox_for_session();
		tokio::time::timeout(WAIT, call_tool(&server, &tool_call("forgotten-1"), None))
			.await
			.expect("a session task must not hold the call")
			.expect("the call must hand back a notice");

		let batch = next_inbox_batch().await;
		assert_eq!(batch.len(), 1, "{batch:?}");
		let content = &batch[0].content;
		assert!(content.contains("is_error=\"true\""), "{content}");
		assert!(
			content.contains("lost track") && content.contains("no longer knows it"),
			"{content}"
		);
		assert!(!crate::session::shell_jobs::has_pending_for_session(&session_id));
		assert_eq!(cancels.load(Ordering::SeqCst), 0, "nothing is cancelled");
		tokio::time::sleep(Duration::from_millis(150)).await;
		assert!(
			crate::session::inbox::drain_inbox_batch().is_empty(),
			"one result, not more"
		);
	})
	.await;

	responder.abort();
	disconnect(&name);
}

/// Dropping the registry entry — what a completion, a replacement or a lost
/// tracking does — only ends the polling: the task is neither cancelled nor
/// delivered for. Only an explicit stop or teardown cancels it.
#[serial]
#[tokio::test]
async fn dropping_the_observer_ends_the_polling_without_cancelling_the_task() {
	let name = unique_server("task-observer-drop");
	let peer = serve_in_memory(&name);
	register(&name, peer.service);
	let server = stdin_config(&name, "unused", Vec::new(), 5);

	let polls = StdArc::new(AtomicUsize::new(0));
	let cancels = StdArc::new(AtomicUsize::new(0));
	let counter = StdArc::clone(&polls);
	let cancel_count = StdArc::clone(&cancels);
	let responder = spawn_responder(peer.outgoing, peer.incoming, move |request| match request {
		ClientRequest::CallToolRequest(_) => Some(ServerResult::CreateTaskResult(CreateTaskResult::new(working_task(
			"task-orphan",
		)))),
		ClientRequest::GetTaskRequest(_) => {
			counter.fetch_add(1, Ordering::SeqCst);
			Some(ServerResult::GetTaskResult(GetTaskResult::new(DetailedTask::new(
				working_task("task-orphan"),
				TaskPayload::Working,
			))))
		}
		ClientRequest::CancelTaskRequest(_) => {
			cancel_count.fetch_add(1, Ordering::SeqCst);
			Some(ServerResult::task_ack(()))
		}
		_ => None,
	});

	let session_id = format!("cov-task-orphan-{}", uuid::Uuid::new_v4());
	crate::session::context::with_session_id(session_id.clone(), async {
		crate::session::inbox::init_inbox_for_session();
		tokio::time::timeout(WAIT, call_tool(&server, &tool_call("orphan-1"), None))
			.await
			.expect("a session task must not hold the call")
			.expect("the call must hand back a notice");
		let key = crate::session::shell_jobs::task_key("orphan-1");
		tokio::time::timeout(WAIT, async {
			while polls.load(Ordering::SeqCst) == 0 {
				tokio::time::sleep(Duration::from_millis(10)).await;
			}
		})
		.await
		.expect("the task must be polled");
		assert!(
			crate::session::shell_jobs::complete_for_session(&session_id, &key, "gone"),
			"the entry is dropped the way a completion drops it"
		);
		tokio::time::sleep(Duration::from_millis(200)).await;
		let settled = polls.load(Ordering::SeqCst);
		tokio::time::sleep(Duration::from_millis(150)).await;
		assert_eq!(polls.load(Ordering::SeqCst), settled, "the poller ended with its entry");
		assert_eq!(cancels.load(Ordering::SeqCst), 0, "a dropped entry cancels nothing");
		assert!(
			crate::session::inbox::drain_inbox_batch().is_empty(),
			"and delivers nothing"
		);
	})
	.await;

	responder.abort();
	disconnect(&name);
}

/// A signalled update whose read fails is retried, not delivered as the job's
/// outcome: the job stays pending, and the read that then succeeds delivers
/// the output once.
#[serial]
#[tokio::test]
async fn a_resource_update_whose_read_fails_is_retried_not_delivered_as_the_outcome() {
	let name = unique_server("watch-read-retry");
	let peer = serve_in_memory(&name);
	register(&name, peer.service);
	let server = stdin_config(&name, "unused", Vec::new(), 5);
	let linked = result_with_resource_link("job://retry-1");
	let uri = "job://retry-1".to_string();

	crate::session::context::with_session_id(format!("cov-watch-retry-{}", uuid::Uuid::new_v4()), async {
		crate::session::inbox::init_inbox_for_session();
		crate::session::shell_jobs::note_watched_from_server_config(&server, &linked);
		let session_id = crate::session::context::current_session_id()
			.expect("session id")
			.to_string();
		let mut outgoing = peer.outgoing;
		let incoming = peer.incoming;
		let reads = StdArc::new(AtomicUsize::new(0));
		let read_count = StdArc::clone(&reads);
		let watched_uri = uri.clone();
		let responder_incoming = incoming.clone();
		let responder = tokio::spawn(async move {
			while let Some(message) = outgoing.next().await {
				if let JsonRpcMessage::Request(request) = message {
					if let ClientRequest::ReadResourceRequest(poll) = request.request {
						assert_eq!(poll.params.uri, watched_uri, "the same resource is re-read");
						let response = if read_count.fetch_add(1, Ordering::SeqCst) == 0 {
							JsonRpcMessage::error(rmcp::ErrorData::internal_error("boom", None), Some(request.id))
						} else {
							JsonRpcMessage::response(text_result(&watched_uri, "job output body"), request.id)
						};
						responder_incoming
							.unbounded_send(response)
							.expect("fake server channel must stay open");
					}
				}
			}
		});
		push_resource_update(&incoming, &uri);
		tokio::time::sleep(Duration::from_millis(100)).await;
		assert_eq!(reads.load(Ordering::SeqCst), 1, "one bounded read, then backoff");
		assert!(
			crate::session::shell_jobs::has_pending_for_session(&session_id),
			"a failed read is not the job's outcome"
		);
		assert!(crate::session::inbox::drain_inbox_batch().is_empty());

		let batch = next_inbox_batch().await;
		assert_eq!(batch.len(), 1, "{batch:?}");
		assert_eq!(
			batch[0].content,
			crate::session::shell_jobs::delivery_message(
				"<background_job resource=\"job://retry-1\">",
				"job output body"
			)
		);
		assert_eq!(reads.load(Ordering::SeqCst), 2);
		assert!(!crate::session::shell_jobs::has_pending_for_session(&session_id));
		responder.abort();
	})
	.await;

	disconnect(&name);
}

/// A read that keeps failing for the whole outage budget ends the tracking with
/// one result saying the outcome is unknown — never the read error passed off
/// as the job's output, and nothing delivered twice.
#[serial]
#[tokio::test]
async fn a_resource_update_nobody_could_read_for_the_whole_budget_is_reported_lost_once() {
	let name = unique_server("watch-read-lost");
	let peer = serve_in_memory(&name);
	register(&name, peer.service);
	let server = stdin_config(&name, "unused", Vec::new(), 5);
	let linked = result_with_resource_link("job://gone-1");
	let uri = "job://gone-1".to_string();

	crate::session::context::with_session_id(format!("cov-watch-lost-{}", uuid::Uuid::new_v4()), async {
		crate::session::inbox::init_inbox_for_session();
		crate::session::shell_jobs::note_watched_from_server_config(&server, &linked);
		let session_id = crate::session::context::current_session_id()
			.expect("session id")
			.to_string();
		let mut outgoing = peer.outgoing;
		let incoming = peer.incoming;
		let reads = StdArc::new(AtomicUsize::new(0));
		let read_count = StdArc::clone(&reads);
		let responder_incoming = incoming.clone();
		let responder = tokio::spawn(async move {
			while let Some(message) = outgoing.next().await {
				if let JsonRpcMessage::Request(request) = message {
					if let ClientRequest::ReadResourceRequest(_) = request.request {
						read_count.fetch_add(1, Ordering::SeqCst);
						responder_incoming
							.unbounded_send(JsonRpcMessage::error(
								rmcp::ErrorData::internal_error("boom", None),
								Some(request.id),
							))
							.expect("fake server channel must stay open");
					}
				}
			}
		});
		push_resource_update(&incoming, &uri);
		// The first failed read starts the budget; mark it as run out, as two
		// hours of failed reads would.
		tokio::time::timeout(WAIT, async {
			while !crate::session::shell_jobs::exhaust_outage_for_tests(&session_id, &uri) {
				tokio::time::sleep(Duration::from_millis(10)).await;
			}
		})
		.await
		.expect("the failed read is recorded");

		let batch = next_inbox_batch().await;
		let content = &batch[0].content;
		assert!(
			content.starts_with(&format!(
				"<background_job resource=\"{uri}\"{}>",
				crate::session::shell_jobs::DELIVERY_ERROR_ATTR
			)),
			"{content}"
		);
		assert!(
			content.contains("lost track") && content.contains("outcome is unknown"),
			"{content}"
		);
		assert_eq!(reads.load(Ordering::SeqCst), 2, "retried on backoff, then lost");
		assert!(!crate::session::shell_jobs::has_pending_for_session(&session_id));
		tokio::time::sleep(Duration::from_millis(100)).await;
		assert!(
			crate::session::inbox::drain_inbox_batch().is_empty(),
			"one result, not more"
		);
		responder.abort();
	})
	.await;

	disconnect(&name);
}

/// A server that answers the read with "resource not found" has forgotten the
/// job. The tracking ends at once — no outage budget to wait out — with one
/// result saying the outcome is unknown, and the resource is no longer watched.
#[serial]
#[tokio::test]
async fn a_resource_update_its_server_no_longer_knows_is_reported_lost_at_once() {
	let name = unique_server("watch-read-forgotten");
	let peer = serve_in_memory(&name);
	register(&name, peer.service);
	let server = stdin_config(&name, "unused", Vec::new(), 5);
	let linked = result_with_resource_link("job://forgotten-1");
	let uri = "job://forgotten-1".to_string();

	crate::session::context::with_session_id(format!("cov-watch-forgotten-{}", uuid::Uuid::new_v4()), async {
		crate::session::inbox::init_inbox_for_session();
		crate::session::shell_jobs::note_watched_from_server_config(&server, &linked);
		let session_id = crate::session::context::current_session_id()
			.expect("session id")
			.to_string();
		let mut outgoing = peer.outgoing;
		let incoming = peer.incoming;
		let responder_incoming = incoming.clone();
		let responder = tokio::spawn(async move {
			while let Some(message) = outgoing.next().await {
				if let JsonRpcMessage::Request(request) = message {
					if let ClientRequest::ReadResourceRequest(_) = request.request {
						responder_incoming
							.unbounded_send(JsonRpcMessage::error(
								rmcp::ErrorData::resource_not_found("no such job", None),
								Some(request.id),
							))
							.expect("fake server channel must stay open");
					}
				}
			}
		});
		push_resource_update(&incoming, &uri);

		let batch = next_inbox_batch().await;
		assert_eq!(batch.len(), 1, "{batch:?}");
		let content = &batch[0].content;
		assert!(
			content.starts_with(&format!(
				"<background_job resource=\"{uri}\"{}>",
				crate::session::shell_jobs::DELIVERY_ERROR_ATTR
			)),
			"{content}"
		);
		assert!(
			content.contains("lost track") && content.contains("no longer knows it"),
			"{content}"
		);
		assert!(!crate::session::shell_jobs::has_pending_for_session(&session_id));
		assert!(!crate::session::shell_jobs::is_watched_for_session(&session_id, &uri));
		tokio::time::sleep(Duration::from_millis(100)).await;
		assert!(
			crate::session::inbox::drain_inbox_batch().is_empty(),
			"one result, not more"
		);
		responder.abort();
	})
	.await;

	disconnect(&name);
}

/// A connection that closes is not the job's end: the signalled read that
/// failed on it is retried over a connection reopened to the same server — the
/// one the job was registered with — and the same resource delivers once.
/// Nothing else is asked of either connection.
#[serial]
#[tokio::test]
async fn a_resource_update_survives_a_closed_connection_and_delivers_once() {
	let name = unique_server("watch-reconnect");
	let first = serve_in_memory(&name);
	register(&name, first.service);
	let server = stdin_config(&name, "unused", Vec::new(), 5);
	let linked = result_with_resource_link("job://reconnect-1");
	let uri = "job://reconnect-1".to_string();

	crate::session::context::with_session_id(format!("cov-watch-reconnect-{}", uuid::Uuid::new_v4()), async {
		crate::session::inbox::init_inbox_for_session();
		crate::session::shell_jobs::note_watched_from_server_config(&server, &linked);
		let session_id = crate::session::context::current_session_id()
			.expect("session id")
			.to_string();

		// The first connection fails the read, then goes away with its transport.
		let reads_first = StdArc::new(AtomicUsize::new(0));
		let count = StdArc::clone(&reads_first);
		let watched_uri = uri.clone();
		let push = first.incoming.clone();
		let first_responder = tokio::spawn(async move {
			let mut outgoing = first.outgoing;
			while let Some(message) = outgoing.next().await {
				if let JsonRpcMessage::Request(request) = message {
					if let ClientRequest::ReadResourceRequest(poll) = request.request {
						assert_eq!(poll.params.uri, watched_uri, "the same resource is read");
						count.fetch_add(1, Ordering::SeqCst);
						first
							.incoming
							.unbounded_send(JsonRpcMessage::error(
								rmcp::ErrorData::internal_error("connection closing", None),
								Some(request.id),
							))
							.expect("fake server channel must stay open");
					}
				}
			}
		});
		push_resource_update(&push, &uri);
		tokio::time::timeout(WAIT, async {
			while reads_first.load(Ordering::SeqCst) == 0 {
				tokio::time::sleep(Duration::from_millis(10)).await;
			}
		})
		.await
		.expect("the read is attempted on the open connection");
		assert!(
			crate::session::inbox::drain_inbox_batch().is_empty(),
			"a failed read is not the outcome"
		);

		// The server is back on a new connection under the same name.
		let second = serve_in_memory(&name);
		register(&name, second.service);
		let reads_second = StdArc::new(AtomicUsize::new(0));
		let count = StdArc::clone(&reads_second);
		let second_responder = tokio::spawn(async move {
			let mut outgoing = second.outgoing;
			while let Some(message) = outgoing.next().await {
				if let JsonRpcMessage::Request(request) = message {
					match request.request {
						ClientRequest::ReadResourceRequest(poll) => {
							assert_eq!(poll.params.uri, "job://reconnect-1", "the same resource is re-read");
							count.fetch_add(1, Ordering::SeqCst);
							second
								.incoming
								.unbounded_send(JsonRpcMessage::response(
									text_result(&poll.params.uri, "job output after reconnect"),
									request.id,
								))
								.expect("fake server channel must stay open");
						}
						other => panic!("no other request reaches the reconnected server: {other:?}"),
					}
				}
			}
		});
		// Dropping the first responder drops the first connection's transport.
		first_responder.abort();

		let batch = next_inbox_batch().await;
		assert_eq!(batch.len(), 1, "{batch:?}");
		assert_eq!(
			batch[0].content,
			crate::session::shell_jobs::delivery_message(
				"<background_job resource=\"job://reconnect-1\">",
				"job output after reconnect"
			)
		);
		assert_eq!(
			reads_second.load(Ordering::SeqCst),
			1,
			"one read on the reopened connection"
		);
		assert_eq!(reads_first.load(Ordering::SeqCst), 1, "no more on the closed one");
		assert!(!crate::session::shell_jobs::has_pending_for_session(&session_id));
		assert_eq!(
			crate::session::shell_jobs::take_completed_for_session(&session_id).len(),
			1
		);
		tokio::time::sleep(Duration::from_millis(200)).await;
		assert!(
			crate::session::inbox::drain_inbox_batch().is_empty(),
			"one result, not more"
		);
		second_responder.abort();
	})
	.await;

	disconnect(&name);
}

/// A waiter whose registration is replaced while it backs off stops: the job
/// launched again under the same URI keeps its own tracking and outcome, and
/// the stale waiter neither reads again nor reconnects nor delivers.
#[serial]
#[tokio::test]
async fn a_resource_waiter_stops_once_its_registration_is_replaced() {
	let name = unique_server("watch-replaced-waiter");
	let mut peer = serve_in_memory(&name);
	register(&name, peer.service);
	let server = stdin_config(&name, "unused", Vec::new(), 5);
	let linked = result_with_resource_link("job://replaced-1");
	let uri = "job://replaced-1".to_string();

	crate::session::context::with_session_id(format!("cov-watch-replaced-{}", uuid::Uuid::new_v4()), async {
		crate::session::inbox::init_inbox_for_session();
		crate::session::shell_jobs::note_watched_from_server_config(&server, &linked);
		let session_id = crate::session::context::current_session_id()
			.expect("session id")
			.to_string();

		let reads = StdArc::new(AtomicUsize::new(0));
		let count = StdArc::clone(&reads);
		let push = peer.incoming.clone();
		let responder = tokio::spawn(async move {
			while let Some(message) = peer.outgoing.next().await {
				if let JsonRpcMessage::Request(request) = message {
					if let ClientRequest::ReadResourceRequest(_) = request.request {
						count.fetch_add(1, Ordering::SeqCst);
						peer.incoming
							.unbounded_send(JsonRpcMessage::error(
								rmcp::ErrorData::internal_error("boom", None),
								Some(request.id),
							))
							.expect("fake server channel must stay open");
					}
				}
			}
		});
		push_resource_update(&push, &uri);
		tokio::time::timeout(WAIT, async {
			while reads.load(Ordering::SeqCst) == 0 {
				tokio::time::sleep(Duration::from_millis(10)).await;
			}
		})
		.await
		.expect("the first read is attempted");

		// The job is launched again under the same URI while the waiter backs off.
		crate::session::shell_jobs::register_for_session(&session_id, &name, &uri, "shell: cargo build");
		// The backoff's one retry finds its registration gone and ends the wait
		// before it reads or reconnects again.
		tokio::time::sleep(Duration::from_millis(4000)).await;
		assert_eq!(
			reads.load(Ordering::SeqCst),
			1,
			"the stale waiter ends without reading again"
		);
		assert!(
			crate::session::inbox::drain_inbox_batch().is_empty(),
			"nothing is delivered for the replaced registration"
		);
		assert!(
			crate::session::shell_jobs::is_watched_for_session(&session_id, &uri),
			"the replacement is tracked"
		);
		let pending = crate::session::shell_jobs::pending_resources_for_session(&session_id);
		assert!(!pending[0].delivering, "and not claimed by the stale waiter");
		assert_eq!(pending[0].label, "shell: cargo build");
		responder.abort();
	})
	.await;

	disconnect(&name);
}

/// The watcher's stream lives on one connection. When that connection goes, the
/// watcher re-arms on the reopened one and reads the resource once more — as a
/// status, since no signal said the job ended — then the signal on the new
/// stream delivers exactly once.
#[serial]
#[tokio::test]
async fn a_watcher_re_arms_on_a_reopened_connection_and_delivers_the_signal_once() {
	let name = unique_server("watch-rearm");
	let first = serve_in_memory(&name);
	register(&name, first.service);
	let server = stdin_config(&name, "unused", Vec::new(), 5);
	let linked = result_with_resource_link("job://rearm-1");
	let uri = "job://rearm-1".to_string();

	crate::session::context::with_session_id(format!("cov-watch-rearm-{}", uuid::Uuid::new_v4()), async {
		crate::session::inbox::init_inbox_for_session();
		crate::session::shell_jobs::note_watched_from_server_config(&server, &linked);
		let mut events = crate::session::shell_jobs::subscribe_events();

		// First connection: the stream is acknowledged, then its transport goes.
		let listens_first = StdArc::new(AtomicUsize::new(0));
		let count = StdArc::clone(&listens_first);
		let first_responder = tokio::spawn(async move {
			use rmcp::model::GetMeta;
			let mut outgoing = first.outgoing;
			while let Some(message) = outgoing.next().await {
				if let JsonRpcMessage::Request(request) = message {
					if let ClientRequest::SubscriptionsListenRequest(_) = request.request {
						count.fetch_add(1, Ordering::SeqCst);
						let mut filter = SubscriptionFilter::new();
						filter.resource_subscriptions = Some(vec!["job://rearm-1".to_string()]);
						let mut ack = ServerNotification::SubscriptionsAcknowledgedNotification(Notification::new(
							SubscriptionsAcknowledgedNotificationParams::new(filter),
						));
						ack.get_meta_mut().set_subscription_id(request.id.clone());
						first
							.incoming
							.unbounded_send(JsonRpcMessage::notification(ack))
							.expect("fake server channel must stay open");
					}
				}
			}
		});
		// Establish the watcher.
		tokio::time::timeout(WAIT, watch_resource_links(&server, &linked))
			.await
			.expect("watch setup must not hang");
		assert_eq!(listens_first.load(Ordering::SeqCst), 1);

		// The server is back on a new connection: it acknowledges a new stream
		// and answers the re-read, then the signal's read.
		let second = serve_in_memory(&name);
		register(&name, second.service);
		let listens_second = StdArc::new(AtomicUsize::new(0));
		let listen_count = StdArc::clone(&listens_second);
		let reads_second = StdArc::new(AtomicUsize::new(0));
		let read_count = StdArc::clone(&reads_second);
		let listen_id: StdArc<StdMutex<Option<RequestId>>> = StdArc::new(StdMutex::new(None));
		let seen_id = StdArc::clone(&listen_id);
		let push_second = second.incoming.clone();
		let second_responder = tokio::spawn(async move {
			use rmcp::model::GetMeta;
			let mut outgoing = second.outgoing;
			while let Some(message) = outgoing.next().await {
				if let JsonRpcMessage::Request(request) = message {
					match request.request {
						ClientRequest::SubscriptionsListenRequest(_) => {
							listen_count.fetch_add(1, Ordering::SeqCst);
							let mut filter = SubscriptionFilter::new();
							filter.resource_subscriptions = Some(vec!["job://rearm-1".to_string()]);
							let mut ack = ServerNotification::SubscriptionsAcknowledgedNotification(Notification::new(
								SubscriptionsAcknowledgedNotificationParams::new(filter),
							));
							ack.get_meta_mut().set_subscription_id(request.id.clone());
							seen_id.lock().expect("listen id lock").replace(request.id.clone());
							second
								.incoming
								.unbounded_send(JsonRpcMessage::notification(ack))
								.expect("fake server channel must stay open");
						}
						ClientRequest::ReadResourceRequest(poll) => {
							// The first read is the re-arm's status probe; the
							// second, the signal's delivery.
							let text = if read_count.fetch_add(1, Ordering::SeqCst) == 0 {
								"status: running, half done"
							} else {
								"status: exited with code 0"
							};
							second
								.incoming
								.unbounded_send(JsonRpcMessage::response(
									text_result(&poll.params.uri, text),
									request.id,
								))
								.expect("fake server channel must stay open");
						}
						other => panic!("no other request reaches the reconnected server: {other:?}"),
					}
				}
			}
		});
		// Dropping the first responder drops the first connection's transport.
		first_responder.abort();

		tokio::time::timeout(WAIT, async {
			while listens_second.load(Ordering::SeqCst) == 0 || reads_second.load(Ordering::SeqCst) == 0 {
				tokio::time::sleep(Duration::from_millis(10)).await;
			}
		})
		.await
		.expect("the watcher re-arms and re-reads");
		tokio::time::sleep(Duration::from_millis(200)).await;
		assert_eq!(listens_second.load(Ordering::SeqCst), 1, "one stream at a time");
		assert_eq!(
			reads_second.load(Ordering::SeqCst),
			1,
			"the re-read is a status, not a delivery"
		);
		assert!(
			crate::session::inbox::drain_inbox_batch().is_empty(),
			"no signal, nothing delivered"
		);
		let session_id = crate::session::context::current_session_id()
			.expect("session id")
			.to_string();
		assert!(crate::session::shell_jobs::is_watched_for_session(&session_id, &uri));
		assert_eq!(
			crate::session::shell_jobs::pending_resources_for_session(&session_id)[0]
				.status
				.as_deref(),
			Some("status: running, half done")
		);

		// The signal on the new stream delivers once.
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
		push_second
			.unbounded_send(JsonRpcMessage::notification(update))
			.expect("fake server channel must stay open");

		let event = tokio::time::timeout(WAIT, events.recv())
			.await
			.expect("update must be delivered within the wait")
			.expect("watch event channel must stay open");
		assert!(
			matches!(&event, crate::session::shell_jobs::WatchEvent::Completed { uri: u, .. } if *u == uri),
			"{event:?}"
		);
		let batch = next_inbox_batch().await;
		assert_eq!(batch.len(), 1, "{batch:?}");
		assert_eq!(
			batch[0].content,
			crate::session::shell_jobs::delivery_message(
				"<background_job resource=\"job://rearm-1\">",
				"status: exited with code 0"
			)
		);
		assert_eq!(reads_second.load(Ordering::SeqCst), 2);
		tokio::time::sleep(Duration::from_millis(200)).await;
		assert!(
			crate::session::inbox::drain_inbox_batch().is_empty(),
			"one result, not more"
		);
		second_responder.abort();
	})
	.await;

	disconnect(&name);
}

/// A `subscriptions/listen` that fails on the reopened connection does not end
/// the watcher: it reads the job, tries again after its first backoff (1s), and
/// the exit the server replays on the stream that finally opens is delivered
/// exactly once.
#[serial]
#[tokio::test]
async fn a_watcher_retries_a_failed_listen_when_re_arming_and_delivers_the_replayed_exit_once() {
	let name = unique_server("watch-rearm-retry");
	let first = serve_in_memory(&name);
	register(&name, first.service);
	let server = stdin_config(&name, "unused", Vec::new(), 5);
	let linked = result_with_resource_link("job://rearm-retry-1");
	let uri = "job://rearm-retry-1".to_string();

	crate::session::context::with_session_id(format!("cov-watch-rearm-retry-{}", uuid::Uuid::new_v4()), async {
		crate::session::inbox::init_inbox_for_session();
		crate::session::shell_jobs::note_watched_from_server_config(&server, &linked);

		// First connection: the stream is acknowledged, then its transport goes.
		let first_responder = tokio::spawn(async move {
			use rmcp::model::GetMeta;
			let mut outgoing = first.outgoing;
			while let Some(message) = outgoing.next().await {
				if let JsonRpcMessage::Request(request) = message {
					if let ClientRequest::SubscriptionsListenRequest(_) = request.request {
						let mut filter = SubscriptionFilter::new();
						filter.resource_subscriptions = Some(vec!["job://rearm-retry-1".to_string()]);
						let mut ack = ServerNotification::SubscriptionsAcknowledgedNotification(Notification::new(
							SubscriptionsAcknowledgedNotificationParams::new(filter),
						));
						ack.get_meta_mut().set_subscription_id(request.id.clone());
						first
							.incoming
							.unbounded_send(JsonRpcMessage::notification(ack))
							.expect("fake server channel must stay open");
					}
				}
			}
		});
		tokio::time::timeout(WAIT, watch_resource_links(&server, &linked))
			.await
			.expect("watch setup must not hang");

		// The reopened connection refuses the first stream, then opens the next
		// one and replays the exit on it. Reads always succeed.
		let second = serve_in_memory(&name);
		register(&name, second.service);
		let listens = StdArc::new(AtomicUsize::new(0));
		let listen_count = StdArc::clone(&listens);
		let reads = StdArc::new(AtomicUsize::new(0));
		let read_count = StdArc::clone(&reads);
		let second_responder = tokio::spawn(async move {
			use rmcp::model::GetMeta;
			let mut outgoing = second.outgoing;
			while let Some(message) = outgoing.next().await {
				if let JsonRpcMessage::Request(request) = message {
					match request.request {
						ClientRequest::SubscriptionsListenRequest(_) => {
							if listen_count.fetch_add(1, Ordering::SeqCst) == 0 {
								second
									.incoming
									.unbounded_send(JsonRpcMessage::error(
										rmcp::ErrorData::internal_error("streams unavailable", None),
										Some(request.id),
									))
									.expect("fake server channel must stay open");
								continue;
							}
							let mut filter = SubscriptionFilter::new();
							filter.resource_subscriptions = Some(vec!["job://rearm-retry-1".to_string()]);
							let mut ack = ServerNotification::SubscriptionsAcknowledgedNotification(Notification::new(
								SubscriptionsAcknowledgedNotificationParams::new(filter),
							));
							ack.get_meta_mut().set_subscription_id(request.id.clone());
							second
								.incoming
								.unbounded_send(JsonRpcMessage::notification(ack))
								.expect("fake server channel must stay open");
							let mut replay = ServerNotification::ResourceUpdatedNotification(Notification::new(
								ResourceUpdatedNotificationParam::new("job://rearm-retry-1".to_string()),
							));
							replay.get_meta_mut().set_subscription_id(request.id.clone());
							second
								.incoming
								.unbounded_send(JsonRpcMessage::notification(replay))
								.expect("fake server channel must stay open");
						}
						ClientRequest::ReadResourceRequest(poll) => {
							// The first two reads are the re-arm's status probes
							// (one per attempt); the third is the signal's delivery.
							let text = if read_count.fetch_add(1, Ordering::SeqCst) < 2 {
								"status: running, half done"
							} else {
								"status: exited with code 0"
							};
							second
								.incoming
								.unbounded_send(JsonRpcMessage::response(
									text_result(&poll.params.uri, text),
									request.id,
								))
								.expect("fake server channel must stay open");
						}
						other => panic!("no other request reaches the reconnected server: {other:?}"),
					}
				}
			}
		});
		// Dropping the first responder drops the first connection's transport.
		first_responder.abort();

		let batch = next_inbox_batch().await;
		assert_eq!(batch.len(), 1, "{batch:?}");
		assert_eq!(
			batch[0].content,
			crate::session::shell_jobs::delivery_message(
				"<background_job resource=\"job://rearm-retry-1\">",
				"status: exited with code 0"
			)
		);
		assert_eq!(listens.load(Ordering::SeqCst), 2, "the failed stream was retried once");
		assert_eq!(reads.load(Ordering::SeqCst), 3, "two status probes, one delivery");
		tokio::time::sleep(Duration::from_millis(200)).await;
		assert!(
			crate::session::inbox::drain_inbox_batch().is_empty(),
			"one result, not more"
		);
		second_responder.abort();
	})
	.await;

	disconnect(&name);
}

/// The reconcile pass reports a resource's status once per distinct reading —
/// the first, then only a changed one, never the same again for the clock's
/// sake — over the live connection to its server, and nothing else is asked of
/// the server.
#[serial]
#[tokio::test]
async fn a_resource_status_reaches_the_model_once_per_distinct_reading() {
	let name = unique_server("watch-status-dedup");
	let mut peer = serve_in_memory(&name);
	register(&name, peer.service);
	let server = stdin_config(&name, "unused", Vec::new(), 5);
	let linked = result_with_resource_link("job://status-1");

	crate::session::context::with_session_id(format!("cov-watch-status-{}", uuid::Uuid::new_v4()), async {
		crate::session::inbox::init_inbox_for_session();
		crate::session::shell_jobs::note_watched_from_server_config(&server, &linked);
		let session_id = crate::session::context::current_session_id()
			.expect("session id")
			.to_string();

		let reads = StdArc::new(AtomicUsize::new(0));
		let count = StdArc::clone(&reads);
		let responder = tokio::spawn(async move {
			while let Some(message) = peer.outgoing.next().await {
				if let JsonRpcMessage::Request(request) = message {
					if let ClientRequest::ReadResourceRequest(poll) = request.request {
						// Quiet, quiet, then progress.
						let text = if count.fetch_add(1, Ordering::SeqCst) < 2 {
							"status: running"
						} else {
							"status: running, 50%"
						};
						peer.incoming
							.unbounded_send(JsonRpcMessage::response(
								text_result(&poll.params.uri, text),
								request.id,
							))
							.expect("fake server channel must stay open");
					}
				}
			}
		});
		let mut reported_statuses = HashMap::new();
		let mut checked_jobs = std::collections::HashSet::new();
		for _ in 0..3 {
			crate::session::chat::session::reconcile_pending_background_jobs(&mut reported_statuses, &mut checked_jobs)
				.await;
		}
		assert_eq!(reads.load(Ordering::SeqCst), 3, "one read per window, no other request");
		let batch = crate::session::inbox::drain_inbox_batch();
		assert_eq!(batch.len(), 2, "the first reading and the changed one only: {batch:?}");
		assert!(
			batch[0].content.ends_with("\nstatus: running\n</background_job>"),
			"{}",
			batch[0].content
		);
		assert!(
			batch[1].content.contains("status: running, 50%"),
			"{}",
			batch[1].content
		);
		assert!(crate::session::shell_jobs::is_watched_for_session(
			&session_id,
			"job://status-1"
		));
		responder.abort();
	})
	.await;

	disconnect(&name);
}

/// A signal from another server claims nothing, whatever URI it names: the job
/// one server launched is read from that server alone, and only its own signal
/// delivers it.
#[serial]
#[tokio::test]
async fn a_signal_from_another_server_claims_nothing_for_the_same_uri() {
	let owner = unique_server("watch-owner");
	let owner_peer = serve_in_memory(&owner);
	register(&owner, owner_peer.service);
	let other = unique_server("watch-other");
	let other_peer = serve_in_memory(&other);
	register(&other, other_peer.service);
	let server = stdin_config(&owner, "unused", Vec::new(), 5);
	let linked = result_with_resource_link("job://shared");
	let uri = "job://shared".to_string();

	crate::session::context::with_session_id(format!("cov-watch-cross-server-{}", uuid::Uuid::new_v4()), async {
		crate::session::inbox::init_inbox_for_session();
		crate::session::shell_jobs::note_watched_from_server_config(&server, &linked);
		let session_id = crate::session::context::current_session_id()
			.expect("session id")
			.to_string();

		let other_reads = StdArc::new(AtomicUsize::new(0));
		let other_count = StdArc::clone(&other_reads);
		let mut other_outgoing = other_peer.outgoing;
		let other_responder = tokio::spawn(async move {
			while let Some(message) = other_outgoing.next().await {
				if let JsonRpcMessage::Request(_) = message {
					other_count.fetch_add(1, Ordering::SeqCst);
				}
			}
		});
		let owner_reads = StdArc::new(AtomicUsize::new(0));
		let owner_count = StdArc::clone(&owner_reads);
		let mut owner_outgoing = owner_peer.outgoing;
		let owner_reply = owner_peer.incoming.clone();
		let owner_responder = tokio::spawn(async move {
			while let Some(message) = owner_outgoing.next().await {
				if let JsonRpcMessage::Request(request) = message {
					if let ClientRequest::ReadResourceRequest(poll) = request.request {
						owner_count.fetch_add(1, Ordering::SeqCst);
						owner_reply
							.unbounded_send(JsonRpcMessage::response(
								text_result(&poll.params.uri, "done from the owner"),
								request.id,
							))
							.expect("fake server channel must stay open");
					}
				}
			}
		});

		// The other server signals the same URI: nobody reads anything, the job
		// stays tracked and unclaimed.
		let other_push = other_peer.incoming.clone();
		push_resource_update(&other_push, &uri);
		tokio::time::sleep(Duration::from_millis(300)).await;
		assert_eq!(
			other_reads.load(Ordering::SeqCst),
			0,
			"no reconnect to the signalling server"
		);
		assert_eq!(
			owner_reads.load(Ordering::SeqCst),
			0,
			"the owner is not read on another server's word"
		);
		assert!(crate::session::inbox::drain_inbox_batch().is_empty());
		assert!(crate::session::shell_jobs::is_watched_for_session(&session_id, &uri));
		assert!(!crate::session::shell_jobs::pending_resources_for_session(&session_id)[0].delivering);

		// The owner's own signal delivers once.
		let owner_push = owner_peer.incoming.clone();
		push_resource_update(&owner_push, &uri);
		let batch = next_inbox_batch().await;
		assert_eq!(batch.len(), 1, "{batch:?}");
		assert_eq!(
			batch[0].content,
			crate::session::shell_jobs::delivery_message(
				"<background_job resource=\"job://shared\">",
				"done from the owner"
			)
		);
		assert_eq!(owner_reads.load(Ordering::SeqCst), 1);
		assert_eq!(other_reads.load(Ordering::SeqCst), 0);
		other_responder.abort();
		owner_responder.abort();
	})
	.await;

	disconnect(&owner);
	disconnect(&other);
}

/// The server signals a resource update unsolicited — the legacy push path.
fn push_resource_update(incoming: &mpsc::UnboundedSender<RxJsonRpcMessage<RoleClient>>, uri: &str) {
	incoming
		.unbounded_send(JsonRpcMessage::notification(
			ServerNotification::ResourceUpdatedNotification(Notification::new(ResourceUpdatedNotificationParam::new(
				uri.to_string(),
			))),
		))
		.expect("fake server channel must stay open");
}

fn text_result(uri: &str, text: &str) -> ServerResult {
	ServerResult::ReadResourceResult(ReadResourceResult::new(vec![ResourceContents::TextResourceContents {
		uri: uri.to_string(),
		mime_type: Some("text/plain".to_string()),
		text: text.to_string(),
		meta: None,
	}]))
}

#[tokio::test]
async fn remote_skill_loads_verified_files_only_after_approval() {
	use crate::mcp::elicitation::{self, Answerer};
	use crate::session::context;
	use crate::websocket::ServerMessage;
	use rmcp::model::{
		ElicitResult, ElicitationAction, GetSkillResult, ListSkillsResult, ServerCapabilities, Skill, SkillFrontmatter,
		SkillResource, SkillResources, SKILLS_EXTENSION_ID,
	};
	use serde_json::json;

	const URI: &str = "skill://release/SKILL.md";
	const REFERENCE: &str = "skill://release/references/checklist.md";
	const SKILL: &str = "---\nname: release\ndescription: Publish a release\n---\nFollow the checklist.\n";
	const CHECKLIST: &str = "Check the release notes.";
	let name = unique_server("remote-skill");
	let config = stdin_config(&name, "unused-in-memory", Vec::new(), 5);
	let skill = Skill::new(
		URI,
		SkillFrontmatter::new("release", "Publish a release"),
		SkillResources::Files(vec![
			SkillResource::from_bytes(URI, SKILL.as_bytes()),
			SkillResource::from_bytes(REFERENCE, CHECKLIST.as_bytes()),
		]),
	);
	let mut capabilities = ServerCapabilities::default();
	capabilities.extensions = Some([(SKILLS_EXTENSION_ID.to_owned(), Default::default())].into());
	let (in_tx, in_rx) = mpsc::unbounded::<RxJsonRpcMessage<RoleClient>>();
	let (out_tx, out_rx) = mpsc::unbounded::<TxJsonRpcMessage<RoleClient>>();
	let service = rmcp::service::serve_directly(
		OctoClientHandler::new(&config),
		(out_tx, in_rx),
		Some(rmcp::model::ServerPeerInfo::new(
			ProtocolVersion::V_2026_07_28,
			capabilities,
		)),
	);
	register(&name, service);
	let reads = StdArc::new(AtomicUsize::new(0));
	let resource_reads = reads.clone();
	let corrupted = StdArc::new(AtomicBool::new(false));
	let tamper = corrupted.clone();
	let responder = spawn_responder(out_rx, in_tx, move |request| {
		Some(match request {
			ClientRequest::ListSkillsRequest(_) => {
				ServerResult::ListSkillsResult(ListSkillsResult::with_all_items(vec![skill.clone()]))
			}
			ClientRequest::GetSkillRequest(request) => {
				assert_eq!(request.params.uri, URI);
				ServerResult::GetSkillResult(GetSkillResult::new(skill.clone()))
			}
			ClientRequest::ReadResourceRequest(request) => {
				resource_reads.fetch_add(1, Ordering::SeqCst);
				let text = match request.params.uri.as_str() {
					URI => SKILL,
					REFERENCE if tamper.load(Ordering::SeqCst) => "Corrupted checklist",
					REFERENCE => CHECKLIST,
					uri => panic!("unexpected resource URI {uri}"),
				};
				ServerResult::ReadResourceResult(ReadResourceResult::new(vec![ResourceContents::text(
					text,
					request.params.uri,
				)]))
			}
			request => panic!("unexpected request {request:?}"),
		})
	});

	let id = format!("remote-skill-{}", uuid::Uuid::new_v4());
	super::super::tool_map::isolate_session_tool_map(&id);
	context::register_dynamic_server_for_session(&id, config);
	assert!(context::enable_dynamic_server_for_session(&id, &name, Vec::new()));
	let (sender, mut frames) = tokio::sync::mpsc::unbounded_channel();
	context::set_notification_sender_for_session(&id, sender);
	elicitation::set_answerer(&id, Some(Answerer::Client));
	let call = |parameters| McpToolCall {
		tool_name: "skill".into(),
		tool_id: "skill-call".into(),
		parameters,
	};
	let execute = crate::mcp::runtime::execute_skill_tool;
	let reference = format!("{name}:{URI}");

	context::with_session_id(id.clone(), async {
		let listed = execute(&call(json!({"action": "list", "pattern": &name})))
			.await
			.unwrap();
		assert!(listed.extract_content().contains(&reference));
		assert_eq!(reads.load(Ordering::SeqCst), 0);

		let load = call(json!({"action": "use", "name": URI}));
		let answer = async |approved, frames: &mut tokio::sync::mpsc::UnboundedReceiver<ServerMessage>| {
			let Some(ServerMessage::Elicitation(question)) = tokio::time::timeout(WAIT, frames.recv())
				.await
				.expect("expected skill approval")
			else {
				panic!("expected skill elicitation");
			};
			assert_eq!(question.server, name);
			assert!(elicitation::resolve(
				&id,
				&question.id,
				ElicitResult::new(ElicitationAction::Accept).with_content(json!({"approve": approved}))
			));
		};

		let (denied, ()) = tokio::join!(execute(&load), answer(false, &mut frames));
		assert!(denied.unwrap().is_error());
		assert_eq!(reads.load(Ordering::SeqCst), 0);

		let (loaded, ()) = tokio::join!(execute(&load), answer(true, &mut frames));
		let loaded = loaded.unwrap();
		assert!(!loaded.is_error());
		let content = loaded.extract_content();
		assert!(content.contains("Follow the checklist."));
		assert!(content.contains(REFERENCE));
		let manifest = content.lines().find_map(|line| line.strip_prefix("Manifest ")).unwrap();
		assert_eq!(reads.load(Ordering::SeqCst), 1);

		let supporting =
			call(json!({"action": "use", "name": &reference, "resource": REFERENCE, "manifest": manifest}));
		assert!(execute(&supporting)
			.await
			.unwrap()
			.extract_content()
			.contains(CHECKLIST));
		corrupted.store(true, Ordering::SeqCst);
		let failure = execute(&supporting).await.unwrap();
		assert!(failure.is_error());
		assert!(failure.extract_content().contains("verification failed"));
	})
	.await;
	context::cleanup_session(&id);
	disconnect(&name);
	responder.abort();
}

/// Runs a `tools/call` that returns a resource link, inside a session, against
/// an in-memory server whose peer info does or does not advertise
/// `resources.subscribe`; reports whether the link is watched afterwards.
async fn link_is_watched_after_call_tool(tag: &str, advertises_subscribe: bool) -> bool {
	use rmcp::model::{GetMeta, ServerCapabilities, ServerPeerInfo};

	let name = unique_server(tag);
	let uri = format!("job://{tag}");
	let config = stdin_config(&name, "unused-in-memory", Vec::new(), 5);
	let peer_info = advertises_subscribe.then(|| {
		ServerPeerInfo::new(
			ProtocolVersion::V_2026_07_28,
			ServerCapabilities::builder()
				.enable_resources()
				.enable_resources_subscribe()
				.build(),
		)
	});
	let (in_tx, in_rx) = mpsc::unbounded::<RxJsonRpcMessage<RoleClient>>();
	let (out_tx, mut out_rx) = mpsc::unbounded::<TxJsonRpcMessage<RoleClient>>();
	let service = rmcp::service::serve_directly(OctoClientHandler::new(&config), (out_tx, in_rx), peer_info);
	register(&name, service);

	let linked_uri = uri.clone();
	let responder = tokio::spawn(async move {
		while let Some(message) = out_rx.next().await {
			if let JsonRpcMessage::Request(request) = message {
				match request.request {
					ClientRequest::CallToolRequest(_) => in_tx
						.unbounded_send(JsonRpcMessage::response(
							ServerResult::CallToolResult(result_with_resource_link(&linked_uri)),
							request.id,
						))
						.expect("fake server channel must stay open"),
					ClientRequest::SubscriptionsListenRequest(_) => {
						let mut filter = SubscriptionFilter::new();
						filter.resource_subscriptions = Some(vec![linked_uri.clone()]);
						let mut ack = ServerNotification::SubscriptionsAcknowledgedNotification(Notification::new(
							SubscriptionsAcknowledgedNotificationParams::new(filter),
						));
						ack.get_meta_mut().set_subscription_id(request.id.clone());
						in_tx
							.unbounded_send(JsonRpcMessage::notification(ack))
							.expect("fake server channel must stay open");
					}
					_ => {}
				}
			}
		}
	});

	let session_id = format!("cov-link-gate-{}", uuid::Uuid::new_v4());
	let watched = crate::session::context::with_session_id(session_id.clone(), async {
		tokio::time::timeout(WAIT, call_tool(&config, &tool_call("link-1"), None))
			.await
			.expect("the call must not hang")
			.expect("the call must return the result");
		let watched = crate::session::shell_jobs::is_watched_for_session(&session_id, &uri);
		// Teardown ends the watcher, which would otherwise retry the dead server
		// for the whole outage budget once the responder is gone.
		crate::session::shell_jobs::clear_for_session(&session_id);
		watched
	})
	.await;

	responder.abort();
	disconnect(&name);
	watched
}

/// A server that does not advertise `resources.subscribe` can never signal the
/// end of a job, so the link it returns is a plain reference: not tracked.
#[serial]
#[tokio::test]
async fn a_resource_link_from_a_server_that_cannot_signal_updates_is_not_watched() {
	assert!(!link_is_watched_after_call_tool("link-gate-plain", false).await);
}

/// A server that advertises `resources.subscribe` can signal the end of a job,
/// so the link it returns is tracked.
#[serial]
#[tokio::test]
async fn a_resource_link_from_a_server_that_signals_updates_is_watched() {
	assert!(link_is_watched_after_call_tool("link-gate-subscribe", true).await);
}
