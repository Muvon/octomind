use anyhow::{Result, Context, anyhow};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

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
	pub fn is_info_enabled(&self) -> bool {
		matches!(self, LogLevel::Info | LogLevel::Debug)
	}

	pub fn is_debug_enabled(&self) -> bool {
		matches!(self, LogLevel::Debug)
	}
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

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PricingConfig {
	#[serde(default = "default_input_price")]
	pub input_price: f64,
	#[serde(default = "default_output_price")]
	pub output_price: f64,
}

fn default_input_price() -> f64 {
	0.000001
}

fn default_output_price() -> f64 {
	0.000002
}

impl Default for PricingConfig {
	fn default() -> Self {
		Self {
			input_price: default_input_price(),
			output_price: default_output_price(),
		}
	}
}

// System defaults
fn default_system_model() -> String {
	"openrouter:anthropic/claude-3.5-haiku".to_string()
}

fn default_mcp_response_warning_threshold() -> usize {
	20000
}

fn default_max_request_tokens_threshold() -> usize {
	50000
}

fn default_cache_tokens_pct_threshold() -> u8 {
	40
}

fn default_cache_timeout_seconds() -> u64 {
	180
}

// Role configuration - simplified
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RoleConfig {
	/// Model in provider:model format
	#[serde(default = "default_system_model")]
	pub model: String,
	/// Enable layered processing
	#[serde(default)]
	pub enable_layers: bool,
	/// Custom system prompt
	pub system: Option<String>,
	/// MCP configuration
	#[serde(default)]
	pub mcp: McpConfig,
}

impl Default for RoleConfig {
	fn default() -> Self {
		Self {
			model: default_system_model(),
			enable_layers: false,
			system: None,
			mcp: McpConfig::default(),
		}
	}
}

