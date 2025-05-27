use anyhow::{Result, Context, anyhow};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

// Type alias to simplify the complex return type for get_mode_config
type ModeConfigResult<'a> = (
	&'a ModeConfig,
	McpConfig,
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

	/// Convert the old debug boolean to LogLevel for backward compatibility
	pub fn from_debug_flag(debug: bool) -> Self {
		if debug { LogLevel::Debug } else { LogLevel::None }
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
	#[serde(default)]
	pub enabled: bool,
	
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
			enabled: true,
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
			enabled: true,
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
			enabled: true,
			name: name.to_string(),
			server_type: McpServerType::Developer,
			tools,
			..Default::default()
		}
	}

	/// Create a filesystem server configuration
	pub fn filesystem(name: &str, tools: Vec<String>) -> Self {
		Self {
			enabled: true,
			name: name.to_string(),
			server_type: McpServerType::Filesystem,
			tools,
			..Default::default()
		}
	}

	/// Create an external HTTP server configuration
	pub fn external_http(name: &str, url: &str, tools: Vec<String>) -> Self {
		Self {
			enabled: true,
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
			enabled: true,
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

impl McpConfig {
	/// Check if MCP has any enabled servers
	pub fn has_enabled_servers(&self) -> bool {
		self.enabled && self.servers.values().any(|server| server.enabled)
	}

	/// Get all enabled servers with auto-detected types
	pub fn get_enabled_servers(&self) -> Vec<McpServerConfig> {
		if !self.enabled {
			return Vec::new();
		}

		let mut result = Vec::new();

		// Add servers from registry
		for (server_name, server_config) in &self.servers {
			if server_config.enabled {
				let mut server = server_config.clone();
				// Auto-set the name from the registry key
				server.name = server_name.clone();
				// Auto-detect server type from name
				server.server_type = match server_name.as_str() {
					"developer" => McpServerType::Developer,
					"filesystem" => McpServerType::Filesystem,
					_ => McpServerType::External,
				};
				// Apply global tool filtering if specified
				if !self.allowed_tools.is_empty() {
					server.tools = self.allowed_tools.clone();
				}
				result.push(server);
			}
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
	pub mcp: McpConfig,
	// Layer configuration
	#[serde(default)]
	pub layers: Option<Vec<crate::session::layers::LayerConfig>>,
	// Command layer configurations
	#[serde(default)]
	pub commands: Option<std::collections::HashMap<String, crate::session::layers::LayerConfig>>,
	// Legacy openrouter field for backward compatibility
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub openrouter: Option<OpenRouterConfig>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AssistantRoleConfig {
	#[serde(flatten)]
	pub config: ModeConfig,
	#[serde(default)]
	pub mcp: McpConfig,
	// Command layer configurations
	#[serde(default)]
	pub commands: Option<std::collections::HashMap<String, crate::session::layers::LayerConfig>>,
	// Legacy openrouter field for backward compatibility
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub openrouter: Option<OpenRouterConfig>,
}

impl Default for DeveloperRoleConfig {
	fn default() -> Self {
		// Create default MCP config with built-in servers
		let mut mcp_servers = std::collections::HashMap::new();
		
		// Add built-in servers
		mcp_servers.insert(
			"developer".to_string(),
			McpServerConfig::developer("developer", vec![])
		);
		mcp_servers.insert(
			"filesystem".to_string(),
			McpServerConfig::filesystem("filesystem", vec![])
		);
		
		// Add octocode server with auto-detection
		let octocode_available = {
			use std::process::Command;
			match Command::new("octocode").arg("--version").output() {
				Ok(output) => output.status.success(),
				Err(_) => false,
			}
		};
		
		mcp_servers.insert(
			"octocode".to_string(),
			McpServerConfig {
				enabled: octocode_available,
				name: "octocode".to_string(),
				server_type: McpServerType::External,
				command: Some("octocode".to_string()),
				args: vec!["mcp".to_string(), "--path=.".to_string()],
				mode: McpServerMode::Stdin,
				timeout_seconds: 30,
				tools: vec![], // Empty means all tools are enabled
				url: None,
				auth_token: None,
			}
		);
		
		Self {
			config: ModeConfig {
				model: "openrouter:anthropic/claude-sonnet-4".to_string(),
				enable_layers: true,
				system: Some("You are an Octodev AI developer assistant with full access to development tools.".to_string()),
			},
			mcp: McpConfig {
				enabled: true,
				servers: mcp_servers,
				allowed_tools: vec![],
			},
			layers: None,
			commands: None,
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
			mcp: McpConfig {
				enabled: false,  // Assistant role has MCP/tools disabled by default
				..McpConfig::default()
			},
			commands: None,
			openrouter: None,
		}
	}
}

// Legacy mode configurations for backward compatibility
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AgentModeConfig {
	#[serde(flatten)]
	pub config: ModeConfig,
	#[serde(default)]
	pub mcp: McpConfig,
	#[serde(default)]
	pub layers: Option<Vec<crate::session::layers::LayerConfig>>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub openrouter: Option<OpenRouterConfig>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ChatModeConfig {
	#[serde(flatten)]
	pub config: ModeConfig,
	#[serde(default)]
	pub mcp: McpConfig,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub openrouter: Option<OpenRouterConfig>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct Config {
	// Root-level log level setting (takes precedence over role-specific)
	#[serde(default)]
	pub log_level: LogLevel,

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
		// Check if developer role MCP config has octocode server
		if let Some(octocode_server) = self.developer.mcp.servers.get_mut("octocode") {
			// If the config doesn't explicitly set the enabled status, auto-detect
			if !octocode_server.enabled {
				let available = Self::is_octocode_available();
				octocode_server.enabled = available;
				
				if available {
					crate::log_info!("Auto-enabled octocode MCP server (binary detected in PATH)");
				} else {
					crate::log_debug!("octocode binary not found in PATH, server remains disabled");
				}
			}
		}
		
		// Also check global MCP config as fallback
		if let Some(octocode_server) = self.mcp.servers.get_mut("octocode") {
			if !octocode_server.enabled {
				let available = Self::is_octocode_available();
				octocode_server.enabled = available;
			}
		}
	}

	/// Initialize the default server registry with auto-detection
	pub fn init_default_server_registry(&mut self) {
		// Initialize default servers for developer role if empty
		if self.developer.mcp.servers.is_empty() {
			let mut mcp_servers = std::collections::HashMap::new();
			
			// Add built-in servers
			mcp_servers.insert(
				"developer".to_string(),
				McpServerConfig::from_name("developer")
			);
			mcp_servers.insert(
				"filesystem".to_string(),
				McpServerConfig::from_name("filesystem")
			);
			
			// Add octocode server with auto-detection
			let octocode_available = Self::is_octocode_available();
			mcp_servers.insert(
				"octocode".to_string(),
				McpServerConfig {
					enabled: octocode_available,
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
			
			self.developer.mcp.servers = mcp_servers;
			
			if octocode_available {
				crate::log_info!("Auto-configured octocode MCP server (binary detected)");
			}
		}

		// Initialize global MCP servers if empty
		if self.mcp.servers.is_empty() {
			self.mcp.servers.insert(
				"developer".to_string(),
				McpServerConfig::from_name("developer")
			);
			self.mcp.servers.insert(
				"filesystem".to_string(),
				McpServerConfig::from_name("filesystem")
			);
			
			// Add octocode to global config too
			let octocode_available = Self::is_octocode_available();
			self.mcp.servers.insert(
				"octocode".to_string(),
				McpServerConfig {
					enabled: octocode_available,
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

	/// Get resolved MCP config for a role (merges server_refs with registry)
	pub fn get_resolved_mcp_config(&self, mcp_config: &McpConfig) -> McpConfig {
		// Always resolve server references from registry

		// Note: In the clean implementation, we expect all configs to use server_refs
		// No fallback to direct server configurations
		mcp_config.clone()
	}
	/// Get the global log level (system-wide setting)
	pub fn get_log_level(&self) -> LogLevel {
		// If root log level is set, use it
		if self.log_level != LogLevel::None {
			return self.log_level.clone();
		}

		// Otherwise, fall back to openrouter config for backward compatibility
		self.openrouter.log_level.clone()
	}

	/// System-wide configuration getters - these settings are global and not role-specific
	/// Get cache timeout seconds (system-wide setting)
	pub fn get_cache_timeout_seconds(&self) -> u64 {
		// If system setting is set (non-zero), use it
		if self.cache_timeout_seconds != 0 {
			return self.cache_timeout_seconds;
		}

		// Otherwise, fall back to openrouter config for backward compatibility
		self.openrouter.cache_timeout_seconds
	}

	/// Get cache tokens absolute threshold (system-wide setting)
	pub fn get_cache_tokens_absolute_threshold(&self) -> u64 {
		// If system setting is set (non-zero), use it
		if self.cache_tokens_absolute_threshold != 0 {
			return self.cache_tokens_absolute_threshold;
		}

		// Otherwise, fall back to openrouter config for backward compatibility
		self.openrouter.cache_tokens_absolute_threshold
	}

	/// Get cache tokens percentage threshold (system-wide setting)
	pub fn get_cache_tokens_pct_threshold(&self) -> u8 {
		// If system setting is set (non-zero), use it
		if self.cache_tokens_pct_threshold != 0 {
			return self.cache_tokens_pct_threshold;
		}

		// Otherwise, fall back to openrouter config for backward compatibility
		self.openrouter.cache_tokens_pct_threshold
	}

	/// Get MCP response warning threshold (system-wide setting)
	pub fn get_mcp_response_warning_threshold(&self) -> usize {
		// If system setting is set (non-zero), use it
		if self.mcp_response_warning_threshold != 0 {
			return self.mcp_response_warning_threshold;
		}

		// Otherwise, fall back to openrouter config for backward compatibility
		self.openrouter.mcp_response_warning_threshold
	}

	/// Get enable auto truncation setting (system-wide setting)
	pub fn get_enable_auto_truncation(&self) -> bool {
		// For boolean, we check if system setting differs from default (false)
		// If it's explicitly set to true, use it; otherwise fall back to openrouter
		if self.enable_auto_truncation {
			return true;
		}

		// Otherwise, fall back to openrouter config for backward compatibility
		self.openrouter.enable_auto_truncation
	}

	/// Get max request tokens threshold (system-wide setting)
	pub fn get_max_request_tokens_threshold(&self) -> usize {
		// If system setting is set (non-zero), use it
		if self.max_request_tokens_threshold != 0 {
			return self.max_request_tokens_threshold;
		}

		// Otherwise, fall back to openrouter config for backward compatibility
		self.openrouter.max_request_tokens_threshold
	}

	/// Get enable markdown rendering setting (system-wide setting)
	pub fn get_enable_markdown_rendering(&self) -> bool {
		// For boolean, we check if system setting differs from default (false)
		// If it's explicitly set to true, use it; otherwise fall back to openrouter
		if self.enable_markdown_rendering {
			return true;
		}

		// Otherwise, fall back to openrouter config for backward compatibility
		self.openrouter.enable_markdown_rendering
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

	/// Backward compatibility methods - these delegate to openrouter config for now
	/// but should eventually be deprecated in favor of role-based methods
	/// Get cache timeout seconds (backward compatibility)
	pub fn get_cache_timeout_seconds_legacy(&self) -> u64 {
		self.openrouter.cache_timeout_seconds
	}

	/// Get cache tokens absolute threshold (backward compatibility)
	pub fn get_cache_tokens_absolute_threshold_legacy(&self) -> u64 {
		self.openrouter.cache_tokens_absolute_threshold
	}

	/// Get cache tokens percentage threshold (backward compatibility)
	pub fn get_cache_tokens_pct_threshold_legacy(&self) -> u8 {
		self.openrouter.cache_tokens_pct_threshold
	}

	/// Get MCP response warning threshold (backward compatibility)
	pub fn get_mcp_response_warning_threshold_legacy(&self) -> usize {
		self.openrouter.mcp_response_warning_threshold
	}

	/// Get enable auto truncation setting (backward compatibility)
	pub fn get_enable_auto_truncation_legacy(&self) -> bool {
		self.openrouter.enable_auto_truncation
	}

	/// Get max request tokens threshold (backward compatibility)
	pub fn get_max_request_tokens_threshold_legacy(&self) -> usize {
		self.openrouter.max_request_tokens_threshold
	}

	/// Get enable markdown rendering setting (backward compatibility)
	pub fn get_enable_markdown_rendering_legacy(&self) -> bool {
		self.openrouter.enable_markdown_rendering
	}

	/// Get enable layers setting (backward compatibility)
	pub fn get_enable_layers_legacy(&self) -> bool {
		self.openrouter.enable_layers
	}
	/// Check if MCP config is "empty" (using only defaults) - then we should fallback to global
	fn is_mcp_config_empty(&self, mcp_config: &McpConfig) -> bool {
		// A config is considered "empty" if:
		// 1. It has no servers configured, AND
		// 2. It has no allowed_tools configured, AND  
		// 3. Either it's disabled OR it has enabled=true but no actual server configurations
		
		// If allowed_tools are customized, it's not empty
		if !mcp_config.allowed_tools.is_empty() {
			return false;
		}

		// If servers are configured with actual server definitions, it's not empty
		// Check if any server has meaningful configuration beyond just being enabled
		for server_config in mcp_config.servers.values() {
			// If the server has specific tool filtering, URL, command, or other config, it's not empty
			if !server_config.tools.is_empty() || 
			   server_config.url.is_some() || 
			   server_config.command.is_some() ||
			   server_config.timeout_seconds != 30 { // non-default timeout
				return false;
			}
		}

		// If servers list is empty OR only contains default server stubs, consider it empty
		// This handles the case where [role.mcp.servers] exists but is empty or contains only defaults
		if mcp_config.servers.is_empty() {
			return true;
		}

		// If we get here, the config has servers but they're all using default configurations
		// This is the key fix: treat minimal server configs as "empty" for inheritance
		true
	}

	/// Get configuration for a specific role with proper fallback logic and role inheritance
	/// Returns: (mode_config, mcp_config, layers, commands, system_prompt)
	/// Role inheritance: any role inherits from 'assistant' first, then applies its own overrides
	pub fn get_mode_config(&self, role: &str) -> ModeConfigResult<'_> {
		match role {
			"developer" => {
				// Developer role - inherits from assistant but with developer-specific config
				let mut mcp_config = self.developer.mcp.clone();

				// If developer.mcp is "empty/default", fall back to global mcp
				if self.is_mcp_config_empty(&mcp_config) {
					mcp_config = self.mcp.clone();
				}

				// Get commands config - prefer role-specific, fallback to global
				let commands_config = self.developer.commands.as_ref()
					.or(self.commands.as_ref());

				(&self.developer.config, mcp_config, self.developer.layers.as_ref(), commands_config, self.developer.config.system.as_ref())
			},
			"assistant" => {
				// Base assistant role
				let mut mcp_config = self.assistant.mcp.clone();

				// If assistant.mcp is "empty/default", fall back to global mcp
				if self.is_mcp_config_empty(&mcp_config) {
					mcp_config = self.mcp.clone();
				}

				// Get commands config - prefer role-specific, fallback to global
				let commands_config = self.assistant.commands.as_ref()
					.or(self.commands.as_ref());

				(&self.assistant.config, mcp_config, None, commands_config, self.assistant.config.system.as_ref())
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
		let (mode_config, mcp_config, layers_config, commands_config, system_prompt) = self.get_mode_config(mode);

		let mut merged = self.clone();

		// Create an OpenRouterConfig from the ModeConfig for backward compatibility
		merged.openrouter = OpenRouterConfig {
			model: mode_config.get_full_model(),
			api_key: mode_config.get_api_key(&self.providers),
			pricing: mode_config.get_pricing(&self.providers),
			enable_layers: mode_config.enable_layers,
			log_level: self.get_log_level(), // Use global log level
			// Use system-wide settings for these configuration options
			mcp_response_warning_threshold: self.get_mcp_response_warning_threshold(),
			max_request_tokens_threshold: self.get_max_request_tokens_threshold(),
			enable_auto_truncation: self.get_enable_auto_truncation(),
			cache_tokens_pct_threshold: self.get_cache_tokens_pct_threshold(),
			cache_tokens_absolute_threshold: self.get_cache_tokens_absolute_threshold(),
			cache_timeout_seconds: self.get_cache_timeout_seconds(),
			enable_markdown_rendering: self.get_enable_markdown_rendering(),
		};

		// Resolve MCP configuration using the new registry system
		merged.mcp = self.get_resolved_mcp_config(&mcp_config);
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
		// Use system-wide configuration getters
		let mcp_warning_threshold = self.get_mcp_response_warning_threshold();
		let max_request_threshold = self.get_max_request_tokens_threshold();
		let cache_pct_threshold = self.get_cache_tokens_pct_threshold();

		if mcp_warning_threshold == 0 {
			return Err(anyhow!("MCP response warning threshold must be greater than 0"));
		}

		if max_request_threshold == 0 {
			return Err(anyhow!("Max request tokens threshold must be greater than 0"));
		}

		if cache_pct_threshold > 100 {
			return Err(anyhow!("Cache tokens percentage threshold must be between 0-100"));
		}

		// Warn if thresholds seem too low
		if mcp_warning_threshold < 1000 {
			eprintln!("Warning: MCP response warning threshold ({}) is quite low", mcp_warning_threshold);
		}

		Ok(())
	}

	fn validate_mcp_config(&self) -> Result<()> {
		// Helper function to validate a single MCP config
		let validate_mcp = |mcp_config: &McpConfig, _context: &str| -> Result<()> {
			if !mcp_config.enabled {
				return Ok(());
			}

			// For role-specific configs, they can be empty and inherit from global
			// Only validate if they have explicit server configurations
			if !mcp_config.servers.is_empty() {
				// If they specify servers, validate that they exist
				// But we don't require them to specify servers since they can inherit from global
			}

			Ok(())
		};

		// Validate global MCP config - this one MUST have servers if enabled
		if self.mcp.enabled && self.mcp.servers.is_empty() {
			return Err(anyhow!("Global MCP config: MCP is enabled but no servers specified"));
		}

		// Validate role-specific MCP configs (they can inherit from global)
		validate_mcp(&self.developer.mcp, "Developer role MCP config")?;
		validate_mcp(&self.assistant.mcp, "Assistant role MCP config")?;

		// Validate server configurations
		for (name, server) in &self.mcp.servers {
			if server.enabled {
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
		}

		Ok(())
	}

	fn validate_layers(&self, layers: &[crate::session::layers::LayerConfig]) -> Result<()> {
		let mut enabled_count = 0;
		let mut names = std::collections::HashSet::new();

		for layer in layers {
			if layer.enabled {
				enabled_count += 1;

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
				if layer.mcp.enabled && layer.mcp.servers.is_empty() {
					return Err(anyhow!(
						"Layer '{}' has MCP enabled but no servers specified",
						layer.name
					));
				}
			}
		}

		// Check if layers are enabled globally by checking if any role has layers enabled
		let layers_enabled_somewhere = self.developer.config.enable_layers || self.assistant.config.enable_layers;
		
		if enabled_count == 0 && layers_enabled_somewhere {
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
			let config = Config {
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
			let config = Config::default();
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
		let mut config = Config::default();

		// Test invalid thresholds using system-wide settings
		config.mcp_response_warning_threshold = 0;
		assert!(config.validate_thresholds().is_err());

		config.mcp_response_warning_threshold = 1000;
		config.cache_tokens_pct_threshold = 101;
		assert!(config.validate_thresholds().is_err());

		// Test valid thresholds
		config.cache_tokens_pct_threshold = 50;
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
