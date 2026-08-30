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

//! `run()`/`serve()` coverage via child-process fixtures.
//!
//! `serve()` is hardwired to the process's real stdin/stdout, so the only
//! honest way to exercise the full stdio bridge is to re-execute this test
//! binary as a child (`--exact`, selected by `OCTOMIND_ACP_TEST_FIXTURE`)
//! and drive it over pipes from the parent test.

use super::*;
use std::time::Duration;

use agent_client_protocol::schema::v1::{
	Implementation, InitializeRequest, NewSessionRequest, PromptRequest,
};
use agent_client_protocol::schema::ProtocolVersion;

fn fixture_name() -> Option<String> {
	std::env::var("OCTOMIND_ACP_TEST_FIXTURE")
		.ok()
		.filter(|value| !value.is_empty())
}

/// The child half: serves real stdio ACP until the parent closes the pipe.
/// Without the fixture env var (the normal `cargo test` run) it is a no-op.
#[tokio::test(flavor = "current_thread")]
#[serial_test::serial]
async fn acp_stdio_child_fixture() {
	let Some(_fixture) = fixture_name() else {
		return;
	};
	let mut config = crate::session::chat::test_support::fake_provider_config();
	config.compression.decision.model = "ollama:fake-model".to_string();
	let result = run(config, "assistant".to_string(), Default::default()).await;
	result.expect("acp run completes cleanly on stdin EOF");
}

// ---- parent-side harness ----

struct ChildAcp {
	child: tokio::process::Child,
	stdin: tokio::process::ChildStdin,
	stdout: tokio::io::BufReader<tokio::process::ChildStdout>,
}

fn spawn_fixture(
	fixture: &str,
	data_dir: &std::path::Path,
	extra_env: &[(&str, &str)],
) -> ChildAcp {
	let exe = std::env::current_exe().expect("test binary path");
	let mut command = tokio::process::Command::new(exe);
	command.args([
		"--exact",
		"acp::tests::acp_stdio_child_fixture",
		"--nocapture",
	]);
	command.env("OCTOMIND_ACP_TEST_FIXTURE", fixture);
	command.env("OCTOMIND_DATA_DIR", data_dir);
	for (key, value) in extra_env {
		command.env(key, value);
	}
	command.stdin(std::process::Stdio::piped());
	command.stdout(std::process::Stdio::piped());
	command.stderr(std::process::Stdio::null());
	let mut child = command.spawn().expect("spawn child fixture");
	let stdin = child.stdin.take().expect("child stdin");
	let stdout = child.stdout.take().expect("child stdout");
	ChildAcp {
		child,
		stdin,
		stdout: tokio::io::BufReader::new(stdout),
	}
}

impl ChildAcp {
	async fn send(&mut self, line: &str) {
		use tokio::io::AsyncWriteExt;
		self.stdin
			.write_all(line.as_bytes())
			.await
			.expect("write request to child");
		self.stdin.write_all(b"\n").await.expect("newline");
		self.stdin.flush().await.expect("flush");
	}

	async fn close_stdin(&mut self) {
		use tokio::io::AsyncWriteExt;
		let _ = self.stdin.shutdown().await;
	}

	/// Reads stdout lines until a JSON-RPC frame arrives, skipping libtest
	/// progress noise (`--nocapture` interleaves it with the protocol).
	async fn next_rpc(&mut self) -> serde_json::Value {
		use tokio::io::AsyncBufReadExt;
		loop {
			let mut line = String::new();
			let read =
				tokio::time::timeout(Duration::from_secs(30), self.stdout.read_line(&mut line))
					.await
					.expect("child must not stall")
					.expect("child stdout must stay open");
			assert!(read > 0, "child stdout ended before the expected frame");
			let trimmed = line.trim();
			if trimmed.is_empty() {
				continue;
			}
			if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
				if value.get("jsonrpc").is_some() {
					return value;
				}
			}
		}
	}

	async fn wait_for_id(&mut self, id: u64) -> serde_json::Value {
		loop {
			let frame = self.next_rpc().await;
			if frame["id"] == id {
				return frame;
			}
		}
	}

	async fn wait_exit(&mut self) -> std::process::ExitStatus {
		tokio::time::timeout(Duration::from_secs(30), self.child.wait())
			.await
			.expect("child must exit after stdin EOF")
			.expect("child must be waitable")
	}
}

fn rpc_line(id: u64, method: &str, params: serde_json::Value) -> String {
	serde_json::json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string()
}

