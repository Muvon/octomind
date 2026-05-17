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

// MCP Protocol Implementation

use crate::config::McpConnectionType;
use crate::log_debug;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::{Arc, RwLock};

/// Progress events for MCP server initialization
#[derive(Debug, Clone)]
pub enum McpInitProgress {
	/// Starting initialization for these servers
	Starting { servers: Vec<String> },
	/// A server finished initializing
	Completed {
		server: String,
		success: bool,
		function_count: usize,
	},
}
// Modules
pub mod hint_accumulator;
pub mod tool_map;
pub mod utils;
pub mod workdir;

pub use utils::{
	ensure_tool_call_ids, guess_tool_category, parse_tool_calls, tool_results_to_messages,
	ToolResponseMessage,
};
pub use workdir::{
	get_thread_original_working_directory, get_thread_working_directory,
	set_session_working_directory, set_thread_working_directory,
};

// Cache for internal server function definitions (static during session)
lazy_static::lazy_static! {
	static ref INTERNAL_FUNCTION_CACHE: Arc<RwLock<std::collections::HashMap<String, Vec<McpFunction>>>> =
		Arc::new(RwLock::new(std::collections::HashMap::new()));
}

// OAuth 2.1 + PKCE authentication
pub mod oauth;

pub mod agent;
pub mod core;
pub mod health_monitor;
pub mod process;
pub mod runtime;
pub mod server;
pub mod shared_utils;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolCall {
	pub tool_name: String,
	pub parameters: Value,
	#[serde(default)]
	pub tool_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolResult {
	pub tool_name: String,
	pub result: rmcp::model::CallToolResult,
	#[serde(default)]
	pub tool_id: String,
}

// MCP Protocol-compliant result creation helpers
impl McpToolResult {
	// Create a successful MCP result with text content
	pub fn success(tool_name: String, tool_id: String, content: String) -> Self {
		Self {
			tool_name,
			tool_id,
			result: rmcp::model::CallToolResult::success(vec![rmcp::model::Content::text(content)]),
		}
	}

	// Create a successful MCP result with rich content (includes metadata)
	pub fn success_with_metadata(
		tool_name: String,
		tool_id: String,
		content: String,
		metadata: serde_json::Value,
	) -> Self {
		Self {
			tool_name,
			tool_id,
			result: {
				let mut r =
					rmcp::model::CallToolResult::success(vec![rmcp::model::Content::text(content)]);
				r.structured_content = Some(metadata);
				r
			},
		}
	}

	// Create an error MCP result
	pub fn error(tool_name: String, tool_id: String, error_message: String) -> Self {
		Self {
			tool_name,
			tool_id,
			result: rmcp::model::CallToolResult::error(vec![rmcp::model::Content::text(
				error_message,
			)]),
		}
	}

	// Check if this result represents an error based on MCP protocol
	pub fn is_error(&self) -> bool {
		self.result.is_error.unwrap_or(false)
	}

