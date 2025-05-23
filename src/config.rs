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
	static CURRENT_CONFIG: RefCell<Option<Config>> = RefCell::new(None);
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
		if let Some(ref config) = *c.borrow() {
			Some(f(config))
		} else {
			None
		}
	})
}

/// Info logging macro with automatic cyan coloring
/// Shows info messages when log level is Info OR Debug
#[macro_export]
macro_rules! log_info {
	($fmt:expr) => {
		if let Some(should_log) = $crate::config::with_thread_config(|config| config.openrouter.log_level.is_info_enabled()) {
			if should_log {
				use colored::Colorize;
				println!("{}", $fmt.cyan());
			}
		}
	};
	($fmt:expr, $($arg:expr),*) => {
		if let Some(should_log) = $crate::config::with_thread_config(|config| config.openrouter.log_level.is_info_enabled()) {
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
		if let Some(should_log) = $crate::config::with_thread_config(|config| config.openrouter.log_level.is_debug_enabled()) {
			if should_log {
				use colored::Colorize;
				println!("{}", $fmt.bright_blue());
			}
		}
	};
	($fmt:expr, $($arg:expr),*) => {
		if let Some(should_log) = $crate::config::with_thread_config(|config| config.openrouter.log_level.is_debug_enabled()) {
			if should_log {
				use colored::Colorize;
				println!("{}", format!($fmt, $($arg),*).bright_blue());
			}
		}
	};
}

/// Conditional logging - prints different messages based on log level
#[macro_export]
macro_rules! log_conditional {
	(debug: $debug_msg:expr, info: $info_msg:expr, none: $none_msg:expr) => {
		if let Some(level) = $crate::config::with_thread_config(|config| config.openrouter.log_level.clone()) {
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
		if let Some(should_debug) = $crate::config::with_thread_config(|config| config.openrouter.log_level.is_debug_enabled()) {
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
		if let Some(should_info) = $crate::config::with_thread_config(|config| config.openrouter.log_level.is_info_enabled()) {
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

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub enum EmbeddingProvider {
	Jina,
	FastEmbed,
}

impl Default for EmbeddingProvider {
	fn default() -> Self {
		Self::FastEmbed // Default to FastEmbed
	}
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GraphRagConfig {
	#[serde(default)]
	pub enabled: bool,
	#[serde(default = "default_description_model")]
	pub description_model: String,
	#[serde(default = "default_relationship_model")]
	pub relationship_model: String,
}

fn default_description_model() -> String {
	"openai/gpt-4.1-nano".to_string()
}

fn default_relationship_model() -> String {
	"openai/gpt-4.1-nano".to_string()
}

impl Default for GraphRagConfig {
	fn default() -> Self {
		Self {
			enabled: false,
			description_model: default_description_model(),
			relationship_model: default_relationship_model(),
		}
	}
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FastEmbedConfig {
	#[serde(default = "default_code_model")]
	pub code_model: String,
	#[serde(default = "default_text_model")]
	pub text_model: String,
}

fn default_code_model() -> String {
	"all-MiniLM-L6-v2".to_string()
}

fn default_text_model() -> String {
	"all-MiniLM-L6-v2".to_string()
}

impl Default for FastEmbedConfig {
	fn default() -> Self {
		Self {
			code_model: default_code_model(),
			text_model: default_text_model(),
		}
	}
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct JinaConfig {
	#[serde(default = "default_jina_code_model")]
	pub code_model: String,
	#[serde(default = "default_jina_text_model")]
	pub text_model: String,
}

fn default_jina_code_model() -> String {
	"jina-embeddings-v2-base-code".to_string()
}

fn default_jina_text_model() -> String {
	"jina-embeddings-v3".to_string()
}

impl Default for JinaConfig {
	fn default() -> Self {
		Self {
			code_model: default_jina_code_model(),
			text_model: default_jina_text_model(),
		}
	}
}

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

impl Default for PricingConfig {
	fn default() -> Self {
		Self {
			input_price: default_input_price(),
			output_price: default_output_price(),
		}
	}
}

fn default_openrouter_model() -> String {
	"anthropic/claude-sonnet-4".to_string()
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
			enable_markdown_rendering: false, // Disabled by default
		}
	}
}

#[derive(Debug, Serialize, Deserialize, Clone)]
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

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct McpServerConfig {
	#[serde(default)]
	pub enabled: bool,
	pub name: String,
	// URL mode configuration (for remote servers)
	pub url: Option<String>,
	pub auth_token: Option<String>,
	// Local mode configuration (for running servers locally)
	pub command: Option<String>,
	#[serde(default)]
	pub args: Vec<String>,
	// Communication mode - http or stdin
	#[serde(default)]
	pub mode: McpServerMode,
	// Timeout in seconds for tool execution
	#[serde(default = "default_timeout")]
	pub timeout_seconds: u64,
	// Common config
	#[serde(default)]
	pub tools: Vec<String>,  // Empty means all tools are enabled
}

fn default_timeout() -> u64 {
	30 // Default timeout of 30 seconds
}

impl Default for McpServerConfig {
	fn default() -> Self {
		Self {
			enabled: true,
			name: "".to_string(),
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

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct McpConfig {
	#[serde(default)]
	pub enabled: bool,
	#[serde(default = "default_mcp_providers")]
	pub providers: Vec<String>,
	#[serde(default)]
	pub servers: Vec<McpServerConfig>,
}

fn default_mcp_providers() -> Vec<String> {
	vec!["core".to_string()]
}

impl Default for McpConfig {
	fn default() -> Self {
		Self {
			enabled: false,
			providers: default_mcp_providers(),
			servers: Vec::new(),
		}
	}
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Config {
	#[serde(default)]
	pub embedding_provider: EmbeddingProvider,
	#[serde(default)]
	pub fastembed: FastEmbedConfig,
	#[serde(default)]
	pub jina: JinaConfig,
	#[serde(default)]
	pub graphrag: GraphRagConfig,
	pub jina_api_key: Option<String>,
	#[serde(default)]
	pub openrouter: OpenRouterConfig,
	#[serde(default)]
	pub mcp: McpConfig,
	// Layer configuration
	// Note: To configure specific models for each layer, add them to this section
	// rather than using global model settings
	#[serde(default)]
	pub layers: Option<Vec<crate::session::layers::LayerConfig>>,
	// Custom system prompt (optional - falls back to default if not provided)
	pub system: Option<String>,
	#[serde(skip)]
	config_path: Option<PathBuf>,
}

impl Default for Config {
	fn default() -> Self {
		Self {
			embedding_provider: EmbeddingProvider::default(),
			fastembed: FastEmbedConfig::default(),
			jina: JinaConfig::default(),
			graphrag: GraphRagConfig::default(),
			jina_api_key: None,
			openrouter: OpenRouterConfig::default(),
			mcp: McpConfig::default(),
			layers: None, // No custom layer configs by default
			system: None, // No custom system prompt by default
			config_path: None,
		}
	}
}

impl Config {
	pub fn ensure_octodev_dir() -> Result<std::path::PathBuf> {
		let current_dir = std::env::current_dir()?;
		let octodev_dir = current_dir.join(".octodev");
		if !octodev_dir.exists() {
			fs::create_dir_all(&octodev_dir)?;
		}
		Ok(octodev_dir)
	}

	/// Validate the configuration for common issues
	pub fn validate(&self) -> Result<()> {
		// Validate OpenRouter model name
		self.validate_openrouter_model()?;

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

		// Check if model starts with a valid provider prefix
		let has_valid_prefix = model.contains('/') &&
		(model.starts_with("anthropic/") ||
		model.starts_with("openai/") ||
		model.starts_with("meta-llama/") ||
		model.starts_with("google/") ||
		model.starts_with("mistralai/") ||
		model.starts_with("perplexity/"));

		if !has_valid_prefix {
			return Err(anyhow!(
				"Invalid OpenRouter model format: '{}'. Expected format: 'provider/model-name'",
				model
			));
		}

		Ok(())
	}

	fn validate_thresholds(&self) -> Result<()> {
		let config = &self.openrouter;

		if config.mcp_response_warning_threshold == 0 {
			return Err(anyhow!("MCP response warning threshold must be greater than 0"));
		}

		if config.max_request_tokens_threshold == 0 {
			return Err(anyhow!("Max request tokens threshold must be greater than 0"));
		}

		if config.cache_tokens_pct_threshold > 100 {
			return Err(anyhow!("Cache tokens percentage threshold must be between 0-100"));
		}

		// Warn if thresholds seem too low
		if config.mcp_response_warning_threshold < 1000 {
			eprintln!("Warning: MCP response warning threshold ({}) is quite low",
				config.mcp_response_warning_threshold);
		}

		Ok(())
	}

	fn validate_mcp_config(&self) -> Result<()> {
		if !self.mcp.enabled {
			return Ok(());
		}

		for server in &self.mcp.servers {
			if server.enabled {
				// Must have either URL or command
				if server.url.is_none() && server.command.is_none() {
					return Err(anyhow!(
						"MCP server '{}' must have either 'url' or 'command' specified",
						server.name
					));
				}

				// Validate timeout
				if server.timeout_seconds == 0 {
					return Err(anyhow!(
						"MCP server '{}' timeout must be greater than 0",
						server.name
					));
				}

				// Validate server name
				if server.name.trim().is_empty() {
					return Err(anyhow!("MCP server name cannot be empty"));
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

				// Validate model format
				if layer.model.trim().is_empty() {
					return Err(anyhow!("Layer '{}' model cannot be empty", layer.name));
				}
			}
		}

		if enabled_count == 0 && self.openrouter.enable_layers {
			eprintln!("Warning: Layers are enabled but no layer configurations are active");
		}

		Ok(())
	}

	pub fn load() -> Result<Self> {
		let octodev_dir = Self::ensure_octodev_dir()?;
		let config_path = octodev_dir.join("config.toml");

		if config_path.exists() {
			let config_str = fs::read_to_string(&config_path)
				.context(format!("Failed to read config from {}", config_path.display()))?;
			let mut config: Config = toml::from_str(&config_str)
				.context("Failed to parse TOML configuration")?;

			// Store the config path for potential future saving
			config.config_path = Some(config_path);

			// Environment variables take precedence over config file values
			if let Ok(jina_key) = std::env::var("JINA_API_KEY") {
				config.jina_api_key = Some(jina_key);
			}
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
			// Create default config
			let mut config = Config::default();
			config.config_path = Some(config_path);

			// Check environment variables for API keys
			config.jina_api_key = std::env::var("JINA_API_KEY").ok();
			config.openrouter.api_key = std::env::var("OPENROUTER_API_KEY").ok();

			Ok(config)
		}
	}

	pub fn save(&self) -> Result<()> {
		// Validate before saving
		self.validate()?;

		let octodev_dir = Self::ensure_octodev_dir()?;
		let config_path = octodev_dir.join("config.toml");

		let config_str = toml::to_string(self)
			.context("Failed to serialize configuration to TOML")?;
		fs::write(&config_path, config_str)
			.context(format!("Failed to write config to {}", config_path.display()))?;

		Ok(())
	}

	pub fn create_default_config() -> Result<PathBuf> {
		let octodev_dir = Self::ensure_octodev_dir()?;
		let config_path = octodev_dir.join("config.toml");

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

		// Test valid models
		let valid_models = [
			"anthropic/claude-3.5-haiku",
			"anthropic/claude-3.5-sonnet",
			"anthropic/claude-3.7-sonnet",
			"anthropic/claude-sonnet-4",
			"anthropic/claude-opus-4",
			"openai/gpt-4o",
			"openai/gpt-4o-mini",
			"openai/gpt-4.1",
			"openai/gpt-4.1-mini",
			"openai/gpt-4.1-nano",
			"openai/o4-mini",
			"openai/o4-mini-high",
			"google/gemini-2.5-flash-preview",
			"google/gemini-2.5-pro-preview",
		];

		for model in valid_models {
			config.openrouter.model = model.to_string();
			assert!(config.validate_openrouter_model().is_ok(), "Model {} should be valid", model);
		}
	}

	#[test]
	fn test_invalid_openrouter_models() {
		let mut config = Config::default();

		// Test invalid models
		let invalid_models = [
			"gpt-4",  // Missing provider prefix
			"openai-gpt-4",  // Wrong separator
			"unknown/model",  // Unknown provider
			"",  // Empty string
		];

		for model in invalid_models {
			config.openrouter.model = model.to_string();
			assert!(config.validate_openrouter_model().is_err(), "Model {} should be invalid", model);
		}
	}

	#[test]
	fn test_threshold_validation() {
		let mut config = Config::default();

		// Test invalid thresholds
		config.openrouter.mcp_response_warning_threshold = 0;
		assert!(config.validate_thresholds().is_err());

		config.openrouter.mcp_response_warning_threshold = 1000;
		config.openrouter.cache_tokens_pct_threshold = 101;
		assert!(config.validate_thresholds().is_err());

		// Test valid thresholds
		config.openrouter.cache_tokens_pct_threshold = 50;
		assert!(config.validate_thresholds().is_ok());
	}

	#[test]
	fn test_environment_variable_precedence() {
		// This test would need to be run with specific environment variables set
		// For now, just test that the load function doesn't panic
		let result = Config::load();
		assert!(result.is_ok());
	}
}