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

use anyhow::{anyhow, Result};

use super::Config;

impl Config {
	/// Validate the configuration for common issues - STRICT MODE
	/// All validation errors are now fatal in strict mode
	pub fn validate(&self) -> Result<()> {
		// Validate threshold values - STRICT
		self.validate_thresholds()?;

		// Validate MCP configuration - STRICT
		self.validate_mcp_config()?;

		// Validate layer configuration if present - STRICT
		if let Some(layers) = &self.layers {
			self.validate_layers(layers)?;
		}

		// Validate webhook hooks - STRICT
		self.validate_hooks()?;

		// STRICT: Validate required fields are not empty
		self.validate_required_fields()?;

		// Optional mechanics are validated only when reachable.
		self.validate_supervisor_plan()?;

		// Compression model: must resolve to a known provider, but EITHER
		// structured-output support (JSON path) OR no support (XML path)
		// is acceptable — `prepare_decision` dispatches on the
		// provider's capability at call time.
		self.validate_compression_model()?;

		Ok(())
	}

	fn validate_supervisor_plan(&self) -> Result<()> {
		if !self.supervisor.enabled || !self.supervisor.plan.enabled {
			return Ok(());
		}
		let plan = &self.supervisor.plan;
		if plan.model.trim().is_empty() {
			return Err(anyhow!(
				"supervisor.plan.model cannot be empty while the external planner is enabled"
			));
		}
		Ok(())
	}

	/// Verify the configured compression model resolves to a known
	/// provider. The runtime picks JSON or XML wire mode from the
	/// provider's `supports_structured_output(model)` capability, so
	/// either capability is acceptable — we only fail when the model
	/// string itself is missing or unresolvable.
	///
	/// Skipped when compression is effectively disabled (threshold = 0)
	/// — there is no compression call to validate against.
	fn validate_compression_model(&self) -> Result<()> {
		if self.compression.threshold == 0 {
			return Ok(());
		}

		let model = &self.compression.decision.model;
		if model.is_empty() {
			return Err(anyhow!(
				"compression.decision.model is empty — set it to a model resolvable by a configured provider (e.g. anthropic:claude-sonnet-4-6, openai:gpt-4.1)"
			));
		}

		crate::providers::ProviderFactory::get_provider_for_model(model)
			.map_err(|e| anyhow!("compression.decision.model '{}' is invalid: {}", model, e))?;

		Ok(())
	}

	/// Validate webhook hook configurations
	fn validate_hooks(&self) -> Result<()> {
		let mut seen_names = std::collections::HashSet::new();
		let mut seen_binds = std::collections::HashSet::new();

		for hook in &self.hooks {
			if hook.name.is_empty() {
				return Err(anyhow!("Hook has empty name"));
			}
			if !seen_names.insert(&hook.name) {
				return Err(anyhow!("Duplicate hook name: '{}'", hook.name));
			}
			if hook.bind.is_empty() {
				return Err(anyhow!("Hook '{}' has empty bind address", hook.name));
			}
			if !seen_binds.insert(&hook.bind) {
				return Err(anyhow!(
					"Hook '{}' has duplicate bind address '{}' (already used by another hook)",
					hook.name,
					hook.bind
				));
			}
			if hook.bind.parse::<std::net::SocketAddr>().is_err() {
				return Err(anyhow!(
					"Hook '{}' has invalid bind address: '{}'",
					hook.name,
					hook.bind
				));
			}
			if hook.script.is_empty() {
				return Err(anyhow!("Hook '{}' has empty script path", hook.name));
			}
			if hook.timeout == 0 {
				return Err(anyhow!("Hook '{}' timeout must be > 0", hook.name));
			}
			if hook.timeout > 3600 {
				return Err(anyhow!(
					"Hook '{}' timeout too high: {}s (max 3600)",
					hook.name,
					hook.timeout
				));
			}
		}
		Ok(())
	}

	/// Validate that all required fields are present and not empty
	fn validate_required_fields(&self) -> Result<()> {
		if self.model.is_empty() {
			return Err(anyhow!("Model field cannot be empty"));
		}

		if self.markdown_theme.is_empty() {
			return Err(anyhow!("Markdown theme field cannot be empty"));
		}

		// Validate role configurations
		for role in &self.roles {
			// Validate temperature
			if role.config.temperature < 0.0 || role.config.temperature > 2.0 {
				return Err(anyhow!(
					"Role '{}' temperature must be between 0.0 and 2.0, got: {}",
					role.name,
					role.config.temperature
				));
			}

			// Validate top_p
			if role.config.top_p < 0.0 || role.config.top_p > 1.0 {
				return Err(anyhow!(
					"Role '{}' top_p must be between 0.0 and 1.0, got: {}",
					role.name,
					role.config.top_p
				));
			}

			// Validate top_k
			if role.config.top_k < 1 || role.config.top_k > 1000 {
				return Err(anyhow!(
					"Role '{}' top_k must be between 1 and 1000, got: {}",
					role.name,
					role.config.top_k
				));
			}
		}

		Ok(())
	}

