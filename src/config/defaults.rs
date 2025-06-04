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

//! Default configuration values and utilities for managing defaults
//! This module centralizes all default values and provides utilities
//! for determining if a value is using defaults vs custom settings.

use super::*;

/// Centralized default values for all configuration options
pub struct ConfigDefaults;

impl ConfigDefaults {
	// Root-level defaults
	pub const DEFAULT_LOG_LEVEL: LogLevel = LogLevel::None;
	pub const DEFAULT_MODEL: &'static str = "openrouter:anthropic/claude-3.5-haiku";
	pub const DEFAULT_MCP_RESPONSE_WARNING_THRESHOLD: usize = 20000;
	pub const DEFAULT_MAX_REQUEST_TOKENS_THRESHOLD: usize = 50000;
	pub const DEFAULT_ENABLE_AUTO_TRUNCATION: bool = false;
	pub const DEFAULT_CACHE_TOKENS_THRESHOLD: u64 = 2048;
	pub const DEFAULT_CACHE_TIMEOUT_SECONDS: u64 = 240;
	pub const DEFAULT_ENABLE_MARKDOWN_RENDERING: bool = false;
	pub const DEFAULT_MARKDOWN_THEME: &'static str = "default";
	pub const DEFAULT_MAX_SESSION_SPENDING_THRESHOLD: f64 = 0.0;

	// Role-specific defaults (no longer include models - use system-wide model)
	pub const DEFAULT_ENABLE_LAYERS: bool = false;

