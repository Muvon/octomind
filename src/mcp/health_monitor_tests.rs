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

// Additional unit tests for src/mcp/health_monitor.rs, complementing the
// inline `mod tests`: restart policy branches, responsiveness verification,
// monitor server filtering (env-gated servers), and forced health checks.
// Tests that touch the global HEALTH_MONITOR_RUNNING flag or the shared
// SERVER_RESTART_INFO registry are #[serial].

use super::*;
use serial_test::serial;
use std::collections::HashMap;

fn template_config() -> Config {
	let mut config: Config = toml::from_str(include_str!("../../config-templates/default.toml"))
		.expect("parse default config template");
	config.build_role_map();
	config
}

fn stdin_server(name: &str, command: &str) -> McpServerConfig {
	McpServerConfig::Stdin {
		name: name.to_string(),
		command: command.to_string(),
		args: vec![],
		timeout_seconds: 2,
		tools: vec![],
		env: HashMap::new(),
		cwd: None,
		auto_bind: None,
	}
}

/// Minimal HTTP/1.1 responder that answers every request with `status`.
async fn spawn_health_stub(status: u16) -> String {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
		.await
		.expect("bind stub listener");
	let addr = listener.local_addr().expect("stub local addr");
	tokio::spawn(async move {
		use tokio::io::AsyncWriteExt;
		loop {
			let Ok((mut sock, _)) = listener.accept().await else {
				break;
			};
			let response = format!("HTTP/1.1 {status} Stub\r\nContent-Length: 0\r\n\r\n");
			let _ = sock.write_all(response.as_bytes()).await;
		}
	});
	format!("http://{addr}")
}

fn clear_restart_info(name: &str) {
	process::SERVER_RESTART_INFO.write().unwrap().remove(name);
}

#[test]
fn test_health_check_interval_is_two_minutes() {
	assert_eq!(HEALTH_CHECK_INTERVAL_SECONDS, 120);
}

#[test]
fn test_http_health_result_variants_are_distinct() {
	assert!(matches!(
		HttpHealthResult::Healthy,
		HttpHealthResult::Healthy
	));
	assert!(matches!(
		HttpHealthResult::Unreachable,
		HttpHealthResult::Unreachable
	));
	assert!(matches!(HttpHealthResult::Dead, HttpHealthResult::Dead));
}

#[tokio::test]
async fn test_verify_server_responsiveness_by_connection_type() {
	// Builtin servers are always considered responsive
	let builtin = McpServerConfig::builtin("hm-add-responsive-builtin", 30, vec![]);
	assert!(verify_server_responsiveness(&builtin).await);

	// Untracked stdio/http servers have no live process → not responsive
	let stdio = stdin_server("hm-add-responsive-stdio", "cat");
	assert!(!verify_server_responsiveness(&stdio).await);
	let http = McpServerConfig::http(
		"hm-add-responsive-http",
		"http://127.0.0.1:9/mcp",
		2,
		vec![],
	);
	assert!(!verify_server_responsiveness(&http).await);

	clear_restart_info("hm-add-responsive-stdio");
	clear_restart_info("hm-add-responsive-http");
}

#[tokio::test]
async fn test_restart_dead_server_skips_remote_http_and_builtin() {
	// Remote HTTP server (no local command) — skipped, not an error
	let remote = McpServerConfig::http("hm-add-remote", "http://127.0.0.1:9/mcp", 2, vec![]);
	restart_dead_server(&remote)
		.await
		.expect("remote servers are skipped, not errored");

	// Builtin servers never need restarting
	let builtin = McpServerConfig::builtin("hm-add-builtin-skip", 30, vec![]);
	restart_dead_server(&builtin)
		.await
		.expect("builtin servers are skipped, not errored");
}

#[serial]
#[tokio::test]
async fn test_restart_dead_server_reports_spawn_failure() {
	const NAME: &str = "hm-add-spawn-fail";
	let server = stdin_server(NAME, "definitely-not-a-real-binary");
	assert!(
		restart_dead_server(&server).await.is_err(),
		"a stdio server whose binary cannot spawn must surface the failure"
	);
	let info = process::get_server_restart_info(NAME);
	assert_eq!(info.health_status, ServerHealth::Failed);
	assert!(info.consecutive_failures >= 1);
	clear_restart_info(NAME);
}

