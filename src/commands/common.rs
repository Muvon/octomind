// Copyright 2025 Muvon Un Limited
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

//! Shared helpers used by run, acp, and server commands.

use anyhow::{Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use octomind::agent::{inputs, registry};
use octomind::config::{loading::merge_agent_toml, Config};
use std::time::Duration;

/// Resolve config and role from a TAG argument.
///
/// - `None` / `"developer"` → plain role, config unchanged
/// - `"some_role"` (no `:`) → plain role name, config unchanged
/// - `"domain:spec"` (contains `:`) → fetch registry manifest, merge into config
///
/// Returns `(resolved_config, role_name)`.
pub async fn resolve_config_and_role(
	tag: Option<&str>,
	config: &Config,
) -> Result<(Config, String)> {
	let tag = tag.unwrap_or("developer");

	if tag.contains(':') {
		// Registry agent: fetch manifest, resolve inputs, merge config
		let raw_toml = registry::fetch_manifest(tag, &config.registry)
			.await
			.context(format!("Failed to fetch agent manifest for '{tag}'"))?;
		// INPUT first (persistent credential store), then ENV (environment / .env fallback)
		let resolved_toml = inputs::resolve_inputs(&raw_toml).await?;
		let resolved_toml = inputs::resolve_env_vars(&resolved_toml).await?;
		// Always inject the tag as the role name — manifests never need to declare it.
		let tagged_toml = inject_role_name(&resolved_toml, tag)
			.context("Failed to inject role name into agent manifest")?;
		let merged = merge_agent_toml(config, &tagged_toml)
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
		Ok((merged, role))
	} else {
		// Plain role name — use config as-is
		Ok((config.clone(), tag.to_string()))
	}
}

/// Initialize MCP servers for the given role.
/// Shows a spinner in interactive mode, silent otherwise.
pub async fn init_mcp(role: &str, config: &Config, is_interactive: bool) -> Result<()> {
	if is_interactive {
		use octomind::mcp::McpInitProgress;
		use std::sync::{Arc, Mutex};

		let spinner = ProgressBar::new_spinner();
		spinner.set_style(
			ProgressStyle::default_spinner()
				.template(" {spinner:.cyan} {msg:.cyan}")
				.unwrap()
				.tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧"),
		);
		spinner.enable_steady_tick(Duration::from_millis(80));

		// Track pending servers — remove each one as it completes
		let pending: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
		let total = Arc::new(Mutex::new(0usize));

		let cb = |progress: McpInitProgress| match &progress {
			McpInitProgress::Starting { servers } => {
				*total.lock().unwrap() = servers.len();
				if servers.is_empty() {
					spinner.set_message("No MCP servers to initialize".to_string());
				} else {
					*pending.lock().unwrap() = servers.clone();
					spinner.set_message(format!(
						"Starting MCP: {} [0/{}]",
						servers.join(", "),
						servers.len()
					));
				}
			}
			McpInitProgress::Completed { server, .. } => {
				let mut pending_guard = pending.lock().unwrap();
				pending_guard.retain(|s| s != server);
				let done = *total.lock().unwrap() - pending_guard.len();
				let total_count = *total.lock().unwrap();
				if pending_guard.is_empty() {
					spinner.set_message(format!("Starting MCP: done [{}/{}]", done, total_count));
				} else {
					spinner.set_message(format!(
						"Starting MCP: {} [{}/{}]",
						pending_guard.join(", "),
						done,
						total_count
					));
				}
			}
		};

		let result =
			octomind::mcp::initialize_mcp_for_role_with_callback(role, config, Some(&cb)).await;
		spinner.finish_and_clear();
		print!("\x1B[2K\r");
		std::io::Write::flush(&mut std::io::stdout()).ok();
		result
	} else {
		octomind::mcp::initialize_mcp_for_role(role, config).await
	}
}

/// Inject the tag (e.g. `"octomind:tap"`) as the `name` field of the first
/// `[[roles]]` entry in the manifest TOML.  This means manifests never need
/// to declare their own name — the tag IS the identity.
fn inject_role_name(toml_str: &str, tag: &str) -> Result<String> {
	// The role name is the part after the colon: "domain:spec" → "spec"
	let role_name = tag.split(':').nth(1).unwrap_or(tag);

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