// MCP Configuration - simplified
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub enum McpServerType {
	#[serde(rename = "external")]
	External,
	#[serde(rename = "developer")]
	Developer,
	#[serde(rename = "filesystem")]
	Filesystem,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub enum McpServerMode {
	#[serde(rename = "http")]
	Http,
	#[serde(rename = "stdin")]
	Stdin,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct McpServerConfig {
	#[serde(default)]
	pub enabled: bool,
	#[serde(skip)]
	pub name: String,
	#[serde(skip)]
	pub server_type: McpServerType,
	pub url: Option<String>,
	pub auth_token: Option<String>,
	pub command: Option<String>,
	#[serde(default)]
	pub args: Vec<String>,
	#[serde(default)]
	pub mode: McpServerMode,
	#[serde(default = "default_timeout")]
	pub timeout_seconds: u64,
	#[serde(default)]
	pub tools: Vec<String>,
}

fn default_timeout() -> u64 {
	30
}

impl Default for McpServerConfig {
	fn default() -> Self {
		Self {
			enabled: true,
			name: "".to_string(),
			server_type: McpServerType::External,
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

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
pub struct McpConfig {
	#[serde(default)]
	pub enabled: bool,
	#[serde(default)]
	pub servers: std::collections::HashMap<String, McpServerConfig>,
	#[serde(default)]
	pub allowed_tools: Vec<String>,
}

impl McpConfig {
	pub fn has_enabled_servers(&self) -> bool {
		self.enabled && self.servers.values().any(|server| server.enabled)
	}

	pub fn get_enabled_servers(&self) -> Vec<McpServerConfig> {
		if !self.enabled {
			return Vec::new();
		}

		self.servers.iter()
			.filter(|(_, config)| config.enabled)
			.map(|(name, config)| {
				let mut server = config.clone();
				server.name = name.clone();
				server.server_type = match name.as_str() {
					"developer" => McpServerType::Developer,
					"filesystem" => McpServerType::Filesystem,
					_ => McpServerType::External,
				};
				server
			})
			.collect()
	}
}

// Clean, simplified main config
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct Config {
	/// Root-level model setting (used by all commands if specified)
	#[serde(default = "default_system_model")]
	pub model: String,

	/// Root-level log level
	#[serde(default)]
	pub log_level: LogLevel,

	/// Provider configurations (API keys, etc.)
	#[serde(default)]
	pub providers: ProvidersConfig,

	/// Role-specific configurations
	#[serde(default)]
	pub developer: RoleConfig,
	#[serde(default)]
	pub assistant: RoleConfig,

	/// System-wide settings
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

	#[serde(skip)]
	config_path: Option<PathBuf>,
}

impl Default for RoleConfig {
	fn default() -> Self {
		Self {
			model: default_system_model(),
			enable_layers: false,
			system: None,
			mcp: McpConfig::default(),
		}
	}
}

impl Config {
	/// Get the effective model to use - checks root config, then falls back to system default
	pub fn get_effective_model(&self) -> String {
		// If root-level model is set and different from default, use it
		if !self.model.is_empty() && self.model != default_system_model() {
			return self.model.clone();
		}
		
		// Otherwise, use the system default
		default_system_model()
	}

	/// Get configuration for a specific role
	pub fn get_role_config(&self, role: &str) -> &RoleConfig {
		match role {
			"developer" => &self.developer,
			"assistant" => &self.assistant,
			_ => &self.assistant, // Default fallback
		}
	}

	/// Get model for a specific role (for backward compatibility)
	pub fn get_model(&self, role: &str) -> String {
		// Always use the effective model for consistency
		self.get_effective_model()
	}

	/// Get enable layers setting for a role
	pub fn get_enable_layers(&self, role: &str) -> bool {
		self.get_role_config(role).enable_layers
	}

	/// Get log level
	pub fn get_log_level(&self) -> LogLevel {
		self.log_level.clone()
	}

	/// System-wide getters
	pub fn get_cache_timeout_seconds(&self) -> u64 {
		if self.cache_timeout_seconds != 0 {
			self.cache_timeout_seconds
		} else {
			default_cache_timeout_seconds()
		}
	}

	pub fn get_cache_tokens_absolute_threshold(&self) -> u64 {
		self.cache_tokens_absolute_threshold
	}

	pub fn get_cache_tokens_pct_threshold(&self) -> u8 {
		if self.cache_tokens_pct_threshold != 0 {
			self.cache_tokens_pct_threshold
		} else {
			default_cache_tokens_pct_threshold()
		}
	}

	pub fn get_mcp_response_warning_threshold(&self) -> usize {
		if self.mcp_response_warning_threshold != 0 {
			self.mcp_response_warning_threshold
		} else {
			default_mcp_response_warning_threshold()
		}
	}

	pub fn get_enable_auto_truncation(&self) -> bool {
		self.enable_auto_truncation
	}

	pub fn get_max_request_tokens_threshold(&self) -> usize {
		if self.max_request_tokens_threshold != 0 {
			self.max_request_tokens_threshold
		} else {
			default_max_request_tokens_threshold()
		}
	}

	pub fn get_enable_markdown_rendering(&self) -> bool {
		self.enable_markdown_rendering
	}

	pub fn load() -> Result<Self> {
		let config_path = crate::directories::get_config_file_path()?;

		if config_path.exists() {
			let config_str = fs::read_to_string(&config_path)
				.context(format!("Failed to read config from {}", config_path.display()))?;
			let mut config: Config = toml::from_str(&config_str)
				.context("Failed to parse TOML configuration")?;

			config.config_path = Some(config_path);

			// Load API keys from environment variables
			if let Ok(key) = std::env::var("OPENROUTER_API_KEY") {
				config.providers.openrouter.api_key = Some(key);
			}
			if let Ok(key) = std::env::var("OPENAI_API_KEY") {
				config.providers.openai.api_key = Some(key);
			}
			if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
				config.providers.anthropic.api_key = Some(key);
			}
			if let Ok(key) = std::env::var("GOOGLE_APPLICATION_CREDENTIALS") {
				config.providers.google.api_key = Some(key);
			}
			if let Ok(key) = std::env::var("AWS_ACCESS_KEY_ID") {
				config.providers.amazon.api_key = Some(key);
			}
			if let Ok(key) = std::env::var("CLOUDFLARE_API_TOKEN") {
				config.providers.cloudflare.api_key = Some(key);
			}

			Ok(config)
		} else {
			// Create default config
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
				..Default::default()
			};

			Ok(config)
		}
	}

	pub fn save(&self) -> Result<()> {
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

// Thread-local config for logging macros
use std::cell::RefCell;

thread_local! {
	static CURRENT_CONFIG: RefCell<Option<Config>> = const { RefCell::new(None) };
}

pub fn set_thread_config(config: &Config) {
	CURRENT_CONFIG.with(|c| {
		*c.borrow_mut() = Some(config.clone());
	});
}

pub fn with_thread_config<F, R>(f: F) -> Option<R>
where
	F: FnOnce(&Config) -> R,
{
	CURRENT_CONFIG.with(|c| {
		(*c.borrow()).as_ref().map(f)
	})
}

// Logging macros
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