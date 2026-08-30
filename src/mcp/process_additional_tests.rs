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

// Additional unit tests for src/mcp/process.rs, complementing process_tests.rs:
// session context, CLI notification buffering, server capabilities, process
// lifecycle, and health-report helpers. The registries here are process
// globals — every test uses unique server names so parallel tests never
// observe each other's state; tests that mutate CLI-mode globals (session
// context, notification sender) are #[serial].

use super::*;
use serial_test::serial;

fn seed_restart_info(name: &str, f: impl FnOnce(&mut ServerRestartInfo)) {
	let mut guard = SERVER_RESTART_INFO.write().unwrap();
	let info = guard.entry(name.to_string()).or_default();
	f(info);
}

fn clear_restart_info(name: &str) {
	SERVER_RESTART_INFO.write().unwrap().remove(name);
}

#[test]
fn test_server_health_variants_are_distinct_and_debuggable() {
	let variants = [
		ServerHealth::Running,
		ServerHealth::Dead,
		ServerHealth::Restarting,
		ServerHealth::Failed,
		ServerHealth::Unreachable,
	];
	for (i, left) in variants.iter().enumerate() {
		for right in variants.iter().skip(i + 1) {
			assert_ne!(left, right);
		}
	}
	assert_eq!(format!("{:?}", ServerHealth::Running), "Running");
	assert_eq!(format!("{:?}", ServerHealth::Dead), "Dead");
	assert_eq!(format!("{:?}", ServerHealth::Restarting), "Restarting");
	assert_eq!(format!("{:?}", ServerHealth::Failed), "Failed");
	assert_eq!(format!("{:?}", ServerHealth::Unreachable), "Unreachable");
}

#[test]
fn test_server_restart_info_clone_preserves_custom_fields() {
	let now = SystemTime::now();
	let info = ServerRestartInfo {
		restart_count: 3,
		last_restart_time: Some(now),
		health_status: ServerHealth::Failed,
		consecutive_failures: 2,
		last_health_check: Some(now),
	};
	let clone = info.clone();
	assert_eq!(clone.restart_count, 3);
	assert_eq!(clone.last_restart_time, Some(now));
	assert_eq!(clone.health_status, ServerHealth::Failed);
	assert_eq!(clone.consecutive_failures, 2);
	assert_eq!(clone.last_health_check, Some(now));
}

#[test]
fn test_derive_project_id_from_path_is_stable_and_path_sensitive() {
	let a = std::path::Path::new("/tmp/octomind-proc-add-a");
	let first = derive_project_id_from_path(a);
	let second = derive_project_id_from_path(a);
	assert_eq!(first, second, "same path must derive the same project id");
	assert_eq!(first.len(), 16, "project id is a 16-hex-char digest");
	assert!(first.chars().all(|c| c.is_ascii_hexdigit()));
	let b = std::path::Path::new("/tmp/octomind-proc-add-b");
	assert_ne!(first, derive_project_id_from_path(b));
}

#[test]
fn test_derive_project_id_matches_cwd_variant() {
	let cwd = std::env::current_dir().expect("current dir");
	assert_eq!(derive_project_id(), derive_project_id_from_path(&cwd));
}

#[serial]
#[test]
fn test_session_context_round_trip_and_role_splitting() {
	let (prev_domain, prev_spec, prev_project, _, prev_workdir) = get_session_context();
	let prev_role = if prev_spec.is_empty() {
		prev_domain
	} else {
		format!("{prev_domain}:{prev_spec}")
	};

	set_session_context("developer:general", "proj-ctx-1", "/tmp/w1");
	let (domain, spec, project, session_id, workdir) = get_session_context();
	assert_eq!(
		(
			domain.as_str(),
			spec.as_str(),
			project.as_str(),
			session_id.as_str(),
			workdir.as_str()
		),
		("developer", "general", "proj-ctx-1", "", "/tmp/w1")
	);

	// Local role without a spec — spec must come back empty
	set_session_context("developer", "proj-ctx-2", "/tmp/w2");
	let (domain, spec, project, _, workdir) = get_session_context();
	assert_eq!(
		(
			domain.as_str(),
			spec.as_str(),
			project.as_str(),
			workdir.as_str()
		),
		("developer", "", "proj-ctx-2", "/tmp/w2")
	);

	// Only the first colon splits — the rest stays in the spec
	set_session_context("doctor:blood:panel", "proj-ctx-3", "/tmp/w3");
	let (domain, spec, _, _, _) = get_session_context();
	assert_eq!((domain.as_str(), spec.as_str()), ("doctor", "blood:panel"));

	set_session_context(&prev_role, &prev_project, &prev_workdir);
}

