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

// MCP local server process manager

use super::{McpFunction, McpToolCall, McpToolResult};
use crate::config::{McpConnectionType, McpServerConfig};
use anyhow::Result;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime};
use tokio::time::sleep;

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

// Type alias for the in-flight JoinHandle slot shared between the global map and call sites.
type InFlightHandle =
	Arc<std::sync::Mutex<Option<tokio::task::JoinHandle<anyhow::Result<serde_json::Value>>>>>;

/// Shared buffer collecting recent stderr lines from a server process.
type StderrBuffer = Arc<std::sync::Mutex<Vec<String>>>;

// Global process registry to keep track of running server processes
lazy_static::lazy_static! {
	pub static ref SERVER_PROCESSES: Arc<RwLock<HashMap<String, Arc<Mutex<ServerProcess>>>>> =
	Arc::new(RwLock::new(HashMap::new()));

	// In-flight spawn_blocking handles, keyed by server name.
	// Stored OUTSIDE ServerProcess so they can be accessed without locking the process mutex.
	// When tokio::select! cancels a tool call, the blocking thread keeps running while holding
	// the ServerProcess mutex. The next call reads this map (no mutex needed) to await the
	// previous handle before trying to lock the process — preventing deadlock.
	static ref SERVER_IN_FLIGHT: Arc<RwLock<HashMap<String, InFlightHandle>>> =
		Arc::new(RwLock::new(HashMap::new()));

	// Reference counts for shared MCP server processes.
	// Each call to ensure_server_running() increments the count; release_server() decrements it.
	// cleanup_server_process() only kills the OS process when the count reaches zero,
	// preventing one session from tearing down a server that another session is still using.
	// stop_all_servers() bypasses ref counts — it is only called on process shutdown.
	// Reference counts for shared MCP server processes.
	// Each call to ensure_server_running() increments the count; release_server() decrements it.
	// cleanup_server_process() only kills the OS process when the count reaches zero,
	// preventing one session from tearing down a server that another session is still using.
	// stop_all_servers() bypasses ref counts — it is only called on process shutdown.
	static ref SERVER_REF_COUNTS: Arc<RwLock<HashMap<String, usize>>> =
		Arc::new(RwLock::new(HashMap::new()));

	/// Recent stderr lines per server — background reader threads push lines here
	/// so that initialization/runtime errors can be surfaced to the user.
	static ref SERVER_STDERR: Arc<RwLock<HashMap<String, StderrBuffer>>> =
		Arc::new(RwLock::new(HashMap::new()));

	/// Parsed server capabilities from the initialize response.
	/// Stored per server name after successful initialization.
	static ref SERVER_CAPABILITIES: Arc<RwLock<HashMap<String, rmcp::model::InitializeResult>>> =
		Arc::new(RwLock::new(HashMap::new()));
}

// Process group IDs for SIGKILL fallback when ServerProcess mutex is locked.
// Stored OUTSIDE ServerProcess so we can kill processes even when busy.
// Unix-only: used to send SIGKILL to -pgid when try_lock() fails.
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

/// Derive a stable project identifier: SHA-256 of the git remote origin URL if available,
/// otherwise SHA-256 of the absolute working directory path.
pub fn derive_project_id() -> String {
	use sha2::{Digest, Sha256};
	let source = std::process::Command::new("git")
		.args(["remote", "get-url", "origin"])
		.output()
		.ok()
		.filter(|o| o.status.success())
		.and_then(|o| String::from_utf8(o.stdout).ok())
		.map(|s| s.trim().to_string())
		.filter(|s| !s.is_empty())
		.unwrap_or_else(|| {
			std::env::current_dir()
				.unwrap_or_default()
				.to_string_lossy()
				.into_owned()
		});
	let hash = Sha256::digest(source.as_bytes());
	hex::encode(hash)[..16].to_string()
}

