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

//! Registry client: fetch and cache agent manifests.
//!
//! Tag format: `category:variant` or `category:variant@version`
//! Example:    `developer:general`, `developer:general@1.2`
//!
//! Manifests are cached at `~/.local/share/octomind/agents/<category>/<variant>.toml`.
//! If the cached file is older than `cache_ttl_hours`, it is refreshed in the background
//! while the cached copy is returned immediately.
//!
//! Sources are resolved from user taps (see `agent::taps`) — user taps first, built-in last.

use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::config::registry::RegistryConfig;

/// Parse a tag string into `(category, variant, version)`.
///
/// - `developer:general`      → `("developer", "rust", None)`
/// - `developer:general@1.2`  → `("developer", "rust", Some("1.2"))`
pub fn parse_tag(tag: &str) -> Result<(String, String, Option<String>)> {
	let (name_part, version) = if let Some((n, v)) = tag.split_once('@') {
		(n, Some(v.to_string()))
	} else {
		(tag, None)
	};

	let (category, variant) = name_part.split_once(':').context(format!(
		"Invalid agent tag '{tag}': expected 'category:variant'"
	))?;

	if category.is_empty() || variant.is_empty() {
		anyhow::bail!("Invalid agent tag '{tag}': category and variant must be non-empty");
	}

	Ok((category.to_string(), variant.to_string(), version))
}

/// Local cache path for a manifest.
fn cache_path(category: &str, variant: &str) -> Result<PathBuf> {
	let dir = crate::directories::get_octomind_data_dir()?
		.join("agents")
		.join(category);
	fs::create_dir_all(&dir).context(format!(
		"Failed to create agent cache dir: {}",
		dir.display()
	))?;
	Ok(dir.join(format!("{variant}.toml")))
}

/// Check whether a cached file is stale (older than ttl).
fn is_stale(path: &PathBuf, ttl_hours: u64) -> bool {
	let Ok(meta) = fs::metadata(path) else {
		return true;
	};
	let Ok(modified) = meta.modified() else {
		return true;
	};
	let age = SystemTime::now()
		.duration_since(modified)
		.unwrap_or(Duration::MAX);
	age > Duration::from_secs(ttl_hours * 3600)
}

/// Fetch raw TOML from a tap for the given category/variant.
///
/// For GitHub taps: reads from cloned repo at `~/.local/share/octomind/taps/user/octomind-repo/`
/// For local taps: reads from the specified local path
async fn fetch_from_tap(
	tap: &crate::agent::taps::Tap,
	category: &str,
	variant: &str,
) -> Result<String> {
	let agents_dir = tap.agents_dir()?;
	let manifest_path = agents_dir.join(category).join(format!("{variant}.toml"));
	fs::read_to_string(&manifest_path).context(format!(
		"Failed to read manifest from tap '{}': {}",
		tap.name,
		manifest_path.display()
	))
}