#[serial]
#[test]
fn test_init_session_context_derives_project_and_workdir() {
	let (prev_domain, prev_spec, prev_project, _, prev_workdir) = get_session_context();
	let prev_role = if prev_spec.is_empty() {
		prev_domain
	} else {
		format!("{prev_domain}:{prev_spec}")
	};

	init_session_context("tester:spec");
	let (domain, spec, project, _, workdir) = get_session_context();
	assert_eq!(domain, "tester");
	assert_eq!(spec, "spec");
	assert_eq!(project, derive_project_id(), "project id derives from cwd");
	let cwd = std::env::current_dir().expect("current dir");
	assert_eq!(workdir, cwd.to_string_lossy());

	set_session_context(&prev_role, &prev_project, &prev_workdir);
}

#[test]
fn test_stderr_buffer_for_isolates_servers_and_preserves_order() {
	let alpha = stderr_buffer_for("proc-add-stderr-alpha");
	let beta = stderr_buffer_for("proc-add-stderr-beta");
	alpha
		.lock()
		.expect("alpha stderr lock")
		.push("first".to_string());
	beta.lock()
		.expect("beta stderr lock")
		.push("other-server".to_string());
	alpha
		.lock()
		.expect("alpha stderr lock")
		.push("second".to_string());
	assert_eq!(
		stderr_lines_for("proc-add-stderr-alpha"),
		vec!["first".to_string(), "second".to_string()]
	);
	assert_eq!(
		stderr_lines_for("proc-add-stderr-beta"),
		vec!["other-server".to_string()]
	);
}

#[test]
fn test_server_capabilities_round_trip() {
	const NAME: &str = "proc-add-caps";
	let info = rmcp::model::ServerPeerInfo::new(
		rmcp::model::ProtocolVersion::V_2026_07_28,
		rmcp::model::ServerCapabilities::default(),
	)
	.with_server_info(rmcp::model::Implementation::new("peer-server", "3.2.1"))
	.with_instructions("Call tools one at a time.".to_string());

	store_server_capabilities(NAME, info.clone());
	assert_eq!(get_server_capabilities(NAME), Some(info));
	assert_eq!(
		get_server_instructions(NAME),
		Some("Call tools one at a time.".to_string())
	);
	assert!(get_server_capabilities("proc-add-caps-unknown").is_none());
	assert!(get_server_instructions("proc-add-caps-unknown").is_none());
}