/// Derive project ID from a specific path (for session-scoped context).
pub fn derive_project_id_from_path(path: &std::path::Path) -> String {
	use sha2::{Digest, Sha256};
	let source = std::process::Command::new("git")
		.args(["remote", "get-url", "origin"])
		.current_dir(path)
		.output()
		.ok()
		.filter(|o| o.status.success())
		.and_then(|o| String::from_utf8(o.stdout).ok())
		.map(|s| s.trim().to_string())
		.filter(|s| !s.is_empty())
		.unwrap_or_else(|| path.to_string_lossy().into_owned());
	let hash = Sha256::digest(source.as_bytes());
	hex::encode(hash)[..16].to_string()
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
/// `session_id` should be captured before entering `spawn_blocking` contexts,
/// since task-local `CURRENT_SESSION_ID` is not available on blocking OS threads.
fn emit_notification(
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

// Structure to hold either an HTTP or stdin-based server process
pub enum ServerProcess {
	Http(Child),
	Stdin {
		child: Child,
		reader: BufReader<std::process::ChildStdout>,
		writer: BufWriter<std::process::ChildStdin>,
		next_id: Arc<AtomicU64>,      // Thread-safe ID counter
		is_shutdown: Arc<AtomicBool>, // Track shutdown state
	},
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
			ServerProcess::Stdin {
				child,
				is_shutdown,
				writer,
				..
			} => {
				// Mark as shutdown first to prevent new communications
				is_shutdown.store(true, Ordering::SeqCst);

				// Try graceful shutdown: flush and close stdin first
				if let Err(e) = writer.flush() {
					crate::log_debug!("Failed to flush stdin before shutdown: {}", e);
				}
				// Note: writer will be dropped when the struct is dropped, closing stdin

				// Give process a moment to terminate gracefully after stdin close
				std::thread::sleep(std::time::Duration::from_millis(100));

				// Check if process terminated gracefully
				match child.try_wait() {
					Ok(Some(_)) => {
						crate::log_debug!("Process terminated gracefully after stdin close");
						return Ok(());
					}
					Ok(None) => {
						crate::log_debug!(
							"Process didn't terminate after stdin close, sending SIGTERM"
						);
					}
					Err(e) => {
						crate::log_debug!("Error checking process status: {}", e);
					}
				}

				// Send SIGTERM to the process group first — gives the server a chance
				// to clean up child processes (e.g. kill_on_drop on shell children).
				#[cfg(unix)]
				{
					let pid = child.id();
					let pgid = pid as libc::pid_t;
					// SAFETY: libc::kill is always safe to call with valid arguments.
					unsafe {
						libc::kill(-pgid, libc::SIGTERM);
					}
					crate::log_debug!(
						"Sent SIGTERM to process group {} for graceful shutdown",
						pgid
					);
					std::thread::sleep(std::time::Duration::from_millis(200));

					// Check if SIGTERM was enough
					match child.try_wait() {
						Ok(Some(_)) => {
							crate::log_debug!("Process terminated after SIGTERM");
							return Ok(());
						}
						_ => {
							crate::log_debug!("Process still alive after SIGTERM, sending SIGKILL");
						}
					}

					// SIGKILL the entire process group as final backstop
					unsafe {
						libc::kill(-pgid, libc::SIGKILL);
					}
				}

				// Fallback for non-unix or if pid was unavailable
				#[cfg(not(unix))]
				{
					let _ = child.kill();
				}

				// Wait for process termination with timeout
				let start = std::time::Instant::now();
				let timeout = std::time::Duration::from_secs(5);
				while start.elapsed() < timeout {
					match child.try_wait() {
						Ok(Some(_)) => return Ok(()), // Process terminated
						Ok(None) => std::thread::sleep(std::time::Duration::from_millis(100)),
						Err(e) => {
							return Err(anyhow::anyhow!("Error waiting for stdin process: {}", e))
						}
					}
				}

				crate::log_debug!("Stdin process did not terminate within timeout, may be zombie");
				Ok(())
			}
		}
	}

	pub fn try_wait(&mut self) -> Result<Option<std::process::ExitStatus>> {
		match self {
			ServerProcess::Http(child) => child
				.try_wait()
				.map_err(|e| anyhow::anyhow!("Failed to check HTTP process: {}", e)),
			ServerProcess::Stdin { child, .. } => child
				.try_wait()
				.map_err(|e| anyhow::anyhow!("Failed to check stdin process: {}", e)),
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

	// Increment ref count on success so cleanup_server_process() knows how many
	// sessions are actively using this server.
	if result.is_ok() {
		let mut counts = SERVER_REF_COUNTS.write().unwrap();
		*counts.entry(server_id.to_string()).or_insert(0) += 1;
		crate::log_debug!("Server '{}' ref count: {}", server_id, counts[server_id]);
	}

	crate::log_debug!("Completed server '{}' check", server_id);

	result
}

// Simple function to start server once if it's truly not running
async fn start_server_once_if_needed(server: &McpServerConfig) -> Result<String> {
	let server_id = server.name();

	// Check if the server is already running and healthy.
	//
	// CRITICAL: Use try_lock() on the process mutex — NEVER .lock() from this
	// async context. The ServerProcess mutex is held for the full read_line()
	// duration by in-flight spawn_blocking tasks (potentially minutes while a
	// MCP child runs `cargo test` etc). Blocking a tokio worker here cascades
	// into runtime starvation → signal handler can't run → Ctrl+C deadlocks.
	// If the mutex is held, the server is by definition alive and healthy —
	// just return the URL without touching it.
	{
		let processes = SERVER_PROCESSES.read().unwrap();
		if let Some(process_arc) = processes.get(server_id) {
			match process_arc.try_lock() {
				Ok(mut process) => {
					// Check if the process is still alive and not marked as shutdown
					let is_alive = match &mut *process {
						ServerProcess::Http(child) => child
							.try_wait()
							.map(|status| status.is_none())
							.unwrap_or(false),
						ServerProcess::Stdin {
							child, is_shutdown, ..
						} => {
							let process_alive = child
								.try_wait()
								.map(|status| status.is_none())
								.unwrap_or(false);
							let not_marked_shutdown = !is_shutdown.load(Ordering::SeqCst);
							process_alive && not_marked_shutdown
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

						match server.connection_type() {
							McpConnectionType::Http => return get_server_url(server),
							McpConnectionType::Stdin => {
								return Ok("stdin://".to_string() + server_id)
							}
							McpConnectionType::Builtin => {
								unreachable!("Builtin servers should not use this function")
							}
						}
					} else {
						// Server process exists but is dead - clean it up
						crate::log_info!(
							"Server '{}' process is dead - cleaning up before restart",
							server_id
						);

						// Try to clean up the dead process
						if let Err(e) = process.kill() {
							crate::log_debug!(
								"Failed to kill dead server process '{}': {}",
								server_id,
								e
							);
						}

						// Mark as dead
						{
							let mut restart_info_guard = SERVER_RESTART_INFO.write().unwrap();
							let info = restart_info_guard.entry(server_id.to_string()).or_default();
							info.health_status = ServerHealth::Dead;
						}
					}
				}
				Err(_) => {
					// Mutex held by in-flight I/O — server is actively processing a request,
					// so it's alive and healthy. Return the URL directly without blocking.
					{
						let mut restart_info_guard = SERVER_RESTART_INFO.write().unwrap();
						let info = restart_info_guard.entry(server_id.to_string()).or_default();
						info.health_status = ServerHealth::Running;
						info.last_health_check = Some(SystemTime::now());
					}

					crate::log_debug!(
						"Server '{}' is busy (in-flight request) — treating as healthy",
						server_id
					);

					match server.connection_type() {
						McpConnectionType::Http => return get_server_url(server),
						McpConnectionType::Stdin => return Ok("stdin://".to_string() + server_id),
						McpConnectionType::Builtin => {
							unreachable!("Builtin servers should not use this function")
						}
					}
				}
			}
		} else {
			// Server not in registry - needs initial start
			crate::log_debug!(
				"Server '{}' not found in registry - needs initial start",
				server_id
			);
		}
	}

	// Clean up dead server from registry before starting new one
	{
		let mut processes = SERVER_PROCESSES.write().unwrap();
		processes.remove(server_id);
	}
	{
		let mut in_flight_map = SERVER_IN_FLIGHT.write().unwrap();
		in_flight_map.remove(server_id);
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
			crate::log_error!("Failed to start server '{}': {}", server_id, e);
			Err(anyhow::anyhow!(
				"Failed to start server '{}': {}",
				server_id,
				e
			))
		}
	}
}
// Start a server process based on configuration
//
// CRITICAL FIX: MCP servers are isolated from parent process group to prevent
// them from being killed when Ctrl+C is pressed. MCP servers should be long-running
// and only terminate when the main program exits, not on session cancellation.
async fn start_server_process(server: &McpServerConfig) -> Result<String> {
	// Get command and args from config based on server type
	let (command, args) = match server {
		McpServerConfig::Stdin { command, args, .. } => (command.as_str(), args.as_slice()),
		McpServerConfig::Http { url, .. } => {
			return Err(anyhow::anyhow!(
					"HTTP server '{}' should not be started as a process (URL: {}) - use Stdin type for local processes",
					server.name(),
					url
				));
		}
		McpServerConfig::Builtin { .. } => {
			return Err(anyhow::anyhow!(
				"Builtin server '{}' should not be started as external process",
				server.name()
			));
		}
	};

	// Build and start the command
	let mut cmd = Command::new(command);

	// Add arguments if present
	if !args.is_empty() {
		cmd.args(args);
	}

	// CRITICAL FIX: Isolate MCP server processes from parent process group
	// This prevents them from receiving SIGINT when Ctrl+C is pressed in the terminal
	#[cfg(unix)]
	{
		use std::os::unix::process::CommandExt;
		cmd.process_group(0); // Create new process group (equivalent to setsid)
	}

	// On Windows, use CREATE_NEW_PROCESS_GROUP to isolate the process
	#[cfg(windows)]
	{
		use std::os::windows::process::CommandExt;
		cmd.creation_flags(0x00000200); // CREATE_NEW_PROCESS_GROUP
	}

	// Configure standard I/O based on connection type
	match server.connection_type() {
		McpConnectionType::Http => {
			// For HTTP mode, we pipe stdout/stderr but don't need stdin
			cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

			// Start the process with signal isolation
			// Debug output
			crate::log_debug!(
				"🚀 Starting MCP server (HTTP mode, signal-isolated): {}",
				server.name()
			);
			let child = cmd.spawn().map_err(|e| {
				anyhow::anyhow!("Failed to start MCP server '{}': {}", server.name(), e)
			})?;

			// Store PGID for SIGKILL fallback when mutex is locked
			#[cfg(unix)]
			{
				let mut pgids = SERVER_PGIDS.write().unwrap();
				pgids.insert(server.name().to_string(), child.id() as libc::pid_t);
			}

			// Add to the registry
			{
				let mut processes = SERVER_PROCESSES.write().unwrap();
				processes.insert(
					server.name().to_string(),
					Arc::new(Mutex::new(ServerProcess::Http(child))),
				);
			}

			// Clear function cache for this server since it's restarting
			crate::mcp::server::clear_function_cache_for_server(server.name());

			// Wait a moment to let the server start
			let start_time = Instant::now();
			let max_wait = Duration::from_secs(10); // Maximum 10 seconds to wait for server to start

			// For local servers, we assume they're running on localhost on some port
			// The URL could be specified in the configuration or we use a default
			let server_url = get_server_url(server)?;

			// Wait for the server to be available
			loop {
				// If it's been too long, give up
				if start_time.elapsed() > max_wait {
					return Err(anyhow::anyhow!(
						"Timed out waiting for MCP server to start: {}",
						server.name()
					));
				}

				// Try to connect to the server
				if can_connect(&server_url).await {
					// Debug output
					crate::log_debug!("✅ MCP server started: {} at {}", server.name(), server_url);
					return Ok(server_url);
				}

				// Wait a bit before trying again
				sleep(Duration::from_millis(500)).await;
			}
		}
		McpConnectionType::Stdin => {
			// For stdin mode, we need bidirectional communication
			cmd.stdin(Stdio::piped())
				.stdout(Stdio::piped())
				.stderr(Stdio::piped());

			// Start the process with signal isolation
			// Debug output
			crate::log_debug!(
				"🚀 Starting MCP server (stdin mode, signal-isolated): {}",
				server.name()
			);
			let mut child = cmd.spawn().map_err(|e| {
				anyhow::anyhow!("Failed to start MCP server '{}': {}", server.name(), e)
			})?;

			// Store PGID for SIGKILL fallback when mutex is locked
			#[cfg(unix)]
			{
				let mut pgids = SERVER_PGIDS.write().unwrap();
				pgids.insert(server.name().to_string(), child.id() as libc::pid_t);
			}

			// Get the stdin/stdout handles
			let child_stdin = child.stdin.take().ok_or_else(|| {
				anyhow::anyhow!("Failed to open stdin for MCP server: {}", server.name())
			})?;

			let child_stdout = child.stdout.take().ok_or_else(|| {
				anyhow::anyhow!("Failed to open stdout for MCP server: {}", server.name())
			})?;

			// Drain stderr in a background thread to prevent pipe buffer deadlock.
			// Keeps last N lines so init-failure diagnostics are available.
			let stderr_buf: Arc<std::sync::Mutex<Vec<String>>> =
				Arc::new(std::sync::Mutex::new(Vec::new()));
			{
				let mut map = SERVER_STDERR.write().unwrap();
				map.insert(server.name().to_string(), stderr_buf.clone());
			}
			if let Some(child_stderr) = child.stderr.take() {
				let buf = stderr_buf;
				let sname = server.name().to_string();
				std::thread::spawn(move || {
					let reader = BufReader::new(child_stderr);
					for line in reader.lines() {
						match line {
							Ok(l) => {
								let trimmed = l.trim().to_string();
								if !trimmed.is_empty() {
									crate::log_debug!("MCP '{}' stderr: {}", sname, trimmed);
									if let Ok(mut b) = buf.lock() {
										b.push(trimmed);
										// Keep only last 50 lines
										if b.len() > 50 {
											let drain_count = b.len() - 50;
											b.drain(..drain_count);
										}
									}
								}
							}
							Err(_) => break,
						}
					}
				});
			}

			// Create buffered reader/writer
			let writer = BufWriter::new(child_stdin);
			let reader = BufReader::new(child_stdout);

			// Create the server process structure with atomic counters and state
			let server_process = ServerProcess::Stdin {
				child,
				reader,
				writer,
				next_id: Arc::new(AtomicU64::new(1)),
				is_shutdown: Arc::new(AtomicBool::new(false)),
			};

			// Register a fresh in-flight handle slot for this server (outside the process mutex).
			{
				let mut in_flight_map = SERVER_IN_FLIGHT.write().unwrap();
				in_flight_map.insert(
					server.name().to_string(),
					Arc::new(std::sync::Mutex::new(None)),
				);
			}

			// Add to the registry
			{
				let mut processes = SERVER_PROCESSES.write().unwrap();
				processes.insert(
					server.name().to_string(),
					Arc::new(Mutex::new(server_process)),
				);
			}

			// Clear function cache for this server since it's restarting
			crate::mcp::server::clear_function_cache_for_server(server.name());

			// Initialize the server by sending the initialize request, following the MCP protocol
			// This also verifies the server is responsive
			let _process_arc = {
				let processes = SERVER_PROCESSES.read().unwrap();
				processes.get(server.name()).cloned().ok_or_else(|| {
					anyhow::anyhow!("Server not found right after creation: {}", server.name())
				})?
			};

			// Initialize the server following the MCP protocol
			let init_result = initialize_stdin_server(server.name()).await;

			if let Err(e) = &init_result {
				// Collect stderr output for diagnostics
				let stderr_lines = {
					let map = SERVER_STDERR.read().unwrap();
					map.get(server.name())
						.and_then(|buf| buf.lock().ok().map(|b| b.clone()))
						.unwrap_or_default()
				};

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

				// Use the proper cleanup function to kill the process
				if let Err(cleanup_err) = cleanup_server_process(server.name()) {
					crate::log_debug!(
						"Failed to cleanup server '{}' after init failure: {}",
						server.name(),
						cleanup_err
					);
				}

				return Err(anyhow::anyhow!(
					"Failed to initialize stdin MCP server '{}': {}{}",
					server.name(),
					e,
					stderr_detail
				));
			}

			// Return a pseudo-URL for stdin-based servers
			let stdin_url = format!("stdin://{}", server.name());
			// Debug output
			// println!("MCP server started and initialized (stdin mode): {} at {}", server.name(), stdin_url);
			Ok(stdin_url)
		}
		McpConnectionType::Builtin => Err(anyhow::anyhow!(
			"Builtin servers should not use process management"
		)),
	}
}

// Initialize a stdin-based server following the MCP protocol
async fn initialize_stdin_server(server_name: &str) -> Result<()> {
	let (role, spec, project, session_id, workdir) = get_session_context();
	// Construct an initialize message according to the MCP protocol
	let session_obj = serde_json::json!({
		"role": role,
		"spec": spec,
		"project": project,
		"session_id": session_id,
		"workdir": workdir,
	});
	let init_message = json!({
		"jsonrpc": "2.0",
		"id": 1,  // Use ID 1 for initialization
		"method": "initialize",
		"params": {
			"clientInfo": {
				"name": "octomind",
				"version": env!("CARGO_PKG_VERSION")
			},
			"protocolVersion": "2025-03-26",
			"capabilities": {
				"experimental": {
					"session": session_obj
				}
			}
		}
	});

	// Send the initialize message and get the response with explicit ID 1 and no cancellation token for init
	let response = communicate_with_stdin_server(server_name, &init_message, 1, None).await?;

	// Check for JSON-RPC errors
	if let Some(error) = response.get("error") {
		return Err(anyhow::anyhow!(
			"Server returned error during initialization: {}",
			error
		));
	}

	// Parse and store server capabilities from the initialize result
	if let Some(result_value) = response.get("result").cloned() {
		match serde_json::from_value::<rmcp::model::InitializeResult>(result_value) {
			Ok(init_info) => {
				crate::log_debug!(
					"Stdin server '{}': {} v{}, protocol {}",
					server_name,
					init_info.server_info.name,
					init_info.server_info.version,
					init_info.protocol_version
				);
				if let Some(ref instructions) = init_info.instructions {
					crate::log_debug!("Server '{}' instructions: {}", server_name, instructions);
				}
				store_server_capabilities(server_name, init_info);
			}
			Err(e) => {
				crate::log_debug!(
					"Failed to parse InitializeResult for '{}': {}",
					server_name,
					e
				);
			}
		}
	} else {
		return Err(anyhow::anyhow!(
			"Server did not return a valid result during initialization"
		));
	}

	// Send initialized notification
	let initialized_message = json!({
		"jsonrpc": "2.0",
		"method": "notifications/initialized",
		"params": {}
	});

	if let Err(e) = send_stdin_notification(server_name, &initialized_message).await {
		crate::log_error!(
			"Warning: Error sending initialized notification to MCP server: {}",
			e
		);
	}

	// If we reach here, initialization was successful
	Ok(())
}

/// Store parsed server capabilities after successful initialization.
pub fn store_server_capabilities(server_name: &str, init_result: rmcp::model::InitializeResult) {
	let mut caps = SERVER_CAPABILITIES.write().unwrap();
	caps.insert(server_name.to_string(), init_result);
}

/// Retrieve stored server capabilities (if the server has been initialized).
pub fn get_server_capabilities(server_name: &str) -> Option<rmcp::model::InitializeResult> {
	let caps = SERVER_CAPABILITIES.read().unwrap();
	caps.get(server_name).cloned()
}

/// Get the server instructions string (if provided during initialization).
pub fn get_server_instructions(server_name: &str) -> Option<String> {
	let caps = SERVER_CAPABILITIES.read().unwrap();
	caps.get(server_name).and_then(|c| c.instructions.clone())
}

// Try to connect to a server to see if it's running
async fn can_connect(url: &str) -> bool {
	// Skip connection check for stdin servers
	if url.starts_with("stdin://") {
		return true;
	}

	// Simple HTTP request to check if server is responding
	match reqwest::Client::new().get(url).send().await {
		Ok(response) => response.status().is_success(),
		Err(_) => false,
	}
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

// Communicate with a stdin-based MCP server using JSON-RPC format with atomic ID generation
pub async fn communicate_with_stdin_server(
	server_name: &str,
	message: &Value,
	override_id: u64,
	cancellation_token: Option<tokio::sync::watch::Receiver<bool>>,
) -> Result<Value> {
	communicate_with_stdin_server_extended_timeout(
		server_name,
		message,
		override_id,
		15,
		cancellation_token,
	)
	.await
}

// Core communication function with atomic ID generation and cancellation handling
pub async fn communicate_with_stdin_server_extended_timeout(
	server_name: &str,
	message: &Value,
	override_id: u64,
	timeout_seconds: u64,
	cancellation_token: Option<tokio::sync::watch::Receiver<bool>>,
) -> Result<Value> {
	// Early cancellation check
	if let Some(ref token) = cancellation_token {
		if *token.borrow() {
			return Err(anyhow::anyhow!("Operation cancelled before communication"));
		}
	}

	// Get the server process safely
	let server_process = {
		let processes = SERVER_PROCESSES
			.read()
			.map_err(|_| anyhow::anyhow!("Failed to acquire read lock on server processes"))?;
		processes
			.get(server_name)
			.cloned()
			.ok_or_else(|| anyhow::anyhow!("Server not found: {}", server_name))?
	};

	// Get the in-flight handle tracker directly from the global map — no process mutex needed.
	// This is the key fix: previously we had to lock server_process to get in_flight, but if
	// a prior spawn_blocking thread was cancelled mid-I/O it still holds that mutex, causing
	// a deadlock. Now in_flight lives outside the process mutex entirely.
	let in_flight_arc = {
		let in_flight_map = SERVER_IN_FLIGHT.read().unwrap();
		in_flight_map
			.get(server_name)
			.cloned()
			.ok_or_else(|| anyhow::anyhow!("No in-flight slot for server: {}", server_name))?
	};

	// Await the previous in-flight task if any, so its mutex hold is released before we proceed.
	// Use a short timeout: if the previous spawn_blocking thread is stuck on read_line() (e.g.
	// because Ctrl+C fired mid-response and the MCP server is now idle waiting for the next
	// request), waiting forever here causes the session to hang. On timeout we mark the server
	// dead so ensure_server_running() restarts it with a fresh process and mutex — the stuck
	// OS thread will eventually exit when the old server process is killed during cleanup.
	let previous_handle = in_flight_arc.lock().unwrap().take();
	if let Some(handle) = previous_handle {
		let wait_secs = std::time::Duration::from_secs(5);
		if tokio::time::timeout(wait_secs, handle).await.is_err() {
			// Timed out — the OS thread is stuck on read_line(). Mark the server dead so the
			// next call triggers a restart instead of trying to lock the same frozen mutex.
			crate::log_debug!(
				"Previous in-flight task for server '{}' did not finish in time — marking dead for restart",
				server_name
			);
			let mut restart_info_guard = SERVER_RESTART_INFO.write().unwrap();
			let info = restart_info_guard
				.entry(server_name.to_string())
				.or_default();
			info.health_status = ServerHealth::Dead;
			return Err(anyhow::anyhow!(
				"Server '{}' previous operation timed out — will restart on next call",
				server_name
			));
		}
	}

	// Get the request ID and child PID atomically — both extracted in one lock to avoid
	// a second lock acquisition later. The child PID is used to kill the server process
	// on cancellation, which unblocks the read_line() in the blocking thread.
	let (final_message, request_id, child_pid) = {
		let mut process_guard = server_process
			.lock()
			.map_err(|_| anyhow::anyhow!("Failed to acquire lock on server process"))?;

		match &mut *process_guard {
			ServerProcess::Stdin {
				next_id,
				is_shutdown,
				child,
				..
			} => {
				// Check if server is shutdown
				if is_shutdown.load(Ordering::SeqCst) {
					return Err(anyhow::anyhow!("Server {} is shut down", server_name));
				}

				// Get request ID atomically
				let actual_id = if override_id > 0 {
					override_id
				} else {
					next_id.fetch_add(1, Ordering::SeqCst)
				};

				// Prepare message with correct ID
				let mut final_msg = message.clone();
				if let Some(obj) = final_msg.as_object_mut() {
					obj.insert("id".to_string(), json!(actual_id));
					if !obj.contains_key("jsonrpc") {
						obj.insert("jsonrpc".to_string(), json!("2.0"));
					}
				}

				// Capture child PID for cancellation kill
				let pid = child.id();
				(final_msg, actual_id, pid)
			}
			_ => {
				return Err(anyhow::anyhow!(
					"Server {} is not a stdin-based server",
					server_name
				))
			}
		}
	}; // Lock is released here

	// Clone data for the blocking task
	let server_name_for_error = server_name.to_string();
	let server_name_for_closure = server_name.to_string();
	let final_message_clone = final_message.clone();
	let request_id_clone = request_id;
	// Capture session_id before spawn_blocking — task-local CURRENT_SESSION_ID
	// is not available on blocking OS threads.
	let session_id_for_closure = crate::session::context::current_session_id();

	// Shared cancellation flag for the blocking thread. The outer tokio::select!
	// sets this on Ctrl+C so the blocking read_line() loop can exit promptly
	// even when the pipe fd is held open by grandchild processes.
	let cancel_flag = Arc::new(AtomicBool::new(false));
	let cancel_flag_for_blocking = cancel_flag.clone();

	// Execute with timeout and cancellation
	// Spawn the blocking I/O task and keep the JoinHandle separate from the timeout wrapper.
	// This lets us store the handle in in_flight on cancellation so the NEXT call can await it,
	// ensuring the OS thread releases the mutex before we try to lock again.
	let blocking_handle = tokio::task::spawn_blocking(move || {
		// Get a lock on the process
		let mut process = server_process
			.lock()
			.map_err(|_| anyhow::anyhow!("Failed to acquire lock on server process"))?;

		// Ensure this is a stdin-based server and not shutdown
		match &mut *process {
			ServerProcess::Stdin {
				writer,
				reader,
				is_shutdown,
				..
			} => {
				// Double-check shutdown state
				if is_shutdown.load(Ordering::SeqCst) {
					return Err(anyhow::anyhow!(
						"Server {} is shut down",
						server_name_for_closure
					));
				}

				// Serialize message to a string and add newline
				let mut message_str = serde_json::to_string(&final_message_clone)?
					.trim_end()
					.to_string();
				message_str.push('\n');

				// Write the message to the process's stdin
				match writer.write_all(message_str.as_bytes()) {
					Ok(_) => {}
					Err(e) => {
						// Check if this is a broken pipe error (server died)
						if e.kind() == std::io::ErrorKind::BrokenPipe {
							// Mark server as dead and schedule cleanup
							{
								let mut restart_info_guard = SERVER_RESTART_INFO.write().unwrap();
								let info = restart_info_guard
									.entry(server_name_for_closure.clone())
									.or_default();
								info.health_status = ServerHealth::Dead;
							}

							// Schedule server cleanup (but don't do it here to avoid deadlocks)
							crate::log_debug!("Broken pipe detected on write for server '{}', marking for cleanup", server_name_for_closure);

							return Err(anyhow::anyhow!(
								"Server '{}' appears to have died (broken pipe on write). Will attempt restart on next call.",
								server_name_for_closure
							));
						}
						return Err(anyhow::anyhow!("Failed to write to stdin: {}", e));
					}
				}

				match writer.flush() {
					Ok(_) => {}
					Err(e) => {
						// Check if this is a broken pipe error (server died)
						if e.kind() == std::io::ErrorKind::BrokenPipe {
							// Mark server as dead and schedule cleanup
							{
								let mut restart_info_guard = SERVER_RESTART_INFO.write().unwrap();
								let info = restart_info_guard
									.entry(server_name_for_closure.clone())
									.or_default();
								info.health_status = ServerHealth::Dead;
							}

							// Schedule server cleanup (but don't do it here to avoid deadlocks)
							crate::log_debug!("Broken pipe detected on flush for server '{}', marking for cleanup", server_name_for_closure);

							return Err(anyhow::anyhow!(
								"Server '{}' appears to have died (broken pipe on flush). Will attempt restart on next call.",
								server_name_for_closure
							));
						}
						return Err(anyhow::anyhow!("Failed to flush stdin: {}", e));
					}
				}

				// Read lines from stdout, skipping any JSON-RPC notifications
				// (messages with a "method" field but no "id") until we get
				// the actual response for our request.
				//
				// The stdout fd is set to non-blocking so we can periodically
				// check the cancellation flag. Without this, read_line() blocks
				// indefinitely when the MCP server's stdout pipe is held open by
				// grandchild processes (e.g. cargo's build server) even after the
				// server itself is killed on Ctrl+C.
				#[cfg(unix)]
				{
					use std::os::unix::io::AsRawFd;
					let fd = reader.get_ref().as_raw_fd();
					unsafe {
						let flags = libc::fcntl(fd, libc::F_GETFL);
						if flags != -1 {
							libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
						}
					}
				}

				// Accumulate bytes across WouldBlock retries until we see a newline.
				// BufReader::read_line returns Err(WouldBlock) mid-line when the pipe
				// drains before a newline — any bytes already appended to the buffer
				// stay there; we must NOT discard them. Only clear after a complete
				// line has been parsed (success or non-JSON stdout noise).
				let mut response_str = String::new();
				let response = loop {
					// Check cancellation flag set by the outer tokio::select! on Ctrl+C
					if cancel_flag_for_blocking.load(Ordering::Relaxed) {
						return Err(anyhow::anyhow!(
							"Operation cancelled while waiting for server response"
						));
					}

					let len_before = response_str.len();
					let read_result = match reader.read_line(&mut response_str) {
						Ok(n) => n,
						Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
							// No data available yet — sleep briefly and retry. Any bytes
							// already read into response_str remain; next iteration appends.
							std::thread::sleep(std::time::Duration::from_millis(50));
							continue;
						}
						Err(e) => {
							return Err(anyhow::anyhow!("Failed to read from stdout: {}", e));
						}
					};

					// Line not yet complete: Ok(n) with no trailing '\n' means the
					// underlying read returned 0 (EOF) OR the line ended without newline.
					// Distinguish: if n==0 AND nothing was appended this round AND buffer
					// has no data → true EOF. Otherwise keep looping until we see '\n'.
					let ended_with_newline = response_str.ends_with('\n');
					if !ended_with_newline {
						if read_result == 0 && response_str.len() == len_before {
							// True EOF with no pending data — server closed.
							let stderr_hint = {
								let map = SERVER_STDERR.read().unwrap();
								map.get(&server_name_for_closure)
									.and_then(|buf| {
										buf.lock().ok().and_then(|b| {
											let last: Vec<_> =
												b.iter().rev().take(10).cloned().collect();
											if last.is_empty() {
												None
											} else {
												let mut lines = last;
												lines.reverse();
												Some(format!(
													"\nServer stderr:\n  {}",
													lines.join("\n  ")
												))
											}
										})
									})
									.unwrap_or_default()
							};
							return Err(anyhow::anyhow!(
								"Server closed connection while reading response{}",
								stderr_hint
							));
						}
						// Partial line — keep reading.
						continue;
					}

					// Complete line captured. Take ownership so we can reset the
					// accumulator before parsing, ready for the next line.
					let line = std::mem::take(&mut response_str);

					// Parse the line as JSON — non-JSON lines are spec violations (stdout noise).
					// Warn and skip rather than hard-fail so badly-behaved servers still work.
					let msg: Value = match serde_json::from_str(&line) {
						Ok(v) => v,
						Err(_) => {
							let trimmed = line.trim();
							if !trimmed.is_empty() {
								eprintln!(
									"⚠️  MCP '{}' prints: {}",
									server_name_for_closure, trimmed
								);
							}
							continue;
						}
					};

					// JSON-RPC notifications have a "method" field but no "id".
					// Forward them and keep reading for the real response.
					if msg.get("method").is_some() && msg.get("id").is_none() {
						let method = msg
							.get("method")
							.and_then(|m| m.as_str())
							.unwrap_or("unknown");
						let params = msg
							.get("params")
							.cloned()
							.unwrap_or(serde_json::Value::Null);
						emit_notification(
							&server_name_for_closure,
							method,
							&params,
							session_id_for_closure.as_deref(),
						);
						continue;
					}

					break msg;
				};

				// Verify the response ID matches the request ID
				let response_id = response.get("id").and_then(|id| id.as_u64()).unwrap_or(0);
				if response_id != request_id_clone && override_id > 0 {
					// Only check ID matching if override_id is provided
					return Err(anyhow::anyhow!(
						"Response ID {} does not match request ID {}",
						response_id,
						request_id_clone
					));
				}

				Ok(response)
			}
			ServerProcess::Http(_) => Err(anyhow::anyhow!(
				"Server {} is not a stdin-based server",
				server_name_for_closure
			)),
		}
	});

	// Check for cancellation during the operation with faster polling
	let cancellation_token_clone = cancellation_token.clone();
	let cancellation_future = async move {
		if let Some(mut token) = cancellation_token_clone {
			// Zero-CPU wait: watch::Receiver::changed() sleeps until the value changes
			while !*token.borrow() {
				if token.changed().await.is_err() {
					break;
				}
			}
		} else {
			std::future::pending::<()>().await;
		}
	};

	// Race between timeout and cancellation.
	// On cancellation: store the handle in in_flight so the NEXT call awaits it before locking,
	// preventing a deadlock from the OS thread still holding the std::sync::Mutex.
	// Use Option so we can move blocking_handle into whichever branch fires.
	let mut handle_opt = Some(blocking_handle);
	tokio::select! {
		result = tokio::time::timeout(
			std::time::Duration::from_secs(timeout_seconds),
			handle_opt.take().unwrap(),
		) => {
			match result {
				Ok(task_result) => {
					// Task completed within the timeout — clean in_flight slot.
					*in_flight_arc.lock().unwrap() = None;
					task_result?
				}
				Err(_) => {
					// Timeout elapsed. The spawn_blocking thread is still alive,
					// holding the std::sync::Mutex on the server process while it
					// waits for a response that may never come (e.g. the server
					// is itself blocked running a long shell command). Without
					// the teardown below, the NEXT tool call deadlocks trying to
					// lock the same mutex. Mirror the cancellation branch: tell
					// the blocking loop to exit, kill the server's process group
					// so its read pipe EOFs immediately, and mark the server
					// dead so the next call restarts a fresh process.
					cancel_flag.store(true, Ordering::Relaxed);

					#[cfg(unix)]
					{
						let pgid = child_pid as libc::pid_t;
						// SAFETY: libc::kill is always safe to call with valid arguments.
						unsafe {
							libc::kill(-pgid, libc::SIGTERM);
						}
						crate::log_debug!(
							"Sent SIGTERM to process group {} (server '{}') on timeout",
							pgid,
							server_name_for_error
						);
						tokio::spawn(async move {
							tokio::time::sleep(std::time::Duration::from_millis(200)).await;
							unsafe {
								libc::kill(-pgid, libc::SIGKILL);
							}
						});
					}

					{
						let mut restart_info_guard = SERVER_RESTART_INFO.write().unwrap();
						let info = restart_info_guard
							.entry(server_name_for_error.clone())
							.or_default();
						info.health_status = ServerHealth::Dead;
					}

					// in_flight already empty (handle_opt was consumed when the
					// timeout future was constructed); clear defensively.
					*in_flight_arc.lock().unwrap() = None;

					Err(anyhow::anyhow!(
						"Timeout ({} seconds) communicating with stdin server: {}",
						timeout_seconds,
						server_name_for_error
					))
				}
			}
		},
		_ = cancellation_future => {
			// Signal the blocking thread to stop polling read_line() immediately.
			// This is the primary cancellation mechanism — even if the pipe fd is
			// held open by grandchild processes, the thread will exit within 50ms.
			cancel_flag.store(true, Ordering::Relaxed);

			// Store the still-running handle so the next call awaits it before locking.
			*in_flight_arc.lock().unwrap() = handle_opt.take();

			// Kill the MCP server child process so the blocking read_line() returns EOF
			// immediately. Without this, the blocking thread stays stuck until the external
			// tool (e.g. `cargo test` inside octofs) finishes on its own — which defeats
			// the purpose of Ctrl+C. The server is in its own process group (set at spawn
			// time), so killing it also kills any grandchild processes it spawned.
			// The server will restart automatically on the next tool call.
			#[cfg(unix)]
			{
				let pgid = child_pid as libc::pid_t;
				// First try SIGTERM — gives the server a chance to clean up child
				// processes gracefully (e.g. octofs's kill_on_drop on shell children).
				// SAFETY: libc::kill is always safe to call with valid arguments.
				unsafe {
					libc::kill(-pgid, libc::SIGTERM);
				}
				crate::log_debug!(
					"Sent SIGTERM to process group {} (server '{}') on cancellation",
					pgid,
					server_name_for_error
				);
				// Fire-and-forget SIGKILL after a brief grace period.
				// Using tokio::spawn avoids blocking a Tokio worker thread with
				// std::thread::sleep, which delays Ctrl+C responsiveness.
				tokio::spawn(async move {
					tokio::time::sleep(std::time::Duration::from_millis(200)).await;
					// SIGKILL to guarantee termination — the server may have ignored
					// SIGTERM or still be cleaning up.
					unsafe {
						libc::kill(-pgid, libc::SIGKILL);
					}
				});
			}

			// Mark the server dead so the next call triggers a restart instead of trying
			// to communicate with the now-killed process.
			{
				let mut restart_info_guard = SERVER_RESTART_INFO.write().unwrap();
				let info = restart_info_guard
					.entry(server_name_for_error.clone())
					.or_default();
				info.health_status = ServerHealth::Dead;
			}

			Err(anyhow::anyhow!("Operation cancelled while communicating with server: {}", server_name_for_error))
		}
	}
}

// Get tool definitions from a stdin-based server with pagination support
pub async fn get_stdin_server_functions(server: &McpServerConfig) -> Result<Vec<McpFunction>> {
	let mut all_functions = Vec::new();
	let mut cursor: Option<String> = None;
	const MAX_PAGES: usize = 20; // Safety limit

	for page in 0..MAX_PAGES {
		let mut params = json!({});
		if let Some(ref c) = cursor {
			params["cursor"] = json!(c);
		}
		let message = json!({
			"jsonrpc": "2.0",
			"id": 1,
			"method": "tools/list",
			"params": params
		});

		crate::log_debug!(
			"tools/list request to '{}' (page {}, cursor: {:?})",
			server.name(),
			page + 1,
			cursor
		);

		let response = communicate_with_stdin_server(server.name(), &message, 1, None).await?;

		// Check for errors in the response
		if let Some(error) = response.get("error") {
			crate::log_error!(
				"Warning: Server returned error during tools/list: {}",
				error
			);
			return Ok(all_functions);
		}

		if let Some(result_value) = response.get("result").cloned() {
			match serde_json::from_value::<rmcp::model::ListToolsResult>(result_value) {
				Ok(list_result) => {
					let next = list_result.next_cursor.clone();
					let functions = crate::mcp::server::parse_tools_from_list_result(&list_result);
					all_functions.extend(functions);

					match next {
						Some(c) if !c.is_empty() => cursor = Some(c),
						_ => break,
					}
				}
				Err(e) => {
					crate::log_debug!("Failed to deserialize ListToolsResult: {}", e);
					break;
				}
			}
		} else {
			crate::log_debug!("Invalid response format from tools/list: {}", response);
			break;
		}
	}

	Ok(all_functions)
}

// Execute a tool on a stdin-based server
pub async fn execute_stdin_tool_call(
	call: &McpToolCall,
	server: &McpServerConfig,
	cancellation_token: Option<tokio::sync::watch::Receiver<bool>>,
) -> Result<McpToolResult> {
	// Debug output
	// println!("Executing tool '{}' on server '{}'", call.tool_name, server.name);

	// Create a call_tool request message following the MCP protocol
	let message = json!({
		"jsonrpc": "2.0",
		"id": 1,
		"method": "tools/call", // Correct MCP method name
		"params": {
		"name": call.tool_name,
		"arguments": call.parameters
	}
	});

	// Execute the tool call with request ID 1 and cancellation support
	let response = match communicate_with_stdin_server_extended_timeout(
		server.name(),
		&message,
		1,
		server.timeout_seconds(),
		cancellation_token,
	)
	.await
	{
		Ok(resp) => resp,
		Err(e) => {
			crate::log_error!("Error executing tool call '{}': {}", call.tool_name, e);
			// Return a formatted error as the tool result rather than failing
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				format!("Error executing tool: {}", e),
			));
		}
	};

	// Debug output
	// println!("Tool call response: {}", response);

	// Check for errors in the response
	if let Some(error) = response.get("error") {
		// Format the error response
		let error_message = error
			.get("message")
			.and_then(|m| m.as_str())
			.unwrap_or("Unknown error");
		let error_code = error.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);

		let _output = json!({
			"error": true,
			"success": false,
			"message": error_message,
			"code": error_code
		});

		return Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("{} (code: {})", error_message, error_code),
		));
	}

	// Deserialize the result directly into CallToolResult
	let call_tool_result = response
		.get("result")
		.cloned()
		.and_then(|v| serde_json::from_value::<rmcp::model::CallToolResult>(v).ok())
		.unwrap_or_else(|| {
			rmcp::model::CallToolResult::success(vec![rmcp::model::Content::text("No result")])
		});

	let tool_result = McpToolResult {
		tool_name: call.tool_name.clone(),
		tool_id: call.tool_id.clone(),
		result: call_tool_result,
	};

	Ok(tool_result)
}

