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

// MCP local server process manager.
//
// The MCP wire protocol itself lives in `super::client` (rmcp-based, speaks
// 2026-07-28 with automatic legacy fallback). This module keeps the process
// orchestration around it: start-once/restart bookkeeping, health tracking,
// stderr diagnostics, session context, and notification forwarding.

use crate::config::{McpConnectionType, McpServerConfig};
use anyhow::Result;
use std::collections::HashMap;
use std::process::Child;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime};

// Server health status tracking
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ServerHealth {
	Running,     // Server is healthy and responding correctly
	Dead,        // Server process not running or unreachable (may restart)
	Restarting,  // Server is in the process of restarting
	Failed,      // Server has failed and cannot be restarted
	Unreachable, // Server is reachable but authentication/config failed (e.g., 401/403)
}

// Server restart tracking information
#[derive(Debug, Clone)]
pub struct ServerRestartInfo {
	pub restart_count: u32,
	pub last_restart_time: Option<SystemTime>,
	pub health_status: ServerHealth,
	pub consecutive_failures: u32,
	pub last_health_check: Option<SystemTime>,
}

impl Default for ServerRestartInfo {
	fn default() -> Self {
		Self {
			restart_count: 0,
			last_restart_time: None,
			health_status: ServerHealth::Running,
			consecutive_failures: 0,
			last_health_check: None,
		}
	}
}

// Global server restart tracking with synchronization
lazy_static::lazy_static! {
	pub static ref SERVER_RESTART_INFO: Arc<RwLock<HashMap<String, ServerRestartInfo>>> =
		Arc::new(RwLock::new(HashMap::new()));

	// Per-server restart mutexes to prevent concurrent restart attempts
	static ref SERVER_RESTART_MUTEXES: Arc<RwLock<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> =
		Arc::new(RwLock::new(HashMap::new()));
}

/// Shared buffer collecting recent stderr lines from a server process.
type StderrBuffer = Arc<std::sync::Mutex<Vec<String>>>;

// Global process registry for locally-spawned HTTP server processes.
// Stdio servers are owned by their rmcp client connection (see `super::client`).
lazy_static::lazy_static! {
	pub static ref SERVER_PROCESSES: Arc<RwLock<HashMap<String, Arc<Mutex<ServerProcess>>>>> =
	Arc::new(RwLock::new(HashMap::new()));

	/// Recent stderr lines per server — background reader tasks push lines here
	/// so that initialization/runtime errors can be surfaced to the user.
	static ref SERVER_STDERR: Arc<RwLock<HashMap<String, StderrBuffer>>> =
		Arc::new(RwLock::new(HashMap::new()));

	/// Negotiated server info (protocol version, capabilities, instructions),
	/// stored per server name after a successful connection.
	static ref SERVER_CAPABILITIES: Arc<RwLock<HashMap<String, rmcp::model::ServerPeerInfo>>> =
		Arc::new(RwLock::new(HashMap::new()));
}

// Process group IDs for SIGKILL fallback and shutdown sweeps.
// Unix-only: used to send signals to -pgid.
#[cfg(unix)]
lazy_static::lazy_static! {
	static ref SERVER_PGIDS: Arc<RwLock<HashMap<String, libc::pid_t>>> =
		Arc::new(RwLock::new(HashMap::new()));
}

// Global notification sender — set by the session when WebSocket or JSONL output is active.
// When set, MCP server notifications are forwarded as structured ServerMessage::McpNotification.
// When not set, notifications are buffered and flushed when a sender is registered.
//
// NOTE: These are process-global for CLI mode. For multi-session WebSocket mode,
// use the session-keyed registries in crate::session::context instead.
lazy_static::lazy_static! {
	// CLI-mode notification sender (single session)
	static ref CLI_NOTIFICATION_SENDER: RwLock<Option<tokio::sync::mpsc::UnboundedSender<crate::websocket::ServerMessage>>> =
		RwLock::new(None);

	// CLI-mode pending notifications (buffered before sender is registered)
	static ref CLI_PENDING_NOTIFICATIONS: RwLock<Vec<crate::websocket::ServerMessage>> =
		RwLock::new(Vec::new());
}

