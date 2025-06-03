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

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::cell::RefCell;

// Re-export all modules
pub mod loading;
pub mod mcp;
pub mod providers;
pub mod roles;
pub mod validation;

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_mcp_config_serialization_skipping() {
		// Test that empty MCP config is skipped during serialization
		let config = Config {
			log_level: LogLevel::Info,
			mcp: McpConfig::default(), // Should be skipped
			..Default::default()
		};

		let clean_config = config.create_clean_copy_for_saving();
		let toml_str = toml::to_string(&clean_config).unwrap();

		// The [mcp] section should not appear in the serialized TOML
		assert!(
			!toml_str.contains("[mcp]"),
			"Empty MCP config should be skipped, but TOML contains: {}",
			toml_str
		);
		assert!(
			toml_str.contains("log_level = \"info\""),
			"Other fields should still be serialized"
		);
	}

	#[test]
	fn test_mcp_config_serialization_not_skipped_with_servers() {
		// Test that MCP config with servers is NOT skipped
		let mut servers = std::collections::HashMap::new();
		servers.insert("test_server".to_string(), McpServerConfig::default());

		let config = Config {
			log_level: LogLevel::Info,
			mcp: McpConfig {
				servers,
				..Default::default()
			},
			..Default::default()
		};

		let clean_config = config.create_clean_copy_for_saving();
		let toml_str = toml::to_string(&clean_config).unwrap();

		// The [mcp] section SHOULD appear because there are servers
		assert!(
			toml_str.contains("[mcp]"),
			"MCP config with servers should NOT be skipped, but TOML: {}",
			toml_str
		);
	}

	#[test]
	fn test_invalid_openrouter_models() {
		let mut config = Config::default();

		// Test invalid models (old format without provider prefix)
		let invalid_models = [
			"gpt-4",                       // Missing provider prefix
			"anthropic/claude-3.5-sonnet", // Old format
			"openai-gpt-4",                // Wrong separator
			"unknown:model",               // Unknown provider
			"",                            // Empty string
			"openai:",                     // Empty model
			":gpt-4o",                     // Empty provider
		];

		for model in invalid_models {
			config.openrouter.model = model.to_string();
			assert!(
				config.validate_openrouter_model().is_err(),
				"Model {} should be invalid",
				model
			);
		}
	}

	#[test]
	fn test_threshold_validation() {
		// Test valid thresholds with auto-truncation enabled
		let config = Config {
			mcp_response_warning_threshold: 0, // Valid for disabling
			cache_tokens_threshold: 2048,
			max_request_tokens_threshold: 50000, // Must be > 0 when auto-truncation enabled
			enable_auto_truncation: true,
			..Default::default()
		};
		assert!(config.validate_thresholds().is_ok());

		// Test valid thresholds with auto-truncation disabled (0 should be allowed)
		let config = Config {
			mcp_response_warning_threshold: 1000,
			cache_tokens_threshold: 5000,
			max_request_tokens_threshold: 0, // Should be valid when auto-truncation disabled
			enable_auto_truncation: false,
			..Default::default()
		};
		assert!(config.validate_thresholds().is_ok());

		// Test invalid: auto-truncation enabled but threshold is 0
		let config = Config {
			mcp_response_warning_threshold: 1000,
			cache_tokens_threshold: 5000,
			max_request_tokens_threshold: 0, // Invalid when auto-truncation enabled
			enable_auto_truncation: true,
			..Default::default()
		};
		assert!(config.validate_thresholds().is_err());
	}

	#[test]
	fn test_environment_variable_precedence() {
		// This test would need to be run with specific environment variables set
		// For now, just test that the load function doesn't panic
		// Note: This may fail if there's no valid config file, which is expected
		let result = Config::load();
		// Don't assert success since config file may not exist in test environment
		match result {
			Ok(_) => println!("Config loaded successfully"),
			Err(e) => println!("Config load failed (expected in test): {}", e),
		}
	}

	#[test]
	fn test_role_specific_cache_config() {
		let config = Config {
			cache_tokens_threshold: 4096,
			cache_timeout_seconds: 300,
			openrouter: OpenRouterConfig {
				..Default::default()
			},
			..Default::default()
		};

		// Test developer role merged config - should use system-wide settings
		let developer_merged = config.get_merged_config_for_mode("developer");
		assert_eq!(developer_merged.cache_tokens_threshold, 4096);
		assert_eq!(developer_merged.cache_timeout_seconds, 300);

		// Test assistant role merged config - should also use system-wide settings
		let assistant_merged = config.get_merged_config_for_mode("assistant");
		assert_eq!(assistant_merged.cache_tokens_threshold, 4096);
		assert_eq!(assistant_merged.cache_timeout_seconds, 300);

		// Test unknown role falls back to assistant but still uses system-wide settings
		let unknown_merged = config.get_merged_config_for_mode("unknown");
		assert_eq!(unknown_merged.cache_tokens_threshold, 4096);
		assert_eq!(unknown_merged.cache_timeout_seconds, 300);
	}
}

