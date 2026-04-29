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

/// A resolved capability's components — extracted from a capability TOML file.
/// Used by both static resolution (agent init) and dynamic resolution (runtime skill activation).
#[derive(Debug, Clone)]
pub struct ResolvedCapability {
	pub name: String,
	pub description: String,
	pub deps: Vec<String>,
	pub server_refs: Vec<String>,
	pub allowed_tools: Vec<String>,
	pub mcp_servers: Vec<crate::config::McpServerConfig>,
}

/// Parse a single capability TOML file and return its resolved components.
///
/// Searches across all taps for `capabilities/<cap_name>/<provider>.toml`.
/// The provider defaults to `"default"` unless overridden in `config.capabilities`.
///
/// Used at runtime when a skill declares `capabilities: [...]` and needs
/// to auto-load the backing MCP servers.
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
		let cap_path = tap_root
			.join("capabilities")
			.join(cap_name)
			.join(format!("{provider}.toml"));

		if !cap_path.exists() {
			continue;
		}

		let cap_str = fs::read_to_string(&cap_path)
			.with_context(|| format!("Failed to read capability file: {}", cap_path.display()))?;
		let cap: toml::Value = toml::from_str(&cap_str)
			.with_context(|| format!("Failed to parse capability file: {}", cap_path.display()))?;

		let description = cap
			.get("description")
			.and_then(|d| d.as_str())
			.map(String::from)
			.unwrap_or_else(|| format!("Capability '{cap_name}' (provider: {provider})"));

		let mut resolved = ResolvedCapability {
			name: cap_name.to_string(),
			description,
			deps: Vec::new(),
			server_refs: Vec::new(),
			allowed_tools: Vec::new(),
			mcp_servers: Vec::new(),
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
	let taps = crate::agent::taps::get_taps()
		.context("Failed to load taps for capability enumeration")?;

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
			let is_dir = entry
				.file_type()
				.map(|ft| ft.is_dir())
				.unwrap_or(false);
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
