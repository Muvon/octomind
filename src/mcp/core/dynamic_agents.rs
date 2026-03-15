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

//! Dynamic Agent Manager
//!
//! Provides runtime management of agents that can be added/removed
//! during a session. These agents are separate from the config-defined agents
//! and are stored in memory only.
//!
//! Unlike config agents (which spawn subprocesses), dynamic agents can be
//! executed in-process using the session infrastructure.

use crate::mcp::{McpFunction, McpToolResult};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

/// Dynamic agent configuration for in-process execution.
///
/// This is separate from `AgentConfig` (config/agents.rs) which is for
/// subprocess-based agents defined in the config file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DynamicAgentConfig {
	/// Unique agent name (becomes tool: agent_<name>)
	pub name: String,
	/// Human-readable description for the tool
	pub description: String,
	/// System prompt for the agent
	pub system: String,
	/// Optional welcome message
	#[serde(default)]
	pub welcome: String,
	/// Optional model override (e.g., "openai:gpt-4")
	pub model: Option<String>,
	/// Optional temperature override
	pub temperature: Option<f32>,
	/// Optional top_p override
	pub top_p: Option<f32>,
	/// Optional top_k override
	pub top_k: Option<u32>,
	/// MCP server references (names of dynamic MCP servers)
	#[serde(default)]
	pub server_refs: Vec<String>,
	/// Allowed tools filter (supports wildcards)
	#[serde(default)]
	pub allowed_tools: Vec<String>,
	/// Working directory for execution
	#[serde(default = "default_workdir")]
	pub workdir: String,
}

fn default_workdir() -> String {
	".".to_string()
}

/// Dynamic agent manager state
struct DynamicAgentManagerState {
	/// Registered agents (name -> config) — registered but not necessarily enabled
	agents: HashMap<String, DynamicAgentConfig>,
	/// Enabled status per agent (name -> bool)
	enabled: HashMap<String, bool>,
}

/// Global dynamic agent manager - initialized once
static DYNAMIC_AGENT_MANAGER: OnceLock<Arc<RwLock<DynamicAgentManagerState>>> = OnceLock::new();

fn get_agent_manager() -> &'static Arc<RwLock<DynamicAgentManagerState>> {
	DYNAMIC_AGENT_MANAGER.get_or_init(|| {
		Arc::new(RwLock::new(DynamicAgentManagerState {
			agents: HashMap::new(),
			enabled: HashMap::new(),
		}))
	})
}

/// Register a new dynamic agent without enabling it.
///
/// Stores the config in the registry with enabled=false.
/// Use `enable_agent` to activate it.
pub fn register_agent(agent: DynamicAgentConfig) -> Result<()> {
	let agent_name = agent.name.clone();

	if agent_name.is_empty() {
		anyhow::bail!("Agent name is required");
	}

	if agent.system.is_empty() {
		anyhow::bail!("Agent system prompt is required");
	}

	let manager = get_agent_manager();
	let mut state = manager.write().unwrap();
	if state.agents.contains_key(&agent_name) {
		anyhow::bail!(
			"Agent '{}' already registered. Use 'remove' first.",
			agent_name
		);
	}
	state.agents.insert(agent_name.clone(), agent);
	state.enabled.insert(agent_name, false);
	Ok(())
}

/// Enable a registered agent.
///
/// Marks the agent as enabled, making it available for execution.
/// Also registers the tool in the global tool map.
pub fn enable_agent(name: &str) -> Result<()> {
	let manager = get_agent_manager();
	let mut state = manager.write().unwrap();
	if !state.agents.contains_key(name) {
		anyhow::bail!("Agent '{}' not registered. Use 'add' first.", name);
	}
	state.enabled.insert(name.to_string(), true);
	drop(state); // Release lock before calling tool_map

	// Register the tool in the global tool map
	crate::mcp::tool_map::register_dynamic_agent_tool(name);
	Ok(())
}

/// Disable an enabled agent.
///
/// Marks the agent as disabled. The config stays registered.
/// Also unregisters the tool from the global tool map.
pub fn disable_agent(name: &str) -> Result<()> {
	let manager = get_agent_manager();
	let mut state = manager.write().unwrap();
	if !state.agents.contains_key(name) {
		anyhow::bail!("Agent '{}' not found", name);
	}
	state.enabled.insert(name.to_string(), false);
	drop(state); // Release lock before calling tool_map

	// Unregister the tool from the global tool map
	crate::mcp::tool_map::unregister_dynamic_agent_tool(name);
	Ok(())
}

