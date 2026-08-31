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

use super::{ModelProfile, ModelProfileOverride};
use crate::config::ReasoningEffortConfig;

fn main_profile() -> ModelProfile {
	ModelProfile {
		model: "openai:main".into(),
		reasoning_effort: ReasoningEffortConfig::Medium,
		max_tokens: 1000,
		temperature: 0.3,
		top_p: 0.8,
		top_k: 20,
		max_retries: 2,
		retry_timeout: 10,
		request_timeout_seconds: 60,
	}
}

#[test]
fn partial_override_inherits_every_unspecified_main_field() {
	let resolved = ModelProfileOverride {
		model: Some("anthropic:worker".into()),
		reasoning_effort: Some(ReasoningEffortConfig::High),
		..Default::default()
	}
	.resolve(&main_profile());

	assert_eq!(resolved.model, "anthropic:worker");
	assert_eq!(resolved.reasoning_effort, ReasoningEffortConfig::High);
	assert_eq!(resolved.max_tokens, 1000);
	assert_eq!(resolved.temperature, 0.3);
	assert_eq!(resolved.top_p, 0.8);
	assert_eq!(resolved.top_k, 20);
	assert_eq!(resolved.max_retries, 2);
	assert_eq!(resolved.retry_timeout, 10);
	assert_eq!(resolved.request_timeout_seconds, 60);
}

#[test]
fn later_override_wins_field_by_field() {
	let role = ModelProfileOverride {
		model: Some("openai:role".into()),
		temperature: Some(0.5),
		..Default::default()
	};
	let runtime = ModelProfileOverride {
		model: Some("google:runtime".into()),
		max_tokens: Some(42),
		..Default::default()
	};
	let resolved = role.overlay(&runtime).resolve(&main_profile());

	assert_eq!(resolved.model, "google:runtime");
	assert_eq!(resolved.temperature, 0.5);
	assert_eq!(resolved.max_tokens, 42);
}

#[test]
fn optional_owner_blocks_inherit_the_complete_main_profile() {
	let mut value: toml::Value =
		toml::from_str(include_str!("../../config-templates/default.toml"))
			.expect("default template parses");
	value["supervisor"].as_table_mut().unwrap().remove("model");
	value["compression"].as_table_mut().unwrap().remove("model");
	value["roles"].as_array_mut().unwrap()[0]
		.as_table_mut()
		.unwrap()
		.remove("model");

	let mut config: crate::config::Config = value.try_into().expect("optional profiles parse");
	config.build_role_map();
	assert_eq!(config.get_supervisor_model_profile(), config.model_profile);
	assert_eq!(config.get_compression_model_profile(), config.model_profile);
	assert_eq!(
		config.get_model_profile_for_role("assistant"),
		config.model_profile
	);
}

#[test]
fn main_model_profile_is_required_and_complete() {
	let mut value: toml::Value =
		toml::from_str(include_str!("../../config-templates/default.toml"))
			.expect("default template parses");
	value.as_table_mut().unwrap().remove("model");
	let parsed: Result<crate::config::Config, _> = value.try_into();
	let error = parsed
		.expect_err("main model table must be required")
		.to_string();
	assert!(error.contains("missing field `model`"), "got: {error}");

	let mut value: toml::Value =
		toml::from_str(include_str!("../../config-templates/default.toml"))
			.expect("default template parses");
	value["model"].as_table_mut().unwrap().remove("top_p");
	let parsed: Result<crate::config::Config, _> = value.try_into();
	let error = parsed
		.expect_err("main model fields must be required")
		.to_string();
	assert!(error.contains("missing field `top_p`"), "got: {error}");
}

#[test]
fn role_can_override_any_subset_of_the_main_profile() {
	let mut config: crate::config::Config =
		toml::from_str(include_str!("../../config-templates/default.toml")).unwrap();
	let role = config
		.roles
		.iter_mut()
		.find(|role| role.name == "assistant")
		.unwrap();
	role.config.model = ModelProfileOverride {
		model: Some("openai:gpt-5".into()),
		reasoning_effort: Some(ReasoningEffortConfig::High),
		..Default::default()
	};
	config.build_role_map();
	let resolved = config.get_model_profile_for_role("assistant");

	assert_eq!(resolved.model, "openai:gpt-5");
	assert_eq!(resolved.reasoning_effort, ReasoningEffortConfig::High);
	assert_eq!(resolved.max_tokens, config.model_profile.max_tokens);
	assert_eq!(resolved.temperature, config.model_profile.temperature);
	assert_eq!(resolved.retry_timeout, config.model_profile.retry_timeout);
}
