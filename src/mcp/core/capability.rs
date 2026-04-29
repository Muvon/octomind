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

//! Capability tool — runtime discovery and activation of capabilities.
//!
//! A capability is a domain abstraction (e.g., `database.postgres`,
//! `web.search`) that resolves to one or more MCP servers and a set of
//! allowed tools. Taps declare must-have capabilities in agent manifests;
//! those are merged into the effective config at boot. This tool exposes
//! the *runtime* lever: agents can list, discover, enable, and disable
//! capabilities on demand without knowing the underlying MCP server names.
//!
//! Actions:
//! - `list`     — show all installed capabilities (active marked).
//! - `enable`   — activate a capability by name (registers + enables its MCP servers).
//! - `disable`  — deactivate a previously-enabled capability.
//! - `discover` — keyword-match an intent string against capability names + descriptions.

use crate::config::Config;
use crate::mcp::{McpFunction, McpToolCall, McpToolResult};
use anyhow::Result;
use serde_json::json;
use std::collections::HashSet;
use std::sync::{Arc, OnceLock, RwLock};

// ---------------------------------------------------------------------------
// Active capabilities registry (process-global; mirrors dynamic.rs pattern)
// ---------------------------------------------------------------------------

/// Capabilities activated at runtime by this tool. Capabilities pre-loaded from
/// the tap manifest at boot are NOT tracked here — they are already merged into
/// the agent's effective config and represented as regular MCP servers.
static ACTIVE_CAPABILITIES: OnceLock<Arc<RwLock<HashSet<String>>>> = OnceLock::new();

fn registry() -> &'static Arc<RwLock<HashSet<String>>> {
	ACTIVE_CAPABILITIES.get_or_init(|| Arc::new(RwLock::new(HashSet::new())))
}

fn is_active(name: &str) -> bool {
	registry().read().unwrap().contains(name)
}

fn mark_active(name: &str) {
	registry().write().unwrap().insert(name.to_string());
}

fn mark_inactive(name: &str) {
	registry().write().unwrap().remove(name);
}

// ---------------------------------------------------------------------------
// McpFunction definition
// ---------------------------------------------------------------------------

pub fn get_capability_function() -> McpFunction {
	McpFunction {
		name: "capability".to_string(),
		description: r#"Discover and activate capabilities mid-session. Capabilities are domain abstractions (e.g., "database.postgres", "web.search") that resolve to MCP servers and tools. Use this when the agent needs functionality outside its starting kit.

Actions:
- `list`     — show all installed capabilities. Active ones are marked. Returns one line per capability: name + brief description.
- `enable`   — activate a capability by name. Registers and enables its MCP servers, exposing the capability's tools in subsequent turns.
- `disable`  — deactivate a previously-enabled capability.
- `discover` — find capabilities matching an intent string (substring match against name and description).

Workflow: call `list` or `discover` to find the right capability, then `enable` to activate it. The agent's tool surface grows on demand; nothing loaded that wasn't asked for."#.to_string(),
		parameters: json!({
			"type": "object",
			"properties": {
				"action": {
					"type": "string",
					"enum": ["list", "enable", "disable", "discover"],
					"description": "Action to perform"
				},
				"name": {
					"type": "string",
					"description": "Capability name (required for enable and disable)"
				},
				"intent": {
					"type": "string",
					"description": "Free-text intent for discover action (e.g., 'I need to query a database')"
				}
			},
			"required": ["action"]
		}),
	}
}

// ---------------------------------------------------------------------------
// Dispatcher
// ---------------------------------------------------------------------------