	pub fn validate_thresholds(&self) -> Result<()> {
		// Validate max session tokens threshold (0 = disabled, >0 = enabled)
		if self.max_session_tokens_threshold > 2_000_000 {
			return Err(anyhow!(
				"Max session tokens threshold too high: {}. Maximum allowed: 2,000,000",
				self.max_session_tokens_threshold
			));
		}

		// Validate cache keepalive max idle (0 = unbounded, otherwise cap at 24h
		// so a typo can't burn through credit on an abandoned session).
		if self.cache_keepalive_max_idle_seconds > 86400 {
			return Err(anyhow!(
				"Cache keepalive max idle too high: {} seconds. Maximum allowed: 86400 (24 hours), or 0 for unbounded",
				self.cache_keepalive_max_idle_seconds
			));
		}

		Ok(())
	}

	fn validate_mcp_config(&self) -> Result<()> {
		// Validate server configurations
		for server_config in &self.mcp.servers {
			let server_name = &server_config.name();
			// Validate timeout
			if server_config.timeout_seconds() == 0 {
				return Err(anyhow!(
					"Server '{}' has invalid timeout: 0. Must be greater than 0",
					server_name
				));
			}

			if server_config.timeout_seconds() > 3600 {
				// 1 hour max
				return Err(anyhow!(
					"Server '{}' timeout too high: {} seconds. Maximum allowed: 3600 (1 hour)",
					server_name,
					server_config.timeout_seconds()
				));
			}

			// Validate external server configuration
			if matches!(
				server_config.connection_type(),
				crate::config::McpConnectionType::Http
			) {
				if server_config.url().is_none() && server_config.command().is_none() {
					return Err(anyhow!(
						"External server '{}' must have either 'url' or 'command' specified",
						server_name
					));
				}

				if server_config.url().is_some() && server_config.command().is_some() {
					return Err(anyhow!(
						"External server '{}' cannot have both 'url' and 'command' specified",
						server_name
					));
				}
			}
		}

		Ok(())
	}

	fn validate_layers(&self, layers: &[crate::session::layers::LayerConfig]) -> Result<()> {
		for (index, layer) in layers.iter().enumerate() {
			// Validate layer name
			if layer.name.is_empty() {
				return Err(anyhow!("Layer at index {} has empty name", index));
			}

			// Validate layer description
			if layer.description.is_empty() {
				return Err(anyhow!(
					"Layer '{}' at index {} has empty description",
					layer.name,
					index
				));
			}

			// Validate layer command (required for ACP execution)
			if layer.command.is_empty() {
				return Err(anyhow!(
					"Layer '{}' at index {} has empty command. Layers now execute via ACP protocol — add a 'command' field (e.g., command = 'octomind acp <role>')",
					layer.name,
					index
				));
			}

			// Additional layer-specific validation can be added here
		}

		Ok(())
	}
}

#[cfg(test)]
mod tests {
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
	fn enabled_external_planner_requires_a_model() {
		let mut config = template_config();
		config.supervisor.plan.model.clear();
		assert!(config.validate_supervisor_plan().is_err());

		config.supervisor.plan.enabled = false;
		assert!(config.validate_supervisor_plan().is_ok());
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
				model: None,
				system: "system prompt".to_string(),
				welcome: "welcome".to_string(),
				temperature,
				top_p,
				top_k,
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
		config.compression.decision.model.clear();
		config
			.validate_compression_model()
			.expect("threshold 0 means there is no compression call to validate");
	}

	#[test]
	fn compression_model_check_requires_a_resolvable_model() {
		let mut config = template_config();
		let shipped = config.compression.decision.model.clone();
		config.compression.decision.model.clear();
		let error = config.validate_compression_model().unwrap_err().to_string();
		assert!(
			error.contains("compression.decision.model is empty"),
			"got: {error}"
		);

		config.compression.decision.model = "not-a-provider:model".to_string();
		let error = config.validate_compression_model().unwrap_err().to_string();
		assert!(
			error.contains("compression.decision.model 'not-a-provider:model' is invalid"),
			"got: {error}"
		);

		config.compression.decision.model = shipped;
		config
			.validate_compression_model()
			.expect("the shipped decision model must resolve");
	}

	#[test]
	fn supervisor_plan_model_is_not_required_when_the_supervisor_is_off() {
		let mut config = template_config();
		config.supervisor.plan.model.clear();
		config.supervisor.enabled = false;
		config
			.validate_supervisor_plan()
			.expect("a disabled supervisor never runs the planner");
	}
}
