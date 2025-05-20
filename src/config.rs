use anyhow::{Result, Context};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

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
	// Layer-specific model configurations
	#[serde(default)]
	pub query_processor_model: Option<String>,
	#[serde(default)]
	pub context_generator_model: Option<String>,
	#[serde(default)]
	pub developer_model: Option<String>,
	#[serde(default)]
	pub summarizer_model: Option<String>,
	#[serde(default)]
	pub next_request_model: Option<String>,
	#[serde(default)]
	pub session_reviewer_model: Option<String>,
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

impl Default for PricingConfig {
	fn default() -> Self {
		Self {
			input_price: default_input_price(),
			output_price: default_output_price(),
		}
	}
}

fn default_openrouter_model() -> String {
	"anthropic/claude-3.7-sonnet".to_string()
}

impl Default for OpenRouterConfig {
	fn default() -> Self {
		Self {
			model: default_openrouter_model(),
			api_key: None,
			pricing: PricingConfig::default(),
			enable_layers: false, // Disabled by default
			query_processor_model: Some("openai/gpt-4o".to_string()),
			context_generator_model: Some("openai/gpt-4o".to_string()),
			developer_model: None, // Use the main model by default
			summarizer_model: Some("openai/gpt-4o".to_string()),
			next_request_model: Some("openai/gpt-4o".to_string()),
			session_reviewer_model: Some("openai/gpt-4o".to_string()),
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
			enabled: false,
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
	vec!["shell".to_string()]
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
	pub jina_api_key: Option<String>,
	#[serde(default)]
	pub openrouter: OpenRouterConfig,
	#[serde(default)]
	pub mcp: McpConfig,
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
			jina_api_key: None,
			openrouter: OpenRouterConfig::default(),
			mcp: McpConfig::default(),
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

	pub fn load() -> Result<Self> {
		let current_dir = std::env::current_dir()?;
		let config_path = current_dir.join(".octodev.toml");

		if config_path.exists() {
			let config_str = fs::read_to_string(&config_path)
				.context(format!("Failed to read config from {}", config_path.display()))?;
			let mut config: Config = toml::from_str(&config_str)
				.context("Failed to parse TOML configuration")?;

			// Store the config path for potential future saving
			config.config_path = Some(config_path);

			// Check environment variable for API keys even if config exists
			if config.jina_api_key.is_none() {
				config.jina_api_key = std::env::var("JINA_API_KEY").ok();
			}
			if config.openrouter.api_key.is_none() {
				config.openrouter.api_key = std::env::var("OPENROUTER_API_KEY").ok();
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
		if let Some(config_path) = &self.config_path {
			let config_str = toml::to_string(self)
				.context("Failed to serialize configuration to TOML")?;
			fs::write(config_path, config_str)
				.context(format!("Failed to write config to {}", config_path.display()))?;
			Ok(())
		} else {
			let octodev_dir = Self::ensure_octodev_dir()?;
			let config_path = octodev_dir.join("config.toml");

			let config_str = toml::to_string(self)
				.context("Failed to serialize configuration to TOML")?;
			fs::write(&config_path, config_str)
				.context(format!("Failed to write config to {}", config_path.display()))?;

			Ok(())
		}
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