// Session context (role + project + workdir) sent to MCP servers during initialization.
// NOTE: This is process-global for CLI mode. For multi-session WebSocket mode,
// use the session-keyed context in crate::session::context.
lazy_static::lazy_static! {
	static ref CLI_SESSION_CONTEXT: RwLock<(String, String, String)> = RwLock::new((String::new(), String::new(), String::new()));
}

/// Derives a stable project identifier via octolib::utils::path_to_id.
pub fn derive_project_id() -> String {
	octolib::utils::path_to_id_cwd()
}

/// Derive project ID from a specific path (for session-scoped context).
pub fn derive_project_id_from_path(path: &std::path::Path) -> String {
	octolib::utils::path_to_id(path)
}
/// Set the session context (role + project + workdir) that will be sent to MCP servers on initialization.
/// Call this before starting MCP servers for a session.
///
/// For multi-session WebSocket mode, this sets the CLI global. Use
/// `session::context::SessionContext` for session-scoped context.
pub fn set_session_context(role: &str, project: &str, workdir: &str) {
	// Check for session-scoped context first (WebSocket mode)
	if let Some(_session_id) = crate::session::context::current_session_id() {
		// In session mode, context is stored per-session in context.rs
		// This CLI global is not used, but we set it for backward compatibility
	}
	// Always set CLI global for backward compatibility
	*CLI_SESSION_CONTEXT.write().unwrap() =
		(role.to_string(), project.to_string(), workdir.to_string());
}

/// Get the session context (role domain, spec, project, session_id, workdir).
/// Splits the full role name on `:` — left part is domain, right part is spec.
/// Local roles like `"developer"` → domain=`"developer"`, spec=`""`.
/// Tap roles like `"doctor:blood"` → domain=`"doctor"`, spec=`"blood"`.
pub fn get_session_context() -> (String, String, String, String, String) {
	let (full_role, project, workdir) = {
		// Check for session-scoped context first (WebSocket mode)
		if let Some(session_id) = crate::session::context::current_session_id() {
			if let Some(role) = crate::session::context::get_session_role(&session_id) {
				let project = crate::session::context::get_session_workdir_anchor(&session_id)
					.map(|p| crate::mcp::process::derive_project_id_from_path(&p))
					.unwrap_or_default();
				let workdir = crate::session::context::get_session_workdir_anchor(&session_id)
					.map(|p| p.to_string_lossy().into_owned())
					.unwrap_or_default();
				(role, project, workdir)
			} else {
				CLI_SESSION_CONTEXT.read().unwrap().clone()
			}
		} else {
			// Fall back to CLI global (CLI mode)
			CLI_SESSION_CONTEXT.read().unwrap().clone()
		}
	};

	let session_id = crate::session::context::current_session_id().unwrap_or_default();

	// Split role into domain + spec
	let (domain, spec) = match full_role.split_once(':') {
		Some((d, s)) => (d.to_string(), s.to_string()),
		None => (full_role, String::new()),
	};

	(domain, spec, project, session_id, workdir)
}

/// Derive and set the project id from the current git remote / cwd, then store role.
pub fn init_session_context(role: &str) {
	let project = derive_project_id();
	let workdir = std::env::current_dir()
		.map(|p| p.to_string_lossy().into_owned())
		.unwrap_or_default();
	set_session_context(role, &project, &workdir);
}

