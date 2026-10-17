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

async fn spawn_openai_stub(script: Option<Vec<&'static str>>) -> String {
	let script = std::sync::Arc::new(std::sync::Mutex::new(
		script.map(std::collections::VecDeque::from),
	));
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
		.await
		.expect("bind stub");
	let addr = listener.local_addr().expect("addr");

	tokio::spawn(async move {
		while let Ok((mut sock, _)) = listener.accept().await {
			let script = std::sync::Arc::clone(&script);
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
				let content = match script.lock().expect("script lock").as_mut() {
					Some(responses) => responses
						.pop_front()
						.expect("unexpected provider call")
						.to_owned(),
					None => format!("{MARKER}: acp answer"),
				};
				let body = serde_json::json!({
					"choices": [{
						"message": {"role": "assistant", "content": content},
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
	/// Rolling tail of the agent's stderr, drained by a background task.
	/// Surfacing this buffer in the panic turns an undebuggable timeout into
	/// log lines pointing at the stall.
	stderr_tail: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
}

impl AcpClient {
	fn stderr_dump(&self) -> String {
		self.stderr_tail
			.lock()
			.map(|t| String::from_utf8_lossy(&t).to_string())
			.unwrap_or_default()
	}
	async fn spawn(home: &std::path::Path, stub_url: &str) -> Self {
		let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_octomind"))
			.env("HOME", home)
			.env("OCTOMIND_DATA_DIR", home.join(".local/share/octomind"))
			.env("OLLAMA_API_URL", stub_url)
			.env("DO_NOT_TRACK", "1")
			.current_dir(home)
			.arg("acp")
			.stdin(Stdio::piped())
			.stdout(Stdio::piped())
			.stderr(Stdio::piped())
			.spawn()
			.expect("spawn octomind acp");
		let stdin = child.stdin.take().expect("stdin");
		let reader = BufReader::new(child.stdout.take().expect("stdout"));
		let stderr = child.stderr.take().expect("stderr");
		let stderr_tail: std::sync::Arc<std::sync::Mutex<Vec<u8>>> =
			std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
		{
			let tail = std::sync::Arc::clone(&stderr_tail);
			tokio::spawn(async move {
				use tokio::io::AsyncReadExt;
				let mut stderr = stderr;
				let mut buf = [0u8; 4096];
				loop {
					match stderr.read(&mut buf).await {
						Ok(0) | Err(_) => break,
						Ok(n) => {
							if let Ok(mut t) = tail.lock() {
								t.extend_from_slice(&buf[..n]);
								let excess = t.len().saturating_sub(64 * 1024);
								if excess > 0 {
									t.drain(..excess);
								}
							}
						}
					}
				}
			});
		}
		Self {
			child,
			stdin,
			reader,
			next_id: 0,
			stderr_tail,
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
					.unwrap_or_else(|_| {
						panic!(
							"timeout waiting for response to {method}; agent stderr tail:\n{}",
							self.stderr_dump()
						)
					})
					.expect("read line");
			assert!(
				read > 0,
				"agent closed stdout while awaiting {method}; agent stderr tail:\n{}",
				self.stderr_dump()
			);
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
	let stub_url = spawn_openai_stub(None).await;
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

	// ACP extension commands share the live session but return structured
	// responses, making broad command coverage deterministic without terminal
	// rendering or model calls.
	for (command, args) in [
		("/info", serde_json::json!([])),
		("/context", serde_json::json!(["all"])),
		("/status", serde_json::json!([])),
		("/learning", serde_json::json!([])),
		("/plan", serde_json::json!([])),
		("/mcp", serde_json::json!(["list"])),
		("/effort", serde_json::json!(["low"])),
		("/report", serde_json::json!([])),
	] {
		let (response, _) = client
			.request(
				"_octomind/command",
				serde_json::json!({
					"session_id": session_id,
					"command": command,
					"args": args,
				}),
			)
			.await;
		assert_eq!(response["success"], true, "{command}: {response}");
	}

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

	// Schedule an immediate inbox message: the agent's idle monitor picks it
	// up and runs an autonomous inbox turn against the stub. No assertion —
	// the session staying functional below is the observable contract; the
	// wait just gives the fire-and-inject path time to run.
	let (sched_result, _) = client
		.request(
			"session/prompt",
			serde_json::json!({
				"sessionId": session_id,
				"prompt": [{"type": "text", "text": "/schedule add when=\"now\" message=\"inbox ping\""}]
			}),
		)
		.await;
	assert!(sched_result.get("stopReason").is_some());
	tokio::time::sleep(Duration::from_secs(5)).await;

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

/// A prompt that triggers a tool round: the ToolCall / ToolCallUpdate
/// translation paths only run when the model actually calls a tool.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_acp_tool_round_updates() {
	use std::sync::atomic::{AtomicUsize, Ordering};
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
				let body = if counter.fetch_add(1, Ordering::SeqCst) == 0 {
					serde_json::json!({
						"choices": [{
							"message": {
								"role": "assistant",
								"content": "",
								"tool_calls": [{
									"id": "call_acp",
									"type": "function",
									"function": {"name": "schedule", "arguments": "{\"action\":\"list\"}"}
								}]
							},
							"finish_reason": "tool_calls"
						}],
						"usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
					})
				} else {
					serde_json::json!({
						"choices": [{
							"message": {"role": "assistant", "content": format!("{MARKER}: tool round done")},
							"finish_reason": "stop"
						}],
						"usage": {"prompt_tokens": 20, "completion_tokens": 8, "total_tokens": 28}
					})
				}
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
	let stub_url = format!("http://{}/v1/chat/completions", addr);

	let home = tempfile::tempdir().expect("temp home");
	write_sandbox_config(home.path());

	let mut client = AcpClient::spawn(home.path(), &stub_url).await;
	let (_, _) = client
		.request(
			"initialize",
			serde_json::json!({"protocolVersion": 1, "clientCapabilities": {}}),
		)
		.await;
	let cwd = home.path().to_string_lossy().to_string();
	let (new_session, _) = client
		.request(
			"session/new",
			serde_json::json!({"cwd": cwd, "mcpServers": []}),
		)
		.await;
	let session_id = new_session["sessionId"].as_str().expect("id").to_string();

	let (result, notifications) = client
		.request(
			"session/prompt",
			serde_json::json!({
				"sessionId": session_id,
				"prompt": [{"type": "text", "text": "list my schedules with your tool"}]
			}),
		)
		.await;
	assert_eq!(result["stopReason"].as_str(), Some("end_turn"));
	let updates = serde_json::to_string(&notifications).expect("serialize");
	assert!(
		updates.contains("tool_call") || updates.contains("toolCall"),
		"no tool-call updates streamed: {updates}"
	);
	assert!(updates.contains(MARKER), "final answer missing: {updates}");

	let AcpClient {
		mut child, stdin, ..
	} = client;
	drop(stdin);
	match tokio::time::timeout(Duration::from_secs(30), child.wait()).await {
		Ok(status) => assert!(status.expect("wait").success()),
		Err(_) => {
			let _ = child.kill().await;
			panic!("acp agent did not exit after stdin EOF");
		}
	}
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn final_usage_notification_matches_info_after_a_nonzero_gate() {
	let stub_url = spawn_openai_stub(Some(vec![
		r#"{"scope":"self_contained","forbids_verification":false,"verification_policy_update":"unchanged","verification_policy_evidence":"","answer_only":false,"conditions":[],"operational_constraints":[]}"#,
		"ACP-GATE-TIMING: finished",
		r#"<shape name="circular" found="no">independent evidence</shape>
<shape name="context-stripped" found="no">representative context</shape>
<shape name="acceptance-only" found="no">not applicable</shape>
<shape name="unenumerated-category" found="no">bounded scope</shape>
<verdict>PASS</verdict>"#,
	])).await;
	let home = tempfile::tempdir().expect("sandbox home");
	write_sandbox_config(home.path());
	let path = home.path().join(".local/share/octomind/config/config.toml");
	let mut config: octomind::config::Config =
		toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
	config.supervisor.enabled = true;
	config.supervisor.model.model = Some("ollama:fake-model".into());
	config.supervisor.learning.enabled = false;
	config.supervisor.plan.enabled = false;
	// Condense compacts a specialist brief concurrently with task resolution;
	// that extra, unordered call would steal a scripted response.
	config.supervisor.condense.enabled = false;
	config.decision.model = None;
	std::fs::write(path, toml::to_string(&config).unwrap()).unwrap();
	let mut client = AcpClient::spawn(home.path(), &stub_url).await;
	client
		.request(
			"initialize",
			serde_json::json!({"protocolVersion":1,"clientCapabilities":{}}),
		)
		.await;
	let (created, _) = client
		.request(
			"session/new",
			serde_json::json!({"cwd":home.path(),"mcpServers":[]}),
		)
		.await;
	let id = created["sessionId"].as_str().unwrap();
	let (_, notifications) = client.request("session/prompt", serde_json::json!({"sessionId":id,"prompt":[{"type":"text","text":"Answer with ACP-GATE-TIMING"}]})).await;
	let usage = notifications
		.iter()
		.filter_map(|message| message["params"]["_meta"].get("octomind.usage"))
		.next_back()
		.expect("final usage notification");
	assert!(
		usage["supervisor_accounting"]["mechanics"]["gate"]["chat_calls"]
			.as_u64()
			.unwrap() > 0
	);
	assert!(
		usage["supervisor_accounting"]["mechanics"]["gate"]["cost"]
			.as_f64()
			.unwrap() > 0.0
	);
	assert_eq!(usage["supervisor_accounting"]["turn_time"]["completed"], 1);
	let (info, _) = client
		.request(
			"_octomind/command",
			serde_json::json!({"session_id":id,"command":"/info","args":[]}),
		)
		.await;
	assert_eq!(
		usage["supervisor_accounting"],
		info["output"]["supervisor_accounting"]
	);
	assert_eq!(usage["session_cost"], info["output"]["total_cost"]);
	let AcpClient {
		mut child, stdin, ..
	} = client;
	drop(stdin);
	assert!(tokio::time::timeout(Duration::from_secs(30), child.wait())
		.await
		.unwrap()
		.unwrap()
		.success());
}
