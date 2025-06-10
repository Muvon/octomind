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

// MCP local server process manager

use super::{McpFunction, McpToolCall, McpToolResult};
use crate::config::{McpServerConfig, McpServerMode};
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
	Running,
	Dead,
	Restarting,
	Failed,
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

// Global process registry to keep track of running server processes
lazy_static::lazy_static! {
	pub static ref SERVER_PROCESSES: Arc<RwLock<HashMap<String, Arc<Mutex<ServerProcess>>>>> =
	Arc::new(RwLock::new(HashMap::new()));
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

				// Give process a moment to terminate gracefully
				std::thread::sleep(std::time::Duration::from_millis(100));

				// Check if process terminated gracefully
				match child.try_wait() {
					Ok(Some(_)) => {
						crate::log_debug!("Process terminated gracefully after stdin close");
						return Ok(());
					}
					Ok(None) => {
						// Process still running, need to force kill
						crate::log_debug!("Process didn't terminate gracefully, force killing");
					}
					Err(e) => {
						crate::log_debug!("Error checking process status: {}", e);
					}
				}

				// Force kill the process
				child
					.kill()
					.map_err(|e| anyhow::anyhow!("Failed to kill stdin process: {}", e))?;

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
	let server_id = &server.name;

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
	let server_id = &server.name;

	// Check if the server is already running and healthy
	{
		let processes = SERVER_PROCESSES.read().unwrap();
		if let Some(process_arc) = processes.get(server_id) {
			let mut process = process_arc.lock().unwrap();

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
					let info = restart_info_guard.entry(server_id.clone()).or_default();
					info.health_status = ServerHealth::Running;
					info.last_health_check = Some(SystemTime::now());
				}

				crate::log_debug!("Server '{}' is already running and healthy", server_id);

				match server.mode {
					McpServerMode::Http => return get_server_url(server),
					McpServerMode::Stdin => return Ok("stdin://".to_string() + server_id),
				}
			} else {
				// Server process exists but is dead - clean it up
				crate::log_info!(
					"Server '{}' process is dead - cleaning up before restart",
					server_id
				);

				// Try to clean up the dead process
				if let Err(e) = process.kill() {
					crate::log_debug!("Failed to kill dead server process '{}': {}", server_id, e);
				}

				// Mark as dead
				{
					let mut restart_info_guard = SERVER_RESTART_INFO.write().unwrap();
					let info = restart_info_guard.entry(server_id.clone()).or_default();
					info.health_status = ServerHealth::Dead;
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

	// Start the server (this is the ONLY place where we start servers)
	crate::log_info!("Starting MCP server: {}", server_id);

	match start_server_process(server).await {
		Ok(url) => {
			// Server started successfully - update health status
			{
				let mut restart_info_guard = SERVER_RESTART_INFO.write().unwrap();
				let info = restart_info_guard.entry(server_id.clone()).or_default();
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
				let info = restart_info_guard.entry(server_id.clone()).or_default();
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

// Start a server process based on configuration
async fn start_server_process(server: &McpServerConfig) -> Result<String> {
	// Get command and args from config
	let command = server.command.as_ref().ok_or_else(|| {
		anyhow::anyhow!(
			"Command not specified for local MCP server: {}",
			server.name
		)
	})?;

	// Build and start the command
	let mut cmd = Command::new(command);

	// Add arguments if present
	if !server.args.is_empty() {
		cmd.args(&server.args);
	}

	// Configure standard I/O based on mode
	match server.mode {
		McpServerMode::Http => {
			// For HTTP mode, we pipe stdout/stderr but don't need stdin
			cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

			// Start the process
			// Debug output
			crate::log_debug!("🚀 Starting MCP server (HTTP mode): {}", server.name);
			let child = cmd.spawn().map_err(|e| {
				anyhow::anyhow!("Failed to start MCP server '{}': {}", server.name, e)
			})?;

			// Add to the registry
			{
				let mut processes = SERVER_PROCESSES.write().unwrap();
				processes.insert(
					server.name.clone(),
					Arc::new(Mutex::new(ServerProcess::Http(child))),
				);
			}

			// Clear function cache for this server since it's restarting
			crate::mcp::server::clear_function_cache_for_server(&server.name);

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
						server.name
					));
				}

				// Try to connect to the server
				if can_connect(&server_url).await {
					// Debug output
					crate::log_debug!("✅ MCP server started: {} at {}", server.name, server_url);
					return Ok(server_url);
				}

				// Wait a bit before trying again
				sleep(Duration::from_millis(500)).await;
			}
		}
		McpServerMode::Stdin => {
			// For stdin mode, we need bidirectional communication
			cmd.stdin(Stdio::piped())
				.stdout(Stdio::piped())
				.stderr(Stdio::piped());

			// Start the process
			// Debug output
			crate::log_debug!("🚀 Starting MCP server (stdin mode): {}", server.name);
			let mut child = cmd.spawn().map_err(|e| {
				anyhow::anyhow!("Failed to start MCP server '{}': {}", server.name, e)
			})?;

			// Get the stdin/stdout handles
			let child_stdin = child.stdin.take().ok_or_else(|| {
				anyhow::anyhow!("Failed to open stdin for MCP server: {}", server.name)
			})?;

			let child_stdout = child.stdout.take().ok_or_else(|| {
				anyhow::anyhow!("Failed to open stdout for MCP server: {}", server.name)
			})?;

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

			// Add to the registry
			{
				let mut processes = SERVER_PROCESSES.write().unwrap();
				processes.insert(server.name.clone(), Arc::new(Mutex::new(server_process)));
			}

			// Clear function cache for this server since it's restarting
			crate::mcp::server::clear_function_cache_for_server(&server.name);

			// Initialize the server by sending the initialize request, following the MCP protocol
			// This also verifies the server is responsive
			let _process_arc = {
				let processes = SERVER_PROCESSES.read().unwrap();
				processes.get(&server.name).cloned().ok_or_else(|| {
					anyhow::anyhow!("Server not found right after creation: {}", server.name)
				})?
			};

			// Initialize the server following the MCP protocol
			let init_result = initialize_stdin_server(&server.name).await;

			if let Err(e) = &init_result {
				eprintln!(
					"Failed to initialize stdin MCP server '{}': {}",
					server.name, e
				);

				// Use the proper cleanup function to kill the process
				if let Err(cleanup_err) = cleanup_server_process(&server.name) {
					crate::log_debug!(
						"Failed to cleanup server '{}' after init failure: {}",
						server.name,
						cleanup_err
					);
				}

				return Err(anyhow::anyhow!(
					"Failed to initialize stdin MCP server '{}': {}",
					server.name,
					e
				));
			}

			// Return a pseudo-URL for stdin-based servers
			let stdin_url = format!("stdin://{}", server.name);
			// Debug output
			// println!("MCP server started and initialized (stdin mode): {} at {}", server.name, stdin_url);
			Ok(stdin_url)
		}
	}
}

// Initialize a stdin-based server following the MCP protocol
async fn initialize_stdin_server(server_name: &str) -> Result<()> {
	// Construct an initialize message according to the MCP protocol
	let init_message = json!({
		"jsonrpc": "2.0",
		"id": 1,  // Use ID 1 for initialization
		"method": "initialize",
		"params": {
			"clientInfo": {
				"name": "octomind",
				"version": env!("CARGO_PKG_VERSION")
			},
			"protocolVersion": "2025-03-26",  // Use latest protocol version
			"capabilities": {
				// Empty capabilities object is fine for client
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

	// Check if we got a valid result
	if response.get("result").is_none() {
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

	let _ = try_communicate_with_stdin_server(server_name, &initialized_message, 0).await;

	// If we reach here, initialization was successful
	Ok(())
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
	// If URL is explicitly specified, use that
	if let Some(url) = &server.url {
		return Ok(url.to_string()); // Convert &str to String without unnecessary clone
	}

	// For stdin-based servers, return a pseudo-URL
	if let McpServerMode::Stdin = server.mode {
		return Ok(format!("stdin://{}", server.name));
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
	cancellation_token: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
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
	cancellation_token: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> Result<Value> {
	// Early cancellation check
	if let Some(ref token) = cancellation_token {
		if token.load(Ordering::SeqCst) {
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

	// Get the request ID atomically and prepare the message
	let (final_message, request_id) = {
		let mut process_guard = server_process
			.lock()
			.map_err(|_| anyhow::anyhow!("Failed to acquire lock on server process"))?;

		match &mut *process_guard {
			ServerProcess::Stdin {
				next_id,
				is_shutdown,
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

				(final_msg, actual_id)
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

	// Execute with timeout and cancellation
	let timeout_future = tokio::time::timeout(
		std::time::Duration::from_secs(timeout_seconds),
		tokio::task::spawn_blocking(move || {
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
									let mut restart_info_guard =
										SERVER_RESTART_INFO.write().unwrap();
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
									let mut restart_info_guard =
										SERVER_RESTART_INFO.write().unwrap();
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

					// Read the response from stdout
					let mut response_str = String::new();
					let read_result = reader
						.read_line(&mut response_str)
						.map_err(|e| anyhow::anyhow!("Failed to read from stdout: {}", e))?;

					if read_result == 0 {
						return Err(anyhow::anyhow!(
							"Server closed connection while reading response"
						));
					}

					// Parse the response JSON
					let response: Value = serde_json::from_str(&response_str).map_err(|e| {
						anyhow::anyhow!(
							"Failed to parse JSON response: {} (raw: {})",
							e,
							response_str
						)
					})?;

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
		}),
	);

	// Check for cancellation during the operation with faster polling
	let cancellation_future = async {
		if let Some(ref token) = cancellation_token {
			loop {
				tokio::time::sleep(Duration::from_millis(10)).await; // Much faster polling
				if token.load(Ordering::SeqCst) {
					break;
				}
			}
		} else {
			std::future::pending::<()>().await;
		}
	};

	// Race between operation, timeout, and cancellation
	tokio::select! {
		result = timeout_future => {
			match result {
				Ok(task_result) => task_result?,
				Err(_) => Err(anyhow::anyhow!("Timeout ({} seconds) communicating with stdin server: {}", timeout_seconds, server_name_for_error))
			}
		},
		_ = cancellation_future => {
			Err(anyhow::anyhow!("Operation cancelled while communicating with server: {}", server_name_for_error))
		}
	}
}

// Get tool definitions from a stdin-based server
pub async fn get_stdin_server_functions(server: &McpServerConfig) -> Result<Vec<McpFunction>> {
	// Create a list_tools request message following the MCP protocol
	let message = json!({
		"jsonrpc": "2.0",
		"id": 1,
		"method": "tools/list", // Correct MCP method name
		"params": {}
	});

	// Try to get tool information from the server with a timeout
	// Pass the same ID that's in the message (1) and no cancellation token for initialization
	let response = communicate_with_stdin_server(&server.name, &message, 1, None).await?;

	// Extract functions from the response
	let mut functions = Vec::new();

	// Debug output
	// println!("Tools/list response: {}", response);

	// Check for errors in the response
	if let Some(error) = response.get("error") {
		eprintln!(
			"Warning: Server returned error during tools/list: {}",
			error
		);
		return Ok(functions); // Return empty list on error
	}

	// Extract the tools list from the result
	if let Some(result) = response.get("result") {
		if let Some(tools) = result.get("tools").and_then(|t| t.as_array()) {
			for tool in tools {
				if let (Some(name), Some(description)) = (
					tool.get("name").and_then(|n| n.as_str()),
					tool.get("description").and_then(|d| d.as_str()),
				) {
					// Check if this tool is enabled
					if server.tools.is_empty() || server.tools.contains(&name.to_string()) {
						// Get parameters from inputSchema if available, otherwise use empty object
						let parameters = tool.get("inputSchema").cloned().unwrap_or(json!({}));

						// Debug output
						// println!("Tool details for {}: {}", name, tool);

						functions.push(McpFunction {
							name: name.to_string(),
							description: description.to_string(),
							parameters,
						});
					}
				}
			}
		}
	} else {
		println!("Invalid response format from tools/list: {}", response);
	}

	Ok(functions)
}

// Execute a tool on a stdin-based server
pub async fn execute_stdin_tool_call(
	call: &McpToolCall,
	server: &McpServerConfig,
	cancellation_token: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
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
		&server.name,
		&message,
		1,
		server.timeout_seconds,
		cancellation_token,
	)
	.await
	{
		Ok(resp) => resp,
		Err(e) => {
			eprintln!("Error executing tool call '{}': {}", call.tool_name, e);
			// Return a formatted error as the tool result rather than failing
			return Ok(McpToolResult {
				tool_name: call.tool_name.clone(),
				tool_id: call.tool_id.clone(),
				result: json!({
					"output": {
						"error": true,
						"success": false,
						"message": format!("Error executing tool: {}", e)
					},
					"parameters": call.parameters
				}),
			});
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

		let output = json!({
			"error": true,
			"success": false,
			"message": error_message,
			"code": error_code
		});

		return Ok(McpToolResult {
			tool_name: call.tool_name.clone(),
			tool_id: call.tool_id.clone(),
			result: json!({
				"output": output,
				"parameters": call.parameters
			}),
		});
	}

	// Extract the result
	let output = response
		.get("result")
		.cloned()
		.unwrap_or(json!("No result"));

	// Create tool result
	let tool_result = McpToolResult {
		tool_name: call.tool_name.clone(),
		tool_id: call.tool_id.clone(),
		result: json!({
			"output": output,
			"parameters": call.parameters
		}),
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
					eprintln!("Failed to kill MCP server '{}': {}", name, e);
				}
			}
			Err(_) => {
				crate::log_debug!("Could not acquire lock for server '{}', may be busy", name);
				// For busy processes, we'll just remove them from registry
				// The process cleanup will happen when the lock is released
			}
		}
	}

	processes.clear();

	// Clear all function cache when stopping all servers
	crate::mcp::server::clear_all_function_cache();

	// Clear all restart mutexes
	{
		let mut mutexes = SERVER_RESTART_MUTEXES.write().unwrap();
		mutexes.clear();
		crate::log_debug!("Cleared all server restart mutexes");
	}

	Ok(())
}

// Cleanup a specific server process (helper function)
pub fn cleanup_server_process(server_name: &str) -> Result<()> {
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
					"Could not acquire lock for server '{}' during cleanup",
					server_name
				);
			}
		}

		// Clear function cache for this server
		crate::mcp::server::clear_function_cache_for_server(server_name);

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

// Check if a server process is still running with enhanced health tracking
pub fn is_server_running(server_name: &str) -> bool {
	let processes = SERVER_PROCESSES.read().unwrap();
	if let Some(process_arc) = processes.get(server_name) {
		let mut process = process_arc.lock().unwrap();
		let is_alive = process
			.try_wait()
			.map(|status| status.is_none())
			.unwrap_or(false);

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
		// Update health status - server not in registry
		{
			let mut restart_info_guard = SERVER_RESTART_INFO.write().unwrap();
			let info = restart_info_guard
				.entry(server_name.to_string())
				.or_default();
			info.health_status = ServerHealth::Dead;
			info.last_health_check = Some(SystemTime::now());
		}
		false
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

// Try to communicate with a stdin-based server, ignoring errors
async fn try_communicate_with_stdin_server(
	server_name: &str,
	message: &Value,
	override_id: u64,
) -> Result<()> {
	if let Err(e) = communicate_with_stdin_server(server_name, message, override_id, None).await {
		eprintln!("Warning: Error sending notification to MCP server: {}", e);
	}
	Ok(())
}
