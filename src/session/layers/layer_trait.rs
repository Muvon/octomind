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

use crate::config::Config;
use crate::session::{ProviderExchange, Session, TokenUsage};
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

// Layer result that contains data returned from a layer's processing
pub struct LayerResult {
	pub outputs: Vec<String>, // All text outputs from layer processing
	pub exchange: ProviderExchange,
	pub token_usage: Option<TokenUsage>,
	pub tool_calls: Option<Vec<crate::mcp::McpToolCall>>,
	// Time tracking
	pub api_time_ms: u64,   // Time spent on API requests
	pub tool_time_ms: u64,  // Time spent executing tools
	pub total_time_ms: u64, // Total processing time for this layer
}

// Input mode determines what part of the previous layer's output will be used
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputMode {
	Last,    // Only the last assistant message from the session
	All,     // All messages/data from the previous layer
	Summary, // A summarized version of all data from the previous layer
}

impl InputMode {
	pub fn as_str(&self) -> &'static str {
		match self {
			InputMode::Last => "last",
			InputMode::All => "all",
			InputMode::Summary => "summary",
		}
	}
}

impl FromStr for InputMode {
	type Err = String;

	fn from_str(s: &str) -> Result<Self, Self::Err> {
		match s.to_lowercase().as_str() {
			"last" => Ok(InputMode::Last),
			"all" => Ok(InputMode::All),
			"summary" => Ok(InputMode::Summary),
			_ => Err(format!(
				"Unknown input mode: '{}'. Valid options: last, all, summary",
				s
			)),
		}
	}
}

// Custom deserializer for InputMode to handle string values from config
fn deserialize_input_mode<'de, D>(deserializer: D) -> Result<InputMode, D::Error>
where
	D: serde::Deserializer<'de>,
{
	use serde::de::Error;
	let s = String::deserialize(deserializer)?;
	InputMode::from_str(&s).map_err(D::Error::custom)
}

// Output mode determines how the layer's output affects the session
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputMode {
	None,    // Don't modify session (intermediate layer like task_refiner)
	Append,  // Add output as new message to session
	Replace, // Replace entire session with output (reducer functionality)
	Last,    // Append only the last response to session (ignore multiple outputs)
	Restart, // Replace session with only the last response (fresh start with last message)
}

impl OutputMode {
	pub fn as_str(&self) -> &'static str {
		match self {
			OutputMode::None => "none",
			OutputMode::Append => "append",
			OutputMode::Replace => "replace",
			OutputMode::Last => "last",
			OutputMode::Restart => "restart",
		}
	}
}

impl FromStr for OutputMode {
	type Err = String;

	fn from_str(s: &str) -> Result<Self, Self::Err> {
		match s.to_lowercase().as_str() {
			"none" => Ok(OutputMode::None),
			"append" => Ok(OutputMode::Append),
			"replace" => Ok(OutputMode::Replace),
			"last" => Ok(OutputMode::Last),
			"restart" => Ok(OutputMode::Restart),
			_ => Err(format!(
				"Unknown output mode: '{}'. Valid options: none, append, replace, last, restart",
				s
			)),
		}
	}
}

// Output role determines the role used when adding messages to the session
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputRole {
	Assistant, // Add output as assistant message (default)
	User,      // Add output as user message
}

impl OutputRole {
	pub fn as_str(&self) -> &'static str {
		match self {
			OutputRole::Assistant => "assistant",
			OutputRole::User => "user",
		}
	}
}

impl FromStr for OutputRole {
	type Err = String;

	fn from_str(s: &str) -> Result<Self, Self::Err> {
		match s.to_lowercase().as_str() {
			"assistant" => Ok(OutputRole::Assistant),
			"user" => Ok(OutputRole::User),
			_ => Err(format!(
				"Unknown output role: '{}'. Valid options: assistant, user",
				s
			)),
		}
	}
}

// Custom deserializer for OutputMode to handle string values from config
fn deserialize_output_mode<'de, D>(deserializer: D) -> Result<OutputMode, D::Error>
where
	D: serde::Deserializer<'de>,
{
	use serde::de::Error;
	let s = String::deserialize(deserializer)?;
	OutputMode::from_str(&s).map_err(D::Error::custom)
}

// Custom deserializer for OutputRole to handle string values from config
fn deserialize_output_role<'de, D>(deserializer: D) -> Result<OutputRole, D::Error>
where
	D: serde::Deserializer<'de>,
{
	use serde::de::Error;
	let s = String::deserialize(deserializer)?;
	OutputRole::from_str(&s).map_err(D::Error::custom)
}