/// Fetch a manifest for `tag` from the registry, using cache when fresh.
///
/// Sources are loaded from user taps (user taps first, built-in default last).
/// If the same manifest is found in multiple taps, the first one wins and a
/// warning is printed.
///
/// Returns `(raw_toml, tap_root)` — the TOML string and the root directory of
/// the tap that provides it (used to locate dep scripts at `<tap_root>/deps/`).
pub async fn fetch_manifest(tag: &str, registry: &RegistryConfig) -> Result<(String, PathBuf)> {
	let (category, variant, _version) = parse_tag(tag)?;
	let cache = cache_path(&category, &variant)?;

	let taps = crate::agent::taps::load_taps().unwrap_or_else(|_| {
		vec![crate::agent::taps::Tap {
			name: crate::agent::taps::DEFAULT_TAP.to_string(),
			local_path: None,
		}]
	});

	// Find the first tap that provides this manifest — this is always the tap root
	// used for dep scripts, regardless of whether we serve TOML from cache.
	let providing_tap = taps
		.iter()
		.find(|tap| {
			tap.agents_dir()
				.map(|d| d.join(&category).join(format!("{variant}.toml")).exists())
				.unwrap_or(false)
		})
		.cloned();

	// Warn if multiple taps provide this manifest (first wins, like Homebrew)
	let providing_count = taps
		.iter()
		.filter(|tap| {
			tap.agents_dir()
				.map(|d| d.join(&category).join(format!("{variant}.toml")).exists())
				.unwrap_or(false)
		})
		.count();
	if providing_count > 1 {
		if let Some(ref tap) = providing_tap {
			crate::log_debug!(
				"'{}' found in multiple taps — using first match: {}",
				tag,
				tap.name
			);
		}
	}

	// Resolve tap root — fall back to default tap dir if no live tap found
	let tap_root = providing_tap
		.as_ref()
		.and_then(|t| t.local_dir().ok())
		.unwrap_or_else(|| {
			crate::agent::taps::Tap {
				name: crate::agent::taps::DEFAULT_TAP.to_string(),
				local_path: None,
			}
			.local_dir()
			.unwrap_or_default()
		});

	// Return cached copy if fresh
	if cache.exists() && !is_stale(&cache, registry.cache_ttl_hours) {
		let toml = fs::read_to_string(&cache).context(format!(
			"Failed to read cached manifest: {}",
			cache.display()
		))?;
		return Ok((toml, tap_root));
	}

	// If stale but exists, return cached and refresh in background
	if cache.exists() {
		let cached = fs::read_to_string(&cache).context(format!(
			"Failed to read cached manifest: {}",
			cache.display()
		))?;

		let cat = category.clone();
		let var = variant.clone();
		let cache_path_bg = cache.clone();
		let taps_bg = taps.clone();
		tokio::spawn(async move {
			for tap in &taps_bg {
				if let Ok(content) = fetch_from_tap(tap, &cat, &var).await {
					let _ = fs::write(&cache_path_bg, content);
					return;
				}
			}
		});

		return Ok((cached, tap_root));
	}

	// No cache — fetch synchronously from taps in order
	let mut tap_errors: Vec<String> = Vec::new();
	for tap in &taps {
		match fetch_from_tap(tap, &category, &variant).await {
			Ok(content) => {
				let _ = fs::write(&cache, &content);
				let root = tap.local_dir().unwrap_or_default();
				return Ok((content, root));
			}
			Err(e) => {
				tap_errors.push(format!("  - {}: {}", tap.name, e));
			}
		}
	}

	let detail = if tap_errors.is_empty() {
		"No taps configured".to_string()
	} else {
		tap_errors.join("\n")
	};
	anyhow::bail!(
		"Failed to fetch agent manifest for '{}' from all taps:\n{}",
		tag,
		detail
	)
}

/// Metadata header for a tap agent manifest.
///
/// Every `<tap>/agents/<category>/<variant>.toml` opens with required
/// `# Title:` and `# Description:` header comments. That convention IS the
/// schema — we don't carry parallel structured fields. `parse_agent_meta`
/// reads those header lines directly; missing values are a hard error.
#[derive(Debug, Clone)]
pub struct AgentMeta {
	pub title: String,
	pub description: String,
}

/// One role enumerated from a tap. Naming follows the agent-facing surface:
/// callers refer to these as "roles" (the unit they `run`); internally the
/// string is still a `category:variant` tag.
#[derive(Debug, Clone)]
pub struct TapAgent {
	/// Role identifier in `category:variant` form — e.g. `developer:general`.
	pub role: String,
	pub meta: AgentMeta,
	/// Tap that provides this manifest (first-wins precedence).
	pub source_tap: String,
}

/// Extract `Title` and `Description` from the manifest's header comment block.
///
/// Scans the leading comment block (everything before the first non-comment,
/// non-blank line) for lines of the form:
///
/// ```text
/// # Title: <title>
/// # Description: <description>
/// ```
///
/// Both are required. Returns an error if either is absent — we own the
/// authoring convention, so there's no fallback.
pub fn parse_agent_meta(raw_toml: &str, tag: &str) -> Result<AgentMeta> {
	let mut title: Option<String> = None;
	let mut description: Option<String> = None;
	for line in raw_toml.lines() {
		let trimmed = line.trim_start();
		// Stop at first non-comment, non-blank line — header block ends here.
		if !trimmed.is_empty() && !trimmed.starts_with('#') {
			break;
		}
		if let Some(rest) = trimmed.strip_prefix("# Title:") {
			let v = rest.trim();
			if !v.is_empty() {
				title = Some(v.to_string());
			}
		} else if let Some(rest) = trimmed.strip_prefix("# Description:") {
			let v = rest.trim();
			if !v.is_empty() {
				description = Some(v.to_string());
			}
		}
	}
	let title = title.ok_or_else(|| {
		anyhow::anyhow!("Agent '{tag}' is missing required `# Title:` header comment.")
	})?;
	let description = description.ok_or_else(|| {
		anyhow::anyhow!("Agent '{tag}' is missing required `# Description:` header comment.")
	})?;
	Ok(AgentMeta { title, description })
}

