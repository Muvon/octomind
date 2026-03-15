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

//! Dynamic MCP Server Manager
//!
//! Provides runtime management of MCP servers that can be added/removed
//! during a session. These servers are separate from the config-defined servers
//! and are stored in memory only.

use crate::config::McpServerConfig;
use crate::mcp::{McpFunction, McpToolResult};
use anyhow::Result;
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

/// Dynamic server manager state
struct DynamicManagerState {
	/// Registered servers (name -> config) — registered but not necessarily enabled
	servers: HashMap<String, McpServerConfig>,
	/// Cached functions for each enabled server (name -> functions)
	functions: HashMap<String, Vec<McpFunction>>,
	/// Enabled status per server (name -> bool)
	enabled: HashMap<String, bool>,
}

/// Global dynamic server manager - initialized once
static DYNAMIC_MANAGER: OnceLock<Arc<RwLock<DynamicManagerState>>> = OnceLock::new();

fn get_manager() -> &'static Arc<RwLock<DynamicManagerState>> {
	DYNAMIC_MANAGER.get_or_init(|| {
		Arc::new(RwLock::new(DynamicManagerState {
			servers: HashMap::new(),
			functions: HashMap::new(),
			enabled: HashMap::new(),
		}))
	})
}

/// Register a new dynamic MCP server without connecting to it.
///
/// Stores the config in the registry with enabled=false.
/// Use `enable_server` to connect and activate it.
pub fn register_server(server: McpServerConfig) -> Result<()> {
	let server_name = server.name().to_string();

	if server_name.is_empty() {
		anyhow::bail!("server name cannot be empty");
	}

	// Validate server has required fields
	match &server {
		McpServerConfig::Stdin { command, .. } if command.is_empty() => {
			anyhow::bail!("stdin server requires a command");
		}
		McpServerConfig::Http { url, .. } if url.is_empty() => {
			anyhow::bail!("http server requires a url");
		}
		_ => {}
	}

	let manager = get_manager();
	let mut state = manager.write().unwrap();
	if state.servers.contains_key(&server_name) {
		anyhow::bail!(
			"Server '{}' already registered. Use 'remove' first.",
			server_name
		);
	}
	state.servers.insert(server_name.clone(), server);
	state.enabled.insert(server_name, false);
	Ok(())
}

/// Enable a registered server: connect, fetch tools, apply optional filter, mark enabled.
///
/// Returns the list of activated functions.
/// Also registers the tools in the global tool map.
pub async fn enable_server(
	name: &str,
	filter_tools: Option<Vec<String>>,
) -> Result<Vec<McpFunction>> {
	let server =
		{
			let manager = get_manager();
			let state = manager.read().unwrap();
			state.servers.get(name).cloned().ok_or_else(|| {
				anyhow::anyhow!("Server '{}' not registered. Use 'add' first.", name)
			})?
		};

	let functions = crate::mcp::server::get_server_functions(&server)
		.await
		.map_err(|e| anyhow::anyhow!("Failed to connect to server '{}': {}", name, e))?;

	// Apply tool filter if provided
	let functions = if let Some(ref filter) = filter_tools {
		if filter.is_empty() {
			functions
		} else {
			crate::mcp::filter_tools_by_patterns(functions, filter)
		}
	} else {
		functions
	};

	crate::log_debug!(
		"Dynamic server '{}' enabled with {} functions",
		name,
		functions.len()
	);

	let tool_names: Vec<String> = functions.iter().map(|f| f.name.clone()).collect();

	let manager = get_manager();
	let mut state = manager.write().unwrap();
	state.functions.insert(name.to_string(), functions.clone());
	state.enabled.insert(name.to_string(), true);
	drop(state); // Release lock before calling tool_map

	// Register tools in the global tool map
	crate::mcp::tool_map::register_dynamic_server_tools(name, &server, &tool_names);

	Ok(functions)
}