// Configuration for layer-specific MCP settings
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct LayerMcpConfig {
	// Server references - list of server names from the global registry to use for this layer
	// Empty list means MCP is disabled for this layer
	#[serde(default)]
	pub server_refs: Vec<String>,

	#[serde(default)]
	pub allowed_tools: Vec<String>, // Specific tools allowed (empty = all tools from enabled servers)
}

impl LayerMcpConfig {
	/// Check if a tool is allowed based on allowed_tools patterns
	/// Supports:
	/// - Exact tool names: "text_editor"
	/// - Server group patterns: "filesystem:*" (all tools from filesystem server)
	/// - Server-specific patterns: "filesystem:text_*" (filesystem tools starting with "text_")
	pub fn is_tool_allowed(&self, tool_name: &str, server_name: &str) -> bool {
		// If no allowed_tools specified, all tools are allowed
		if self.allowed_tools.is_empty() {
			return true;
		}

		for pattern in &self.allowed_tools {
			// Check for server group pattern (e.g., "filesystem:*" or "filesystem:text_*")
			if let Some((server_prefix, tool_pattern)) = pattern.split_once(':') {
				// Check if server matches
				if server_prefix == server_name {
					// Check tool pattern
					if tool_pattern == "*" {
						// All tools from this server are allowed
						return true;
					} else if let Some(prefix) = tool_pattern.strip_suffix('*') {
						// Prefix matching (e.g., "text_*")
						if tool_name.starts_with(prefix) {
							return true;
						}
					} else {
						// Exact tool name within server namespace
						if tool_name == tool_pattern {
							return true;
						}
					}
				}
			} else {
				// Exact tool name match (backward compatibility)
				if tool_name == pattern {
					return true;
				}
			}
		}

		false
	}
}

// Common configuration properties for all layers - extended for flexibility
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LayerConfig {
	pub name: String,
	// Model is now optional - falls back to session model if not specified
	pub model: Option<String>,
	// System prompt is optional - uses built-in prompts for known layer types
	pub system_prompt: Option<String>,
	// Description for this layer (required - used for agents, commands, and documentation)
	pub description: String,
	pub temperature: f32,
	pub top_p: f32,
	pub top_k: u32,
	pub max_tokens: u32,
	#[serde(deserialize_with = "deserialize_input_mode")]
	pub input_mode: InputMode,
	#[serde(deserialize_with = "deserialize_output_mode")]
	pub output_mode: OutputMode,
	#[serde(deserialize_with = "deserialize_output_role")]
	pub output_role: OutputRole,
	// MCP configuration for this layer
	#[serde(default)]
	pub mcp: LayerMcpConfig,
	// Custom parameters that can be used in system prompts via placeholders
	#[serde(default)]
	pub parameters: std::collections::HashMap<String, serde_json::Value>,
	// Cached processed system prompt (not serialized - computed at session initialization)
	#[serde(skip)]
	pub processed_system_prompt: Option<String>,
}

impl LayerConfig {
	/// Get the effective model for this layer (fallback to session model if not specified)
	pub fn get_effective_model(&self, session_model: &str) -> String {
		self.model
			.clone()
			.unwrap_or_else(|| session_model.to_string())
	}

	/// Create a merged config that respects this layer's MCP settings
	/// This ensures that API calls use the layer's MCP configuration rather than just global settings
	pub fn get_merged_config_for_layer(
		&self,
		base_config: &crate::config::Config,
	) -> crate::config::Config {
		let mut merged_config = base_config.clone();

		// Create role-like MCP config from layer's server_refs
		if !self.mcp.server_refs.is_empty() {
			// CRITICAL BUG FIX: Use the base_config's full server registry instead of reloading
			// The base_config should already have the complete server registry available
			// Reloading Config::load() bypasses runtime processing and causes server_refs to fail

			// Use the same logic as RoleMcpConfig::get_enabled_servers()
			let layer_mcp_config = crate::config::RoleMcpConfig {
				server_refs: self.mcp.server_refs.clone(),
				allowed_tools: self.mcp.allowed_tools.clone(),
			};

			// Use base_config's server registry - it should contain all configured servers
			let enabled_servers = layer_mcp_config.get_enabled_servers(&base_config.mcp.servers);

			crate::log_debug!(
				"Layer '{}' enabling {} servers from server_refs: {:?}",
				self.name,
				enabled_servers.len(),
				self.mcp.server_refs
			);

			merged_config.mcp = crate::config::McpConfig {
				servers: enabled_servers,
				allowed_tools: self.mcp.allowed_tools.clone(),
			};
		} else {
			// No server_refs means MCP is disabled for this layer
			// Clear servers to ensure no MCP functionality
			merged_config.mcp.servers.clear();
			merged_config.mcp.allowed_tools.clear();
		}

		merged_config
	}

