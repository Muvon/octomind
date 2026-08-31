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
		.env("OCTOMIND_DATA_DIR", home.join(".local/share/octomind"))
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

/// Bare `octomind` (no subcommand) defaults to `run`. Non-interactively that
/// must still fail loudly: empty piped stdin is an error, not a silent hang.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_bare_invocation_non_interactive_requires_input() {
	let stub_url = spawn_openai_stub().await;
	let home = tempfile::tempdir().expect("temp home");
	write_sandbox_config(home.path());

	let mut child = octomind_cmd(home.path(), &stub_url)
		.stdin(Stdio::piped())
		.stdout(Stdio::piped())
		.stderr(Stdio::piped())
		.spawn()
		.expect("spawn bare octomind");
	drop(child.stdin.take());

	let output = child.wait_with_output().expect("octomind exits");
	let stdout = String::from_utf8_lossy(&output.stdout);
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(
		!output.status.success(),
		"bare octomind with empty stdin must fail.\nstdout:\n{stdout}\nstderr:\n{stderr}"
	);
	assert!(
		stderr.contains("No input provided via stdin"),
		"expected stdin error.\nstdout:\n{stdout}\nstderr:\n{stderr}"
	);
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
		config.supervisor.model.model = Some("ollama:fake-model".to_string());
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

