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
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;

// Re-export all modules
pub mod layers;
pub mod loading;
pub mod mcp;
pub mod migrations;
pub mod providers;
pub mod roles;
pub mod validation;

// Tests removed - strict configuration mode doesn't support Default implementations
// Tests should be rewritten to use complete config structures

// Re-export commonly used types
pub use layers::*;
pub use mcp::*;
pub use providers::*;
pub use roles::*;

// Agent configuration
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct AgentConfig {
	// Layer name to route to (must match a layer name in config)
	pub name: String,

	// Tool description for this specific agent
	pub description: String,
}

// Current config version - increment when making breaking changes
pub const CURRENT_CONFIG_VERSION: u32 = 1;

// Type alias to simplify the complex return type for get_mode_config
type ModeConfigResult<'a> = (
	&'a ModeConfig,
	&'a RoleMcpConfig,
	Option<&'a Vec<crate::session::layers::LayerConfig>>,
	Option<&'a Vec<crate::session::layers::LayerConfig>>,
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

// REMOVED: Default implementation - LogLevel must be explicitly set in config

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

// REMOVED: All default functions - config must be complete and explicit

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Config {
	// Config version for future migrations (always first field)
	pub version: u32,

	// Root-level log level setting (takes precedence over role-specific)
	pub log_level: LogLevel,

	// Root-level model setting (used by all commands if specified)
	pub model: String,

	// System-wide configuration settings (not role-specific)
	pub mcp_response_warning_threshold: usize,
	pub max_request_tokens_threshold: usize,
	pub enable_auto_truncation: bool,
	pub cache_tokens_threshold: u64,
	pub cache_timeout_seconds: u64,
	pub enable_markdown_rendering: bool,
	// Markdown theme for styling
	pub markdown_theme: String,
	// Session spending threshold in USD - if > 0, prompt user when exceeded
	pub max_session_spending_threshold: f64,

	// Use long-term (1h) caching for system messages (strict: must be in config)
	pub use_long_system_cache: bool,

	// Agent configurations - array of agent definitions
	#[serde(default)]
	pub agents: Vec<AgentConfig>,

	// REMOVED: Providers configuration - API keys now only from ENV variables for security

	// Role configurations - array format like layers
	pub roles: Vec<crate::config::roles::Role>,

	// Internal role lookup map (populated during loading)
	#[serde(skip)]
	pub role_map: HashMap<String, crate::config::roles::Role>,

	// Global MCP configuration (fallback for roles)
	#[serde(skip_serializing_if = "McpConfig::is_default_for_serialization")]
	pub mcp: McpConfig,

	// Global command configurations (fallback for roles) - array format consistent with layers
	pub commands: Option<Vec<crate::session::layers::LayerConfig>>,

	// Global layer configurations - array of layer definitions
	pub layers: Option<Vec<crate::session::layers::LayerConfig>>,

	// Legacy system prompt field for backward compatibility
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
	/// Now relies entirely on config - no more runtime injection
	pub fn get_all_servers(&self) -> Vec<McpServerConfig> {
		let mut result = Vec::new();

		// Add servers from loaded registry
		for server_config in &self.servers {
			let server = server_config.clone();
			// Name is already set in the server config
			result.push(server);
		}

		result
	}

	/// Create a config using server configurations
	pub fn with_servers(
		servers: std::collections::HashMap<String, McpServerConfig>,
		allowed_tools: Option<Vec<String>>,
	) -> Self {
		// Convert HashMap to Vec, setting names from keys
		let servers_vec: Vec<McpServerConfig> = servers
			.into_iter()
			.map(|(name, mut server)| {
				server.name = name;
				server
			})
			.collect();

		Self {
			servers: servers_vec,
			allowed_tools: allowed_tools.unwrap_or_default(),
		}
	}
}

impl Config {
	/// Get the effective model to use - uses root config model (now always required)
	pub fn get_effective_model(&self) -> String {
		// Model is now always required in config, no fallback needed
		self.model.clone()
	}

	/// Get server configuration by name from the config registry
	/// Now relies entirely on config - no more runtime injection
	pub fn get_server_config(&self, server_name: &str) -> Option<McpServerConfig> {
		// Get from loaded registry
		self.mcp
			.servers
			.iter()
			.find(|s| s.name == server_name)
			.cloned()
	}