	/// Get the effective system prompt for this layer
	/// Returns the pre-processed system prompt (processed once during session initialization)
	pub fn get_effective_system_prompt(&self) -> String {
		// Return cached processed prompt if available
		if let Some(ref processed) = self.processed_system_prompt {
			processed.clone()
		} else {
			// Fallback for layers that haven't been processed yet
			// This should rarely happen in normal operation
			if let Some(ref custom_prompt) = self.system_prompt {
				custom_prompt.clone()
			} else {
				format!("You are a specialized AI layer named '{}'. Process the input according to your purpose.", self.name)
			}
		}
	}

	/// Process and cache the system prompt for this layer (called once during session initialization)
	pub async fn process_and_cache_system_prompt(&mut self, project_dir: &std::path::Path) {
		if let Some(ref custom_prompt) = self.system_prompt {
			let processed = self
				.process_prompt_placeholders_async(custom_prompt, project_dir)
				.await;
			self.processed_system_prompt = Some(processed);
		} else {
			// STRICT CONFIG: system_prompt must be defined in config for all layers
			panic!("CRITICAL CONFIG ERROR: Layer '{}' missing system_prompt. All layers must have system_prompt defined in config.", self.name);
		}
	}

	/// Process placeholders in system prompt using layer parameters (async version)
	async fn process_prompt_placeholders_async(
		&self,
		prompt: &str,
		project_dir: &std::path::Path,
	) -> String {
		let mut processed = prompt.to_string();

		// Replace standard placeholders using the async version
		processed =
			crate::session::helper_functions::process_placeholders_async(&processed, project_dir)
				.await;

		// Replace custom parameter placeholders — {{KEY}} syntax
		for (key, value) in &self.parameters {
			let replacement = match value {
				serde_json::Value::String(s) => s.clone(),
				serde_json::Value::Number(n) => n.to_string(),
				serde_json::Value::Bool(b) => b.to_string(),
				_ => serde_json::to_string(value).unwrap_or_default(),
			};
			processed = processed.replace(&format!("{{{{{}}}}}", key), &replacement);
		}

		processed
	}
}

// Trait that all layers must implement
#[async_trait]
pub trait Layer {
	fn name(&self) -> &str;
	fn config(&self) -> &LayerConfig;

	// Process the input through this layer
	// Each layer handles its own function calls with its own model
	// The process function is responsible for executing any function calls
	// and incorporating their results into the final output
	async fn process(
		&self,
		input: &str,
		session: &Session,
		config: &Config,
		operation_cancelled: tokio::sync::watch::Receiver<bool>,
	) -> Result<LayerResult>;

	// Helper function to prepare input based on input_mode
	fn prepare_input(&self, input: &str, session: &Session) -> String {
		// Each layer processes input in its own isolated context
		// The input mode determines what part of the previous context is used
		match self.config().input_mode {
			InputMode::Last => {
				// In Last mode, we get the last assistant response from the session
				// This is useful for commands that want to analyze or work with the last AI response
				if input.trim().is_empty() {
					// If no explicit input provided, get the last assistant message
					session
						.messages
						.iter()
						.rfind(|m| m.role == "assistant")
						.map(|m| m.content.clone())
						.unwrap_or_else(|| {
							// Fallback: if no assistant messages, get last user message
							session
								.messages
								.iter()
								.rfind(|m| m.role == "user")
								.map(|m| m.content.clone())
								.unwrap_or_else(|| "No previous messages found".to_string())
						})
				} else {
					// If explicit input provided, use it but also include last assistant context
					let last_assistant = session
						.messages
						.iter()
						.rfind(|m| m.role == "assistant")
						.map(|m| {
							format!(
								"Previous response:\n{}\n\nCurrent input:\n{}",
								m.content, input
							)
						})
						.unwrap_or_else(|| input.to_string());
					last_assistant
				}
			}
			InputMode::All => {
				// Build a chronological transcript of the session in natural reading order
				// (oldest → newest), followed by the current input as the task to act on.
				// Skips system messages — those are already in the layer's own system prompt.
				let transcript = session
					.messages
					.iter()
					.filter(|m| m.role != "system")
					.map(|m| {
						let label = match m.role.as_str() {
							"assistant" => "Assistant",
							"user" => "User",
							other => other,
						};
						format!("[{}]\n{}", label, m.content)
					})
					.collect::<Vec<_>>()
					.join("\n\n");

				if transcript.is_empty() {
					input.to_string()
				} else {
					format!("{}\n\n[Current task]\n{}", transcript, input)
				}
			}

			InputMode::Summary => {
				// For summary mode, we generate a concise summary of the conversation
				// This helps maintain context while reducing token usage
				crate::session::summarize_context(session, input)
			}
		}
	}
}