#[serial]
#[tokio::test]
async fn test_emit_notification_buffers_until_sender_registered() {
	use crate::websocket::{McpNotificationPayload, ServerMessage};

	clear_notification_sender(None);

	let params = serde_json::json!({"level": "info"});
	emit_notification(
		"proc-add-notify",
		"notifications/message",
		&params,
		None,
		Some("tool-42".to_string()),
	);

	let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
	set_notification_sender(None, tx);

	// The buffered notification is flushed on registration
	match rx.try_recv().expect("buffered notification must flush") {
		ServerMessage::McpNotification(p) => {
			assert_eq!(p.server, "proc-add-notify");
			assert_eq!(p.method, "notifications/message");
			assert_eq!(p.params, params);
			assert_eq!(p.tool_id.as_deref(), Some("tool-42"));
		}
		other => panic!("expected McpNotification, got {:?}", other),
	}

	// With a sender active, emissions go straight to the channel
	emit_notification(
		"proc-add-notify",
		"notifications/progress",
		&serde_json::json!({}),
		None,
		None,
	);
	match rx.try_recv().expect("direct notification must arrive") {
		ServerMessage::McpNotification(p) => {
			assert_eq!(p.method, "notifications/progress");
			assert_eq!(p.tool_id, None);
		}
		other => panic!("expected McpNotification, got {:?}", other),
	}

	// send_notification_message uses the same CLI channel
	send_notification_message(ServerMessage::McpNotification(McpNotificationPayload {
		server: "proc-add-notify".to_string(),
		method: "notifications/initialized".to_string(),
		params: serde_json::json!({}),
		tool_id: None,
	}));
	assert!(matches!(
		rx.try_recv(),
		Ok(ServerMessage::McpNotification(_))
	));

	// After the sender is cleared, messages are dropped without panicking
	clear_notification_sender(None);
	send_notification_message(ServerMessage::McpNotification(McpNotificationPayload {
		server: "proc-add-notify".to_string(),
		method: "notifications/initialized".to_string(),
		params: serde_json::json!({}),
		tool_id: None,
	}));
	assert!(rx.try_recv().is_err());

	// A late registration must not re-flush anything
	let (tx2, mut rx2) = tokio::sync::mpsc::unbounded_channel();
	set_notification_sender(None, tx2);
	assert!(rx2.try_recv().is_err());
	clear_notification_sender(None);
}

#[cfg(unix)]
#[test]
fn test_register_pgid_overwrites_latest_pid() {
	let name = "proc-add-pgid";
	let mut first = std::process::Command::new("sleep")
		.arg("5")
		.spawn()
		.expect("spawn first sleep");
	register_pgid(name, first.id());
	assert_eq!(is_stdio_process_alive(name), Some(true));

	let mut second = std::process::Command::new("sleep")
		.arg("5")
		.spawn()
		.expect("spawn second sleep");
	register_pgid(name, second.id());
	// Killing the FIRST child must not flip liveness — the registry tracks
	// the most recently registered pid.
	first.kill().expect("kill first");
	first.wait().expect("reap first");
	assert_eq!(is_stdio_process_alive(name), Some(true));

	second.kill().expect("kill second");
	second.wait().expect("reap second");
	assert_eq!(is_stdio_process_alive(name), Some(false));
}

#[cfg(unix)]
#[test]
fn test_server_process_http_lifecycle_kill_and_reap() {
	let child = std::process::Command::new("sleep")
		.arg("30")
		.spawn()
		.expect("spawn sleep");
	let mut process = ServerProcess::Http(child);
	assert!(process
		.try_wait()
		.expect("try_wait while running")
		.is_none());
	process.kill().expect("kill http process");

	let mut exit = None;
	for _ in 0..500 {
		if let Some(status) = process.try_wait().expect("try_wait after kill") {
			exit = Some(status);
			break;
		}
		std::thread::sleep(Duration::from_millis(10));
	}
	assert!(exit.is_some(), "http process must terminate after kill");
}

#[test]
fn test_is_server_running_unknown_server_records_dead_health() {
	const NAME: &str = "proc-add-unknown-running";
	assert!(!is_server_running(NAME));
	assert_eq!(get_server_health(NAME), ServerHealth::Dead);
	let info = get_server_restart_info(NAME);
	assert!(
		info.last_health_check.is_some(),
		"probe must record the check time"
	);
	clear_restart_info(NAME);
}

#[cfg(unix)]
#[test]
fn test_is_server_running_reports_tracked_live_process() {
	const NAME: &str = "proc-add-live-process";
	let child = std::process::Command::new("sleep")
		.arg("30")
		.spawn()
		.expect("spawn sleep");
	let process_arc = Arc::new(Mutex::new(ServerProcess::Http(child)));
	SERVER_PROCESSES
		.write()
		.unwrap()
		.insert(NAME.to_string(), process_arc.clone());

	assert!(is_server_running(NAME));
	assert_eq!(get_server_health(NAME), ServerHealth::Running);

	SERVER_PROCESSES.write().unwrap().remove(NAME);
	{
		let mut guard = process_arc.lock().unwrap();
		let _ = guard.kill();
	}
	clear_restart_info(NAME);
}

