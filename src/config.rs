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
	"openrouter:openai/gpt-4.1-nano".to_string()
}

fn default_relationship_model() -> String {
	"openrouter:openai/gpt-4.1-nano".to_string()
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
	// Log level setting
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
	#[serde(default = "default_cache_tokens_pct_threshold")]
	pub cache_tokens_pct_threshold: u8,
	// Alternative: Auto-cache when non-cached tokens reach this absolute number
	#[serde(default)]
	pub cache_tokens_absolute_threshold: u64,
	// Auto-cache timeout in seconds (3 minutes = 180 seconds by default)
	#[serde(default = "default_cache_timeout_seconds")]
	pub cache_timeout_seconds: u64,
	// Enable markdown rendering for AI responses
	#[serde(default)]
	pub enable_markdown_rendering: bool,
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
			log_level: LogLevel::default(),
			mcp_response_warning_threshold: default_mcp_response_warning_threshold(),
			max_request_tokens_threshold: default_max_request_tokens_threshold(),
			enable_auto_truncation: false,
			cache_tokens_pct_threshold: default_cache_tokens_pct_threshold(),
			cache_tokens_absolute_threshold: 0,
			cache_timeout_seconds: default_cache_timeout_seconds(),
			enable_markdown_rendering: false,
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
	pub name: String,
	
	// Server type - determines how the server is handled
	#[serde(default)]
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

impl McpServerConfig {
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

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct McpConfig {
	#[serde(default)]
	pub enabled: bool,
	#[serde(default = "default_mcp_servers")]
	pub servers: Vec<McpServerConfig>,
	
	// Legacy field for backward compatibility - will be migrated to servers
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub providers: Vec<String>,
}

fn default_mcp_servers() -> Vec<McpServerConfig> {
	vec![
		McpServerConfig::developer("developer", vec![]), // All developer tools enabled
		McpServerConfig::filesystem("filesystem", vec![]), // All filesystem tools enabled
	]
}

impl Default for McpConfig {
	fn default() -> Self {
		Self {
			enabled: false,
			servers: default_mcp_servers(),
			providers: Vec::new(), // Legacy field, empty by default
		}
	}
}

impl McpConfig {
	/// Check if MCP has any enabled servers (including built-in)
	pub fn has_enabled_servers(&self) -> bool {
		self.enabled && self.servers.iter().any(|server| server.enabled)
	}
	
	/// Get all enabled servers
	pub fn get_enabled_servers(&self) -> Vec<&McpServerConfig> {
		if !self.enabled {
			return Vec::new();
		}
		self.servers.iter().filter(|server| server.enabled).collect()
	}
	
	/// Migrate legacy providers configuration to new servers format
	pub fn migrate_from_legacy(&mut self) {
		if !self.providers.is_empty() {
			// If we have legacy providers and no servers (or only defaults), create servers from providers
			if self.servers.is_empty() || self.servers == default_mcp_servers() {
				self.servers.clear();
				
				for provider in &self.providers {
					match provider.as_str() {
						"core" => {
							// Add both developer and filesystem servers for legacy "core" provider
							self.servers.push(McpServerConfig::developer("developer", vec![]));
							self.servers.push(McpServerConfig::filesystem("filesystem", vec![]));
						}
						name => {
							// Unknown provider - create a placeholder
							eprintln!("Warning: Unknown legacy provider '{}' cannot be migrated", name);
						}
					}
				}
			}
			
			// Clear the legacy providers field after migration
			self.providers.clear();
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
				..ModeConfig::default()
			},
			mcp: McpConfig {
				enabled: true,
				..McpConfig::default()
			},
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
				..ModeConfig::default()
			},
			mcp: McpConfig {
				enabled: false,  // Assistant role has MCP/tools disabled by default
				..McpConfig::default()
			},
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
	#[serde(default)]
	pub embedding_provider: EmbeddingProvider,
	#[serde(default)]
	pub fastembed: FastEmbedConfig,
	#[serde(default)]
	pub jina: JinaConfig,
	#[serde(default)]
	pub graphrag: GraphRagConfig,
	pub jina_api_key: Option<String>,
	
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
	/// Check if MCP config is "empty" (using only defaults) - then we should fallback to global
	fn is_mcp_config_empty(&self, mcp_config: &McpConfig) -> bool {
		// If it's exactly the default McpConfig, consider it empty
		let default_mcp = McpConfig::default();
		
		// More sophisticated check: if only enabled differs from default, it's not empty
		// If enabled is different AND (providers differ OR servers differ), it's not empty
		if mcp_config.enabled != default_mcp.enabled {
			// If enabled is different but everything else is default, consider it meaningful
			return false;
		}
		
		// If providers or servers are customized, it's not empty
		if mcp_config.providers != default_mcp.providers || !mcp_config.servers.is_empty() {
			return false;
		}
		
		// Otherwise, it's effectively empty/default
		true
	}

	/// Get configuration for a specific role with proper fallback logic and role inheritance
	/// Returns: (mode_config, mcp_config, layers, system_prompt)
	/// Role inheritance: any role inherits from 'assistant' first, then applies its own overrides
	pub fn get_mode_config(&self, role: &str) -> (&ModeConfig, McpConfig, Option<&Vec<crate::session::layers::LayerConfig>>, Option<&String>) {
		match role {
			"developer" => {
				// Developer role - inherits from assistant but with developer-specific config
				let mut mcp_config = self.developer.mcp.clone();
				
				// If developer.mcp is "empty/default", fall back to global mcp
				if self.is_mcp_config_empty(&mcp_config) {
					mcp_config = self.mcp.clone();
				}
				
				(&self.developer.config, mcp_config, self.developer.layers.as_ref(), self.developer.config.system.as_ref())
			},
			"assistant" => {
				// Base assistant role
				let mut mcp_config = self.assistant.mcp.clone();
				
				// If assistant.mcp is "empty/default", fall back to global mcp
				if self.is_mcp_config_empty(&mcp_config) {
					mcp_config = self.mcp.clone();
				}
				
				(&self.assistant.config, mcp_config, None, self.assistant.config.system.as_ref())
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
		let (mode_config, mcp_config, layers_config, system_prompt) = self.get_mode_config(mode);
		
		let mut merged = self.clone();
		
		// Create an OpenRouterConfig from the ModeConfig for backward compatibility
		merged.openrouter = OpenRouterConfig {
			model: mode_config.get_full_model(),
			api_key: mode_config.get_api_key(&self.providers),
			pricing: mode_config.get_pricing(&self.providers),
			enable_layers: mode_config.enable_layers,
			log_level: mode_config.log_level.clone(),
			mcp_response_warning_threshold: mode_config.mcp_response_warning_threshold,
			max_request_tokens_threshold: mode_config.max_request_tokens_threshold,
			enable_auto_truncation: mode_config.enable_auto_truncation,
			cache_tokens_pct_threshold: mode_config.cache_tokens_pct_threshold,
			cache_tokens_absolute_threshold: mode_config.cache_tokens_absolute_threshold,
			cache_timeout_seconds: mode_config.cache_timeout_seconds,
			enable_markdown_rendering: mode_config.enable_markdown_rendering,
		};
		
		merged.mcp = mcp_config;
		merged.layers = layers_config.cloned();
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

	/// Simple migration for providers only
	fn migrate_legacy_config(&mut self) {
		// Migrate API keys from legacy openrouter config to providers
		if let Some(api_key) = &self.openrouter.api_key {
			if self.providers.openrouter.api_key.is_none() {
				self.providers.openrouter.api_key = Some(api_key.clone());
			}
		}
		
		// Migrate global MCP configuration
		self.mcp.migrate_from_legacy();
		
		// Migrate role-specific MCP configurations
		self.developer.mcp.migrate_from_legacy();
		self.assistant.mcp.migrate_from_legacy();
	}

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
		if let Err(e) = self.validate_openrouter_model() {
			eprintln!("Configuration validation warning: {}", e);
			eprintln!("The application will continue, but you may want to fix these issues.");
			eprintln!("Update your .octodev/config.toml to use the new format:");
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
		// Helper function to validate a single MCP config
		let validate_mcp = |mcp_config: &McpConfig, context: &str| -> Result<()> {
			if !mcp_config.enabled {
				return Ok(());
			}

			for server in &mcp_config.servers {
				if server.enabled {
					match server.server_type {
						McpServerType::External => {
							// External servers must have either URL or command
							if server.url.is_none() && server.command.is_none() {
								return Err(anyhow!(
									"{}: External MCP server '{}' must have either 'url' or 'command' specified",
									context, server.name
								));
							}
						}
						McpServerType::Developer | McpServerType::Filesystem => {
							// Built-in servers should not have URL or command
							if server.url.is_some() || server.command.is_some() {
								eprintln!(
									"Warning: {}: Built-in server '{}' has URL/command specified, which will be ignored",
									context, server.name
								);
							}
						}
					}

					// Validate timeout
					if server.timeout_seconds == 0 {
						return Err(anyhow!(
							"{}: MCP server '{}' timeout must be greater than 0",
							context, server.name
						));
					}

					// Validate server name
					if server.name.trim().is_empty() {
						return Err(anyhow!("{}: MCP server name cannot be empty", context));
					}
				}
			}

			Ok(())
		};

		// Validate global MCP config
		validate_mcp(&self.mcp, "Global MCP config")?;
		
		// Validate role-specific MCP configs
		validate_mcp(&self.developer.mcp, "Developer role MCP config")?;
		validate_mcp(&self.assistant.mcp, "Assistant role MCP config")?;

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

			// Migrate legacy configuration
			config.migrate_legacy_config();

			// Environment variables take precedence over config file values
			if let Ok(jina_key) = std::env::var("JINA_API_KEY") {
				config.jina_api_key = Some(jina_key);
			}
			
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
			// Create default config
			let config = Config {
				config_path: Some(config_path),
				jina_api_key: std::env::var("JINA_API_KEY").ok(),
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