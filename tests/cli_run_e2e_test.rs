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

//! Binary-level end-to-end: spawn the real `octomind` binary with HOME
//! sandboxed into a tempdir and the ollama provider pointed at a local
//! scripted stub. Exercises the full stack a user hits: CLI parsing, config
//! load, session creation and persistence, the non-interactive main loop,
//! provider round trip, and process exit — with zero network and zero
//! writes outside the tempdir.

use std::io::Write as _;
use std::process::{Command, Stdio};

use tokio::io::{AsyncReadExt, AsyncWriteExt};

const MARKER: &str = "E2E-OK-MARKER";

/// Minimal always-answers OpenAI-compatible stub. Every request gets the
/// same final response carrying MARKER.
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
						"message": {"role": "assistant", "content": format!("{MARKER}: everything works")},
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

/// Sandboxed config derived from the shipped template: fake-provider model,
/// every network/model-heavy subsystem off.
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

fn octomind_cmd(home: &std::path::Path, stub_url: &str) -> Command {
	let mut cmd = Command::new(env!("CARGO_BIN_EXE_octomind"));
	cmd.env("HOME", home)
		.env("OLLAMA_API_URL", stub_url)
		.env_remove("OCTOMIND_TELEMETRY")
		.env("DO_NOT_TRACK", "1")
		.current_dir(home);
	cmd
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_run_non_interactive_end_to_end() {
	let stub_url = spawn_openai_stub().await;
	let home = tempfile::tempdir().expect("temp home");
	write_sandbox_config(home.path());

	let mut child = octomind_cmd(home.path(), &stub_url)
		.args(["run", "--format", "plain"])
		.stdin(Stdio::piped())
		.stdout(Stdio::piped())
		.stderr(Stdio::piped())
		.spawn()
		.expect("spawn octomind run");
	child
		.stdin
		.take()
		.expect("stdin")
		.write_all(b"please respond with the marker\n")
		.expect("write prompt");

	let output = child.wait_with_output().expect("octomind exits");
	let stdout = String::from_utf8_lossy(&output.stdout);
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(
		output.status.success(),
		"octomind run failed.\nstdout:\n{stdout}\nstderr:\n{stderr}"
	);
	assert!(
		stdout.contains(MARKER),
		"assistant answer missing from output.\nstdout:\n{stdout}\nstderr:\n{stderr}"
	);

	// The session was persisted inside the sandbox
	let sessions_dir = home.path().join(".local/share/octomind/sessions");
	let persisted = std::fs::read_dir(&sessions_dir)
		.map(|entries| entries.count())
		.unwrap_or(0);
	assert!(persisted > 0, "no session file written in sandbox");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_config_show_against_sandbox() {
	let stub_url = spawn_openai_stub().await;
	let home = tempfile::tempdir().expect("temp home");
	write_sandbox_config(home.path());

	let output = octomind_cmd(home.path(), &stub_url)
		.args(["config", "--show"])
		.output()
		.expect("octomind config --show runs");
	let stdout = String::from_utf8_lossy(&output.stdout);
	assert!(
		output.status.success(),
		"config --show failed: {}",
		String::from_utf8_lossy(&output.stderr)
	);
	assert!(
		stdout.contains("ollama:fake-model"),
		"sandbox config not in effect:\n{stdout}"
	);
}

/// Stateful stub: first request answers with a tool call, every later
/// request answers with the final MARKER response — driving the child
/// binary through a full tool round.
async fn spawn_tool_round_stub() -> String {
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
									"id": "call_e2e",
									"type": "function",
									"function": {"name": "e2e_missing_tool", "arguments": "{}"}
								}]
							},
							"finish_reason": "tool_calls"
						}],
						"usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
					})
				} else {
					serde_json::json!({
						"choices": [{
							"message": {"role": "assistant", "content": format!("{MARKER}: tool round survived")},
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

	format!("http://{}/v1/chat/completions", addr)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_run_with_tool_round_end_to_end() {
	let stub_url = spawn_tool_round_stub().await;
	let home = tempfile::tempdir().expect("temp home");
	write_sandbox_config(home.path());

	let mut child = octomind_cmd(home.path(), &stub_url)
		.args(["run", "--format", "plain"])
		.stdin(Stdio::piped())
		.stdout(Stdio::piped())
		.stderr(Stdio::piped())
		.spawn()
		.expect("spawn octomind run");
	child
		.stdin
		.take()
		.expect("stdin")
		.write_all(b"use your tool\n")
		.expect("write prompt");

	let output = child.wait_with_output().expect("octomind exits");
	let stdout = String::from_utf8_lossy(&output.stdout);
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(
		output.status.success(),
		"tool-round run failed.\nstdout:\n{stdout}\nstderr:\n{stderr}"
	);
	// The unknown tool errored inside the round, the follow-up call still
	// produced the final answer — the loop must survive tool failures.
	assert!(
		stdout.contains(MARKER),
		"final answer missing.\nstdout:\n{stdout}\nstderr:\n{stderr}"
	);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_workflow_single_step_end_to_end() {
	let stub_url = spawn_openai_stub().await;
	let home = tempfile::tempdir().expect("temp home");
	write_sandbox_config(home.path());

	// Minimal one-step workflow: the step runs `octomind run` as a child of
	// the workflow process, inside the same sandbox and against the stub.
	let workflow_path = home.path().join("e2e-workflow.toml");
	std::fs::write(
		&workflow_path,
		r#"name = "e2e"

[[steps]]
name = "answer"
role = "assistant"
session = "fresh"
prompt = "{{input}}"
"#,
	)
	.expect("write workflow");

	let mut child = octomind_cmd(home.path(), &stub_url)
		.args([
			"workflow",
			workflow_path.to_str().expect("utf8 path"),
			"--format",
			"jsonl",
		])
		.stdin(Stdio::piped())
		.stdout(Stdio::piped())
		.stderr(Stdio::piped())
		.spawn()
		.expect("spawn octomind workflow");
	child
		.stdin
		.take()
		.expect("stdin")
		.write_all(b"answer with the marker\n")
		.expect("write input");

	let output = child.wait_with_output().expect("workflow exits");
	let stdout = String::from_utf8_lossy(&output.stdout);
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(
		output.status.success(),
		"workflow failed.\nstdout:\n{stdout}\nstderr:\n{stderr}"
	);
	assert!(
		stdout.contains(MARKER),
		"step output missing marker.\nstdout:\n{stdout}\nstderr:\n{stderr}"
	);
}

/// Loop (with exit condition) + parallel fan-out + final synthesis: the
/// three orchestration shapes in one run. The stub always answers with the
/// marker, so the loop's exit_when fires on the first iteration.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_workflow_loop_and_parallel_end_to_end() {
	let stub_url = spawn_openai_stub().await;
	let home = tempfile::tempdir().expect("temp home");
	write_sandbox_config(home.path());

	let workflow_path = home.path().join("shapes-workflow.toml");
	std::fs::write(
		&workflow_path,
		format!(
			r#"name = "shapes"

[[steps]]
name           = "refine"
loop           = true
max_iterations = 2
exit_when      = {{ output = "worker", contains = "{MARKER}" }}

  [[steps.run]]
  name    = "worker"
  role    = "assistant"
  session = "fresh"
  prompt  = "work on: {{{{input}}}}"

[[steps]]
name     = "fanout"
parallel = true

  [[steps.run]]
  name   = "left"
  role   = "assistant"
  prompt = "left view of {{{{input}}}}"

  [[steps.run]]
  name   = "right"
  role   = "assistant"
  prompt = "right view of {{{{input}}}}"

[[steps]]
name   = "synthesis"
role   = "assistant"
prompt = "combine: {{{{left}}}} and {{{{right}}}} and {{{{worker}}}}"
"#
		),
	)
	.expect("write workflow");

	let mut child = octomind_cmd(home.path(), &stub_url)
		.args([
			"workflow",
			workflow_path.to_str().expect("utf8 path"),
			"--format",
			"jsonl",
		])
		.stdin(Stdio::piped())
		.stdout(Stdio::piped())
		.stderr(Stdio::piped())
		.spawn()
		.expect("spawn octomind workflow");
	child
		.stdin
		.take()
		.expect("stdin")
		.write_all(b"the task\n")
		.expect("write input");

	let output = child.wait_with_output().expect("workflow exits");
	let stdout = String::from_utf8_lossy(&output.stdout);
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(
		output.status.success(),
		"shapes workflow failed.\nstdout:\n{stdout}\nstderr:\n{stderr}"
	);
	// Every step emits a jsonl assistant event; the final synthesis step
	// must be present and carry the marker.
	assert!(
		stdout.contains("synthesis"),
		"synthesis step missing.\nstdout:\n{stdout}"
	);
	assert!(stdout.contains(MARKER));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_workflow_dry_run_prints_plan() {
	let stub_url = spawn_openai_stub().await;
	let home = tempfile::tempdir().expect("temp home");
	write_sandbox_config(home.path());
	let workflow_path = home.path().join("plan-workflow.toml");
	std::fs::write(
		&workflow_path,
		"name = \"plan\"\n\n[[steps]]\nname = \"only\"\nrole = \"assistant\"\nprompt = \"{{input}}\"\n",
	)
	.expect("write workflow");

	let output = octomind_cmd(home.path(), &stub_url)
		.args([
			"workflow",
			workflow_path.to_str().expect("utf8"),
			"--dry-run",
		])
		.output()
		.expect("dry run executes");
	assert!(
		output.status.success(),
		"dry-run failed: {}",
		String::from_utf8_lossy(&output.stderr)
	);
	let stdout = String::from_utf8_lossy(&output.stdout);
	assert!(stdout.contains("only"), "plan missing step name:\n{stdout}");
}

/// Supervisor fully enabled, with the supervisor model pointed at the same
/// stub. Every supervisor mechanic (task classify, gate, plan reconcile)
/// receives a nonsense-but-valid completion; the control plane must degrade
/// to observe-only and NEVER break the user turn.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_run_with_supervisor_enabled_survives_garbage_verdicts() {
	let stub_url = spawn_openai_stub().await;
	let home = tempfile::tempdir().expect("temp home");
	{
		let mut config: octomind::config::Config =
			toml::from_str(include_str!("../config-templates/default.toml"))
				.expect("parse default config template");
		config.model = "ollama:fake-model".to_string();
		config.default = "assistant".to_string();
		config.supervisor.enabled = true;
		config.supervisor.model = "ollama:fake-model".to_string();
		config.telemetry = false;
		config.auto_capabilities = false;
		config.skills.auto_activation = false;
		config.skills.auto_validation = false;

		let config_dir = home.path().join(".local/share/octomind/config");
		std::fs::create_dir_all(&config_dir).expect("create config dir");
		std::fs::write(
			config_dir.join("config.toml"),
			toml::to_string(&config).expect("serialize config"),
		)
		.expect("write config");
	}

	let mut child = octomind_cmd(home.path(), &stub_url)
		.args(["run", "--format", "plain"])
		.stdin(Stdio::piped())
		.stdout(Stdio::piped())
		.stderr(Stdio::piped())
		.spawn()
		.expect("spawn octomind run");
	child
		.stdin
		.take()
		.expect("stdin")
		.write_all(b"do a thing and finish\n")
		.expect("write prompt");

	let output = child.wait_with_output().expect("octomind exits");
	let stdout = String::from_utf8_lossy(&output.stdout);
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(
		output.status.success(),
		"supervised run failed.\nstdout:\n{stdout}\nstderr:\n{stderr}"
	);
	assert!(
		stdout.contains(MARKER),
		"answer missing under supervision.\nstdout:\n{stdout}\nstderr:\n{stderr}"
	);
}

/// Named session, then resume: the second run must load the persisted
/// session (restore path) instead of starting fresh.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_named_session_resume_roundtrip() {
	let stub_url = spawn_openai_stub().await;
	let home = tempfile::tempdir().expect("temp home");
	write_sandbox_config(home.path());

	for (turn, prompt) in [(1, "first turn\n"), (2, "second turn\n")] {
		let mut cmd = octomind_cmd(home.path(), &stub_url);
		if turn == 1 {
			cmd.args(["run", "--format", "plain", "-n", "resume-e2e"]);
		} else {
			cmd.args(["run", "--format", "plain", "-r", "resume-e2e"]);
		}
		let mut child = cmd
			.stdin(Stdio::piped())
			.stdout(Stdio::piped())
			.stderr(Stdio::piped())
			.spawn()
			.expect("spawn octomind run");
		child
			.stdin
			.take()
			.expect("stdin")
			.write_all(prompt.as_bytes())
			.expect("write prompt");
		let output = child.wait_with_output().expect("octomind exits");
		let stdout = String::from_utf8_lossy(&output.stdout);
		let stderr = String::from_utf8_lossy(&output.stderr);
		assert!(
			output.status.success(),
			"turn {turn} failed.\nstdout:\n{stdout}\nstderr:\n{stderr}"
		);
		assert!(stdout.contains(MARKER), "turn {turn} missing answer");
	}
}

#[test]
fn test_version_flag() {
	let output = Command::new(env!("CARGO_BIN_EXE_octomind"))
		.arg("--version")
		.output()
		.expect("octomind --version runs");
	assert!(output.status.success());
	assert!(String::from_utf8_lossy(&output.stdout).contains("octomind"));
}
