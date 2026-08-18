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

//! WebSocket end-to-end: spawn `octomind server` sandboxed, connect as a
//! real client over tokio-tungstenite, create a session, send a message,
//! and read the streamed server events — against the fake ollama provider.

use std::process::Stdio;
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

fn write_sandbox_config(home: &std::path::Path) {
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

	let config_dir = home.join(".local/share/octomind/config");
	std::fs::create_dir_all(&config_dir).expect("create config dir");
	std::fs::write(
		config_dir.join("config.toml"),
		toml::to_string(&config).expect("serialize config"),
	)
	.expect("write config");
}

async fn free_port() -> u16 {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
		.await
		.expect("probe port");
	let port = listener.local_addr().expect("addr").port();
	drop(listener);
	port
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_ws_session_message_roundtrip() {
	let stub_url = spawn_openai_stub().await;
	let home = tempfile::tempdir().expect("temp home");
	write_sandbox_config(home.path());
	let port = free_port().await;

	let mut server = tokio::process::Command::new(env!("CARGO_BIN_EXE_octomind"))
		.env("HOME", home.path())
		.env("OLLAMA_API_URL", &stub_url)
		.env("DO_NOT_TRACK", "1")
		.current_dir(home.path())
		.args(["server", "--port", &port.to_string()])
		.stdin(Stdio::null())
		.stdout(Stdio::null())
		.stderr(Stdio::null())
		.spawn()
		.expect("spawn octomind server");

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

	// Collect streamed events until the assistant answer with the marker
	// arrives (or time out loudly with everything we saw).
	let mut seen = Vec::new();
	let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
	let mut got_marker = false;
	while tokio::time::Instant::now() < deadline {
		let next = tokio::time::timeout_at(deadline, socket.next()).await;
		match next {
			Ok(Some(Ok(WsMessage::Text(text)))) => {
				let owned = text.to_string();
				if owned.contains(MARKER) {
					got_marker = true;
					seen.push(owned);
					break;
				}
				seen.push(owned);
			}
			Ok(Some(Ok(_))) => continue,
			Ok(Some(Err(e))) => panic!("ws error: {e}; seen: {seen:?}"),
			Ok(None) => panic!("server closed connection; seen: {seen:?}"),
			Err(_) => break,
		}
	}
	assert!(got_marker, "assistant marker never arrived; seen: {seen:?}");

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
	let mut got_error = false;
	let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
	while tokio::time::Instant::now() < deadline {
		match tokio::time::timeout_at(deadline, socket.next()).await {
			Ok(Some(Ok(WsMessage::Text(text)))) => {
				if text.contains("error") || text.contains("not") {
					got_error = true;
					break;
				}
			}
			Ok(Some(Ok(_))) => continue,
			_ => break,
		}
	}
	assert!(got_error, "no error response for unknown session");

	let _ = socket.close(None).await;
	let _ = server.kill().await;
}