fn initialize_params() -> serde_json::Value {
	serde_json::to_value(
		InitializeRequest::new(ProtocolVersion::LATEST)
			.client_info(Implementation::new("e2e-client", "0.0.1")),
	)
	.expect("serialize initialize request")
}

#[tokio::test(flavor = "current_thread")]
async fn acp_stdio_serves_initialize_new_session_prompt_and_eof() {
	let data = tempfile::tempdir().expect("tempdir");
	let stub_url = crate::session::chat::test_support::spawn_stub(vec![
		crate::session::chat::test_support::final_response("ACP-E2E-DONE"),
	])
	.await;
	let mut child = spawn_fixture("serve", data.path(), &[("OLLAMA_API_URL", &stub_url)]);

	// 1. initialize
	child
		.send(&rpc_line(1, "initialize", initialize_params()))
		.await;
	let response = child.wait_for_id(1).await;
	assert!(
		response["result"]["agentInfo"].is_object(),
		"got: {response}"
	);

	// 2. session/new
	let new_session = serde_json::to_value(NewSessionRequest::new(data.path().to_path_buf()))
		.expect("serialize session/new");
	child.send(&rpc_line(2, "session/new", new_session)).await;
	let response = child.wait_for_id(2).await;
	let session_id = response["result"]["sessionId"]
		.as_str()
		.expect("session id in response")
		.to_string();

	// 3. session/prompt — streaming notifications must arrive before the
	//    response, proving the forwarder bridge works.
	let prompt = serde_json::to_value(PromptRequest::new(session_id.clone(), vec!["hello".into()]))
		.expect("serialize session/prompt");
	child.send(&rpc_line(3, "session/prompt", prompt)).await;
	let mut updates = 0;
	loop {
		let frame = child.next_rpc().await;
		if frame["id"] == 3 {
			assert_eq!(frame["result"]["stopReason"], "end_turn", "got: {frame}");
			break;
		}
		if frame["method"] == "session/update" {
			updates += 1;
		}
	}
	assert!(
		updates > 0,
		"the turn must stream session/update notifications"
	);

	// 4. stdin EOF ends the process cleanly.
	child.close_stdin().await;
	let status = child.wait_exit().await;
	assert!(status.success(), "clean exit expected: {status}");
}

#[tokio::test(flavor = "current_thread")]
async fn acp_tracing_failure_is_recorded_in_the_init_error_log() {
	let data = tempfile::tempdir().expect("tempdir");
	// `logs/acp-debug.log` exists as a DIRECTORY: the tracing file sink
	// cannot be created, but the logs dir itself stays writable so the
	// fallback error log can be written.
	std::fs::create_dir_all(data.path().join("logs")).expect("logs dir");
	std::fs::create_dir(data.path().join("logs/acp-debug.log")).expect("sabotage tracing sink");

	let mut child = spawn_fixture("tracing_dir", data.path(), &[]);
	child
		.send(&rpc_line(1, "initialize", initialize_params()))
		.await;
	let response = child.wait_for_id(1).await;
	assert!(
		response["result"]["agentInfo"].is_object(),
		"logging failures must not break the protocol: {response}"
	);
	child.close_stdin().await;
	let status = child.wait_exit().await;
	assert!(status.success(), "clean exit expected: {status}");

	let error_log = std::fs::read_to_string(data.path().join("logs/acp-init-errors.log"))
		.expect("the init error fallback log must exist");
	assert!(
		error_log.contains("Failed to initialize tracing"),
		"got: {error_log}"
	);
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn acp_unwritable_data_dir_still_serves_and_exits_cleanly() {
	let data = tempfile::tempdir().expect("tempdir");
	// A read-only data dir with no logs subdir: both get_logs_dir() callers
	// fail, so the tracing init AND the ACP error sink init both fail —
	// and the server must still serve.
	use std::os::unix::fs::PermissionsExt;
	std::fs::set_permissions(data.path(), PermissionsExt::from_mode(0o555))
		.expect("chmod read-only");

	let mut child = spawn_fixture("readonly_data", data.path(), &[]);
	child
		.send(&rpc_line(1, "initialize", initialize_params()))
		.await;
	let response = child.wait_for_id(1).await;
	assert!(
		response["result"]["agentInfo"].is_object(),
		"init failures must be non-fatal: {response}"
	);
	child.close_stdin().await;
	let status = child.wait_exit().await;
	assert!(status.success(), "clean exit expected: {status}");

	std::fs::set_permissions(data.path(), PermissionsExt::from_mode(0o755))
		.expect("restore permissions for tempdir cleanup");
}
