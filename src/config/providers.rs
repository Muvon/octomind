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

use serde::{Deserialize, Serialize};

// Provider configurations - ONLY contain API keys and provider-specific settings
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ProviderConfig {
	pub api_key: Option<String>,
}

impl Default for ProviderConfig {
	fn default() -> Self {
		Self { api_key: None }
	}
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ProvidersConfig {
	pub openrouter: ProviderConfig,
	pub openai: ProviderConfig,
	pub anthropic: ProviderConfig,
	pub google: ProviderConfig,
	pub amazon: ProviderConfig,
	pub cloudflare: ProviderConfig,
}

impl Default for ProvidersConfig {
	fn default() -> Self {
		Self {
			openrouter: ProviderConfig::default(),
			openai: ProviderConfig::default(),
			anthropic: ProviderConfig::default(),
			google: ProviderConfig::default(),
			amazon: ProviderConfig::default(),
			cloudflare: ProviderConfig::default(),
		}
	}
}

// Legacy OpenRouterConfig for backward compatibility
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct OpenRouterConfig {
	pub model: String,
	pub api_key: Option<String>,
}

// REMOVED: Default implementations - all config must be explicit
