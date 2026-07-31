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

// External MCP server provider.
//
// Both stdio and Streamable HTTP servers are reached through the rmcp client
// in `super::client` (MCP 2026-07-28 with automatic legacy fallback). This
// module adds function caching, health gating, and result wrapping on top.

use super::process;
use super::{McpFunction, McpToolCall, McpToolResult};
use crate::config::{Config, McpConnectionType, McpServerConfig};
use crate::mcp::oauth::token_store;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

// Global cache for server function definitions to avoid repeated tools/list calls
// Functions are cached until server restarts (no TTL needed)
lazy_static::lazy_static! {
	static ref FUNCTION_CACHE: Arc<RwLock<HashMap<String, Vec<McpFunction>>>> =
		Arc::new(RwLock::new(HashMap::new()));
}

/// Map rmcp tools to octomind function definitions.
///
/// Returns the server's FULL tool inventory — no role/capability filtering
/// applied here. The cached result is reused across role merges and runtime
/// capability activations, so filtering at parse time would permanently
/// hide tools that the role's static `tools = [...]` excludes, defeating
/// `capability enable <cap>` whose runtime overlay extends that filter
/// (see `server_functions_for` in `src/mcp/mod.rs` for the union with
/// `runtime_overlay::extras_for_server`).
pub fn tools_to_functions(tools: &[rmcp::model::Tool]) -> Vec<McpFunction> {
	tools
		.iter()
		.map(|tool| McpFunction {
			name: tool.name.as_ref().to_string(),
			description: tool.description.as_deref().unwrap_or("").to_string(),
			parameters: tool.schema_as_json_value(),
		})
		.collect()
}

// Get server function definitions (will start/connect the server if needed)
pub async fn get_server_functions(server: &McpServerConfig) -> Result<Vec<McpFunction>> {
	// Note: enabled check is now handled at the role level via server_refs
	match server.connection_type() {
		McpConnectionType::Http | McpConnectionType::Stdin => {
			let tools = super::client::list_tools(server).await?;
			Ok(tools_to_functions(&tools))
		}
		McpConnectionType::Builtin => Err(anyhow::anyhow!(
			"Built-in servers should not use get_server_functions"
		)),
	}
}