/// Stub that answers call N with `bodies[N]` (clamped to the last entry).
async fn spawn_scripted_stub(bodies: Vec<String>) -> String {
	use std::sync::atomic::{AtomicUsize, Ordering};
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
		.await
		.expect("bind stub");
	let addr = listener.local_addr().expect("addr");
	let counter = std::sync::Arc::new(AtomicUsize::new(0));
	let bodies = std::sync::Arc::new(bodies);

	tokio::spawn(async move {
		while let Ok((mut sock, _)) = listener.accept().await {
			let counter = counter.clone();
			let bodies = bodies.clone();
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
				let idx = counter.fetch_add(1, Ordering::SeqCst).min(bodies.len() - 1);
				let body = &bodies[idx];
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

fn completion_body(content: &str) -> String {
	serde_json::json!({
		"choices": [{
			"message": {"role": "assistant", "content": content},
			"finish_reason": "stop"
		}],
		"usage": {"prompt_tokens": 10, "completion_tokens": 8, "total_tokens": 18, "cost": 0.0001}
	})
	.to_string()
}

/// `config --model/--log-level` must persist into the sandbox config file
/// and be visible on the next `config --show`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_config_setters_roundtrip() {
	let stub_url = spawn_openai_stub().await;
	let home = tempfile::tempdir().expect("temp home");
	write_sandbox_config(home.path());

	let output = octomind_cmd(home.path(), &stub_url)
		.args([
			"config",
			"--model",
			"ollama:cfg-model",
			"--log-level",
			"debug",
		])
		.output()
		.expect("config setters run");
	assert!(
		output.status.success(),
		"config setters failed: {}",
		String::from_utf8_lossy(&output.stderr)
	);

	let output = octomind_cmd(home.path(), &stub_url)
		.args(["config", "--show"])
		.output()
		.expect("config --show runs");
	let stdout = String::from_utf8_lossy(&output.stdout);
	assert!(output.status.success());
	assert!(
		stdout.contains("ollama:cfg-model"),
		"model change not persisted:\n{stdout}"
	);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_vars_preview() {
	let stub_url = spawn_openai_stub().await;
	let home = tempfile::tempdir().expect("temp home");
	write_sandbox_config(home.path());

	for args in [vec!["vars"], vec!["vars", "--preview"]] {
		let output = octomind_cmd(home.path(), &stub_url)
			.args(&args)
			.output()
			.expect("vars runs");
		let stdout = String::from_utf8_lossy(&output.stdout);
		assert!(
			output.status.success(),
			"{args:?} failed: {}",
			String::from_utf8_lossy(&output.stderr)
		);
		assert!(stdout.contains("DATE"), "{args:?} missing DATE:\n{stdout}");
	}
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_complete_run_lists_roles() {
	let stub_url = spawn_openai_stub().await;
	let home = tempfile::tempdir().expect("temp home");
	write_sandbox_config(home.path());

	let output = octomind_cmd(home.path(), &stub_url)
		.args(["complete", "run"])
		.output()
		.expect("complete run executes");
	assert!(output.status.success());
	let stdout = String::from_utf8_lossy(&output.stdout);
	assert!(
		stdout.lines().any(|l| l.trim() == "assistant"),
		"roles missing from candidates:\n{stdout}"
	);
}

/// Full lesson-distillation pipeline against scripted verdicts: extraction
/// call returns LEARN + one lesson (whose evidence is verbatim in a user
/// turn) + one orientation; verification keeps everything. Both must land
/// in the sandbox learning store.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_distill_stores_lessons() {
	let extraction = completion_body(concat!(
		"<decision>LEARN</decision>\n",
		"<lesson scope=\"scoped\" confidence=\"high\" tags=\"testing\" ",
		"evidence=\"always run the tests on the box\">",
		"Run the tests on the box, never locally",
		"</lesson>\n",
		"<orientation tags=\"arch\" confidence=\"high\" evidence=\"M3\">",
		"The project is a Rust CLI with a workspace build",
		"</orientation>"
	));
	let verification = completion_body("{\"unsupported\": []}");
	let stub_url = spawn_scripted_stub(vec![extraction, verification]).await;

	let home = tempfile::tempdir().expect("temp home");
	{
		let mut config: octomind::config::Config =
			toml::from_str(include_str!("../config-templates/default.toml"))
				.expect("parse default config template");
		config.model = "ollama:fake-model".to_string();
		config.default = "assistant".to_string();
		config.supervisor.enabled = false;
		config.supervisor.model.model = Some("ollama:fake-model".to_string());
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

	// Transcript snapshot exactly as an exiting session writes it. The user
	// turn carries the lesson quote verbatim, while the tool observation is
	// addressable grounding for the orientation.
	let transcript = serde_json::json!([
		{"role": "user", "content": "for this repo, always run the tests on the box please", "timestamp": 1},
		{"role": "assistant", "content": "Understood — running everything on the box.", "timestamp": 2},
		{"role": "tool", "content": "Cargo.toml defines the Rust CLI workspace build", "timestamp": 3, "tool_call_id": "call-1", "name": "read"}
	]);
	let transcript_path = home.path().join("transcript.json");
	std::fs::write(&transcript_path, transcript.to_string()).expect("write transcript");

	let output = octomind_cmd(home.path(), &stub_url)
		.args([
			"distill",
			"--messages",
			transcript_path.to_str().expect("utf8 path"),
			"--role",
			"assistant",
			"--project",
			"e2eproj",
			"--session",
			"e2esess",
		])
		.output()
		.expect("distill runs");
	assert!(
		output.status.success(),
		"distill failed.\nstdout:\n{}\nstderr:\n{}",
		String::from_utf8_lossy(&output.stdout),
		String::from_utf8_lossy(&output.stderr)
	);

	// The snapshot is consumed on read
	assert!(
		!transcript_path.exists(),
		"transcript snapshot must be deleted after distillation"
	);

	// Lesson + orientation landed in the scoped learning store
	let learning_dir = home
		.path()
		.join(".local/share/octomind/learning/e2eproj/assistant");
	let mut stored = String::new();
	for entry in std::fs::read_dir(&learning_dir).expect("learning dir exists") {
		let path = entry.expect("dir entry").path();
		if path.is_file() {
			stored.push_str(&std::fs::read_to_string(&path).unwrap_or_default());
		}
	}
	assert!(
		stored.contains("Run the tests on the box"),
		"lesson missing from store:\n{stored}"
	);
	assert!(
		stored.contains("Rust CLI with a workspace build"),
		"orientation missing from store:\n{stored}"
	);
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_send_to_unknown_session_fails() {
	let stub_url = spawn_openai_stub().await;
	let home = tempfile::tempdir().expect("temp home");
	write_sandbox_config(home.path());

	let output = octomind_cmd(home.path(), &stub_url)
		.args(["send", "-n", "no-such-session-e2e", "hello?"])
		.output()
		.expect("send runs");
	assert!(
		!output.status.success(),
		"send to missing session must fail"
	);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_tap_list_and_untap_unknown() {
	let stub_url = spawn_openai_stub().await;
	let home = tempfile::tempdir().expect("temp home");
	write_sandbox_config(home.path());

	// Bare `tap` lists active taps from the sandbox (no network)
	let output = octomind_cmd(home.path(), &stub_url)
		.args(["tap"])
		.output()
		.expect("tap runs");
	assert!(
		output.status.success(),
		"tap list failed: {}",
		String::from_utf8_lossy(&output.stderr)
	);

	// Removing a tap that was never added must fail
	let output = octomind_cmd(home.path(), &stub_url)
		.args(["untap", "no-such-tap-e2e"])
		.output()
		.expect("untap runs");
	assert!(!output.status.success(), "untap of unknown tap must fail");
}

/// The jsonl and json output formats drive their own sinks through the
/// non-interactive loop; both must deliver the answer machine-readably.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_run_jsonl_and_json_formats() {
	let stub_url = spawn_openai_stub().await;
	let home = tempfile::tempdir().expect("temp home");
	write_sandbox_config(home.path());

	for format in ["jsonl", "json"] {
		let mut child = octomind_cmd(home.path(), &stub_url)
			.args(["run", "--format", format])
			.stdin(Stdio::piped())
			.stdout(Stdio::piped())
			.stderr(Stdio::piped())
			.spawn()
			.expect("spawn octomind run");
		child
			.stdin
			.take()
			.expect("stdin")
			.write_all(b"answer with the marker\n")
			.expect("write prompt");
		let output = child.wait_with_output().expect("octomind exits");
		let stdout = String::from_utf8_lossy(&output.stdout);
		let stderr = String::from_utf8_lossy(&output.stderr);
		assert!(
			output.status.success(),
			"--format {format} failed.\nstdout:\n{stdout}\nstderr:\n{stderr}"
		);
		assert!(
			stdout.contains(MARKER),
			"--format {format} missing answer.\nstdout:\n{stdout}"
		);
	}
}

/// Remaining config surfaces: markdown setters persist, validate and
/// upgrade run cleanly against the sandbox config.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_config_markdown_validate_upgrade() {
	let stub_url = spawn_openai_stub().await;
	let home = tempfile::tempdir().expect("temp home");
	write_sandbox_config(home.path());

	let output = octomind_cmd(home.path(), &stub_url)
		.args([
			"config",
			"--markdown-enable",
			"true",
			"--markdown-theme",
			"dark",
		])
		.output()
		.expect("markdown setters run");
	assert!(
		output.status.success(),
		"markdown setters failed: {}",
		String::from_utf8_lossy(&output.stderr)
	);

	let output = octomind_cmd(home.path(), &stub_url)
		.args(["config", "--show"])
		.output()
		.expect("config --show runs");
	assert!(output.status.success());
	assert!(
		String::from_utf8_lossy(&output.stdout).contains("dark"),
		"theme change not visible"
	);

	for flag in ["--validate", "--upgrade"] {
		let output = octomind_cmd(home.path(), &stub_url)
			.args(["config", flag])
			.output()
			.expect("config flag runs");
		assert!(
			output.status.success(),
			"config {flag} failed: {}",
			String::from_utf8_lossy(&output.stderr)
		);
	}
}

/// A workflow whose provider is unreachable: every step attempt fails, the
/// retry loop runs out, and the workflow must exit nonzero with diagnostics
/// — never hang or report success.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_workflow_dead_provider_fails_cleanly() {
	let home = tempfile::tempdir().expect("temp home");
	write_sandbox_config(home.path());
	let workflow_path = home.path().join("dead-provider.toml");
	std::fs::write(
		&workflow_path,
		"name = \"dead\"\n\n[[steps]]\nname = \"only\"\nrole = \"assistant\"\nsession = \"fresh\"\nprompt = \"{{input}}\"\n",
	)
	.expect("write workflow");

	// Port 9 (discard) — nothing answers; the provider call fails fast.
	let mut child = octomind_cmd(home.path(), "http://127.0.0.1:9/v1/chat/completions")
		.args(["workflow", workflow_path.to_str().expect("utf8")])
		.stdin(Stdio::piped())
		.stdout(Stdio::piped())
		.stderr(Stdio::piped())
		.spawn()
		.expect("spawn workflow");
	child
		.stdin
		.take()
		.expect("stdin")
		.write_all(b"the task\n")
		.expect("write input");
	let output = child.wait_with_output().expect("workflow exits");
	assert!(
		!output.status.success(),
		"dead-provider workflow must fail.\nstdout:\n{}\nstderr:\n{}",
		String::from_utf8_lossy(&output.stdout),
		String::from_utf8_lossy(&output.stderr)
	);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_completion_generation_and_api_key_setter() {
	let stub_url = spawn_openai_stub().await;
	let home = tempfile::tempdir().expect("temp home");
	write_sandbox_config(home.path());

	for shell in ["bash", "zsh"] {
		let output = octomind_cmd(home.path(), &stub_url)
			.args(["completion", shell])
			.output()
			.expect("completion runs");
		assert!(output.status.success(), "completion {shell} failed");
		assert!(
			!output.stdout.is_empty(),
			"completion {shell} produced nothing"
		);
	}

	// API key setter writes into the sandbox config only
	let output = octomind_cmd(home.path(), &stub_url)
		.args(["config", "--api-key", "openrouter:test-key-e2e"])
		.output()
		.expect("api-key setter runs");
	assert!(
		output.status.success(),
		"api-key setter failed: {}",
		String::from_utf8_lossy(&output.stderr)
	);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_vars_expand_and_system_prompt_setter() {
	let stub_url = spawn_openai_stub().await;
	let home = tempfile::tempdir().expect("temp home");
	write_sandbox_config(home.path());

	let output = octomind_cmd(home.path(), &stub_url)
		.args(["vars", "--expand"])
		.output()
		.expect("vars --expand runs");
	assert!(output.status.success());
	assert!(String::from_utf8_lossy(&output.stdout).contains("DATE"));

	// Custom system prompt, then reset to default — both must persist cleanly
	for system in ["my custom sandbox prompt", "default"] {
		let output = octomind_cmd(home.path(), &stub_url)
			.args(["config", "--system", system])
			.output()
			.expect("system setter runs");
		assert!(
			output.status.success(),
			"config --system {system} failed: {}",
			String::from_utf8_lossy(&output.stderr)
		);
	}
}
