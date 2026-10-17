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

//! End-to-end validation tests complementing the inline unit tests: every
//! rule exercised through the public `validate()` entry point, plus boundary
//! cases the per-rule tests leave open.

use super::*;
use crate::config::{HookConfig, McpServerConfig, Role, RoleConfig};
use crate::session::layers::{InputMode, LayerConfig, OutputMode, OutputRole};

fn template_config() -> Config {
	toml::from_str(include_str!("../../config-templates/default.toml"))
		.expect("default template must deserialize")
}

fn valid_layer() -> LayerConfig {
	LayerConfig {
		name: "test_layer".to_string(),
		description: "A test layer".to_string(),
		command: "octomind acp test_role".to_string(),
		workdir: ".".to_string(),
		input_mode: InputMode::Last,
		output_mode: OutputMode::None,
		output_role: OutputRole::Assistant,
	}
}

fn hook(name: &str, bind: &str, script: &str, timeout: u64) -> HookConfig {
	HookConfig {
		name: name.to_string(),
		bind: bind.to_string(),
		script: script.to_string(),
		timeout,
	}
}

fn role_with(name: &str, temperature: f32, top_p: f32, top_k: u32) -> Role {
	Role {
		name: name.to_string(),
		config: RoleConfig {
			model: crate::config::ModelProfileOverride {
				temperature: Some(temperature),
				top_p: Some(top_p),
				top_k: Some(top_k),
				..Default::default()
			},
			system: "system prompt".to_string(),
			welcome: "welcome".to_string(),
			temperature: None,
			top_p: None,
			top_k: None,
		},
		mcp: Default::default(),
	}
}

#[test]
fn full_validate_catches_a_zero_timeout_server() {
	let mut config = template_config();
	config.mcp.servers = vec![McpServerConfig::stdin("local", "node", vec![], 0, vec![])];
	let error = config.validate().unwrap_err().to_string();
	assert!(error.contains("invalid timeout"), "got: {error}");
}

#[test]
fn full_validate_catches_a_malformed_hook() {
	let mut config = template_config();
	config.hooks = vec![hook("bad", "not-an-address", "s.sh", 30)];
	let error = config.validate().unwrap_err().to_string();
	assert!(error.contains("invalid bind address"), "got: {error}");
}

#[test]
fn full_validate_catches_an_out_of_range_role_knob() {
	let mut config = template_config();
	config.roles = vec![role_with("runner", 2.5, 1.0, 100)];
	let error = config.validate().unwrap_err().to_string();
	assert!(error.contains("temperature"), "got: {error}");
}

#[test]
fn full_validate_accepts_every_optional_section_at_once() {
	let mut config = template_config();
	config.layers = Some(vec![valid_layer()]);
	config.hooks = vec![hook("deploy", "127.0.0.1:9876", "./hooks/deploy.sh", 30)];
	config.mcp.servers = vec![
		McpServerConfig::builtin("core", 30, vec![]),
		McpServerConfig::stdin("local", "node", vec![], 3600, vec![]),
		McpServerConfig::http("remote", "https://example.com/mcp", 1, vec![]),
	];
	config
		.validate()
		.expect("a config with every optional section valid must pass");
}

#[test]
fn whitespace_only_supervisor_model_is_rejected_like_an_empty_one() {
	let mut config = template_config();
	config.supervisor.model.model = Some("   ".to_string());
	let error = config.validate_model_profiles().unwrap_err().to_string();
	assert!(error.contains("cannot be empty"), "got: {error}");
}

#[test]
fn an_empty_roles_collection_passes_the_required_field_checks() {
	let mut config = template_config();
	config.roles = Vec::new();
	config
		.validate_required_fields()
		.expect("no roles means no per-role checks");
}

#[test]
fn layer_errors_report_the_offending_index() {
	let mut config = template_config();
	let mut second = valid_layer();
	second.name = "second".to_string();
	second.description = String::new();
	config.layers = Some(vec![valid_layer(), second]);
	let error = config.validate().unwrap_err().to_string();
	assert!(error.contains("index 1"), "got: {error}");
	assert!(error.contains("second"), "got: {error}");
}

#[test]
fn role_checks_cover_every_role_not_just_the_first() {
	let mut config = template_config();
	config.roles = vec![
		role_with("fine", 1.0, 1.0, 100),
		role_with("broken", 1.0, 1.0, 1001),
	];
	let error = config.validate_model_profiles().unwrap_err().to_string();
	assert!(error.contains("broken"), "got: {error}");
	assert!(error.contains("top_k"), "got: {error}");
}

#[test]
fn hook_timeout_of_one_second_is_the_valid_floor() {
	let mut config = template_config();
	config.hooks = vec![hook("fast", "127.0.0.1:1", "s.sh", 1)];
	config.validate_hooks().expect("1s timeout must pass");
}

