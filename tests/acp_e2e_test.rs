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

//! ACP end-to-end: spawn `octomind acp` with a sandboxed HOME and speak the
//! real newline-delimited JSON-RPC protocol over its stdio — initialize,
//! session/new, session/prompt — against the fake ollama provider. This is
//! exactly how Zed and other ACP clients drive octomind.

use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

const MARKER: &str = "ACP-E2E-MARKER";

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
						"message": {"role": "assistant", "content": format!("{MARKER}: acp answer")},
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

struct AcpClient {
	child: tokio::process::Child,
	stdin: tokio::process::ChildStdin,
	reader: BufReader<tokio::process::ChildStdout>,
	next_id: u64,
}

impl AcpClient {
	async fn spawn(home: &std::path::Path, stub_url: &str) -> Self {
		let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_octomind"))
			.env("HOME", home)
			.env("OLLAMA_API_URL", stub_url)
			.env("DO_NOT_TRACK", "1")
			.current_dir(home)
			.arg("acp")
			.stdin(Stdio::piped())
			.stdout(Stdio::piped())
			.stderr(Stdio::null())
			.spawn()
			.expect("spawn octomind acp");
		let stdin = child.stdin.take().expect("stdin");
		let reader = BufReader::new(child.stdout.take().expect("stdout"));
		Self {
			child,
			stdin,
			reader,
			next_id: 0,
		}
	}

	/// Send a request and collect (response, notifications-seen-before-it).
	async fn request(
		&mut self,
		method: &str,
		params: serde_json::Value,
	) -> (serde_json::Value, Vec<serde_json::Value>) {
		let id = self.next_id;
		self.next_id += 1;
		let line = serde_json::json!({
			"jsonrpc": "2.0",
			"id": id,
			"method": method,
			"params": params
		})
		.to_string();
		self.stdin
			.write_all(format!("{line}\n").as_bytes())
			.await
			.expect("write request");
		self.stdin.flush().await.expect("flush");

		let mut notifications = Vec::new();
		loop {
			let mut buf = String::new();
			let read =
				tokio::time::timeout(Duration::from_secs(60), self.reader.read_line(&mut buf))
					.await
					.unwrap_or_else(|_| panic!("timeout waiting for response to {method}"))
					.expect("read line");
			assert!(read > 0, "agent closed stdout while awaiting {method}");
			let trimmed = buf.trim();
			if trimmed.is_empty() {
				continue;
			}
			let msg: serde_json::Value =
				serde_json::from_str(trimmed).unwrap_or_else(|e| panic!("bad json {trimmed}: {e}"));
			if msg.get("id").and_then(|v| v.as_u64()) == Some(id) {
				assert!(msg.get("error").is_none(), "{method} returned error: {msg}");
				return (msg["result"].clone(), notifications);
			}
			// Everything else is a notification (session/update etc.)
			notifications.push(msg);
		}
	}
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_acp_initialize_new_session_prompt() {
	let stub_url = spawn_openai_stub().await;
	let home = tempfile::tempdir().expect("temp home");
	write_sandbox_config(home.path());

	let mut client = AcpClient::spawn(home.path(), &stub_url).await;

	// initialize
	let (init, _) = client
		.request(
			"initialize",
			serde_json::json!({
				"protocolVersion": 1,
				"clientCapabilities": {}
			}),
		)
		.await;
	assert!(
		init.get("protocolVersion").is_some(),
		"initialize result: {init}"
	);
	assert!(
		init.get("agentCapabilities").is_some(),
		"initialize result: {init}"
	);

	// session/new
	let cwd = home.path().to_string_lossy().to_string();
	let (new_session, _) = client
		.request(
			"session/new",
			serde_json::json!({"cwd": cwd, "mcpServers": []}),
		)
		.await;
	let session_id = new_session["sessionId"]
		.as_str()
		.unwrap_or_else(|| panic!("no sessionId in {new_session}"))
		.to_string();

	// session/prompt → the turn runs against the stub provider
	let (prompt_result, notifications) = client
		.request(
			"session/prompt",
			serde_json::json!({
				"sessionId": session_id,
				"prompt": [{"type": "text", "text": "answer with the marker"}]
			}),
		)
		.await;
	assert_eq!(
		prompt_result["stopReason"].as_str(),
		Some("end_turn"),
		"prompt result: {prompt_result}"
	);
	// The assistant chunk with the stub's marker arrived as a session/update
	let updates_text = serde_json::to_string(&notifications).expect("serialize updates");
	assert!(
		updates_text.contains(MARKER),
		"no update carried the marker: {updates_text}"
	);

	// Slash commands ride the same prompt channel per the ACP spec — /help
	// must be handled as a command, not forwarded to the model.
	let (help_result, _) = client
		.request(
			"session/prompt",
			serde_json::json!({
				"sessionId": session_id,
				"prompt": [{"type": "text", "text": "/help"}]
			}),
		)
		.await;
	assert!(
		help_result.get("stopReason").is_some(),
		"/help prompt result: {help_result}"
	);

	// Multi-turn: a second real prompt on the same session still round-trips
	let (second_result, second_updates) = client
		.request(
			"session/prompt",
			serde_json::json!({
				"sessionId": session_id,
				"prompt": [{"type": "text", "text": "and once more"}]
			}),
		)
		.await;
	assert_eq!(second_result["stopReason"].as_str(), Some("end_turn"));
	let updates_text = serde_json::to_string(&second_updates).expect("serialize updates");
	assert!(updates_text.contains(MARKER), "second turn missing marker");

	// A second agent process loads the persisted session (session/load
	// replays history as session/update notifications) and continues it.
	let mut client2 = AcpClient::spawn(home.path(), &stub_url).await;
	let (_, _) = client2
		.request(
			"initialize",
			serde_json::json!({"protocolVersion": 1, "clientCapabilities": {}}),
		)
		.await;
	// This agent does not replay history on load — the contract is that the
	// load succeeds (request() already rejects error responses) and the
	// session is continuable.
	let (_, _) = client2
		.request(
			"session/load",
			serde_json::json!({"sessionId": session_id, "cwd": cwd, "mcpServers": []}),
		)
		.await;
	let (loaded_result, _) = client2
		.request(
			"session/prompt",
			serde_json::json!({
				"sessionId": session_id,
				"prompt": [{"type": "text", "text": "continue after load"}]
			}),
		)
		.await;
	assert_eq!(loaded_result["stopReason"].as_str(), Some("end_turn"));

	let AcpClient {
		mut child, stdin, ..
	} = client2;
	drop(stdin);
	match tokio::time::timeout(Duration::from_secs(30), child.wait()).await {
		Ok(status) => {
			let status = status.expect("wait on acp child 2");
			assert!(status.success(), "second acp agent exited with {status}");
		}
		Err(_) => {
			let _ = child.kill().await;
			panic!("second acp agent did not exit after stdin EOF");
		}
	}

	// Graceful shutdown: closing stdin fires the agent's EOF disconnect and
	// the process must exit cleanly on its own (this is also what lets the
	// instrumented child flush its coverage profile — kill() would drop it).
	let AcpClient {
		mut child, stdin, ..
	} = client;
	drop(stdin);
	match tokio::time::timeout(Duration::from_secs(30), child.wait()).await {
		Ok(status) => {
			let status = status.expect("wait on acp child");
			assert!(status.success(), "acp agent exited with {status}");
		}
		Err(_) => {
			let _ = child.kill().await;
			panic!("acp agent did not exit after stdin EOF");
		}
	}
}
