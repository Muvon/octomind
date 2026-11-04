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
	assert_eq!(get_server_restart_info("proc-test-unknown").restart_count, 0);
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
	assert_eq!(get_server_url(&http).expect("http url"), "http://example.com/mcp");

	let stdio = McpServerConfig::stdin("proc-test-stdio", "cat", Vec::new(), 30, Vec::new());
	assert_eq!(get_server_url(&stdio).expect("stdio url"), "stdin://proc-test-stdio");
}

#[serial_test::serial]
#[test]
fn test_stderr_buffer_is_shared() {
	let name = "proc-test-stderr";
	let buffer = stderr_buffer_for(name);
	buffer.lock().expect("stderr buffer lock").push("boom".to_string());
	// A second lookup sees the same underlying buffer
	assert_eq!(stderr_lines_for(name), vec!["boom".to_string()]);
}

#[serial_test::serial]
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

#[serial_test::serial]
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

#[test]
fn project_id_is_derived_once_per_directory() {
	let repo = tempfile::tempdir().expect("temp repo");
	let git = |args: &[&str]| {
		let status = std::process::Command::new("git")
			.args(args)
			.current_dir(repo.path())
			.status()
			.expect("run git");
		assert!(status.success());
	};
	git(&["init", "-q"]);
	git(&["remote", "add", "origin", "https://github.com/muvon/first.git"]);
	let first = derive_project_id_from_path(repo.path());
	assert_eq!(first, octolib::utils::path_to_id(repo.path()));

	git(&["remote", "set-url", "origin", "https://github.com/muvon/second.git"]);
	assert_ne!(octolib::utils::path_to_id(repo.path()), first);
	assert_eq!(derive_project_id_from_path(repo.path()), first);
}

#[cfg(target_os = "linux")]
#[test]
fn test_workload_runs_niced_and_first_for_the_oom_killer() {
	let read_own = |field: &str| -> i32 {
		let stat = std::fs::read_to_string(format!("/proc/self/{field}")).expect("read /proc/self");
		if field == "stat" {
			// Niceness is field 19; field 2 (comm) may hold spaces, so count from its ')'.
			let after_comm = &stat[stat.rfind(')').expect("comm end") + 2..];
			after_comm
				.split(' ')
				.nth(16)
				.expect("nice field")
				.parse()
				.expect("nice value")
		} else {
			stat.trim().parse().expect("oom_score_adj value")
		}
	};
	let parent_nice = read_own("stat");
	let parent_oom = read_own("oom_score_adj");

	// The child reports its own values: `cut` and `cat` inherit the shell's.
	let mut cmd = std::process::Command::new("sh");
	cmd.args([
		"-c",
		"cut -d')' -f2 /proc/self/stat | cut -d' ' -f18; cat /proc/self/oom_score_adj",
	]);
	run_as_workload(&mut cmd);
	let out = cmd.output().expect("run workload");
	assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
	let text = String::from_utf8(out.stdout).expect("utf8");
	let values: Vec<i32> = text.lines().map(|l| l.trim().parse().expect("number")).collect();

	assert_eq!(
		values[0],
		(parent_nice + WORKLOAD_NICE_INCREMENT).min(19),
		"workload niceness"
	);
	// A score already above 500 cannot be lowered without CAP_SYS_RESOURCE.
	assert_eq!(values[1], parent_oom.max(500), "workload oom_score_adj");
}