/// Remove a dynamic agent by name.
///
/// Returns the removed agent config if it existed, or None if not found.
/// Also unregisters the tool from the global tool map.
pub fn remove_agent(name: &str) -> Option<DynamicAgentConfig> {
	let manager = get_agent_manager();
	let mut state = manager.write().unwrap();
	state.enabled.remove(name);
	let removed = state.agents.remove(name);
	drop(state); // Release lock before calling tool_map

	// Unregister from tool map if it was enabled
	if removed.is_some() {
		crate::mcp::tool_map::unregister_dynamic_agent_tool(name);
	}
	removed
}

/// List all registered agents with their enabled status.
pub fn list_agents() -> Vec<(DynamicAgentConfig, bool)> {
	let manager = get_agent_manager();
	let state = manager.read().unwrap();
	state
		.agents
		.iter()
		.map(|(name, config)| {
			let is_enabled = *state.enabled.get(name).unwrap_or(&false);
			(config.clone(), is_enabled)
		})
		.collect()
}

/// Get all enabled agent configs (for tool execution).
pub fn get_all_configs() -> Vec<DynamicAgentConfig> {
	let manager = get_agent_manager();
	let state = manager.read().unwrap();
	state
		.agents
		.iter()
		.filter(|(name, _)| *state.enabled.get(*name).unwrap_or(&false))
		.map(|(_, config)| config.clone())
		.collect()
}

/// Get a specific enabled agent by name.
pub fn get_enabled_agent(name: &str) -> Option<DynamicAgentConfig> {
	let manager = get_agent_manager();
	let state = manager.read().unwrap();
	if !state.enabled.get(name).unwrap_or(&false) {
		return None;
	}
	state.agents.get(name).cloned()
}

/// Check if an agent name is dynamically managed (registered, regardless of enabled).
pub fn is_dynamic(name: &str) -> bool {
	let manager = get_agent_manager();
	let state = manager.read().unwrap();
	state.agents.contains_key(name)
}

/// Check if an agent name is enabled.
pub fn is_enabled(name: &str) -> bool {
	let manager = get_agent_manager();
	let state = manager.read().unwrap();
	state.enabled.get(name).copied().unwrap_or(false) && state.agents.contains_key(name)
}

/// Check if a tool name belongs to any dynamic agent.
pub fn is_dynamic_by_tool(tool_name: &str) -> bool {
	let prefix = "agent_";
	if let Some(name) = tool_name.strip_prefix(prefix) {
		is_dynamic(name)
	} else {
		false
	}
}

/// Get the dynamic agent name for a specific tool.
pub fn get_dynamic_agent_name_by_tool(tool_name: &str) -> Option<String> {
	let prefix = "agent_";
	if let Some(name) = tool_name.strip_prefix(prefix) {
		let manager = get_agent_manager();
		let state = manager.read().unwrap();
		if state.agents.contains_key(name) {
			return Some(name.to_string());
		}
	}
	None
}

/// Clear all dynamic agents (useful for testing).
#[cfg(test)]
pub fn clear_all() {
	let manager = get_agent_manager();
	let mut state = manager.write().unwrap();
	state.agents.clear();
	state.enabled.clear();
}

/// Get the agent tool definition for managing dynamic agents.
pub fn get_agent_tool_function() -> McpFunction {
	McpFunction {
		name: "agent".to_string(),
		description: "Manage agents at runtime without editing config. Use when:\n- You need a specialized agent for a specific task\n- You want to test an agent temporarily before adding to config\n- You need different agents for different tasks\n\nIMPORTANT: This is for MANAGING agents (add/list/remove), NOT for executing agents.\nTo execute an agent, use the tool 'agent_<name>' directly after adding it.\n\nActions:\n- list: Show currently loaded dynamic agents\n- add: Register a new agent config (does NOT enable it yet)\n- enable: Enable a registered agent (makes it available for execution)\n- disable: Disable an agent (config stays registered)\n- remove: Unregister an agent entirely\n\nExample to add: {action: \"add\", name: \"researcher\", system: \"You are a research assistant...\", description: \"Research agent\"}".to_string(),
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
					"description": "Unique agent name (becomes tool: agent_<name>)"
				},
				"description": {
					"type": "string",
					"description": "Human-readable description for the agent tool"
				},
				"system": {
					"type": "string",
					"description": "System prompt for the agent (required for add)"
				},
				"welcome": {
					"type": "string",
					"description": "Optional welcome message"
				},
				"model": {
					"type": "string",
					"description": "Optional model override (e.g., 'openai:gpt-4')"
				},
				"temperature": {
					"type": "number",
					"description": "Optional temperature override"
				},
				"top_p": {
					"type": "number",
					"description": "Optional top_p override"
				},
				"top_k": {
					"type": "integer",
					"description": "Optional top_k override"
				},
				"server_refs": {
					"type": "array",
					"items": { "type": "string" },
					"description": "MCP server references (names of dynamic MCP servers)"
				},
				"allowed_tools": {
					"type": "array",
					"items": { "type": "string" },
					"description": "Allowed tools filter (supports wildcards)"
				},
				"workdir": {
					"type": "string",
					"description": "Working directory (optional, default: '.')"
				}
			},
			"required": ["action"],
			"additionalProperties": false
		}),
	}
}