#[cfg(test)]
mod meta_tests {
	use super::*;

	#[test]
	fn parses_title_and_description_from_header_comments() {
		let raw = "# agents/developer/general.toml\n\
		           # Agent: developer:general\n\
		           # Title: General Developer\n\
		           # Description: Elite senior developer.\n\
		           \n\
		           [[roles]]\n\
		           temperature = 0.1\n";
		let m = parse_agent_meta(raw, "developer:general").unwrap();
		assert_eq!(m.title, "General Developer");
		assert_eq!(m.description, "Elite senior developer.");
	}

	#[test]
	fn errors_when_title_missing() {
		let raw = "# Description: Only description\n[[roles]]\n";
		let err = parse_agent_meta(raw, "x:y").unwrap_err().to_string();
		assert!(err.contains("Title"));
	}

	#[test]
	fn errors_when_description_missing() {
		let raw = "# Title: Only title\n[[roles]]\n";
		let err = parse_agent_meta(raw, "x:y").unwrap_err().to_string();
		assert!(err.contains("Description"));
	}

	#[test]
	fn stops_at_first_non_comment_line() {
		// A `# Title:` after the header block should NOT be picked up.
		let raw = "# Title: Real\n\
		           # Description: Real desc\n\
		           [[roles]]\n\
		           system = \"# Title: not metadata\"\n";
		let m = parse_agent_meta(raw, "x:y").unwrap();
		assert_eq!(m.title, "Real");
		assert_eq!(m.description, "Real desc");
	}
}

/// Enumerate every agent installed across all configured taps.
///
/// Walks each tap's `agents/` directory, reads `<category>/<variant>.toml`,
/// extracts the `# Title:` / `# Description:` header comments (required by
/// tap-level linting), and returns `TapAgent` entries in first-tap-wins
/// order (later taps with same tag are skipped). Used by the `tap` core
/// tool to power `discover`.
pub fn list_all_tap_agents() -> Result<Vec<TapAgent>> {
	let taps =
		crate::agent::taps::get_taps().context("Failed to load taps for agent enumeration")?;

	let mut seen: HashSet<String> = HashSet::new();
	let mut out: Vec<TapAgent> = Vec::new();

	for tap in &taps {
		let agents_dir = match tap.agents_dir() {
			Ok(d) => d,
			Err(_) => continue,
		};
		if !agents_dir.is_dir() {
			continue;
		}
		let cat_entries = match fs::read_dir(&agents_dir) {
			Ok(e) => e,
			Err(_) => continue,
		};
		for cat_entry in cat_entries.flatten() {
			if !cat_entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
				continue;
			}
			let category = cat_entry.file_name().to_string_lossy().to_string();
			let var_entries = match fs::read_dir(cat_entry.path()) {
				Ok(e) => e,
				Err(_) => continue,
			};
			for var_entry in var_entries.flatten() {
				let path = var_entry.path();
				if path.extension().and_then(|s| s.to_str()) != Some("toml") {
					continue;
				}
				let variant = match path.file_stem().and_then(|s| s.to_str()) {
					Some(s) => s.to_string(),
					None => continue,
				};
				let role = format!("{category}:{variant}");
				if seen.contains(&role) {
					continue;
				}
				let raw = match fs::read_to_string(&path) {
					Ok(s) => s,
					Err(_) => continue,
				};
				let meta = parse_agent_meta(&raw, &role)?;
				seen.insert(role.clone());
				out.push(TapAgent {
					role,
					meta,
					source_tap: tap.name.clone(),
				});
			}
		}
	}

	out.sort_by(|a, b| a.role.cmp(&b.role));
	Ok(out)
}