#[test]
fn test_reset_server_failure_state_resets_tracked_server() {
	const NAME: &str = "proc-add-reset";
	seed_restart_info(NAME, |info| {
		info.restart_count = 4;
		info.consecutive_failures = 3;
		info.health_status = ServerHealth::Failed;
	});
	reset_server_failure_state(NAME).expect("tracked server must reset");
	let info = get_server_restart_info(NAME);
	assert_eq!(info.restart_count, 0);
	assert_eq!(info.consecutive_failures, 0);
	assert_eq!(info.health_status, ServerHealth::Dead);
	clear_restart_info(NAME);
}

#[test]
fn test_get_server_status_report_reflects_restart_info() {
	const ALPHA: &str = "proc-add-report-alpha";
	const BETA: &str = "proc-add-report-beta";
	seed_restart_info(ALPHA, |info| {
		info.restart_count = 2;
		info.health_status = ServerHealth::Restarting;
	});
	seed_restart_info(BETA, |info| {
		info.consecutive_failures = 1;
		info.health_status = ServerHealth::Unreachable;
	});

	let report = get_server_status_report();
	let (alpha_health, alpha_info) = report.get(ALPHA).expect("alpha in report");
	assert_eq!(*alpha_health, ServerHealth::Restarting);
	assert_eq!(alpha_info.restart_count, 2);
	let (beta_health, beta_info) = report.get(BETA).expect("beta in report");
	assert_eq!(*beta_health, ServerHealth::Unreachable);
	assert_eq!(beta_info.consecutive_failures, 1);

	clear_restart_info(ALPHA);
	clear_restart_info(BETA);
}

#[cfg(unix)]
#[tokio::test]
async fn test_perform_health_check_all_servers_reports_registered_processes() {
	const NAME: &str = "proc-add-sweep";
	let child = std::process::Command::new("sleep")
		.arg("30")
		.spawn()
		.expect("spawn sleep");
	let process_arc = Arc::new(Mutex::new(ServerProcess::Http(child)));
	SERVER_PROCESSES
		.write()
		.unwrap()
		.insert(NAME.to_string(), process_arc.clone());

	let health = perform_health_check_all_servers().await;
	assert_eq!(health.get(NAME), Some(&ServerHealth::Running));

	SERVER_PROCESSES.write().unwrap().remove(NAME);
	{
		let mut guard = process_arc.lock().unwrap();
		let _ = guard.kill();
	}
	clear_restart_info(NAME);
}

#[test]
fn test_cleanup_server_process_unknown_name_is_error() {
	assert!(cleanup_server_process("proc-add-cleanup-unknown").is_err());
}

#[cfg(unix)]
#[test]
fn test_cleanup_server_process_kills_tracked_process() {
	const NAME: &str = "proc-add-cleanup-tracked";
	let child = std::process::Command::new("sleep")
		.arg("30")
		.spawn()
		.expect("spawn sleep");
	SERVER_PROCESSES.write().unwrap().insert(
		NAME.to_string(),
		Arc::new(Mutex::new(ServerProcess::Http(child))),
	);

	cleanup_server_process(NAME).expect("tracked process must clean up");
	assert!(!SERVER_PROCESSES.read().unwrap().contains_key(NAME));
	// Registry entry is gone — a second cleanup is an explicit error
	assert!(cleanup_server_process(NAME).is_err());
}

#[tokio::test]
async fn test_ensure_server_running_marks_failed_when_spawn_fails() {
	const NAME: &str = "proc-add-spawn-fail";
	let server = McpServerConfig::stdin(
		NAME,
		"definitely-not-a-real-binary",
		Vec::new(),
		2,
		Vec::new(),
	);
	assert!(ensure_server_running(&server).await.is_err());
	let info = get_server_restart_info(NAME);
	assert_eq!(info.health_status, ServerHealth::Failed);
	assert_eq!(info.consecutive_failures, 1);
	clear_restart_info(NAME);
}

