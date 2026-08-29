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

//! WebSocket end-to-end: run the real `WebSocketServer` in-process (the same
//! code path `octomind server` executes), connect as a real client over
//! tokio-tungstenite, create a session, send a message and a command, and
//! read the streamed server events — against the fake ollama provider.
//!
//! In-process (not a spawned binary) so the server has no shutdown problem:
//! the accept loop runs forever by design, and killing a child would lose
//! its coverage profile. HOME is redirected to a tempdir before any session
//! I/O happens; this file holds exactly one test, so the process-wide env
//! mutation races with nothing.

use std::time::Duration;

use futures::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::Message as WsMessage;

const MARKER: &str = "WS-E2E-MARKER";

async fn spawn_openai_stub() -> String {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
		.await
		.expect("bind stub");
	let addr = listener.local_addr().expect("addr");

	tokio::spawn(async move {
		while let Ok((mut sock, _)) = listener.accept().await {
			tokio::spawn(async move {
				let mut buf = Vec::new();
				let mut tmp = [0u8; 8192];
				let header_end = loop {
					let n = sock.read(&mut tmp).await.unwrap_or(0);
					if n == 0 {
						return;
					}
					buf.extend_from_slice(&tmp[..n]);
					if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
						break pos + 4;
					}
				};
				let headers = String::from_utf8_lossy(&buf[..header_end]).to_lowercase();
				let content_length: usize = headers
					.lines()
					.find_map(|l| l.strip_prefix("content-length:"))
					.and_then(|v| v.trim().parse().ok())
					.unwrap_or(0);
				while buf.len() < header_end + content_length {
					let n = sock.read(&mut tmp).await.unwrap_or(0);
					if n == 0 {
						break;
					}
					buf.extend_from_slice(&tmp[..n]);
				}
				let body = serde_json::json!({
					"choices": [{
						"message": {"role": "assistant", "content": format!("{MARKER}: ws answer")},
						"finish_reason": "stop"
					}],
					"usage": {"prompt_tokens": 10, "completion_tokens": 8, "total_tokens": 18, "cost": 0.0001}
				})
				.to_string();
				let response = format!(
					"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
					body.len(),
					body
				);
				let _ = sock.write_all(response.as_bytes()).await;
				let _ = sock.shutdown().await;
			});
		}
	});

	format!("http://{}/v1/chat/completions", addr)
}

fn sandbox_config() -> octomind::config::Config {
	let mut config: octomind::config::Config =
		toml::from_str(include_str!("../config-templates/default.toml"))
			.expect("parse default config template");
	config.model = "ollama:fake-model".to_string();
	config.default = "assistant".to_string();
	config.supervisor.enabled = false;
	config.telemetry = false;
	config.auto_capabilities = false;
	config.skills.auto_activation = false;
	config.skills.auto_validation = false;
	config.build_role_map();
	config
}

async fn free_port() -> u16 {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
		.await
		.expect("probe port");
	let port = listener.local_addr().expect("addr").port();
	drop(listener);
	port
}