/// A resolved capability — split across two files in the tap:
///
/// - `<tap>/capabilities/<name>/config.toml` — capability-level metadata
///   (`triggers = [...]`). Required, shared across all providers.
/// - `<tap>/capabilities/<name>/<provider>.toml` — provider-specific MCP
///   wiring (deps, server_refs, allowed_tools, mcp.servers).
///
/// We don't carry a `description` field; the deterministic routing layer
/// uses triggers, and the authoring comments in each TOML cover human
/// reading. `capability list` shows name + first few triggers as preview.
#[derive(Debug, Clone)]
pub struct ResolvedCapability {
	pub name: String,
	/// Required. Phrases a user might write to trigger this capability —
	/// drive the deterministic auto-activation path (mean-of-top-K cosine
	/// + margin gate). Authored in `<tap>/capabilities/<name>/config.toml`.
	pub triggers: Vec<String>,
	/// Optional domain bindings from `<tap>/capabilities/<name>/config.toml`.
	/// Empty = capability is available in every role (filesystem-style universal
	/// utility). Non-empty = capability only loads when the active role's
	/// domain part (`developer:general` → `"developer"`) is in this list.
	/// Hard-bound: applies to auto-activation, `capability list`, `capability
	/// discover`, manual `capability enable`, and `OCTOMIND_CAPABILITIES`
	/// env loading. There is no bypass — out-of-domain access is rejected
	/// everywhere so a `medical:*` role never sees code-search and a
	/// `developer:*` role never sees medical-reference.
	pub domains: Vec<String>,
	pub deps: Vec<String>,
	pub server_refs: Vec<String>,
	pub allowed_tools: Vec<String>,
	pub mcp_servers: Vec<crate::config::McpServerConfig>,
	/// Root of the tap that provided this capability (first-wins across
	/// `get_taps()`). Used to resolve `<tap_root>/deps/<org>/<tool>.sh`
	/// paths when activation needs to run dep installers.
	pub tap_root: PathBuf,
}

/// True when the capability is available to the given runtime domain.
/// Empty `cap_domains` means universal (no restriction). Non-empty restricts
/// to exact-match domain strings.
///
/// The single shared filter — used at every consumption site so the rule is
/// consistent (auto-activation, list, discover, enable, env-load).
pub fn cap_available_in_domain(cap_domains: &[String], current: &str) -> bool {
	cap_domains.is_empty() || cap_domains.iter().any(|d| d == current)
}

/// Read `triggers` (required) and `domains` (optional) arrays from
/// `<cap_dir>/config.toml`. Returns `(triggers, domains)`.
///
/// `triggers` errors when absent or empty — we own the schema in the tap, so
/// triggers are required. `domains` defaults to empty (universal availability)
/// when absent. Both arrays trim whitespace and drop empty entries silently.
fn read_capability_config(cap_dir: &Path, cap_name: &str) -> Result<(Vec<String>, Vec<String>)> {
	let config_path = cap_dir.join("config.toml");
	if !config_path.exists() {
		anyhow::bail!(
			"Capability '{cap_name}' is missing `config.toml` (expected at {})",
			config_path.display()
		);
	}
	let raw = fs::read_to_string(&config_path)
		.with_context(|| format!("Failed to read {}", config_path.display()))?;
	let value: toml::Value = toml::from_str(&raw)
		.with_context(|| format!("Failed to parse {}", config_path.display()))?;

	let read_string_array = |field: &str| -> Vec<String> {
		value
			.get(field)
			.and_then(|u| u.as_array())
			.map(|arr| {
				arr.iter()
					.filter_map(|v| v.as_str())
					.map(str::trim)
					.filter(|s| !s.is_empty())
					.map(String::from)
					.collect()
			})
			.unwrap_or_default()
	};

	let triggers = read_string_array("triggers");
	if triggers.is_empty() {
		anyhow::bail!(
			"Capability '{cap_name}' has no `triggers = [...]` in {}. \
			 Author 5–15 short trigger phrases — they drive the deterministic \
			 routing layer that activates this capability when the user's \
			 message embeds close to one of them.",
			config_path.display()
		);
	}
	let domains = read_string_array("domains");
	Ok((triggers, domains))
}

