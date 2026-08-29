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

// Background health monitoring for MCP servers

use super::process::{self, is_server_running, ServerHealth};
use crate::config::{Config, McpConnectionType, McpServerConfig};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use std::time::Duration;
use tokio::time::interval;

// Result of HTTP health check - distinguishes auth failures from other issues
enum HttpHealthResult {
	Healthy,     // Server is responding correctly
	Unreachable, // Server reachable but auth/config failed (401/403)
	Dead,        // Server not reachable or other errors
}

// Global flag to control the health monitor
static HEALTH_MONITOR_RUNNING: AtomicBool = AtomicBool::new(false);

// Health monitoring configuration
const HEALTH_CHECK_INTERVAL_SECONDS: u64 = 120; // Check every 2 minutes (balanced for production)

/// Start the background health monitoring task
pub async fn start_health_monitor(config: Arc<Config>) -> Result<(), anyhow::Error> {
	// Prevent multiple health monitors from running
	if HEALTH_MONITOR_RUNNING
		.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
		.is_err()
	{
		crate::log_debug!("Health monitor is already running");
		return Ok(());
	}

	crate::log_debug!(
		"Starting MCP server health monitor (checking every {}s)",
		HEALTH_CHECK_INTERVAL_SECONDS
	);

	// Get external servers that need monitoring (all external servers, but only restart local ones)
	let external_servers: Vec<McpServerConfig> = config
		.mcp
		.servers
		.iter()
		.filter(|server| {
			matches!(
				server.connection_type(),
				McpConnectionType::Http | McpConnectionType::Stdin
			)
		})
		.filter(|server| crate::mcp::client::missing_env_keys(server).is_empty())
		.cloned()
		.collect();

	if external_servers.is_empty() {
		crate::log_debug!("No external servers to monitor, health monitor stopping");
		HEALTH_MONITOR_RUNNING.store(false, Ordering::SeqCst);
		return Ok(());
	}

	crate::log_debug!(
		"Health monitor will track {} external servers: {}",
		external_servers.len(),
		external_servers
			.iter()
			.map(|s| {
				let server_type = match s.connection_type() {
					McpConnectionType::Stdin => "stdio",
					McpConnectionType::Http => "http",
					McpConnectionType::Builtin => "builtin",
				};
				format!("{}({})", s.name(), server_type)
			})
			.collect::<Vec<_>>()
			.join(", ")
	);

	// Spawn the monitoring task
	tokio::spawn(async move {
		// Add initial delay to prevent immediate health check on startup
		// This avoids double token loading when user runs /mcp shortly after session start
		tokio::time::sleep(Duration::from_secs(2)).await;

		let mut check_interval = interval(Duration::from_secs(HEALTH_CHECK_INTERVAL_SECONDS));

		loop {
			// Wait for the next check interval
			check_interval.tick().await;

			// Check if we should stop monitoring
			if !HEALTH_MONITOR_RUNNING.load(Ordering::SeqCst) {
				crate::log_debug!("Health monitor stopping");
				break;
			}

			// Perform health check on all external servers and restart if process is dead
			for server in &external_servers {
				if let Err(e) = check_server_health_and_restart_if_dead(server).await {
					crate::log_error!(
						"Health check failed for server '{}': {}. Verify the server is running at the configured URL.",
						server.name(),
						e
					);
				}
			}
		}

		crate::log_debug!("Health monitor task completed");
	});

	Ok(())
}

/// Stop the background health monitoring task
pub fn stop_health_monitor() {
	if HEALTH_MONITOR_RUNNING
		.compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst)
		.is_ok()
	{
		crate::log_debug!("Stopping health monitor");
	}
}