// ---------------------------------------------------------------------------
// Session-scoped context and notification routing
// ---------------------------------------------------------------------------

#[serial]
#[tokio::test]
async fn test_get_session_context_prefers_session_scoped_role_and_anchor() {
	let sid = "proc-add-ctx-session".to_string();
	let anchor = std::path::PathBuf::from("/tmp/octomind-proc-add-anchor");
	crate::session::context::with_session_id(sid.clone(), async {
		crate::session::context::set_session_role(&sid, "doctor:blood");
		crate::mcp::workdir::set_session_working_directory(anchor.clone());

		let (domain, spec, project, session_id, workdir) = get_session_context();
		assert_eq!(domain, "doctor");
		assert_eq!(spec, "blood");
		assert_eq!(project, derive_project_id_from_path(&anchor));
		assert_eq!(session_id, sid);
		assert_eq!(workdir, anchor.to_string_lossy());
	})
	.await;
	crate::session::context::clear_session_role(&sid);
	crate::session::context::clear_session_workdir(&sid);
}

#[serial]
#[tokio::test]
async fn test_notification_sender_session_scoped_round_trip() {
	let sid = "proc-add-notify-session".to_string();
	crate::session::context::with_session_id(sid.clone(), async {
		let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
		set_notification_sender(Some(sid.clone()), tx);

		send_notification_message(crate::websocket::ServerMessage::Thinking(
			crate::websocket::ThinkingPayload {
				content: "live".to_string(),
				session_id: sid.clone(),
			},
		));
		match rx.try_recv().expect("session sender must receive") {
			crate::websocket::ServerMessage::Thinking(p) => assert_eq!(p.content, "live"),
			_ => panic!("expected Thinking"),
		}

		clear_notification_sender(Some(sid.clone()));
		send_notification_message(crate::websocket::ServerMessage::Thinking(
			crate::websocket::ThinkingPayload {
				content: "dropped".to_string(),
				session_id: sid.clone(),
			},
		));
		assert!(rx.try_recv().is_err(), "cleared sender must not receive");
	})
	.await;
}

// ---------------------------------------------------------------------------
// ensure_server_running — alive short-circuit and non-spawnable types
// ---------------------------------------------------------------------------

#[cfg(unix)]
#[serial]
#[tokio::test]
async fn test_ensure_server_running_returns_url_for_live_http_process() {
	const NAME: &str = "proc-add-live-http";
	let child = std::process::Command::new("sleep")
		.arg("30")
		.spawn()
		.expect("spawn sleep");
	let process_arc = Arc::new(Mutex::new(ServerProcess::Http(child)));
	SERVER_PROCESSES
		.write()
		.unwrap()
		.insert(NAME.to_string(), process_arc.clone());

	let server = McpServerConfig::http(NAME, "http://127.0.0.1:9/mcp", 2, Vec::new());
	let url = ensure_server_running(&server)
		.await
		.expect("live process must be reused, not restarted");
	assert_eq!(url, "http://127.0.0.1:9/mcp");
	assert_eq!(get_server_health(NAME), ServerHealth::Running);
	assert_eq!(
		get_server_restart_info(NAME).restart_count,
		0,
		"no restart may be counted for an alive server"
	);

	SERVER_PROCESSES.write().unwrap().remove(NAME);
	{
		let mut guard = process_arc.lock().unwrap();
		let _ = guard.kill();
	}
	clear_restart_info(NAME);
}

#[serial]
#[tokio::test]
async fn test_ensure_server_running_http_without_process_fails_cleanly() {
	const NAME: &str = "proc-add-http-dead";
	let server = McpServerConfig::http(NAME, "http://127.0.0.1:9/mcp", 2, Vec::new());
	let err = ensure_server_running(&server)
		.await
		.expect_err("http configs must never be spawned as processes");
	assert!(
		err.to_string()
			.contains("should not be started as a process"),
		"err: {err}"
	);
	assert_eq!(get_server_health(NAME), ServerHealth::Failed);
	assert_eq!(get_server_restart_info(NAME).consecutive_failures, 1);
	clear_restart_info(NAME);
}

