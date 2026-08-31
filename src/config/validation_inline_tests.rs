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

/// validate_layers doesn't use `self` — it only inspects the layers slice.
/// We replicate the logic here to test it without needing a full Config.
fn validate_layer_rules(layers: &[LayerConfig]) -> Result<()> {
	for (index, layer) in layers.iter().enumerate() {
		if layer.name.is_empty() {
			return Err(anyhow!("Layer at index {} has empty name", index));
		}
		if layer.description.is_empty() {
			return Err(anyhow!(
				"Layer '{}' at index {} has empty description",
				layer.name,
				index
			));
		}
		if layer.command.is_empty() {
			return Err(anyhow!(
					"Layer '{}' at index {} has empty command. Layers now execute via ACP protocol — add a 'command' field (e.g., command = 'octomind acp <role>')",
					layer.name,
					index
				));
		}
	}
	Ok(())
}

#[test]
fn validate_layers_empty_command_fails() {
	let mut layer = valid_layer();
	layer.command = String::new();
	let result = validate_layer_rules(&[layer]);
	assert!(result.is_err(), "empty command should fail validation");
	let err = result.unwrap_err().to_string();
	assert!(
		err.contains("empty command"),
		"error should mention 'empty command', got: {err}"
	);
}

#[test]
fn validate_layers_valid_command_passes() {
	let layer = valid_layer();
	let result = validate_layer_rules(&[layer]);
	assert!(result.is_ok(), "valid layer should pass validation");
}

#[test]
fn validate_layers_empty_name_fails() {
	let mut layer = valid_layer();
	layer.name = String::new();
	let result = validate_layer_rules(&[layer]);
	assert!(result.is_err(), "empty name should fail validation");
}

#[test]
fn validate_layers_empty_description_fails() {
	let mut layer = valid_layer();
	layer.description = String::new();
	let result = validate_layer_rules(&[layer]);
	assert!(result.is_err(), "empty description should fail validation");
}

#[test]
fn enabled_supervisor_requires_a_model() {
	let mut config = template_config();
	config.supervisor.model.model = Some(String::new());
	assert!(config.validate_model_profiles().is_err());

	config.supervisor.enabled = false;
	assert!(config.validate_model_profiles().is_ok());
}
use crate::config::{HookConfig, McpServerConfig, Role, RoleConfig};

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
	assert!(
		error.contains("Model field cannot be empty"),
		"got: {error}"
	);
}

#[test]
fn layers_validation_runs_through_the_real_validate_path() {
	let mut config = template_config();
	config.layers = Some(vec![valid_layer()]);
	config
		.validate()
		.expect("a well-formed layer must pass full validation");

	let mut bad = valid_layer();
	bad.name = String::new();
	config.layers = Some(vec![bad]);
	let error = config.validate().unwrap_err().to_string();
	assert!(error.contains("empty name"), "got: {error}");
}

#[test]
fn session_token_threshold_caps_at_two_million() {
	let mut config = template_config();
	config.max_session_tokens_threshold = 2_000_001;
	let error = config.validate_thresholds().unwrap_err().to_string();
	assert!(error.contains("2,000,000"), "got: {error}");

	config.max_session_tokens_threshold = 2_000_000;
	config
		.validate_thresholds()
		.expect("the boundary itself must pass");
}

#[test]
fn cache_keepalive_idle_cap_allows_a_day_and_zero() {
	let mut config = template_config();
	config.cache_keepalive_max_idle_seconds = 86401;
	assert!(config.validate_thresholds().is_err());

	config.cache_keepalive_max_idle_seconds = 86400;
	config.validate_thresholds().expect("exactly 24h must pass");

	config.cache_keepalive_max_idle_seconds = 0;
	config
		.validate_thresholds()
		.expect("zero means unbounded and must pass");
}

#[test]
fn role_sampling_bounds_are_enforced_with_inclusive_boundaries() {
	let mut config = template_config();
	config.roles = vec![
		role_with("lower-edge", 0.0, 0.0, 1),
		role_with("upper-edge", 2.0, 1.0, 1000),
	];
	config
		.validate_required_fields()
		.expect("both edges are legal values");

	let cases = [
		("too-hot", 2.1, 1.0, 1000, "temperature"),
		("too-cold", -0.1, 1.0, 1000, "temperature"),
		("too-wide", 1.0, 1.1, 1000, "top_p"),
		("too-narrow", 1.0, -0.1, 1000, "top_p"),
		("no-choices", 1.0, 1.0, 0, "top_k"),
		("too-many", 1.0, 1.0, 1001, "top_k"),
	];
	for (name, temperature, top_p, top_k, knob) in cases {
		config.roles = vec![role_with(name, temperature, top_p, top_k)];
		let error = config.validate_required_fields().unwrap_err().to_string();
		assert!(error.contains(name), "must name the role, got: {error}");
		assert!(error.contains(knob), "must name {knob}, got: {error}");
	}
}

#[test]
fn markdown_theme_cannot_be_empty() {
	let mut config = template_config();
	config.markdown_theme.clear();
	let error = config.validate_required_fields().unwrap_err().to_string();
	assert!(
		error.contains("Markdown theme field cannot be empty"),
		"got: {error}"
	);
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
		(
			"empty name",
			hook("", "127.0.0.1:1", "s.sh", 30),
			"empty name",
		),
		(
			"empty bind",
			hook("a", "", "s.sh", 30),
			"empty bind address",
		),
		(
			"invalid bind",
			hook("a", "not-an-address", "s.sh", 30),
			"invalid bind address",
		),
		(
			"empty script",
			hook("a", "127.0.0.1:1", "", 30),
			"empty script path",
		),
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

	config.mcp.servers = vec![McpServerConfig::stdin(
		"local",
		"node",
		vec![],
		3601,
		vec![],
	)];
	let error = config.validate_mcp_config().unwrap_err().to_string();
	assert!(error.contains("too high"), "got: {error}");
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
	assert!(
		error.contains("compression.model.name cannot be empty"),
		"got: {error}"
	);

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

#[test]
fn supervisor_model_is_not_required_when_the_supervisor_is_off() {
	let mut config = template_config();
	config.supervisor.model.model = Some(String::new());
	config.supervisor.enabled = false;
	config
		.validate_model_profiles()
		.expect("a disabled supervisor never runs the planner");
}