/// Disable an enabled server: mark disabled and remove cached functions.
///
/// The server config stays registered; use `enable_server` to re-activate.
/// Also unregisters the tools from the global tool map.
pub fn disable_server(name: &str) -> Result<()> {
	let manager = get_manager();
	let mut state = manager.write().unwrap();
	if !state.servers.contains_key(name) {
		anyhow::bail!("Server '{}' not found", name);
	}
	state.enabled.insert(name.to_string(), false);
	let tool_names: Vec<String> = state
		.functions
		.get(name)
		.map(|f| f.iter().map(|f| f.name.clone()).collect())
		.unwrap_or_default();
	state.functions.remove(name);
	drop(state); // Release lock before calling tool_map

	crate::mcp::tool_map::unregister_dynamic_server_tools(name, &tool_names);
	Ok(())
}

/// Remove a dynamic MCP server by name
///
/// Returns the removed server config if it existed, or None if not found.
/// Also unregisters the tools from the global tool map.
pub fn remove_server(name: &str) -> Option<McpServerConfig> {
	let manager = get_manager();
	let mut state = manager.write().unwrap();
	let tool_names: Vec<String> = state
		.functions
		.get(name)
		.map(|f| f.iter().map(|f| f.name.clone()).collect())
		.unwrap_or_default();
	state.functions.remove(name);
	state.enabled.remove(name);
	let removed = state.servers.remove(name);
	drop(state); // Release lock before calling tool_map

	// Unregister tools from the global tool map
	if removed.is_some() {
		crate::mcp::tool_map::unregister_dynamic_server_tools(name, &tool_names);
	}
	removed
}

/// List all registered dynamic servers with their tool filter and enabled status.
pub fn list_servers() -> Vec<(String, Vec<String>, bool)> {
	let manager = get_manager();
	let state = manager.read().unwrap();
	state
		.servers
		.iter()
		.map(|(name, config)| {
			let tools = config.tools().to_vec();
			let is_enabled = *state.enabled.get(name).unwrap_or(&false);
			(name.clone(), tools, is_enabled)
		})
		.collect()
}

/// Get functions for a specific dynamic server
pub fn get_functions(name: &str) -> Option<Vec<McpFunction>> {
	let manager = get_manager();
	let state = manager.read().unwrap();
	state.functions.get(name).cloned()
}

/// Get all enabled dynamic server configs (for tool map building).
pub fn get_all_configs() -> Vec<McpServerConfig> {
	let manager = get_manager();
	let state = manager.read().unwrap();
	state
		.servers
		.iter()
		.filter(|(name, _)| *state.enabled.get(*name).unwrap_or(&false))
		.map(|(_, config)| config.clone())
		.collect()
}

/// Get all dynamic server functions (for tool map building)
pub fn get_all_functions() -> Vec<McpFunction> {
	let manager = get_manager();
	let state = manager.read().unwrap();
	state.functions.values().flatten().cloned().collect()
}

/// Check if a server name is dynamically managed
pub fn is_dynamic(name: &str) -> bool {
	let manager = get_manager();
	let state = manager.read().unwrap();
	state.servers.contains_key(name)
}

/// Check if a tool belongs to any dynamic server (by tool name)
pub fn is_dynamic_by_tool(tool_name: &str) -> bool {
	let manager = get_manager();
	let state = manager.read().unwrap();
	state
		.functions
		.values()
		.any(|funcs| funcs.iter().any(|f| f.name == tool_name))
}

/// Get the dynamic server name for a specific tool (by tool name)
pub fn get_dynamic_server_name_by_tool(tool_name: &str) -> Option<String> {
	let manager = get_manager();
	let state = manager.read().unwrap();
	for (server_name, funcs) in &state.functions {
		if funcs.iter().any(|f| f.name == tool_name) {
			return Some(server_name.clone());
		}
	}
	None
}

/// Clear all dynamic servers (useful for testing).
#[cfg(test)]
pub fn clear_all() {
	let manager = get_manager();
	let mut state = manager.write().unwrap();
	state.servers.clear();
	state.functions.clear();
	state.enabled.clear();
}

