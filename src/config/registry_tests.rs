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
fn registry_config_default_has_24h_ttl() {
	let config = RegistryConfig::default();
	assert_eq!(config.cache_ttl_hours, 24);
}

#[test]
fn every_registry_field_is_required() {
	for partial in ["", "tap = \"a/b\"\nref = \"x\"\n", "cache_ttl_hours = 1\nref = \"x\"\n"] {
		assert!(toml::from_str::<RegistryConfig>(partial).is_err(), "{partial:?}");
	}
}

#[test]
fn test_defaults_equal_the_template_registry() {
	let template: toml::Value =
		toml::from_str(include_str!("../../config-templates/default.toml")).expect("template parses");
	let from_template: RegistryConfig = template["registry"].clone().try_into().expect("template [registry]");
	let defaults = RegistryConfig::default();
	assert_eq!(from_template.cache_ttl_hours, defaults.cache_ttl_hours);
	assert_eq!(from_template.tap, defaults.tap);
	assert_eq!(from_template.git_ref, defaults.git_ref);
}

#[test]
fn serialization_round_trip() {
	let config = RegistryConfig {
		cache_ttl_hours: 48,
		tap: "acme/tap".to_string(),
		git_ref: "v2".to_string(),
	};
	let serialized = toml::to_string(&config).expect("failed to serialize RegistryConfig");
	let deserialized: RegistryConfig =
		toml::from_str(&serialized).expect("failed to deserialize RegistryConfig round-trip");
	assert_eq!(deserialized.cache_ttl_hours, 48);
	assert_eq!(deserialized.tap, "acme/tap");
	assert_eq!(deserialized.git_ref, "v2");
}

#[test]
fn ref_key_is_read_into_git_ref() {
	let config: RegistryConfig =
		toml::from_str("cache_ttl_hours = 1\ntap = \"acme/tap\"\nref = \"abc123\"\n").expect("failed to parse");
	assert_eq!(config.tap, "acme/tap");
	assert_eq!(config.git_ref, "abc123");
}