#[test]
fn zero_session_token_threshold_means_disabled_and_passes() {
	let mut config = template_config();
	config.max_session_tokens_threshold = 0;
	config
		.validate_thresholds()
		.expect("0 disables the session token cap");
}

#[test]
fn compression_model_errors_surface_through_full_validate() {
	let mut config = template_config();
	config.compression.model.model = Some("not-a-provider:model".to_string());
	let error = config.validate().unwrap_err().to_string();
	assert!(error.contains("compression.model.name"), "got: {error}");
}

#[test]
fn evaluate_model_must_be_a_supported_provider_model_pair() {
	for bad in [
		"jev-latest",
		"openai:gpt",
		"typesafe:",
		":jev",
		"cloudflare:@cf/meta/llama",
		"typesafe:gpt-5",
	] {
		let mut config = template_config();
		config.decision.model = Some(bad.to_string());
		let error = config.validate().unwrap_err().to_string();
		assert!(error.contains("decision.model"), "{bad:?}: {error}");
	}
	for good in [
		"typesafe:jev-latest",
		"cloudflare:typesafe/jev",
		"octohub:auto",
	] {
		let mut config = template_config();
		config.decision.model = Some(good.to_string());
		config.validate().expect(good);
	}
}

#[test]
fn evaluate_section_is_strict_about_seam_keys() {
	let template = include_str!("../../config-templates/default.toml");
	let section = "[decision]\nmodel = \"cloudflare:typesafe/jev\"\n";
	assert!(template.contains(section), "template layout changed");
	let mistyped = template.replace(
		&format!("{section}# Filter recalled scoped lessons for relevance. Unavailable keeps candidates.\nrecall = false\n"),
		&format!("{section}recall = \"yes\"\n"),
	);
	assert_ne!(mistyped, template);
	let error = toml::from_str::<crate::config::Config>(&mistyped)
		.unwrap_err()
		.to_string();
	assert!(error.contains("recall"), "got: {error}");

	let missing = template.replace("skills = false\n", "");
	assert_ne!(missing, template);
	let error = toml::from_str::<crate::config::Config>(&missing)
		.unwrap_err()
		.to_string();
	assert!(error.contains("skills"), "got: {error}");
}

#[test]
fn evaluate_condense_and_compression_keys_are_strict_and_default_off() {
	let template = include_str!("../../config-templates/default.toml");
	let config: crate::config::Config = toml::from_str(template).unwrap();
	assert!(!config.decision.supervisor);
	assert!(!config.decision.compression);

	for key in ["supervisor", "compression"] {
		let line = format!("{key} = false\n");
		assert!(template.contains(&line), "template layout changed: {key}");
		let mistyped = template.replace(&line, &format!("{key} = \"yes\"\n"));
		let error = toml::from_str::<crate::config::Config>(&mistyped)
			.unwrap_err()
			.to_string();
		assert!(error.contains(key), "{key} mistyped: {error}");

		let missing = template.replace(&line, "");
		let error = toml::from_str::<crate::config::Config>(&missing)
			.unwrap_err()
			.to_string();
		assert!(error.contains(key), "{key} missing: {error}");
	}
}

#[test]
fn evaluate_distill_plan_and_gate_keys_are_strict_and_default_off() {
	let template = include_str!("../../config-templates/default.toml");
	let config: crate::config::Config = toml::from_str(template).unwrap();
	assert!(!config.decision.supervisor);

	let key = "supervisor";
	let line = format!("\n{key} = false\n");
	assert_eq!(
		template.matches(&line).count(),
		1,
		"template layout changed: {key}"
	);
	let mistyped = template.replace(&line, &format!("\n{key} = \"yes\"\n"));
	let error = toml::from_str::<crate::config::Config>(&mistyped)
		.unwrap_err()
		.to_string();
	assert!(error.contains(key), "{key} mistyped: {error}");

	let missing = template.replace(&line, "\n");
	let error = toml::from_str::<crate::config::Config>(&missing)
		.unwrap_err()
		.to_string();
	assert!(error.contains(key), "{key} missing: {error}");
}

#[test]
fn evaluate_capabilities_key_is_strict_and_default_off() {
	let template = include_str!("../../config-templates/default.toml");
	let config: crate::config::Config = toml::from_str(template).unwrap();
	assert!(!config.decision.capabilities);

	let line = "\ncapabilities = false\n";
	assert_eq!(template.matches(line).count(), 1, "template layout changed");
	let mistyped = template.replace(line, "\ncapabilities = \"yes\"\n");
	let error = toml::from_str::<crate::config::Config>(&mistyped)
		.unwrap_err()
		.to_string();
	assert!(error.contains("capabilities"), "mistyped: {error}");

	let missing = template.replace(line, "\n");
	let error = toml::from_str::<crate::config::Config>(&missing)
		.unwrap_err()
		.to_string();
	assert!(error.contains("capabilities"), "missing: {error}");
}