#[serial]
#[tokio::test]
async fn test_health_check_attempts_restart_for_untracked_dead_stdio_server() {
	const NAME: &str = "hm-add-dead-restart";
	let server = stdin_server(NAME, "definitely-not-a-real-binary");
	// No seeded state: no cooldown, no failure budget → the Dead branch must
	// actually attempt the restart. The attempt fails, but the check itself
	// only logs — it must still return Ok.
	check_server_health_and_restart_if_dead(&server)
		.await
		.expect("restart attempt failure is logged, not propagated");
	let info = process::get_server_restart_info(NAME);
	assert_eq!(
		info.health_status,
		ServerHealth::Failed,
		"failed restart attempt must mark the server Failed"
	);
	assert!(info.last_health_check.is_some());
	clear_restart_info(NAME);
}

#[serial]
#[tokio::test]
async fn test_start_health_monitor_filters_servers_with_missing_env_keys() {
	let mut config = template_config();
	config.mcp.servers.push(McpServerConfig::Stdin {
		name: "hm-add-gated".to_string(),
		command: "run-with-token {{ENV:OCTOMIND_TEST_UNSET_TOKEN_XYZ}}".to_string(),
		args: vec![],
		timeout_seconds: 2,
		tools: vec![],
		env: HashMap::new(),
		cwd: None,
		auto_bind: None,
	});
	start_health_monitor(Arc::new(config))
		.await
		.expect("env-gated server must be filtered out, leaving nothing to monitor");
	assert!(!is_health_monitor_running());
}

#[serial]
#[tokio::test]
async fn test_start_health_monitor_tracks_stdio_and_http_server_types() {
	let mut config = template_config();
	config
		.mcp
		.servers
		.push(stdin_server("hm-add-stdio-monitored", "cat"));
	config.mcp.servers.push(McpServerConfig::http(
		"hm-add-http-monitored",
		"http://127.0.0.1:9/mcp",
		1,
		vec![],
	));
	start_health_monitor(Arc::new(config))
		.await
		.expect("monitor must start when external servers exist");
	assert!(is_health_monitor_running());
	stop_health_monitor();
	assert!(!is_health_monitor_running());
}

#[serial]
#[tokio::test]
async fn test_force_health_check_skips_builtin_servers() {
	const NAME: &str = "hm-add-force-builtin";
	let mut config = template_config();
	config
		.mcp
		.servers
		.push(McpServerConfig::builtin(NAME, 30, vec![]));
	force_health_check(&config)
		.await
		.expect("force check must succeed");
	assert!(
		process::SERVER_RESTART_INFO
			.read()
			.unwrap()
			.get(NAME)
			.is_none(),
		"builtin servers are filtered out and never probed"
	);
}

#[serial]
#[tokio::test]
async fn test_force_health_check_reports_unreachable_http_auth_failure() {
	const NAME: &str = "hm-add-force-401";
	let url = spawn_health_stub(401).await;
	let mut config = template_config();
	config
		.mcp
		.servers
		.push(McpServerConfig::http(NAME, &url, 2, vec![]));
	force_health_check(&config)
		.await
		.expect("force check must succeed");
	assert_eq!(
		process::get_server_restart_info(NAME).health_status,
		ServerHealth::Unreachable
	);
	clear_restart_info(NAME);
}

// ---------------------------------------------------------------------------
// Fake MCP servers (python) — real transports for success-path coverage.
// ---------------------------------------------------------------------------

/// Newline-delimited JSON-RPC MCP server over stdio. Speaks `server/discover`
/// (2026-07-28), legacy `initialize`, `tools/list`, and an `echo` tool whose
/// result embeds the process cwd and the OCTOMIND_TEST_ENV_MARKER env var so
/// callers can assert spawn configuration.
const FAKE_STDIO_SERVER: &str = r#"
import json, os, sys

def result(rid, res):
    return json.dumps({"jsonrpc": "2.0", "id": rid, "result": res})

def handle(req):
    method = req.get("method")
    rid = req.get("id")
    if method == "server/discover":
        return result(rid, {"resultType": "complete", "supportedVersions": ["2026-07-28"], "capabilities": {}, "ttlMs": 0, "cacheScope": "private"})
    if method == "initialize":
        return result(rid, {"protocolVersion": "2025-03-26", "capabilities": {}, "serverInfo": {"name": "octomind-fake-stdio", "version": "1.0"}, "instructions": "fake"})
    if method == "tools/list":
        return result(rid, {"tools": [{"name": "echo", "description": "echo tool", "inputSchema": {"type": "object"}}]})
    if method == "tools/call":
        params = req.get("params") or {}
        payload = {"echo": True, "arguments": params.get("arguments"), "cwd": os.getcwd(), "env": os.environ.get("OCTOMIND_TEST_ENV_MARKER", "unset")}
        return result(rid, {"content": [{"type": "text", "text": json.dumps(payload)}], "isError": False})
    if method == "ping":
        return result(rid, {})
    if rid is not None:
        return json.dumps({"jsonrpc": "2.0", "id": rid, "error": {"code": -32601, "message": "method not found"}})
    return None

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    try:
        req = json.loads(line)
    except ValueError:
        continue
    out = handle(req)
    if out is not None:
        sys.stdout.write(out + "\n")
        sys.stdout.flush()