// Re-export commonly used types
pub use mcp::*;
pub use providers::*;
pub use roles::*;

// Type alias to simplify the complex return type for get_mode_config
type ModeConfigResult<'a> = (
	&'a ModeConfig,
	&'a RoleMcpConfig,
	Option<&'a Vec<crate::session::layers::LayerConfig>>,
	Option<&'a std::collections::HashMap<String, crate::session::layers::LayerConfig>>,
	Option<&'a String>,
);

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub enum LogLevel {
	#[serde(rename = "none")]
	None,
	#[serde(rename = "info")]
	Info,
	#[serde(rename = "debug")]
	Debug,
}

impl Default for LogLevel {
	fn default() -> Self {
		Self::None
	}
}

impl LogLevel {
	/// Check if info logging is enabled
	pub fn is_info_enabled(&self) -> bool {
		matches!(self, LogLevel::Info | LogLevel::Debug)
	}

	/// Check if debug logging is enabled
	pub fn is_debug_enabled(&self) -> bool {
		matches!(self, LogLevel::Debug)
	}
}

// Default functions
fn default_system_model() -> String {
	"openrouter:anthropic/claude-3.5-haiku".to_string()
}

fn default_mcp_response_warning_threshold() -> usize {
	20000 // Default threshold for warning about large MCP responses (20k tokens)
}

fn default_max_request_tokens_threshold() -> usize {
	50000 // Default threshold for auto-truncation (50k tokens)
}

fn default_cache_tokens_threshold() -> u64 {
	2048 // Default 2048 tokens threshold for automatic cache marker movement
}

fn default_cache_timeout_seconds() -> u64 {
	240 // Default 4 minutes timeout for time-based auto-caching
}

fn default_markdown_theme() -> String {
	"default".to_string()
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct Config {
	// Root-level log level setting (takes precedence over role-specific)
	#[serde(default)]
	pub log_level: LogLevel,

	// Root-level model setting (used by all commands if specified)
	#[serde(default = "default_system_model")]
	pub model: String,

	// System-wide configuration settings (not role-specific)
	#[serde(default = "default_mcp_response_warning_threshold")]
	pub mcp_response_warning_threshold: usize,
	#[serde(default = "default_max_request_tokens_threshold")]
	pub max_request_tokens_threshold: usize,
	#[serde(default)]
	pub enable_auto_truncation: bool,
	#[serde(default = "default_cache_tokens_threshold")]
	pub cache_tokens_threshold: u64,
	#[serde(default = "default_cache_timeout_seconds")]
	pub cache_timeout_seconds: u64,
	#[serde(default)]
	pub enable_markdown_rendering: bool,
	// Markdown theme for styling
	#[serde(default = "default_markdown_theme")]
	pub markdown_theme: String,

	// NEW: Providers configuration - centralized API keys
	#[serde(default)]
	pub providers: ProvidersConfig,

	// Role-specific configurations
	#[serde(default)]
	pub developer: DeveloperRoleConfig,
	#[serde(default)]
	pub assistant: AssistantRoleConfig,

	// Global MCP configuration (fallback for roles)
	#[serde(
		default,
		skip_serializing_if = "McpConfig::is_default_for_serialization"
	)]
	pub mcp: McpConfig,

	// Global command configurations (fallback for roles)
	#[serde(default)]
	pub commands: Option<std::collections::HashMap<String, crate::session::layers::LayerConfig>>,

	// Legacy fields for backward compatibility - REMOVED for new approach
	#[serde(default)]
	pub openrouter: OpenRouterConfig,
	#[serde(default)]
	pub layers: Option<Vec<crate::session::layers::LayerConfig>>,
	pub system: Option<String>,

	#[serde(skip)]
	config_path: Option<PathBuf>,
}