/// Get the mcp tool definition for managing dynamic servers.
pub fn get_mcp_tool_function() -> McpFunction {
	McpFunction {
		name: "mcp".to_string(),
		description: "Manage MCP servers at runtime without editing config. Use when:\n- You need a tool that's available in an MCP server but not currently configured\n- You want to test a server temporarily before adding to config\n- You need different servers for different tasks\n\nActions:\n- list: Show currently loaded dynamic servers and their tools\n- add: Register a new MCP server config (does NOT connect yet)\n- enable: Connect to a registered server and activate its tools\n- disable: Deactivate a server's tools (config stays registered)\n- remove: Unregister a server entirely".to_string(),
		parameters: json!({
			"type": "object",
			"properties": {
				"action": {
					"type": "string",
					"description": "Action to perform",
					"enum": ["add", "remove", "enable", "disable", "list"]
				},
				"name": {
					"type": "string",
					"description": "Unique name to identify this server instance"
				},
				"server_type": {
					"type": "string",
					"description": "How to connect to the server",
					"enum": ["stdin", "http"]
				},
				"command": {
					"type": "string",
					"description": "Executable to run (e.g., 'npx', 'uvx', './server'). Required for stdin type."
				},
				"args": {
					"type": "array",
					"items": { "type": "string" },
					"description": "Arguments passed to command (e.g., ['-m', 'mcp-server-github']). Required for stdin type."
				},
				"url": {
					"type": "string",
					"description": "HTTP endpoint of MCP server (e.g., 'http://localhost:3000'). Required for http type."
				},
				"auth_token": {
					"type": "string",
					"description": "Bearer token for authentication (optional, for http type)"
				},
				"timeout_seconds": {
					"type": "number",
					"description": "How long to wait for server response (default: 60)"
				},
				"tools": {
					"type": "array",
					"items": { "type": "string" },
					"description": "Which tools to expose from this server. Empty = all tools. Supports wildcards (e.g., ['github_*'])"
				}
			},
			"required": ["action"],
			"additionalProperties": false
		}),
	}
}

/// Execute the mcp tool command
pub async fn execute_mcp_command(call: &crate::mcp::McpToolCall) -> Result<McpToolResult> {
	let params = &call.parameters;

	// Extract action
	let action = match params.get("action").and_then(|v| v.as_str()) {
		Some(a) => a,
		None => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Missing required parameter 'action'".to_string(),
			));
		}
	};

	match action {
		"list" => handle_list(call).await,
		"add" => handle_add(call).await,
		"enable" => handle_enable(call).await,
		"disable" => handle_disable(call).await,
		"remove" => handle_remove(call).await,
		_ => Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!(
				"Unknown action '{}'. Use: add, enable, disable, remove, list",
				action
			),
		)),
	}
}

async fn handle_list(call: &crate::mcp::McpToolCall) -> Result<McpToolResult> {
	let servers = list_servers();

	if servers.is_empty() {
		return Ok(McpToolResult::success(
			call.tool_name.clone(),
			call.tool_id.clone(),
			"No dynamic MCP servers registered yet. Use 'add' to register a server.".to_string(),
		));
	}

	let mut lines = vec!["Dynamic MCP Servers:".to_string(), "".to_string()];
	for (name, tools, is_enabled) in servers {
		let status = if is_enabled { "\u{2713}" } else { "\u{2717}" };
		let tools_str = if tools.is_empty() {
			"(all tools)".to_string()
		} else {
			tools.join(", ")
		};
		lines.push(format!("  {} [{}] \u{2192} {}", name, status, tools_str));
	}

	Ok(McpToolResult::success(
		call.tool_name.clone(),
		call.tool_id.clone(),
		lines.join("\n"),
	))
}