/// Execute the agent tool command
pub async fn execute_agent_tool_command(call: &crate::mcp::McpToolCall) -> Result<McpToolResult> {
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
		"list" => handle_agent_list(call).await,
		"add" => handle_agent_add(call).await,
		"enable" => handle_agent_enable(call).await,
		"disable" => handle_agent_disable(call).await,
		"remove" => handle_agent_remove(call).await,
		_ => Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!(
				"Unknown action: {}. Use: add, enable, disable, remove, list",
				action
			),
		)),
	}
}

async fn handle_agent_list(call: &crate::mcp::McpToolCall) -> Result<McpToolResult> {
	let agents = list_agents();

	if agents.is_empty() {
		return Ok(McpToolResult::success(
			call.tool_name.clone(),
			call.tool_id.clone(),
			json!({
				"message": "No dynamic agents registered. Use 'add' to register an agent.",
				"agents": []
			})
			.to_string(),
		));
	}

	let agent_summaries: Vec<serde_json::Value> = agents
		.iter()
		.map(|(a, is_enabled)| {
			let status = if *is_enabled { "\u{2713}" } else { "\u{2717}" };
			json!({
				"name": a.name,
				"description": a.description,
				"system": a.system,
				"model": a.model,
				"server_refs": a.server_refs,
				"allowed_tools": a.allowed_tools,
				"workdir": a.workdir,
				"enabled": is_enabled,
				"status": status,
				"tool_name": format!("agent_{}", a.name)
			})
		})
		.collect();

	Ok(McpToolResult::success(
		call.tool_name.clone(),
		call.tool_id.clone(),
		json!({
			"message": format!("{} dynamic agent(s) registered", agents.len()),
			"agents": agent_summaries
		})
		.to_string(),
	))
}

async fn handle_agent_add(call: &crate::mcp::McpToolCall) -> Result<McpToolResult> {
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

	let description = params
		.get("description")
		.and_then(|v| v.as_str())
		.unwrap_or("")
		.to_string();

	let system = match params.get("system").and_then(|v| v.as_str()) {
		Some(s) if !s.trim().is_empty() => s.trim().to_string(),
		_ => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Missing required parameter 'system' (agent system prompt)".to_string(),
			));
		}
	};

	let welcome = params
		.get("welcome")
		.and_then(|v| v.as_str())
		.unwrap_or("")
		.to_string();

	let model = params
		.get("model")
		.and_then(|v| v.as_str())
		.map(String::from);

	let temperature = params
		.get("temperature")
		.and_then(|v| v.as_f64())
		.map(|t| t as f32);

	let top_p = params
		.get("top_p")
		.and_then(|v| v.as_f64())
		.map(|t| t as f32);

	let top_k = params
		.get("top_k")
		.and_then(|v| v.as_u64())
		.map(|t| t as u32);

	let server_refs: Vec<String> = params
		.get("server_refs")
		.and_then(|v| v.as_array())
		.map(|arr| {
			arr.iter()
				.filter_map(|v| v.as_str().map(String::from))
				.collect()
		})
		.unwrap_or_default();

	// Validate server_refs - check if referenced servers exist
	if !server_refs.is_empty() {
		let available_servers = crate::mcp::core::dynamic::list_servers();
		let available_names: std::collections::HashSet<_> = available_servers
			.iter()
			.map(|(name, _, _)| name.as_str())
			.collect();

		for server_ref in &server_refs {
			if !available_names.contains(server_ref.as_str()) {
				return Ok(McpToolResult::error(
					call.tool_name.clone(),
					call.tool_id.clone(),
					format!(
						"Server '{}' not found. Available servers: {}",
						server_ref,
						available_names
							.iter()
							.cloned()
							.collect::<Vec<_>>()
							.join(", ")
					),
				));
			}
		}
	}

	let allowed_tools: Vec<String> = params
		.get("allowed_tools")
		.and_then(|v| v.as_array())
		.map(|arr| {
			arr.iter()
				.filter_map(|v| v.as_str().map(String::from))
				.collect()
		})
		.unwrap_or_default();

	let workdir = params
		.get("workdir")
		.and_then(|v| v.as_str())
		.unwrap_or(".")
		.to_string();

	let agent = DynamicAgentConfig {
		name: name.clone(),
		description,
		system,
		welcome,
		model,
		temperature,
		top_p,
		top_k,
		server_refs,
		allowed_tools,
		workdir,
	};

	match register_agent(agent) {
		Ok(()) => Ok(McpToolResult::success(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("Agent '{}' registered. Use 'enable' to activate it.", name),
		)),
		Err(e) => Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("Failed to register agent: {}", e),
		)),
	}
}