/// Register a channel sender so MCP notifications are forwarded as structured messages.
/// Flushes any notifications that arrived before this call (e.g. during server initialization).
/// Call this when starting a WebSocket or JSONL session.
///
/// For multi-session WebSocket mode, pass session_id to register in session-scoped registry.
/// For CLI mode, pass None to use process-global storage.
pub fn set_notification_sender(
	session_id: Option<String>,
	tx: tokio::sync::mpsc::UnboundedSender<crate::websocket::ServerMessage>,
) {
	match session_id {
		Some(sid) => {
			// Session-scoped (WebSocket mode)
			crate::session::context::register_notification_sender(sid, tx);
		}
		None => {
			// CLI mode - flush buffered notifications first, then register
			let pending = {
				let mut guard = CLI_PENDING_NOTIFICATIONS.write().unwrap();
				std::mem::take(&mut *guard)
			};
			for msg in pending {
				let _ = tx.send(msg);
			}
			let mut guard = CLI_NOTIFICATION_SENDER.write().unwrap();
			*guard = Some(tx);
		}
	}
}

/// Remove the notification sender (e.g. when a session ends).
pub fn clear_notification_sender(session_id: Option<String>) {
	match session_id {
		Some(sid) => {
			crate::session::context::unregister_notification_sender(sid);
		}
		None => {
			let mut guard = CLI_NOTIFICATION_SENDER.write().unwrap();
			*guard = None;
		}
	}
}

/// Send any ServerMessage directly through the notification channel.
/// Uses session-scoped sender if in a session context, otherwise CLI global.
pub fn send_notification_message(msg: crate::websocket::ServerMessage) {
	// Try session-scoped sender first
	if let Some(session_id) = crate::session::context::current_session_id() {
		if let Some(sender) = crate::session::context::get_notification_sender_by_id(&session_id) {
			let _ = sender.send(msg);
			return;
		}
	}
	// Fall back to CLI global
	let sender = CLI_NOTIFICATION_SENDER.read().unwrap();
	if let Some(tx) = sender.as_ref() {
		let _ = tx.send(msg);
	}
	// If no sender is registered (CLI mode), the message is intentionally dropped.
}

/// Emit a notification — structured if a sender is registered, buffered otherwise.
/// Buffered notifications are flushed when set_notification_sender() is called.
///
/// `session_id` should be captured at connection time, since the rmcp service
/// loop runs outside the task-local `CURRENT_SESSION_ID` scope.
pub(crate) fn emit_notification(
	server_name: &str,
	method: &str,
	params: &serde_json::Value,
	session_id: Option<&str>,
) {
	let msg = crate::websocket::ServerMessage::McpNotification(
		crate::websocket::McpNotificationPayload {
			server: server_name.to_string(),
			method: method.to_string(),
			params: params.clone(),
		},
	);

	// Use explicit session_id if provided, otherwise try task-local
	let effective_session_id = session_id
		.map(|s| s.to_string())
		.or_else(crate::session::context::current_session_id);

	// Try session-scoped sender first
	if let Some(sid) = effective_session_id {
		if let Some(sender) = crate::session::context::get_notification_sender_by_id(&sid) {
			let _ = sender.send(msg);
			return;
		}
	}

	// Fall back to CLI global
	let sender = CLI_NOTIFICATION_SENDER.read().unwrap();
	if let Some(tx) = sender.as_ref() {
		// Sender active — forward immediately
		let _ = tx.send(msg);
	} else {
		// No sender yet (e.g. notification arrived during server init before session started).
		// Buffer it so it gets flushed when set_notification_sender() is called.
		drop(sender); // release read lock before taking write lock on PENDING
		CLI_PENDING_NOTIFICATIONS.write().unwrap().push(msg);
	}
}

/// Get (creating if needed) the stderr diagnostic buffer for a server.
pub(crate) fn stderr_buffer_for(server_name: &str) -> StderrBuffer {
	let mut map = SERVER_STDERR.write().unwrap();
	map.entry(server_name.to_string()).or_default().clone()
}

/// Recent stderr lines captured for a server (for failure diagnostics).
fn stderr_lines_for(server_name: &str) -> Vec<String> {
	let map = SERVER_STDERR.read().unwrap();
	map.get(server_name)
		.and_then(|buf| buf.lock().ok().map(|b| b.clone()))
		.unwrap_or_default()
}

