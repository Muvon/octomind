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
use serde::{Deserialize, Serialize};

use super::mcp::RoleMcpConfig;
use super::providers::{OpenRouterConfig, ProvidersConfig};

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
				system: Some("You are an Octomind AI developer assistant with full access to development tools.".to_string()),
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

impl RoleMcpConfig {
	/// Create a new RoleMcpConfig with server references
	pub fn with_server_refs(server_refs: Vec<String>) -> Self {
		Self {
			server_refs,
			allowed_tools: Vec::new(),
		}
	}

	/// Create a new RoleMcpConfig with server references and allowed tools
	pub fn with_server_refs_and_tools(
		server_refs: Vec<String>,
		allowed_tools: Vec<String>,
	) -> Self {
		Self {
			server_refs,
			allowed_tools,
		}
	}
}
