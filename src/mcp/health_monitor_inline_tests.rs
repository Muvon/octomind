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
		use tokio::io::{AsyncReadExt, AsyncWriteExt};
		loop {
			let Ok((mut sock, _)) = listener.accept().await else {
				break;
			};
			// Drain headers and body before closing: unread request data can
			// cause Windows to reset the connection and discard the response.
			let mut buf = Vec::new();
			let mut tmp = [0u8; 8192];
			let header_end = loop {
				let n = sock.read(&mut tmp).await.unwrap_or(0);
				if n == 0 {
					break 0;
				}
				buf.extend_from_slice(&tmp[..n]);
				if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
					break pos + 4;
				}
			};
			if header_end > 0 {
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
			}
			let response =
				format!("HTTP/1.1 {status} Stub\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
			let _ = sock.write_all(response.as_bytes()).await;
			let _ = sock.shutdown().await;
		}
	});
	format!("http://{addr}")
}

fn seed_restart_info(name: &str, f: impl FnOnce(&mut process::ServerRestartInfo)) {
	let mut guard = process::SERVER_RESTART_INFO.write().unwrap();
	let info = guard.entry(name.to_string()).or_default();
	f(info);
}

fn clear_restart_info(name: &str) {
	process::SERVER_RESTART_INFO.write().unwrap().remove(name);
}

#[serial]
#[tokio::test]
async fn start_health_monitor_without_external_servers_stops_cleanly() {
	let config = Arc::new(template_config());
	stop_health_monitor();
	start_health_monitor(config)
		.await
		.expect("no external servers must start cleanly");
	assert!(!is_health_monitor_running());
}

#[serial]
#[tokio::test]
async fn start_health_monitor_is_idempotent_while_running() {
	// Ensure clean state: any prior test's background task must be stopped.
	stop_health_monitor();
	HEALTH_MONITOR_RUNNING.store(true, Ordering::SeqCst);
	let config = Arc::new(template_config());
	start_health_monitor(config)
		.await
		.expect("already-running monitor must be a no-op");
	assert!(is_health_monitor_running());
	HEALTH_MONITOR_RUNNING.store(false, Ordering::SeqCst);
}

#[serial]
#[tokio::test]
async fn start_health_monitor_with_external_server_runs_until_stopped() {
	let mut config = template_config();
	stop_health_monitor();
	// Port 9 (discard) is never a live MCP endpoint; the monitor's first
	// check only fires after its 2s startup delay, well past this test.
	config.mcp.servers.push(McpServerConfig::http(
		"stub-monitor",
		"http://127.0.0.1:9/mcp",
		1,
		vec![],
	));
	start_health_monitor(Arc::new(config))
		.await
		.expect("monitor with an external server must start");
	assert!(is_health_monitor_running());
	stop_health_monitor();
	assert!(!is_health_monitor_running());
}

#[serial]
#[test]
fn stop_health_monitor_without_running_monitor_is_noop() {
	stop_health_monitor();
	stop_health_monitor();
}

#[serial]
#[tokio::test]
async fn force_health_check_without_external_servers_is_noop() {
	let config = template_config();
	force_health_check(&config)
		.await
		.expect("no external servers must check cleanly");
}

#[serial]
#[tokio::test]
async fn health_check_reports_builtin_servers_as_running() {
	const NAME: &str = "hm-test-builtin";
	let server = McpServerConfig::builtin(NAME, 30, vec![]);
	check_server_health_and_restart_if_dead(&server)
		.await
		.expect("builtin health check must succeed");
	let info = process::get_server_restart_info(NAME);
	assert_eq!(info.health_status, ServerHealth::Running);
	assert!(info.last_health_check.is_some());
	clear_restart_info(NAME);
}

#[serial]
#[tokio::test]
async fn health_check_marks_dead_stdio_server_and_cooldown_blocks_restart() {
	const NAME: &str = "hm-test-dead-stdio";
	let server = stdin_server(NAME, "definitely-not-a-real-binary");
	seed_restart_info(NAME, |info| {
		info.last_restart_time = Some(std::time::SystemTime::now());
	});
	check_server_health_and_restart_if_dead(&server)
		.await
		.expect("cooldown path must succeed without spawning");
	let info = process::get_server_restart_info(NAME);
	assert_eq!(info.health_status, ServerHealth::Dead);
	clear_restart_info(NAME);
}