	// MCP defaults
	pub const DEFAULT_DEVELOPER_SERVER_REFS: &'static [&'static str] =
		&["developer", "filesystem", "octocode"];
	pub const DEFAULT_ASSISTANT_SERVER_REFS: &'static [&'static str] = &["filesystem"];
	pub const DEFAULT_MCP_SERVER_TIMEOUT: u64 = 30;

	// Layer defaults
	pub const DEFAULT_DEVELOPER_LAYER_REFS: &'static [&'static str] = &[];
	pub const DEFAULT_ASSISTANT_LAYER_REFS: &'static [&'static str] = &[];

	// System prompt defaults - now explicit in config template
	pub const DEFAULT_DEVELOPER_SYSTEM: &'static str = "You are an Octomind – top notch fully autonomous AI developer.\nCurrent working dir: %{CWD}\n**DEVELOPMENT APPROACH:**\n1. Analyze problems thoroughly first\n2. Think through solutions step-by-step\n3. Execute necessary changes directly using available tools\n4. Test your implementations when possible\n\n**CODE QUALITY GUIDELINES:**\n• Provide validated, working solutions\n• Keep code clear and concise\n• Focus on practical solutions and industry best practices\n• Avoid unnecessary abstractions - solve problems directly\n• Balance file size and readability\n• Don't over-fragment code across multiple files\n\n**MISSING CONTEXT COLLECTION CHECKLIST:**\n1. Examine key project files to understand the codebase structure \n2. Use text_editor view to examine files and understand interfaces and code signatures \n2. If needed, use list_files to find relevant implementation patterns \n3. As a last resort, use text_editor to view specific file contents \n**WHEN WORKING WITH FILES:**\n1. First understand which files you need to read/write\n2. Process files efficiently, preferably in a single operation\n3. Utilize the provided tools proactively without asking if you should use them\n\n%{SYSTEM}\n\n%{CONTEXT}\n\nIMPORTANT:\n- Right now you are *NOT* in the chat only mode and have access to tool use and system.\n- Please follow the task provided and make sure you do only changes required by the task, if you found something outside of task scope, you can mention it and ask.\n- Make sure when you refactor code or do changes, you do not remove critical parts of the codebase.";
	pub const DEFAULT_ASSISTANT_SYSTEM: &'static str = "You are a helpful assistant.";

	/// Check if a value matches the default
	pub fn is_default_log_level(value: &LogLevel) -> bool {
		matches!(value, LogLevel::None)
	}

	pub fn is_default_model(value: &str) -> bool {
		value == Self::DEFAULT_MODEL
	}


	pub fn is_default_mcp_response_warning_threshold(value: usize) -> bool {
		value == Self::DEFAULT_MCP_RESPONSE_WARNING_THRESHOLD
	}

	pub fn is_default_max_request_tokens_threshold(value: usize) -> bool {
		value == Self::DEFAULT_MAX_REQUEST_TOKENS_THRESHOLD
	}

	pub fn is_default_enable_auto_truncation(value: bool) -> bool {
		value == Self::DEFAULT_ENABLE_AUTO_TRUNCATION
	}

	pub fn is_default_cache_tokens_threshold(value: u64) -> bool {
		value == Self::DEFAULT_CACHE_TOKENS_THRESHOLD
	}

	pub fn is_default_cache_timeout_seconds(value: u64) -> bool {
		value == Self::DEFAULT_CACHE_TIMEOUT_SECONDS
	}

	pub fn is_default_enable_markdown_rendering(value: bool) -> bool {
		value == Self::DEFAULT_ENABLE_MARKDOWN_RENDERING
	}

	pub fn is_default_markdown_theme(value: &str) -> bool {
		value == Self::DEFAULT_MARKDOWN_THEME
	}

	pub fn is_default_max_session_spending_threshold(value: f64) -> bool {
		value == Self::DEFAULT_MAX_SESSION_SPENDING_THRESHOLD
	}

	pub fn is_default_enable_layers(value: bool) -> bool {
		value == Self::DEFAULT_ENABLE_LAYERS
	}

	/// Get the default system prompt for a role
	pub fn get_default_system_prompt(role: &str) -> String {
		match role {
			"developer" => Self::DEFAULT_DEVELOPER_SYSTEM.to_string(),
			"assistant" => Self::DEFAULT_ASSISTANT_SYSTEM.to_string(),
			_ => Self::DEFAULT_DEVELOPER_SYSTEM.to_string(), // Default to developer prompt for unknown roles
		}
	}

	/// Check if a system prompt is using the default
	pub fn is_default_system_prompt(role: &str, prompt: &Option<String>) -> bool {
		match prompt {
			None => true, // None means using default
			Some(p) => p == &Self::get_default_system_prompt(role),
		}
	}

	pub fn is_default_developer_server_refs(value: &[String]) -> bool {
		let default_refs: Vec<String> = Self::DEFAULT_DEVELOPER_SERVER_REFS
			.iter()
			.map(|s| s.to_string())
			.collect();
		value == default_refs
	}

	pub fn is_default_assistant_server_refs(value: &[String]) -> bool {
		let default_refs: Vec<String> = Self::DEFAULT_ASSISTANT_SERVER_REFS
			.iter()
			.map(|s| s.to_string())
			.collect();
		value == default_refs
	}

	pub fn is_default_developer_layer_refs(value: &[String]) -> bool {
		let default_refs: Vec<String> = Self::DEFAULT_DEVELOPER_LAYER_REFS
			.iter()
			.map(|s| s.to_string())
			.collect();
		value == default_refs
	}

	pub fn is_default_assistant_layer_refs(value: &[String]) -> bool {
		let default_refs: Vec<String> = Self::DEFAULT_ASSISTANT_LAYER_REFS
			.iter()
			.map(|s| s.to_string())
			.collect();
		value == default_refs
	}

	/// Get a complete default configuration
	pub fn create_default_config() -> Config {
		Config {
			version: crate::config::CURRENT_CONFIG_VERSION,
			log_level: Self::DEFAULT_LOG_LEVEL,
			model: Self::DEFAULT_MODEL.to_string(),
			mcp_response_warning_threshold: Self::DEFAULT_MCP_RESPONSE_WARNING_THRESHOLD,
			max_request_tokens_threshold: Self::DEFAULT_MAX_REQUEST_TOKENS_THRESHOLD,
			enable_auto_truncation: Self::DEFAULT_ENABLE_AUTO_TRUNCATION,
			cache_tokens_threshold: Self::DEFAULT_CACHE_TOKENS_THRESHOLD,
			cache_timeout_seconds: Self::DEFAULT_CACHE_TIMEOUT_SECONDS,
			enable_markdown_rendering: Self::DEFAULT_ENABLE_MARKDOWN_RENDERING,
			markdown_theme: Self::DEFAULT_MARKDOWN_THEME.to_string(),
			max_session_spending_threshold: Self::DEFAULT_MAX_SESSION_SPENDING_THRESHOLD,
			// REMOVED: providers section - API keys only from ENV variables
			developer: DeveloperRoleConfig {
				config: ModeConfig {
					enable_layers: Self::DEFAULT_ENABLE_LAYERS,
					system: None,
				},
				mcp: RoleMcpConfig {
					server_refs: Self::DEFAULT_DEVELOPER_SERVER_REFS
						.iter()
						.map(|s| s.to_string())
						.collect(),
					allowed_tools: Vec::new(),
				},
				layer_refs: Self::DEFAULT_DEVELOPER_LAYER_REFS
					.iter()
					.map(|s| s.to_string())
					.collect(),
			},
			assistant: AssistantRoleConfig {
				config: ModeConfig {
					enable_layers: Self::DEFAULT_ENABLE_LAYERS,
					system: None,
				},
				mcp: RoleMcpConfig {
					server_refs: Self::DEFAULT_ASSISTANT_SERVER_REFS
						.iter()
						.map(|s| s.to_string())
						.collect(),
					allowed_tools: Vec::new(),
				},
				layer_refs: Self::DEFAULT_ASSISTANT_LAYER_REFS
					.iter()
					.map(|s| s.to_string())
					.collect(),
			},
			mcp: McpConfig {
				servers: Vec::new(),
				allowed_tools: Vec::new(),
			},
			commands: None,
			layers: None,
			system: None,
			config_path: None,
		}
	}
}