#[tokio::test]
async fn test_start_server_process_rejects_builtin_configs() {
	let server = McpServerConfig::builtin("proc-add-builtin", 5, Vec::new());
	let err = start_server_process(&server)
		.await
		.expect_err("builtin servers must never spawn");
	assert!(
		err.to_string()
			.contains("should not be started as external process"),
		"err: {err}"
	);
}

// ---------------------------------------------------------------------------
// is_server_running — known-dead stdio child overrides a live client handle
// ---------------------------------------------------------------------------

#[serial]
#[test]
fn test_is_server_running_dead_pgid_reports_dead() {
	const NAME: &str = "proc-add-dead-pgid";
	// A pid beyond the OS allocation range can never resolve: the liveness
	// probe deterministically reports Some(false) without spawning anything.
	register_pgid(NAME, 4_000_000);
	assert_eq!(is_stdio_process_alive(NAME), Some(false));
	assert!(!is_server_running(NAME));
	assert_eq!(get_server_health(NAME), ServerHealth::Dead);

	SERVER_PGIDS.write().unwrap().remove(NAME);
	clear_restart_info(NAME);
}

// ---------------------------------------------------------------------------
// stop_all_servers — full registry teardown
// ---------------------------------------------------------------------------

#[cfg(unix)]
#[serial]
#[tokio::test]
async fn test_stop_all_servers_clears_every_registry() {
	const NAME: &str = "proc-add-stop-all";
	let child = std::process::Command::new("sleep")
		.arg("30")
		.spawn()
		.expect("spawn sleep");
	let process_arc = Arc::new(Mutex::new(ServerProcess::Http(child)));
	SERVER_PROCESSES
		.write()
		.unwrap()
		.insert(NAME.to_string(), process_arc.clone());

	// An unallocatable pid keeps the PGID sweep a verified no-op (kill fails
	// with ESRCH), so the test can never signal a real process group.
	register_pgid(NAME, 4_000_000);
	stderr_buffer_for(NAME)
		.lock()
		.unwrap()
		.push("line".to_string());
	let mutex_before = get_server_restart_mutex(NAME);

	stop_all_servers().expect("stop-all must succeed");

	assert!(!SERVER_PROCESSES.read().unwrap().contains_key(NAME));
	assert!(
		!Arc::ptr_eq(&mutex_before, &get_server_restart_mutex(NAME)),
		"restart mutexes must be cleared"
	);
	assert!(
		stderr_lines_for(NAME).is_empty(),
		"stderr buffers must be cleared"
	);
	assert!(
		is_stdio_process_alive(NAME).is_none(),
		"pgids must be cleared"
	);

	// Reap the child stop_all killed.
	let mut guard = process_arc.lock().unwrap();
	let _ = guard.try_wait();
}

// ---------------------------------------------------------------------------
// Remaining branches: CLI-global fallback, explicit-session notifications,
// locked-process liveness, reaped-process kill, and stdio failure diagnostics
// ---------------------------------------------------------------------------

#[serial]
#[tokio::test]
async fn test_get_session_context_falls_back_to_cli_global_without_session_role() {
	let (prev_domain, prev_spec, prev_project, _, prev_workdir) = get_session_context();
	let prev_role = if prev_spec.is_empty() {
		prev_domain
	} else {
		format!("{prev_domain}:{prev_spec}")
	};

	let sid = "proc-add-ctx-no-role".to_string();
	crate::session::context::with_session_id(sid.clone(), async {
		// No session role registered for this id: the CLI global must win.
		set_session_context("developer:general", "proj-cli-fallback", "/tmp/w-cli");
		let (domain, spec, project, session_id, workdir) = get_session_context();
		assert_eq!(domain, "developer");
		assert_eq!(spec, "general");
		assert_eq!(project, "proj-cli-fallback");
		assert_eq!(session_id, sid);
		assert_eq!(workdir, "/tmp/w-cli");
	})
	.await;

	set_session_context(&prev_role, &prev_project, &prev_workdir);
}

