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

use crate::config::agents::AgentConfig;
use crate::mcp::{McpFunction, McpToolResult};
use anyhow::Result;
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

/// Dynamic agent manager state
#[derive(Default)]
struct DynamicAgentManagerState {
	/// Dynamically added agents (name -> config)
	agents: HashMap<String, AgentConfig>,
}

/// Global dynamic agent manager - initialized once
static DYNAMIC_AGENT_MANAGER: OnceLock<Arc<RwLock<DynamicAgentManagerState>>> = OnceLock::new();

fn get_agent_manager() -> &'static Arc<RwLock<DynamicAgentManagerState>> {
	DYNAMIC_AGENT_MANAGER.get_or_init(|| Arc::new(RwLock::new(DynamicAgentManagerState::default())))
}

/// Add a new dynamic agent
///
/// Returns the agent config on success.
pub fn add_agent(agent: AgentConfig) -> Result<AgentConfig> {
	let agent_name = agent.name.clone();

	if agent_name.is_empty() {
		anyhow::bail!("Agent name is required");
	}

	if agent.command.is_empty() {
		anyhow::bail!("Agent command is required");
	}

	// Check for duplicate
	let manager = get_agent_manager();
	{
		let state = manager.read().unwrap();
		if state.agents.contains_key(&agent_name) {
			anyhow::bail!("Agent '{}' already exists. Use 'remove' first.", agent_name);
		}
	}

	// Store the agent
	let mut state = manager.write().unwrap();
	state.agents.insert(agent_name, agent.clone());

	Ok(agent)
}

/// Remove a dynamic agent by name
///
/// Returns the removed agent config if it existed, or None if not found.
pub fn remove_agent(name: &str) -> Option<AgentConfig> {
	let manager = get_agent_manager();
	let mut state = manager.write().unwrap();
	state.agents.remove(name)
}

/// List all dynamic agents
pub fn list_agents() -> Vec<AgentConfig> {
	let manager = get_agent_manager();
	let state = manager.read().unwrap();
	state.agents.values().cloned().collect()
}

/// Get all dynamic agent configs (for tool execution)
pub fn get_all_configs() -> Vec<AgentConfig> {
	list_agents()
}

/// Check if an agent name is dynamically managed
pub fn is_dynamic(name: &str) -> bool {
	let manager = get_agent_manager();
	let state = manager.read().unwrap();
	state.agents.contains_key(name)
}

/// Check if a tool name belongs to any dynamic agent
pub fn is_dynamic_by_tool(tool_name: &str) -> bool {
	let prefix = "agent_";
	if let Some(name) = tool_name.strip_prefix(prefix) {
		is_dynamic(name)
	} else {
		false
	}
}

/// Get the dynamic agent name for a specific tool
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

/// Clear all dynamic agents (useful for testing)
#[cfg(test)]
pub fn clear_all() {
	let manager = get_agent_manager();
	let mut state = manager.write().unwrap();
	state.agents.clear();
}

