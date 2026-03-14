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
	/// Dynamically added servers (name -> config)
	servers: HashMap<String, McpServerConfig>,
	/// Cached functions for each server (name -> functions)
	functions: HashMap<String, Vec<McpFunction>>,
}

/// Global dynamic server manager - initialized once
static DYNAMIC_MANAGER: OnceLock<Arc<RwLock<DynamicManagerState>>> = OnceLock::new();

fn get_manager() -> &'static Arc<RwLock<DynamicManagerState>> {
	DYNAMIC_MANAGER.get_or_init(|| {
		Arc::new(RwLock::new(DynamicManagerState {
			servers: HashMap::new(),
			functions: HashMap::new(),
		}))
	})
}

/// Add a new dynamic MCP server
///
/// Tests the server blockingly before adding to ensure it works.
/// Returns the functions provided by the server on success.
pub async fn add_server(server: McpServerConfig) -> Result<Vec<McpFunction>> {
	let server_name = server.name().to_string();

	// Validate server has required fields
	match &server {
		McpServerConfig::Stdin { command, args, .. } if command.is_empty() => {
			anyhow::bail!("stdin server requires a command");
		}
		McpServerConfig::Http { url, .. } if url.is_empty() => {
			anyhow::bail!("http server requires a url");
		}
		_ => {}
	}

	// Test the server blockingly before adding
	match crate::mcp::server::get_server_functions(&server).await {
		Ok(functions) => {
			let func_count = functions.len();
			crate::log_debug!(
				"Dynamic server '{}' test passed with {} functions",
				server_name,
				func_count
			);

			// Store the server and its functions
			let manager = get_manager();
			let mut state = manager.write().unwrap();
			state.servers.insert(server_name.clone(), server);
			state
				.functions
				.insert(server_name.clone(), functions.clone());

			Ok(functions)
		}
		Err(e) => {
			anyhow::bail!("Server test failed: {}", e)
		}
	}
}

/// Remove a dynamic MCP server by name
///
/// Returns the removed server config if it existed, or None if not found.
pub fn remove_server(name: &str) -> Option<McpServerConfig> {
	let manager = get_manager();
	let mut state = manager.write().unwrap();
	state.functions.remove(name);
	state.servers.remove(name)
}

/// List all dynamic servers
pub fn list_servers() -> Vec<(String, Vec<String>)> {
	let manager = get_manager();
	let state = manager.read().unwrap();
	state
		.servers
		.iter()
		.map(|(name, config)| {
			let tools = config.tools().to_vec();
			(name.clone(), tools)
		})
		.collect()
}

/// Get functions for a specific dynamic server
pub fn get_functions(name: &str) -> Option<Vec<McpFunction>> {
	let manager = get_manager();
	let state = manager.read().unwrap();
	state.functions.get(name).cloned()
}

/// Get all dynamic server configs (for tool map building)
pub fn get_all_configs() -> Vec<McpServerConfig> {
	let manager = get_manager();
	let state = manager.read().unwrap();
	state.servers.values().cloned().collect()
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

/// Clear all dynamic servers (useful for testing)
#[cfg(test)]
pub fn clear_all() {
	let manager = get_manager();
	let mut state = manager.write().unwrap();
	state.servers.clear();
	state.functions.clear();
}

/// Get the mcp tool definition for managing dynamic servers
pub fn get_mcp_tool_function() -> McpFunction {
	McpFunction {
		name: "mcp".to_string(),
		description: "Manage MCP servers at runtime without editing config. Use when:\n- You need a tool that's available in an MCP server but not currently configured\n- You want to test a server temporarily before adding to config\n- You need different servers for different tasks\n\nActions:\n- list: Show currently loaded dynamic servers and their tools\n- add: Add a new MCP server (tests connection first, returns available tools)\n- remove: Unload a dynamic server by name".to_string(),
		parameters: json!({
			"type": "object",
			"properties": {
				"action": {
					"type": "string",
					"description": "Action to perform",
					"enum": ["add", "remove", "list"]
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
		"remove" => handle_remove(call).await,
		_ => Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("Unknown action '{}'. Use: add, remove, list", action),
		)),
	}
}

async fn handle_list(call: &crate::mcp::McpToolCall) -> Result<McpToolResult> {
	let servers = list_servers();

	if servers.is_empty() {
		return Ok(McpToolResult::success(
			call.tool_name.clone(),
			call.tool_id.clone(),
			"No dynamic MCP servers added yet. Use 'add' to add a server.".to_string(),
		));
	}

	let mut lines = vec!["Dynamic MCP Servers:".to_string(), "".to_string()];
	for (name, tools) in servers {
		let tools_str = if tools.is_empty() {
			"(all tools)".to_string()
		} else {
			tools.join(", ")
		};
		lines.push(format!("  {} → {}", name, tools_str));
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

	// Try to add the server (will test blockingly)
	match add_server(server_config).await {
		Ok(functions) => {
			let func_names: Vec<&str> = functions.iter().map(|f| f.name.as_str()).collect();
			let message = format!(
				"Successfully added server '{}' with {} tools:\n{}",
				server_name,
				func_names.len(),
				func_names.join(", ")
			);

			Ok(McpToolResult::success(
				call.tool_name.clone(),
				call.tool_id.clone(),
				message,
			))
		}
		Err(e) => Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("Failed to add server: {}", e),
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