#[serial]
#[tokio::test]
async fn test_emit_notification_with_explicit_session_id_routes_to_session_sender() {
	let sid = "proc-add-emit-session".to_string();
	crate::session::context::with_session_id(sid.clone(), async {
		let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
		set_notification_sender(Some(sid.clone()), tx);

		emit_notification(
			"proc-add-emit",
			"notifications/message",
			&serde_json::json!({"level": "warning"}),
			Some(&sid),
			None,
		);
		match rx.try_recv().expect("explicit session sender must receive") {
			crate::websocket::ServerMessage::McpNotification(p) => {
				assert_eq!(p.server, "proc-add-emit");
				assert_eq!(p.method, "notifications/message");
				assert_eq!(p.params, serde_json::json!({"level": "warning"}));
			}
			other => panic!("expected McpNotification, got {:?}", other),
		}

		clear_notification_sender(Some(sid));
	})
	.await;
}

#[cfg(unix)]
#[serial]
#[tokio::test]
async fn test_perform_health_check_reports_dead_for_exited_process() {
	const NAME: &str = "proc-add-sweep-dead";
	let mut child = std::process::Command::new("sleep")
		.arg("0")
		.spawn()
		.expect("spawn sleep 0");
	while child.try_wait().expect("try_wait exited child").is_none() {
		tokio::time::sleep(Duration::from_millis(10)).await;
	}
	let process_arc = Arc::new(Mutex::new(ServerProcess::Http(child)));
	SERVER_PROCESSES
		.write()
		.unwrap()
		.insert(NAME.to_string(), process_arc);

	let health = perform_health_check_all_servers().await;
	assert_eq!(health.get(NAME), Some(&ServerHealth::Dead));

	SERVER_PROCESSES.write().unwrap().remove(NAME);
	clear_restart_info(NAME);
}

#[cfg(unix)]
#[serial]
#[test]
fn test_is_server_running_treats_locked_process_as_alive() {
	const NAME: &str = "proc-add-locked-alive";
	let child = std::process::Command::new("sleep")
		.arg("30")
		.spawn()
		.expect("spawn sleep");
	let process_arc = Arc::new(Mutex::new(ServerProcess::Http(child)));
	SERVER_PROCESSES
		.write()
		.unwrap()
		.insert(NAME.to_string(), process_arc.clone());

	// A held registry mutex means someone is actively managing the process.
	let guard = process_arc.lock().unwrap();
	assert!(is_server_running(NAME));
	assert_eq!(get_server_health(NAME), ServerHealth::Running);
	drop(guard);

	SERVER_PROCESSES.write().unwrap().remove(NAME);
	{
		let mut guard = process_arc.lock().unwrap();
		let _ = guard.kill();
	}
	clear_restart_info(NAME);
}

#[cfg(unix)]
#[serial]
#[tokio::test]
async fn test_ensure_server_running_treats_locked_http_process_as_alive() {
	const NAME: &str = "proc-add-ensure-locked";
	let child = std::process::Command::new("sleep")
		.arg("30")
		.spawn()
		.expect("spawn sleep");
	let process_arc = Arc::new(Mutex::new(ServerProcess::Http(child)));
	SERVER_PROCESSES
		.write()
		.unwrap()
		.insert(NAME.to_string(), process_arc.clone());

	// The per-process lock must stay held for the whole call — a locked
	// process counts as "managed elsewhere ⇒ alive" — but a MutexGuard may
	// not live across .await, so a helper thread holds it until the call
	// completes.
	let (locked_tx, locked_rx) = std::sync::mpsc::channel::<()>();
	let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
	let held_arc = process_arc.clone();
	let lock_holder = std::thread::spawn(move || {
		let _guard = held_arc.lock().unwrap();
		let _ = locked_tx.send(());
		let _ = release_rx.recv();
	});
	locked_rx
		.recv()
		.expect("helper thread holds the process lock");
	let server = McpServerConfig::http(NAME, "http://127.0.0.1:9/mcp", 2, Vec::new());
	let url = ensure_server_running(&server)
		.await
		.expect("locked process counts as alive, not restartable");
	drop(release_tx);
	lock_holder.join().expect("lock holder thread exits");
	assert_eq!(url, "http://127.0.0.1:9/mcp");
	assert_eq!(
		get_server_restart_info(NAME).restart_count,
		0,
		"no restart may be counted for a locked-alive server"
	);

	SERVER_PROCESSES.write().unwrap().remove(NAME);
	{
		let mut guard = process_arc.lock().unwrap();
		let _ = guard.kill();
	}
	clear_restart_info(NAME);
}