#[serial]
#[tokio::test]
async fn health_check_gives_up_after_three_consecutive_failures() {
	const NAME: &str = "hm-test-give-up";
	let server = stdin_server(NAME, "definitely-not-a-real-binary");
	seed_restart_info(NAME, |info| {
		info.consecutive_failures = 3;
	});
	check_server_health_and_restart_if_dead(&server)
		.await
		.expect("give-up path must succeed");
	assert_eq!(
		process::get_server_restart_info(NAME).health_status,
		ServerHealth::Failed
	);
	clear_restart_info(NAME);
}

#[serial]
#[tokio::test]
async fn health_check_leaves_failed_servers_untouched() {
	const NAME: &str = "hm-test-failed-terminal";
	let server = stdin_server(NAME, "definitely-not-a-real-binary");
	seed_restart_info(NAME, |info| {
		info.health_status = ServerHealth::Failed;
	});
	check_server_health_and_restart_if_dead(&server)
		.await
		.expect("terminal Failed state must short-circuit");
	let info = process::get_server_restart_info(NAME);
	assert_eq!(info.health_status, ServerHealth::Failed);
	assert!(
		info.last_health_check.is_none(),
		"failed entry must not be recomputed"
	);
	clear_restart_info(NAME);
}

#[tokio::test]
async fn http_health_check_requires_a_url() {
	let server = stdin_server("hm-test-no-url", "echo");
	assert!(perform_http_health_check(&server).await.is_err());
}

#[test]
fn http_auth_failure_uses_transport_status_and_challenges() {
	use rmcp::service::ClientInitializeError;
	use rmcp::transport::streamable_http_client::{
		AuthRequiredError, InsufficientScopeError, StreamableHttpError,
	};

	let classify = |error: StreamableHttpError<reqwest::Error>| {
		is_http_auth_failure(&ClientInitializeError::TransportError {
			error: rmcp::transport::DynamicTransportError::from_parts(
				"test",
				std::any::TypeId::of::<()>(),
				Box::new(error),
			),
			context: "health probe".into(),
		})
	};
	for (response, expected) in [
		("HTTP 401 Unauthorized: ", true),
		("HTTP 403 Forbidden: ", true),
		(
			"HTTP 500 Internal Server Error: HTTP 401 Unauthorized",
			false,
		),
		("HTTP 404 Not Found: /403/forbidden", false),
	] {
		assert_eq!(
			classify(StreamableHttpError::UnexpectedServerResponse(
				response.into()
			)),
			expected,
			"{response}"
		);
	}
	assert!(classify(StreamableHttpError::AuthRequired(
		AuthRequiredError::new("Bearer realm=\"test\"".into())
	)));
	assert!(classify(StreamableHttpError::InsufficientScope(
		InsufficientScopeError::new("Bearer scope=\"tools\"".into(), Some("tools".into()))
	)));
}

#[serial]
#[tokio::test]
async fn http_health_check_classifies_auth_failure_as_unreachable() {
	let url = spawn_health_stub(401).await;
	let server = McpServerConfig::http("hm-test-auth-rejected", &url, 2, vec![]);
	let result = perform_http_health_check(&server)
		.await
		.expect("health probe must classify, not fail");
	assert!(matches!(result, HttpHealthResult::Unreachable));
}

#[serial]
#[tokio::test]
async fn http_health_check_classifies_refused_connection_as_dead() {
	// Reserve the port without listening so another test cannot claim it.
	let socket = tokio::net::TcpSocket::new_v4().expect("socket");
	socket.bind("127.0.0.1:0".parse().unwrap()).expect("bind");
	let addr = socket.local_addr().expect("addr");
	// Auth-looking text in a name/URL must not turn a transport failure into
	// an auth failure (CI originally hit this with ephemeral port 34011).
	let server = McpServerConfig::http(
		"hm-test-refused-401-403-unauthorized-forbidden",
		&format!("http://{addr}/34011/403/unauthorized/forbidden"),
		2,
		vec![],
	);
	let result = perform_http_health_check(&server)
		.await
		.expect("health probe must classify, not fail");
	assert!(matches!(result, HttpHealthResult::Dead));
}

#[serial]
#[tokio::test]
async fn health_check_records_unreachable_for_auth_rejecting_http_server() {
	const NAME: &str = "hm-test-auth-status";
	let url = spawn_health_stub(403).await;
	let server = McpServerConfig::http(NAME, &url, 2, vec![]);
	check_server_health_and_restart_if_dead(&server)
		.await
		.expect("unreachable servers must not error");
	assert_eq!(
		process::get_server_restart_info(NAME).health_status,
		ServerHealth::Unreachable
	);
	clear_restart_info(NAME);
}