/// Record the process group id of a spawned server (Unix signal fallback).
#[cfg(unix)]
pub(crate) fn register_pgid(server_name: &str, pid: u32) {
	let mut pgids = SERVER_PGIDS.write().unwrap();
	pgids.insert(server_name.to_string(), pid as libc::pid_t);
}

#[cfg(not(unix))]
pub(crate) fn register_pgid(_server_name: &str, _pid: u32) {}

/// Check whether the OS process for a stdio server is still alive.
/// Returns `None` when no pid is recorded (non-Unix, or server not started here).
#[cfg(unix)]
pub(crate) fn is_stdio_process_alive(server_name: &str) -> Option<bool> {
	let pgids = SERVER_PGIDS.read().unwrap();
	pgids
		.get(server_name)
		.map(|&pid| unsafe { libc::kill(pid, 0) == 0 })
}

#[cfg(not(unix))]
pub(crate) fn is_stdio_process_alive(_server_name: &str) -> Option<bool> {
	None
}

/// Kill a server's process group: SIGTERM first for graceful cleanup, then SIGKILL.
#[cfg(unix)]
fn kill_process_group(server_name: &str) {
	let pgid = {
		let pgids = SERVER_PGIDS.read().unwrap();
		pgids.get(server_name).copied()
	};
	if let Some(pgid) = pgid {
		// SAFETY: libc::kill is always safe to call with valid arguments.
		unsafe {
			libc::kill(-pgid, libc::SIGTERM);
		}
		std::thread::sleep(Duration::from_millis(200));
		unsafe {
			libc::kill(-pgid, libc::SIGKILL);
		}
		crate::log_debug!(
			"Sent SIGTERM+SIGKILL to process group {} for server '{}'",
			pgid,
			server_name
		);
	}
}

#[cfg(not(unix))]
fn kill_process_group(_server_name: &str) {}

// Locally-spawned HTTP server process. Stdio servers are owned by their rmcp
// client connection and are not tracked here.
pub enum ServerProcess {
	Http(Child),
}

impl ServerProcess {
	pub fn kill(&mut self) -> Result<()> {
		match self {
			ServerProcess::Http(child) => {
				// For HTTP processes, kill immediately
				child
					.kill()
					.map_err(|e| anyhow::anyhow!("Failed to kill HTTP process: {}", e))?;

				// Wait for process termination with timeout
				let start = std::time::Instant::now();
				let timeout = std::time::Duration::from_secs(5);
				while start.elapsed() < timeout {
					match child.try_wait() {
						Ok(Some(_)) => return Ok(()), // Process terminated
						Ok(None) => std::thread::sleep(std::time::Duration::from_millis(100)),
						Err(e) => {
							return Err(anyhow::anyhow!("Error waiting for HTTP process: {}", e))
						}
					}
				}
				crate::log_debug!("HTTP process did not terminate within timeout, may be zombie");
				Ok(())
			}
		}
	}

	pub fn try_wait(&mut self) -> Result<Option<std::process::ExitStatus>> {
		match self {
			ServerProcess::Http(child) => child
				.try_wait()
				.map_err(|e| anyhow::anyhow!("Failed to check HTTP process: {}", e)),
		}
	}
}

// Get or create a restart mutex for a server to prevent concurrent restart attempts
fn get_server_restart_mutex(server_id: &str) -> Arc<tokio::sync::Mutex<()>> {
	let mutexes = SERVER_RESTART_MUTEXES.read().unwrap();
	if let Some(mutex) = mutexes.get(server_id) {
		return mutex.clone();
	}
	drop(mutexes);

	// Create new mutex if not found
	let mut mutexes = SERVER_RESTART_MUTEXES.write().unwrap();
	// Double-check in case another thread created it
	if let Some(mutex) = mutexes.get(server_id) {
		return mutex.clone();
	}

	let new_mutex = Arc::new(tokio::sync::Mutex::new(()));
	mutexes.insert(server_id.to_string(), new_mutex.clone());
	new_mutex
}