async fn handle_add(call: &crate::mcp::McpToolCall) -> Result<McpToolResult> {
	let params = &call.parameters;

	// Extract required fields
	let name = match params.get("name").and_then(|v| v.as_str()) {
		Some(n) if !n.trim().is_empty() => n.trim().to_string(),
		_ => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Missing required parameter 'name'".to_string(),
			));
		}
	};

	let server_name = name.clone();

	let server_type = match params.get("server_type").and_then(|v| v.as_str()) {
		Some(t) => t,
		None => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Missing required parameter 'server_type' (stdin or http)".to_string(),
			));
		}
	};

	// Get optional fields
	let timeout_seconds = params
		.get("timeout_seconds")
		.and_then(|v| v.as_u64())
		.unwrap_or(60);

	let tools: Vec<String> = params
		.get("tools")
		.and_then(|v| v.as_array())
		.map(|arr| {
			arr.iter()
				.filter_map(|v| v.as_str().map(String::from))
				.collect()
		})
		.unwrap_or_default();

	// Build server config based on type
	let server_config = match server_type {
		"stdin" => {
			let command = match params.get("command").and_then(|v| v.as_str()) {
				Some(c) if !c.trim().is_empty() => c.trim().to_string(),
				_ => {
					return Ok(McpToolResult::error(
						call.tool_name.clone(),
						call.tool_id.clone(),
						"stdin server requires 'command' parameter".to_string(),
					));
				}
			};

			let args: Vec<String> = params
				.get("args")
				.and_then(|v| v.as_array())
				.map(|arr| {
					arr.iter()
						.filter_map(|v| v.as_str().map(String::from))
						.collect()
				})
				.unwrap_or_default();

			McpServerConfig::Stdin {
				name,
				command,
				args,
				timeout_seconds,
				tools,
			}
		}
		"http" => {
			let url = match params.get("url").and_then(|v| v.as_str()) {
				Some(u) if !u.trim().is_empty() => u.trim().to_string(),
				_ => {
					return Ok(McpToolResult::error(
						call.tool_name.clone(),
						call.tool_id.clone(),
						"http server requires 'url' parameter".to_string(),
					));
				}
			};

			let auth_token = params
				.get("auth_token")
				.and_then(|v| v.as_str())
				.map(String::from);

			McpServerConfig::Http {
				name,
				url,
				auth_token,
				oauth: None,
				timeout_seconds,
				tools,
			}
		}
		_ => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Invalid server_type. Use: stdin or http".to_string(),
			));
		}
	};

	// Register the server config (no connection yet)
	match register_server(server_config) {
		Ok(()) => Ok(McpToolResult::success(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!(
				"Server '{}' registered. Use 'enable' to activate it.",
				server_name
			),
		)),
		Err(e) => Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("Failed to register server: {}", e),
		)),
	}
}

async fn handle_enable(call: &crate::mcp::McpToolCall) -> Result<McpToolResult> {
	let params = &call.parameters;

	let name = match params.get("name").and_then(|v| v.as_str()) {
		Some(n) if !n.trim().is_empty() => n.trim().to_string(),
		_ => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Missing required parameter 'name'".to_string(),
			));
		}
	};

	let filter_tools: Option<Vec<String>> =
		params.get("tools").and_then(|v| v.as_array()).map(|arr| {
			arr.iter()
				.filter_map(|v| v.as_str().map(String::from))
				.collect()
		});

	match enable_server(&name, filter_tools).await {
		Ok(functions) => {
			let func_names: Vec<&str> = functions.iter().map(|f| f.name.as_str()).collect();
			Ok(McpToolResult::success(
				call.tool_name.clone(),
				call.tool_id.clone(),
				format!(
					"Server '{}' enabled with {} tools:\n{}",
					name,
					func_names.len(),
					func_names.join(", ")
				),
			))
		}
		Err(e) => Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("Failed to enable server: {}", e),
		)),
	}
}

async fn handle_disable(call: &crate::mcp::McpToolCall) -> Result<McpToolResult> {
	let params = &call.parameters;

	let name = match params.get("name").and_then(|v| v.as_str()) {
		Some(n) if !n.trim().is_empty() => n.trim().to_string(),
		_ => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Missing required parameter 'name'".to_string(),
			));
		}
	};

	match disable_server(&name) {
		Ok(()) => Ok(McpToolResult::success(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("Server '{}' disabled.", name),
		)),
		Err(e) => Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("Failed to disable server: {}", e),
		)),
	}
}

async fn handle_remove(call: &crate::mcp::McpToolCall) -> Result<McpToolResult> {
	let params = &call.parameters;

	let name = match params.get("name").and_then(|v| v.as_str()) {
		Some(n) if !n.trim().is_empty() => n.trim().to_string(),
		_ => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Missing required parameter 'name'".to_string(),
			));
		}
	};

	if let Some(_removed) = remove_server(&name) {
		Ok(McpToolResult::success(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("Removed server '{}'", name),
		))
	} else {
		Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("Server '{}' not found in dynamic servers", name),
		))
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test]
	async fn test_list_empty() {
		clear_all();
		let servers = list_servers();
		assert!(servers.is_empty());
	}

	#[test]
	fn test_mcp_function_definition() {
		let func = get_mcp_tool_function();
		assert_eq!(func.name, "mcp");
		assert!(func.parameters.get("properties").is_some());
	}
}