// Stop all running server processes with proper cleanup
pub fn stop_all_servers() -> Result<()> {
	let mut processes = SERVER_PROCESSES.write().unwrap();

	for (name, process_arc) in processes.iter() {
		crate::log_debug!("Stopping MCP server: {}", name);

		// Try to get the process with a timeout
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
				// For busy processes, use PGID to send SIGKILL directly
				#[cfg(unix)]
				{
					let pgids = SERVER_PGIDS.read().unwrap();
					if let Some(pgid) = pgids.get(name) {
						crate::log_debug!(
							"Sending SIGKILL to process group {} for server '{}'",
							pgid,
							name
						);
						unsafe {
							libc::kill(-*pgid, libc::SIGKILL);
						}
					} else {
						crate::log_debug!("No PGID found for server '{}', process may leak", name);
					}
				}
				#[cfg(not(unix))]
				{
					crate::log_debug!("No PGID fallback on non-Unix for server '{}'", name);
				}
			}
		}
	}

	processes.clear();

	// Clear PGIDs (Unix only)
	#[cfg(unix)]
	{
		let mut pgids = SERVER_PGIDS.write().unwrap();
		pgids.clear();
	}

	// Clear all in-flight handles
	{
		let mut in_flight_map = SERVER_IN_FLIGHT.write().unwrap();
		in_flight_map.clear();
	}

	// Clear all function cache when stopping all servers
	crate::mcp::server::clear_all_function_cache();

	// Clear all restart mutexes
	{
		let mut mutexes = SERVER_RESTART_MUTEXES.write().unwrap();
		mutexes.clear();
		crate::log_debug!("Cleared all server restart mutexes");
	}

	// Clear ref counts — process is shutting down, counts no longer meaningful
	{
		let mut counts = SERVER_REF_COUNTS.write().unwrap();
		counts.clear();
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
// Respects reference counting: if other sessions are still using this server,
// the OS process is kept alive and only the caller's ref is decremented.
// Use release_server() for normal session teardown; this function is for
// error recovery paths (init failure) where the ref was never fully established.
pub fn cleanup_server_process(server_name: &str) -> Result<()> {
	// Check ref count — skip kill if other sessions still hold references.
	{
		let counts = SERVER_REF_COUNTS.read().unwrap();
		let refs = counts.get(server_name).copied().unwrap_or(0);
		if refs > 0 {
			crate::log_debug!(
				"Skipping cleanup of server '{}': {} session(s) still using it",
				server_name,
				refs
			);
			return Ok(());
		}
	}

	let mut processes = SERVER_PROCESSES.write().unwrap();

	if let Some(process_arc) = processes.remove(server_name) {
		// Try to kill the process properly
		match process_arc.try_lock() {
			Ok(mut process) => {
				crate::log_debug!("Cleaning up server process '{}'", server_name);
				if let Err(e) = process.kill() {
					crate::log_debug!("Failed to kill server process '{}': {}", server_name, e);
				}
			}
			Err(_) => {
				crate::log_debug!(
					"Could not acquire lock for server '{}' during cleanup, using PGID for SIGKILL",
					server_name
				);
				// For busy processes, use PGID to send SIGKILL directly
				#[cfg(unix)]
				{
					let pgids = SERVER_PGIDS.read().unwrap();
					if let Some(pgid) = pgids.get(server_name) {
						crate::log_debug!(
							"Sending SIGKILL to process group {} for server '{}' during cleanup",
							pgid,
							server_name
						);
						unsafe {
							libc::kill(-*pgid, libc::SIGKILL);
						}
					} else {
						crate::log_debug!(
							"No PGID found for server '{}' during cleanup, process may leak",
							server_name
						);
					}
				}
				#[cfg(not(unix))]
				{
					crate::log_debug!(
						"No PGID fallback on non-Unix for server '{}' during cleanup",
						server_name
					);
				}
			}
		}

		// Clear function cache for this server
		crate::mcp::server::clear_function_cache_for_server(server_name);

		// Clean up PGID (Unix only)
		#[cfg(unix)]
		{
			let mut pgids = SERVER_PGIDS.write().unwrap();
			pgids.remove(server_name);
		}

		// Clean up in-flight handle slot
		{
			let mut in_flight_map = SERVER_IN_FLIGHT.write().unwrap();
			in_flight_map.remove(server_name);
		}

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
	} else {
		Err(anyhow::anyhow!(
			"Server '{}' not found in registry",
			server_name
		))
	}
}

/// Decrement the ref count for a server and clean it up when the count reaches zero.
///
/// Call this when a session is done with a server (e.g., on dynamic server disable/remove
/// or session teardown). The OS process is only killed when no sessions hold references.
pub fn release_server(server_name: &str) {
	let should_cleanup = {
		let mut counts = SERVER_REF_COUNTS.write().unwrap();
		if let Some(count) = counts.get_mut(server_name) {
			if *count > 0 {
				*count -= 1;
			}
			let remaining = *count;
			crate::log_debug!(
				"Server '{}' ref count after release: {}",
				server_name,
				remaining
			);
			if remaining == 0 {
				counts.remove(server_name);
				true
			} else {
				false
			}
		} else {
			false
		}
	};

	if should_cleanup {
		if let Err(e) = cleanup_server_process(server_name) {
			crate::log_debug!(
				"Failed to cleanup server '{}' after release: {}",
				server_name,
				e
			);
		}
	}
}

// Check if a server process is still running with enhanced health tracking
// This function now properly handles different server types.
//
// CRITICAL: This function MUST NOT block on the ServerProcess mutex.
// It is called from async/tokio contexts (health monitor task, /mcp command,
// on-demand health checks). The ServerProcess mutex is a std::sync::Mutex
// held for the full duration of read_line() inside spawn_blocking during
// tool execution — potentially minutes when the MCP server is running a
// long child process like `cargo test`. Using `.lock()` here would block
// a tokio worker thread for that entire duration. With enough concurrent
// health checks, all workers block → signal handler task can't be scheduled
// → Ctrl+C becomes invisible → full runtime deadlock.
//
// The fix: use try_lock(). If the mutex is held, the server IS alive
// (something is actively using it), so return true without blocking.
// If try_lock succeeds, fall through to the normal try_wait() check.
pub fn is_server_running(server_name: &str) -> bool {
	let processes = SERVER_PROCESSES.read().unwrap();
	if let Some(process_arc) = processes.get(server_name) {
		// This is a local server (stdin or local HTTP) - check the process.
		// Non-blocking try_lock: if held by an in-flight spawn_blocking (mid-read_line),
		// the server is by definition alive — return true without blocking tokio worker.
		let is_alive = match process_arc.try_lock() {
			Ok(mut process) => process
				.try_wait()
				.map(|status| status.is_none())
				.unwrap_or(false),
			Err(_) => {
				// Mutex held by active spawn_blocking I/O — server is actively running.
				true
			}
		};

		// Update health status based on actual process state
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
	} else {
		// Server not in process registry - could be a remote server or not started yet
		// For now, mark as unknown and let the proper health check determine status
		{
			let mut restart_info_guard = SERVER_RESTART_INFO.write().unwrap();
			let info = restart_info_guard
				.entry(server_name.to_string())
				.or_default();
			// Don't automatically mark as Dead - let proper health check handle it
			info.last_health_check = Some(SystemTime::now());
		}
		false // Return false for "not running locally" but don't mark as Dead
	}
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

	let server_names: Vec<String> = {
		let processes = SERVER_PROCESSES.read().unwrap();
		processes.keys().cloned().collect()
	};

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

/// Send a JSON-RPC notification to a stdin server (fire-and-forget).
/// Per JSON-RPC spec, notifications have no `id` and expect no response.
/// This avoids the deadlock that occurs when `communicate_with_stdin_server`
/// injects an `id` and blocks on `read_line()` for a response that never comes.
async fn send_stdin_notification(server_name: &str, message: &Value) -> Result<()> {
	let server_process = {
		let processes = SERVER_PROCESSES
			.read()
			.map_err(|_| anyhow::anyhow!("Failed to acquire read lock on server processes"))?;
		processes
			.get(server_name)
			.cloned()
			.ok_or_else(|| anyhow::anyhow!("Server not found: {}", server_name))?
	};

	let server_name_owned = server_name.to_string();
	let message_clone = message.clone();

	tokio::task::spawn_blocking(move || {
		let mut process = server_process
			.lock()
			.map_err(|_| anyhow::anyhow!("Failed to acquire lock on server process"))?;

		match &mut *process {
			ServerProcess::Stdin {
				writer,
				is_shutdown,
				..
			} => {
				if is_shutdown.load(Ordering::SeqCst) {
					return Err(anyhow::anyhow!("Server {} is shut down", server_name_owned));
				}

				// Serialize without injecting an id — notifications MUST NOT have one
				let mut message_str = serde_json::to_string(&message_clone)?
					.trim_end()
					.to_string();
				message_str.push('\n');

				writer
					.write_all(message_str.as_bytes())
					.map_err(|e| anyhow::anyhow!("Failed to write notification: {}", e))?;
				writer
					.flush()
					.map_err(|e| anyhow::anyhow!("Failed to flush notification: {}", e))?;

				Ok(())
			}
			_ => Err(anyhow::anyhow!(
				"Server {} is not a stdin-based server",
				server_name_owned
			)),
		}
	})
	.await
	.map_err(|e| anyhow::anyhow!("Blocking task failed: {}", e))?
}
