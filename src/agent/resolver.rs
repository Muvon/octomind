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
//! fetch, INPUT/ENV/dep resolution, and config merge. A bare domain
//! (e.g. `developer`) is first expanded to its orchestrator's tag
//! (`developer:chief`); a plain tag that is not a domain is returned unchanged
//! against the given config. A role defined in the local config wins over a
//! domain of the same name.

use crate::agent::taps::ORCHESTRATOR_SPEC;
use crate::agent::{deps, inputs, registry};
use crate::config::{loading::merge_agent_toml, Config};
use anyhow::{Context, Result};

/// Placeholder in a tap manifest's `system` prompt, replaced by the other agents of the same domain.
const SPECIALISTS_PLACEHOLDER: &str = "{{SPECIALISTS}}";

/// Placeholder in a tap manifest's `system` prompt, replaced by the orchestrator (`chief`) of every other domain.
const DOMAINS_PLACEHOLDER: &str = "{{DOMAINS}}";

/// Placeholder in a tap manifest's `system` prompt, replaced by the agents and domains its `# Uses:` header declares.
const USES_PLACEHOLDER: &str = "{{USES}}";

/// Roster text when a placeholder has nothing to list.
const EMPTY_ROSTER: &str = "(none)";

/// Expand a bare domain (no `:`) to the tag of its orchestrator, `<domain>:chief`.
///
/// A role defined in `config.roles` wins, so existing plain-role usage never
/// breaks. A tag with `:` and a name that is not a domain come back unchanged.
pub fn expand_domain_role(role: &str, config: &Config) -> Result<String> {
	if role.contains(':') || config.roles.iter().any(|r| r.name == role) {
		return Ok(role.to_string());
	}
	Ok(crate::agent::taps::resolve_domain_chief(role)?.unwrap_or_else(|| role.to_string()))
}

/// Resolve config and role from a TAG argument.
///
/// - `None` / default role → plain role, config unchanged
/// - plain string (no `:`) that is a role in `config.roles` → plain role, config unchanged
/// - plain string (no `:`) that is a domain → treated as its orchestrator's tag (`<domain>:chief`)
/// - plain string (no `:`) that is neither → plain role name, config unchanged
/// - `"domain:spec"` (contains `:`) → fetch tap manifest, merge into config
///
/// `[taps]` model overrides apply per tag, falling back to the bare domain.
///
/// `status_cb` receives human-readable status strings (tap fetch, dep checks)
/// so callers can drive a spinner.
///
/// Returns `(resolved_config, role_name)`; the role name is the full tag.
pub async fn resolve_config_and_role(
	tag: Option<&str>,
	config: &Config,
	status_cb: Option<&(dyn Fn(&str) + Send + Sync)>,
) -> Result<(Config, String)> {
	let requested = tag.unwrap_or(config.default.as_str());
	// A bare domain can be the first thing a new user runs, so the built-in tap
	// has to exist before the lookup reads it. A local config role never needs
	// the tap, which keeps local roles usable offline.
	if !requested.contains(':') && !config.roles.iter().any(|r| r.name == requested) {
		crate::agent::taps::ensure_default_tap()?;
	}
	let tag = expand_domain_role(requested, config)?;
	let tag = tag.as_str();

	if tag.contains(':') {
		// Registry agent: fetch manifest, resolve inputs, merge config
		if let Some(cb) = status_cb {
			cb(&format!("Fetching agent: {tag}"));
		}
		let (raw_toml, tap_root) = registry::fetch_manifest(tag, &config.registry)
			.await
			.context(format!("Failed to fetch agent manifest for '{tag}'"))?;
		// Resolve capabilities before input/env/dep resolution
		let (resolved_toml, dep_entries) = registry::resolve_capabilities(&raw_toml, &tap_root, &config.capabilities)
			.context("Failed to resolve agent capabilities")?;
		// INPUT first (persistent credential store), then ENV (environment / .env
		// fallback), under one escape scope so `{{{{ENV:KEY}}}}` stays literal.
		let resolved_toml = inputs::resolve_manifest_placeholders(&resolved_toml).await?;
		// Run dep scripts before MCP init — idempotent, exit 0 if already installed.
		// Grouped by owning tap: a capability reached through the `octomind/`
		// prefix keeps its dep scripts in the baseline tap, not the agent's.
		// Groups keep declaration order (agent's own deps first) so installs
		// run deterministically.
		let mut deps_by_root: Vec<(std::path::PathBuf, Vec<String>)> = Vec::new();
		for (entry, root) in dep_entries {
			match deps_by_root.iter_mut().find(|(r, _)| *r == root) {
				Some((_, entries)) => entries.push(entry),
				None => deps_by_root.push((root, vec![entry])),
			}
		}
		for (root, entries) in deps_by_root {
			deps::run_dep_entries(&entries, &root, status_cb).await?;
		}
		// Always inject the tag as the role name — manifests never need to declare it.
		let tagged_toml = prepare_role_manifest(&resolved_toml, tag).context("Failed to prepare agent manifest")?;
		let mut merged =
			merge_agent_toml(config, &tagged_toml).context("Failed to merge agent manifest into config")?;

		// First role in merged config that isn't in the base config
		let base_names: std::collections::HashSet<&str> = config.roles.iter().map(|r| r.name.as_str()).collect();
		let role = merged
			.roles
			.iter()
			.find(|r| !base_names.contains(r.name.as_str()))
			.map(|r| r.name.clone())
			.context(format!(
				"Agent manifest for '{tag}' must define at least one new [[roles]] entry"
			))?;

		// Apply tap model override if configured: exact tag first, then the bare domain
		let domain = tag.split_once(':').map_or(tag, |(domain, _)| domain);
		if let Some(tap_model) = config.taps.get(tag).or_else(|| config.taps.get(domain)) {
			merged.model_profile.model = tap_model.clone();
			crate::log_debug!("Applied tap model override: {} -> {}", tag, tap_model);
		}

		Ok((merged, role))
	} else {
		// Plain role name — use config as-is
		Ok((config.clone(), tag.to_string()))
	}
}

