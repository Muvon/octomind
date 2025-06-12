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

// External MCP server provider

use super::process;
use super::{McpFunction, McpToolCall, McpToolResult};
use crate::config::{Config, McpServerConfig, McpServerMode};
use anyhow::Result;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

// Global cache for server function definitions to avoid repeated JSON-RPC calls
// Functions are cached until server restarts (no TTL needed)
lazy_static::lazy_static! {
	static ref FUNCTION_CACHE: Arc<RwLock<HashMap<String, Vec<McpFunction>>>> =
		Arc::new(RwLock::new(HashMap::new()));
}

// Get server function definitions (will start server if needed)
pub async fn get_server_functions(server: &McpServerConfig) -> Result<Vec<McpFunction>> {
	// Note: enabled check is now handled at the role level via server_refs
	// All servers in the registry are considered available

	// Handle different server modes
	match server.mode {
		McpServerMode::Http => {
			// Handle local vs remote servers
			let server_url = get_server_base_url(server).await?;

			// Create a client
			let client = Client::new();

			// Prepare headers
			let mut headers = HeaderMap::new();
			headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

			// Add auth token if present
			if let Some(token) = &server.auth_token {
				headers.insert(
					AUTHORIZATION,
					HeaderValue::from_str(&format!("Bearer {}", token))?,
				);
			}

			// Get schema URL - should be schema endpoint
			let schema_url = format!("{}/tools/list", server_url); // Correct endpoint

			// Make request to get schema
			let response = client.get(&schema_url).headers(headers).send().await?;

			// Debug output
			// println!("Schema response from HTTP server: {}", response.status());

			// Check if request was successful
			if !response.status().is_success() {
				return Err(anyhow::anyhow!(
					"Failed to get schema from MCP server: {}",
					response.status()
				));
			}

			// Parse response
			let schema: Value = response.json().await?;

			// Debug output
			// println!("Schema response body: {}", schema);

			// Extract functions
			let mut functions = Vec::new();

			// Extract tools from result.tools if available
			if let Some(result) = schema.get("result") {
				if let Some(tools) = result.get("tools").and_then(|t| t.as_array()) {
					for func in tools {
						if let (Some(name), Some(description)) = (
							func.get("name").and_then(|n| n.as_str()),
							func.get("description").and_then(|d| d.as_str()),
						) {
							// Check if this tool is enabled
							if server.tools.is_empty() || server.tools.contains(&name.to_string()) {
								// Get the parameters from the inputSchema field if available
								let parameters =
									func.get("inputSchema").cloned().unwrap_or(json!({}));

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
				// Legacy support for functions directly in schema
				if let Some(schema_functions) = schema.get("functions").and_then(|f| f.as_array()) {
					for func in schema_functions {
						if let (Some(name), Some(description)) = (
							func.get("name").and_then(|n| n.as_str()),
							func.get("description").and_then(|d| d.as_str()),
						) {
							// Check if this tool is enabled
							if server.tools.is_empty() || server.tools.contains(&name.to_string()) {
								// Get the parameters from the inputSchema field if available
								let parameters =
									func.get("inputSchema").cloned().unwrap_or(json!({}));

								functions.push(McpFunction {
									name: name.to_string(),
									description: description.to_string(),
									parameters,
								});
							}
						}
					}
				}
			}

			Ok(functions)
		}
		McpServerMode::Stdin => {
			// For stdin-based servers, ensure the server is running and get functions
			process::ensure_server_running(server).await?;
			process::get_stdin_server_functions(server).await
		}
	}
}

// Get server function definitions WITHOUT making JSON-RPC calls (optimized for system prompt generation)
pub async fn get_server_functions_cached(server: &McpServerConfig) -> Result<Vec<McpFunction>> {
	let server_id = &server.name;

	// First, check if we have cached functions
	{
		let cache = FUNCTION_CACHE.read().unwrap();
		if let Some(cached_functions) = cache.get(server_id) {
			return Ok(cached_functions.clone());
		}
	}

	// Check if server is currently running
	let is_running = is_server_running_for_cache_check(server_id);

	if is_running {
		// Server is running - get fresh functions and cache them
		crate::log_debug!(
			"Server '{}' is running - fetching and caching function definitions",
			server_id
		);

		match get_server_functions(server).await {
			Ok(functions) => {
				// Cache the functions (no expiration - only cleared on server restart)
				{
					let mut cache = FUNCTION_CACHE.write().unwrap();
					cache.insert(server_id.clone(), functions.clone());
				}
				crate::log_debug!(
					"Cached {} functions for server '{}'",
					functions.len(),
					server_id
				);
				Ok(functions)
			}
			Err(e) => {
				crate::log_error!(
					"Failed to get functions from running server '{}': {}",
					server_id,
					e
				);
				// Fall back to configured tools
				get_fallback_functions(server)
			}
		}
	} else {
		// Server is not running - return configured tools or empty list
		crate::log_debug!(
			"Server '{}' is not running - using fallback function definitions",
			server_id
		);
		get_fallback_functions(server)
	}
}

// Helper function to get fallback functions when server is not running
fn get_fallback_functions(server: &McpServerConfig) -> Result<Vec<McpFunction>> {
	if !server.tools.is_empty() {
		// Return lightweight function entries based on configuration
		Ok(server
			.tools
			.iter()
			.map(|tool_name| McpFunction {
				name: tool_name.clone(),
				description: format!(
					"External tool '{}' from server '{}' (server not started)",
					tool_name, server.name
				),
				parameters: serde_json::json!({}),
			})
			.collect())
	} else {
		// No specific tools configured and server not running
		Ok(vec![])
	}
}

// Optimized server running check that doesn't hold locks for long
fn is_server_running_for_cache_check(server_name: &str) -> bool {
	let processes = process::SERVER_PROCESSES.read().unwrap();
	if let Some(process_arc) = processes.get(server_name) {
		// Try to get a quick lock - if we can't, assume it's busy and running
		if let Ok(mut process) = process_arc.try_lock() {
			match &mut *process {
				process::ServerProcess::Http(child) => child
					.try_wait()
					.map(|status| status.is_none())
					.unwrap_or(false),
				process::ServerProcess::Stdin {
					child, is_shutdown, ..
				} => {
					let process_alive = child
						.try_wait()
						.map(|status| status.is_none())
						.unwrap_or(false);
					let not_marked_shutdown =
						!is_shutdown.load(std::sync::atomic::Ordering::SeqCst);
					process_alive && not_marked_shutdown
				}
			}
		} else {
			// If we can't get the lock, assume the server is busy and running
			true
		}
	} else {
		false
	}
}

// Clear cached functions for a specific server (called when server restarts)
pub fn clear_function_cache_for_server(server_name: &str) {
	let mut cache = FUNCTION_CACHE.write().unwrap();
	if cache.remove(server_name).is_some() {
		crate::log_debug!(
			"Cleared function cache for server '{}' due to restart",
			server_name
		);
	}
}

// Clear all cached functions (useful for cleanup)
pub fn clear_all_function_cache() {
	let mut cache = FUNCTION_CACHE.write().unwrap();
	let count = cache.len();
	cache.clear();
	if count > 0 {
		crate::log_debug!("Cleared function cache for {} servers", count);
	}
}

// Check if a server is already running with enhanced health checking
// Takes server config to properly handle internal vs external servers
pub fn is_server_already_running_with_config(server: &crate::config::McpServerConfig) -> bool {
	match server.server_type {
		crate::config::McpServerType::Developer
		| crate::config::McpServerType::Filesystem
		| crate::config::McpServerType::Agent => {
			// Internal servers are always considered running since they're built-in
			{
				let mut restart_info_guard = process::SERVER_RESTART_INFO.write().unwrap();
				let info = restart_info_guard.entry(server.name.clone()).or_default();
				info.health_status = process::ServerHealth::Running;
				info.last_health_check = Some(std::time::SystemTime::now());
			}
			true
		}
		crate::config::McpServerType::External => {
			// External servers - check the process registry
			let is_process_running = {
				let processes = process::SERVER_PROCESSES.read().unwrap();
				if let Some(process_arc) = processes.get(&server.name) {
					let mut process = process_arc.lock().unwrap();
					match &mut *process {
						process::ServerProcess::Http(child) => child
							.try_wait()
							.map(|status| status.is_none())
							.unwrap_or(false),
						process::ServerProcess::Stdin {
							child, is_shutdown, ..
						} => {
							let process_alive = child
								.try_wait()
								.map(|status| status.is_none())
								.unwrap_or(false);
							let not_marked_shutdown =
								!is_shutdown.load(std::sync::atomic::Ordering::SeqCst);
							process_alive && not_marked_shutdown
						}
					}
				} else {
					false
				}
			};

			// Update health status based on actual process state
			let health_status = if is_process_running {
				process::ServerHealth::Running
			} else {
				process::ServerHealth::Dead
			};

			// Update restart tracking
			{
				let mut restart_info_guard = process::SERVER_RESTART_INFO.write().unwrap();
				let info = restart_info_guard.entry(server.name.clone()).or_default();
				info.health_status = health_status;
				info.last_health_check = Some(std::time::SystemTime::now());
			}

			is_process_running
		}
	}
}

// Legacy function for backward compatibility - tries to guess server type
pub fn is_server_already_running(server_name: &str) -> bool {
	// For internal servers, we need to determine their type first
	// Internal servers (Developer/Filesystem) are always "running" since they're built-in

	// Check if this is an internal server by looking for it in a typical config
	// This is a bit of a hack, but we need to distinguish internal vs external servers
	if server_name == "developer" || server_name == "filesystem" {
		// Internal servers are always considered running
		let mut restart_info_guard = process::SERVER_RESTART_INFO.write().unwrap();
		let info = restart_info_guard
			.entry(server_name.to_string())
			.or_default();
		info.health_status = process::ServerHealth::Running;
		info.last_health_check = Some(std::time::SystemTime::now());
		return true;
	}

	// For external servers, check the process registry
	let is_process_running = {
		let processes = process::SERVER_PROCESSES.read().unwrap();
		if let Some(process_arc) = processes.get(server_name) {
			let mut process = process_arc.lock().unwrap();
			match &mut *process {
				process::ServerProcess::Http(child) => child
					.try_wait()
					.map(|status| status.is_none())
					.unwrap_or(false),
				process::ServerProcess::Stdin {
					child, is_shutdown, ..
				} => {
					let process_alive = child
						.try_wait()
						.map(|status| status.is_none())
						.unwrap_or(false);
					let not_marked_shutdown =
						!is_shutdown.load(std::sync::atomic::Ordering::SeqCst);
					process_alive && not_marked_shutdown
				}
			}
		} else {
			false
		}
	};

	// Update health status based on actual process state
	let health_status = if is_process_running {
		process::ServerHealth::Running
	} else {
		process::ServerHealth::Dead
	};

	// Update restart tracking
	{
		let mut restart_info_guard = process::SERVER_RESTART_INFO.write().unwrap();
		let info = restart_info_guard
			.entry(server_name.to_string())
			.or_default();
		info.health_status = health_status;
		info.last_health_check = Some(std::time::SystemTime::now());
	}

	is_process_running
}

// Execute tool call on MCP server (either local or remote)
pub async fn execute_tool_call(
	call: &McpToolCall,
	server: &McpServerConfig,
	cancellation_token: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> Result<McpToolResult> {
	use std::sync::atomic::Ordering;

	// Check for cancellation before starting
	if let Some(ref token) = cancellation_token {
		if token.load(Ordering::SeqCst) {
			return Err(anyhow::anyhow!("External tool execution cancelled"));
		}
	}

	// Check server health before attempting execution (but don't restart)
	let server_health = process::get_server_health(&server.name);
	match server_health {
		process::ServerHealth::Failed => {
			return Err(anyhow::anyhow!(
				"Server '{}' is in failed state. Cannot execute tool '{}'. Server will not be restarted automatically.",
				server.name,
				call.tool_name
			));
		}
		process::ServerHealth::Restarting => {
			return Err(anyhow::anyhow!(
				"Server '{}' is currently starting. Please try again in a moment.",
				server.name
			));
		}
		process::ServerHealth::Dead => {
			return Err(anyhow::anyhow!(
				"Server '{}' is not running. Cannot execute tool '{}'. Server will not be restarted automatically.",
				server.name,
				call.tool_name
			));
		}
		process::ServerHealth::Running => {
			// Server is running, proceed with execution
		}
	}

	// Execute the tool call directly (no restart logic)
	execute_tool_call_internal(call, server, cancellation_token).await
}

// Internal function to execute tool call without restart logic
async fn execute_tool_call_internal(
	call: &McpToolCall,
	server: &McpServerConfig,
	cancellation_token: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> Result<McpToolResult> {
	use std::sync::atomic::Ordering;

	// Check for cancellation before starting
	if let Some(ref token) = cancellation_token {
		if token.load(Ordering::SeqCst) {
			return Err(anyhow::anyhow!("External tool execution cancelled"));
		}
	}

	// Note: enabled check is now handled at the role level via server_refs
	// All servers in the registry are considered available for execution

	// Extract tool name and parameters
	let tool_name = &call.tool_name;
	let parameters = &call.parameters;

	// Tool execution display is now handled in response.rs to avoid duplication

	// Handle different server modes
	match server.mode {
		McpServerMode::Http => {
			// Check for cancellation before HTTP request
			if let Some(ref token) = cancellation_token {
				if token.load(Ordering::SeqCst) {
					return Err(anyhow::anyhow!("External tool execution cancelled"));
				}
			}

			// Handle local vs remote servers for HTTP mode
			let server_url = get_server_base_url(server).await?;

			// Create a client with configured timeout
			let client = Client::builder()
				.timeout(std::time::Duration::from_secs(server.timeout_seconds))
				.build()
				.unwrap_or_else(|_| Client::new());

			// Prepare headers
			let mut headers = HeaderMap::new();
			headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

			// Add auth token if present
			if let Some(token) = &server.auth_token {
				headers.insert(
					AUTHORIZATION,
					HeaderValue::from_str(&format!("Bearer {}", token))?,
				);
			}

			// Get execution URL
			let execute_url = format!("{}/tools/call", server_url);

			// Prepare request body
			let request_body = json!({
				"name": tool_name,
				"arguments": parameters
			});

			// Check for cancellation one more time before sending request
			if let Some(ref token) = cancellation_token {
				if token.load(Ordering::SeqCst) {
					return Err(anyhow::anyhow!("External tool execution cancelled"));
				}
			}

			// Make request to execute tool
			let response = client
				.post(&execute_url)
				.headers(headers)
				.json(&request_body)
				.send()
				.await?;

			// Check if request was successful
			if !response.status().is_success() {
				// Save the status before consuming the response with text()
				let status = response.status();
				let error_text = response.text().await?;
				return Err(anyhow::anyhow!(
					"Failed to execute tool on MCP server: {}, {}",
					status,
					error_text
				));
			}

			// Parse response
			let result: Value = response.json().await?;

			// Extract result or error from the response
			let output = if let Some(_error) = result.get("error") {
				json!({
					"error": true,
					"success": false,
					"message": result.get("message").and_then(|m| m.as_str()).unwrap_or("Server error")
				})
			} else {
				result.get("result").cloned().unwrap_or(json!("No result"))
			};

			// Create MCP-compliant tool result
			let tool_result = McpToolResult::success(
				tool_name.clone(),
				call.tool_id.clone(),
				serde_json::to_string_pretty(&output).unwrap_or_else(|_| output.to_string()),
			);

			Ok(tool_result)
		}
		McpServerMode::Stdin => {
			// For stdin-based servers, use the stdin communication channel with cancellation support
			process::execute_stdin_tool_call(call, server, cancellation_token).await
		}
	}
}

// Get the base URL for a server, starting it if necessary for local servers
async fn get_server_base_url(server: &McpServerConfig) -> Result<String> {
	match server.mode {
		McpServerMode::Http => {
			// Check if this is a local server that needs to be started
			if server.command.is_some() {
				// This is a local server, ensure it's running
				process::ensure_server_running(server).await
			} else if let Some(url) = &server.url {
				// This is a remote server with a URL
				Ok(url.trim_end_matches("/").to_string())
			} else {
				// Neither remote nor local configuration
				Err(anyhow::anyhow!("Invalid server configuration: neither URL nor command specified for server '{}'", server.name))
			}
		}
		McpServerMode::Stdin => {
			// For stdin-based servers, return a pseudo-URL
			if server.command.is_some() {
				// Ensure the stdin server is running
				process::ensure_server_running(server).await
			} else {
				Err(anyhow::anyhow!("Invalid server configuration: command not specified for stdin-based server '{}'", server.name))
			}
		}
	}
}

// Get all available functions from all configured servers
pub async fn get_all_server_functions(
	config: &Config,
) -> Result<HashMap<String, (McpFunction, McpServerConfig)>> {
	let mut functions = HashMap::new();

	// Only proceed if MCP has any servers configured
	if config.mcp.servers.is_empty() {
		return Ok(functions);
	}

	// Get available servers from merged config (which should already be filtered by server_refs)
	let servers: Vec<crate::config::McpServerConfig> = config.mcp.servers.to_vec();

	// Check each server
	for server in &servers {
		let server_functions = get_server_functions(server).await?;

		for func in server_functions {
			functions.insert(func.name.clone(), (func, server.clone()));
		}
	}

	Ok(functions)
}

// Clean up any running server processes when the program exits
pub fn cleanup_servers() -> Result<()> {
	// Stop the health monitor first
	crate::mcp::health_monitor::stop_health_monitor();

	// Then stop all server processes
	process::stop_all_servers()
}

// Get server health status for monitoring
pub fn get_server_health_status(server_name: &str) -> process::ServerHealth {
	process::get_server_health(server_name)
}

// Get detailed server restart information
pub fn get_server_restart_info(server_name: &str) -> process::ServerRestartInfo {
	process::get_server_restart_info(server_name)
}

// Reset server failure state (useful for manual recovery)
pub fn reset_server_failure_state(server_name: &str) -> Result<()> {
	process::reset_server_failure_state(server_name)
}

// Perform health check on all servers
pub async fn perform_health_check_all_servers(
) -> std::collections::HashMap<String, process::ServerHealth> {
	process::perform_health_check_all_servers().await
}

// Get comprehensive server status report
pub fn get_server_status_report(
) -> std::collections::HashMap<String, (process::ServerHealth, process::ServerRestartInfo)> {
	process::get_server_status_report()
}
