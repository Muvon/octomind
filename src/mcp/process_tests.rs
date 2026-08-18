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

// The registries here are process globals — every test uses unique server
// names so parallel tests never observe each other's state.

use super::*;

#[test]
fn test_restart_info_defaults() {
	let info = ServerRestartInfo::default();
	assert_eq!(info.restart_count, 0);
	assert_eq!(info.consecutive_failures, 0);
	assert_eq!(info.health_status, ServerHealth::Running);
	assert!(info.last_restart_time.is_none());
	assert!(info.last_health_check.is_none());
}

#[test]
fn test_unknown_server_reads_are_safe() {
	// Health for a server nobody registered → Dead, not a panic
	assert_eq!(get_server_health("proc-test-unknown"), ServerHealth::Dead);
	assert_eq!(
		get_server_restart_info("proc-test-unknown").restart_count,
		0
	);
	// Resetting an untracked server is an explicit error
	assert!(reset_server_failure_state("proc-test-unknown").is_err());
	// No pgid registered → liveness is unknowable
	assert!(is_stdio_process_alive("proc-test-unknown").is_none());
	// No stderr captured → empty diagnostics
	assert!(stderr_lines_for("proc-test-unknown").is_empty());
}

#[test]
fn test_get_server_url() {
	let http = McpServerConfig::http("proc-test-http", "http://example.com/mcp", 30, Vec::new());
	assert_eq!(
		get_server_url(&http).expect("http url"),
		"http://example.com/mcp"
	);

	let stdio = McpServerConfig::stdin("proc-test-stdio", "cat", Vec::new(), 30, Vec::new());
	assert_eq!(
		get_server_url(&stdio).expect("stdio url"),
		"stdin://proc-test-stdio"
	);
}

#[test]
fn test_stderr_buffer_is_shared() {
	let name = "proc-test-stderr";
	let buffer = stderr_buffer_for(name);
	buffer
		.lock()
		.expect("stderr buffer lock")
		.push("boom".to_string());
	// A second lookup sees the same underlying buffer
	assert_eq!(stderr_lines_for(name), vec!["boom".to_string()]);
}

#[test]
fn test_restart_mutex_identity() {
	let name = "proc-test-mutex";
	let first = get_server_restart_mutex(name);
	let second = get_server_restart_mutex(name);
	assert!(Arc::ptr_eq(&first, &second));

	cleanup_server_restart_mutex(name);
	let third = get_server_restart_mutex(name);
	assert!(!Arc::ptr_eq(&first, &third));
}

#[cfg(unix)]
#[test]
fn test_stdio_liveness_tracks_real_process() {
	let name = "proc-test-liveness";
	let mut child = std::process::Command::new("sleep")
		.arg("5")
		.spawn()
		.expect("spawn sleep");
	register_pgid(name, child.id());
	assert_eq!(is_stdio_process_alive(name), Some(true));

	child.kill().expect("kill child");
	child.wait().expect("reap child");
	// Reaped pid → kill(pid, 0) fails → reported dead
	assert_eq!(is_stdio_process_alive(name), Some(false));
}
