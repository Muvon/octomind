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

		self.validate_model_profiles()?;

		self.validate_evaluate_model()?;

		Ok(())
	}

	/// `decision.model` must be `provider:model` on a provider that
	/// serves that evaluation model — octolib's evaluation factory is the list,
	/// and it resolves the pair without touching the network. Checked even when
	/// the supervisor is off: a typo here is a typo regardless of which switches
	/// are on. An absent or empty model deliberately disables decision routing.
	pub(super) fn validate_evaluate_model(&self) -> Result<()> {
		use octolib::evaluation::EvaluationProviderFactory;
		let Some(model) = self
			.decision
			.model
			.as_deref()
			.filter(|model| !model.is_empty())
		else {
			return Ok(());
		};
		EvaluationProviderFactory::get_provider_for_model(model)
			.map(|_| ())
			.map_err(|error| anyhow!("decision.model: {error}"))
	}

	fn validate_model_profiles(&self) -> Result<()> {
		self.model_profile.validate("main")?;
		for role in &self.roles {
			role.config
				.model_override()
				.resolve(&self.model_profile)
				.validate(&format!("roles.{}", role.name))?;
		}
		for (tag, model) in &self.taps {
			let mut profile = self.model_profile.clone();
			profile.model = model.clone();
			profile.validate(&format!("taps.{tag}"))?;
		}
		if self.supervisor.enabled {
			self.get_supervisor_model_profile().validate("supervisor")?;
		}
		if self.compression.threshold != 0 {
			self.get_compression_model_profile()
				.validate("compression.model")?;
		}
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
		if self.markdown_theme.is_empty() {
			return Err(anyhow!("Markdown theme field cannot be empty"));
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

	/// Check for any unknown/ignored keys captured during deserialization.
	pub fn check_ignored_keys(&self) -> Vec<IgnoredKeyReport> {
		self.ignored_keys
			.iter()
			.map(|path| IgnoredKeyReport {
				path: path.clone(),
				hint: detect_scalar_after_table_hint(path),
			})
			.collect()
	}
}

/// Report for a configuration key ignored by serde during deserialization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IgnoredKeyReport {
	pub path: String,
	pub hint: Option<String>,
}

impl IgnoredKeyReport {
	pub fn format_warning(&self) -> String {
		format!("warning: unknown configuration key `{}`", self.path)
	}
}

static DEFAULT_TEMPLATE: std::sync::LazyLock<toml::Value> = std::sync::LazyLock::new(|| {
	let template = include_str!("../../config-templates/default.toml");
	toml::from_str(template).expect("config-templates/default.toml must parse")
});

/// Root `[model].name` shipped in `config-templates/default.toml`.
pub fn default_model() -> &'static str {
	DEFAULT_TEMPLATE
		.get("model")
		.and_then(|m| m.get("name"))
		.and_then(|n| n.as_str())
		.expect("config-templates/default.toml must define [model].name")
}

/// Field names of each `McpServerConfig` variant, `type` tag included.
/// `test_validate_accepts_every_mcp_server_field` fails when these drift from the enum.
const BUILTIN_SERVER_KEYS: &[&str] = &["type", "name", "timeout_seconds", "tools", "auto_bind"];
const HTTP_SERVER_KEYS: &[&str] = &[
	"type",
	"name",
	"url",
	"timeout_seconds",
	"tools",
	"headers",
	"auto_bind",
];
const STDIO_SERVER_KEYS: &[&str] = &[
	"type",
	"name",
	"command",
	"args",
	"timeout_seconds",
	"tools",
	"env",
	"cwd",
	"auto_bind",
];