	// Extract plain text content from all text items in the result
	pub fn extract_content(&self) -> String {
		use rmcp::model::RawContent;
		let main_content = self
			.result
			.content
			.iter()
			.filter_map(|item| match &item.raw {
				RawContent::Text(t) => Some(t.text.as_str()),
				_ => None,
			})
			.collect::<Vec<_>>()
			.join("\n");

		// Include structured_content (metadata) if present
		if let Some(metadata) = &self.result.structured_content {
			if !metadata.is_null() {
				return format!(
					"{}\n\n[Metadata: {}]",
					main_content,
					serde_json::to_string_pretty(metadata).unwrap_or_default()
				);
			}
		}

		main_content
	}
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpFunction {
	pub name: String,
	pub description: String,
	pub parameters: Value,
}

// Initialize all servers for a specific mode/role ONCE at startup
pub async fn initialize_servers_for_role(config: &crate::config::Config) -> Result<()> {
	initialize_servers_for_role_with_callback(config, None).await
}

// Initialize servers with optional progress callback for UI updates
pub async fn initialize_servers_for_role_with_callback(
	config: &crate::config::Config,
	progress_callback: Option<&(dyn Fn(McpInitProgress) + Send + Sync)>,
) -> Result<()> {
	// Kick off embedding model warmup in the background so the first
	// `capability discover` / auto-activation call doesn't block on a
	// model download. Idempotent — only the first call actually triggers init.
	crate::embeddings::warmup();

	// Pre-embed every installed capability's trigger phrases in the
	// background. Combined with warmup's disk-cache load, this means the
	// first auto-activation after the model is ready hits an all-cached
	// path (~30 ms for the user-input embed only) instead of paying the
	// ~300-500 ms trigger-batch cost on the user's hot path. Tap updates
	// invalidate per-text (content hash) so only changed triggers are
	// recomputed.
	if let Ok(caps) = crate::agent::registry::list_all_capabilities(&config.capabilities) {
		let trigger_texts: Vec<String> = caps.into_iter().flat_map(|c| c.triggers).collect();
		crate::embeddings::prewarm(trigger_texts);
	}

	// The config passed here should be the merged config for the role
	// config.mcp.servers already contains only the role's enabled servers
	if config.mcp.servers.is_empty() {
		crate::log_debug!("No MCP servers enabled for this role");
		return Ok(());
	}

	crate::log_debug!(
		"Initializing {} MCP servers for role",
		config.mcp.servers.len()
	);

	// Collect external servers that need initialization (parallel-safe)
	let external_servers: Vec<_> = config
		.mcp
		.servers
		.iter()
		.filter(|server| {
			matches!(
				server.connection_type(),
				McpConnectionType::Http | McpConnectionType::Stdin
			)
		})
		.filter(|server| !server::is_server_already_running_with_config(server))
		.collect();

	// Log internal servers (no initialization needed)
	for server in config.mcp.servers.iter().filter(|s| {
		!matches!(
			s.connection_type(),
			McpConnectionType::Http | McpConnectionType::Stdin
		)
	}) {
		crate::log_debug!(
			"Skipping initialization for internal server: {} ({:?})",
			server.name(),
			server.connection_type()
		);
	}

	// Notify progress callback that we're starting
	if let Some(callback) = &progress_callback {
		callback(McpInitProgress::Starting {
			servers: external_servers
				.iter()
				.map(|s| s.name().to_string())
				.collect(),
		});
	}

	// Initialize all external servers in parallel
	let init_futures: Vec<_> = external_servers
		.into_iter()
		.map(|server| {
			let name = server.name().to_string();
			let callback = progress_callback;
			async move {
				crate::log_debug!("Initializing external server: {}", name);
				let result = server::get_server_functions(server).await;
				let (success, function_count) = match &result {
					Ok(functions) => (true, functions.len()),
					Err(_) => (false, 0),
				};
				if let Some(cb) = callback {
					cb(McpInitProgress::Completed {
						server: name.clone(),
						success,
						function_count,
					});
				}
				(name, result)
			}
		})
		.collect();

	let results = futures::future::join_all(init_futures).await;

	// Log results
	for (server_name, result) in results {
		match result {
			Ok(functions) => {
				crate::log_debug!(
					"Successfully initialized server '{}' with {} functions",
					server_name,
					functions.len()
				);
				for func in &functions {
					crate::log_debug!("  - Available: {}", func.name);
				}
			}
			Err(e) => {
				crate::log_debug!(
					"Failed to initialize server '{}': {} (will retry on first use)",
					server_name,
					e
				);
			}
		}
	}

	// Start the health monitor for external servers
	let config_arc = std::sync::Arc::new(config.clone());
	if let Err(e) = health_monitor::start_health_monitor(config_arc).await {
		crate::log_debug!("Failed to start health monitor: {}", e);
		// Don't fail startup - health monitoring is optional
	}

	crate::log_debug!("MCP server initialization completed");
	Ok(())
}

/// Initialize MCP servers and tool map for a role (used at startup and role switching).
pub async fn initialize_mcp_for_role(role: &str, config: &crate::config::Config) -> Result<()> {
	initialize_mcp_for_role_with_callback(role, config, None).await
}

/// Initialize MCP servers with optional progress callback for UI updates
pub async fn initialize_mcp_for_role_with_callback(
	role: &str,
	config: &crate::config::Config,
	progress_callback: Option<&(dyn Fn(McpInitProgress) + Send + Sync)>,
) -> Result<()> {
	let config_for_role = config.get_merged_config_for_role(role);
	// Set session context (role + project) so MCP servers receive it during initialization
	process::init_session_context(role);

	// Step 1: Initialize MCP servers first
	if let Err(e) =
		initialize_servers_for_role_with_callback(&config_for_role, progress_callback).await
	{
		crate::log_debug!("Warning: Failed to initialize MCP servers: {}", e);
		// Continue anyway - servers can be started on-demand if needed
	}

	// Step 2: Initialize tool map after servers are ready
	if let Err(e) = tool_map::initialize_tool_map(&config_for_role).await {
		crate::log_debug!("Warning: Failed to initialize tool map: {}", e);
		// Continue anyway - will fall back to building tool map on each use
	}

	Ok(())
}

// Collect filtered functions for a single server. Returns an empty Vec for unknown/unavailable servers.
async fn server_functions_for(
	server: &crate::config::McpServerConfig,
	config: &crate::config::Config,
) -> Vec<McpFunction> {
	match server.connection_type() {
		McpConnectionType::Builtin => match server.name() {
			"core" => {
				get_filtered_server_functions("core", server.tools(), core::get_all_functions)
			}
			"runtime" => {
				get_filtered_server_functions("runtime", server.tools(), runtime::get_all_functions)
			}
			"agent" => {
				let fns = agent::get_all_functions(config);
				filter_tools_by_patterns(fns, server.tools())
			}
			other => {
				crate::log_debug!("Unknown builtin server: {}", other);
				Vec::new()
			}
		},
		McpConnectionType::Http | McpConnectionType::Stdin => {
			match server::get_server_functions_cached(server).await {
				Ok(fns) => {
					let allowed = server.tools();
					// When the static filter is restrictive, also include tools unlocked
					// at runtime by capability activation (e.g. `capability enable shell`).
					if !allowed.is_empty() {
						let overlay =
							crate::config::runtime_overlay::extras_for_server(server.name());
						if !overlay.is_empty() {
							let mut effective = allowed.to_vec();
							for extra in overlay {
								if !effective.contains(&extra) {
									effective.push(extra);
								}
							}
							return filter_tools_by_patterns(fns, &effective);
						}
					}
					filter_tools_by_patterns(fns, allowed)
				}
				Err(e) => {
					crate::log_error!(
						"Failed to get cached functions from external server '{}': {} (will be available when server starts)",
						server.name(),
						e
					);
					Vec::new()
				}
			}
		}
	}
}

// Gather available functions from enabled servers WITHOUT spawning servers
// This is used for system prompt generation and should be fast
pub async fn get_available_functions(config: &crate::config::Config) -> Vec<McpFunction> {
	if config.mcp.servers.is_empty() {
		crate::log_debug!("MCP has no servers configured, no functions available");
		return Vec::new();
	}

	let mut functions = Vec::new();
	let session_id = crate::session::context::current_session_id();
	for server in &config.mcp.servers {
		// Skip config servers that are disabled in the dynamic registry
		if let Some(ref sid) = session_id {
			if let Some((_, enabled)) =
				crate::session::context::get_dynamic_server_for_session(sid, server.name())
			{
				if !enabled {
					continue;
				}
			}
		}
		functions.extend(server_functions_for(server, config).await);
	}

	// Include functions from dynamically added servers and agents.
	// Dynamic-side capability activation is responsible for not re-registering
	// servers that are already in the role's static config (see
	// `handle_enable` / `activate_capability_inline` in `core::capability`),
	// so this extend doesn't double-count tools.
	functions.extend(crate::mcp::core::dynamic::get_all_functions());
	functions.extend(crate::mcp::core::dynamic_agents::get_all_functions());

	// Project-local tools (`<workdir>/.agents/tools/<name>`) — always-on,
	// role-agnostic. Same shape as OCTOMIND_SKILLS but driven by disk presence.
	functions.extend(crate::mcp::core::local_tool::get_all_functions());

	functions
}

// Helper function to filter tools based on patterns
fn filter_tools_by_patterns(tools: Vec<McpFunction>, allowed_tools: &[String]) -> Vec<McpFunction> {
	if allowed_tools.is_empty() {
		tools
	} else {
		tools
			.into_iter()
			.filter(|func| is_tool_allowed_by_patterns(&func.name, allowed_tools))
			.collect()
	}
}

// Helper function to check if a tool matches allowed patterns
pub fn is_tool_allowed_by_patterns(tool_name: &str, allowed_tools: &[String]) -> bool {
	if allowed_tools.is_empty() {
		return true;
	}

	for pattern in allowed_tools {
		// Handle wildcard patterns
		if pattern.ends_with('*') {
			let prefix = &pattern[..pattern.len() - 1];
			if tool_name.starts_with(prefix) {
				return true;
			}
		} else {
			// Exact match
			if tool_name == pattern {
				return true;
			}
		}
	}

	false
}

// Get functions from server with optional filtering and caching
pub fn get_filtered_server_functions<F>(
	server_type: &str,
	allowed_tools: &[String],
	get_functions: F,
) -> Vec<McpFunction>
where
	F: FnOnce() -> Vec<McpFunction>,
{
	let cache_key = if allowed_tools.is_empty() {
		format!("{}_all", server_type)
	} else {
		format!("{}_{}", server_type, allowed_tools.join(","))
	};

	// Try to get from cache first
	{
		let cache = INTERNAL_FUNCTION_CACHE.read().unwrap();
		if let Some(cached_functions) = cache.get(&cache_key) {
			return cached_functions.clone();
		}
	}

	// Not in cache - compute and cache
	crate::log_debug!("Computing and caching {} functions", server_type);
	let all_functions = get_functions();
	let filtered_functions = if allowed_tools.is_empty() {
		all_functions
	} else {
		all_functions
			.into_iter()
			.filter(|func| is_tool_allowed_by_patterns(&func.name, allowed_tools))
			.collect()
	};

	// Cache the result
	{
		let mut cache = INTERNAL_FUNCTION_CACHE.write().unwrap();
		cache.insert(cache_key, filtered_functions.clone());
	}

	filtered_functions
}

// Clear function cache (useful for testing or when tools configuration changes)
pub fn clear_function_cache() {
	let mut cache = INTERNAL_FUNCTION_CACHE.write().unwrap();
	let count = cache.len();
	cache.clear();
	if count > 0 {
		crate::log_debug!("Cleared internal function cache for {} entries", count);
	}
}

// Execute a tool call
pub async fn execute_tool_call(
	call: &McpToolCall,
	config: &crate::config::Config,
	cancellation_token: Option<tokio::sync::watch::Receiver<bool>>,
) -> Result<(McpToolResult, u64)> {
	// Debug logging for tool execution
	log_debug!("Debug: Executing tool call: {}", call.tool_name);
	log_debug!(
		"Debug: MCP has {} servers configured",
		config.mcp.servers.len()
	);
	if let Ok(params) = serde_json::to_string_pretty(&call.parameters) {
		log_debug!("Debug: Tool parameters: {}", params);
	}

	// Only execute if MCP has any servers configured
	if config.mcp.servers.is_empty() {
		return Err(anyhow::anyhow!("MCP has no servers configured"));
	}

	// Check for cancellation before starting
	if let Some(ref token) = cancellation_token {
		if *token.borrow() {
			return Err(anyhow::anyhow!("Tool execution cancelled"));
		}
	}

	// Track tool execution time
	let tool_start = std::time::Instant::now();

	let result = try_execute_tool_call(call, config, cancellation_token).await;

	// Calculate tool execution time
	let tool_duration = tool_start.elapsed();
	let tool_time_ms = tool_duration.as_millis() as u64;

	// LRU bookkeeping: if this tool came from a dynamic-server backed
	// capability, bump its last_used so eviction tracks real usage.
	// Cheap (one HashMap scan); only walks active capabilities.
	if result.is_ok() {
		if let Some(server_name) =
			crate::mcp::core::dynamic::get_dynamic_server_name_by_tool(&call.tool_name)
		{
			crate::mcp::core::capability::touch_capability_for_server(&server_name);
		}
	}

	match result {
		Ok(tool_result) => {
			// Skip individual large response handling when called from parallel execution
			// Large response handling is now done in batch after all tools complete
			Ok((tool_result, tool_time_ms))
		}
		Err(e) => Err(e),
	}
}

// Build a simple tool-to-server lookup map for instant routing
pub async fn build_tool_server_map(
	config: &crate::config::Config,
) -> std::collections::HashMap<String, crate::config::McpServerConfig> {
	let mut tool_map = std::collections::HashMap::new();

	for server in &config.mcp.servers {
		let server_functions = server_functions_for(server, config).await;
		for function in server_functions {
			// CONFIGURATION ORDER PRIORITY: First server wins for each tool
			tool_map
				.entry(function.name)
				.or_insert_with(|| server.clone());
		}
	}

	crate::log_debug!("Built tool-to-server map with {} tools", tool_map.len());
	tool_map
}

// Internal function to actually execute the tool call with cancellation support
async fn try_execute_tool_call(
	call: &McpToolCall,
	config: &crate::config::Config,
	cancellation_token: Option<tokio::sync::watch::Receiver<bool>>,
) -> Result<McpToolResult> {
	// Only execute if MCP has any servers configured
	if config.mcp.servers.is_empty() {
		return Err(anyhow::anyhow!("MCP has no servers configured"));
	}

	// Create the actual tool execution future (without cancellation handling)
	let tool_execution_future =
		execute_tool_without_cancellation(call, config, cancellation_token.clone());

	// Apply centralized cancellation wrapper
	if let Some(token) = cancellation_token {
		// Check for cancellation before proceeding
		if *token.borrow() {
			return Err(anyhow::anyhow!("Tool execution cancelled"));
		}

		// Create cancellation future
		let mut cancel_receiver = token.clone();
		let cancellation_future = async move {
			loop {
				if *cancel_receiver.borrow() {
					break;
				}
				cancel_receiver.changed().await.ok();
			}
		};

		// Race between tool execution and cancellation
		tokio::select! {
			biased;

			_ = cancellation_future => {
				Err(anyhow::anyhow!("Tool execution cancelled during execution"))
			}
			result = tool_execution_future => {
				result
			}
		}
	} else {
		// No cancellation token - execute directly
		tool_execution_future.await
	}
}

// Dispatch a tool call to the correct builtin server handler.
// Returns Ok(McpToolResult) on success or a soft error, Err on hard routing failure.
async fn route_builtin_tool(
	call: &McpToolCall,
	server_name: &str,
	config: &crate::config::Config,
	cancellation_token: Option<tokio::sync::watch::Receiver<bool>>,
) -> Result<McpToolResult> {
	match server_name {
		"core" => {
			crate::log_debug!("Executing '{}' via core builtin server", call.tool_name);
			let result = match call.tool_name.as_str() {
				"plan" => core::execute_plan(call)
					.await
					.map_err(|e| format!("Plan execution failed: {}", e)),
				"tap" => core::execute_tap_command(call, config)
					.await
					.map_err(|e| format!("Tap tool failed: {}", e)),
				other => {
					return Err(anyhow::anyhow!(
						"Tool '{}' not implemented in core server",
						other
					))
				}
			};
			match result {
				Ok(mut r) => {
					r.tool_id = call.tool_id.clone();
					Ok(r)
				}
				Err(msg) => Ok(McpToolResult::error(
					call.tool_name.clone(),
					call.tool_id.clone(),
					msg,
				)),
			}
		}
		"runtime" => {
			crate::log_debug!("Executing '{}' via runtime builtin server", call.tool_name);
			let result = runtime::execute_runtime_tool(call, config)
				.await
				.map_err(|e| format!("Runtime tool failed: {}", e));
			match result {
				Ok(mut r) => {
					r.tool_id = call.tool_id.clone();
					Ok(r)
				}
				Err(msg) => Ok(McpToolResult::error(
					call.tool_name.clone(),
					call.tool_id.clone(),
					msg,
				)),
			}
		}
		"agent" => {
			if !call.tool_name.starts_with("agent_") {
				return Err(anyhow::anyhow!(
					"Tool '{}' not implemented in agent server",
					call.tool_name
				));
			}
			crate::log_debug!(
				"Executing agent tool '{}' via agent builtin server",
				call.tool_name
			);
			let mut result = agent::execute_agent_command(call, config, cancellation_token).await?;
			result.tool_id = call.tool_id.clone();
			Ok(result)
		}
		"local" => {
			crate::log_debug!(
				"Executing '{}' via local-tool runner (.agents/tools)",
				call.tool_name
			);
			let result = match core::local_tool::execute(call).await {
				Ok(mut r) => {
					r.tool_id = call.tool_id.clone();
					r
				}
				Err(e) => McpToolResult::error(
					call.tool_name.clone(),
					call.tool_id.clone(),
					format!("local tool '{}' failed: {}", call.tool_name, e),
				),
			};
			Ok(result)
		}
		other => Err(anyhow::anyhow!("Unknown builtin server: {}", other)),
	}
}

// Execute tool without any cancellation handling - pure tool logic
async fn execute_tool_without_cancellation(
	call: &McpToolCall,
	config: &crate::config::Config,
	cancellation_token: Option<tokio::sync::watch::Receiver<bool>>,
) -> Result<McpToolResult> {
	// STATIC ROUTING: Use pre-built tool map ONLY
	if let Some(target_server) = tool_map::get_server_for_tool(&call.tool_name) {
		// Session-ownership check: the global tool map may contain tools registered
		// by other sessions. In a session context, verify dynamic tools belong to us.
		if crate::session::context::current_session_id().is_some() {
			// Dynamic agent tools (agent_* prefix): allow if config-defined or owned by this session
			if let Some(agent_name) = call.tool_name.strip_prefix("agent_") {
				let is_config_agent = config.agents.iter().any(|a| a.name == agent_name);
				if !is_config_agent && !core::dynamic_agents::is_dynamic_by_tool(&call.tool_name) {
					return Ok(McpToolResult::error(
						call.tool_name.clone(),
						call.tool_id.clone(),
						format!(
							"Tool '{}' not found (belongs to another session)",
							call.tool_name
						),
					));
				}
			}
			// Dynamic server tools: allow if from a config-defined server or owned by this session.
			// Project-local tools live under the synthetic "local" server and are workdir-scoped,
			// so they bypass this check (no session-ownership concept applies).
			if !core::dynamic::is_dynamic_by_tool(&call.tool_name)
				&& target_server.name() != core::local_tool::SERVER_NAME
			{
				let is_config_server = config
					.mcp
					.servers
					.iter()
					.any(|s| s.name() == target_server.name());
				if !is_config_server {
					return Ok(McpToolResult::error(
						call.tool_name.clone(),
						call.tool_id.clone(),
						format!(
							"Tool '{}' not found (belongs to another session)",
							call.tool_name
						),
					));
				}
			}
		}

		crate::log_debug!(
			"Routing tool '{}' to server '{}' ({:?})",
			call.tool_name,
			target_server.name(),
			target_server.connection_type()
		);

		return match target_server.connection_type() {
			McpConnectionType::Builtin => {
				route_builtin_tool(call, target_server.name(), config, cancellation_token).await
			}
			McpConnectionType::Http | McpConnectionType::Stdin => {
				let mut result =
					server::execute_tool_call(call, &target_server, cancellation_token).await?;
				result.tool_id = call.tool_id.clone();
				Ok(result)
			}
		};
	}

	// Tool was not found in any server
	let available_tools = tool_map::get_all_tool_names();
	Err(anyhow::anyhow!(
		"Tool '{}' not found in any configured MCP server. Available tools: {}",
		call.tool_name,
		if available_tools.is_empty() {
			"none (tool map not initialized)".to_string()
		} else {
			available_tools.join(", ")
		}
	))
}

// Execute a tool call for a layer/agent context.
// Tool access is controlled by the ACP session's role config, not by per-layer restrictions.
pub async fn execute_layer_tool_call(
	call: &McpToolCall,
	config: &crate::config::Config,
	cancellation_token: Option<tokio::sync::watch::Receiver<bool>>,
) -> Result<(McpToolResult, u64)> {
	execute_tool_call(call, config, cancellation_token).await
}

// Execute multiple tool calls
pub async fn execute_tool_calls(
	calls: &[McpToolCall],
	config: &crate::config::Config,
) -> Vec<Result<(McpToolResult, u64)>> {
	let mut results = Vec::new();

	for call in calls {
		// Execute the tool call
		let result = execute_tool_call(call, config, None).await;
		results.push(result);
	}

	results
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_mcp_tool_result_is_error() {
		// Test success result
		let success_result = McpToolResult::success(
			"test_tool".to_string(),
			"test_id".to_string(),
			"Success message".to_string(),
		);
		assert!(
			!success_result.is_error(),
			"Success result should not be an error"
		);

		// Test error result
		let error_result = McpToolResult::error(
			"test_tool".to_string(),
			"test_id".to_string(),
			"Error message".to_string(),
		);
		assert!(error_result.is_error(), "Error result should be an error");

		// Test extract_content on success
		assert_eq!(success_result.extract_content(), "Success message");

		// Test extract_content on error
		assert_eq!(error_result.extract_content(), "Error message");
	}
}
