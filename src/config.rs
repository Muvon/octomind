use anyhow::{Result, Context, anyhow};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

// Type alias to simplify the complex return type for get_mode_config
type ModeConfigResult<'a> = (
	&'a ModeConfig,
	&'a RoleMcpConfig,
	Option<&'a Vec<crate::session::layers::LayerConfig>>,
	Option<&'a std::collections::HashMap<String, crate::session::layers::LayerConfig>>,
	Option<&'a String>
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

/// Logging macros for different log levels
/// These macros automatically check the current log level and only print if appropriate
use std::cell::RefCell;

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
	CURRENT_CONFIG.with(|c| {
		(*c.borrow()).as_ref().map(f)
	})
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
		if let Some(should_debug) = $crate::config::with_thread_config(|config| config.get_log_level().is_debug_enabled()) {
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
		if let Some(should_info) = $crate::config::with_thread_config(|config| config.get_log_level().is_info_enabled()) {
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

// Provider configurations - ONLY contain API keys and provider-specific settings
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct ProviderConfig {
	pub api_key: Option<String>,
	#[serde(default)]
	pub pricing: PricingConfig,
}


#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct ProvidersConfig {
	#[serde(default)]
	pub openrouter: ProviderConfig,
	#[serde(default)]
	pub openai: ProviderConfig,
	#[serde(default)]
	pub anthropic: ProviderConfig,
	#[serde(default)]
	pub google: ProviderConfig,
	#[serde(default)]
	pub amazon: ProviderConfig,
	#[serde(default)]
	pub cloudflare: ProviderConfig,
}



// Mode configuration - contains all behavior settings but NOT API keys
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ModeConfig {
	// Model in provider:model format (e.g., "openrouter:anthropic/claude-3.5-sonnet")
	#[serde(default = "default_full_model")]
	pub model: String,
	// Layer configurations
	#[serde(default)]
	pub enable_layers: bool,
	// Custom system prompt
	pub system: Option<String>,
}

fn default_full_model() -> String {
	"openrouter:anthropic/claude-3.5-sonnet".to_string()
}

fn default_system_model() -> String {
	"openrouter:anthropic/claude-3.5-haiku".to_string()
}

impl Default for ModeConfig {
	fn default() -> Self {
		Self {
			model: default_full_model(),
			enable_layers: false,
			system: None,
		}
	}
}

impl ModeConfig {
	/// Get the full model string in provider:model format for API calls
	pub fn get_full_model(&self) -> String {
		self.model.clone()
	}

	/// Get the provider name from the model string
	pub fn get_provider(&self) -> Result<String> {
		if let Ok((provider, _)) = crate::session::ProviderFactory::parse_model(&self.model) {
			Ok(provider)
		} else {
			Err(anyhow!("Invalid model format: {}", self.model))
		}
	}

	/// Get the API key for this mode's provider
	pub fn get_api_key(&self, providers: &ProvidersConfig) -> Option<String> {
		if let Ok(provider) = self.get_provider() {
			match provider.as_str() {
				"openrouter" => providers.openrouter.api_key.clone(),
				"openai" => providers.openai.api_key.clone(),
				"anthropic" => providers.anthropic.api_key.clone(),
				"google" => providers.google.api_key.clone(),
				"amazon" => providers.amazon.api_key.clone(),
				"cloudflare" => providers.cloudflare.api_key.clone(),
				_ => None,
			}
		} else {
			None
		}
	}

	/// Get pricing config for this mode's provider
	pub fn get_pricing(&self, providers: &ProvidersConfig) -> PricingConfig {
		if let Ok(provider) = self.get_provider() {
			match provider.as_str() {
				"openrouter" => providers.openrouter.pricing.clone(),
				"openai" => providers.openai.pricing.clone(),
				"anthropic" => providers.anthropic.pricing.clone(),
				"google" => providers.google.pricing.clone(),
				"amazon" => providers.amazon.pricing.clone(),
				"cloudflare" => providers.cloudflare.pricing.clone(),
				_ => PricingConfig::default(),
			}
		} else {
			PricingConfig::default()
		}
	}
}

// Legacy OpenRouterConfig for backward compatibility
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct OpenRouterConfig {
	#[serde(default = "default_openrouter_model")]
	pub model: String,
	pub api_key: Option<String>,
	#[serde(default)]
	pub pricing: PricingConfig,
	// Layer configurations for the new layered architecture
	#[serde(default)]
	pub enable_layers: bool,
	// Log level setting (replaces debug mode)
	#[serde(default)]
	pub log_level: LogLevel,
	// Maximum response tokens warning threshold
	#[serde(default = "default_mcp_response_warning_threshold")]
	pub mcp_response_warning_threshold: usize,
	// Maximum request tokens threshold (for automatic context truncation)
	#[serde(default = "default_max_request_tokens_threshold")]
	pub max_request_tokens_threshold: usize,
	// Enable automatic context truncation when max threshold is reached
	#[serde(default)]
	pub enable_auto_truncation: bool,
	// Threshold percentage for automatic cache marker movement
	// 0 or 100 disables auto-cache, any value between 1-99 enables it
	#[serde(default = "default_cache_tokens_pct_threshold")]
	pub cache_tokens_pct_threshold: u8,
	// Alternative: Auto-cache when non-cached tokens reach this absolute number
	// If set to 0, uses percentage threshold instead
	#[serde(default)]
	pub cache_tokens_absolute_threshold: u64,
	// Auto-cache timeout in seconds (3 minutes = 180 seconds by default)
	// If time since last cache checkpoint exceeds this, auto-cache triggers
	#[serde(default = "default_cache_timeout_seconds")]
	pub cache_timeout_seconds: u64,
	// Enable markdown rendering for AI responses
	#[serde(default)]
	pub enable_markdown_rendering: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PricingConfig {
	#[serde(default = "default_input_price")]
	pub input_price: f64,
	#[serde(default = "default_output_price")]
	pub output_price: f64,
}

fn default_input_price() -> f64 {
	0.000001 // Default price per input token in USD, adjust based on model
}

fn default_output_price() -> f64 {
	0.000002 // Default price per output token in USD, adjust based on model
}

fn default_mcp_response_warning_threshold() -> usize {
	20000 // Default threshold for warning about large MCP responses (20k tokens)
}

fn default_max_request_tokens_threshold() -> usize {
	50000 // Default threshold for auto-truncation (50k tokens)
}

fn default_cache_tokens_pct_threshold() -> u8 {
	40 // Default 40% threshold for automatic cache marker movement
}

fn default_cache_timeout_seconds() -> u64 {
	180 // Default 3 minutes timeout for time-based auto-caching
}

impl Default for PricingConfig {
	fn default() -> Self {
		Self {
			input_price: default_input_price(),
			output_price: default_output_price(),
		}
	}
}

fn default_openrouter_model() -> String {
	"openrouter:anthropic/claude-sonnet-4".to_string()
}

impl Default for OpenRouterConfig {
	fn default() -> Self {
		Self {
			model: default_openrouter_model(),
			api_key: None,
			pricing: PricingConfig::default(),
			enable_layers: false, // Disabled by default
			log_level: LogLevel::default(),
			mcp_response_warning_threshold: default_mcp_response_warning_threshold(),
			max_request_tokens_threshold: default_max_request_tokens_threshold(),
			enable_auto_truncation: false, // Disabled by default
			cache_tokens_pct_threshold: default_cache_tokens_pct_threshold(),
			cache_tokens_absolute_threshold: 0, // Disabled by default, use percentage
			cache_timeout_seconds: default_cache_timeout_seconds(),
			enable_markdown_rendering: false, // Disabled by default
		}
	}
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub enum McpServerType {
	#[serde(rename = "external")]
	External,      // External server (URL or command)
	#[serde(rename = "developer")]
	Developer,     // Built-in developer tools
	#[serde(rename = "filesystem")]
	Filesystem,    // Built-in filesystem tools
}

impl Default for McpServerType {
	fn default() -> Self {
		Self::External
	}
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub enum McpServerMode {
	#[serde(rename = "http")]
	Http,
	#[serde(rename = "stdin")]
	Stdin,
}

impl Default for McpServerMode {
	fn default() -> Self {
		Self::Http
	}
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct McpServerConfig {
	// Name is auto-set from registry key (runtime field)
	#[serde(skip)]
	pub name: String,

	// Server type is auto-detected from name (runtime field)
	#[serde(skip)]
	pub server_type: McpServerType,

	// External server configuration
	pub url: Option<String>,
	pub auth_token: Option<String>,
	pub command: Option<String>,
	#[serde(default)]
	pub args: Vec<String>,

	// Communication mode - http or stdin (for external servers)
	#[serde(default)]
	pub mode: McpServerMode,

	// Timeout in seconds for tool execution
	#[serde(default = "default_timeout")]
	pub timeout_seconds: u64,

	// Tool filtering - empty means all tools are enabled
	#[serde(default)]
	pub tools: Vec<String>,
}

fn default_timeout() -> u64 {
	30 // Default timeout of 30 seconds
}

impl Default for McpServerConfig {
	fn default() -> Self {
		Self {
			name: "".to_string(),
			server_type: McpServerType::External, // Will be auto-detected
			url: None,
			auth_token: None,
			command: None,
			args: Vec::new(),
			mode: McpServerMode::Http,
			timeout_seconds: default_timeout(),
			tools: Vec::new(),
		}
	}
}

impl McpServerConfig {
	/// Create a server config from just the key name, auto-detecting type
	pub fn from_name(name: &str) -> Self {
		let server_type = match name {
			"developer" => McpServerType::Developer,
			"filesystem" => McpServerType::Filesystem,
			_ => McpServerType::External,
		};

		Self {
			name: name.to_string(),
			server_type,
			url: None,
			auth_token: None,
			command: None,
			args: Vec::new(),
			mode: McpServerMode::Http,
			timeout_seconds: 30,
			tools: Vec::new(),
		}
	}

	/// Create a developer server configuration
	pub fn developer(name: &str, tools: Vec<String>) -> Self {
		Self {
			name: name.to_string(),
			server_type: McpServerType::Developer,
			tools,
			..Default::default()
		}
	}

	/// Create a filesystem server configuration
	pub fn filesystem(name: &str, tools: Vec<String>) -> Self {
		Self {
			name: name.to_string(),
			server_type: McpServerType::Filesystem,
			tools,
			..Default::default()
		}
	}

	/// Create an external HTTP server configuration
	pub fn external_http(name: &str, url: &str, tools: Vec<String>) -> Self {
		Self {
			name: name.to_string(),
			server_type: McpServerType::External,
			url: Some(url.to_string()),
			mode: McpServerMode::Http,
			tools,
			..Default::default()
		}
	}

	/// Create an external command-based server configuration
	pub fn external_command(name: &str, command: &str, args: Vec<String>, tools: Vec<String>) -> Self {
		Self {
			name: name.to_string(),
			server_type: McpServerType::External,
			command: Some(command.to_string()),
			args,
			mode: McpServerMode::Stdin,
			tools,
			..Default::default()
		}
	}
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
pub struct McpConfig {
	#[serde(default)]
	pub enabled: bool,

	// Server registry - server configurations
	#[serde(default)]
	pub servers: std::collections::HashMap<String, McpServerConfig>,

	// Tool filtering - allows limiting tools across all enabled servers
	#[serde(default)]
	pub allowed_tools: Vec<String>,
}

// Role-specific MCP configuration with server_refs
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
pub struct RoleMcpConfig {
	// Server references - list of server names from the global registry to use for this role
	// Empty list means MCP is disabled for this role
	#[serde(default)]
	pub server_refs: Vec<String>,

	// Tool filtering - allows limiting tools across all enabled servers for this role
	#[serde(default)]
	pub allowed_tools: Vec<String>,
}

impl RoleMcpConfig {
	/// Check if MCP is enabled for this role (has any server references)
	pub fn is_enabled(&self) -> bool {
		!self.server_refs.is_empty()
	}

	/// Get enabled servers from the global registry for this role
	pub fn get_enabled_servers(&self, global_servers: &std::collections::HashMap<String, McpServerConfig>) -> Vec<McpServerConfig> {
		if self.server_refs.is_empty() {
			return Vec::new();
		}

		let mut result = Vec::new();
		for server_name in &self.server_refs {
			if let Some(server_config) = global_servers.get(server_name) {
				let mut server = server_config.clone();
				// Auto-set the name from the registry key
				server.name = server_name.clone();
				// Auto-detect server type from name
				server.server_type = match server_name.as_str() {
					"developer" => McpServerType::Developer,
					"filesystem" => McpServerType::Filesystem,
					_ => McpServerType::External,
				};
				// Apply role-specific tool filtering if specified
				if !self.allowed_tools.is_empty() {
					server.tools = self.allowed_tools.clone();
				}
				result.push(server);
			} else {
				crate::log_debug!("Server '{}' referenced by role but not found in global registry", server_name);
			}
		}

		result
	}

	/// Create a config with specific server references
	pub fn with_server_refs(server_refs: Vec<String>) -> Self {
		Self {
			server_refs,
			allowed_tools: Vec::new(),
		}
	}

	/// Create a config with specific server references and allowed tools
	pub fn with_server_refs_and_tools(server_refs: Vec<String>, allowed_tools: Vec<String>) -> Self {
		Self {
			server_refs,
			allowed_tools,
		}
	}
}

impl McpConfig {
	/// Check if the global MCP registry is enabled
	pub fn is_enabled(&self) -> bool {
		self.enabled
	}

	/// Get all servers from the registry (for populating role configs)
	pub fn get_all_servers(&self) -> Vec<McpServerConfig> {
		let mut result = Vec::new();

		// Add servers from registry
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
		}

		result
	}

	/// Create a config using server configurations
	pub fn with_servers(enabled: bool, servers: std::collections::HashMap<String, McpServerConfig>, allowed_tools: Option<Vec<String>>) -> Self {
		Self {
			enabled,
			servers,
			allowed_tools: allowed_tools.unwrap_or_default(),
		}
	}
}

// Updated role configurations using the new ModeConfig structure
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DeveloperRoleConfig {
	#[serde(flatten)]
	pub config: ModeConfig,
	#[serde(default)]
	pub mcp: RoleMcpConfig,
	// Layer configuration
	#[serde(default)]
	pub layers: Option<Vec<crate::session::layers::LayerConfig>>,
	// Legacy openrouter field for backward compatibility
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub openrouter: Option<OpenRouterConfig>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AssistantRoleConfig {
	#[serde(flatten)]
	pub config: ModeConfig,
	#[serde(default)]
	pub mcp: RoleMcpConfig,
	// Legacy openrouter field for backward compatibility
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub openrouter: Option<OpenRouterConfig>,
}

impl Default for DeveloperRoleConfig {
	fn default() -> Self {		
		Self {
			config: ModeConfig {
				model: "openrouter:anthropic/claude-sonnet-4".to_string(),
				enable_layers: true,
				system: Some("You are an Octodev AI developer assistant with full access to development tools.".to_string()),
			},
			mcp: RoleMcpConfig::with_server_refs(vec![
				"octocode".to_string(),
				"filesystem".to_string(),
				"developer".to_string(),
			]),
			layers: None,
			openrouter: None,
		}
	}
}

impl Default for AssistantRoleConfig {
	fn default() -> Self {
		Self {
			config: ModeConfig {
				model: "openrouter:anthropic/claude-3.5-haiku".to_string(),
				enable_layers: false,
				system: Some("You are a helpful assistant.".to_string()),
			},
			mcp: RoleMcpConfig::default(), // Empty server_refs = MCP disabled
			openrouter: None,
		}
	}
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
	#[serde(default = "default_cache_tokens_pct_threshold")]
	pub cache_tokens_pct_threshold: u8,
	#[serde(default)]
	pub cache_tokens_absolute_threshold: u64,
	#[serde(default = "default_cache_timeout_seconds")]
	pub cache_timeout_seconds: u64,
	#[serde(default)]
	pub enable_markdown_rendering: bool,

	// NEW: Providers configuration - centralized API keys
	#[serde(default)]
	pub providers: ProvidersConfig,

	// Role-specific configurations
	#[serde(default)]
	pub developer: DeveloperRoleConfig,
	#[serde(default)]
	pub assistant: AssistantRoleConfig,

	// Global MCP configuration (fallback for roles)
	#[serde(default)]
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



impl Config {
	/// Check if the octocode binary is available in PATH
	fn is_octocode_available() -> bool {
		use std::process::Command;
		
		// Try to run `octocode --version` to check if it's available
		match Command::new("octocode")
			.arg("--version")
			.output()
		{
			Ok(output) => output.status.success(),
			Err(_) => false,
		}
	}

	/// Auto-configure octocode server based on binary availability
	fn auto_configure_octocode(&mut self) {
		// Check if developer role has octocode in server_refs but global registry doesn't have octocode server yet
		if self.developer.mcp.server_refs.contains(&"octocode".to_string()) && !self.mcp.servers.contains_key("octocode") {
			let available = Self::is_octocode_available();
			
			self.mcp.servers.insert(
				"octocode".to_string(),
				McpServerConfig {
					name: "octocode".to_string(),
					server_type: McpServerType::External,
					command: Some("octocode".to_string()),
					args: vec!["mcp".to_string(), "--path=.".to_string()],
					mode: McpServerMode::Stdin,
					timeout_seconds: 30,
					tools: vec![],
					url: None,
					auth_token: None,
				}
			);
			
			if available {
				crate::log_info!("Auto-configured octocode MCP server (binary detected in PATH)");
			} else {
				crate::log_debug!("octocode binary not found in PATH, server configured but may not work");
			}
		}
	}

	/// Get the effective model to use - checks root config, then falls back to system default
	pub fn get_effective_model(&self) -> String {
		// If root-level model is set (not the default), use it
		if !self.model.is_empty() && self.model != default_system_model() {
			return self.model.clone();
		}
		
		// Otherwise, use the system default
		default_system_model()
	}

	/// Initialize the default server registry with auto-detection
	pub fn init_default_server_registry(&mut self) {
		// ALWAYS ensure core servers are in the global registry
		// This is the server registry that all roles reference via server_refs
		
		// Add built-in servers if not present
		if !self.mcp.servers.contains_key("developer") {
			self.mcp.servers.insert(
				"developer".to_string(),
				McpServerConfig::from_name("developer")
			);
		}
		
		if !self.mcp.servers.contains_key("filesystem") {
			self.mcp.servers.insert(
				"filesystem".to_string(),
				McpServerConfig::from_name("filesystem")
			);
		}
		
		// Add octocode server with auto-detection if not present
		if !self.mcp.servers.contains_key("octocode") {
			let octocode_available = Self::is_octocode_available();
			self.mcp.servers.insert(
				"octocode".to_string(),
				McpServerConfig {
					name: "octocode".to_string(),
					server_type: McpServerType::External,
					command: Some("octocode".to_string()),
					args: vec!["mcp".to_string(), "--path=.".to_string()],
					mode: McpServerMode::Stdin,
					timeout_seconds: 30,
					tools: vec![],
					url: None,
					auth_token: None,
				}
			);
			
			if octocode_available {
				crate::log_info!("Auto-configured octocode MCP server (binary detected)");
			} else {
				crate::log_debug!("octocode binary not found in PATH, server configured but may not work");
			}
		}
		
		// Enable the global MCP registry by default
		if !self.mcp.enabled {
			self.mcp.enabled = true;
		}
	}

	/// Get server configuration by name from registry, with fallback to defaults
	pub fn get_server_config(&self, server_name: &str) -> Option<McpServerConfig> {
		// First check registry
		if let Some(server) = self.mcp.servers.get(server_name) {
			return Some(server.clone());
		}

		// Fallback to auto-generated built-in server types
		match server_name {
			"developer" | "filesystem" => Some(McpServerConfig::from_name(server_name)),
			_ => None,
		}
	}

	/// Get enabled servers for a role from the global registry
	pub fn get_enabled_servers_for_role(&self, role_mcp_config: &RoleMcpConfig) -> Vec<McpServerConfig> {
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
				(&self.developer.config, &self.developer.mcp, self.developer.layers.as_ref(), self.commands.as_ref(), self.developer.config.system.as_ref())
			},
			"assistant" => {
				// Base assistant role
				(&self.assistant.config, &self.assistant.mcp, None, self.commands.as_ref(), self.assistant.config.system.as_ref())
			},
			_ => {
				// For any custom role, inherit from assistant first
				// This implements the inheritance pattern where new roles start from assistant base
				// TODO: In future, load custom role config and merge with assistant as base

				// For now, fall back to assistant role as the base inheritance
				self.get_mode_config("assistant")
			}
		}
	}

	/// Get a merged config for a specific mode that can be used for API calls
	/// This returns a Config with the mode-specific settings applied
	pub fn get_merged_config_for_mode(&self, mode: &str) -> Config {
		let (mode_config, role_mcp_config, layers_config, commands_config, system_prompt) = self.get_mode_config(mode);

		let mut merged = self.clone();

		// Create an OpenRouterConfig from the ModeConfig for backward compatibility
		merged.openrouter = OpenRouterConfig {
			model: mode_config.get_full_model(),
			api_key: mode_config.get_api_key(&self.providers),
			pricing: mode_config.get_pricing(&self.providers),
			enable_layers: mode_config.enable_layers,
			log_level: self.get_log_level(), // Use global log level
			// Use system-wide settings for these configuration options
			mcp_response_warning_threshold: self.mcp_response_warning_threshold,
			max_request_tokens_threshold: self.max_request_tokens_threshold,
			enable_auto_truncation: self.enable_auto_truncation,
			cache_tokens_pct_threshold: self.cache_tokens_pct_threshold,
			cache_tokens_absolute_threshold: self.cache_tokens_absolute_threshold,
			cache_timeout_seconds: self.cache_timeout_seconds,
			enable_markdown_rendering: self.enable_markdown_rendering,
		};

		// Create a legacy McpConfig for backward compatibility with existing code
		// This gets servers from the role's server_refs
		let enabled_servers = self.get_enabled_servers_for_role(role_mcp_config);
		let mut legacy_servers = std::collections::HashMap::new();
		
		for server in enabled_servers {
			legacy_servers.insert(server.name.clone(), server);
		}
		
		merged.mcp = McpConfig {
			enabled: !role_mcp_config.server_refs.is_empty(), // MCP enabled if role has server_refs
			servers: legacy_servers,
			allowed_tools: role_mcp_config.allowed_tools.clone(),
		};
		
		merged.layers = layers_config.cloned();
		merged.commands = commands_config.cloned();
		merged.system = system_prompt.cloned();

		merged
	}

	/// Helper method to get the role config struct directly
	pub fn get_mode_config_struct(&self, role: &str) -> &ModeConfig {
		match role {
			"developer" => &self.developer.config,
			"assistant" => &self.assistant.config,
			_ => &self.assistant.config, // Default fallback to assistant for inheritance
		}
	}

	/// Initialize the server registry and API keys
	fn initialize_config(&mut self) {
		// Initialize default server registry if empty
		self.init_default_server_registry();

		// Auto-configure octocode server based on binary availability
		self.auto_configure_octocode();

		// Migrate API keys from legacy openrouter config to providers
		if let Some(api_key) = &self.openrouter.api_key {
			if self.providers.openrouter.api_key.is_none() {
				self.providers.openrouter.api_key = Some(api_key.clone());
			}
		}
	}

	pub fn ensure_octodev_dir() -> Result<std::path::PathBuf> {
		// Use the system-wide directory
		crate::directories::get_octodev_data_dir()
	}

	/// Validate the configuration for common issues
	pub fn validate(&self) -> Result<()> {
		// Validate OpenRouter model name
		if let Err(e) = self.validate_openrouter_model() {
			eprintln!("Configuration validation warning: {}", e);
			eprintln!("The application will continue, but you may want to fix these issues.");
			eprintln!("Update your system config to use the new format:");
			eprintln!("  Before: model = \"anthropic/claude-3.5-sonnet\"");
			eprintln!("  After:  model = \"openrouter:anthropic/claude-3.5-sonnet\"");
			// Don't return error, just warn
		}

		// Validate threshold values
		self.validate_thresholds()?;

		// Validate MCP configuration
		self.validate_mcp_config()?;

		// Validate layer configuration if present
		if let Some(layers) = &self.layers {
			self.validate_layers(layers)?;
		}

		Ok(())
	}

	fn validate_openrouter_model(&self) -> Result<()> {
		let model = &self.openrouter.model;

		// Check if model has the required provider:model format
		if !model.contains(':') {
			return Err(anyhow!(
				"Invalid model format: '{}'. Must use 'provider:model' format (e.g., 'openrouter:anthropic/claude-3.5-sonnet', 'openai:gpt-4o')",
				model
			));
		}

		// Parse and validate using the provider factory
		match crate::session::ProviderFactory::parse_model(model) {
			Ok((provider_name, model_name)) => {
				// Try to create the provider to validate it exists
				match crate::session::ProviderFactory::create_provider(&provider_name) {
					Ok(provider) => {
						// Check if the provider supports this model
						if !provider.supports_model(&model_name) {
							return Err(anyhow!(
								"Provider '{}' does not support model '{}'. Check the provider documentation for supported models.",
								provider_name, model_name
							));
						}
					},
					Err(_) => {
						return Err(anyhow!(
							"Unsupported provider: '{}'. Supported providers: openrouter, openai, anthropic, google, amazon, cloudflare",
							provider_name
						));
					}
				}
			},
			Err(e) => {
				return Err(anyhow!("Model validation failed: {}", e));
			}
		}

		Ok(())
	}

	fn validate_thresholds(&self) -> Result<()> {
		// Check raw configured values - 0 is a valid explicit choice for disabling features
		let cache_pct_threshold = self.cache_tokens_pct_threshold;

		// Only percentage threshold has a logical upper bound
		if cache_pct_threshold > 100 {
			return Err(anyhow!("Cache tokens percentage threshold must be between 0-100"));
		}

		// Warn if thresholds seem unusual (but don't error - user's choice)
		if self.mcp_response_warning_threshold != 0 && self.mcp_response_warning_threshold < 1000 {
			eprintln!("Warning: MCP response warning threshold ({}) is quite low", self.mcp_response_warning_threshold);
		}

		Ok(())
	}

	fn validate_mcp_config(&self) -> Result<()> {
		// Validate global MCP config - this one MUST have servers if enabled
		if self.mcp.enabled && self.mcp.servers.is_empty() {
			return Err(anyhow!("Global MCP config: MCP is enabled but no servers specified"));
		}

		// Validate that role server_refs point to existing servers in the global registry
		for server_ref in &self.developer.mcp.server_refs {
			if !self.mcp.servers.contains_key(server_ref) {
				// Check if it's a core server that we can auto-add
				match server_ref.as_str() {
					"developer" | "filesystem" | "octocode" => {
						// These are core servers that will be auto-added, so don't error
						crate::log_debug!("Core server '{}' referenced but not in registry - will be auto-added", server_ref);
					}
					_ => {
						return Err(anyhow!(
							"Developer role references server '{}' but it's not defined in global MCP registry",
							server_ref
						));
					}
				}
			}
		}

		for server_ref in &self.assistant.mcp.server_refs {
			if !self.mcp.servers.contains_key(server_ref) {
				// Check if it's a core server that we can auto-add
				match server_ref.as_str() {
					"developer" | "filesystem" | "octocode" => {
						// These are core servers that will be auto-added, so don't error
						crate::log_debug!("Core server '{}' referenced but not in registry - will be auto-added", server_ref);
					}
					_ => {
						return Err(anyhow!(
							"Assistant role references server '{}' but it's not defined in global MCP registry",
							server_ref
						));
					}
				}
			}
		}

		// Validate server configurations
		for (name, server) in &self.mcp.servers {
			// Auto-detect server type for validation
			let effective_type = match name.as_str() {
				"developer" => McpServerType::Developer,
				"filesystem" => McpServerType::Filesystem,
				_ => McpServerType::External,
			};

			match effective_type {
				crate::config::McpServerType::External => {
					// External servers must have either URL or command
					if server.url.is_none() && server.command.is_none() {
						return Err(anyhow!(
							"MCP server '{}': External server must have either 'url' or 'command' specified",
							name
						));
					}
				}
				crate::config::McpServerType::Developer | crate::config::McpServerType::Filesystem => {
					// Built-in servers should not have URL or command
					if server.url.is_some() || server.command.is_some() {
						eprintln!(
							"Warning: MCP server '{}': Built-in server has URL/command specified, which will be ignored",
							name
						);
					}
				}
			}

			// Validate timeout
			if server.timeout_seconds == 0 {
				return Err(anyhow!(
					"MCP server '{}': timeout must be greater than 0",
					name
				));
			}
		}

		Ok(())
	}

	fn validate_layers(&self, layers: &[crate::session::layers::LayerConfig]) -> Result<()> {
		let mut layer_count = 0;
		let mut names = std::collections::HashSet::new();

		for layer in layers {
			// We assume all configured layers are enabled (no more 'enabled' field)
			layer_count += 1;

			// Check for duplicate names
			if !names.insert(&layer.name) {
				return Err(anyhow!("Duplicate layer name: '{}'", layer.name));
			}

			// Validate temperature
			if layer.temperature < 0.0 || layer.temperature > 2.0 {
				return Err(anyhow!(
					"Layer '{}' temperature must be between 0.0 and 2.0",
					layer.name
				));
			}

			// Validate model format (only if specified - model is now optional)
			if let Some(ref model) = layer.model {
				if model.trim().is_empty() {
					return Err(anyhow!("Layer '{}' model cannot be empty", layer.name));
				}

				// Validate model format using provider factory if specified
				if !model.contains(':') {
					return Err(anyhow!(
						"Layer '{}' model '{}' must use 'provider:model' format (e.g., 'openrouter:anthropic/claude-3.5-sonnet')",
						layer.name, model
					));
				}
			}

			// Validate MCP configuration if enabled
			if !layer.mcp.server_refs.is_empty() && layer.mcp.server_refs.iter().all(|s| s.trim().is_empty()) {
				return Err(anyhow!(
					"Layer '{}' has server_refs configured but all entries are empty",
					layer.name
				));
			}
		}

		// Check if layers are enabled globally by checking if any role has layers enabled
		let layers_enabled_somewhere = self.developer.config.enable_layers || self.assistant.config.enable_layers;
		
		if layer_count == 0 && layers_enabled_somewhere {
			eprintln!("Warning: Layers are enabled but no layer configurations are active");
		}

		Ok(())
	}

	pub fn load() -> Result<Self> {
		// Use the new system-wide config file path
		let config_path = crate::directories::get_config_file_path()?;

		if config_path.exists() {
			let config_str = fs::read_to_string(&config_path)
				.context(format!("Failed to read config from {}", config_path.display()))?;
			let mut config: Config = toml::from_str(&config_str)
				.context("Failed to parse TOML configuration")?;

			// Store the config path for potential future saving
			config.config_path = Some(config_path);

			// Initialize the configuration
			config.initialize_config();

			// Environment variables take precedence over config file values
			// Handle provider API keys from environment variables
			if let Ok(openrouter_key) = std::env::var("OPENROUTER_API_KEY") {
				config.providers.openrouter.api_key = Some(openrouter_key);
			}
			if let Ok(openai_key) = std::env::var("OPENAI_API_KEY") {
				config.providers.openai.api_key = Some(openai_key);
			}
			if let Ok(anthropic_key) = std::env::var("ANTHROPIC_API_KEY") {
				config.providers.anthropic.api_key = Some(anthropic_key);
			}
			if let Ok(google_credentials) = std::env::var("GOOGLE_APPLICATION_CREDENTIALS") {
				config.providers.google.api_key = Some(google_credentials);
			}
			if let Ok(amazon_key) = std::env::var("AWS_ACCESS_KEY_ID") {
				config.providers.amazon.api_key = Some(amazon_key);
			}
			if let Ok(cloudflare_key) = std::env::var("CLOUDFLARE_API_TOKEN") {
				config.providers.cloudflare.api_key = Some(cloudflare_key);
			}

			// Legacy environment variable support for backward compatibility
			if let Ok(openrouter_key) = std::env::var("OPENROUTER_API_KEY") {
				config.openrouter.api_key = Some(openrouter_key);
			}

			// Validate the loaded configuration
			if let Err(e) = config.validate() {
				eprintln!("Configuration validation warning: {}", e);
				eprintln!("The application will continue, but you may want to fix these issues.");
			}

			Ok(config)
		} else {
			// Create default config with system-wide path
			let mut config = Config {
				config_path: Some(config_path),
				providers: ProvidersConfig {
					openrouter: ProviderConfig {
						api_key: std::env::var("OPENROUTER_API_KEY").ok(),
						..Default::default()
					},
					openai: ProviderConfig {
						api_key: std::env::var("OPENAI_API_KEY").ok(),
						..Default::default()
					},
					anthropic: ProviderConfig {
						api_key: std::env::var("ANTHROPIC_API_KEY").ok(),
						..Default::default()
					},
					google: ProviderConfig {
						api_key: std::env::var("GOOGLE_APPLICATION_CREDENTIALS").ok(),
						..Default::default()
					},
					amazon: ProviderConfig {
						api_key: std::env::var("AWS_ACCESS_KEY_ID").ok(),
						..Default::default()
					},
					cloudflare: ProviderConfig {
						api_key: std::env::var("CLOUDFLARE_API_TOKEN").ok(),
						..Default::default()
					},
				},
				openrouter: OpenRouterConfig {
					api_key: std::env::var("OPENROUTER_API_KEY").ok(),
					..Default::default()
				},
				..Default::default()
			};

			// CRITICAL FIX: Initialize the configuration when no file exists
			config.initialize_config();

			Ok(config)
		}
	}

	pub fn save(&self) -> Result<()> {
		// Validate before saving
		self.validate()?;

		// Use the stored config path, or fallback to system-wide default
		let config_path = if let Some(path) = &self.config_path {
			path.clone()
		} else {
			crate::directories::get_config_file_path()?
		};

		let config_str = toml::to_string(self)
			.context("Failed to serialize configuration to TOML")?;
		fs::write(&config_path, config_str)
			.context(format!("Failed to write config to {}", config_path.display()))?;

		Ok(())
	}

	pub fn create_default_config() -> Result<PathBuf> {
		let config_path = crate::directories::get_config_file_path()?;

		if !config_path.exists() {
			let mut config = Config::default();
			
			// CRITICAL FIX: Initialize the configuration before saving defaults
			config.initialize_config();
			
			let config_str = toml::to_string(&config)
				.context("Failed to serialize default configuration to TOML")?;

			fs::write(&config_path, config_str)
				.context(format!("Failed to write default config to {}", config_path.display()))?;

			println!("Created default configuration at {}", config_path.display());
		}

		Ok(config_path)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_valid_openrouter_models() {
		let mut config = Config::default();

		// Test valid models with proper provider:model format
		let valid_models = [
			"openrouter:anthropic/claude-3.5-haiku",
			"openrouter:anthropic/claude-3.5-sonnet",
			"openrouter:anthropic/claude-3.7-sonnet",
			"openrouter:anthropic/claude-sonnet-4",
			"openrouter:anthropic/claude-opus-4",
			"openrouter:openai/gpt-4o",
			"openrouter:openai/gpt-4o-mini",
			"openrouter:openai/gpt-4.1",
			"openrouter:openai/gpt-4.1-mini",
			"openrouter:openai/gpt-4.1-nano",
			"openrouter:openai/o4-mini",
			"openrouter:openai/o4-mini-high",
			"openrouter:google/gemini-2.5-flash-preview",
			"openrouter:google/gemini-2.5-pro-preview",
			"openai:gpt-4o",
			"openai:gpt-4o-mini",
			"openai:gpt-3.5-turbo",
			"openai:o1-preview",
			"openai:o1-mini",
			"anthropic:claude-3-5-sonnet",
			"anthropic:claude-3-5-haiku",
			"anthropic:claude-3-opus",
			"google:gemini-1.5-pro",
			"google:gemini-1.5-flash",
			"amazon:anthropic.claude-3-5-sonnet-20241022-v2:0",
			"amazon:anthropic.claude-3-5-haiku-20241022-v1:0",
			"amazon:anthropic.claude-3-opus-20240229-v1:0",
			"amazon:meta.llama3-2-90b-instruct-v1:0",
			"cloudflare:@cf/meta/llama-3.1-8b-instruct",
			"cloudflare:@hf/thebloke/llama-2-13b-chat-awq",
		];

		for model in valid_models {
			config.openrouter.model = model.to_string();
			assert!(config.validate_openrouter_model().is_ok(), "Model {} should be valid", model);
		}
	}

	#[test]
	fn test_invalid_openrouter_models() {
		let mut config = Config::default();

		// Test invalid models (old format without provider prefix)
		let invalid_models = [
			"gpt-4",  // Missing provider prefix
			"anthropic/claude-3.5-sonnet",  // Old format
			"openai-gpt-4",  // Wrong separator
			"unknown:model",  // Unknown provider
			"",  // Empty string
			"openai:",  // Empty model
			":gpt-4o",  // Empty provider
		];

		for model in invalid_models {
			config.openrouter.model = model.to_string();
			assert!(config.validate_openrouter_model().is_err(), "Model {} should be invalid", model);
		}
	}

	#[test]
	fn test_threshold_validation() {
		// Test invalid thresholds using system-wide settings
		let config = Config {
			mcp_response_warning_threshold: 0,
			..Default::default()
		};
		assert!(config.validate_thresholds().is_err());

		let config = Config {
			mcp_response_warning_threshold: 1000,
			cache_tokens_pct_threshold: 101,
			..Default::default()
		};
		assert!(config.validate_thresholds().is_err());

		// Test valid thresholds
		let config = Config {
			mcp_response_warning_threshold: 1000,
			cache_tokens_pct_threshold: 50,
			..Default::default()
		};
		assert!(config.validate_thresholds().is_ok());
	}

	#[test]
	fn test_environment_variable_precedence() {
		// This test would need to be run with specific environment variables set
		// For now, just test that the load function doesn't panic
		let result = Config::load();
		assert!(result.is_ok());
	}

	#[test]
	fn test_role_specific_cache_config() {
		let config = Config {
			cache_tokens_absolute_threshold: 4096,
			cache_timeout_seconds: 300,
			openrouter: OpenRouterConfig {
				cache_tokens_absolute_threshold: 0,
				cache_timeout_seconds: 180,
				..Default::default()
			},
			..Default::default()
		};

		// Test developer role merged config - should use system-wide settings
		let developer_merged = config.get_merged_config_for_mode("developer");
		assert_eq!(developer_merged.openrouter.cache_tokens_absolute_threshold, 4096);
		assert_eq!(developer_merged.openrouter.cache_timeout_seconds, 300);

		// Test assistant role merged config - should also use system-wide settings
		let assistant_merged = config.get_merged_config_for_mode("assistant");
		assert_eq!(assistant_merged.openrouter.cache_tokens_absolute_threshold, 4096);
		assert_eq!(assistant_merged.openrouter.cache_timeout_seconds, 300);

		// Test unknown role falls back to assistant but still uses system-wide settings
		let unknown_merged = config.get_merged_config_for_mode("unknown");
		assert_eq!(unknown_merged.openrouter.cache_tokens_absolute_threshold, 4096);
		assert_eq!(unknown_merged.openrouter.cache_timeout_seconds, 300);
	}
}