fn section_has_field(section: &str, field: &str) -> bool {
	let mut current: &toml::Value = &DEFAULT_TEMPLATE;
	if !section.is_empty() {
		for segment in section.split('.') {
			if let Ok(idx) = segment.parse::<usize>() {
				if let Some(arr) = current.as_array() {
					if let Some(item) = arr.get(idx).or_else(|| arr.first()) {
						current = item;
						continue;
					}
				}
				return false;
			} else if let Some(next) = current.get(segment) {
				current = next;
			} else if let Some(arr) = current.as_array() {
				if let Some(item) = arr.first().and_then(|v| v.get(segment)) {
					current = item;
				} else {
					return false;
				}
			} else {
				return false;
			}
		}
	}

	match current {
		toml::Value::Table(table) => table.contains_key(field),
		toml::Value::Array(arr) => arr
			.iter()
			.any(|v| v.as_table().map(|t| t.contains_key(field)).unwrap_or(false)),
		_ => false,
	}
}

/// Deserialize a TOML value into Config, capturing unknown/ignored keys across
/// the root configuration, flattened `roles`, `roles.model`, and tagged `mcp.servers`.
pub fn deserialize_with_ignored_keys(value: toml::Value) -> Result<(Config, Vec<String>)> {
	let mut ignored_keys = Vec::new();

	let config: Config = serde_ignored::deserialize(value.clone(), |path| {
		ignored_keys.push(path.to_string());
	})
	.map_err(|e| anyhow!("Failed to parse TOML configuration: {e}"))?;

	// Roles: serde_ignored cannot penetrate #[serde(flatten)] on Role,
	// so re-run deserialization on each role table into RoleConfig.
	if let Some(roles) = value.get("roles").and_then(toml::Value::as_array) {
		for (i, role) in roles.iter().enumerate() {
			if let Some(role_table) = role.as_table() {
				let role_val = toml::Value::Table(role_table.clone());
				let _: Result<crate::config::roles::RoleConfig, _> =
					serde_ignored::deserialize(role_val, |path| {
						let p = path.to_string();
						if p == "name" || p == "mcp" || p.starts_with("mcp.") {
							return;
						}
						ignored_keys.push(format!("roles.{i}.{p}"));
					});
				if let Some(mcp) = role_table.get("mcp") {
					let _: Result<crate::config::mcp::RoleMcpConfig, _> =
						serde_ignored::deserialize(mcp.clone(), |path| {
							ignored_keys.push(format!("roles.{i}.mcp.{path}"));
						});
				}
			}
		}
	}

	// MCP Servers: serde_ignored cannot penetrate #[serde(tag = "type")] on McpServerConfig,
	// so compare each server table's keys against its variant's fields.
	if let Some(servers) = value
		.get("mcp")
		.and_then(|m| m.get("servers"))
		.and_then(toml::Value::as_array)
	{
		for (i, server) in servers.iter().enumerate() {
			let Some(server_table) = server.as_table() else {
				continue;
			};
			let known = match server_table.get("type").and_then(toml::Value::as_str) {
				Some("builtin") => BUILTIN_SERVER_KEYS,
				Some("http") => HTTP_SERVER_KEYS,
				Some("stdio") => STDIO_SERVER_KEYS,
				_ => continue,
			};
			ignored_keys.extend(
				server_table
					.keys()
					.filter(|key| !known.contains(&key.as_str()))
					.map(|key| format!("mcp.servers.{i}.{key}")),
			);
		}
	}

	let mut seen = std::collections::HashSet::new();
	ignored_keys.retain(|k| seen.insert(k.clone()));

	Ok((config, ignored_keys))
}

/// Detects if an ignored dotted path looks like a scalar placed after a table header:
/// the key is unknown to its table but a field of the section enclosing that table.
pub fn detect_scalar_after_table_hint(path: &str) -> Option<String> {
	let (table, key) = path.rsplit_once('.')?;
	let parent = table.rsplit_once('.').map_or("", |(parent, _)| parent);
	section_has_field(parent, key).then(|| {
		"in TOML, a scalar placed after a [header] becomes a field of that table; did you mean to place this in the parent section before the header?".to_string()
	})
}

#[cfg(test)]
#[path = "validation_inline_tests.rs"]
mod inline_tests;

#[cfg(test)]
#[path = "validation_tests.rs"]
mod validation_tests;