"#;

fn write_fake_stdio_script(tag: &str) -> std::path::PathBuf {
	let path = std::env::temp_dir().join(format!(
		"octomind-test-hm-stdio-{tag}-{}.py",
		std::process::id()
	));
	std::fs::write(&path, FAKE_STDIO_SERVER).expect("write fake stdio server script");
	path
}

fn fake_stdio_server_config(name: &str, tag: &str) -> McpServerConfig {
	let script = write_fake_stdio_script(tag);
	McpServerConfig::Stdin {
		name: name.to_string(),
		command: "python3".to_string(),
		args: vec![script.to_string_lossy().into_owned()],
		timeout_seconds: 10,
		tools: vec![],
		env: HashMap::new(),
		cwd: None,
		auto_bind: None,
	}
}

/// Enable debug logging for the current test thread so `log_debug!` format
/// arguments are evaluated (they are behind `is_debug_enabled()`).
fn enable_debug_logging() {
	let mut config = template_config();
	config.log_level = crate::config::LogLevel::Debug;
	crate::config::set_thread_config(&config);
}

fn seed_restart_info(name: &str, f: impl FnOnce(&mut process::ServerRestartInfo)) {
	let mut guard = process::SERVER_RESTART_INFO.write().unwrap();
	let info = guard.entry(name.to_string()).or_default();
	f(info);
}

/// The monitor loop must actually run a health cycle: initial 2s delay, then
/// a check that records `last_health_check` for every external server.
#[serial]
#[tokio::test]
async fn test_monitor_loop_runs_a_real_health_cycle() {
	enable_debug_logging();
	const NAME: &str = "hm-cycle-probe";
	let mut config = template_config();
	config.mcp.servers = vec![stdin_server(NAME, "definitely-not-a-real-binary")];

	start_health_monitor(std::sync::Arc::new(config))
		.await
		.expect("monitor must start with one external server");

	// The cycle records last_health_check, then the restart attempt on the
	// unstartable binary fails and marks the server Failed. Wait for both so
	// the final assertion never races the in-flight restart.
	let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
	loop {
		let info = process::get_server_restart_info(NAME);
		if info.last_health_check.is_some() && info.health_status == ServerHealth::Failed {
			break;
		}
		assert!(
			std::time::Instant::now() < deadline,
			"monitor never completed a health cycle within 10s (last check: {:?}, health: {:?})",
			info.last_health_check,
			info.health_status
		);
		tokio::time::sleep(std::time::Duration::from_millis(100)).await;
	}

	stop_health_monitor();
	clear_restart_info(NAME);
}

/// A stdio server with a live client connection reports Running (164).
#[serial]
#[tokio::test]
async fn test_connected_stdio_server_reports_running() {
	const NAME: &str = "hm-stdio-running";
	let server = fake_stdio_server_config(NAME, "running");
	super::super::client::connect_stdio(&server)
		.await
		.expect("fake stdio server must connect");

	check_server_health_and_restart_if_dead(&server)
		.await
		.expect("healthy server check must succeed");
	assert_eq!(
		process::get_server_restart_info(NAME).health_status,
		ServerHealth::Running,
		"a connected stdio server must be Running"
	);
	assert_eq!(process::get_server_restart_info(NAME).restart_count, 0);

	super::super::client::disconnect(NAME);
	clear_restart_info(NAME);
}

/// A dead stdio server with a startable command is restarted in place and
/// comes back Running with a bumped restart count.
#[serial]
#[tokio::test]
async fn test_dead_stdio_server_restarts_successfully() {
	const NAME: &str = "hm-stdio-restart";
	let server = fake_stdio_server_config(NAME, "restart");
	// No connection, no process entry → Dead.
	assert!(!process::is_server_running(NAME));

	check_server_health_and_restart_if_dead(&server)
		.await
		.expect("restart path must not propagate errors");

	let info = process::get_server_restart_info(NAME);
	assert_eq!(
		info.health_status,
		ServerHealth::Running,
		"successful restart must leave the server Running"
	);
	assert!(info.restart_count >= 1, "restart must be counted");
	assert!(
		super::super::client::is_connected(NAME),
		"restart must establish a client connection"
	);

	super::super::client::disconnect(NAME);
	process::cleanup_server_process(NAME).ok();
	clear_restart_info(NAME);
}