/// Extension trait for Config to provide default-checking methods
pub trait ConfigDefaultsExt {
	/// Check if the current configuration uses default values
	fn is_using_defaults(&self) -> bool;

	/// Get a list of fields that are using non-default values
	fn get_customized_fields(&self) -> Vec<String>;

	/// Reset a specific field to its default value
	fn reset_to_default(&mut self, field_name: &str) -> Result<(), anyhow::Error>;

	/// Get the default value for a specific field as a string
	fn get_default_value_string(&self, field_name: &str) -> Option<String>;
}

impl ConfigDefaultsExt for Config {
	fn is_using_defaults(&self) -> bool {
		ConfigDefaults::is_default_log_level(&self.log_level)
			&& ConfigDefaults::is_default_model(&self.model)
			&& ConfigDefaults::is_default_mcp_response_warning_threshold(
				self.mcp_response_warning_threshold,
			) && ConfigDefaults::is_default_max_request_tokens_threshold(
			self.max_request_tokens_threshold,
		) && ConfigDefaults::is_default_enable_auto_truncation(self.enable_auto_truncation)
			&& ConfigDefaults::is_default_cache_tokens_threshold(self.cache_tokens_threshold)
			&& ConfigDefaults::is_default_cache_timeout_seconds(self.cache_timeout_seconds)
			&& ConfigDefaults::is_default_enable_markdown_rendering(self.enable_markdown_rendering)
			&& ConfigDefaults::is_default_markdown_theme(&self.markdown_theme)
			&& ConfigDefaults::is_default_max_session_spending_threshold(
				self.max_session_spending_threshold,
			) && ConfigDefaults::is_default_enable_layers(self.developer.config.enable_layers)
			&& ConfigDefaults::is_default_enable_layers(self.assistant.config.enable_layers)
			&& ConfigDefaults::is_default_developer_server_refs(&self.developer.mcp.server_refs)
			&& ConfigDefaults::is_default_assistant_server_refs(&self.assistant.mcp.server_refs)
			&& self.developer.config.system.is_none()
			&& self.assistant.config.system.is_none()
			&& self.layers.is_none()
			&& self.commands.is_none()
			&& self.system.is_none()
	}

