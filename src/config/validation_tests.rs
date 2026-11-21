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
use crate::config::{HookConfig, McpServerConfig};

fn template_config() -> Config {
	toml::from_str(include_str!("../../config-templates/default.toml")).expect("default template must deserialize")
}

fn hook(name: &str, bind: &str, script: &str, timeout: u64) -> HookConfig {
	HookConfig {
		name: name.to_string(),
		bind: bind.to_string(),
		script: script.to_string(),
		timeout,
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
fn full_validate_accepts_every_optional_section_at_once() {
	let mut config = template_config();
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
fn an_empty_roles_collection_passes_the_required_field_checks() {
	let mut config = template_config();
	config.roles = Vec::new();
	config
		.validate_required_fields()
		.expect("no roles means no per-role checks");
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
	config.validate_thresholds().expect("0 disables the session token cap");
}

#[test]
fn compression_model_errors_surface_through_full_validate() {
	let mut config = template_config();
	config.compression.model.model = Some("not-a-provider:model".to_string());
	let error = config.validate().unwrap_err().to_string();
	assert!(error.contains("compression.model.name"), "got: {error}");
}

#[test]
fn max_identical_rounds_is_a_required_root_key() {
	let template = include_str!("../../config-templates/default.toml");
	let config: crate::config::Config = toml::from_str(template).unwrap();
	assert_eq!(config.max_identical_rounds, 10);

	let line = "max_identical_rounds = 10\n";
	assert!(template.contains(line), "template layout changed");
	let error = toml::from_str::<crate::config::Config>(&template.replace(line, ""))
		.unwrap_err()
		.to_string();
	assert!(error.contains("max_identical_rounds"), "got: {error}");
}

#[test]
fn test_validate_clean_default_template_has_no_ignored_keys() {
	let template = include_str!("../../config-templates/default.toml");
	let value: toml::Value = toml::from_str(template).unwrap();
	let (mut config, ignored_keys) = crate::config::validation::deserialize_with_ignored_keys(value).unwrap();
	config.ignored_keys = ignored_keys;

	let reports = config.check_ignored_keys();
	assert!(reports.is_empty(), "expected 0 ignored keys, got: {reports:?}");
}

#[test]
fn test_validate_reports_misspelled_key() {
	let template = include_str!("../../config-templates/default.toml");
	let raw = template.replace("\n[model]\n", "\n[model]\nmax_tokns = 4096\n");
	let value: toml::Value = toml::from_str(&raw).unwrap();
	let (mut config, ignored_keys) = crate::config::validation::deserialize_with_ignored_keys(value).unwrap();
	config.ignored_keys = ignored_keys;

	let reports = config.check_ignored_keys();
	assert_eq!(reports.len(), 1);
	assert_eq!(reports[0].path, "model.max_tokns");
	assert!(reports[0].hint.is_none());
	assert!(reports[0].format_warning().contains("model.max_tokns"));
}

#[test]
fn test_validate_reports_scalar_after_table_with_hint() {
	let template = include_str!("../../config-templates/default.toml");
	let raw = template.replace("\n[model]\n", "\n[model]\nsandbox = true\n");
	let value: toml::Value = toml::from_str(&raw).unwrap();
	let (mut config, ignored_keys) = crate::config::validation::deserialize_with_ignored_keys(value).unwrap();
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
}

#[test]
fn test_validate_reports_scalar_after_nested_table_with_hint() {
	let template = include_str!("../../config-templates/default.toml");
	let raw = template.replace(
		"\n[compression.attention]\n",
		"\n[compression.attention]\nthreshold = 50000\n",
	);
	let value: toml::Value = toml::from_str(&raw).unwrap();
	let (mut config, ignored_keys) = crate::config::validation::deserialize_with_ignored_keys(value).unwrap();
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
}

#[test]
fn test_validate_reports_ignored_key_in_roles() {
	let template = include_str!("../../config-templates/default.toml");
	let raw = template.replace(
		"name = \"assistant\"\n",
		"name = \"assistant\"\nunknown_role_key = \"bad\"\n",
	);
	let value: toml::Value = toml::from_str(&raw).unwrap();
	let (mut config, ignored_keys) = crate::config::validation::deserialize_with_ignored_keys(value).unwrap();
	config.ignored_keys = ignored_keys;

	let reports = config.check_ignored_keys();
	assert!(
		reports.iter().any(|r| r.path == "roles.0.unknown_role_key"),
		"expected roles.0.unknown_role_key, got: {reports:?}"
	);
}

#[test]
fn test_validate_reports_ignored_key_in_roles_model() {
	let template = include_str!("../../config-templates/default.toml");
	let raw = template.replace(
		"allowed_tools = [\"core:*\", \"orchestration:*\", \"runtime:*\", \"filesystem:*\", \"agent:*\"]\n",
		"allowed_tools = [\"core:*\", \"orchestration:*\", \"runtime:*\", \"filesystem:*\", \"agent:*\"]\n\n[roles.model]\nunknown_model_key = \"bad\"\n",
	);
	let value: toml::Value = toml::from_str(&raw).unwrap();
	let (mut config, ignored_keys) = crate::config::validation::deserialize_with_ignored_keys(value).unwrap();
	config.ignored_keys = ignored_keys;

	let reports = config.check_ignored_keys();
	assert!(
		reports.iter().any(|r| r.path == "roles.0.model.unknown_model_key"),
		"expected roles.0.model.unknown_model_key, got: {reports:?}"
	);
}

#[test]
fn deprecated_sampling_keys_load_and_use_the_deprecation_report_for_config_and_tap_roles() {
	let template = include_str!("../../config-templates/default.toml");
	let raw = template
		.replace("\n[model]\n", "\n[model]\ntemperature = 0.4\ntop_p = 0.8\ntop_k = 40\n")
		.replace(
			"\n[compression.model]\n",
			"\n[compression.model]\ntemperature = 0.3\ntop_p = 1.0\ntop_k = 0\n",
		) + r#"

[[roles]]
name = "legacy:flat"
temperature = 0.2
top_p = 0.9
top_k = 10
system = "legacy flat tap role"
welcome = ""

[[roles]]
name = "legacy:nested"
system = "legacy nested tap role"
welcome = ""

[roles.model]
temperature = 0.1
top_p = 0.7
top_k = 5
"#;
	let value: toml::Value = toml::from_str(&raw).unwrap();
	let role_count = value["roles"].as_array().unwrap().len();
	let flat_index = role_count - 2;
	let nested_index = role_count - 1;
	let (mut config, ignored_keys) = crate::config::validation::deserialize_with_ignored_keys(value).unwrap();
	config.ignored_keys = ignored_keys;
	config.build_role_map();

	let reports = config.check_ignored_keys();
	let expected_paths = [
		"model.temperature".to_string(),
		"model.top_p".to_string(),
		"model.top_k".to_string(),
		"compression.model.temperature".to_string(),
		"compression.model.top_p".to_string(),
		"compression.model.top_k".to_string(),
		format!("roles.{flat_index}.temperature"),
		format!("roles.{flat_index}.top_p"),
		format!("roles.{flat_index}.top_k"),
		format!("roles.{nested_index}.model.temperature"),
		format!("roles.{nested_index}.model.top_p"),
		format!("roles.{nested_index}.model.top_k"),
	];
	assert_eq!(reports.len(), expected_paths.len(), "got: {reports:?}");
	for path in expected_paths {
		let report = reports
			.iter()
			.find(|report| report.path == path)
			.unwrap_or_else(|| panic!("missing report for {path}; got: {reports:?}"));
		assert_eq!(
			report.format_warning(),
			format!("warning: `{path}` is deprecated and ignored (sampling parameters are no longer sent); remove it")
		);
		assert!(!report.format_warning().contains("unknown configuration key"));
	}
	assert_eq!(
		config.get_model_profile_for_role("legacy:flat"),
		config.model_profile,
		"flat sampling keys must have no effect"
	);
	assert_eq!(
		config.get_model_profile_for_role("legacy:nested"),
		config.model_profile,
		"nested sampling keys must have no effect"
	);
}

#[test]
fn test_validate_reports_ignored_key_in_mcp_servers() {
	let template = include_str!("../../config-templates/default.toml");
	let raw = template.replace(
		"name = \"orchestration\"\n",
		"name = \"orchestration\"\nunknown_mcp_key = \"bad\"\n",
	);
	let value: toml::Value = toml::from_str(&raw).unwrap();
	let (mut config, ignored_keys) = crate::config::validation::deserialize_with_ignored_keys(value).unwrap();
	config.ignored_keys = ignored_keys;

	let reports = config.check_ignored_keys();
	assert!(
		reports
			.iter()
			.any(|r| r.path.starts_with("mcp.servers.") && r.path.ends_with(".unknown_mcp_key")),
		"expected mcp.servers.*.unknown_mcp_key, got: {reports:?}"
	);
}

#[test]
fn test_validate_accepts_every_mcp_server_field() {
	// Every variant fully populated. A field added to `McpServerConfig` has to be added
	// here (struct literal), and this then fails until the matching `*_SERVER_KEYS` list in
	// `validation.rs` knows it too.
	let map = std::collections::HashMap::from([("KEY".to_string(), "value".to_string())]);
	let servers = [
		McpServerConfig::Builtin {
			name: "drift_builtin".to_string(),
			timeout_seconds: 30,
			tools: vec!["tool".to_string()],
			auto_bind: Some(vec!["developer".to_string()]),
		},
		McpServerConfig::Http {
			name: "drift_http".to_string(),
			url: "http://127.0.0.1:1".to_string(),
			timeout_seconds: 30,
			tools: vec!["tool".to_string()],
			headers: map.clone(),
			auto_bind: Some(vec!["developer".to_string()]),
		},
		McpServerConfig::Stdin {
			name: "drift_stdio".to_string(),
			command: "true".to_string(),
			args: vec!["arg".to_string()],
			timeout_seconds: 30,
			tools: vec!["tool".to_string()],
			env: map,
			cwd: Some(".".to_string()),
			auto_bind: Some(vec!["developer".to_string()]),
		},
	];
	let template = include_str!("../../config-templates/default.toml");
	let mut value: toml::Value = toml::from_str(template).unwrap();
	let template_servers = value
		.get_mut("mcp")
		.and_then(|mcp| mcp.get_mut("servers"))
		.and_then(toml::Value::as_array_mut)
		.expect("template has [[mcp.servers]]");
	for server in &servers {
		template_servers.push(toml::Value::try_from(server).expect("server serializes"));
	}

	let (_, ignored_keys) = crate::config::validation::deserialize_with_ignored_keys(value).unwrap();
	assert!(
		ignored_keys.is_empty(),
		"every McpServerConfig field must be known, got: {ignored_keys:?}"
	);
}
