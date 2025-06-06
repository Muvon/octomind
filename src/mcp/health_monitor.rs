// Copyright 2025 Muvon Un Limited
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

use super::process::{self, ServerHealth};
use crate::config::{Config, McpServerConfig, McpServerType};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::{interval, sleep};

// Global flag to control the health monitor
static HEALTH_MONITOR_RUNNING: AtomicBool = AtomicBool::new(false);

// Health monitoring configuration
const HEALTH_CHECK_INTERVAL_SECONDS: u64 = 30; // Check every 30 seconds
const RESTART_RETRY_DELAY_SECONDS: u64 = 5; // Wait 5 seconds between restart attempts

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

	// Get external servers that need monitoring
	let external_servers: Vec<McpServerConfig> = config
		.mcp
		.servers
		.iter()
		.filter(|server| matches!(server.server_type, McpServerType::External))
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
			.map(|s| s.name.as_str())
			.collect::<Vec<_>>()
			.join(", ")
	);

	// Spawn the monitoring task
	tokio::spawn(async move {
		let mut check_interval = interval(Duration::from_secs(HEALTH_CHECK_INTERVAL_SECONDS));

		loop {
			// Wait for the next check interval
			check_interval.tick().await;

			// Check if we should stop monitoring
			if !HEALTH_MONITOR_RUNNING.load(Ordering::SeqCst) {
				crate::log_debug!("Health monitor stopping");
				break;
			}

			// Perform health check on all external servers
			for server in &external_servers {
				if let Err(e) = check_and_restart_server_if_needed(server).await {
					crate::log_debug!("Health monitor error for server '{}': {}", server.name, e);
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

/// Check a single server's health and restart if needed
async fn check_and_restart_server_if_needed(server: &McpServerConfig) -> Result<(), anyhow::Error> {
	// Get current server health status
	let health_status = process::get_server_health(&server.name);
	let restart_info = process::get_server_restart_info(&server.name);

	crate::log_debug!(
		"Health check: server '{}' status = {:?}, restart_count = {}",
		server.name,
		health_status,
		restart_info.restart_count
	);

	match health_status {
		ServerHealth::Dead => {
			// Server is dead, attempt restart if within limits
			crate::log_debug!(
				"Health monitor detected dead server '{}', attempting restart",
				server.name
			);

			// Check if we should attempt restart (respect cooldown and max attempts)
			if restart_info.restart_count >= 3 {
				crate::log_debug!(
					"Server '{}' has exceeded max restart attempts ({}), marking as failed",
					server.name,
					restart_info.restart_count
				);

				// Mark as failed to prevent further restart attempts
				let mut restart_info_guard = process::SERVER_RESTART_INFO.write().unwrap();
				if let Some(info) = restart_info_guard.get_mut(&server.name) {
					info.health_status = ServerHealth::Failed;
				}
				return Ok(());
			}

			// Check cooldown period
			if let Some(last_restart) = restart_info.last_restart_time {
				let time_since_restart = std::time::SystemTime::now()
					.duration_since(last_restart)
					.unwrap_or(Duration::from_secs(0));

				if time_since_restart < Duration::from_secs(30) {
					crate::log_debug!(
						"Server '{}' is in cooldown period, skipping restart attempt",
						server.name
					);
					return Ok(());
				}
			}

			// Attempt to restart the server
			match restart_server_with_retry(server).await {
				Ok(_) => {
					crate::log_debug!(
						"Health monitor successfully restarted server '{}'",
						server.name
					);
				}
				Err(e) => {
					crate::log_debug!(
						"Health monitor failed to restart server '{}': {}",
						server.name,
						e
					);
				}
			}
		}
		ServerHealth::Failed => {
			// Server is in failed state, check if enough time has passed for a retry
			if let Some(last_restart) = restart_info.last_restart_time {
				let time_since_failure = std::time::SystemTime::now()
					.duration_since(last_restart)
					.unwrap_or(Duration::from_secs(0));

				// Reset failed state after 5 minutes to allow retry
				if time_since_failure > Duration::from_secs(300) {
					crate::log_debug!(
						"Resetting failed state for server '{}' after cooldown period",
						server.name
					);
					if let Err(e) = process::reset_server_failure_state(&server.name) {
						crate::log_debug!(
							"Failed to reset failure state for server '{}': {}",
							server.name,
							e
						);
					}
				}
			}
		}
		ServerHealth::Running => {
			// Server is healthy, no action needed
			// Optionally verify it's actually responsive
			if !verify_server_responsiveness(server).await {
				crate::log_debug!(
					"Server '{}' appears running but not responsive, marking as dead",
					server.name
				);

				// Mark as dead so it will be restarted on next check
				let mut restart_info_guard = process::SERVER_RESTART_INFO.write().unwrap();
				if let Some(info) = restart_info_guard.get_mut(&server.name) {
					info.health_status = ServerHealth::Dead;
				}
			}
		}
		ServerHealth::Restarting => {
			// Server is currently restarting, wait for it to complete
			crate::log_debug!(
				"Server '{}' is currently restarting, waiting...",
				server.name
			);
		}
	}

	Ok(())
}

/// Attempt to restart a server with retry logic
async fn restart_server_with_retry(server: &McpServerConfig) -> Result<(), anyhow::Error> {
	const MAX_RETRY_ATTEMPTS: u32 = 3;
	let mut last_error = None;

	for attempt in 1..=MAX_RETRY_ATTEMPTS {
		crate::log_debug!(
			"Health monitor restart attempt {} for server '{}'",
			attempt,
			server.name
		);

		match process::ensure_server_running(server).await {
			Ok(_) => {
				crate::log_debug!(
					"Health monitor successfully restarted server '{}' on attempt {}",
					server.name,
					attempt
				);
				return Ok(());
			}
			Err(e) => {
				crate::log_debug!(
					"Health monitor restart attempt {} failed for server '{}': {}",
					attempt,
					server.name,
					e
				);
				last_error = Some(e);

				// Wait before next attempt (except for the last one)
				if attempt < MAX_RETRY_ATTEMPTS {
					sleep(Duration::from_secs(RESTART_RETRY_DELAY_SECONDS)).await;
				}
			}
		}
	}

	// All attempts failed
	Err(last_error.unwrap_or_else(|| anyhow::anyhow!("Unknown restart error")))
}

/// Verify that a server is actually responsive (basic health check)
async fn verify_server_responsiveness(server: &McpServerConfig) -> bool {
	// For stdin servers, we can try a simple ping-like operation
	// For HTTP servers, we could do a simple HTTP request

	match server.mode {
		crate::config::McpServerMode::Stdin => {
			// For stdin servers, we'll just check if the process is alive
			// A more sophisticated check could send a simple JSON-RPC ping
			process::is_server_running(&server.name)
		}
		crate::config::McpServerMode::Http => {
			// For HTTP servers, we could implement a simple health check endpoint call
			// For now, just check if the process is running
			process::is_server_running(&server.name)
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
		.filter(|server| matches!(server.server_type, McpServerType::External))
		.cloned()
		.collect();

	for server in &external_servers {
		if let Err(e) = check_and_restart_server_if_needed(server).await {
			crate::log_debug!(
				"Force health check error for server '{}': {}",
				server.name,
				e
			);
		}
	}

	Ok(())
}