	/// Get enabled layers for a role with layer references
	/// This ensures layers are filtered by role layer_refs
	pub fn get_enabled_layers_for_role(
		&self,
		role: &str,
	) -> Vec<crate::session::layers::LayerConfig> {
		self.get_enabled_layers(role)
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
	pub fn get_model(&self, _role: &str) -> String {
		// All roles now use the system-wide model
		self.get_effective_model()
	}

	/// Get configuration for a specific role
	/// Returns: (mode_config, role_mcp_config, layers, commands, system_prompt)
	pub fn get_mode_config(&self, role: &str) -> ModeConfigResult<'_> {
		if let Some(role_config) = self.role_map.get(role) {
			(
				&role_config.config,
				&role_config.mcp,
				self.layers.as_ref(),
				self.commands.as_ref(),
				role_config.config.system.as_ref(),
			)
		} else {
			// Unknown role - create minimal fallback
			static DEFAULT_MODE_CONFIG: ModeConfig = ModeConfig {
				enable_layers: false,
				system: None,
				temperature: 0.7, // Fallback temperature for unknown roles
			};
			static DEFAULT_MCP_CONFIG: RoleMcpConfig = RoleMcpConfig {
				server_refs: Vec::new(),
				allowed_tools: Vec::new(),
			};
			(
				&DEFAULT_MODE_CONFIG,
				&DEFAULT_MCP_CONFIG,
				self.layers.as_ref(),
				self.commands.as_ref(),
				None,
			)
		}
	}

	/// Get a merged config for a specific mode (for backward compatibility)
	/// This creates a new Config with role-specific settings merged into system-wide settings
	pub fn get_merged_config_for_mode(&self, mode: &str) -> Config {
		let (_mode_config, role_mcp_config, _role_layers_config, commands, system_prompt) =
			self.get_mode_config(mode);

		let mut merged = self.clone();

		// CRITICAL FIX: Create a legacy McpConfig for backward compatibility with existing code
		// Use the new runtime injection method to ensure core servers are ALWAYS available
		let enabled_servers = self.get_enabled_servers_for_role(role_mcp_config);

		crate::log_debug!(
			"TRACE: Role '{}' server_refs: {:?}",
			mode,
			role_mcp_config.server_refs
		);
		crate::log_debug!(
			"TRACE: Found {} enabled servers for role",
			enabled_servers.len()
		);

		for server in &enabled_servers {
			crate::log_debug!("TRACE: Adding server '{}' to merged config", server.name);
		}

		merged.mcp = McpConfig {
			servers: enabled_servers, // Only role-enabled servers (with runtime injection)
			allowed_tools: role_mcp_config.allowed_tools.clone(),
		};

		// Role-specific layers (only enabled via layer_refs) - NOT USED ANYWHERE
		// Keep merged.layers as original registry for agent tools
		// let enabled_layers = self.get_enabled_layers_for_role(mode);

		merged.commands = commands.cloned();
		merged.system = system_prompt.cloned();

		merged
	}

	/// Get the mode config struct for a specific role
	pub fn get_mode_config_struct(&self, role: &str) -> &ModeConfig {
		let (mode_config, _, _, _, _) = self.get_mode_config(role);
		mode_config
	}

	/// Get layer references for a specific role
	pub fn get_layer_refs(&self, role: &str) -> &Vec<String> {
		if let Some(role_config) = self.role_map.get(role) {
			&role_config.layer_refs
		} else {
			// Return empty vec for unknown roles
			static EMPTY_VEC: Vec<String> = Vec::new();
			&EMPTY_VEC
		}
	}

	/// Get enabled layers for a specific role (filters global layers by role layer_refs)
	pub fn get_enabled_layers(&self, role: &str) -> Vec<crate::session::layers::LayerConfig> {
		let layer_refs = self.get_layer_refs(role);
		if layer_refs.is_empty() {
			return Vec::new();
		}

		let mut result = Vec::new();
		for layer_name in layer_refs {
			// Get from global layers registry
			let layer_config = if let Some(all_layers) = &self.layers {
				all_layers
					.iter()
					.find(|layer| layer.name == *layer_name)
					.cloned()
			} else {
				None
			};

			if let Some(mut layer) = layer_config {
				// Auto-set the name from the registry key
				layer.name = layer_name.clone();
				result.push(layer);
			} else {
				crate::log_debug!(
					"Layer '{}' referenced by role '{}' but not found in global registry",
					layer_name,
					role
				);
			}
		}

		result
	}

	/// Build the internal role map from the roles array for fast lookup
	pub fn build_role_map(&mut self) {
		self.role_map.clear();
		for role in &self.roles {
			self.role_map.insert(role.name.clone(), role.clone());
		}
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
