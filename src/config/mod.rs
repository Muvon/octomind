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

use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

// Global environment tracker for source detection
static ENV_TRACKER: OnceLock<Mutex<env_source::EnvTracker>> = OnceLock::new();

/// Get global environment tracker instance
pub fn get_env_tracker() -> &'static Mutex<env_source::EnvTracker> {
	ENV_TRACKER.get_or_init(|| Mutex::new(env_source::EnvTracker::new()))
}
// Re-export all modules
pub mod agents;
pub mod env_source;

pub mod layers;

pub mod loading;

pub mod mcp;

pub mod migrations;

// OAuth 2.1 + PKCE configuration
pub mod oauth_config;

pub mod providers;

pub mod roles;

pub mod validation;

pub mod registry;

pub mod workflows;

// Tests removed - strict configuration mode doesn't support Default implementations
// Tests should be rewritten to use complete config structures

// Re-export commonly used types
pub use layers::*;
pub use mcp::*;
pub use oauth_config::*; // OAuth 2.1 + PKCE configuration types
pub use providers::*;
pub use registry::*;
pub use roles::*;
pub use workflows::*;

// Agent configuration - removed, now uses LayerConfig directly

// Current config version - increment when making breaking changes
pub const CURRENT_CONFIG_VERSION: u32 = 1;

// Type alias to simplify the complex return type for get_role_config
type RoleConfigResult<'a> = (
	&'a RoleConfig,
	&'a RoleMcpConfig,
	Option<&'a Vec<crate::session::layers::LayerConfig>>,
	Option<&'a Vec<crate::session::layers::LayerConfig>>,
	&'a String,
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

	/// Get string representation for tracing
	pub fn as_str(&self) -> &'static str {
		match self {
			LogLevel::None => "off",
			LogLevel::Info => "info",
			LogLevel::Debug => "debug",
		}
	}
}

// REMOVED: All default functions - config must be complete and explicit

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PressureLevel {
	/// Absolute token threshold at which this level applies
	pub threshold: usize,
	/// Target compression ratio (e.g., 2.0 = compress to 1/2 size, 4.0 = compress to 1/4 size)
	pub target_ratio: f64,
}

