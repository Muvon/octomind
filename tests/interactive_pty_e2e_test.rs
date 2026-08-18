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

//! Interactive-mode end-to-end: run the real binary inside a pseudo-terminal
//! (tests/fixtures/pty_driver.py), type a prompt, read the streamed answer,
//! and leave via /exit. This is the only way the interactive main loop,
//! reedline input layer, and terminal rendering ever execute under test.

use std::process::{Command, Stdio};

use tokio::io::{AsyncReadExt, AsyncWriteExt};

const MARKER: &str = "PTY-E2E-MARKER";

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
						"message": {"role": "assistant", "content": format!("{MARKER}: interactive answer")},
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
	// Learning is on by default and its retrieval/distill models point at
	// octohub — a 401 there kills the interactive turn. Off entirely.
	config.supervisor.learning.enabled = false;
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_interactive_session_prompt_and_exit() {
	let stub_url = spawn_openai_stub().await;
	let home = tempfile::tempdir().expect("temp home");
	write_sandbox_config(home.path());

	let driver = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/pty_driver.py");
	let child = Command::new("python3")
		.arg(driver)
		.arg(MARKER)
		.arg("please answer with the marker")
		.arg("--")
		.arg(env!("CARGO_BIN_EXE_octomind"))
		.arg("run")
		.env("HOME", home.path())
		.env("OLLAMA_API_URL", &stub_url)
		.env("DO_NOT_TRACK", "1")
		.current_dir(home.path())
		.stdin(Stdio::null())
		.stdout(Stdio::piped())
		.stderr(Stdio::piped())
		.spawn()
		.expect("spawn pty driver");

	let output = tokio::task::spawn_blocking(move || child.wait_with_output())
		.await
		.expect("join")
		.expect("driver exits");
	let stdout = String::from_utf8_lossy(&output.stdout);
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(
		output.status.success() && stdout.contains("PTY_OK"),
		"interactive pty session failed.\nstdout:\n{stdout}\ntranscript:\n{stderr}"
	);

	// The interactive session persisted itself on /exit
	let sessions_dir = home.path().join(".local/share/octomind/sessions");
	let persisted = std::fs::read_dir(&sessions_dir)
		.map(|entries| entries.count())
		.unwrap_or(0);
	assert!(persisted > 0, "no session file written in sandbox");
}

/// Interactive slash-command dispatch: typing /help must render the command
/// list through the terminal display path (no model call involved).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_interactive_help_command() {
	let stub_url = spawn_openai_stub().await;
	let home = tempfile::tempdir().expect("temp home");
	write_sandbox_config(home.path());

	let driver = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/pty_driver.py");
	let child = Command::new("python3")
		.arg(driver)
		.arg("/loglevel") // unique string from the rendered help table
		.arg("/help")
		.arg("--")
		.arg(env!("CARGO_BIN_EXE_octomind"))
		.arg("run")
		.env("HOME", home.path())
		.env("OLLAMA_API_URL", &stub_url)
		.env("DO_NOT_TRACK", "1")
		.current_dir(home.path())
		.stdin(Stdio::null())
		.stdout(Stdio::piped())
		.stderr(Stdio::piped())
		.spawn()
		.expect("spawn pty driver");

	let output = tokio::task::spawn_blocking(move || child.wait_with_output())
		.await
		.expect("join")
		.expect("driver exits");
	let stdout = String::from_utf8_lossy(&output.stdout);
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(
		output.status.success() && stdout.contains("PTY_OK"),
		"interactive /help failed.\nstdout:\n{stdout}\ntranscript:\n{stderr}"
	);
}