	fn get_customized_fields(&self) -> Vec<String> {
		let mut customized = Vec::new();

		if !ConfigDefaults::is_default_log_level(&self.log_level) {
			customized.push("log_level".to_string());
		}
		if !ConfigDefaults::is_default_model(&self.model) {
			customized.push("model".to_string());
		}
		if !ConfigDefaults::is_default_mcp_response_warning_threshold(
			self.mcp_response_warning_threshold,
		) {
			customized.push("mcp_response_warning_threshold".to_string());
		}
		if !ConfigDefaults::is_default_max_request_tokens_threshold(
			self.max_request_tokens_threshold,
		) {
			customized.push("max_request_tokens_threshold".to_string());
		}
		if !ConfigDefaults::is_default_enable_auto_truncation(self.enable_auto_truncation) {
			customized.push("enable_auto_truncation".to_string());
		}
		if !ConfigDefaults::is_default_cache_tokens_threshold(self.cache_tokens_threshold) {
			customized.push("cache_tokens_threshold".to_string());
		}
		if !ConfigDefaults::is_default_cache_timeout_seconds(self.cache_timeout_seconds) {
			customized.push("cache_timeout_seconds".to_string());
		}
		if !ConfigDefaults::is_default_enable_markdown_rendering(self.enable_markdown_rendering) {
			customized.push("enable_markdown_rendering".to_string());
		}
		if !ConfigDefaults::is_default_markdown_theme(&self.markdown_theme) {
			customized.push("markdown_theme".to_string());
		}
		if !ConfigDefaults::is_default_max_session_spending_threshold(
			self.max_session_spending_threshold,
		) {
			customized.push("max_session_spending_threshold".to_string());
		}
		if !ConfigDefaults::is_default_enable_layers(self.developer.config.enable_layers) {
			customized.push("developer.enable_layers".to_string());
		}
		if !ConfigDefaults::is_default_enable_layers(self.assistant.config.enable_layers) {
			customized.push("assistant.enable_layers".to_string());
		}
		if !ConfigDefaults::is_default_developer_server_refs(&self.developer.mcp.server_refs) {
			customized.push("developer.mcp.server_refs".to_string());
		}
		if !ConfigDefaults::is_default_assistant_server_refs(&self.assistant.mcp.server_refs) {
			customized.push("assistant.mcp.server_refs".to_string());
		}
		if self.developer.config.system.is_some() {
			customized.push("developer.system".to_string());
		}
		if self.assistant.config.system.is_some() {
			customized.push("assistant.system".to_string());
		}
		if self.layers.is_some() {
			customized.push("layers".to_string());
		}
		if self.commands.is_some() {
			customized.push("commands".to_string());
		}
		if self.system.is_some() {
			customized.push("system".to_string());
		}

		customized
	}