impl McpConfig {
	/// Check if this config should be skipped during serialization
	/// This helps avoid writing empty [mcp] sections when only internal servers exist
	pub fn is_default_for_serialization(&self) -> bool {
		self.servers.is_empty() && self.allowed_tools.is_empty()
	}

	/// Get all servers from the registry (for populating role configs)
	/// UPDATED to use runtime injection for core servers
	pub fn get_all_servers(&self) -> Vec<McpServerConfig> {
		let mut result = Vec::new();
		let mut added_servers = std::collections::HashSet::new();

		// Add servers from loaded registry
		for (server_name, server_config) in &self.servers {
			let mut server = server_config.clone();
			// Auto-set the name from the registry key
			server.name = server_name.clone();
			// Auto-detect server type from name
			server.server_type = match server_name.as_str() {
				"developer" => McpServerType::Developer,
				"filesystem" => McpServerType::Filesystem,
				_ => McpServerType::External,
			};
			result.push(server);
			added_servers.insert(server_name.clone());
		}

		// CRITICAL: Always add core servers if not already in registry
		// This ensures they're available even if config file is empty
		for core_server_name in ["developer", "filesystem", "octocode"] {
			if !added_servers.contains(core_server_name) {
				if let Some(core_server) =
					Config::get_core_server_config(core_server_name)
				{
					result.push(core_server);
				}
			}
		}

		result
	}

	/// Create a config using server configurations
	pub fn with_servers(
		servers: std::collections::HashMap<String, McpServerConfig>,
		allowed_tools: Option<Vec<String>>,
	) -> Self {
		Self {
			servers,
			allowed_tools: allowed_tools.unwrap_or_default(),
		}
	}
}

impl Config {

	/// Get the effective model to use - checks root config, then falls back to system default
	pub fn get_effective_model(&self) -> String {
		// If root-level model is set (not the default), use it
		if !self.model.is_empty() && self.model != default_system_model() {
			return self.model.clone();
		}

		// Otherwise, use the system default
		default_system_model()
	}

	/// Get server configuration by name, with runtime core server injection
	/// This method ALWAYS provides core servers regardless of config file state
	pub fn get_server_config(&self, server_name: &str) -> Option<McpServerConfig> {
		// First check loaded registry
		if let Some(server) = self.mcp.servers.get(server_name) {
			return Some(server.clone());
		}

		// CRITICAL: Always provide core servers, even if not in loaded config
		// This ensures MCP works consistently regardless of config file state
		Self::get_core_server_config(server_name)
	}

	/// Get core server configuration - these are always available
	/// This is separated from the config loading to ensure consistency
	pub fn get_core_server_config(server_name: &str) -> Option<McpServerConfig> {
		mcp::get_core_server_config(server_name)
	}