/// Check a single server's health and restart ONLY if process is dead
async fn check_server_health_and_restart_if_dead(
	server: &McpServerConfig,
) -> Result<(), anyhow::Error> {
	// A server already in the terminal Failed state gave up on purpose — it
	// failed its initial startup, or exhausted its restart budget. Don't
	// recompute it to Dead and re-spawn it: "if it failed to start, leave it."
	// Shutdown/death after a successful run is recorded as Dead (not Failed),
	// so legitimate Ctrl+C recovery is unaffected.
	// This check must run BEFORE any probe: `is_server_running` overwrites
	// health_status/last_health_check as a side effect and would clobber the
	// terminal Failed state before we could observe it.
	let restart_info = process::get_server_restart_info(server.name());
	if matches!(restart_info.health_status, ServerHealth::Failed) {
		crate::log_debug!(
			"Health monitor: server '{}' is in Failed state — not restarting",
			server.name()
		);
		return Ok(());
	}

	// Perform different health checks based on server type
	let health_status = match server.connection_type() {
		McpConnectionType::Stdin => {
			// For stdin servers, check if the process is running
			if is_server_running(server.name()) {
				ServerHealth::Running
			} else {
				ServerHealth::Dead
			}
		}
		McpConnectionType::Http => {
			// Remote HTTP server - perform HTTP health check
			match perform_http_health_check(server).await {
				Ok(HttpHealthResult::Healthy) => ServerHealth::Running,
				Ok(HttpHealthResult::Unreachable) => ServerHealth::Unreachable,
				Ok(HttpHealthResult::Dead) | Err(_) => ServerHealth::Dead,
			}
		}
		McpConnectionType::Builtin => {
			// Builtin servers are always running
			ServerHealth::Running
		}
	};

	crate::log_debug!(
		"Health check: server '{}' status = {:?}, restart_count = {}",
		server.name(),
		health_status,
		restart_info.restart_count
	);

	// Update health status and last health check time
	{
		let mut restart_info_guard = process::SERVER_RESTART_INFO.write().unwrap();
		let info = restart_info_guard
			.entry(server.name().to_string())
			.or_default();
		info.health_status = health_status;
		info.last_health_check = Some(std::time::SystemTime::now());
	}

	match health_status {
		ServerHealth::Dead => {
			// Server process is actually dead - this is when we should restart
			crate::log_debug!(
				"Health monitor detected dead server '{}' - attempting restart",
				server.name()
			);

			// Check if we should give up. Use consecutive_failures (failed starts
			// in a row, reset to 0 on any successful start) — NOT restart_count,
			// which counts every lifetime start including deliberate restarts and
			// role switches, and would wrongly mark a healthy-but-often-restarted
			// server as Failed.
			if restart_info.consecutive_failures >= 3 {
				crate::log_debug!(
					"Server '{}' has exceeded max restart attempts ({} consecutive failures), marking as failed",
					server.name(),
					restart_info.consecutive_failures
				);

				// Mark as failed to prevent further restart attempts
				let mut restart_info_guard = process::SERVER_RESTART_INFO.write().unwrap();
				if let Some(info) = restart_info_guard.get_mut(server.name()) {
					info.health_status = ServerHealth::Failed;
				}
				return Ok(());
			}

			// Check cooldown period to avoid rapid restart attempts
			if let Some(last_restart) = restart_info.last_restart_time {
				let time_since_restart = std::time::SystemTime::now()
					.duration_since(last_restart)
					.unwrap_or(std::time::Duration::from_secs(0));

				if time_since_restart < Duration::from_secs(30) {
					crate::log_debug!(
						"Server '{}' is in cooldown period, skipping restart attempt",
						server.name()
					);
					return Ok(());
				}
			}

			// Attempt to restart the dead server
			match restart_dead_server(server).await {
				Ok(()) => {
					crate::log_info!(
						"Health monitor successfully restarted dead server '{}'",
						server.name()
					);
				}
				Err(e) => {
					crate::log_debug!(
						"Health monitor failed to restart dead server '{}': {}",
						server.name(),
						e
					);
				}
			}
		}
		ServerHealth::Unreachable => {
			// Server is unreachable (auth failed or connection refused) - don't restart
			crate::log_debug!(
				"Health monitor: server '{}' is unreachable - check configuration/authentication",
				server.name()
			);
			// Don't attempt restart - remote servers can't be restarted automatically
		}
		ServerHealth::Failed => {
			// Server has failed - check if enough time has passed to reset failure state
			if let Some(last_restart) = restart_info.last_restart_time {
				let time_since_last_restart = std::time::SystemTime::now()
					.duration_since(last_restart)
					.unwrap_or(std::time::Duration::from_secs(0));

				// Reset failure state after 5 minutes
				if time_since_last_restart > Duration::from_secs(300) {
					crate::log_debug!(
						"Resetting failed state for server '{}' after cooldown period",
						server.name()
					);
					if let Err(e) = process::reset_server_failure_state(server.name()) {
						crate::log_debug!(
							"Failed to reset failure state for server '{}': {}",
							server.name(),
							e
						);
					}
				}
			}
		}
		ServerHealth::Running => {
			// Server is running - verify responsiveness but don't restart on failed responses
			// Failed responses are normal due to misled requests
			if !verify_server_responsiveness(server).await {
				crate::log_debug!(
					"Health monitor: server '{}' process is running but not responsive (this is normal for failed requests)",
					server.name()
				);
				// Don't mark as dead - failed responses are normal
				// Only mark as dead if the actual process is not running
			}
		}
		ServerHealth::Restarting => {
			// Server is currently restarting, just monitor
			crate::log_debug!(
				"Health monitor: server '{}' is currently restarting",
				server.name()
			);
		}
	}

	Ok(())
}

