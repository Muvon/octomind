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
	let mut functions = Vec::new();

	// Generate one function per agent configuration
	for agent_config in &config.agents {
		functions.push(McpFunction {
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
		});
	}

	// Add call_llm function
	functions.push(McpFunction {
		name: "call_llm".to_string(),
		description: "Make a direct LLM call with runtime parameters, bypassing agent configuration".to_string(),
		parameters: json!({
			"type": "object",
			"properties": {
				"prompt": {
					"type": "string",
					"description": "The input/prompt to process"
				},
				"model": {
					"type": "string",
					"description": "Model in 'provider:model' format (e.g., 'openai:gpt-4o', 'openrouter:anthropic/claude-3.5-sonnet')"
				},
				"system": {
					"type": "string",
					"description": "System prompt for the LLM"
				},
				"temperature": {
					"type": "number",
					"description": "Temperature for randomness (0.0-2.0, default: 0.7)",
					"minimum": 0.0,
					"maximum": 2.0
				},
				"max_tokens": {
					"type": "integer",
					"description": "Maximum output tokens (default: 4096)",
					"minimum": 1
				}
			},
			"required": ["prompt", "model", "system"]
		}),
	});

	functions
}

// Execute agent tool call
pub async fn execute_agent_command(
	call: &McpToolCall,
	config: &crate::config::Config,
	_cancellation_token: Option<Arc<AtomicBool>>,
) -> Result<McpToolResult> {
	// Handle call_llm tool
	if call.tool_name == "call_llm" {
		return execute_call_llm(call, config).await;
	}

	// Extract layer name from tool name (agent_<layer_name>)
	let layer_name = match call.tool_name.strip_prefix("agent_") {
		Some(name) => name,
		None => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				format!("Invalid agent tool name: {}", call.tool_name),
			));
		}
	};

	let task = match call.parameters.get("task").and_then(|v| v.as_str()) {
		Some(t) => {
			if t.trim().is_empty() {
				return Ok(McpToolResult::error(
					call.tool_name.clone(),
					call.tool_id.clone(),
					"Task parameter cannot be empty".to_string(),
				));
			}
			t
		}
		None => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Agent tool requires 'task' parameter".to_string(),
			));
		}
	};

	// Find the agent configuration directly (agents are now LayerConfigs)
	let agent_config = match config.agents.iter().find(|agent| agent.name == layer_name) {
		Some(config) => config,
		None => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				format!("Agent '{}' not configured", layer_name),
			));
		}
	};

	// Process task through the agent layer using the provider system
	let (result, agent_costs) = match process_layer_as_agent(agent_config, task, config).await {
		Ok(res) => res,
		Err(e) => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				format!("Agent processing failed: {}", e),
			));
		}
	};

	// Return MCP-compliant result with cost metadata
	match serde_json::to_value(agent_costs) {
		Ok(metadata) => Ok(McpToolResult::success_with_metadata(
			call.tool_name.clone(),
			call.tool_id.clone(),
			result,
			metadata,
		)),
		Err(e) => Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("Failed to serialize agent costs: {}", e),
		)),
	}
}