// Clean up restart mutex when server is permanently removed
fn cleanup_server_restart_mutex(server_id: &str) {
	let mut mutexes = SERVER_RESTART_MUTEXES.write().unwrap();
	mutexes.remove(server_id);
}

// Start a local MCP server process if not already running - START ONCE approach
// This function will only start servers that are truly not running
pub async fn ensure_server_running(server: &McpServerConfig) -> Result<String> {
	let server_id = server.name();

	// Use per-server mutex to prevent concurrent start attempts
	let restart_mutex = get_server_restart_mutex(server_id);
	let _guard = restart_mutex.lock().await;

	crate::log_debug!("Checking server '{}' status for potential start", server_id);

	let result = start_server_once_if_needed(server).await;

	crate::log_debug!("Completed server '{}' check", server_id);

	result
}

// Simple function to start server once if it's truly not running
async fn start_server_once_if_needed(server: &McpServerConfig) -> Result<String> {
	let server_id = server.name();

	// Check if the server is already running and healthy.
	let is_alive = match server.connection_type() {
		McpConnectionType::Stdin => {
			super::client::is_connected(server_id)
				&& is_stdio_process_alive(server_id).unwrap_or(true)
		}
		McpConnectionType::Http => {
			let processes = SERVER_PROCESSES.read().unwrap();
			match processes.get(server_id) {
				Some(process_arc) => match process_arc.try_lock() {
					Ok(mut process) => process
						.try_wait()
						.map(|status| status.is_none())
						.unwrap_or(false),
					// Mutex held — process is being managed elsewhere, treat as alive.
					Err(_) => true,
				},
				None => false,
			}
		}
		McpConnectionType::Builtin => {
			unreachable!("Builtin servers should not use this function")
		}
	};

	if is_alive {
		// Server is running and healthy - return URL without any restart attempts
		{
			let mut restart_info_guard = SERVER_RESTART_INFO.write().unwrap();
			let info = restart_info_guard.entry(server_id.to_string()).or_default();
			info.health_status = ServerHealth::Running;
			info.last_health_check = Some(SystemTime::now());
		}

		crate::log_debug!("Server '{}' is already running and healthy", server_id);

		return match server.connection_type() {
			McpConnectionType::Http => get_server_url(server),
			McpConnectionType::Stdin => Ok("stdin://".to_string() + server_id),
			McpConnectionType::Builtin => {
				unreachable!("Builtin servers should not use this function")
			}
		};
	}

	// Clean up any dead state before starting fresh.
	super::client::disconnect(server_id);
	{
		let mut processes = SERVER_PROCESSES.write().unwrap();
		processes.remove(server_id);
	}

	// Start the server (this is the ONLY place where we start servers)
	crate::log_info!("Starting MCP server: {}", server_id);

	match start_server_process(server).await {
		Ok(url) => {
			// Server started successfully - update health status
			{
				let mut restart_info_guard = SERVER_RESTART_INFO.write().unwrap();
				let info = restart_info_guard.entry(server_id.to_string()).or_default();
				info.health_status = ServerHealth::Running;
				info.restart_count += 1; // Track that we started it
				info.last_restart_time = Some(SystemTime::now());
				info.last_health_check = Some(SystemTime::now());
				info.consecutive_failures = 0;
			}
			crate::log_info!("Successfully started server '{}'", server_id);
			Ok(url)
		}
		Err(e) => {
			// Server failed to start - mark as failed but don't retry
			{
				let mut restart_info_guard = SERVER_RESTART_INFO.write().unwrap();
				let info = restart_info_guard.entry(server_id.to_string()).or_default();
				info.health_status = ServerHealth::Failed;
				info.consecutive_failures += 1;
			}
			// Already logged with full detail by start_server_process; just
			// propagate so callers (init / health monitor) can react.
			Err(anyhow::anyhow!(
				"Failed to start server '{}': {}",
				server_id,
				e
			))
		}
	}
}

