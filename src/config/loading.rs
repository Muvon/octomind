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

use anyhow::{Context, Result};
use std::fs;

use super::{Config, ProviderConfig, ProvidersConfig};

impl Config {
	/// Initialize the server registry and API keys
	fn initialize_config(&mut self) {
		// SIMPLIFIED: No longer populate internal servers in loaded config
		// Internal servers are now provided at runtime via get_core_server_config()

		// Migrate API keys from legacy openrouter config to providers
		if let Some(api_key) = &self.openrouter.api_key {
			if self.providers.openrouter.api_key.is_none() {
				self.providers.openrouter.api_key = Some(api_key.clone());
			}
		}
	}

	pub fn ensure_octomind_dir() -> Result<std::path::PathBuf> {
		// Use the system-wide directory
		crate::directories::get_octomind_data_dir()
	}

	/// Load configuration from the system-wide config file
	pub fn load() -> Result<Self> {
		// Use the new system-wide config file path
		let config_path = crate::directories::get_config_file_path()?;

		if config_path.exists() {
			let config_str = fs::read_to_string(&config_path).context(format!(
				"Failed to read config from {}",
				config_path.display()
			))?;
			let mut config: Config =
				toml::from_str(&config_str).context("Failed to parse TOML configuration")?;

			// Store the config path for future saves
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
			// Create default config with environment variables
			let mut config = Self::default_with_env();
			config.config_path = Some(config_path);

			// CRITICAL FIX: Initialize the configuration when no file exists
			config.initialize_config();

			Ok(config)
		}
	}

	/// Create a default configuration with environment variables
	fn default_with_env() -> Self {
		Self {
			providers: ProvidersConfig {
				openrouter: ProviderConfig {
					api_key: std::env::var("OPENROUTER_API_KEY").ok(),
				},
				openai: ProviderConfig {
					api_key: std::env::var("OPENAI_API_KEY").ok(),
				},
				anthropic: ProviderConfig {
					api_key: std::env::var("ANTHROPIC_API_KEY").ok(),
				},
				google: ProviderConfig {
					api_key: std::env::var("GOOGLE_APPLICATION_CREDENTIALS").ok(),
				},
				amazon: ProviderConfig {
					api_key: std::env::var("AWS_ACCESS_KEY_ID").ok(),
				},
				cloudflare: ProviderConfig {
					api_key: std::env::var("CLOUDFLARE_API_TOKEN").ok(),
				},
			},
			openrouter: super::OpenRouterConfig {
				api_key: std::env::var("OPENROUTER_API_KEY").ok(),
				..Default::default()
			},
			..Default::default()
		}
	}

	/// Save configuration to file
	pub fn save(&self) -> Result<()> {
		// Validate before saving
		self.validate()?;

		// Use the stored config path, or fallback to system-wide default
		let config_path = if let Some(path) = &self.config_path {
			path.clone()
		} else {
			crate::directories::get_config_file_path()?
		};

		// Ensure the parent directory exists
		if let Some(parent) = config_path.parent() {
			fs::create_dir_all(parent).context(format!(
				"Failed to create config directory: {}",
				parent.display()
			))?;
		}

		// Serialize to TOML
		let config_str =
			toml::to_string_pretty(self).context("Failed to serialize configuration to TOML")?;

		// Write to file
		fs::write(&config_path, config_str).context(format!(
			"Failed to write config to {}",
			config_path.display()
		))?;

		println!("Configuration saved to {}", config_path.display());
		Ok(())
	}

	/// Load configuration from a specific file path
	pub fn load_from_path(path: &std::path::Path) -> Result<Self> {
		let config_str = fs::read_to_string(path)
			.context(format!("Failed to read config from {}", path.display()))?;
		let mut config: Config =
			toml::from_str(&config_str).context("Failed to parse TOML configuration")?;

		// Store the config path for future saves
		config.config_path = Some(path.to_path_buf());

		// Initialize the configuration
		config.initialize_config();

		// Validate the configuration
		config.validate()?;

		Ok(config)
	}

	/// Save configuration to a specific file path
	pub fn save_to_path(&self, path: &std::path::Path) -> Result<()> {
		// Validate before saving
		self.validate()?;

		// Ensure the parent directory exists
		if let Some(parent) = path.parent() {
			fs::create_dir_all(parent).context(format!(
				"Failed to create config directory: {}",
				parent.display()
			))?;
		}

		// Serialize to TOML
		let config_str =
			toml::to_string_pretty(self).context("Failed to serialize configuration to TOML")?;

		// Write to file
		fs::write(path, config_str)
			.context(format!("Failed to write config to {}", path.display()))?;

		println!("Configuration saved to {}", path.display());
		Ok(())
	}