#[test]
fn absent_and_empty_decision_models_disable_routing_but_keep_flags_required() {
	for model in [None, Some(String::new())] {
		let mut config = template_config();
		config.decision.model = model;
		config.decision.supervisor = true;
		config.validate().unwrap();
		assert!(!crate::supervisor::evaluate::enabled(
			&config,
			crate::supervisor::evaluate::Seam::Gate
		));
	}
	let mut value: toml::Value =
		toml::from_str(include_str!("../../config-templates/default.toml")).unwrap();
	value["decision"].as_table_mut().unwrap().remove("model");
	let config: crate::config::Config = value.try_into().unwrap();
	assert!(config.decision.model.is_none());
}

#[test]
fn decision_rejects_unknown_keys_and_requires_the_section() {
	let template = include_str!("../../config-templates/default.toml");
	for key in [
		"unknown",
		"gate",
		"authorizer",
		"distill",
		"condense",
		"plan",
		"evolution",
	] {
		let raw = template.replace("[decision]", &format!("[decision]\n{key} = false"));
		assert!(toml::from_str::<crate::config::Config>(&raw)
			.unwrap_err()
			.to_string()
			.contains(key));
	}
	let mut value: toml::Value = toml::from_str(template).unwrap();
	value.as_table_mut().unwrap().remove("decision");
	assert!(value
		.try_into::<crate::config::Config>()
		.unwrap_err()
		.to_string()
		.contains("decision"));
}

#[test]
fn test_validate_clean_default_template_has_no_ignored_keys() {
	let template = include_str!("../../config-templates/default.toml");
	let value: toml::Value = toml::from_str(template).unwrap();
	let mut ignored_keys = Vec::new();
	let mut config: crate::config::Config = serde_ignored::deserialize(value, |path| {
		ignored_keys.push(path.to_string());
	})
	.unwrap();
	config.ignored_keys = ignored_keys;

	let reports = config.check_ignored_keys();
	assert!(
		reports.is_empty(),
		"expected 0 ignored keys, got: {reports:?}"
	);
	assert!(config.validate_ignored_keys().is_ok());
}

#[test]
fn test_validate_reports_misspelled_key() {
	let template = include_str!("../../config-templates/default.toml");
	let raw = template.replace("\n[model]\n", "\n[model]\nmax_tokns = 4096\n");
	let value: toml::Value = toml::from_str(&raw).unwrap();
	let mut ignored_keys = Vec::new();
	let mut config: crate::config::Config = serde_ignored::deserialize(value, |path| {
		ignored_keys.push(path.to_string());
	})
	.unwrap();
	config.ignored_keys = ignored_keys;

	let reports = config.check_ignored_keys();
	assert_eq!(reports.len(), 1);
	assert_eq!(reports[0].path, "model.max_tokns");
	assert!(reports[0].hint.is_none());
	assert!(reports[0].format_warning().contains("model.max_tokns"));
	let err = config.validate_ignored_keys().unwrap_err().to_string();
	assert!(err.contains("model.max_tokns"));
}

#[test]
fn test_validate_reports_scalar_after_table_with_hint() {
	let template = include_str!("../../config-templates/default.toml");
	let raw = template.replace("\n[model]\n", "\n[model]\nsandbox = true\n");
	let value: toml::Value = toml::from_str(&raw).unwrap();
	let mut ignored_keys = Vec::new();
	let mut config: crate::config::Config = serde_ignored::deserialize(value, |path| {
		ignored_keys.push(path.to_string());
	})
	.unwrap();
	config.ignored_keys = ignored_keys;

	let reports = config.check_ignored_keys();
	assert_eq!(reports.len(), 1);
	assert_eq!(reports[0].path, "model.sandbox");
	assert!(reports[0].hint.is_some());
	assert!(reports[0]
		.hint
		.as_ref()
		.unwrap()
		.contains("scalar placed after a [header]"));
	let err = config.validate_ignored_keys().unwrap_err().to_string();
	assert!(err.contains("model.sandbox"));
}

#[test]
fn test_validate_reports_scalar_after_nested_table_with_hint() {
	let template = include_str!("../../config-templates/default.toml");
	let raw = template.replace(
		"\n[compression.attention]\n",
		"\n[compression.attention]\nthreshold = 50000\n",
	);
	let value: toml::Value = toml::from_str(&raw).unwrap();
	let mut ignored_keys = Vec::new();
	let mut config: crate::config::Config = serde_ignored::deserialize(value, |path| {
		ignored_keys.push(path.to_string());
	})
	.unwrap();
	config.ignored_keys = ignored_keys;

	let reports = config.check_ignored_keys();
	assert_eq!(reports.len(), 1);
	assert_eq!(reports[0].path, "compression.attention.threshold");
	assert!(reports[0].hint.is_some());
	assert!(reports[0]
		.hint
		.as_ref()
		.unwrap()
		.contains("scalar placed after a [header]"));
	let err = config.validate_ignored_keys().unwrap_err().to_string();
	assert!(err.contains("compression.attention.threshold"));
}