/// Attempt to restart a dead server (only for servers that can be restarted)
async fn restart_dead_server(server: &McpServerConfig) -> Result<(), anyhow::Error> {
	// Check if this server can actually be restarted
	let can_restart = match server.connection_type() {
		McpConnectionType::Stdin => true, // Stdin servers can always be restarted
		McpConnectionType::Http => server.command().is_some(), // Only local HTTP servers can be restarted
		McpConnectionType::Builtin => false, // Builtin servers don't need restart
	};

	if !can_restart {
		crate::log_debug!(
			"Server '{}' is a remote server and cannot be restarted by health monitor",
			server.name()
		);
		return Ok(()); // Not an error - just can't restart remote servers
	}

	crate::log_debug!(
		"Health monitor attempting to restart dead server '{}'",
		server.name()
	);

	match process::ensure_server_running(server).await {
		Ok(_) => {
			crate::log_info!(
				"Health monitor successfully restarted dead server '{}'",
				server.name()
			);
			Ok(())
		}
		Err(e) => {
			crate::log_debug!(
				"Health monitor failed to restart dead server '{}': {}",
				server.name(),
				e
			);
			Err(e)
		}
	}
}

/// Verify that a server is actually responsive (basic health check)
async fn verify_server_responsiveness(server: &McpServerConfig) -> bool {
	// For stdin servers, we can try a simple ping-like operation
	// For HTTP servers, we could do a simple HTTP request
	// BUT: Failed responses are normal due to misled requests
	// We should only check if the PROCESS is alive, not if it responds correctly

	match server.connection_type() {
		McpConnectionType::Stdin => {
			// For stdin servers, just check if the process is alive
			// Don't try to communicate - that might fail due to misled requests
			process::is_server_running(server.name())
		}
		McpConnectionType::Http => {
			// For HTTP servers, just check if the process is running
			// Don't make HTTP requests - failed responses are normal
			process::is_server_running(server.name())
		}
		McpConnectionType::Builtin => {
			// Built-in servers are always "running"
			true
		}
	}
}

/// Get health monitor status
pub fn is_health_monitor_running() -> bool {
	HEALTH_MONITOR_RUNNING.load(Ordering::SeqCst)
}

