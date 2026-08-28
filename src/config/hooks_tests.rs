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
fn default_hook_timeout_is_30() {
	assert_eq!(default_hook_timeout(), 30);
}

#[test]
fn deserializes_from_toml_with_all_fields() {
	let config: HookConfig = toml::from_str(
		r#"
name = "ci-hook"
bind = "0.0.0.0:9876"
script = "/usr/local/bin/handler.sh"
timeout = 60
"#,
	)
	.expect("failed to deserialize HookConfig from TOML");
	assert_eq!(config.name, "ci-hook");
	assert_eq!(config.bind, "0.0.0.0:9876");
	assert_eq!(config.script, "/usr/local/bin/handler.sh");
	assert_eq!(config.timeout, 60);
}

#[test]
fn timeout_defaults_to_30_when_missing() {
	let config: HookConfig = toml::from_str(
		r#"
name = "minimal"
bind = "127.0.0.1:8080"
script = "./hook.sh"
"#,
	)
	.expect("failed to deserialize HookConfig without timeout");
	assert_eq!(config.timeout, 30);
}

#[test]
fn serialization_round_trip() {
	let config = HookConfig {
		name: "round-trip".to_string(),
		bind: "0.0.0.0:9999".to_string(),
		script: "./script.sh".to_string(),
		timeout: 45,
	};
	let serialized = toml::to_string(&config).expect("failed to serialize HookConfig");
	let deserialized: HookConfig =
		toml::from_str(&serialized).expect("failed to deserialize HookConfig round-trip");
	assert_eq!(deserialized.name, config.name);
	assert_eq!(deserialized.bind, config.bind);
	assert_eq!(deserialized.script, config.script);
	assert_eq!(deserialized.timeout, config.timeout);
}
