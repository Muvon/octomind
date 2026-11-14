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

/// `# Uses:` entry standing for every other domain that has a `chief`.
const ALL_DOMAINS_ENTRY: &str = "*";

/// Expand a bare domain (no `:`) to the tag of its orchestrator, `<domain>:chief`.
///
/// A role defined in `config.roles` wins, so existing plain-role usage never
/// breaks. An alias tag (`developer:cto`, a symlink to `chief.toml` in its tap) becomes the tag it
/// points to; every other tag with `:` and a name that is not a domain come back unchanged.
pub fn expand_domain_role(role: &str, config: &Config) -> Result<String> {
	if role.contains(':') {
		return crate::agent::taps::resolve_alias_tag(role);
	}
	if config.roles.iter().any(|r| r.name == role) {
		return Ok(role.to_string());
	}
	// A bare domain can be the first thing a new user runs, so the built-in tap
	// has to exist before the lookup reads it. A local config role never needs
	// the tap, which keeps local roles usable offline. Idempotent: only the
	// first clone blocks, later calls just refresh in the background.
	crate::agent::taps::ensure_default_tap()?;
	let chief = match crate::agent::taps::resolve_domain_chief(role) {
		Ok(Some(chief)) => Some(chief),
		// The clone can predate the domain or its `chief` (first run after an upgrade):
		// look once more when the refresh this process started has ended.
		_ if crate::agent::taps::wait_for_started_refreshes() => crate::agent::taps::resolve_domain_chief(role)?,
		missing => missing?,
	};
	Ok(chief.unwrap_or_else(|| role.to_string()))
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
		// A bad `# Uses:` header fails the load, not the agent's first delegation.
		let agents = crate::agent::taps::list_agent_headers().context("Failed to list tap agents")?;
		reachable_specialists(tag, &agents)?;
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

		// Apply tap model override if configured: exact tag first, then an alias of it (the name
		// this agent had before a rename: the config upgrade rewrites `config.toml` only, so a key
		// in another config file still carries the old name), then the bare domain
		let domain = tag.split_once(':').map_or(tag, |(domain, _)| domain);
		let aliases = crate::agent::taps::alias_tags_of(tag)?;
		if let Some(tap_model) = config
			.taps
			.get(tag)
			.or_else(|| aliases.iter().find_map(|alias| config.taps.get(alias)))
			.or_else(|| config.taps.get(domain))
		{
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
/// as the `name` field of the first `[[roles]]` entry. Manifests never need to
/// declare their own name — the tag IS the identity.
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
		}
	}

	toml::to_string(&value).context("Failed to re-serialize agent manifest TOML")
}

/// The agents `tag` may hand a task to, sorted by tag, each once.
///
/// A tap agent reaches the other agents of its own domain plus what its `# Uses:` header
/// declares: a bare domain stands for that domain's `chief`, a full tag for that agent, `*`
/// for the `chief` of every other domain. A caller that is not a tap agent (a role from the
/// local config) has no header and reaches every agent.
pub fn reachable_specialists<'a>(
	tag: &str,
	agents: &'a [crate::agent::taps::TapAgentHeader],
) -> Result<Vec<&'a crate::agent::taps::TapAgentHeader>> {
	let Some(caller) = agents.iter().find(|a| a.tag == tag) else {
		return Ok(agents.iter().collect());
	};
	let own_domain = caller.category.as_str();
	let mut reachable: Vec<&crate::agent::taps::TapAgentHeader> = agents
		.iter()
		.filter(|a| a.category == own_domain && a.tag != tag)
		.collect();
	if caller.header.uses.iter().any(String::is_empty) {
		anyhow::bail!(
			"`{tag}` declares `# Uses: {}`, which has an empty entry",
			caller.header.uses.join(", ")
		);
	}
	for entry in &caller.header.uses {
		let declared = format!("`{tag}` declares `# Uses: {entry}`");
		if entry == ALL_DOMAINS_ENTRY {
			reachable.extend(
				agents
					.iter()
					.filter(|a| a.category != own_domain && a.tag == format!("{}:{ORCHESTRATOR_SPEC}", a.category)),
			);
			continue;
		}
		let callee_tag = if entry.contains(':') {
			entry.clone()
		} else if entry == own_domain {
			anyhow::bail!("{declared}, but `{entry}` is its own domain: its agents are reachable without it");
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
		reachable.push(callee);
	}
	// A domain named both explicitly and through `*`, or a tag of the caller's own domain,
	// would otherwise appear twice.
	reachable.sort_by(|a, b| a.tag.cmp(&b.tag));
	reachable.dedup_by(|a, b| a.tag == b.tag);
	Ok(reachable)
}

#[cfg(test)]
#[path = "resolver_tests.rs"]
mod tests;
