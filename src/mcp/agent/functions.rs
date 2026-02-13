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
use crate::utils::file_parser::has_context_blocks;
use crate::utils::file_renderer::expand_context_blocks;
use anyhow::Result;
use serde_json::json;

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
		description: "Make a direct LLM call with runtime parameters, bypassing agent configuration.

		Parameters:
		- `prompt`: The input/prompt to process
		- `model`: Model in 'provider:model' format (e.g., 'openai:gpt-4o', 'openrouter:anthropic/claude-3.5-sonnet')
		- `system`: System prompt for the LLM
		- `temperature`: Temperature for randomness (0.0-2.0, default: 0.7)

		Note: Response size is controlled by global mcp_response_tokens_threshold setting.
		Use more specific prompts to reduce output size if responses are truncated.

		Examples:
		- Basic call: `{\"prompt\": \"Explain quantum computing\", \"model\": \"openai:gpt-4o\", \"system\": \"You are a helpful assistant\"}`
		- With temperature: `{\"prompt\": \"Write a poem\", \"model\": \"openrouter:anthropic/claude-3.5-sonnet\", \"system\": \"You are a creative writer\", \"temperature\": 1.2}`".to_string(),
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
	cancellation_token: Option<tokio::sync::watch::Receiver<bool>>,
) -> Result<McpToolResult> {
	// Handle call_llm tool
	if call.tool_name == "call_llm" {
		return execute_call_llm(call, config, cancellation_token).await;
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
				format!("Agent '{layer_name}' not configured"),
			));
		}
	};

	// Process task through the agent layer using the provider system
	let (result, agent_costs) =
		match process_layer_as_agent(agent_config, task, config, cancellation_token).await {
			Ok(res) => res,
			Err(e) => {
				return Ok(McpToolResult::error(
					call.tool_name.clone(),
					call.tool_id.clone(),
					format!("Agent processing failed: {e}"),
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
			format!("Failed to serialize agent costs: {e}"),
		)),
	}
}

// Process layer as agent using isolated session with full layer processing
async fn process_layer_as_agent(
	layer_config: &crate::session::layers::LayerConfig,
	task: &str,
	config: &crate::config::Config,
	cancellation_token: Option<tokio::sync::watch::Receiver<bool>>,
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

	// Process placeholders in agent system prompt before creating layer
	if let Some(ref system_prompt) = agent_layer_config.system_prompt {
		let current_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
		let processed_prompt = crate::session::helper_functions::process_placeholders_async(
			system_prompt,
			&current_dir,
		)
		.await;
		agent_layer_config.processed_system_prompt = Some(processed_prompt);
	}

	// Create GenericLayer from processed config
	let layer = GenericLayer::new(agent_layer_config);

	// Process task through layer with full MCP tools support
	// CRITICAL FIX: Use the passed cancellation token instead of creating a new one
	let operation_cancelled = cancellation_token.unwrap_or_else(|| {
		// Fallback: create a never-cancelled token if none provided
		tokio::sync::watch::channel(false).1
	});
	let result = layer
		.process(task, &agent_session, config, operation_cancelled)
		.await?;

	// Extract cost data from agent session
	let agent_costs = crate::session::AgentCostData {
		agent_name: layer_config.name.clone(),
		model: agent_session.info.model.clone(),
		input_tokens: agent_session.info.input_tokens,
		output_tokens: agent_session.info.output_tokens,
		cache_read_tokens: agent_session.info.cache_read_tokens,
		cache_write_tokens: agent_session.info.cache_write_tokens,
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

	// UNIFIED CONTEXT PROTOCOL: Expand context blocks in agent output
	let final_output = if has_context_blocks(&output) {
		crate::log_debug!(
			"Context blocks detected in agent {} output, expanding...",
			layer_config.name
		);
		expand_context_blocks(&output)
	} else {
		output
	};

	Ok((final_output, agent_costs))
}

// Execute call_llm tool - direct LLM call with runtime parameters
async fn execute_call_llm(
	call: &McpToolCall,
	config: &crate::config::Config,
	cancellation_token: Option<tokio::sync::watch::Receiver<bool>>,
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

	// Extract optional parameters - temperature must come from role config, not hardcoded
	// For agent calls, we need to get the default role's temperature
	let role_config_result = config.get_role_config("developer");
	let (default_role_config, _, _, _, _) = role_config_result;

	let temperature = call
		.parameters
		.get("temperature")
		.and_then(|v| v.as_f64())
		.map(|t| t as f32)
		.unwrap_or(default_role_config.temperature);

	let top_p = call
		.parameters
		.get("top_p")
		.and_then(|v| v.as_f64())
		.map(|t| t as f32)
		.unwrap_or(default_role_config.top_p);

	let top_k = call
		.parameters
		.get("top_k")
		.and_then(|v| v.as_u64())
		.map(|t| t as u32)
		.unwrap_or(default_role_config.top_k);

	let max_tokens = call
		.parameters
		.get("max_tokens")
		.and_then(|v| v.as_u64())
		.unwrap_or(4096) as u32;

	// Process placeholders in the provided system prompt
	let current_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
	let processed_system_prompt =
		crate::session::helper_functions::process_placeholders_async(system_prompt, &current_dir)
			.await;

	// Create temporary LayerConfig with runtime parameters
	let layer_config = crate::session::layers::LayerConfig {
		name: "call_llm".to_string(),
		model: Some(model.to_string()),
		system_prompt: Some(system_prompt.to_string()),
		description: "Direct LLM call with runtime parameters".to_string(),
		temperature,
		top_p,
		top_k,
		max_tokens,
		input_mode: crate::session::layers::layer_trait::InputMode::Last, // Doesn't matter as input is provided
		output_mode: crate::session::layers::layer_trait::OutputMode::Last, // Return only the last output
		output_role: crate::session::layers::layer_trait::OutputRole::Assistant, // Default role
		mcp: crate::session::layers::layer_trait::LayerMcpConfig {
			server_refs: vec![], // No MCP tools
			allowed_tools: vec![],
		},
		parameters: std::collections::HashMap::new(), // No custom parameters
		processed_system_prompt: Some(processed_system_prompt), // ✅ PROCESSED
	};

	// Process task through the layer using existing logic
	let (result, agent_costs) =
		process_layer_as_agent(&layer_config, task, config, cancellation_token).await?;

	// Return MCP-compliant result with cost metadata
	Ok(McpToolResult::success_with_metadata(
		call.tool_name.clone(),
		call.tool_id.clone(),
		result,
		serde_json::to_value(agent_costs)?,
	))
}