// Process layer as agent using isolated session with full layer processing
async fn process_layer_as_agent(
	layer_config: &crate::session::layers::LayerConfig,
	task: &str,
	config: &crate::config::Config,
) -> Result<(String, crate::session::AgentCostData)> {
	// Create isolated session for agent
	let agent_session = crate::session::Session::new(
		format!("agent_{}", layer_config.name),
		layer_config.get_effective_model(&config.model),
		"agent".to_string(),
	);

	// Create a modified layer config with agent prefix for display context
	let mut agent_layer_config = layer_config.clone();
	agent_layer_config.name = format!("agent_{}", layer_config.name);

	// Create GenericLayer from modified config (reuse existing pattern)
	let layer = GenericLayer::new(agent_layer_config);

	// Process task through layer with full MCP tools support
	let operation_cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
	let result = layer
		.process(task, &agent_session, config, operation_cancelled)
		.await?;

	// Extract cost data from agent session
	let agent_costs = crate::session::AgentCostData {
		agent_name: layer_config.name.clone(),
		model: agent_session.info.model.clone(),
		input_tokens: agent_session.info.input_tokens,
		output_tokens: agent_session.info.output_tokens,
		cached_tokens: agent_session.info.cached_tokens,
		cost: agent_session.info.total_cost,
		api_time_ms: agent_session.info.total_api_time_ms,
		tool_time_ms: agent_session.info.total_tool_time_ms,
		layer_time_ms: agent_session.info.total_layer_time_ms,
	};

	// Handle output_mode to determine what gets returned by the agent tool
	use crate::session::layers::layer_trait::OutputMode;
	let output = match layer_config.output_mode {
		OutputMode::None => {
			// Return only the final layer output (cleanest for tool use)
			result.outputs.last().unwrap_or(&String::new()).clone()
		}
		OutputMode::Append => result.outputs.join("\n---\n"),
		OutputMode::Replace => {
			// For agents, same as None - return only the layer output
			result.outputs.last().unwrap_or(&String::new()).clone()
		}
		OutputMode::Last => {
			// Return only the last layer output
			result.outputs.last().unwrap_or(&String::new()).clone()
		}
		OutputMode::Restart => {
			// For agents, same as Last - return only the last layer output
			result.outputs.last().unwrap_or(&String::new()).clone()
		}
	};

	Ok((output, agent_costs))
}

// Execute call_llm tool - direct LLM call with runtime parameters
async fn execute_call_llm(
	call: &McpToolCall,
	config: &crate::config::Config,
) -> Result<McpToolResult> {
	// Extract required parameters
	let task = call
		.parameters
		.get("prompt")
		.and_then(|v| v.as_str())
		.ok_or_else(|| anyhow::anyhow!("call_llm requires 'prompt' parameter"))?;

	let model = call
		.parameters
		.get("model")
		.and_then(|v| v.as_str())
		.ok_or_else(|| anyhow::anyhow!("call_llm requires 'model' parameter"))?;

	let system_prompt = call
		.parameters
		.get("system")
		.and_then(|v| v.as_str())
		.ok_or_else(|| anyhow::anyhow!("call_llm requires 'system' parameter"))?;

	// Extract optional parameters with defaults
	let temperature = call
		.parameters
		.get("temperature")
		.and_then(|v| v.as_f64())
		.unwrap_or(0.7) as f32;

	let max_tokens = call
		.parameters
		.get("max_tokens")
		.and_then(|v| v.as_u64())
		.unwrap_or(4096) as u32;

	// Create temporary LayerConfig with runtime parameters
	let layer_config = crate::session::layers::LayerConfig {
		name: "call_llm".to_string(),
		model: Some(model.to_string()),
		system_prompt: Some(system_prompt.to_string()),
		description: "Direct LLM call with runtime parameters".to_string(),
		temperature,
		max_tokens,
		input_mode: crate::session::layers::layer_trait::InputMode::Last, // Doesn't matter as input is provided
		output_mode: crate::session::layers::layer_trait::OutputMode::Last, // Return only the last output
		mcp: crate::session::layers::layer_trait::LayerMcpConfig {
			server_refs: vec![], // No MCP tools
			allowed_tools: vec![],
		},
		parameters: std::collections::HashMap::new(), // No custom parameters
		processed_system_prompt: None,                // Will be processed during execution
	};

	// Process task through the layer using existing logic
	let (result, agent_costs) = process_layer_as_agent(&layer_config, task, config).await?;

	// Return MCP-compliant result with cost metadata
	Ok(McpToolResult::success_with_metadata(
		call.tool_name.clone(),
		call.tool_id.clone(),
		result,
		serde_json::to_value(agent_costs)?,
	))
}
