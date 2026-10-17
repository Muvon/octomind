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

//! Windows-only: cleaning up a stdio MCP server must also end the processes
//! it spawned (`npx` → `node`). Unix gets this from process groups; Windows
//! from the per-server Job Object. Uses the stub server in
//! tests/fixtures/mcp_stub_server.py, whose `spawn_child` tool starts a
//! long-lived child and reports both pids.
#![cfg(windows)]

use std::time::{Duration, Instant};

use octomind::config::McpServerConfig;
use octomind::mcp::McpToolCall;

/// How long cleanup gets to take the process tree down.
const EXIT_DEADLINE: Duration = Duration::from_secs(10);

/// Probe via `tasklist` so the test needs no Win32 bindings of its own. The
/// image filter keeps a recycled pid from matching an unrelated process (and
/// from being force-killed by the leak guard below).
fn is_running(pid: u32) -> bool {
	let output = std::process::Command::new("tasklist")
		.args([
			"/FI",
			&format!("PID eq {pid}"),
			"/FI",
			"IMAGENAME eq python.exe",
			"/FO",
			"CSV",
			"/NH",
		])
		.output()
		.expect("run tasklist");
	String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\""))
}

#[tokio::test]
async fn stdio_server_process_tree_is_gone_after_cleanup() {
	let name = "stub_process_tree";
	let script = concat!(
		env!("CARGO_MANIFEST_DIR"),
		"/tests/fixtures/mcp_stub_server.py"
	);
	let server = McpServerConfig::stdin(name, "python", vec![script.to_string()], 30, Vec::new());

	let call = McpToolCall {
		tool_name: "spawn_child".to_string(),
		parameters: serde_json::json!({}),
		tool_id: "t-spawn".to_string(),
	};
	let result = octomind::mcp::client::call_tool(&server, &call, None)
		.await
		.expect("spawn_child against stub server");
	let text: String = result
		.content
		.iter()
		.filter_map(|block| match block {
			rmcp::model::ContentBlock::Text(t) => Some(t.text.clone()),
			_ => None,
		})
		.collect();
	let pids: Vec<u32> = text
		.split_whitespace()
		.map(|p| p.parse().expect("pid"))
		.collect();
	let [server_pid, child_pid] = pids[..] else {
		panic!("expected \"<server pid> <child pid>\", got {text:?}");
	};
	assert!(
		is_running(server_pid),
		"server must be running before cleanup"
	);
	assert!(
		is_running(child_pid),
		"child must be running before cleanup"
	);

	octomind::mcp::process::cleanup_server_process(name).expect("cleanup stub server");

	let started = Instant::now();
	while (is_running(server_pid) || is_running(child_pid)) && started.elapsed() < EXIT_DEADLINE {
		tokio::time::sleep(Duration::from_millis(200)).await;
	}
	let server_alive = is_running(server_pid);
	let child_alive = is_running(child_pid);
	// Never leak the stub's processes into the rest of the CI run.
	for (pid, alive) in [(server_pid, server_alive), (child_pid, child_alive)] {
		if alive {
			let _ = std::process::Command::new("taskkill")
				.args(["/F", "/PID", &pid.to_string()])
				.output();
		}
	}
	assert!(
		!server_alive,
		"server process {server_pid} survived cleanup"
	);
	assert!(!child_alive, "child process {child_pid} survived cleanup");
}