/// Compression decision model configuration
/// All standard model parameters for the compression decision API call
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CompressionDecisionConfig {
	/// Model to use for compression decisions (provider:model format)
	/// Example: "openrouter:anthropic/claude-haiku-4" (cost-efficient for decisions)
	pub model: String,
	/// Maximum tokens to generate (0 = no limit, let AI decide based on prompt)
	pub max_tokens: u32,
	/// Sampling temperature (0.0 to 2.0)
	pub temperature: f32,
	/// Top-p nucleus sampling (0.0 to 1.0)
	pub top_p: f32,
	/// Top-k sampling (1 to infinity)
	pub top_k: u32,
	/// Maximum retry attempts on failure
	pub max_retries: u32,
	/// Base timeout for exponential backoff retry logic (seconds)
	pub retry_timeout: u64,
	/// Ignore compression decision cost in session tracking (useful for subscription models)
	/// When true, the compression decision API call is treated as free and not added to total cost
	pub ignore_cost: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CompressionHintConfig {
	/// Enable compression system (task → phase → project, all automatic)
	pub hints_enabled: bool,
	/// Context pressure threshold (0.0-1.0) at which to start showing hints
	pub hints_pressure_threshold: f64,
	/// Minimum tool executions between hints
	pub hints_min_interval: usize,
	/// Enable adaptive pressure-based compression (SOTA approach)
	/// When true, compression triggers based on absolute token count from pressure_levels
	pub adaptive_threshold: bool,
	/// Compression aggressiveness levels based on absolute token count
	/// Each level defines threshold (token count) and target compression ratio
	/// Compression triggers when context exceeds ANY threshold, using the highest matched ratio
	/// Example: At 100k tokens, uses 4.0x compression (75% reduction)
	pub pressure_levels: Vec<PressureLevel>,
	/// Decision model configuration for compression decisions and summary generation
	/// Use a fast, cheap model like Haiku for cost savings (10x cheaper than Sonnet)
	pub decision: CompressionDecisionConfig,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PromptConfig {
	/// Name of the prompt (used with /prompt command)
	pub name: String,
	/// The prompt template text
	pub prompt: String,
	/// Optional description for help display
	pub description: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Config {
	// Config version for future migrations (always first field)
	pub version: u32,

	// Root-level log level setting (takes precedence over role-specific)
	pub log_level: LogLevel,

	// Root-level model setting (used by all commands if specified)
	pub model: String,

	// Default tag used when no TAG is passed to `octomind run/acp/server`.
	// Can be a role name (e.g. "developer") or a tap agent (e.g. "octomind:assistant").
	pub default: String,

	// Root-level max_tokens setting (used by all commands if specified)
	pub max_tokens: u32,

	// Custom instructions file name (relative to project root)
	pub custom_instructions_file_name: String,

	// Custom constraints file name (relative to project root)
	pub custom_constraints_file_name: String,

	// System-wide configuration settings (not role-specific)
	pub mcp_response_warning_threshold: usize,
	pub mcp_response_tokens_threshold: usize,
	pub max_session_tokens_threshold: usize,
	pub cache_tokens_threshold: u64,
	pub cache_timeout_seconds: u64,
	pub enable_markdown_rendering: bool,
	// Markdown theme for styling
	pub markdown_theme: String,
	// Session spending threshold in USD - if > 0, prompt user when exceeded
	pub max_session_spending_threshold: f64,
	// Request spending threshold in USD - if > 0, stop execution when exceeded during single request
	pub max_request_spending_threshold: f64,

	// Use long-term (1h) caching for system messages (strict: must be in config)
	pub use_long_system_cache: bool,

	// Maximum number of retries for API calls (can be overridden by --max-retries CLI flag)
	pub max_retries: u32,

	// Base timeout for exponential backoff retry logic (config-only, no CLI override)
	pub retry_timeout: u32,

	// Agent configurations - simplified ACP-based definitions
	#[serde(default)]
	pub agents: Vec<crate::config::agents::AgentConfig>,

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

	// Workflows configuration - array of workflow definitions
	#[serde(default)]
	pub workflows: Vec<WorkflowDefinition>,

	// Prompt template configurations
	pub prompts: Vec<PromptConfig>,

	// Plan-driven compression configuration
	pub compression: CompressionHintConfig,

	// Legacy system prompt field for backward compatibility
	pub system: Option<String>,
	// Runtime output mode set by CLI (plain or jsonl)
	#[serde(skip)]
	pub runtime_output_mode: Option<String>,

	// Runtime working directory for parallel execution (not serialized)
	// When set, all file/shell operations use this directory instead of current_dir
	#[serde(skip)]
	pub working_directory: Option<PathBuf>,

	// Sandbox mode: restrict all filesystem writes to the current working directory
	// Can also be enabled at runtime with --sandbox CLI flag
	pub sandbox: bool,

	// Capability provider overrides (capability_name → provider_name)
	// Empty by default — uses "default" provider for each capability.
	// User can override e.g. capabilities = { codesearch = "octocode" }
	#[serde(default)]
	pub capabilities: HashMap<String, String>,

	// Agent registry configuration
	#[serde(default)]
	pub registry: crate::config::registry::RegistryConfig,

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
		// Convert HashMap to Vec, ensuring names match keys
		let servers_vec: Vec<McpServerConfig> = servers
			.into_iter()
			.map(|(name, server)| {
				// Recreate server with correct name if it doesn't match
				match server {
					McpServerConfig::Builtin {
						timeout_seconds,
						tools,
						auto_bind,
						..
					} => McpServerConfig::Builtin {
						name,
						timeout_seconds,
						tools,
						auto_bind,
					},
					McpServerConfig::Http {
						name: _,
						url,
						auth_token,
						oauth,
						timeout_seconds,
						tools,
						auto_bind,
					} => McpServerConfig::Http {
						name,
						url,
						auth_token,
						oauth,
						timeout_seconds,
						tools,
						auto_bind,
					},
					McpServerConfig::Stdin {
						command,
						args,
						timeout_seconds,
						tools,
						auto_bind,
						..
					} => McpServerConfig::Stdin {
						name,
						command,
						args,
						timeout_seconds,
						tools,
						auto_bind,
					},
				}
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

	/// Get the effective max_tokens to use - uses root config max_tokens (now always required)
	pub fn get_effective_max_tokens(&self) -> u32 {
		// Max tokens is now always required in config, no fallback needed
		self.max_tokens
	}

	/// Get server configuration by name from the config registry
	/// Now relies entirely on config - no more runtime injection
	pub fn get_server_config(&self, server_name: &str) -> Option<McpServerConfig> {
		// Get from loaded registry
		self.mcp
			.servers
			.iter()
			.find(|s| s.name() == server_name)
			.cloned()
	}

	/// Get enabled servers for a role with runtime core server injection
	/// This ensures core servers are ALWAYS available regardless of config file state
	/// Also includes servers that auto-bind to the given role.
	pub fn get_enabled_servers_for_role(
		&self,
		role_mcp_config: &RoleMcpConfig,
		role_name: Option<&str>,
	) -> Vec<McpServerConfig> {
		// Use the updated RoleMcpConfig method that has runtime injection
		role_mcp_config.get_enabled_servers(&self.mcp.servers, role_name)
	}
	/// Get the global log level (system-wide setting)
	pub fn get_log_level(&self) -> LogLevel {
		self.log_level.clone()
	}

	/// Get the current output mode as a typed enum
	pub fn output_mode(&self) -> crate::session::output::OutputMode {
		crate::session::output::OutputMode::from_runtime_mode(
			self.runtime_output_mode.as_deref().unwrap_or("plain"),
		)
	}

	/// Get the model for the specified role
	pub fn get_model(&self, _role: &str) -> String {
		// All roles now use the system-wide model
		self.get_effective_model()
	}

	/// Get the max_tokens for the specified role
	pub fn get_max_tokens(&self, _role: &str) -> u32 {
		// All roles now use the system-wide max_tokens
		self.get_effective_max_tokens()
	}

	/// Check whether a role is defined in the config.
	pub fn has_role(&self, role: &str) -> bool {
		self.role_map.contains_key(role)
	}

	/// Get configuration for a specific role
	/// Returns: (role_config, role_mcp_config, layers, commands, system_prompt)
	/// Panics if the role is not found — call `has_role` first when the role comes from user input.
	pub fn get_role_config(&self, role: &str) -> RoleConfigResult<'_> {
		if let Some(role_config) = self.role_map.get(role) {
			(
				&role_config.config,
				&role_config.mcp,
				self.layers.as_ref(),
				self.commands.as_ref(),
				&role_config.config.system,
			)
		} else {
			// STRICT CONFIG: Unknown roles are not allowed - all roles must be explicitly defined
			panic!("CRITICAL CONFIG ERROR: Role '{role}' not found in config. All roles must be explicitly defined in config template.");
		}
	}

	/// Get a merged config for a specific role (for backward compatibility)
	/// This creates a new Config with role-specific settings merged into system-wide settings
	pub fn get_merged_config_for_role(&self, mode: &str) -> Config {
		let (_role_config, role_mcp_config, _role_layers_config, commands, system_prompt) =
			self.get_role_config(mode);

		let mut merged = self.clone();

		// CRITICAL FIX: Create a legacy McpConfig for backward compatibility with existing code
		// Use the new runtime injection method to ensure core servers are ALWAYS available
		// Also includes servers that auto-bind to this role.
		let enabled_servers = self.get_enabled_servers_for_role(role_mcp_config, Some(mode));

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
			crate::log_debug!("TRACE: Adding server '{}' to merged config", server.name());
		}

		merged.mcp = McpConfig {
			servers: enabled_servers, // Only role-enabled servers (with runtime injection)
			allowed_tools: role_mcp_config.allowed_tools.clone(),
		};

		// Role-specific layers are now managed by workflows
		// Keep merged.layers as original registry for agent tools
		// let enabled_layers = self.get_enabled_layers_for_role(mode);

		merged.commands = commands.cloned();
		merged.system = Some(system_prompt.clone());

		merged
	}

	/// Get the current working directory for file/shell operations
	/// Returns the runtime working_directory if set, otherwise falls back to current_dir
	pub fn get_working_directory(&self) -> PathBuf {
		self.working_directory
			.clone()
			.unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
	}

	/// Set the runtime working directory for parallel execution
	pub fn set_working_directory(&mut self, path: PathBuf) {
		self.working_directory = Some(path);
	}

	/// Get the role config struct for a specific role
	pub fn get_role_config_struct(&self, role: &str) -> &RoleConfig {
		let (role_config, _, _, _, _) = self.get_role_config(role);
		role_config
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
thread_local! {
	static CURRENT_CONFIG: RefCell<Option<Config>> = const { RefCell::new(None) };
	static CURRENT_ROLE: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Set the current config for the thread (to be used by logging macros)
pub fn set_thread_config(config: &Config) {
	CURRENT_CONFIG.with(|c| {
		*c.borrow_mut() = Some(config.clone());
	});
}

/// Set the current role for the thread (to be used by MCP tools)
pub fn set_thread_role(role: &str) {
	CURRENT_ROLE.with(|r| {
		*r.borrow_mut() = Some(role.to_string());
	});
}

/// Get the current role for the thread
pub fn get_thread_role() -> Option<String> {
	CURRENT_ROLE.with(|r| (*r.borrow()).clone())
}

/// Get the current config for the thread
pub fn with_thread_config<F, R>(f: F) -> Option<R>
where
	F: FnOnce(&Config) -> R,
{
	CURRENT_CONFIG.with(|c| (*c.borrow()).as_ref().map(f))
}
// LOGGING MACROS
// ============================================================================
// These macros route log output based on whether tracing is initialized:
// - Tracing initialized (CLI/ACP/WebSocket): use tracing (stderr or file)
// - No tracing: use colored println/eprintln for CLI
//
// IMPORTANT: In ACP/WebSocket mode, tracing writes to file only.
// stdout/stderr are reserved for JSON-RPC protocol communication.

/// Info logging macro with automatic cyan coloring (CLI) or tracing (ACP/WebSocket).
/// Shows info messages when log level is Info OR Debug.
#[macro_export]
macro_rules! log_info {
	($fmt:expr) => {
		if let Some(should_log) = $crate::config::with_thread_config(|config| {
			config.get_log_level().is_info_enabled()
		}) {
			if should_log {
				if $crate::logging::tracing_setup::is_tracing_initialized() {
					tracing::info!("{}", $fmt);
				} else if $crate::config::with_thread_config(|config| {
					!config.output_mode().should_suppress_cli_output()
				}).unwrap_or(true) {
					use colored::Colorize;
					$crate::println!("{}", $fmt.cyan());
				}
			}
		}
	};
	($fmt:expr, $($arg:expr),*) => {
		if let Some(should_log) = $crate::config::with_thread_config(|config| {
			config.get_log_level().is_info_enabled()
		}) {
			if should_log {
				if $crate::logging::tracing_setup::is_tracing_initialized() {
					tracing::info!($fmt, $($arg),*);
				} else if $crate::config::with_thread_config(|config| {
					!config.output_mode().should_suppress_cli_output()
				}).unwrap_or(true) {
					use colored::Colorize;
					$crate::println!("{}", format!($fmt, $($arg),*).cyan());
				}
			}
		}
	};
}

/// Debug logging macro with automatic bright blue coloring (CLI) or tracing (ACP/WebSocket).
#[macro_export]
macro_rules! log_debug {
	($fmt:expr) => {
		if let Some(should_log) = $crate::config::with_thread_config(|config| {
			config.get_log_level().is_debug_enabled()
		}) {
			if should_log {
				if $crate::logging::tracing_setup::is_tracing_initialized() {
					tracing::debug!("{}", $fmt);
				} else if $crate::config::with_thread_config(|config| {
					!config.output_mode().should_suppress_cli_output()
				}).unwrap_or(true) {
					use colored::Colorize;
					$crate::println!("{}", $fmt.bright_blue());
				}
			}
		}
	};
	($fmt:expr, $($arg:expr),*) => {
		if let Some(should_log) = $crate::config::with_thread_config(|config| {
			config.get_log_level().is_debug_enabled()
		}) {
			if should_log {
				if $crate::logging::tracing_setup::is_tracing_initialized() {
					tracing::debug!($fmt, $($arg),*);
				} else if $crate::config::with_thread_config(|config| {
					!config.output_mode().should_suppress_cli_output()
				}).unwrap_or(true) {
					use colored::Colorize;
					$crate::println!("{}", format!($fmt, $($arg),*).bright_blue());
				}
			}
		}
	};
}

/// Error logging macro with automatic bright red coloring (CLI) or tracing + file (ACP/WebSocket).
/// Always visible regardless of log level.
/// In ACP mode, also writes to the dedicated error sink for structured JSONL error tracking.
#[macro_export]
macro_rules! log_error {
	($fmt:expr) => {{
		if $crate::logging::tracing_setup::is_tracing_initialized() {
			tracing::error!("{}", $fmt);
			// In ACP mode, also write to the structured error sink
			if $crate::logging::tracing_setup::is_structured_output_mode() {
				if let Some(sink) = $crate::logging::AcpErrorSink::get_global() {
					let _ = sink.log_error_simple($fmt);
				}
			}
		} else {
			use colored::Colorize;
			$crate::eprintln!("{}", $fmt.bright_red());
		}
	}};
	($fmt:expr, $($arg:expr),*) => {{
		if $crate::logging::tracing_setup::is_tracing_initialized() {
			tracing::error!($fmt, $($arg),*);
			if $crate::logging::tracing_setup::is_structured_output_mode() {
				if let Some(sink) = $crate::logging::AcpErrorSink::get_global() {
					let _ = sink.log_error_simple(&format!($fmt, $($arg),*));
				}
			}
		} else {
			use colored::Colorize;
			$crate::eprintln!("{}", format!($fmt, $($arg),*).bright_red());
		}
	}};
}

/// Conditional logging - prints different messages based on log level.
#[macro_export]
macro_rules! log_conditional {
	(debug: $debug_msg:expr, info: $info_msg:expr, none: $none_msg:expr) => {
		if let Some(level) = $crate::config::with_thread_config(|config| config.get_log_level()) {
			match level {
				$crate::config::LogLevel::Debug => {
					if $crate::logging::tracing_setup::is_tracing_initialized() {
						tracing::debug!("{}", $debug_msg);
					} else {
						$crate::println!("{}", $debug_msg);
					}
				}
				$crate::config::LogLevel::Info => {
					if $crate::logging::tracing_setup::is_tracing_initialized() {
						tracing::info!("{}", $info_msg);
					} else {
						$crate::println!("{}", $info_msg);
					}
				}
				$crate::config::LogLevel::None => {
					if $crate::logging::tracing_setup::is_tracing_initialized() {
						tracing::info!("{}", $none_msg);
					} else {
						$crate::println!("{}", $none_msg);
					}
				}
			}
		} else {
			// Fallback if no config is set
			$crate::println!("{}", $none_msg);
		}
	};
	(debug: $debug_msg:expr, default: $default_msg:expr) => {
		if let Some(should_debug) =
			$crate::config::with_thread_config(|config| config.get_log_level().is_debug_enabled())
		{
			if should_debug {
				if $crate::logging::tracing_setup::is_tracing_initialized() {
					tracing::debug!("{}", $debug_msg);
				} else {
					$crate::println!("{}", $debug_msg);
				}
			} else {
				if $crate::logging::tracing_setup::is_tracing_initialized() {
					tracing::info!("{}", $default_msg);
				} else {
					$crate::println!("{}", $default_msg);
				}
			}
		} else {
			// Fallback if no config is set
			$crate::println!("{}", $default_msg);
		}
	};
	(info: $info_msg:expr, default: $default_msg:expr) => {
		if let Some(should_info) =
			$crate::config::with_thread_config(|config| config.get_log_level().is_info_enabled())
		{
			if should_info {
				if $crate::logging::tracing_setup::is_tracing_initialized() {
					tracing::info!("{}", $info_msg);
				} else {
					$crate::println!("{}", $info_msg);
				}
			} else {
				if $crate::logging::tracing_setup::is_tracing_initialized() {
					tracing::info!("{}", $default_msg);
				} else {
					$crate::println!("{}", $default_msg);
				}
			}
		} else {
			// Fallback if no config is set
			$crate::println!("{}", $default_msg);
		}
	};
}
