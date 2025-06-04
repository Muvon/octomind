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

use anyhow::{anyhow, Result};

use super::Config;

impl Config {
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

	pub fn validate_openrouter_model(&self) -> Result<()> {
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
					}
					Err(_) => {
						return Err(anyhow!(
							"Unsupported provider: '{}'. Supported providers: openrouter, openai, anthropic, google, amazon, cloudflare",
							provider_name
						));
					}
				}
			}
			Err(_) => {
				return Err(anyhow!(
					"Invalid model format: '{}'. Must use 'provider:model' format",
					model
				));
			}
		}

		Ok(())
	}

	pub fn validate_thresholds(&self) -> Result<()> {
		// Validate cache tokens threshold (0 is valid for disabling)
		if self.cache_tokens_threshold > 1_000_000 {
			return Err(anyhow!(
				"Cache tokens threshold too high: {}. Maximum allowed: 1,000,000",
				self.cache_tokens_threshold
			));
		}

		// Validate MCP response warning threshold (0 is valid for disabling)
		if self.mcp_response_warning_threshold > 1_000_000 {
			return Err(anyhow!(
				"MCP response warning threshold too high: {}. Maximum allowed: 1,000,000",
				self.mcp_response_warning_threshold
			));
		}

		// Validate max request tokens threshold
		// Only validate if auto-truncation is enabled
		if self.enable_auto_truncation {
			if self.max_request_tokens_threshold == 0 {
				return Err(anyhow!(
					"Max request tokens threshold cannot be 0 when auto-truncation is enabled. Use a positive value or disable auto-truncation."
				));
			}

			if self.max_request_tokens_threshold > 2_000_000 {
				return Err(anyhow!(
					"Max request tokens threshold too high: {}. Maximum allowed: 2,000,000",
					self.max_request_tokens_threshold
				));
			}
		} else {
			// When auto-truncation is disabled, we still validate the upper bound if a value is set
			if self.max_request_tokens_threshold > 2_000_000 {
				return Err(anyhow!(
					"Max request tokens threshold too high: {}. Maximum allowed: 2,000,000",
					self.max_request_tokens_threshold
				));
			}
		}

		// Validate cache timeout
		if self.cache_timeout_seconds > 86400 {
			// 24 hours max
			return Err(anyhow!(
				"Cache timeout too high: {} seconds. Maximum allowed: 86400 (24 hours)",
				self.cache_timeout_seconds
			));
		}

		Ok(())
	}

	fn validate_mcp_config(&self) -> Result<()> {
		// Validate server configurations
		for (server_name, server_config) in &self.mcp.servers {
			// Validate timeout
			if server_config.timeout_seconds == 0 {
				return Err(anyhow!(
					"Server '{}' has invalid timeout: 0. Must be greater than 0",
					server_name
				));
			}

			if server_config.timeout_seconds > 3600 {
				// 1 hour max
				return Err(anyhow!(
					"Server '{}' timeout too high: {} seconds. Maximum allowed: 3600 (1 hour)",
					server_name,
					server_config.timeout_seconds
				));
			}

			// Validate external server configuration
			if matches!(
				server_config.server_type,
				crate::config::McpServerType::External
			) {
				if server_config.url.is_none() && server_config.command.is_none() {
					return Err(anyhow!(
						"External server '{}' must have either 'url' or 'command' specified",
						server_name
					));
				}

				if server_config.url.is_some() && server_config.command.is_some() {
					return Err(anyhow!(
						"External server '{}' cannot have both 'url' and 'command' specified",
						server_name
					));
				}
			}
		}

		Ok(())
	}

	fn validate_layers(&self, layers: &[crate::session::layers::LayerConfig]) -> Result<()> {
		for (index, layer) in layers.iter().enumerate() {
			// Validate layer name
			if layer.name.is_empty() {
				return Err(anyhow!("Layer at index {} has empty name", index));
			}

			// Validate layer name is not empty (layer_type field doesn't exist)
			// Additional layer-specific validation can be added here if needed

			// Additional layer-specific validation can be added here
		}

		Ok(())
	}
}
