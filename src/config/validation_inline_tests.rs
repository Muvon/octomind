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

fn template_config() -> Config {
	toml::from_str(include_str!("../../config-templates/default.toml")).expect("default template must deserialize")
}

use crate::config::{HookConfig, McpServerConfig};

fn hook(name: &str, bind: &str, script: &str, timeout: u64) -> HookConfig {
	HookConfig {
		name: name.to_string(),
		bind: bind.to_string(),
		script: script.to_string(),
		timeout,
	}
}

#[test]
fn template_config_passes_full_validation() {
	template_config()
		.validate()
		.expect("the shipped default configuration must validate");
}

#[test]
fn validate_rejects_an_empty_model() {
	let mut config = template_config();
	config.model.clear();
	let error = config.validate().unwrap_err().to_string();
	assert!(error.contains("main.name cannot be empty"), "got: {error}");
}

#[test]
fn session_token_threshold_caps_at_two_million() {
	let mut config = template_config();
	config.max_session_tokens_threshold = 2_000_001;
	let error = config.validate_thresholds().unwrap_err().to_string();
	assert!(error.contains("2,000,000"), "got: {error}");

	config.max_session_tokens_threshold = 2_000_000;
	config.validate_thresholds().expect("the boundary itself must pass");
}

#[test]
fn markdown_theme_cannot_be_empty() {
	let mut config = template_config();
	config.markdown_theme.clear();
	let error = config.validate_required_fields().unwrap_err().to_string();
	assert!(error.contains("Markdown theme field cannot be empty"), "got: {error}");
}

#[test]
fn markdown_theme_rejects_unknown_values_and_accepts_case_insensitive_names() {
	let mut config = template_config();
	config.markdown_theme = "solarised".into();
	let error = config.validate_required_fields().unwrap_err().to_string();
	assert!(error.contains("solarised"), "got: {error}");
	for theme in ["Dark", "monokai"] {
		config.markdown_theme = theme.into();
		config
			.validate_required_fields()
			.unwrap_or_else(|error| panic!("{theme} should be accepted: {error}"));
	}
}

#[test]
fn hooks_validation_accepts_well_formed_hooks() {
	let mut config = template_config();
	config.hooks = vec![
		hook("deploy", "127.0.0.1:9876", "./hooks/deploy.sh", 30),
		hook("notify", "0.0.0.0:9999", "./hooks/notify.sh", 3600),
	];
	config
		.validate_hooks()
		.expect("valid hooks (3600s boundary included) must pass");
}

#[test]
fn hooks_validation_rejects_each_malformed_shape() {
	let cases = [
		("empty name", hook("", "127.0.0.1:1", "s.sh", 30), "empty name"),
		("empty bind", hook("a", "", "s.sh", 30), "empty bind address"),
		(
			"invalid bind",
			hook("a", "not-an-address", "s.sh", 30),
			"invalid bind address",
		),
		("empty script", hook("a", "127.0.0.1:1", "", 30), "empty script path"),
		(
			"zero timeout",
			hook("a", "127.0.0.1:1", "s.sh", 0),
			"timeout must be > 0",
		),
		(
			"over-large timeout",
			hook("a", "127.0.0.1:1", "s.sh", 3601),
			"timeout too high",
		),
	];
	for (description, bad, needle) in cases {
		let mut config = template_config();
		config.hooks = vec![bad];
		let error = config.validate_hooks().unwrap_err().to_string();
		assert!(
			error.contains(needle),
			"{description} must fail with '{needle}', got: {error}"
		);
	}
}

#[test]
fn hooks_validation_rejects_duplicate_names_and_bind_addresses() {
	let mut config = template_config();
	config.hooks = vec![
		hook("first", "127.0.0.1:1111", "a.sh", 30),
		hook("first", "127.0.0.1:2222", "b.sh", 30),
	];
	assert!(config
		.validate_hooks()
		.unwrap_err()
		.to_string()
		.contains("Duplicate hook name"));

	config.hooks = vec![
		hook("first", "127.0.0.1:1111", "a.sh", 30),
		hook("second", "127.0.0.1:1111", "b.sh", 30),
	];
	assert!(config
		.validate_hooks()
		.unwrap_err()
		.to_string()
		.contains("duplicate bind address"));
}

#[test]
fn mcp_validation_rejects_zero_and_over_large_timeouts() {
	let mut config = template_config();
	config.mcp.servers = vec![McpServerConfig::stdin("local", "node", vec![], 0, vec![])];
	let error = config.validate_mcp_config().unwrap_err().to_string();
	assert!(error.contains("invalid timeout"), "got: {error}");

	config.mcp.servers = vec![McpServerConfig::stdin("local", "node", vec![], 3601, vec![])];
	let error = config.validate_mcp_config().unwrap_err().to_string();
	assert!(error.contains("too high"), "got: {error}");
}

#[test]
fn mcp_validation_rejects_empty_url_and_command() {
	let mut config = template_config();
	config.mcp.servers = vec![McpServerConfig::http("remote", "", 30, vec![])];
	let error = config.validate_mcp_config().unwrap_err().to_string();
	assert!(error.contains("HTTP server URL cannot be empty"), "got: {error}");
	config.mcp.servers = vec![McpServerConfig::stdin("local", "", vec![], 30, vec![])];
	let error = config.validate_mcp_config().unwrap_err().to_string();
	assert!(error.contains("Stdin server command cannot be empty"), "got {error}");
}

#[test]
fn mcp_validation_accepts_boundary_timeouts_and_every_server_kind() {
	let mut config = template_config();
	config.mcp.servers = vec![
		McpServerConfig::stdin("local", "node", vec![], 3600, vec![]),
		McpServerConfig::builtin("core", 30, vec![]),
		McpServerConfig::http("remote", "https://example.com/mcp", 30, vec![]),
	];
	config
		.validate_mcp_config()
		.expect("servers at legal timeouts must pass");
}

#[test]
fn compression_model_check_skips_when_compression_is_disabled() {
	let mut config = template_config();
	config.compression.threshold = 0;
	config.compression.model.model = Some(String::new());
	config
		.validate_model_profiles()
		.expect("threshold 0 means there is no compression call to validate");
}

#[test]
fn compression_model_check_requires_a_resolvable_model() {
	let mut config = template_config();
	let shipped = config.get_compression_model_profile().model;
	config.compression.model.model = Some(String::new());
	let error = config.validate_model_profiles().unwrap_err().to_string();
	assert!(error.contains("compression.model.name cannot be empty"), "got: {error}");

	config.compression.model.model = Some("not-a-provider:model".to_string());
	let error = config.validate_model_profiles().unwrap_err().to_string();
	assert!(
		error.contains("compression.model.name 'not-a-provider:model' is invalid"),
		"got: {error}"
	);

	config.compression.model.model = Some(shipped);
	config
		.validate_model_profiles()
		.expect("the shipped decision model must resolve");
}