/// Parse a capability and return its resolved components.
///
/// Reads two files from the first tap that has them:
/// - `<tap>/capabilities/<name>/config.toml` — `triggers` (required)
/// - `<tap>/capabilities/<name>/<provider>.toml` — provider wiring
///
/// `<provider>` is taken from `config.capabilities` overrides, defaulting
/// to `"default"`. Used at runtime when a skill declares `capabilities: [...]`
/// or when `auto_activate_capabilities` flips a capability on.
pub fn parse_capability_toml(
	cap_name: &str,
	overrides: &HashMap<String, String>,
) -> Result<ResolvedCapability> {
	let taps =
		crate::agent::taps::get_taps().context("Failed to load taps for capability resolution")?;

	let provider = overrides
		.get(cap_name)
		.map(|s| s.as_str())
		.unwrap_or("default");

	for tap in &taps {
		let tap_root = match tap.local_dir() {
			Ok(d) => d,
			Err(_) => continue,
		};
		let cap_dir = tap_root.join("capabilities").join(cap_name);
		let provider_path = cap_dir.join(format!("{provider}.toml"));

		// Both files must exist for this tap to provide the capability.
		if !cap_dir.is_dir() || !provider_path.exists() {
			continue;
		}

		let (triggers, domains) = read_capability_config(&cap_dir, cap_name)?;

		let cap_str = fs::read_to_string(&provider_path).with_context(|| {
			format!("Failed to read provider file: {}", provider_path.display())
		})?;
		let cap: toml::Value = toml::from_str(&cap_str).with_context(|| {
			format!("Failed to parse provider file: {}", provider_path.display())
		})?;

		let mut resolved = ResolvedCapability {
			name: cap_name.to_string(),
			triggers,
			domains,
			deps: Vec::new(),
			server_refs: Vec::new(),
			allowed_tools: Vec::new(),
			mcp_servers: Vec::new(),
			tap_root: tap_root.clone(),
		};

		// [deps] require
		if let Some(deps) = cap
			.get("deps")
			.and_then(|d| d.get("require"))
			.and_then(|r| r.as_array())
		{
			resolved.deps = deps
				.iter()
				.filter_map(|v| v.as_str().map(String::from))
				.collect();
		}

		// [roles.mcp] server_refs
		if let Some(refs) = cap
			.get("roles")
			.and_then(|r| r.get("mcp"))
			.and_then(|m| m.get("server_refs"))
			.and_then(|s| s.as_array())
		{
			resolved.server_refs = refs
				.iter()
				.filter_map(|v| v.as_str().map(String::from))
				.collect();
		}

		// [roles.mcp] allowed_tools
		if let Some(tools) = cap
			.get("roles")
			.and_then(|r| r.get("mcp"))
			.and_then(|m| m.get("allowed_tools"))
			.and_then(|a| a.as_array())
		{
			resolved.allowed_tools = tools
				.iter()
				.filter_map(|v| v.as_str().map(String::from))
				.collect();
		}

		// [[mcp.servers]] blocks — deserialize into McpServerConfig
		if let Some(servers) = cap
			.get("mcp")
			.and_then(|m| m.get("servers"))
			.and_then(|s| s.as_array())
		{
			for server_val in servers {
				let server_str = toml::to_string(server_val).unwrap_or_default();
				if let Ok(server_config) =
					toml::from_str::<crate::config::McpServerConfig>(&server_str)
				{
					resolved.mcp_servers.push(server_config);
				}
			}
		}

		return Ok(resolved);
	}

	anyhow::bail!(
		"Capability '{}' not found (provider: '{}') in any tap",
		cap_name,
		provider
	)
}