async fn handle_agent_enable(call: &crate::mcp::McpToolCall) -> Result<McpToolResult> {
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

	match enable_agent(&name) {
		Ok(()) => Ok(McpToolResult::success(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("Agent '{}' enabled as tool 'agent_{}'.", name, name),
		)),
		Err(e) => Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("Failed to enable agent: {}", e),
		)),
	}
}

async fn handle_agent_disable(call: &crate::mcp::McpToolCall) -> Result<McpToolResult> {
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

	match disable_agent(&name) {
		Ok(()) => Ok(McpToolResult::success(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("Agent '{}' disabled.", name),
		)),
		Err(e) => Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("Failed to disable agent: {}", e),
		)),
	}
}

async fn handle_agent_remove(call: &crate::mcp::McpToolCall) -> Result<McpToolResult> {
	let params = &call.parameters;

	let name = match params.get("name").and_then(|v| v.as_str()) {
		Some(n) if !n.trim().is_empty() => n.trim(),
		_ => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Missing required parameter 'name'".to_string(),
			));
		}
	};

	match remove_agent(name) {
		Some(_) => Ok(McpToolResult::success(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("Agent '{}' removed.", name),
		)),
		None => Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("Agent '{}' not found", name),
		)),
	}
}

/// Generate MCP functions for all enabled dynamic agents.
pub fn get_all_functions() -> Vec<McpFunction> {
	let agents = get_all_configs();
	agents
		.iter()
		.map(|agent_config| {
			McpFunction {
				name: format!("agent_{}", agent_config.name),
				description: format!(
					"{}\n\n## Async Execution\n\n**async: false** (default) — blocks until complete, result returned immediately.\n**async: true** — returns immediately, result injected as user message when done.",
					agent_config.description
				),
				parameters: json!({
					"type": "object",
					"properties": {
						"task": {
							"type": "string",
							"description": "Task description in human language for the agent to process"
						},
						"async": {
							"type": "boolean",
							"description": "Run asynchronously. Result injected as user message when complete. Use for long-running tasks where you can continue other work. Default: false.",
							"default": false
						}
					},
					"required": ["task"]
				}),
			}
		})
		.collect()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_agent_register_enable_disable() {
		clear_all();

		let agent = DynamicAgentConfig {
			name: "test_agent".to_string(),
			description: "Test agent".to_string(),
			system: "You are a test agent.".to_string(),
			welcome: String::new(),
			model: None,
			temperature: None,
			top_p: None,
			top_k: None,
			server_refs: vec![],
			allowed_tools: vec![],
			workdir: ".".to_string(),
		};

		// Register
		register_agent(agent.clone()).unwrap();

		// List shows registered but not enabled
		let agents = list_agents();
		assert_eq!(agents.len(), 1);
		assert_eq!(agents[0].0.name, "test_agent");
		assert!(!agents[0].1); // not enabled

		// Enable
		enable_agent("test_agent").unwrap();
		let agents = list_agents();
		assert!(agents[0].1); // now enabled

		// Disable
		disable_agent("test_agent").unwrap();
		let agents = list_agents();
		assert!(!agents[0].1); // disabled again

		// Remove
		remove_agent("test_agent");
		let agents = list_agents();
		assert!(agents.is_empty());
	}

	#[test]
	fn test_duplicate_agent() {
		clear_all();

		let agent = DynamicAgentConfig {
			name: "dup_test".to_string(),
			description: "Test".to_string(),
			system: "You are a test.".to_string(),
			welcome: String::new(),
			model: None,
			temperature: None,
			top_p: None,
			top_k: None,
			server_refs: vec![],
			allowed_tools: vec![],
			workdir: ".".to_string(),
		};

		register_agent(agent.clone()).unwrap();
		let result = register_agent(agent);
		assert!(result.is_err());
	}

	#[test]
	fn test_agent_function_definition() {
		let func = get_agent_tool_function();
		assert_eq!(func.name, "agent");
		assert!(func.parameters.get("properties").is_some());
	}
}