/// Multi-turn interactive dialogue: a model round, then /info and /model
/// rendered inside the live loop — the paths a real terminal session hits
/// between turns.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_interactive_multi_turn_with_commands() {
	let stub_url = spawn_openai_stub().await;
	let home = tempfile::tempdir().expect("temp home");
	write_sandbox_config(home.path());

	let driver = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/pty_driver.py");
	let child = Command::new("python3")
		.arg(driver)
		// (expect, prompt) pairs, in order
		.arg(MARKER)
		.arg("please answer with the marker")
		.arg("session")
		.arg("/info")
		.arg("fake-model")
		.arg("/model")
		.arg(MARKER)
		.arg("and answer once more")
		.arg("--")
		.arg(env!("CARGO_BIN_EXE_octomind"))
		.arg("run")
		.env("HOME", home.path())
		.env("OLLAMA_API_URL", &stub_url)
		.env("DO_NOT_TRACK", "1")
		.current_dir(home.path())
		.stdin(Stdio::null())
		.stdout(Stdio::piped())
		.stderr(Stdio::piped())
		.spawn()
		.expect("spawn pty driver");

	let output = tokio::task::spawn_blocking(move || child.wait_with_output())
		.await
		.expect("join")
		.expect("driver exits");
	let stdout = String::from_utf8_lossy(&output.stdout);
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(
		output.status.success() && stdout.contains("PTY_OK"),
		"multi-turn interactive session failed.\nstdout:\n{stdout}\ntranscript:\n{stderr}"
	);
}

/// First request fails at the provider; typing again triggers the
/// retry-last-failed path in the interactive loop, which must then succeed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_interactive_retry_after_provider_failure() {
	use std::sync::atomic::{AtomicUsize, Ordering};
	// Stateful stub: two 500s, then marker answers forever.
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
		.await
		.expect("bind stub");
	let addr = listener.local_addr().expect("addr");
	let counter = std::sync::Arc::new(AtomicUsize::new(0));
	tokio::spawn(async move {
		while let Ok((mut sock, _)) = listener.accept().await {
			let counter = counter.clone();
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
				let (status, body) = if counter.fetch_add(1, Ordering::SeqCst) < 2 {
					(
						500u16,
						serde_json::json!({"error": {"message": "stub exploded"}}).to_string(),
					)
				} else {
					(
						200u16,
						serde_json::json!({
							"choices": [{
								"message": {"role": "assistant", "content": format!("{MARKER}: recovered")},
								"finish_reason": "stop"
							}],
							"usage": {"prompt_tokens": 10, "completion_tokens": 8, "total_tokens": 18, "cost": 0.0001}
						})
						.to_string(),
					)
				};
				let reason = if status == 200 { "OK" } else { "Error" };
				let response = format!(
					"HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
					body.len(),
					body
				);
				let _ = sock.write_all(response.as_bytes()).await;
				let _ = sock.shutdown().await;
			});
		}
	});
	let stub_url = format!("http://{}/v1/chat/completions", addr);

	let home = tempfile::tempdir().expect("temp home");
	write_sandbox_config(home.path());

	let driver = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/pty_driver.py");
	let child = Command::new("python3")
		.arg(driver)
		// First prompt fails at the provider ("rror" matches Error/error output)
		.arg("rror")
		.arg("please answer with the marker")
		// Typing again drives the retry path, which now succeeds
		.arg(MARKER)
		.arg("try again")
		.arg("--")
		.arg(env!("CARGO_BIN_EXE_octomind"))
		.arg("run")
		.env("HOME", home.path())
		.env("OLLAMA_API_URL", &stub_url)
		.env("DO_NOT_TRACK", "1")
		.current_dir(home.path())
		.stdin(Stdio::null())
		.stdout(Stdio::piped())
		.stderr(Stdio::piped())
		.spawn()
		.expect("spawn pty driver");

	let output = tokio::task::spawn_blocking(move || child.wait_with_output())
		.await
		.expect("join")
		.expect("driver exits");
	let stdout = String::from_utf8_lossy(&output.stdout);
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(
		output.status.success() && stdout.contains("PTY_OK"),
		"retry-after-failure session failed.\nstdout:\n{stdout}\ntranscript:\n{stderr}"
	);
}
