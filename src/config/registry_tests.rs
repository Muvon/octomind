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
fn default_cache_ttl_hours_is_24() {
	assert_eq!(default_cache_ttl_hours(), 24);
}

#[test]
fn registry_config_default_has_24h_ttl() {
	let config = RegistryConfig::default();
	assert_eq!(config.cache_ttl_hours, 24);
}

#[test]
fn missing_ttl_deserializes_to_24() {
	let config: RegistryConfig =
		toml::from_str("").expect("failed to deserialize RegistryConfig from empty TOML");
	assert_eq!(config.cache_ttl_hours, 24);
}

#[test]
fn serialization_round_trip() {
	let config = RegistryConfig {
		cache_ttl_hours: 48,
	};
	let serialized = toml::to_string(&config).expect("failed to serialize RegistryConfig");
	let deserialized: RegistryConfig =
		toml::from_str(&serialized).expect("failed to deserialize RegistryConfig round-trip");
	assert_eq!(deserialized.cache_ttl_hours, 48);
}
