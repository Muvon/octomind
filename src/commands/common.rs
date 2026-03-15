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
use octomind::agent::{deps, inputs, registry};
use octomind::config::{loading::merge_agent_toml, Config};
use std::time::Duration;

/// Resolve config and role from a TAG argument.
///
/// - `None` / `"developer"` → plain role, config unchanged
/// - `"some_role"` (no `:`) → plain role name, config unchanged
/// - `"domain:spec"` (contains `:`) → fetch registry manifest, merge into config
///
/// `status_cb` is called with human-readable status strings during resolution
/// (tap fetch, dep checks) so callers can update a spinner.
///
/// Returns `(resolved_config, role_name)`.
pub async fn resolve_config_and_role(
	tag: Option<&str>,
	config: &Config,
	status_cb: Option<&dyn Fn(&str)>,
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
		// INPUT first (persistent credential store), then ENV (environment / .env fallback)
		let resolved_toml = inputs::resolve_inputs(&raw_toml).await?;
		let resolved_toml = inputs::resolve_env_vars(&resolved_toml).await?;
		// Run dep scripts before MCP init — idempotent, exit 0 if already installed
		deps::resolve_deps(&resolved_toml, &tap_root, status_cb).await?;
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

/// Run the full startup sequence (tap/dep resolution + MCP init) under a single spinner.
///
/// Interactive mode: shows an animated spinner with live status messages.
/// Non-interactive mode: silent (errors still propagate).
pub async fn startup(
	tag: Option<&str>,
	config: &Config,
	is_interactive: bool,
) -> Result<(Config, String)> {
	if is_interactive {
		let spinner = make_spinner();

		// Phase 1: resolve config + deps (spinner shows tap/dep status)
		let spinner_ref = &spinner;
		let status_cb = |msg: &str| spinner_ref.set_message(msg.to_string());
		let resolve_result = resolve_config_and_role(tag, config, Some(&status_cb)).await;
		let (run_config, role) = match resolve_result {
			Ok(v) => v,
			Err(e) => {
				spinner.finish_and_clear();
				print!("\x1B[2K\r");
				std::io::Write::flush(&mut std::io::stdout()).ok();
				return Err(e);
			}
		};

		// Phase 2: MCP init under the same spinner
		if let Err(e) = mcp_init_with_spinner(&role, &run_config, &spinner).await {
			spinner.finish_and_clear();
			print!("\x1B[2K\r");
			std::io::Write::flush(&mut std::io::stdout()).ok();
			return Err(e);
		}
		spinner.finish_and_clear();
		print!("\x1B[2K\r");
		std::io::Write::flush(&mut std::io::stdout()).ok();
		Ok((run_config, role))
	} else {
		// Non-interactive: silent
		let (run_config, role) = resolve_config_and_role(tag, config, None).await?;
		octomind::mcp::initialize_mcp_for_role(&role, &run_config).await?;
		Ok((run_config, role))
	}
}

/// Initialize MCP servers only (no tap/dep resolution). Used by the server command,
/// which sets up tracing between config resolution and MCP init.
/// Shows a spinner in interactive mode, silent otherwise.
pub async fn startup_mcp_only(role: &str, config: &Config, is_interactive: bool) -> Result<()> {
	if is_interactive {
		let spinner = make_spinner();
		let result = mcp_init_with_spinner(role, config, &spinner).await;
		spinner.finish_and_clear();
		print!("\x1B[2K\r");
		std::io::Write::flush(&mut std::io::stdout()).ok();
		result
	} else {
		octomind::mcp::initialize_mcp_for_role(role, config).await
	}
}

fn make_spinner() -> ProgressBar {
	let spinner = ProgressBar::new_spinner();
	spinner.set_style(
		ProgressStyle::default_spinner()
			.template(" {spinner:.cyan} {msg:.cyan}")
			.unwrap()
			.tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧"),
	);
	spinner.enable_steady_tick(Duration::from_millis(80));
	spinner
}

async fn mcp_init_with_spinner(role: &str, config: &Config, spinner: &ProgressBar) -> Result<()> {
	use octomind::mcp::McpInitProgress;
	use std::sync::{Arc, Mutex};

	let pending: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
	let total = Arc::new(Mutex::new(0usize));

	let cb = |progress: McpInitProgress| match &progress {
		McpInitProgress::Starting { servers } => {
			*total.lock().unwrap() = servers.len();
			if servers.is_empty() {
				spinner.set_message("Starting MCP...".to_string());
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

	octomind::mcp::initialize_mcp_for_role_with_callback(role, config, Some(&cb)).await
}

/// Inject the tag (e.g. `"octomind:tap"`) as the `name` field of the first
/// `[[roles]]` entry in the manifest TOML.  This means manifests never need
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