	/// Get enabled servers for a role with runtime core server injection
	/// This ensures core servers are ALWAYS available regardless of config file state
	pub fn get_enabled_servers_for_role(
		&self,
		role_mcp_config: &RoleMcpConfig,
	) -> Vec<McpServerConfig> {
		// Use the updated RoleMcpConfig method that has runtime injection
		role_mcp_config.get_enabled_servers(&self.mcp.servers)
	}
	/// Get the global log level (system-wide setting)
	pub fn get_log_level(&self) -> LogLevel {
		self.log_level.clone()
	}

	/// Role-based configuration getters - these delegate to role configs
	/// Get enable layers setting for the specified role
	pub fn get_enable_layers(&self, role: &str) -> bool {
		let (mode_config, _, _, _, _) = self.get_mode_config(role);
		mode_config.enable_layers
	}

	/// Get the model for the specified role
	pub fn get_model(&self, role: &str) -> String {
		let (mode_config, _, _, _, _) = self.get_mode_config(role);
		mode_config.get_full_model()
	}

	/// Get configuration for a specific role with proper fallback logic and role inheritance
	/// Returns: (mode_config, role_mcp_config, layers, commands, system_prompt)
	/// Role inheritance: any role inherits from 'assistant' first, then applies its own overrides
	pub fn get_mode_config(&self, role: &str) -> ModeConfigResult<'_> {
		match role {
			"developer" => {
				// Developer role - uses its own MCP config with server_refs
				(
					&self.developer.config,
					&self.developer.mcp,
					self.developer.layers.as_ref(),
					self.commands.as_ref(),
					self.developer.config.system.as_ref(),
				)
			}
			"assistant" => {
				// Base assistant role
				(
					&self.assistant.config,
					&self.assistant.mcp,
					None, // Assistant doesn't have layers
					self.commands.as_ref(),
					self.assistant.config.system.as_ref(),
				)
			}
			_ => {
				// Unknown role - fallback to assistant
				(
					&self.assistant.config,
					&self.assistant.mcp,
					None,
					self.commands.as_ref(),
					self.assistant.config.system.as_ref(),
				)
			}
		}
	}

	/// Get a merged config for a specific mode (for backward compatibility)
	/// This creates a new Config with role-specific settings merged into system-wide settings
	pub fn get_merged_config_for_mode(&self, mode: &str) -> Config {
		let (mode_config, role_mcp_config, layers, commands, system_prompt) =
			self.get_mode_config(mode);

		let mut merged = self.clone();

		// Create an OpenRouterConfig from the ModeConfig for backward compatibility
		merged.openrouter = OpenRouterConfig {
			model: mode_config.get_full_model(),
			api_key: mode_config.get_api_key(&self.providers),
			pricing: mode_config.get_pricing(&self.providers),
		};

		// CRITICAL FIX: Create a legacy McpConfig for backward compatibility with existing code
		// Use the new runtime injection method to ensure core servers are ALWAYS available
		let enabled_servers = self.get_enabled_servers_for_role(role_mcp_config);
		let mut legacy_servers = std::collections::HashMap::new();

		crate::log_debug!(
			"TRACE: Role '{}' server_refs: {:?}",
			mode,
			role_mcp_config.server_refs
		);
		crate::log_debug!(
			"TRACE: Found {} enabled servers for role",
			enabled_servers.len()
		);

		for server in enabled_servers {
			crate::log_debug!("TRACE: Adding server '{}' to merged config", server.name);
			legacy_servers.insert(server.name.clone(), server);
		}

		merged.mcp = McpConfig {
			servers: legacy_servers, // Only role-enabled servers (with runtime injection)
			allowed_tools: role_mcp_config.allowed_tools.clone(),
		};

		merged.layers = layers.cloned();
		merged.commands = commands.cloned();
		merged.system = system_prompt.cloned();

		merged
	}

	/// Get the mode config struct for a specific role
	pub fn get_mode_config_struct(&self, role: &str) -> &ModeConfig {
		let (mode_config, _, _, _, _) = self.get_mode_config(role);
		mode_config
	}
}

// Logging macros for different log levels
// These macros automatically check the current log level and only print if appropriate