	fn reset_to_default(&mut self, field_name: &str) -> Result<(), anyhow::Error> {
		match field_name {
			"log_level" => self.log_level = ConfigDefaults::DEFAULT_LOG_LEVEL,
			"model" => self.model = ConfigDefaults::DEFAULT_MODEL.to_string(),
			"mcp_response_warning_threshold" => {
				self.mcp_response_warning_threshold =
					ConfigDefaults::DEFAULT_MCP_RESPONSE_WARNING_THRESHOLD
			}
			"max_request_tokens_threshold" => {
				self.max_request_tokens_threshold =
					ConfigDefaults::DEFAULT_MAX_REQUEST_TOKENS_THRESHOLD
			}
			"enable_auto_truncation" => {
				self.enable_auto_truncation = ConfigDefaults::DEFAULT_ENABLE_AUTO_TRUNCATION
			}
			"cache_tokens_threshold" => {
				self.cache_tokens_threshold = ConfigDefaults::DEFAULT_CACHE_TOKENS_THRESHOLD
			}
			"cache_timeout_seconds" => {
				self.cache_timeout_seconds = ConfigDefaults::DEFAULT_CACHE_TIMEOUT_SECONDS
			}
			"enable_markdown_rendering" => {
				self.enable_markdown_rendering = ConfigDefaults::DEFAULT_ENABLE_MARKDOWN_RENDERING
			}
			"markdown_theme" => {
				self.markdown_theme = ConfigDefaults::DEFAULT_MARKDOWN_THEME.to_string()
			}
			"max_session_spending_threshold" => {
				self.max_session_spending_threshold =
					ConfigDefaults::DEFAULT_MAX_SESSION_SPENDING_THRESHOLD
			}
			"developer.enable_layers" => {
				self.developer.config.enable_layers = ConfigDefaults::DEFAULT_ENABLE_LAYERS
			}
			"assistant.enable_layers" => {
				self.assistant.config.enable_layers = ConfigDefaults::DEFAULT_ENABLE_LAYERS
			}
			"developer.mcp.server_refs" => {
				self.developer.mcp.server_refs = ConfigDefaults::DEFAULT_DEVELOPER_SERVER_REFS
					.iter()
					.map(|s| s.to_string())
					.collect()
			}
			"assistant.mcp.server_refs" => {
				self.assistant.mcp.server_refs = ConfigDefaults::DEFAULT_ASSISTANT_SERVER_REFS
					.iter()
					.map(|s| s.to_string())
					.collect()
			}
			"developer.system" => self.developer.config.system = None,
			"assistant.system" => self.assistant.config.system = None,
			"layers" => self.layers = None,
			"commands" => self.commands = None,
			"system" => self.system = None,
			_ => {
				return Err(anyhow::anyhow!("Unknown field name: {}", field_name));
			}
		}
		Ok(())
	}

	fn get_default_value_string(&self, field_name: &str) -> Option<String> {
		match field_name {
			"log_level" => Some(format!("{:?}", ConfigDefaults::DEFAULT_LOG_LEVEL)),
			"model" => Some(ConfigDefaults::DEFAULT_MODEL.to_string()),
			"mcp_response_warning_threshold" => {
				Some(ConfigDefaults::DEFAULT_MCP_RESPONSE_WARNING_THRESHOLD.to_string())
			}
			"max_request_tokens_threshold" => {
				Some(ConfigDefaults::DEFAULT_MAX_REQUEST_TOKENS_THRESHOLD.to_string())
			}
			"enable_auto_truncation" => {
				Some(ConfigDefaults::DEFAULT_ENABLE_AUTO_TRUNCATION.to_string())
			}
			"cache_tokens_threshold" => {
				Some(ConfigDefaults::DEFAULT_CACHE_TOKENS_THRESHOLD.to_string())
			}
			"cache_timeout_seconds" => {
				Some(ConfigDefaults::DEFAULT_CACHE_TIMEOUT_SECONDS.to_string())
			}
			"enable_markdown_rendering" => {
				Some(ConfigDefaults::DEFAULT_ENABLE_MARKDOWN_RENDERING.to_string())
			}
			"markdown_theme" => Some(ConfigDefaults::DEFAULT_MARKDOWN_THEME.to_string()),
			"max_session_spending_threshold" => {
				Some(ConfigDefaults::DEFAULT_MAX_SESSION_SPENDING_THRESHOLD.to_string())
			}
			"developer.enable_layers" | "assistant.enable_layers" => {
				Some(ConfigDefaults::DEFAULT_ENABLE_LAYERS.to_string())
			}
			"developer.mcp.server_refs" => {
				Some(ConfigDefaults::DEFAULT_DEVELOPER_SERVER_REFS.join(", "))
			}
			"assistant.mcp.server_refs" => {
				Some(ConfigDefaults::DEFAULT_ASSISTANT_SERVER_REFS.join(", "))
			}
			"developer.system" | "assistant.system" | "layers" | "commands" | "system" => {
				Some("None".to_string())
			}
			_ => None,
		}
	}
}
