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

//! Tag → (Config, role_name) resolution.
//!
//! Shared by the `octomind run/acp/server` commands AND by the runtime `/role`
//! command. A tag with `:` (e.g. `developer:general`) triggers tap manifest
//! fetch, INPUT/ENV/dep resolution, and config merge. A plain tag is returned
//! unchanged against the given config.

use crate::agent::{deps, inputs, registry};
use crate::config::{loading::merge_agent_toml, Config};
use anyhow::{Context, Result};

/// Resolve config and role from a TAG argument.
///
/// - `None` / default role → plain role, config unchanged
/// - plain string (no `:`) → plain role name, config unchanged
/// - `"domain:spec"` (contains `:`) → fetch tap manifest, merge into config
///
/// `status_cb` receives human-readable status strings (tap fetch, dep checks)
/// so callers can drive a spinner.
///
/// Returns `(resolved_config, role_name)`.
pub async fn resolve_config_and_role(
	tag: Option<&str>,
	config: &Config,
	status_cb: Option<&(dyn Fn(&str) + Send + Sync)>,
) -> Result<(Config, String)> {
	let tag = tag.unwrap_or(config.default.as_str());

	if tag.contains(':') {
		// Registry agent: fetch manifest, resolve inputs, merge config
		if let Some(cb) = status_cb {
			cb(&format!("Fetching agent: {tag}"));
		}
		let (raw_toml, tap_root) = registry::fetch_manifest(tag, &config.registry)
			.await
			.context(format!("Failed to fetch agent manifest for '{tag}'"))?;
		// Resolve capabilities before input/env/dep resolution
		let resolved_toml =
			registry::resolve_capabilities(&raw_toml, &tap_root, &config.capabilities)
				.context("Failed to resolve agent capabilities")?;
		// INPUT first (persistent credential store), then ENV (environment / .env fallback)
		let resolved_toml = inputs::resolve_inputs(&resolved_toml).await?;
		let resolved_toml = inputs::resolve_env_vars(&resolved_toml).await?;
		// Run dep scripts before MCP init — idempotent, exit 0 if already installed
		deps::resolve_deps(&resolved_toml, &tap_root, status_cb).await?;
		// Always inject the tag as the role name — manifests never need to declare it.
		let tagged_toml = inject_role_name(&resolved_toml, tag)
			.context("Failed to inject role name into agent manifest")?;
		let mut merged = merge_agent_toml(config, &tagged_toml)
			.context("Failed to merge agent manifest into config")?;

		// First role in merged config that isn't in the base config
		let base_names: std::collections::HashSet<&str> =
			config.roles.iter().map(|r| r.name.as_str()).collect();
		let role = merged
			.roles
			.iter()
			.find(|r| !base_names.contains(r.name.as_str()))
			.map(|r| r.name.clone())
			.context(format!(
				"Agent manifest for '{tag}' must define at least one new [[roles]] entry"
			))?;

		// Apply tap model override if configured
		if let Some(tap_model) = config.taps.get(tag) {
			merged.model = tap_model.clone();
			crate::log_debug!("Applied tap model override: {} -> {}", tag, tap_model);
		}

		Ok((merged, role))
	} else {
		// Plain role name — use config as-is
		Ok((config.clone(), tag.to_string()))
	}
}

/// Inject the tag (e.g. `"octomind:tap"`) as the `name` field of the first
/// `[[roles]]` entry in the manifest TOML. This means manifests never need
/// to declare their own name — the tag IS the identity.
fn inject_role_name(toml_str: &str, tag: &str) -> Result<String> {
	// The full tag IS the role identity — "doctor:blood" becomes the role name.
	// This matches what resolve_config_and_role returns as the role string.
	let role_name = tag;

	let mut value: toml::Value =
		toml::from_str(toml_str).context("Failed to parse agent manifest TOML")?;

	if let Some(toml::Value::Array(roles)) = value.get_mut("roles") {
		if let Some(toml::Value::Table(table)) = roles.first_mut() {
			table.insert(
				"name".to_string(),
				toml::Value::String(role_name.to_string()),
			);
		}
	}

	toml::to_string(&value).context("Failed to re-serialize agent manifest TOML")
}
