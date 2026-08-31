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
fn with_server_refs_sets_refs_and_empty_tools() {
	let config = RoleMcpConfig::with_server_refs(vec!["core".to_string(), "runtime".to_string()]);
	assert_eq!(
		config.server_refs,
		vec!["core".to_string(), "runtime".to_string()]
	);
	assert!(config.allowed_tools.is_empty());
}

#[test]
fn with_server_refs_and_tools_sets_both() {
	let config = RoleMcpConfig::with_server_refs_and_tools(
		vec!["filesystem".to_string()],
		vec!["read_file".to_string(), "write_file".to_string()],
	);
	assert_eq!(config.server_refs, vec!["filesystem".to_string()]);
	assert_eq!(
		config.allowed_tools,
		vec!["read_file".to_string(), "write_file".to_string()]
	);
}

fn sample_role() -> Role {
	Role {
		name: "developer".to_string(),
		config: RoleConfig {
			model: crate::config::ModelProfileOverride {
				model: Some("claude-sonnet-4".to_string()),
				temperature: Some(0.7),
				top_p: Some(0.95),
				top_k: Some(40),
				..Default::default()
			},
			system: "You are a developer.".to_string(),
			welcome: "Welcome!".to_string(),
			temperature: None,
			top_p: None,
			top_k: None,
		},
		mcp: RoleMcpConfig::with_server_refs_and_tools(
			vec!["core".to_string()],
			vec!["plan".to_string()],
		),
	}
}

#[test]
fn role_serde_json_round_trip() {
	let role = sample_role();
	let json = serde_json::to_string(&role).expect("failed to serialize Role");
	let deserialized: Role = serde_json::from_str(&json).expect("failed to deserialize Role");
	assert_eq!(deserialized.name, "developer");
	assert_eq!(
		deserialized.config.model.model,
		Some("claude-sonnet-4".to_string())
	);
	assert_eq!(deserialized.config.system, "You are a developer.");
	assert_eq!(deserialized.config.welcome, "Welcome!");
	assert_eq!(deserialized.config.model.temperature, Some(0.7));
	assert_eq!(deserialized.config.model.top_p, Some(0.95));
	assert_eq!(deserialized.config.model.top_k, Some(40));
	assert_eq!(deserialized.mcp.server_refs, vec!["core".to_string()]);
	assert_eq!(deserialized.mcp.allowed_tools, vec!["plan".to_string()]);
}

#[test]
fn role_config_flattens_into_role_json() {
	let json = serde_json::to_value(sample_role()).expect("failed to serialize Role to JSON");
	// #[serde(flatten)] hoists RoleConfig fields to the top level
	assert_eq!(json["name"], "developer");
	assert_eq!(json["system"], "You are a developer.");
	assert_eq!(json["welcome"], "Welcome!");
	let temperature = json["model"]["temperature"]
		.as_f64()
		.expect("temperature not a number");
	assert!(
		(temperature - 0.7f64).abs() < 1e-6,
		"temperature {temperature}"
	);
	assert!(
		json.get("config").is_none(),
		"config must be flattened, not nested"
	);
}
