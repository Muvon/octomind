use crate::config::Config;
use crate::session::{Session, ProviderExchange, TokenUsage};
use anyhow::Result;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

// Layer result that contains data returned from a layer's processing
pub struct LayerResult {
	pub output: String,
	pub exchange: ProviderExchange,
	pub token_usage: Option<TokenUsage>,
	pub tool_calls: Option<Vec<crate::mcp::McpToolCall>>,
	// Time tracking
	pub api_time_ms: u64,    // Time spent on API requests
	pub tool_time_ms: u64,   // Time spent executing tools
	pub total_time_ms: u64,  // Total processing time for this layer
}

// Input mode determines what part of the previous layer's output will be used
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputMode {
	Last,    // Only the last message from the previous layer
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
	type Err = ();

	fn from_str(s: &str) -> Result<Self, Self::Err> {
		match s.to_lowercase().as_str() {
			"last" => Ok(InputMode::Last),
			"all" => Ok(InputMode::All),
			"summary" => Ok(InputMode::Summary),
			_ => Err(()), // Return error for unknown values
		}
	}
}

// Configuration for layer-specific MCP settings
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LayerMcpConfig {
	// Server references - list of server names from the global registry to use for this layer
	// Empty list means MCP is disabled for this layer
	#[serde(default)]
	pub server_refs: Vec<String>,

	#[serde(default)]
	pub allowed_tools: Vec<String>, // Specific tools allowed (empty = all tools from enabled servers)
}

// Common configuration properties for all layers - extended for flexibility
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayerConfig {
	pub name: String,
	// Model is now optional - falls back to session model if not specified
	pub model: Option<String>,
	// System prompt is optional - uses built-in prompts for known layer types
	pub system_prompt: Option<String>,
	#[serde(default = "default_temperature")]
	pub temperature: f32,
	#[serde(default)]
	pub input_mode: InputMode,
	// MCP configuration for this layer
	#[serde(default)]
	pub mcp: LayerMcpConfig,
	// Custom parameters that can be used in system prompts via placeholders
	#[serde(default)]
	pub parameters: std::collections::HashMap<String, serde_json::Value>,
}

fn default_temperature() -> f32 {
	0.2
}

impl LayerConfig {
	/// Get the effective model for this layer (fallback to session model if not specified)
	pub fn get_effective_model(&self, session_model: &str) -> String {
		self.model.clone().unwrap_or_else(|| session_model.to_string())
	}