/// Force a health check on all servers (for manual triggering)
pub async fn force_health_check(config: &Config) -> Result<(), anyhow::Error> {
	crate::log_debug!("Forcing health check on all external servers");

	let external_servers: Vec<McpServerConfig> = config
		.mcp
		.servers
		.iter()
		.filter(|server| {
			matches!(
				server.connection_type(),
				McpConnectionType::Http | McpConnectionType::Stdin
			)
		})
		.cloned()
		.collect();

	for server in &external_servers {
		if let Err(e) = check_server_health_and_restart_if_dead(server).await {
			crate::log_debug!(
				"Force health check error for server '{}': {}",
				server.name(),
				e
			);
		}
	}

	Ok(())
}

/// Perform HTTP health check for remote servers.
///
/// A live rmcp client connection counts as healthy. When there is none (or it
/// died), a reconnect attempt doubles as the health probe — the client speaks
/// MCP 2026-07-28 with automatic legacy fallback, so this exercises the same
/// path tool execution uses.
async fn perform_http_health_check(
	server: &McpServerConfig,
) -> Result<HttpHealthResult, anyhow::Error> {
	if server.url().is_none() {
		return Err(anyhow::anyhow!("No URL configured for HTTP server"));
	}

	if crate::mcp::client::is_connected(server.name()) {
		crate::log_debug!(
			"HTTP health check for '{}': ✅ Healthy (client connection alive)",
			server.name()
		);
		return Ok(HttpHealthResult::Healthy);
	}

	// No live connection — try to (re)connect as the health probe.
	crate::mcp::client::disconnect(server.name());
	match crate::mcp::client::connect_http(server).await {
		Ok(_) => {
			crate::log_debug!(
				"HTTP health check for '{}': ✅ Healthy (reconnected)",
				server.name()
			);
			Ok(HttpHealthResult::Healthy)
		}
		Err(e) => {
			let msg = e.to_string();
			let lower = msg.to_lowercase();
			// 401/403 means reachable but auth/config failed — don't treat as dead.
			if lower.contains("401")
				|| lower.contains("403")
				|| lower.contains("unauthorized")
				|| lower.contains("forbidden")
			{
				crate::log_error!(
					"HTTP health check for '{}': 🔒 Authentication failed - check your credentials ({})",
					server.name(),
					msg
				);
				Ok(HttpHealthResult::Unreachable)
			} else {
				crate::log_error!(
					"HTTP health check for '{}': ❌ Connection failed - {}",
					server.name(),
					msg
				);
				Ok(HttpHealthResult::Dead)
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use serial_test::serial;
	use std::collections::HashMap;

	fn template_config() -> Config {
		let mut config: Config =
			toml::from_str(include_str!("../../config-templates/default.toml"))
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

	#[serial]
	#[tokio::test]
	async fn http_health_check_classifies_auth_failure_as_unreachable() {
		let url = spawn_health_stub(401).await;
		let server = McpServerConfig::http("hm-test-401", &url, 2, vec![]);
		let result = perform_http_health_check(&server)
			.await
			.expect("health probe must classify, not fail");
		assert!(matches!(result, HttpHealthResult::Unreachable));
	}

	#[serial]
	#[tokio::test]
	async fn http_health_check_classifies_refused_connection_as_dead() {
		// Bind then drop a listener to get a guaranteed-closed port.
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
			.await
			.expect("bind");
		let addr = listener.local_addr().expect("addr");
		drop(listener);
		let server = McpServerConfig::http("hm-test-refused", &format!("http://{addr}"), 2, vec![]);
		let result = perform_http_health_check(&server)
			.await
			.expect("health probe must classify, not fail");
		assert!(matches!(result, HttpHealthResult::Dead));
	}

	#[serial]
	#[tokio::test]
	async fn health_check_records_unreachable_for_auth_rejecting_http_server() {
		const NAME: &str = "hm-test-403";
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
}

#[cfg(test)]
#[path = "health_monitor_tests.rs"]
mod additional_tests;