pub async fn execute_capability_command(
	call: &McpToolCall,
	config: &Config,
) -> Result<McpToolResult> {
	let action = match call.parameters.get("action").and_then(|v| v.as_str()) {
		Some(a) if !a.trim().is_empty() => a.trim().to_string(),
		_ => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Missing required parameter 'action'".to_string(),
			));
		}
	};
	match action.as_str() {
		"list" => handle_list(call, config).await,
		"enable" => handle_enable(call, config).await,
		"disable" => handle_disable(call, config).await,
		"discover" => handle_discover(call, config).await,
		other => Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!(
				"Unknown action '{other}'. Use list, enable, disable, or discover."
			),
		)),
	}
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn handle_list(call: &McpToolCall, config: &Config) -> Result<McpToolResult> {
	let caps = match crate::agent::registry::list_all_capabilities(&config.capabilities) {
		Ok(c) => c,
		Err(e) => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				format!("Failed to enumerate capabilities: {e}"),
			));
		}
	};
	if caps.is_empty() {
		return Ok(McpToolResult::success(
			call.tool_name.clone(),
			call.tool_id.clone(),
			"No capabilities installed in any tap.".to_string(),
		));
	}
	let mut output = format!("Installed capabilities ({}):\n", caps.len());
	for cap in &caps {
		let marker = if is_active(&cap.name) { "[active] " } else { "" };
		output.push_str(&format!(
			"- {}{} — {}\n",
			marker, cap.name, cap.description
		));
	}
	output.push_str("\nUse capability(action=\"enable\", name=\"<name>\") to activate.");
	Ok(McpToolResult::success(
		call.tool_name.clone(),
		call.tool_id.clone(),
		output,
	))
}

async fn handle_enable(call: &McpToolCall, config: &Config) -> Result<McpToolResult> {
	let name = match call.parameters.get("name").and_then(|v| v.as_str()) {
		Some(n) if !n.trim().is_empty() => n.trim().to_string(),
		_ => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Missing required parameter 'name'".to_string(),
			));
		}
	};

	if is_active(&name) {
		return Ok(McpToolResult::success(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("Capability '{name}' is already active."),
		));
	}

	let resolved =
		match crate::agent::registry::parse_capability_toml(&name, &config.capabilities) {
			Ok(r) => r,
			Err(e) => {
				return Ok(McpToolResult::error(
					call.tool_name.clone(),
					call.tool_id.clone(),
					format!("Capability '{name}' not found: {e}"),
				));
			}
		};

	if resolved.mcp_servers.is_empty() {
		return Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!(
				"Capability '{name}' has no MCP servers configured (no [[mcp.servers]] blocks)."
			),
		));
	}

	let filter_tools: Option<Vec<String>> = if resolved.allowed_tools.is_empty() {
		None
	} else {
		Some(resolved.allowed_tools.clone())
	};

	let mut activated_tools: Vec<String> = Vec::new();
	let mut activated_servers: Vec<String> = Vec::new();

	for server in &resolved.mcp_servers {
		let server_name = server.name().to_string();

		// Register if not already in the dynamic registry (idempotent on conflicts).
		if !crate::mcp::core::dynamic::is_dynamic(&server_name) {
			if let Err(e) = crate::mcp::core::dynamic::register_server(server.clone()) {
				return Ok(McpToolResult::error(
					call.tool_name.clone(),
					call.tool_id.clone(),
					format!(
						"Failed to register server '{server_name}' for capability '{name}': {e}"
					),
				));
			}
		}

		match crate::mcp::core::dynamic::enable_server(&server_name, filter_tools.clone()).await {
			Ok(functions) => {
				activated_tools.extend(functions.iter().map(|f| f.name.clone()));
				activated_servers.push(server_name);
			}
			Err(e) => {
				return Ok(McpToolResult::error(
					call.tool_name.clone(),
					call.tool_id.clone(),
					format!(
						"Failed to enable server '{server_name}' for capability '{name}': {e}"
					),
				));
			}
		}
	}

	mark_active(&name);

	let msg = format!(
		"Capability '{name}' enabled. Activated {} server(s): {}\nTools available: {}",
		activated_servers.len(),
		activated_servers.join(", "),
		if activated_tools.is_empty() {
			"none".to_string()
		} else {
			activated_tools.join(", ")
		}
	);
	Ok(McpToolResult::success(
		call.tool_name.clone(),
		call.tool_id.clone(),
		msg,
	))
}