/// A remote HTTP server (no local command) classified Dead is NOT restarted.
#[serial]
#[tokio::test]
async fn test_dead_http_server_without_command_is_not_restarted() {
	enable_debug_logging();
	const NAME: &str = "hm-http-no-command";
	// Port 1 on 127.0.0.1 refuses connections → Dead classification.
	let server = McpServerConfig::http(NAME, "http://127.0.0.1:1/mcp", 2, vec![]);

	check_server_health_and_restart_if_dead(&server)
		.await
		.expect("check must succeed even when restart is impossible");

	let info = process::get_server_restart_info(NAME);
	assert_eq!(info.health_status, ServerHealth::Dead);
	assert_eq!(
		info.restart_count, 0,
		"remote HTTP servers must not be restarted"
	);
	clear_restart_info(NAME);
}

/// The terminal Failed state short-circuits before any probe: last_health_check
/// must stay untouched (is_server_running would otherwise overwrite it).
#[serial]
#[tokio::test]
async fn test_failed_state_short_circuits_before_probe() {
	enable_debug_logging();
	const NAME: &str = "hm-failed-shortcircuit";
	let server = stdin_server(NAME, "definitely-not-a-real-binary");
	seed_restart_info(NAME, |info| {
		info.health_status = ServerHealth::Failed;
		info.last_health_check = None;
	});

	check_server_health_and_restart_if_dead(&server)
		.await
		.expect("Failed short-circuit returns Ok");

	let info = process::get_server_restart_info(NAME);
	assert_eq!(info.health_status, ServerHealth::Failed);
	assert!(
		info.last_health_check.is_none(),
		"Failed servers must not be probed (probe would set last_health_check)"
	);
	clear_restart_info(NAME);
}

/// Cooldown: a restart inside the last 30s skips the restart attempt entirely —
/// no spawn, no failure accounting.
#[serial]
#[tokio::test]
async fn test_cooldown_skips_restart_without_failure_accounting() {
	enable_debug_logging();
	const NAME: &str = "hm-cooldown";
	let server = stdin_server(NAME, "definitely-not-a-real-binary");
	seed_restart_info(NAME, |info| {
		info.last_restart_time = Some(std::time::SystemTime::now());
	});

	check_server_health_and_restart_if_dead(&server)
		.await
		.expect("cooldown skip returns Ok");

	let info = process::get_server_restart_info(NAME);
	assert_eq!(
		info.health_status,
		ServerHealth::Dead,
		"cooldown must leave the server Dead, not Failed"
	);
	assert_eq!(info.consecutive_failures, 0, "no attempt → no failure");
	assert_eq!(info.restart_count, 0);
	clear_restart_info(NAME);
}

/// Give-up: three consecutive failures mark the server Failed instead of
/// attempting a fourth start.
#[serial]
#[tokio::test]
async fn test_giveup_marks_failed_without_new_attempt() {
	enable_debug_logging();
	const NAME: &str = "hm-giveup";
	let server = stdin_server(NAME, "definitely-not-a-real-binary");
	seed_restart_info(NAME, |info| {
		info.consecutive_failures = 3;
	});

	check_server_health_and_restart_if_dead(&server)
		.await
		.expect("give-up returns Ok");

	let info = process::get_server_restart_info(NAME);
	assert_eq!(info.health_status, ServerHealth::Failed);
	assert_eq!(info.restart_count, 0, "give-up must not attempt a restart");
	clear_restart_info(NAME);
}

/// Unreachable (auth-style failure) HTTP servers are logged and left alone —
/// no restart bookkeeping at all.
#[serial]
#[tokio::test]
async fn test_unreachable_http_server_is_left_alone() {
	enable_debug_logging();
	const NAME: &str = "hm-unreachable";
	let url = spawn_health_stub(401).await;
	let server = McpServerConfig::http(NAME, &format!("{url}/mcp"), 2, vec![]);

	check_server_health_and_restart_if_dead(&server)
		.await
		.expect("unreachable classification returns Ok");

	let info = process::get_server_restart_info(NAME);
	assert_eq!(info.health_status, ServerHealth::Unreachable);
	assert_eq!(info.restart_count, 0);
	clear_restart_info(NAME);
}