#[cfg(unix)]
#[serial]
#[tokio::test]
async fn test_stop_all_servers_locked_process_falls_back_to_pgid() {
	const NAME: &str = "proc-add-stop-locked";
	let child = std::process::Command::new("sleep")
		.arg("30")
		.spawn()
		.expect("spawn sleep");
	let process_arc = Arc::new(Mutex::new(ServerProcess::Http(child)));
	SERVER_PROCESSES
		.write()
		.unwrap()
		.insert(NAME.to_string(), process_arc.clone());
	// Unallocatable pid keeps the PGID backstop a verified no-op.
	register_pgid(NAME, 4_000_000);

	let guard = process_arc.lock().unwrap();
	stop_all_servers().expect("stop-all must succeed even with a locked process");
	drop(guard);

	assert!(!SERVER_PROCESSES.read().unwrap().contains_key(NAME));
	// Reap the child the held lock protected from stop_all's kill.
	{
		let mut guard = process_arc.lock().unwrap();
		let _ = guard.kill();
		let _ = guard.try_wait();
	}
}

#[cfg(unix)]
#[serial]
#[test]
fn test_cleanup_server_process_locked_process_uses_pgid_path() {
	const NAME: &str = "proc-add-cleanup-locked";
	let child = std::process::Command::new("sleep")
		.arg("30")
		.spawn()
		.expect("spawn sleep");
	let process_arc = Arc::new(Mutex::new(ServerProcess::Http(child)));
	SERVER_PROCESSES
		.write()
		.unwrap()
		.insert(NAME.to_string(), process_arc.clone());
	register_pgid(NAME, 4_000_000);

	let guard = process_arc.lock().unwrap();
	cleanup_server_process(NAME).expect("cleanup must succeed via the PGID path");
	drop(guard);

	assert!(!SERVER_PROCESSES.read().unwrap().contains_key(NAME));
	{
		let mut guard = process_arc.lock().unwrap();
		let _ = guard.kill();
		let _ = guard.try_wait();
	}
}

#[serial]
#[tokio::test]
async fn test_ensure_server_running_stdio_failure_includes_stderr_detail() {
	const NAME: &str = "proc-add-stdio-stderr";
	// Seed the diagnostic buffer so the failure error deterministically
	// includes the stderr section (the drain task may not have run yet).
	stderr_buffer_for(NAME)
		.lock()
		.unwrap()
		.push("stub exploded".to_string());

	let server = McpServerConfig::stdin(
		NAME,
		"sh",
		vec!["-c".to_string(), "exec sleep 30".to_string()],
		1,
		Vec::new(),
	);
	let error = ensure_server_running(&server)
		.await
		.expect_err("silent stub must fail both handshake attempts");
	let msg = error.to_string();
	assert!(msg.contains("Failed to start server"), "err: {msg}");
	assert!(
		msg.contains("Failed to initialize stdin MCP server"),
		"err: {msg}"
	);
	assert!(msg.contains("Server stderr:"), "err: {msg}");
	assert!(msg.contains("stub exploded"), "err: {msg}");

	assert_eq!(get_server_health(NAME), ServerHealth::Failed);
	assert_eq!(get_server_restart_info(NAME).consecutive_failures, 1);
	clear_restart_info(NAME);
}
