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

// Agent functions - routes tasks to configured layers

use crate::mcp::{McpFunction, McpToolCall, McpToolResult};
use crate::session::layers::{GenericLayer, Layer};
use anyhow::Result;
use serde_json::json;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

// Get all available agent functions based on config
pub fn get_all_functions(config: &crate::config::Config) -> Vec<McpFunction> {
	// Generate one function per agent configuration
	config
		.agents
		.iter()
		.map(|agent_config| McpFunction {
			name: format!("agent_{}", agent_config.name),
			description: agent_config.description.clone(),
			parameters: json!({
				"type": "object",
				"properties": {
					"task": {
						"type": "string",
						"description": "Task description in human language for the agent to process"
					}
				},
				"required": ["task"]
			}),
		})
		.collect()
}

// Execute agent tool call
pub async fn execute_agent_command(
	call: &McpToolCall,
	config: &crate::config::Config,
	_cancellation_token: Option<Arc<AtomicBool>>,
) -> Result<McpToolResult> {
	// Extract layer name from tool name (agent_<layer_name>)
	let layer_name = call
		.tool_name
		.strip_prefix("agent_")
		.ok_or_else(|| anyhow::anyhow!("Invalid agent tool name: {}", call.tool_name))?;

	let task = call
		.parameters
		.get("task")
		.and_then(|v| v.as_str())
		.ok_or_else(|| anyhow::anyhow!("Agent tool requires 'task' parameter"))?;

	// Verify this agent is configured
	let _agent_config = config
		.agents
		.iter()
		.find(|agent| agent.name == layer_name)
		.ok_or_else(|| anyhow::anyhow!("Agent '{}' not configured", layer_name))?;

	// Find the layer by name in the full layers registry (not role-filtered)
	let layer_config = if let Some(all_layers) = &config.layers {
		all_layers
			.iter()
			.find(|layer| layer.name == layer_name)
			.cloned()
	} else {
		None
	}
	.ok_or_else(|| anyhow::anyhow!("Layer '{}' not found in configuration", layer_name))?;

	// Process task through the layer using the provider system
	let result = process_layer_as_agent(&layer_config, task, config).await?;

	// Return MCP-compliant result
	Ok(McpToolResult::success(
		call.tool_name.clone(),
		call.tool_id.clone(),
		result,
	))
}

// Process layer as agent using isolated session with full layer processing
async fn process_layer_as_agent(
	layer_config: &crate::session::layers::LayerConfig,
	task: &str,
	config: &crate::config::Config,
) -> Result<String> {
	// Create isolated session for agent
	let agent_session = crate::session::Session::new(
		format!("agent_{}", layer_config.name),
		layer_config.get_effective_model(&config.model),
		"agent".to_string(),
	);

	// Create GenericLayer from config (reuse existing pattern)
	let layer = GenericLayer::new(layer_config.clone());

	// Process task through layer with full MCP tools support
	let operation_cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
	let result = layer
		.process(task, &agent_session, config, operation_cancelled)
		.await?;

	// Collect all messages from agent session in chronological order
	let all_messages: Vec<String> = agent_session
		.messages
		.iter()
		.map(|msg| format!("[{}] {}", msg.role, msg.content))
		.collect();

	// Return combined result: layer output + session messages
	let final_result = if all_messages.is_empty() {
		result.output
	} else {
		format!(
			"{}\n\n--- Session Messages ---\n{}",
			result.output,
			all_messages.join("\n")
		)
	};

	Ok(final_result)
}