// Start a server process based on configuration.
//
// Stdio servers are spawned and connected by the rmcp client (signal-isolated
// in their own process group so terminal Ctrl+C doesn't kill them). Local
// HTTP server processes are spawned here and polled until reachable.
async fn start_server_process(server: &McpServerConfig) -> Result<String> {
	match server.connection_type() {
		McpConnectionType::Stdin => {
			// Clear function cache for this server since it's (re)starting
			crate::mcp::server::clear_function_cache_for_server(server.name());

			match super::client::connect_stdio(server).await {
				Ok(_service) => Ok(format!("stdin://{}", server.name())),
				Err(e) => {
					let stderr_lines = stderr_lines_for(server.name());
					let stderr_detail = if stderr_lines.is_empty() {
						String::new()
					} else {
						format!("\nServer stderr:\n  {}", stderr_lines.join("\n  "))
					};

					crate::log_error!(
						"Failed to initialize stdin MCP server '{}': {}{}",
						server.name(),
						e,
						stderr_detail
					);

					// The just-spawned process must die — it never completed its handshake.
					if let Err(cleanup_err) = cleanup_server_process(server.name()) {
						crate::log_debug!(
							"Failed to cleanup server '{}' after init failure: {}",
							server.name(),
							cleanup_err
						);
					}

					Err(anyhow::anyhow!(
						"Failed to initialize stdin MCP server '{}': {}{}",
						server.name(),
						e,
						stderr_detail
					))
				}
			}
		}
		McpConnectionType::Http => Err(anyhow::anyhow!(
			"HTTP server '{}' should not be started as a process - use stdio type for local processes",
			server.name()
		)),
		McpConnectionType::Builtin => Err(anyhow::anyhow!(
			"Builtin server '{}' should not be started as external process",
			server.name()
		)),
	}
}

/// Store negotiated server info after a successful connection.
pub fn store_server_capabilities(server_name: &str, info: rmcp::model::ServerPeerInfo) {
	if let Some(server_info) = &info.server_info {
		crate::log_debug!(
			"Server '{}': {} v{}, protocol {}",
			server_name,
			server_info.name,
			server_info.version,
			info.protocol_version
		);
	}
	if let Some(instructions) = &info.instructions {
		crate::log_debug!("Server '{}' instructions: {}", server_name, instructions);
	}
	let mut caps = SERVER_CAPABILITIES.write().unwrap();
	caps.insert(server_name.to_string(), info);
}

/// Retrieve stored server capabilities (if the server has been connected).
pub fn get_server_capabilities(server_name: &str) -> Option<rmcp::model::ServerPeerInfo> {
	let caps = SERVER_CAPABILITIES.read().unwrap();
	caps.get(server_name).cloned()
}

/// Get the server instructions string (if provided during connection).
pub fn get_server_instructions(server_name: &str) -> Option<String> {
	let caps = SERVER_CAPABILITIES.read().unwrap();
	caps.get(server_name).and_then(|c| c.instructions.clone())
}

// Get the URL for a server based on configuration
fn get_server_url(server: &McpServerConfig) -> Result<String> {
	// Check if URL is explicitly specified (remote HTTP server)
	if let Some(url) = server.url() {
		return Ok(url.to_string());
	}

	// For stdin-based servers, return a pseudo-URL
	if let McpConnectionType::Stdin = server.connection_type() {
		return Ok(format!("stdin://{}", server.name()));
	}

	// Otherwise, assume it's running on localhost
	// For now we use a default port, but ideally this would be configurable
	// or the server would output its port when starting
	Ok("http://localhost:8008".to_string())
}