/// Prepare the role entry of a tap manifest: inject the tag (e.g. `"octomind:tap"`)
/// as the `name` field of the first `[[roles]]` entry, and expand the roster
/// placeholders in its `system` prompt. Manifests never need to declare their
/// own name — the tag IS the identity.
///
/// Works on the parsed value so descriptions with quotes or backslashes
/// cannot break the TOML.
fn prepare_role_manifest(toml_str: &str, tag: &str) -> Result<String> {
	// The full tag IS the role identity — "doctor:blood" becomes the role name.
	// This matches what resolve_config_and_role returns as the role string.
	let role_name = tag;

	let mut value: toml::Value = toml::from_str(toml_str).context("Failed to parse agent manifest TOML")?;

	if let Some(toml::Value::Array(roles)) = value.get_mut("roles") {
		if let Some(toml::Value::Table(table)) = roles.first_mut() {
			table.insert("name".to_string(), toml::Value::String(role_name.to_string()));
			if let Some(toml::Value::String(system)) = table.get_mut("system") {
				expand_roster_placeholders(system, tag)?;
			}
		}
	}

	toml::to_string(&value).context("Failed to re-serialize agent manifest TOML")
}

/// Replace `{{SPECIALISTS}}`, `{{DOMAINS}}` and `{{USES}}` in a system prompt.
/// Skips the tap scan when none is present. Output depends only on the taps'
/// content, never on run order, so the prompt stays cacheable.
fn expand_roster_placeholders(system: &mut String, tag: &str) -> Result<()> {
	// An escaped `{{{{USES}}}}` contains `{{USES}}`; hide it from the replacements.
	*system = inputs::protect_escaped_braces(system);
	let wants_specialists = system.contains(SPECIALISTS_PLACEHOLDER);
	let wants_domains = system.contains(DOMAINS_PLACEHOLDER);
	let wants_uses = system.contains(USES_PLACEHOLDER);
	if !wants_specialists && !wants_domains && !wants_uses {
		*system = inputs::restore_escaped_braces(system);
		return Ok(());
	}

	let agents = crate::agent::taps::list_agent_headers().context("Failed to list tap agents for roster")?;
	let own_domain = tag.split_once(':').map_or(tag, |(domain, _)| domain);
	let describe = |name: &str, description: Option<&str>| match description {
		Some(description) => format!("- {name} — {description}"),
		None => format!("- {name}"),
	};
	let join_lines = |lines: Vec<String>| {
		if lines.is_empty() {
			EMPTY_ROSTER.to_string()
		} else {
			lines.join("\n")
		}
	};

	if wants_specialists {
		let lines = agents
			.iter()
			.filter(|a| a.category == own_domain && a.tag != tag)
			.map(|a| describe(&a.tag, a.header.description.as_deref()))
			.collect();
		*system = system.replace(SPECIALISTS_PLACEHOLDER, &join_lines(lines));
	}
	if wants_domains {
		// `agents` is sorted by tag, where `x-y:chief` precedes `x:chief`; sort by domain instead.
		let mut chiefs: Vec<_> = agents
			.iter()
			.filter(|a| a.category != own_domain && a.tag == format!("{}:{ORCHESTRATOR_SPEC}", a.category))
			.collect();
		chiefs.sort_by(|a, b| a.category.cmp(&b.category));
		let lines = chiefs
			.into_iter()
			.map(|a| describe(&a.category, a.header.description.as_deref()))
			.collect();
		*system = system.replace(DOMAINS_PLACEHOLDER, &join_lines(lines));
	}
	if wants_uses {
		let mut entries = agents
			.iter()
			.find(|a| a.tag == tag)
			.map_or_else(Vec::new, |a| a.header.uses.clone());
		if entries.iter().any(String::is_empty) {
			anyhow::bail!(
				"`{tag}` declares `# Uses: {}`, which has an empty entry",
				entries.join(", ")
			);
		}
		entries.sort();
		let lines = entries
			.iter()
			.map(|entry| {
				let declared = format!("`{tag}` declares `# Uses: {entry}`");
				let callee_tag = if entry.contains(':') {
					entry.clone()
				} else if entry == own_domain {
					anyhow::bail!(
						"{declared}, but `{entry}` is its own domain: use `{SPECIALISTS_PLACEHOLDER}` for your own domain"
					);
				} else {
					format!("{entry}:{ORCHESTRATOR_SPEC}")
				};
				let callee = agents.iter().find(|a| a.tag == callee_tag).with_context(|| {
					if entry.contains(':') {
						format!("{declared}, but no such agent exists")
					} else {
						format!("{declared}, but domain `{entry}` has no `{ORCHESTRATOR_SPEC}`")
					}
				})?;
				Ok(describe(entry, callee.header.description.as_deref()))
			})
			.collect::<Result<Vec<_>>>()?;
		*system = system.replace(USES_PLACEHOLDER, &join_lines(lines));
	}
	*system = inputs::restore_escaped_braces(system);
	Ok(())
}

#[cfg(test)]
#[path = "resolver_tests.rs"]
mod tests;