/// Enumerate every capability installed across all configured taps.
///
/// Walks each tap's `capabilities/` directory, collects unique capability names
/// (first-tap-wins precedence — same as `parse_capability_toml`), and returns
/// the resolved metadata for each. Used at runtime by the `capability` tool to
/// answer `list` and `discover` queries without the agent needing to know
/// which tap provides what.
pub fn list_all_capabilities(
	overrides: &HashMap<String, String>,
) -> Result<Vec<ResolvedCapability>> {
	let taps =
		crate::agent::taps::get_taps().context("Failed to load taps for capability enumeration")?;

	let mut seen_names: HashSet<String> = HashSet::new();
	let mut resolved: Vec<ResolvedCapability> = Vec::new();

	for tap in &taps {
		let tap_root = match tap.local_dir() {
			Ok(d) => d,
			Err(_) => continue,
		};
		let cap_dir = tap_root.join("capabilities");
		if !cap_dir.is_dir() {
			continue;
		}
		let entries = match fs::read_dir(&cap_dir) {
			Ok(e) => e,
			Err(_) => continue,
		};
		for entry in entries.flatten() {
			let is_dir = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
			if !is_dir {
				continue;
			}
			let cap_name = entry.file_name().to_string_lossy().to_string();
			if seen_names.contains(&cap_name) {
				continue;
			}
			// parse_capability_toml iterates taps internally and applies the same
			// first-wins precedence; we just collect unique names here.
			if let Ok(r) = parse_capability_toml(&cap_name, overrides) {
				seen_names.insert(cap_name);
				resolved.push(r);
			}
		}
	}

	resolved.sort_by(|a, b| a.name.cmp(&b.name));
	Ok(resolved)
}