// Stop all running server processes with proper cleanup
pub fn stop_all_servers() -> Result<()> {
	// Disconnect all rmcp client connections (cancels service loops; the
	// child-process transports kill their children on drop).
	super::client::disconnect_all();

	let mut processes = SERVER_PROCESSES.write().unwrap();

	for (name, process_arc) in processes.iter() {
		crate::log_debug!("Stopping MCP server: {}", name);

		match process_arc.try_lock() {
			Ok(mut process) => {
				if let Err(e) = process.kill() {
					crate::log_error!("Failed to kill MCP server '{}': {}", name, e);
				}
			}
			Err(_) => {
				crate::log_debug!(
					"Could not acquire lock for server '{}', using PGID for SIGKILL",
					name
				);
			}
		}
	}

	processes.clear();
	drop(processes);

	// Signal every recorded process group as a final backstop (stdio children
	// that survived transport shutdown, busy HTTP processes).
	#[cfg(unix)]
	{
		let names: Vec<String> = {
			let pgids = SERVER_PGIDS.read().unwrap();
			pgids.keys().cloned().collect()
		};
		for name in names {
			kill_process_group(&name);
		}
		let mut pgids = SERVER_PGIDS.write().unwrap();
		pgids.clear();
	}

	// Clear all function cache when stopping all servers
	crate::mcp::server::clear_all_function_cache();

	// Clear all restart mutexes
	{
		let mut mutexes = SERVER_RESTART_MUTEXES.write().unwrap();
		mutexes.clear();
		crate::log_debug!("Cleared all server restart mutexes");
	}

	// Clear stderr buffers
	{
		let mut stderr_map = SERVER_STDERR.write().unwrap();
		stderr_map.clear();
	}

	// Clear capabilities
	{
		let mut caps = SERVER_CAPABILITIES.write().unwrap();
		caps.clear();
	}

	Ok(())
}

// Cleanup a specific server process (helper function).
// Tears down the client connection and/or OS process and removes every
// registry entry for the server. Used on init failure (the just-spawned
// process must die regardless of who else references the name — it never
// completed its handshake) and on dynamic server removal. A concurrent
// session that still needs the server gets it back automatically via the
// start-once path in ensure_server_running().
pub fn cleanup_server_process(server_name: &str) -> Result<()> {
	let had_client =
		super::client::is_connected(server_name) || super::client::get(server_name).is_some();
	super::client::disconnect(server_name);

	let had_process = {
		let mut processes = SERVER_PROCESSES.write().unwrap();
		if let Some(process_arc) = processes.remove(server_name) {
			match process_arc.try_lock() {
				Ok(mut process) => {
					crate::log_debug!("Cleaning up server process '{}'", server_name);
					if let Err(e) = process.kill() {
						crate::log_debug!("Failed to kill server process '{}': {}", server_name, e);
					}
				}
				Err(_) => {
					crate::log_debug!(
						"Could not acquire lock for server '{}' during cleanup, using PGID",
						server_name
					);
				}
			}
			true
		} else {
			false
		}
	};

	// Signal the process group so stray children die too (stdio servers that
	// spawned their own subprocesses, busy HTTP processes).
	kill_process_group(server_name);
	#[cfg(unix)]
	{
		let mut pgids = SERVER_PGIDS.write().unwrap();
		pgids.remove(server_name);
	}

	if !had_client && !had_process {
		return Err(anyhow::anyhow!(
			"Server '{}' not found in registry",
			server_name
		));
	}

	// Clear function cache for this server
	crate::mcp::server::clear_function_cache_for_server(server_name);

	// Clean up stderr buffer
	{
		let mut stderr_map = SERVER_STDERR.write().unwrap();
		stderr_map.remove(server_name);
	}

	// Clean up capabilities
	{
		let mut caps = SERVER_CAPABILITIES.write().unwrap();
		caps.remove(server_name);
	}

	// Clean up restart mutex
	cleanup_server_restart_mutex(server_name);

	crate::log_debug!("Server '{}' removed from registry", server_name);
	Ok(())
}