	/// Create a clean copy of the config for saving (removes runtime-only fields)
	pub fn create_clean_copy_for_saving(&self) -> Self {
		let mut clean_config = self.clone();

		// Remove internal servers from the MCP registry before saving
		let internal_servers = ["developer", "filesystem", "octocode"];
		for server_name in &internal_servers {
			clean_config.mcp.servers.remove(*server_name);
		}

		// CRITICAL FIX: Don't save the [mcp] section at all if it only contains internal servers
		// The MCP functionality should be controlled by role server_refs, not by a global enabled flag
		// If there are no user-defined servers, ensure the config will be skipped during serialization
		if clean_config.mcp.servers.is_empty() && clean_config.mcp.allowed_tools.is_empty() {
			// Clear the config so it gets skipped during serialization
			clean_config.mcp.servers.clear();
			clean_config.mcp.allowed_tools.clear();
		}

		clean_config
	}

	/// Update configuration with a closure and save
	pub fn update_and_save<F>(&mut self, updater: F) -> Result<()>
	where
		F: FnOnce(&mut Self),
	{
		// Validate before saving
		self.validate()?;

		// Use the stored config path, or fallback to system-wide default
		let config_path = if let Some(path) = &self.config_path {
			path.clone()
		} else {
			crate::directories::get_config_file_path()?
		};

		// Ensure the parent directory exists
		if let Some(parent) = config_path.parent() {
			fs::create_dir_all(parent).context(format!(
				"Failed to create config directory: {}",
				parent.display()
			))?;
		}

		// Create clean config for saving (no internal servers)
		let clean_config = self.create_clean_copy_for_saving();
		let config_str =
			toml::to_string(&clean_config).context("Failed to serialize configuration to TOML")?;

		// Write to file
		fs::write(&config_path, config_str).context(format!(
			"Failed to write config to {}",
			config_path.display()
		))?;

		// Update self with the changes (but keep internal servers in memory)
		updater(self);

		Ok(())
	}

	pub fn create_default_config() -> Result<std::path::PathBuf> {
		let config_path = crate::directories::get_config_file_path()?;

		if !config_path.exists() {
			let mut config = Config::default();

			// SIMPLIFIED: Initialize the configuration without populating internal servers
			config.initialize_config();

			// Create clean config for saving (no internal servers)
			let clean_config = config.create_clean_copy_for_saving();
			let config_str = toml::to_string(&clean_config)
				.context("Failed to serialize default configuration to TOML")?;

			fs::write(&config_path, config_str).context(format!(
				"Failed to write default config to {}",
				config_path.display()
			))?;

			println!("Created default configuration at {}", config_path.display());
		}

		Ok(config_path)
	}

	/// Update a specific field in the configuration and save to disk
	pub fn update_specific_field<F>(&mut self, updater: F) -> Result<()>
	where
		F: Fn(&mut Config),
	{
		// Load existing config from disk without initializing internal servers
		let config_path = if let Some(path) = &self.config_path {
			path.clone()
		} else {
			crate::directories::get_config_file_path()?
		};

		let mut disk_config = if config_path.exists() {
			let config_str = fs::read_to_string(&config_path).context(format!(
				"Failed to read config from {}",
				config_path.display()
			))?;
			let mut config: Config =
				toml::from_str(&config_str).context("Failed to parse TOML configuration")?;
			config.config_path = Some(config_path.clone());
			// SIMPLIFIED: Don't initialize internal servers
			config
		} else {
			// Create default config if file doesn't exist
			Config {
				config_path: Some(config_path.clone()),
				..Default::default()
			}
		};

		// Apply the update to the disk config
		updater(&mut disk_config);

		// Validate the updated config
		disk_config.validate()?;

		// Ensure the parent directory exists
		if let Some(parent) = config_path.parent() {
			fs::create_dir_all(parent).context(format!(
				"Failed to create config directory: {}",
				parent.display()
			))?;
		}

		// Create clean config for saving (no internal servers)
		let clean_config = disk_config.create_clean_copy_for_saving();
		let config_str =
			toml::to_string(&clean_config).context("Failed to serialize configuration to TOML")?;

		// Write to file
		fs::write(&config_path, config_str).context(format!(
			"Failed to write config to {}",
			config_path.display()
		))?;

		// Update self with the changes (but keep internal servers in memory)
		updater(self);

		Ok(())
	}
}