thread_local! {
	static CURRENT_CONFIG: RefCell<Option<Config>> = const { RefCell::new(None) };
}

/// Set the current config for the thread (to be used by logging macros)
pub fn set_thread_config(config: &Config) {
	CURRENT_CONFIG.with(|c| {
		*c.borrow_mut() = Some(config.clone());
	});
}

/// Get the current config for the thread
pub fn with_thread_config<F, R>(f: F) -> Option<R>
where
	F: FnOnce(&Config) -> R,
{
	CURRENT_CONFIG.with(|c| (*c.borrow()).as_ref().map(f))
}

/// Info logging macro with automatic cyan coloring
/// Shows info messages when log level is Info OR Debug
#[macro_export]
macro_rules! log_info {
	($fmt:expr) => {
		if let Some(should_log) = $crate::config::with_thread_config(|config| config.get_log_level().is_info_enabled()) {
		if should_log {
		use colored::Colorize;
		println!("{}", $fmt.cyan());
		}
		}
	};
	($fmt:expr, $($arg:expr),*) => {
		if let Some(should_log) = $crate::config::with_thread_config(|config| config.get_log_level().is_info_enabled()) {
		if should_log {
		use colored::Colorize;
	println!("{}", format!($fmt, $($arg),*).cyan());
	}
	}
	};
}

/// Debug logging macro with automatic bright blue coloring
#[macro_export]
macro_rules! log_debug {
	($fmt:expr) => {
		if let Some(should_log) = $crate::config::with_thread_config(|config| config.get_log_level().is_debug_enabled()) {
		if should_log {
		use colored::Colorize;
		println!("{}", $fmt.bright_blue());
		}
		}
	};
	($fmt:expr, $($arg:expr),*) => {
		if let Some(should_log) = $crate::config::with_thread_config(|config| config.get_log_level().is_debug_enabled()) {
		if should_log {
		use colored::Colorize;
	println!("{}", format!($fmt, $($arg),*).bright_blue());
	}
	}
	};
}

/// Error logging macro with automatic bright red coloring
/// Always visible regardless of log level (errors should always be shown)
#[macro_export]
macro_rules! log_error {
	($fmt:expr) => {{
		use colored::Colorize;
		eprintln!("{}", $fmt.bright_red());
		}};
	($fmt:expr, $($arg:expr),*) => {{
		use colored::Colorize;
		eprintln!("{}", format!($fmt, $($arg),*).bright_red());
		}};
}

/// Conditional logging - prints different messages based on log level
#[macro_export]
macro_rules! log_conditional {
	(debug: $debug_msg:expr, info: $info_msg:expr, none: $none_msg:expr) => {
		if let Some(level) = $crate::config::with_thread_config(|config| config.get_log_level()) {
			match level {
				$crate::config::LogLevel::Debug => println!("{}", $debug_msg),
				$crate::config::LogLevel::Info => println!("{}", $info_msg),
				$crate::config::LogLevel::None => println!("{}", $none_msg),
			}
		} else {
			// Fallback if no config is set
			println!("{}", $none_msg);
		}
	};
	(debug: $debug_msg:expr, default: $default_msg:expr) => {
		if let Some(should_debug) =
			$crate::config::with_thread_config(|config| config.get_log_level().is_debug_enabled())
		{
			if should_debug {
				println!("{}", $debug_msg);
			} else {
				println!("{}", $default_msg);
			}
		} else {
			// Fallback if no config is set
			println!("{}", $default_msg);
		}
	};
	(info: $info_msg:expr, default: $default_msg:expr) => {
		if let Some(should_info) =
			$crate::config::with_thread_config(|config| config.get_log_level().is_info_enabled())
		{
			if should_info {
				println!("{}", $info_msg);
			} else {
				println!("{}", $default_msg);
			}
		} else {
			// Fallback if no config is set
			println!("{}", $default_msg);
		}
	};
}