/// Resolve `capabilities = [...]` declared in an agent manifest.
///
/// For each capability name, loads `<tap_root>/capabilities/<name>/<provider>.toml`
/// where `<provider>` comes from the user's `[capabilities]` config overrides,
/// falling back to `"default"` when not overridden.
///
/// Merges into the agent TOML:
/// - `[deps] require` → union
/// - `[roles.mcp] server_refs` → union
/// - `[roles.mcp] allowed_tools` → union
/// - `[[mcp.servers]]` blocks → append (deduplicated by name)
///
/// Strips the `capabilities = [...]` line from the output.
pub fn resolve_capabilities(
	raw_toml: &str,
	tap_root: &Path,
	overrides: &HashMap<String, String>,
) -> Result<String> {
	let mut value: toml::Value =
		toml::from_str(raw_toml).context("Failed to parse agent manifest TOML")?;

	// Extract and remove capabilities list
	let cap_names: Vec<String> = match value.get("capabilities") {
		Some(toml::Value::Array(arr)) => arr
			.iter()
			.filter_map(|v| v.as_str().map(String::from))
			.collect(),
		_ => return Ok(raw_toml.to_string()), // No capabilities declared — pass through
	};
	if let toml::Value::Table(t) = &mut value {
		t.remove("capabilities");
	}

	// Collect merged values from all capability files
	let mut all_deps: Vec<String> = Vec::new();
	let mut all_server_refs: Vec<String> = Vec::new();
	let mut all_allowed_tools: Vec<String> = Vec::new();
	let mut all_mcp_servers: Vec<toml::Value> = Vec::new();
	let mut seen_server_names: HashSet<String> = HashSet::new();

	// Track server names already in the agent manifest
	if let Some(servers) = value
		.get("mcp")
		.and_then(|m| m.get("servers"))
		.and_then(|s| s.as_array())
	{
		for s in servers {
			if let Some(name) = s.get("name").and_then(|n| n.as_str()) {
				seen_server_names.insert(name.to_string());
			}
		}
	}

	for cap_name in &cap_names {
		let provider = overrides
			.get(cap_name)
			.map(|s| s.as_str())
			.unwrap_or("default");
		let cap_path = tap_root
			.join("capabilities")
			.join(cap_name)
			.join(format!("{provider}.toml"));

		if !cap_path.exists() {
			anyhow::bail!(
				"Capability file not found: {} (looked in {})",
				cap_name,
				cap_path.display()
			);
		}

		let cap_str = fs::read_to_string(&cap_path)
			.with_context(|| format!("Failed to read capability file: {}", cap_path.display()))?;
		let cap: toml::Value = toml::from_str(&cap_str)
			.with_context(|| format!("Failed to parse capability file: {}", cap_path.display()))?;

		// [deps] require
		if let Some(deps) = cap
			.get("deps")
			.and_then(|d| d.get("require"))
			.and_then(|r| r.as_array())
		{
			for d in deps {
				if let Some(s) = d.as_str() {
					if !all_deps.contains(&s.to_string()) {
						all_deps.push(s.to_string());
					}
				}
			}
		}

		// [roles.mcp] server_refs
		if let Some(refs) = cap
			.get("roles")
			.and_then(|r| r.get("mcp"))
			.and_then(|m| m.get("server_refs"))
			.and_then(|s| s.as_array())
		{
			for r in refs {
				if let Some(s) = r.as_str() {
					if !all_server_refs.contains(&s.to_string()) {
						all_server_refs.push(s.to_string());
					}
				}
			}
		}

		// [roles.mcp] allowed_tools
		if let Some(tools) = cap
			.get("roles")
			.and_then(|r| r.get("mcp"))
			.and_then(|m| m.get("allowed_tools"))
			.and_then(|a| a.as_array())
		{
			for t in tools {
				if let Some(s) = t.as_str() {
					if !all_allowed_tools.contains(&s.to_string()) {
						all_allowed_tools.push(s.to_string());
					}
				}
			}
		}

		// [[mcp.servers]] blocks — deduplicate by name
		if let Some(servers) = cap
			.get("mcp")
			.and_then(|m| m.get("servers"))
			.and_then(|s| s.as_array())
		{
			for server in servers {
				let name = server.get("name").and_then(|n| n.as_str()).unwrap_or("");
				if !name.is_empty() && seen_server_names.insert(name.to_string()) {
					all_mcp_servers.push(server.clone());
				}
			}
		}
	}

	// Merge deps into agent value
	if !all_deps.is_empty() {
		let existing_deps: Vec<String> = value
			.get("deps")
			.and_then(|d| d.get("require"))
			.and_then(|r| r.as_array())
			.map(|arr| {
				arr.iter()
					.filter_map(|v| v.as_str().map(String::from))
					.collect()
			})
			.unwrap_or_default();

		let mut merged = existing_deps;
		for d in all_deps {
			if !merged.contains(&d) {
				merged.push(d);
			}
		}

		let deps_table = value
			.as_table_mut()
			.unwrap()
			.entry("deps")
			.or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
		if let toml::Value::Table(t) = deps_table {
			t.insert(
				"require".to_string(),
				toml::Value::Array(merged.into_iter().map(toml::Value::String).collect()),
			);
		}
	}

	// Merge server_refs and allowed_tools into [roles] entries
	// The agent manifest has [[roles]] as an array — merge into each entry's mcp section
	if let Some(toml::Value::Array(roles)) = value.get_mut("roles") {
		for role in roles.iter_mut() {
			if let toml::Value::Table(role_table) = role {
				let mcp = role_table
					.entry("mcp")
					.or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
				if let toml::Value::Table(mcp_table) = mcp {
					// server_refs
					merge_string_array(mcp_table, "server_refs", &all_server_refs);
					// allowed_tools
					merge_string_array(mcp_table, "allowed_tools", &all_allowed_tools);
				}
			}
		}
	}

	// Append [[mcp.servers]] blocks
	if !all_mcp_servers.is_empty() {
		let mcp = value
			.as_table_mut()
			.unwrap()
			.entry("mcp")
			.or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
		if let toml::Value::Table(mcp_table) = mcp {
			let servers = mcp_table
				.entry("servers")
				.or_insert_with(|| toml::Value::Array(Vec::new()));
			if let toml::Value::Array(arr) = servers {
				arr.extend(all_mcp_servers);
			}
		}
	}

	toml::to_string(&value)
		.context("Failed to re-serialize agent manifest after capability resolution")
}

/// Merge a list of strings into an existing TOML array field, deduplicating.
fn merge_string_array(
	table: &mut toml::map::Map<String, toml::Value>,
	key: &str,
	additions: &[String],
) {
	let existing: Vec<String> = table
		.get(key)
		.and_then(|v| v.as_array())
		.map(|arr| {
			arr.iter()
				.filter_map(|v| v.as_str().map(String::from))
				.collect()
		})
		.unwrap_or_default();

	let mut merged = existing;
	for item in additions {
		if !merged.contains(item) {
			merged.push(item.clone());
		}
	}

	table.insert(
		key.to_string(),
		toml::Value::Array(merged.into_iter().map(toml::Value::String).collect()),
	);
}