// Check if a server is still running with health tracking.
// Stdio servers: the rmcp service loop is alive AND the OS child still exists.
// HTTP processes: try_wait.
pub fn is_server_running(server_name: &str) -> bool {
	let client_alive = super::client::is_connected(server_name);
	let process_state = {
		let processes = SERVER_PROCESSES.read().unwrap();
		processes.get(server_name).map(|process_arc| {
			match process_arc.try_lock() {
				Ok(mut process) => process
					.try_wait()
					.map(|status| status.is_none())
					.unwrap_or(false),
				// Mutex held — something is actively managing the process.
				Err(_) => true,
			}
		})
	};
	// Stdio children can die without rmcp noticing immediately. If we know the
	// pid and the OS child is gone, treat the server as dead even when the
	// service handle hasn't marked itself closed yet.
	let stdio_process_alive = is_stdio_process_alive(server_name);

	let is_alive = match stdio_process_alive {
		Some(false) => false,
		_ => client_alive || process_state.unwrap_or(false),
	};

	{
		let mut restart_info_guard = SERVER_RESTART_INFO.write().unwrap();
		let info = restart_info_guard
			.entry(server_name.to_string())
			.or_default();
		info.health_status = if is_alive {
			ServerHealth::Running
		} else {
			ServerHealth::Dead
		};
		info.last_health_check = Some(SystemTime::now());
	}
	is_alive
}

// Get server health status
pub fn get_server_health(server_name: &str) -> ServerHealth {
	let restart_info_guard = SERVER_RESTART_INFO.read().unwrap();
	restart_info_guard
		.get(server_name)
		.map(|info| info.health_status)
		.unwrap_or(ServerHealth::Dead)
}

// Get server restart information
pub fn get_server_restart_info(server_name: &str) -> ServerRestartInfo {
	let restart_info_guard = SERVER_RESTART_INFO.read().unwrap();
	restart_info_guard
		.get(server_name)
		.cloned()
		.unwrap_or_default()
}

// Reset server failure state (useful for manual recovery)
pub fn reset_server_failure_state(server_name: &str) -> Result<()> {
	let mut restart_info_guard = SERVER_RESTART_INFO.write().unwrap();
	if let Some(info) = restart_info_guard.get_mut(server_name) {
		info.restart_count = 0;
		info.consecutive_failures = 0;
		info.health_status = ServerHealth::Dead; // Will be updated on next check
		crate::log_debug!("Reset failure state for server '{}'", server_name);
		Ok(())
	} else {
		Err(anyhow::anyhow!(
			"Server '{}' not found in restart tracking",
			server_name
		))
	}
}

// Perform health check on all registered servers
pub async fn perform_health_check_all_servers() -> HashMap<String, ServerHealth> {
	let mut health_status = HashMap::new();

	let mut server_names: Vec<String> = {
		let processes = SERVER_PROCESSES.read().unwrap();
		processes.keys().cloned().collect()
	};
	server_names.extend(super::client::connected_names());
	server_names.sort();
	server_names.dedup();

	for server_name in server_names {
		let is_running = is_server_running(&server_name);
		let health = if is_running {
			ServerHealth::Running
		} else {
			ServerHealth::Dead
		};
		health_status.insert(server_name.clone(), health);

		crate::log_debug!("Health check: Server '{}' is {:?}", server_name, health);
	}

	health_status
}

// Get comprehensive server status report
pub fn get_server_status_report() -> HashMap<String, (ServerHealth, ServerRestartInfo)> {
	let mut report = HashMap::new();

	let restart_info_guard = SERVER_RESTART_INFO.read().unwrap();
	for (server_name, info) in restart_info_guard.iter() {
		let current_health = get_server_health(server_name);
		report.insert(server_name.clone(), (current_health, info.clone()));
	}

	report
}