	/// Create a merged config that respects this layer's MCP settings
	/// This ensures that API calls use the layer's MCP configuration rather than just global settings
	pub fn get_merged_config_for_layer(&self, base_config: &crate::config::Config) -> crate::config::Config {
		let mut merged_config = base_config.clone();

		// Create role-like MCP config from layer's server_refs
		if !self.mcp.server_refs.is_empty() {
			// Get servers from the global registry based on server_refs
			let mut legacy_servers = std::collections::HashMap::new();

			for server_name in &self.mcp.server_refs {
				// Try to get from loaded registry first, then fallback to core servers
				let server_config = base_config.mcp.servers.get(server_name)
					.cloned()
					.or_else(|| crate::config::Config::get_core_server_config(server_name));

				if let Some(mut server) = server_config {
					// Auto-set the name from the registry key
					server.name = server_name.clone();
					// Auto-detect server type from name
					server.server_type = match server_name.as_str() {
						"developer" => crate::config::McpServerType::Developer,
						"filesystem" => crate::config::McpServerType::Filesystem,
						_ => crate::config::McpServerType::External,
					};
					// Apply layer-specific tool filtering if specified
					if !self.mcp.allowed_tools.is_empty() {
						server.tools = self.mcp.allowed_tools.clone();
					}
					legacy_servers.insert(server_name.clone(), server);
				}
			}

			// Override the global MCP configuration with layer-specific servers
			merged_config.mcp = crate::config::McpConfig {
				servers: legacy_servers,
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
	/// Uses custom prompt if provided, otherwise uses built-in prompt for known layer types
	pub fn get_effective_system_prompt(&self) -> String {
		if let Some(ref custom_prompt) = self.system_prompt {
			// Process placeholders in custom system prompt
			self.process_prompt_placeholders(custom_prompt)
		} else {
			// Use built-in prompt for known layer types
			match self.name.as_str() {
				"query_processor" => crate::session::helper_functions::get_raw_system_prompt("query_processor"),
				"context_generator" => crate::session::helper_functions::get_raw_system_prompt("context_generator"),
				"reducer" => crate::session::helper_functions::get_raw_system_prompt("reducer"),
				_ => {
					// For unknown layer types, use a generic prompt
					format!("You are a specialized AI layer named '{}'. Process the input according to your purpose.", self.name)
				}
			}
		}
	}

	/// Process placeholders in system prompt using layer parameters
	fn process_prompt_placeholders(&self, prompt: &str) -> String {
		let mut processed = prompt.to_string();

		// Replace standard placeholders
		if let Ok(project_dir) = std::env::current_dir() {
			processed = crate::session::process_placeholders(&processed, &project_dir);
		}

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
				mcp: LayerMcpConfig {
					server_refs: vec![],
					allowed_tools: vec![]
				},
				parameters: std::collections::HashMap::new(),
			},
			"context_generator" => Self {
				name: layer_type.to_string(),
				model: Some("openrouter:google/gemini-2.5-flash-preview".to_string()),
				system_prompt: None, // Use built-in prompt
				temperature: 0.2,
				input_mode: InputMode::Last,
				mcp: LayerMcpConfig {
					server_refs: vec!["developer".to_string(), "filesystem".to_string()],
					allowed_tools: vec!["text_editor".to_string(), "list_files".to_string()]
				},
				parameters: std::collections::HashMap::new(),
			},
			"reducer" => Self {
				name: layer_type.to_string(),
				model: Some("openrouter:openai/o4-mini".to_string()),
				system_prompt: None, // Use built-in prompt
				temperature: 0.2,
				input_mode: InputMode::All,
				mcp: LayerMcpConfig {
					server_refs: vec![],
					allowed_tools: vec![]
				},
				parameters: std::collections::HashMap::new(),
			},
			_ => Self {
				name: layer_type.to_string(),
				model: None, // Use session model
				system_prompt: None, // Use generic prompt
				temperature: 0.2,
				input_mode: InputMode::Last,
				mcp: LayerMcpConfig::default(),
				parameters: std::collections::HashMap::new(),
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
		operation_cancelled: Arc<AtomicBool>
	) -> Result<LayerResult>;

	// Helper function to prepare input based on input_mode
	fn prepare_input(&self, input: &str, session: &Session) -> String {
		// Each layer processes input in its own isolated context
		// The input mode determines what part of the previous context is used
		match self.config().input_mode {
			InputMode::Last => {
				// In Last mode, we just use the last message content as provided
				// This is the default and most common mode for layer-to-layer communication
				input.to_string()
			},
			InputMode::All => {
				// For "all" mode, we format the entire conversation context to include
				// the original user request and any relevant message history
				let mut context = String::new();

				// Add previous assistant messages if available for context
				let history = session.messages.iter()
					.filter(|m| m.role == "assistant")
					.map(|m| m.content.clone())
					.collect::<Vec<_>>();

				// Format as a structured prompt with original input and context
				if !history.is_empty() {
					context = format!("Previous conversation context:\n{}\n\n",
						history.join("\n\n"));
				}

				format!("User request:\n{}\n\n{}", input, context)
			},
			InputMode::Summary => {
				// For summary mode, we generate a concise summary of the conversation
				// This helps maintain context while reducing token usage
				crate::session::summarize_context(session, input)
			}
		}
	}
}
