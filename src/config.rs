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
pub struct Config {
    #[serde(default)]
    pub embedding_provider: EmbeddingProvider,
    #[serde(default)]
    pub fastembed: FastEmbedConfig,
    #[serde(default)]
    pub jina: JinaConfig,
    pub jina_api_key: Option<String>,
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
            config_path: None,
        }
    }
}

impl Config {
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

            // Check environment variable for API key even if config exists
            if config.jina_api_key.is_none() {
                config.jina_api_key = std::env::var("JINA_API_KEY").ok();
            }

            Ok(config)
        } else {
            // Create default config
            let mut config = Config::default();

            // Check environment variable for API key
            config.jina_api_key = std::env::var("JINA_API_KEY").ok();

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
            let current_dir = std::env::current_dir()?;
            let config_path = current_dir.join(".octodev.toml");

            let config_str = toml::to_string(self)
                .context("Failed to serialize configuration to TOML")?;
            fs::write(&config_path, config_str)
                .context(format!("Failed to write config to {}", config_path.display()))?;

            Ok(())
        }
    }

    pub fn create_default_config() -> Result<PathBuf> {
        let current_dir = std::env::current_dir()?;
        let config_path = current_dir.join(".octodev.toml");

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
