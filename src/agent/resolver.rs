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
//! command. A tag is `<ns>` (the base agent of a namespace) or `<ns>:<variant>`;
//! `<ns>:default` is `<ns>`. A role defined in the local config wins and the
//! config stays unchanged; otherwise a tap that has the agent triggers tap manifest
//! fetch, INPUT/ENV/dep resolution, and config merge; a name without `:` that is
//! neither is returned unchanged, and a tag with `:` that no tap has is an error.

use crate::agent::registry::{canonical_tag, namespace_of};
use crate::agent::{deps, inputs, registry};
use crate::config::{loading::merge_agent_toml, Config};
use anyhow::{Context, Result};

/// `# Uses:` entry standing for every other agent.
const ALL_AGENTS_ENTRY: &str = "*";

/// Whether a tap has the agent `name`. A local config role never needs the tap, so callers
/// ask only after the config had no such role, which keeps local roles usable offline.
fn tap_has_agent(name: &str) -> Result<bool> {
	// The built-in tap has to exist before the lookup reads it, as a new user's first run
	// may name one of its agents. Idempotent: only the first clone blocks, later calls just
	// refresh in the background.
	crate::agent::taps::ensure_default_tap()?;
	if crate::agent::taps::has_agent(name)? {
		return Ok(true);
	}
	// The clone can predate the agent (first run after an upgrade): look once more when the
	// refresh this process started has ended.
	if crate::agent::taps::wait_for_started_refreshes() && crate::agent::taps::has_agent(name)? {
		return Ok(true);
	}
	crate::agent::taps::explain_missing_agent(name)?;
	Ok(false)
}

/// Resolve config and role from a TAG argument.
///
/// - `None` / default role → the config's `default`
/// - a role in `config.roles` → plain role, config unchanged
/// - an agent of a tap (`<ns>`, `<ns>:<variant>`, `<ns>:default` as `<ns>`) → fetch tap
///   manifest, merge into config
/// - a name with `:` that no tap has → error naming the namespace's agents, if it has any
/// - anything else → plain role name, config unchanged
///
/// `[taps]` model overrides apply per tag, falling back to the namespace.
///
/// `status_cb` receives human-readable status strings (tap fetch, dep checks)
/// so callers can drive a spinner.
///
/// Returns `(resolved_config, role_name)`; the role name is the tag.
pub async fn resolve_config_and_role(
	tag: Option<&str>,
	config: &Config,
	status_cb: Option<&(dyn Fn(&str) + Send + Sync)>,
) -> Result<(Config, String)> {
	let tag = tag.unwrap_or(config.default.as_str());

	if config.roles.iter().any(|r| r.name == tag) {
		return Ok((config.clone(), tag.to_string()));
	}
	// The tap lookup is by canonical tag; a `@version` suffix belongs to the fetch below.
	let name = canonical_tag(tag.split_once('@').map_or(tag, |(name, _)| name));
	if !tap_has_agent(name)? {
		if name.contains(':') {
			anyhow::bail!("Agent '{name}' is not in any installed tap");
		}
		return Ok((config.clone(), tag.to_string()));
	}

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
	reachable_agents(name, true, &agents)?;
	// Always inject the agent name as the role name — manifests never need to declare it. A
	// `@version` pinned the manifest fetched above; it is not part of the name that skills,
	// capabilities and `# Uses:` reach bind to.
	let tagged_toml = prepare_role_manifest(&resolved_toml, name).context("Failed to prepare agent manifest")?;
	let mut merged = merge_agent_toml(config, &tagged_toml).context("Failed to merge agent manifest into config")?;

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

	// Apply tap model override if configured: the exact tag first, then its namespace
	if let Some(tap_model) = config.taps.get(name).or_else(|| config.taps.get(namespace_of(name))) {
		merged.model_profile.model = tap_model.clone();
		crate::log_debug!("Applied tap model override: {} -> {}", tag, tap_model);
	}

	tap_agent_roles()
		.write()
		.expect("tap agent roles lock poisoned")
		.insert(role.clone());
	Ok((merged, role))
}

fn tap_agent_roles() -> &'static std::sync::RwLock<std::collections::HashSet<String>> {
	static ROLES: std::sync::OnceLock<std::sync::RwLock<std::collections::HashSet<String>>> =
		std::sync::OnceLock::new();
	ROLES.get_or_init(Default::default)
}

/// Whether this process resolved `role` from a tap agent. A role defined in the local config
/// wins over a tap agent of the same name and is never recorded, so a `true` answer means the
/// running role is the tap agent, not a local role that happens to share its name.
pub fn resolved_from_tap(role: &str) -> bool {
	tap_agent_roles()
		.read()
		.expect("tap agent roles lock poisoned")
		.contains(role)
}

/// Prepare the role entry of a tap manifest: inject the tag (e.g. `"tap"`)
/// as the `name` field of the first `[[roles]]` entry. Manifests never need to
/// declare their own name — the tag IS the identity.
///
/// Works on the parsed value so descriptions with quotes or backslashes
/// cannot break the TOML.
fn prepare_role_manifest(toml_str: &str, tag: &str) -> Result<String> {
	// The tag IS the role identity — "doctor" becomes the role name.
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
/// A base agent (`<ns>`, `caller_is_tap_agent`) reaches the variants of its own namespace, as
/// if its `# Uses:` header listed them, plus what the header declares: `<ns>`, `<ns>:<variant>`,
/// or `*` for every other agent, variants included. A variant (`<ns>:<variant>`) reaches
/// nobody: its `# Uses:` header is not read. A caller that is not a tap agent (a role from the
/// local config, even one named like a tap agent) has no header and reaches every other agent.
pub fn reachable_agents<'a>(
	tag: &str,
	caller_is_tap_agent: bool,
	agents: &'a [crate::agent::taps::TapAgentHeader],
) -> Result<Vec<&'a crate::agent::taps::TapAgentHeader>> {
	if !caller_is_tap_agent {
		return Ok(agents.iter().filter(|a| a.tag != tag).collect());
	}
	let Some(caller) = agents.iter().find(|a| a.tag == tag) else {
		anyhow::bail!("`{tag}` is not an agent of an installed tap");
	};
	if tag.contains(':') {
		return Ok(Vec::new());
	}
	if caller.header.uses.iter().any(String::is_empty) {
		anyhow::bail!(
			"`{tag}` declares `# Uses: {}`, which has an empty entry",
			caller.header.uses.join(", ")
		);
	}
	let mut reachable: Vec<&crate::agent::taps::TapAgentHeader> = agents
		.iter()
		.filter(|a| namespace_of(&a.tag) == tag && a.tag != tag)
		.collect();
	for entry in &caller.header.uses {
		let declared = format!("`{tag}` declares `# Uses: {entry}`");
		if entry == ALL_AGENTS_ENTRY {
			reachable.extend(agents.iter().filter(|a| a.tag != tag));
			continue;
		}
		let entry = canonical_tag(entry);
		if entry == tag {
			anyhow::bail!("{declared}, which is its own name");
		}
		let callee = agents
			.iter()
			.find(|a| a.tag == entry)
			.with_context(|| format!("{declared}, but no such agent exists"))?;
		reachable.push(callee);
	}
	// An agent named explicitly and again through `*` or as a variant would otherwise appear twice.
	reachable.sort_by(|a, b| a.tag.cmp(&b.tag));
	reachable.dedup_by(|a, b| a.tag == b.tag);
	Ok(reachable)
}

#[cfg(test)]
#[path = "resolver_tests.rs"]
mod tests;