/// Get the agent tool definition for managing dynamic agents
pub fn get_agent_tool_function() -> McpFunction {
	McpFunction {
		name: "agent".to_string(),
		description: "Manage agents at runtime without editing config. Use when:\n- You need a specialized agent for a specific task\n- You want to test an agent temporarily before adding to config\n- You need different agents for different tasks\n\nIMPORTANT: This is for MANAGING agents (add/list/remove), NOT for executing agents.\nTo execute an agent, use the tool 'agent_<name>' directly after adding it.\n\nActions:\n- list: Show currently loaded dynamic agents\n- add: Add a new agent (requires 'command' - the full shell command)\n- remove: Unload a dynamic agent by name\n\nExample to add: {action: \"add\", name: \"researcher\", command: \"octomind acp --role researcher\", description: \"Research agent\"}".to_string(),
		parameters: json!({
			"type": "object",
			"properties": {
				"action": {
					"type": "string",
					"description": "Action: 'list', 'add', or 'remove'",
					"enum": ["add", "remove", "list"]
				},
				"name": {
					"type": "string",
					"description": "Unique agent name (becomes tool: agent_<name>)"
				},
				"description": {
					"type": "string",
					"description": "Description for the agent tool"
				},
				"command": {
					"type": "string",
					"description": "Full shell command to run the agent (e.g., 'octomind acp --role researcher')"
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
		"remove" => handle_agent_remove(call).await,
		_ => Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("Unknown action: {}. Use: list, add, remove", action),
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
				"message": "No dynamic agents loaded",
				"agents": []
			})
			.to_string(),
		));
	}

	let agent_summaries: Vec<serde_json::Value> = agents
		.iter()
		.map(|a| {
			json!({
				"name": a.name,
				"description": a.description,
				"command": a.command,
				"workdir": a.workdir,
				"tool_name": format!("agent_{}", a.name)
			})
		})
		.collect();

	Ok(McpToolResult::success(
		call.tool_name.clone(),
		call.tool_id.clone(),
		json!({
			"message": format!("{} dynamic agent(s) loaded", agents.len()),
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

	let command = match params.get("command").and_then(|v| v.as_str()) {
		Some(c) if !c.trim().is_empty() => c.trim().to_string(),
		_ => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Missing required parameter 'command'".to_string(),
			));
		}
	};

	let workdir = params
		.get("workdir")
		.and_then(|v| v.as_str())
		.unwrap_or(".")
		.to_string();

	let agent = AgentConfig {
		name: name.clone(),
		description,
		command,
		workdir,
	};

	match add_agent(agent) {
		Ok(added) => Ok(McpToolResult::success(
			call.tool_name.clone(),
			call.tool_id.clone(),
			json!({
				"message": format!("Agent '{}' added successfully", name),
				"agent": {
					"name": added.name,
					"description": added.description,
					"command": added.command,
					"workdir": added.workdir,
					"tool_name": format!("agent_{}", added.name)
				}
			})
			.to_string(),
		)),
		Err(e) => Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("Failed to add agent: {}", e),
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
			json!({
				"message": format!("Agent '{}' removed successfully", name)
			})
			.to_string(),
		)),
		None => Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("Agent '{}' not found", name),
		)),
	}
}

/// Generate MCP functions for all dynamic agents
pub fn get_all_functions() -> Vec<McpFunction> {
	let agents = list_agents();
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
	fn test_agent_add_list_remove() {
		clear_all();

		// Add an agent
		let agent = AgentConfig {
			name: "test_agent".to_string(),
			description: "Test agent".to_string(),
			command: "echo hello".to_string(),
			workdir: ".".to_string(),
		};
		let added = add_agent(agent.clone()).unwrap();
		assert_eq!(added.name, "test_agent");

		// List agents
		let agents = list_agents();
		assert_eq!(agents.len(), 1);
		assert_eq!(agents[0].name, "test_agent");

		// Check tool name detection
		assert!(is_dynamic_by_tool("agent_test_agent"));
		assert!(!is_dynamic_by_tool("agent_nonexistent"));
		assert!(!is_dynamic_by_tool("other_tool"));

		// Remove agent
		let removed = remove_agent("test_agent");
		assert!(removed.is_some());

		// List should be empty
		let agents = list_agents();
		assert!(agents.is_empty());
	}

	#[test]
	fn test_duplicate_agent() {
		clear_all();

		let agent = AgentConfig {
			name: "dup_test".to_string(),
			description: "Test".to_string(),
			command: "echo test".to_string(),
			workdir: ".".to_string(),
		};

		add_agent(agent.clone()).unwrap();
		let result = add_agent(agent);
		assert!(result.is_err());
	}

	#[test]
	fn test_agent_function_definition() {
		clear_all();

		let func = get_agent_tool_function();
		assert_eq!(func.name, "agent");
		assert!(func.parameters.get("properties").is_some());
	}
}
