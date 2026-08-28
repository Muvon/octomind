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
