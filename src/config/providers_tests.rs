// Copyright 2026 Muvon Un Limited
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

use super::*;

#[test]
fn provider_config_default_has_no_api_key() {
	let config = ProviderConfig::default();
	assert_eq!(config.api_key, None);
}

#[test]
fn providers_config_default_all_api_keys_none() {
	let config = ProvidersConfig::default();
	assert_eq!(config.openrouter.api_key, None);
	assert_eq!(config.openai.api_key, None);
	assert_eq!(config.anthropic.api_key, None);
	assert_eq!(config.google.api_key, None);
	assert_eq!(config.amazon.api_key, None);
	assert_eq!(config.cloudflare.api_key, None);
}

#[test]
fn openrouter_config_serializes_model_and_api_key() {
	let config = OpenRouterConfig {
		model: "anthropic/claude-sonnet-4".to_string(),
		api_key: Some("sk-test-key".to_string()),
	};
	let json = serde_json::to_value(&config).expect("failed to serialize OpenRouterConfig");
	assert_eq!(json["model"], "anthropic/claude-sonnet-4");
	assert_eq!(json["api_key"], "sk-test-key");
}

#[test]
fn providers_config_toml_round_trip() {
	let config = ProvidersConfig {
		openrouter: ProviderConfig {
			api_key: Some("sk-or-key".to_string()),
		},
		..ProvidersConfig::default()
	};
	let serialized = toml::to_string(&config).expect("failed to serialize ProvidersConfig");
	let deserialized: ProvidersConfig =
		toml::from_str(&serialized).expect("failed to deserialize ProvidersConfig");
	assert_eq!(
		deserialized.openrouter.api_key,
		Some("sk-or-key".to_string())
	);
	assert_eq!(deserialized.openai.api_key, None);
}
