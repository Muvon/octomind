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
use std::io::{IsTerminal, Write};
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
				Ok(fns) => filter_tools_by_patterns(fns, server.tools()),
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
	for server in &config.mcp.servers {
		functions.extend(server_functions_for(server, config).await);
	}

	// Include functions from dynamically added servers and agents
	functions.extend(crate::mcp::core::dynamic::get_all_functions());
	functions.extend(crate::mcp::core::dynamic_agents::get_all_functions());

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
				"mcp" => core::execute_mcp_command(call, config)
					.await
					.map_err(|e| format!("MCP management failed: {}", e)),
				"agent" => core::execute_agent_tool_command(call)
					.await
					.map_err(|e| format!("Agent management failed: {}", e)),
				"schedule" => core::execute_schedule_tool(call)
					.await
					.map_err(|e| format!("Schedule execution failed: {}", e)),
				"skill" => core::execute_skill_tool(call)
					.await
					.map_err(|e| format!("Skill tool failed: {}", e)),
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
			// Dynamic server tools: allow if from a config-defined server or owned by this session
			if !core::dynamic::is_dynamic_by_tool(&call.tool_name) {
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

// Helper function to handle large response warnings
pub async fn handle_large_response(
	result: McpToolResult,
	config: &crate::config::Config,
	mode: crate::session::output::OutputMode,
) -> Result<McpToolResult> {
	// Check if result is large - warn user if it exceeds threshold
	let estimated_tokens = crate::session::estimate_tokens(&result.extract_content());
	if config.mcp_response_warning_threshold > 0
		&& estimated_tokens > config.mcp_response_warning_threshold
	{
		// Create a modified result that warns about the size
		use colored::Colorize;
		let suppress_cli_output = mode.should_suppress_cli_output();
		let non_interactive = !mode.is_interactive() || !std::io::stdin().is_terminal();

		// Get server name for better identification
		let server_name =
			crate::session::chat::response::get_tool_server_name_async(&result.tool_name, config)
				.await;

		if !suppress_cli_output {
			println!(
				"{}",
				format!(
					"! WARNING: Tool '{}' ({}){} produced a large output ({} tokens)",
					result.tool_name,
					server_name,
					if !result.tool_id.is_empty() {
						format!(" [ID: {}]", result.tool_id)
					} else {
						String::new()
					},
					estimated_tokens
				)
				.bright_yellow()
			);
			println!(
				"{}",
				"This may consume significant tokens and impact your usage limits.".bright_yellow()
			);
		}

		// Auto-decline in structured output or non-interactive mode.
		if suppress_cli_output || non_interactive {
			if !suppress_cli_output {
				println!(
					"{}",
					format!(
						"Large output from '{}' ({}) automatically declined in non-interactive mode. Continuing...",
						result.tool_name, server_name
					)
					.bright_red()
				);
			}
			return Ok(McpToolResult::error(
				result.tool_name.clone(),
				result.tool_id.clone(),
				format!("Large output from tool '{}' ({} tokens) was automatically declined in non-interactive mode to avoid excessive token usage. The tool executed successfully but the output was too large.", result.tool_name, estimated_tokens)
			));
		}

		// CRITICAL: Suspend animation before prompting user
		// This prevents animation from covering the prompt and from being restarted
		// by other code paths while waiting for user input
		use crate::session::chat::get_animation_manager;
		let animation_manager = get_animation_manager();
		animation_manager.suspend().await;

		// Interactive terminal mode - ask user for confirmation before proceeding
		print!(
			"{}",
			"Do you want to continue with this large output? [y/N]: ".bright_cyan()
		);
		std::io::stdout().flush().unwrap();

		let mut input = String::new();
		std::io::stdin().read_line(&mut input).unwrap_or_default();

		// Resume animation now that user input is complete
		animation_manager.resume();

		if !input.trim().to_lowercase().starts_with('y') {
			// User declined large output. Return an MCP-compliant error result instead of
			// breaking the communication flow. This allows the conversation to continue
			// normally while informing the AI that the user declined the large output.
			println!(
				"{}",
				format!(
					"Large output from '{}' ({}) declined by user. Continuing conversation...",
					result.tool_name, server_name
				)
				.bright_red()
			);
			return Ok(McpToolResult::error(
				result.tool_name.clone(),
				result.tool_id.clone(),
				format!("User declined to process large output from tool '{}' ({} tokens). The tool executed successfully but the output was too large and the user chose not to include it in the conversation to avoid excessive token usage.", result.tool_name, estimated_tokens)
			));
		}

		// User confirmed, continue with original result
		println!("{}", "Proceeding with full output...".bright_green());
	}

	Ok(result)
}

// Execute a tool call with layer-specific restrictions
pub async fn execute_layer_tool_call(
	call: &McpToolCall,
	config: &crate::config::Config,
	layer_config: &crate::session::layers::LayerConfig,
	cancellation_token: Option<tokio::sync::watch::Receiver<bool>>,
) -> Result<(McpToolResult, u64)> {
	// Check if tools are enabled for this layer (has server_refs)
	if layer_config.mcp.server_refs.is_empty() {
		return Err(anyhow::anyhow!("Tool execution is disabled for this layer"));
	}

	// Check if specific tool is allowed for this layer using pattern-based validation
	let server_name = crate::mcp::tool_map::get_tool_server_name(&call.tool_name)
		.unwrap_or_else(|| "unknown".to_string());

	if !layer_config
		.mcp
		.is_tool_allowed(&call.tool_name, &server_name)
	{
		return Err(anyhow::anyhow!(
			"Tool '{}' is not allowed for this layer",
			call.tool_name
		));
	}

	// Pass to regular tool execution with cancellation token
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