async fn handle_disable(call: &McpToolCall, config: &Config) -> Result<McpToolResult> {
	let name = match call.parameters.get("name").and_then(|v| v.as_str()) {
		Some(n) if !n.trim().is_empty() => n.trim().to_string(),
		_ => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Missing required parameter 'name'".to_string(),
			));
		}
	};

	if !is_active(&name) {
		return Ok(McpToolResult::success(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("Capability '{name}' is not active."),
		));
	}

	let resolved =
		match crate::agent::registry::parse_capability_toml(&name, &config.capabilities) {
			Ok(r) => r,
			Err(e) => {
				return Ok(McpToolResult::error(
					call.tool_name.clone(),
					call.tool_id.clone(),
					format!(
						"Capability '{name}' not found (cannot determine servers to disable): {e}"
					),
				));
			}
		};

	let mut disabled_servers: Vec<String> = Vec::new();
	for server in &resolved.mcp_servers {
		let server_name = server.name().to_string();
		if crate::mcp::core::dynamic::is_dynamic(&server_name) {
			if let Err(e) =
				crate::mcp::core::dynamic::disable_server(&server_name, Some(config))
			{
				return Ok(McpToolResult::error(
					call.tool_name.clone(),
					call.tool_id.clone(),
					format!("Failed to disable server '{server_name}': {e}"),
				));
			}
			disabled_servers.push(server_name);
		}
	}

	mark_inactive(&name);

	Ok(McpToolResult::success(
		call.tool_name.clone(),
		call.tool_id.clone(),
		format!(
			"Capability '{name}' disabled. Deactivated {} server(s): {}",
			disabled_servers.len(),
			disabled_servers.join(", ")
		),
	))
}

async fn handle_discover(call: &McpToolCall, config: &Config) -> Result<McpToolResult> {
	let intent = match call.parameters.get("intent").and_then(|v| v.as_str()) {
		Some(i) if !i.trim().is_empty() => i.trim().to_lowercase(),
		_ => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Missing required parameter 'intent'".to_string(),
			));
		}
	};

	let caps = match crate::agent::registry::list_all_capabilities(&config.capabilities) {
		Ok(c) => c,
		Err(e) => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				format!("Failed to enumerate capabilities: {e}"),
			));
		}
	};

	// Keyword scoring: each whitespace-separated token in `intent` that appears
	// as a substring of `<name> <description>` (lowercase) adds 1 to the score.
	// Embedding-based scoring lands in a follow-up commit (Layer B).
	let intent_words: Vec<&str> = intent.split_whitespace().collect();
	let mut scored: Vec<(usize, &crate::agent::registry::ResolvedCapability)> = caps
		.iter()
		.map(|cap| {
			let haystack = format!("{} {}", cap.name, cap.description).to_lowercase();
			let score = intent_words
				.iter()
				.filter(|word| haystack.contains(*word))
				.count();
			(score, cap)
		})
		.filter(|(score, _)| *score > 0)
		.collect();
	scored.sort_by(|a, b| b.0.cmp(&a.0));

	let top: Vec<_> = scored.into_iter().take(5).collect();
	if top.is_empty() {
		return Ok(McpToolResult::success(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!(
				"No capabilities matched intent '{intent}'. Try `capability list` to see all installed capabilities."
			),
		));
	}

	let mut output = format!("Capabilities matching '{intent}':\n");
	for (score, cap) in top {
		let marker = if is_active(&cap.name) { "[active] " } else { "" };
		output.push_str(&format!(
			"- {}{} (score {}) — {}\n",
			marker, cap.name, score, cap.description
		));
	}
	output.push_str("\nUse capability(action=\"enable\", name=\"<name>\") to activate.");
	Ok(McpToolResult::success(
		call.tool_name.clone(),
		call.tool_id.clone(),
		output,
	))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn schema_has_required_action() {
		let f = get_capability_function();
		assert_eq!(f.name, "capability");
		let required = f
			.parameters
			.get("required")
			.and_then(|v| v.as_array())
			.expect("required array");
		assert!(required.iter().any(|v| v.as_str() == Some("action")));
	}

	#[test]
	fn active_registry_marks_and_clears() {
		let cap = "test.cap.alpha";
		assert!(!is_active(cap));
		mark_active(cap);
		assert!(is_active(cap));
		mark_inactive(cap);
		assert!(!is_active(cap));
	}
}