/// Read text frames until one satisfies `pred` (returned) or the deadline
/// passes (panics with everything seen).
async fn read_until<S>(socket: &mut S, what: &str, secs: u64, pred: impl Fn(&str) -> bool) -> String
where
	S: StreamExt<Item = Result<WsMessage, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
	let mut seen = Vec::new();
	let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
	while tokio::time::Instant::now() < deadline {
		match tokio::time::timeout_at(deadline, socket.next()).await {
			Ok(Some(Ok(WsMessage::Text(text)))) => {
				let owned = text.to_string();
				if pred(&owned) {
					return owned;
				}
				seen.push(owned);
			}
			Ok(Some(Ok(_))) => continue,
			Ok(Some(Err(e))) => panic!("ws error while waiting for {what}: {e}; seen: {seen:?}"),
			Ok(None) => panic!("server closed connection while waiting for {what}; seen: {seen:?}"),
			Err(_) => break,
		}
	}
	panic!("{what} never arrived; seen: {seen:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_ws_session_message_roundtrip() {
	let stub_url = spawn_openai_stub().await;
	let home = tempfile::tempdir().expect("temp home");
	// Redirect all session/data I/O into the sandbox before the server does
	// any of it. Sole test in this binary — no other thread reads env.
	std::env::set_var("HOME", home.path());
	std::env::set_var(
		"OCTOMIND_DATA_DIR",
		home.path().join(".local/share/octomind"),
	);
	std::env::set_var("OLLAMA_API_URL", &stub_url);
	std::env::set_var("DO_NOT_TRACK", "1");

	let port = free_port().await;
	let server = octomind::websocket::WebSocketServer::new(
		"127.0.0.1",
		port,
		sandbox_config(),
		"assistant".to_string(),
		Vec::new(),
	)
	.expect("create server");
	tokio::spawn(async move {
		let _ = server.start().await;
	});

	// Retry until the server accepts websocket connections
	let url = format!("ws://127.0.0.1:{port}");
	let mut socket = None;
	for _ in 0..100 {
		match tokio_tungstenite::connect_async(&url).await {
			Ok((ws, _)) => {
				socket = Some(ws);
				break;
			}
			Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
		}
	}
	let mut socket = socket.expect("websocket connects");

	// Create-or-resume a session, then send a user message
	socket
		.send(WsMessage::Text(
			serde_json::json!({"type": "session", "session_id": "ws-e2e"})
				.to_string()
				.into(),
		))
		.await
		.expect("send session");
	socket
		.send(WsMessage::Text(
			serde_json::json!({
				"type": "message",
				"session_id": "ws-e2e",
				"content": "answer with the marker"
			})
			.to_string()
			.into(),
		))
		.await
		.expect("send message");
	read_until(&mut socket, "assistant marker", 60, |t| t.contains(MARKER)).await;

	// Command channel: /help must come back as command output, not a model turn
	socket
		.send(WsMessage::Text(
			serde_json::json!({
				"type": "command",
				"session_id": "ws-e2e",
				"command": "help",
				"request_id": "cmd-1"
			})
			.to_string()
			.into(),
		))
		.await
		.expect("send command");
	read_until(&mut socket, "help command ack", 30, |t| t.contains("cmd-1")).await;

	// Bare model command: renders the current model back to the client
	socket
		.send(WsMessage::Text(
			serde_json::json!({
				"type": "command",
				"session_id": "ws-e2e",
				"command": "model",
				"request_id": "cmd-2"
			})
			.to_string()
			.into(),
		))
		.await
		.expect("send model command");
	read_until(&mut socket, "model command ack", 30, |t| {
		t.contains("cmd-2")
	})
	.await;

	// Exercise the remaining read-only/session-local command surfaces through
	// the production WebSocket dispatcher. Each command must acknowledge its
	// request id; rendering details are covered by command unit tests.
	for (index, (command, args)) in [
		("info", serde_json::json!([])),
		("context", serde_json::json!(["all"])),
		("agents", serde_json::json!([])),
		("learning", serde_json::json!([])),
		("plan", serde_json::json!([])),
		("mcp", serde_json::json!(["list"])),
		("effort", serde_json::json!(["low"])),
		("loglevel", serde_json::json!(["none"])),
		("cache", serde_json::json!([])),
		("report", serde_json::json!([])),
	]
	.into_iter()
	.enumerate()
	{
		let request_id = format!("cmd-extra-{index}");
		socket
			.send(WsMessage::Text(
				serde_json::json!({
					"type": "command",
					"session_id": "ws-e2e",
					"command": command,
					"args": args,
					"request_id": request_id,
				})
				.to_string()
				.into(),
			))
			.await
			.expect("send command");
		read_until(&mut socket, command, 30, |text| text.contains(&request_id)).await;
	}

	// Re-sending the same session id takes the resume arm of create-or-resume
	socket
		.send(WsMessage::Text(
			serde_json::json!({"type": "session", "session_id": "ws-e2e", "request_id": "sess-2"})
				.to_string()
				.into(),
		))
		.await
		.expect("resume session");
	read_until(&mut socket, "session resume ack", 30, |t| {
		t.contains("sess-2")
	})
	.await;

	// Inbox injection: a queued message for this session is surfaced to the
	// client as an Injected frame before the AI answers it.
	octomind::session::inbox::push_inbox_message_for_session(
		"ws-e2e",
		octomind::session::inbox::InboxMessage {
			source: octomind::session::inbox::InboxSource::Schedule {
				id: "sched-1".to_string(),
			},
			content: "INBOX-INJECTED-MARKER: check the queue".to_string(),
		},
	);
	socket
		.send(WsMessage::Text(
			serde_json::json!({
				"type": "message",
				"session_id": "ws-e2e",
				"content": "and now answer again"
			})
			.to_string()
			.into(),
		))
		.await
		.expect("send post-inbox message");
	read_until(&mut socket, "injected frame", 60, |t| {
		t.contains("INBOX-INJECTED-MARKER")
	})
	.await;
	read_until(&mut socket, "post-inbox answer", 60, |t| t.contains(MARKER)).await;

	// A structured error for a bogus session must come back, not a hang
	socket
		.send(WsMessage::Text(
			serde_json::json!({
				"type": "message",
				"session_id": "no-such-session-e2e",
				"content": "hello?"
			})
			.to_string()
			.into(),
		))
		.await
		.expect("send bogus");
	read_until(&mut socket, "unknown-session error", 30, |t| {
		t.contains("error") || t.contains("not")
	})
	.await;

	// Malformed JSON must produce an error response, not kill the connection
	socket
		.send(WsMessage::Text("{not json".to_string().into()))
		.await
		.expect("send malformed");
	read_until(&mut socket, "malformed-input error", 30, |t| {
		t.to_lowercase().contains("error") || t.to_lowercase().contains("invalid")
	})
	.await;

	// The session was persisted inside the sandbox HOME
	let sessions_dir = home.path().join(".local/share/octomind/sessions");
	let persisted = std::fs::read_dir(&sessions_dir)
		.map(|entries| entries.count())
		.unwrap_or(0);
	assert!(persisted > 0, "no session file written in sandbox");

	// Browser-origin handshakes are refused when no --allow-origin is set:
	// a connection carrying an Origin header must fail at the handshake.
	use tokio_tungstenite::tungstenite::client::IntoClientRequest;
	let mut request = url.clone().into_client_request().expect("request");
	request.headers_mut().insert(
		"Origin",
		"http://evil.example".parse().expect("header value"),
	);
	assert!(
		tokio_tungstenite::connect_async(request).await.is_err(),
		"handshake with a browser Origin must be refused"
	);

	let _ = socket.close(None).await;
}