// Get server function definitions WITHOUT connecting when the server isn't
// available (optimized for system prompt generation)
pub async fn get_server_functions_cached(server: &McpServerConfig) -> Result<Vec<McpFunction>> {
	let server_id = server.name();

	// First, check if we have cached functions
	{
		let cache = FUNCTION_CACHE.read().unwrap();
		if let Some(cached_functions) = cache.get(server_id) {
			return Ok(cached_functions.clone());
		}
	}

	// For HTTP servers with a URL, always try to get tools - they're endpoints we can reach
	// For stdin servers, check if the connection is alive first
	let should_fetch = match server.connection_type() {
		McpConnectionType::Http => server.url().is_some(), // Always try for HTTP servers
		McpConnectionType::Stdin => super::client::is_connected(server_id),
		McpConnectionType::Builtin => false, // Builtin servers handled separately
	};

	if should_fetch {
		// Check if we have a cached OAuth token before attempting fetch
		// This prevents triggering OAuth flow during tool map initialization
		// Only check for servers that have been discovered to require OAuth
		if crate::mcp::oauth::discovery::has_cached_discovery(server_id) {
			match token_store::get_valid_token(server_id, 300).await {
				Ok(None) => {
					// No valid token - don't trigger OAuth, return empty
					crate::log_debug!(
						"Server '{}' requires OAuth but no token available - skipping cache fetch",
						server_id
					);
					return Ok(Vec::new());
				}
				Err(e) => {
					crate::log_debug!(
						"Failed to check OAuth token for server '{}': {} - skipping cache fetch",
						server_id,
						e
					);
					return Ok(Vec::new());
				}
				Ok(Some(_)) => {
					// Token exists, proceed with fetch
				}
			}
		}

		// Server should be available - get fresh functions and cache them
		crate::log_debug!("Fetching function definitions from server '{}'", server_id);

		match get_server_functions(server).await {
			Ok(functions) => {
				// Cache the functions (no expiration - only cleared on server restart)
				{
					let mut cache = FUNCTION_CACHE.write().unwrap();
					cache.insert(server_id.to_string(), functions.clone());
				}
				crate::log_debug!("Server '{}' returned {} tools", server_id, functions.len());
				Ok(functions)
			}
			Err(e) => {
				// Server failed - log error and return empty
				crate::log_error!(
					"Failed to connect to MCP server '{}': {}. Verify the server is running at the configured URL.",
					server_id,
					e
				);
				// Do NOT cache the empty result: the cache is never invalidated, so
				// a single transient fetch failure would make the server appear to
				// have zero tools forever. Returning empty without caching lets the
				// next tools/list refresh retry once the server recovers.
				Ok(Vec::new())
			}
		}
	} else {
		// Server is not running (stdin server without connection). We only know a
		// server's real tool names after it starts and answers tools/list.
		// `server.tools()` holds the role's allowed_tools *filter patterns*
		// (e.g. `*`, `text_*`), not real tool names — fabricating functions
		// from them leaks invalid names (failing `^[a-zA-Z0-9_-]{1,128}$`) to
		// the provider. If the server didn't start, expose none of its tools.
		crate::log_debug!(
			"Server '{}' is not running - exposing no tools (skipping)",
			server_id
		);
		Ok(Vec::new())
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
	match server.connection_type() {
		McpConnectionType::Builtin => {
			// Internal servers are always considered running since they're built-in
			{
				let mut restart_info_guard = process::SERVER_RESTART_INFO.write().unwrap();
				let info = restart_info_guard
					.entry(server.name().to_string())
					.or_default();
				info.health_status = process::ServerHealth::Running;
				info.last_health_check = Some(std::time::SystemTime::now());
			}
			true
		}
		McpConnectionType::Http | McpConnectionType::Stdin => {
			// External servers — client connection liveness (updates health tracking).
			process::is_server_running(server.name())
		}
	}
}

// Execute tool call on MCP server (either local or remote)
pub async fn execute_tool_call(
	call: &McpToolCall,
	server: &McpServerConfig,
	cancellation_token: Option<tokio::sync::watch::Receiver<bool>>,
) -> Result<McpToolResult> {
	// Check for cancellation before starting
	if let Some(ref token) = cancellation_token {
		if *token.borrow() {
			return Err(anyhow::anyhow!("External tool execution cancelled"));
		}
	}
	// Check server health before attempting execution. Dead stdin servers are restarted here.
	// Refresh OS-level liveness first: a killed child may leave the cached health as Running.
	if server.connection_type() == McpConnectionType::Stdin {
		process::is_server_running(server.name());
	}
	let server_health = process::get_server_health(server.name());
	match server_health {
		process::ServerHealth::Failed => {
			return Err(anyhow::anyhow!(
				"Server '{}' is in failed state. Cannot execute tool '{}'. Server will not be restarted automatically.",
				server.name(),
				call.tool_name
			));
		}
		process::ServerHealth::Restarting => {
			return Err(anyhow::anyhow!(
				"Server '{}' is currently starting. Please try again in a moment.",
				server.name()
			));
		}
		process::ServerHealth::Dead => {
			// For HTTP servers, "Dead" might just mean health check failed —
			// execution reconnects with a fresh OAuth token on demand.
			if server.connection_type() == McpConnectionType::Http {
				crate::log_debug!(
					"HTTP server '{}' health check failed, but allowing tool execution to proceed with fresh connection",
					server.name()
				);
			} else {
				// For stdin servers, Dead means the process was killed (e.g. after Ctrl+C).
				// Restart it now so the next tool call succeeds — same auto-recovery HTTP uses.
				crate::log_info!(
					"Server '{}' is dead — restarting before executing tool '{}'",
					server.name(),
					call.tool_name
				);
				if let Err(e) = process::ensure_server_running(server).await {
					return Err(anyhow::anyhow!(
						"Server '{}' failed to restart: {}",
						server.name(),
						e
					));
				}
			}
		}
		process::ServerHealth::Unreachable => {
			// For HTTP servers with OAuth, "Unreachable" often means auth failed in health check
			// But tool execution reconnects with its own OAuth token that might succeed
			crate::log_debug!(
				"Server '{}' marked as unreachable (likely auth issue in health check), but allowing tool execution to proceed",
				server.name()
			);
		}
		process::ServerHealth::Running => {
			// Server is running, proceed with execution
		}
	}

	execute_tool_with_cancellation(call, server, cancellation_token).await
}

// Execute tool call with cancellation support
async fn execute_tool_with_cancellation(
	call: &McpToolCall,
	server: &McpServerConfig,
	cancellation_token: Option<tokio::sync::watch::Receiver<bool>>,
) -> Result<McpToolResult> {
	// Check for cancellation before starting
	if let Some(ref token) = cancellation_token {
		if *token.borrow() {
			return Err(anyhow::anyhow!("External tool execution cancelled"));
		}
	}

	match server.connection_type() {
		McpConnectionType::Http | McpConnectionType::Stdin => {
			match super::client::call_tool(server, call, cancellation_token).await {
				Ok(result) => Ok(McpToolResult {
					tool_name: call.tool_name.clone(),
					tool_id: call.tool_id.clone(),
					result,
				}),
				Err(e) => {
					crate::log_error!("Error executing tool call '{}': {}", call.tool_name, e);
					// Return a formatted error as the tool result rather than failing
					Ok(McpToolResult::error(
						call.tool_name.clone(),
						call.tool_id.clone(),
						format!("Error executing tool: {}", e),
					))
				}
			}
		}
		McpConnectionType::Builtin => {
			// Built-in servers should not use this function
			Err(anyhow::anyhow!(
				"Built-in servers should not use execute_tool_call"
			))
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
