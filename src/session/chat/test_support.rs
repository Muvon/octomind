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

//! Shared fake-provider test harness: a scripted OpenAI-compatible HTTP stub
//! plus session/config builders wired to octolib's ollama provider via
//! `OLLAMA_API_URL`. Used by every in-crate e2e-style test that drives a
//! real LLM round trip (api executor, conversation compression, …).

use crate::config::Config;
use crate::session::chat::session::ChatSession;
use std::collections::VecDeque;
use std::sync::Mutex as StdMutex;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// `OLLAMA_API_URL` is process-global env — tests touching it must not
/// overlap, across ALL test modules in this binary. An async mutex because
/// the guard is deliberately held across the awaited LLM round trip, and it
/// cannot poison — a failed test must not cascade.
pub(crate) static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Spawn a one-shot-per-connection HTTP stub returning scripted
/// chat-completion bodies in order. Returns the chat-completions URL.
pub(crate) async fn spawn_stub(responses: Vec<serde_json::Value>) -> String {
	spawn_stub_with_status(responses.into_iter().map(|r| (200, r)).collect()).await
}

/// Like [`spawn_stub`] but each scripted entry carries its HTTP status,
/// so provider-level error handling can be exercised.
pub(crate) async fn spawn_stub_with_status(responses: Vec<(u16, serde_json::Value)>) -> String {
	spawn_recording_stub_with_status(responses).await.0
}

pub(crate) type CapturedRequests = std::sync::Arc<StdMutex<Vec<serde_json::Value>>>;

pub(crate) async fn spawn_recording_stub(responses: Vec<serde_json::Value>) -> (String, CapturedRequests) {
	spawn_recording_stub_with_status(responses.into_iter().map(|r| (200, r)).collect()).await
}

async fn spawn_recording_stub_with_status(responses: Vec<(u16, serde_json::Value)>) -> (String, CapturedRequests) {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
		.await
		.expect("bind stub listener");
	let addr = listener.local_addr().expect("stub addr");
	let queue = std::sync::Arc::new(StdMutex::new(VecDeque::from(responses)));
	let captured = std::sync::Arc::new(StdMutex::new(Vec::new()));
	let requests = captured.clone();

	tokio::spawn(async move {
		while let Ok((mut sock, _)) = listener.accept().await {
			let queue = queue.clone();
			let requests = requests.clone();
			tokio::spawn(async move {
				// Read headers + Content-Length body of the POST request.
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

				requests
					.lock()
					.expect("captured requests")
					.push(serde_json::from_slice(&buf[header_end..header_end + content_length]).expect("request JSON"));

				let (status, body) = queue.lock().expect("stub queue").pop_front().unwrap_or_else(|| {
					(
						200,
						serde_json::json!({
							"choices": [{
								"message": {"role": "assistant", "content": "SCRIPT EXHAUSTED"},
								"finish_reason": "stop"
							}],
							"usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
						}),
					)
				});
				let body = body.to_string();
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

	(format!("http://{}/v1/chat/completions", addr), captured)
}

pub(crate) fn final_response(text: &str) -> serde_json::Value {
	serde_json::json!({
		"choices": [{
			"message": {"role": "assistant", "content": text},
			"finish_reason": "stop"
		}],
		"usage": {"prompt_tokens": 20, "completion_tokens": 10, "total_tokens": 30, "cost": 0.001}
	})
}

pub(crate) fn tool_calls_response(calls: &[(&str, &str, serde_json::Value)]) -> serde_json::Value {
	let tool_calls: Vec<serde_json::Value> = calls
		.iter()
		.map(|(id, name, arguments)| {
			serde_json::json!({
				"id": id,
				"type": "function",
				"function": {"name": name, "arguments": arguments.to_string()}
			})
		})
		.collect();
	serde_json::json!({
		"choices": [{
			"message": {"role": "assistant", "content": "", "tool_calls": tool_calls},
			"finish_reason": "tool_calls"
		}],
		"usage": {"prompt_tokens": 25, "completion_tokens": 15, "total_tokens": 40, "cost": 0.002}
	})
}

pub(crate) fn tool_call_response(tool_name: &str, arguments: serde_json::Value) -> serde_json::Value {
	tool_calls_response(&[("call_1", tool_name, arguments)])
}

/// Merged config wired for the fake provider: real template + assistant role.
pub(crate) fn fake_provider_config() -> Config {
	let mut config: Config =
		toml::from_str(&crate::config::template_with_assistant_role::template_with_assistant_role())
			.expect("parse default config template");
	config.build_role_map();
	config.model = "ollama:fake-model".to_string();
	// Scripted queues describe exact call sequences. Tests that exercise retry
	// behavior opt back in explicitly instead of consuming an unrelated entry.
	config.max_retries = 0;
	let mut merged = config.get_merged_config_for_role("assistant");
	merged.model = "ollama:fake-model".to_string();
	merged
}

pub(crate) fn fake_session(user_input: &str) -> ChatSession {
	let mut session = ChatSession::for_tests(Vec::new());
	session.model = "ollama:fake-model".to_string();
	session.session.info.model = "ollama:fake-model".to_string();
	session.add_user_message(user_input).expect("add user message");
	session
}

/// Snapshot environment variables and restore them on drop, so a failed assert
/// cannot leak a sandboxed data dir or API URL into the next test.
pub(crate) struct EnvGuard(Vec<(&'static str, Option<std::ffi::OsString>)>);

impl EnvGuard {
	pub(crate) fn new(keys: &[&'static str]) -> Self {
		Self(keys.iter().map(|k| (*k, std::env::var_os(k))).collect())
	}
}

impl Drop for EnvGuard {
	fn drop(&mut self) {
		for (key, saved) in &self.0 {
			match saved {
				Some(v) => std::env::set_var(key, v),
				None => std::env::remove_var(key),
			}
		}
	}
}

/// Tests using this fixture must hold `serial_test::serial` because the data
/// directory is selected through a process-wide environment variable.
pub(crate) struct DataDirGuard {
	_env: EnvGuard,
	_dir: tempfile::TempDir,
}

impl DataDirGuard {
	pub(crate) fn new() -> Self {
		let dir = tempfile::tempdir().expect("temp data dir");
		let env = EnvGuard::new(&["OCTOMIND_DATA_DIR"]);
		std::env::set_var("OCTOMIND_DATA_DIR", dir.path());
		Self { _env: env, _dir: dir }
	}
}
