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
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

// Layer result that contains data returned from a layer's processing
pub struct LayerResult {
	pub output: String,
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

impl Default for InputMode {
	fn default() -> Self {
		Self::Last
	}
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
	None,    // Don't modify session (intermediate layer like query_processor)
	Append,  // Add output as new message to session
	Replace, // Replace entire session with output (reducer functionality)
}

impl Default for OutputMode {
	fn default() -> Self {
		Self::None
	}
}

impl OutputMode {
	pub fn as_str(&self) -> &'static str {
		match self {
			OutputMode::None => "none",
			OutputMode::Append => "append",
			OutputMode::Replace => "replace",
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
			_ => Err(format!(
				"Unknown output mode: '{}'. Valid options: none, append, replace",
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

// Common configuration properties for all layers - extended for flexibility
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LayerConfig {
	pub name: String,
	// Model is now optional - falls back to session model if not specified
	pub model: Option<String>,
	// System prompt is optional - uses built-in prompts for known layer types
	pub system_prompt: Option<String>,
	#[serde(default = "default_temperature")]
	pub temperature: f32,
	#[serde(default, deserialize_with = "deserialize_input_mode")]
	pub input_mode: InputMode,
	#[serde(default, deserialize_with = "deserialize_output_mode")]
	pub output_mode: OutputMode,
	// MCP configuration for this layer
	#[serde(default)]
	pub mcp: LayerMcpConfig,
	// Custom parameters that can be used in system prompts via placeholders
	#[serde(default)]
	pub parameters: std::collections::HashMap<String, serde_json::Value>,
	// Mark if this is a builtin layer (affects serialization and maintenance)
	#[serde(default)]
	pub builtin: bool,
	// Cached processed system prompt (not serialized - computed at session initialization)
	#[serde(skip)]
	pub processed_system_prompt: Option<String>,
}

fn default_temperature() -> f32 {
	0.2
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
			// Get servers from the global registry based on server_refs
			let mut layer_servers = Vec::new();

			for server_name in &self.mcp.server_refs {
				// Get from loaded registry
				let server_config = base_config
					.mcp
					.servers
					.iter()
					.find(|s| s.name == *server_name)
					.cloned();

				if let Some(mut server) = server_config {
					// Name is already set in the server config
					// Apply layer-specific tool filtering if specified
					if !self.mcp.allowed_tools.is_empty() {
						server.tools = self.mcp.allowed_tools.clone();
					}
					layer_servers.push(server);
				}
			}

			// Override the global MCP configuration with layer-specific servers
			merged_config.mcp = crate::config::McpConfig {
				servers: layer_servers,
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
			// Cache the fallback prompt as well
			let fallback = format!("You are a specialized AI layer named '{}'. Process the input according to your purpose.", self.name);
			self.processed_system_prompt = Some(fallback);
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

		// Replace custom parameter placeholders
		for (key, value) in &self.parameters {
			let placeholder = format!("%{{{}}}", key);
			let replacement = match value {
				serde_json::Value::String(s) => s.clone(),
				serde_json::Value::Number(n) => n.to_string(),
				serde_json::Value::Bool(b) => b.to_string(),
				_ => serde_json::to_string(value).unwrap_or_default(),
			};
			processed = processed.replace(&placeholder, &replacement);
		}

		processed
	}

	/// Create a default configuration for known system layer types
	pub fn create_system_layer(layer_type: &str) -> Self {
		match layer_type {
			"query_processor" => Self {
				name: layer_type.to_string(),
				model: Some("openrouter:openai/gpt-4.1-nano".to_string()),
				system_prompt: None, // Use built-in prompt
				temperature: 0.2,
				input_mode: InputMode::Last,
				output_mode: OutputMode::None, // Intermediate layer - doesn't modify session
				mcp: LayerMcpConfig {
					server_refs: vec![],
					allowed_tools: vec![],
				},
				parameters: std::collections::HashMap::new(),
				builtin: true,                 // Mark as builtin
				processed_system_prompt: None, // Will be processed during session initialization
			},
			"context_generator" => Self {
				name: layer_type.to_string(),
				model: Some("openrouter:google/gemini-2.5-flash-preview".to_string()),
				system_prompt: None, // Use built-in prompt
				temperature: 0.2,
				input_mode: InputMode::Last,
				output_mode: OutputMode::Replace, // Replaces input with processed context
				mcp: LayerMcpConfig {
					server_refs: vec!["developer".to_string(), "filesystem".to_string()],
					allowed_tools: vec!["text_editor".to_string(), "list_files".to_string()],
				},
				parameters: std::collections::HashMap::new(),
				builtin: true,                 // Mark as builtin
				processed_system_prompt: None, // Will be processed during session initialization
			},
			"reducer" => Self {
				name: layer_type.to_string(),
				model: Some("openrouter:openai/o4-mini".to_string()),
				system_prompt: None, // Use built-in prompt
				temperature: 0.2,
				input_mode: InputMode::All,
				output_mode: OutputMode::Replace, // Replaces entire session with reduced content
				mcp: LayerMcpConfig {
					server_refs: vec![],
					allowed_tools: vec![],
				},
				parameters: std::collections::HashMap::new(),
				builtin: true,                 // Mark as builtin
				processed_system_prompt: None, // Will be processed during session initialization
			},
			_ => Self {
				name: layer_type.to_string(),
				model: None,         // Use session model
				system_prompt: None, // Use generic prompt
				temperature: 0.2,
				input_mode: InputMode::Last,
				output_mode: OutputMode::None, // Default: intermediate layer
				mcp: LayerMcpConfig::default(),
				parameters: std::collections::HashMap::new(),
				builtin: false,                // Custom layers are not builtin
				processed_system_prompt: None, // Will be processed during session initialization
			},
		}
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
		operation_cancelled: Arc<AtomicBool>,
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
						.filter(|m| m.role == "assistant")
						.next_back()
						.map(|m| m.content.clone())
						.unwrap_or_else(|| {
							// Fallback: if no assistant messages, get last user message
							session
								.messages
								.iter()
								.filter(|m| m.role == "user")
								.next_back()
								.map(|m| m.content.clone())
								.unwrap_or_else(|| "No previous messages found".to_string())
						})
				} else {
					// If explicit input provided, use it but also include last assistant context
					let last_assistant = session
						.messages
						.iter()
						.filter(|m| m.role == "assistant")
						.next_back()
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
				// For "all" mode, we format the entire conversation context to include
				// the original user request and any relevant message history
				let mut context = String::new();

				// Add previous assistant messages if available for context
				let history = session
					.messages
					.iter()
					.filter(|m| m.role == "assistant")
					.map(|m| m.content.clone())
					.collect::<Vec<_>>();

				// Format as a structured prompt with original input and context
				if !history.is_empty() {
					context = format!(
						"Previous conversation context:\n{}\n\n",
						history.join("\n\n")
					);
				}

				format!("User request:\n{}\n\n{}", input, context)
			}
			InputMode::Summary => {
				// For summary mode, we generate a concise summary of the conversation
				// This helps maintain context while reducing token usage
				crate::session::summarize_context(session, input)
			}
		}
	}
}
