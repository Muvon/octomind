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

//! Agent Plugins 1.0.0 support (agent-plugins.org).
//!
//! A plugin is a directory with a `plugin.json` manifest, optional
//! `skills/<name>/SKILL.md` skill dirs (Agent Skills spec — same format the
//! rest of `skill.rs` already loads) and an optional `mcp.json` declaring MCP
//! servers. Scanned locations, in priority order:
//! 1. Tap `plugins/` dirs
//! 2. Project: `<workdir>/.agents/plugins/`
//! 3. Global: `~/.config/agents/plugins/`
//!
//! Per spec, failures are component-isolated: a bad manifest rejects the
//! whole plugin, but a bad skill or MCP entry only skips that component.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use crate::config::{McpServerConfig, DEFAULT_MCP_TIMEOUT_SECONDS};

pub const PLUGIN_SCHEMA: &str = "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json";
pub const MCP_SCHEMA: &str = "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json";

#[derive(Debug, Clone)]
pub struct Plugin {
	/// Manifest `name` — plugin identity, used for dedupe and `PLUGIN_DATA`.
	pub name: String,
	/// Absolute plugin root directory.
	pub root: PathBuf,
}

// ---------------------------------------------------------------------------
// plugin.json manifest
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize)]
struct Manifest {
	#[serde(rename = "$schema")]
	schema: String,
	name: String,
}

/// Spec name rules: 1-64 chars, lowercase alphanumeric / `-` / `.`,
/// no leading or trailing `-`/`.`, no `--` or `..` runs.
fn valid_plugin_name(name: &str) -> bool {
	if name.is_empty() || name.len() > 64 {
		return false;
	}
	if !name
		.chars()
		.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.')
	{
		return false;
	}
	if name.starts_with(['-', '.']) || name.ends_with(['-', '.']) {
		return false;
	}
	!name.contains("--") && !name.contains("..")
}

/// Parse and validate a plugin.json. Returns the plugin name, or None when
/// the manifest is invalid (fatal per spec — the whole plugin is rejected).
/// Unknown top-level fields are ignored (non-fatal per spec).
fn parse_manifest(json: &str) -> Option<String> {
	let manifest: Manifest = match serde_json::from_str(json) {
		Ok(m) => m,
		Err(e) => {
			crate::log_debug!("plugin: invalid plugin.json: {}", e);
			return None;
		}
	};
	if manifest.schema != PLUGIN_SCHEMA {
		crate::log_debug!("plugin: unsupported $schema: {}", manifest.schema);
		return None;
	}
	if !valid_plugin_name(&manifest.name) {
		crate::log_debug!("plugin: invalid plugin name: {}", manifest.name);
		return None;
	}
	Some(manifest.name)
}

// ---------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------

/// Parent directories that may contain plugins, in priority order.
fn plugin_parent_dirs(workdir: &Path) -> Vec<PathBuf> {
	let mut dirs = Vec::new();

	if let Ok(taps) = crate::agent::taps::get_taps() {
		for tap in &taps {
			if let Ok(dir) = tap.plugins_dir() {
				if dir.is_dir() {
					dirs.push(dir);
				}
			}
		}
	}

	let project = workdir.join(".agents").join("plugins");
	if project.is_dir() {
		dirs.push(project);
	}

	if let Some(home) = dirs::home_dir() {
		let global = home.join(".config").join("agents").join("plugins");
		if global.is_dir() {
			dirs.push(global);
		}
	}

	dirs
}

/// Scan one parent dir: every immediate child dir with a valid plugin.json.
fn scan_plugins_in(dir: &Path) -> Vec<Plugin> {
	let entries = match std::fs::read_dir(dir) {
		Ok(e) => e,
		Err(_) => return Vec::new(),
	};

	let mut plugins = Vec::new();
	for entry in entries.flatten() {
		let root = entry.path();
		if !root.is_dir() {
			continue;
		}
		let manifest_path = root.join("plugin.json");
		if !manifest_path.is_file() {
			continue;
		}
		let json = match std::fs::read_to_string(&manifest_path) {
			Ok(c) => c,
			Err(_) => continue,
		};
		if let Some(name) = parse_manifest(&json) {
			plugins.push(Plugin { name, root });
		}
	}
	plugins
}

/// All installed plugins, deduplicated by manifest name (first source wins).
pub fn find_plugins() -> Vec<Plugin> {
	let workdir = crate::mcp::workdir::get_thread_working_directory();
	let mut seen = std::collections::HashSet::new();
	let mut plugins = Vec::new();
	for dir in plugin_parent_dirs(&workdir) {
		for plugin in scan_plugins_in(&dir) {
			if seen.insert(plugin.name.clone()) {
				plugins.push(plugin);
			}
		}
	}
	plugins
}

/// Skill dirs of a plugin: immediate children of `<root>/skills/` containing
/// a SKILL.md. Per spec, deeper descendants are not scanned.
pub fn skill_dirs(plugin: &Plugin) -> Vec<PathBuf> {
	let skills = plugin.root.join("skills");
	let entries = match std::fs::read_dir(&skills) {
		Ok(e) => e,
		Err(_) => return Vec::new(),
	};
	entries
		.flatten()
		.map(|e| e.path())
		.filter(|p| p.is_dir() && p.join("SKILL.md").is_file())
		.collect()
}

/// Reverse lookup: `<plugin_root>/skills/<skill>` → the owning plugin.
/// Used on skill activation to bring in the plugin's mcp.json servers.
pub fn plugin_for_skill_dir(skill_dir: &Path) -> Option<Plugin> {
	let skills = skill_dir.parent()?;
	if skills.file_name()? != "skills" {
		return None;
	}
	let root = skills.parent()?;
	let json = std::fs::read_to_string(root.join("plugin.json")).ok()?;
	let name = parse_manifest(&json)?;
	Some(Plugin {
		name,
		root: root.to_path_buf(),
	})
}

// ---------------------------------------------------------------------------
// mcp.json
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize)]
struct McpFile {
	#[serde(rename = "$schema")]
	schema: String,
	#[serde(rename = "mcpServers")]
	mcp_servers: BTreeMap<String, serde_json::Value>,
}

#[derive(serde::Deserialize)]
struct StdioEntry {
	command: String,
	#[serde(default)]
	args: Vec<String>,
	#[serde(default)]
	env: HashMap<String, String>,
	#[serde(default)]
	cwd: Option<String>,
}

#[derive(serde::Deserialize)]
struct HttpEntry {
	url: String,
	#[serde(default)]
	headers: HashMap<String, String>,
}

/// Client-managed persistent dir the spec exposes as `PLUGIN_DATA`.
fn plugin_data_dir(plugin_name: &str) -> anyhow::Result<PathBuf> {
	Ok(crate::directories::get_octomind_data_dir()?
		.join("plugin-data")
		.join(plugin_name))
}

/// Single non-recursive substitution of the two spec placeholders.
fn expand_placeholders(s: &str, root: &str, data: &str) -> String {
	s.replace("${PLUGIN_ROOT}", root)
		.replace("${PLUGIN_DATA}", data)
}

/// Resolve a `cwd` value to an absolute path. Spec allows only `./`-relative,
/// `${PLUGIN_ROOT}` and `${PLUGIN_DATA}` forms; `..` segments are rejected
/// (path containment).
fn resolve_cwd(cwd: &str, root: &Path, data: &Path) -> Option<PathBuf> {
	let resolved = if let Some(rest) = cwd.strip_prefix("./") {
		root.join(rest)
	} else if cwd == "${PLUGIN_ROOT}" {
		root.to_path_buf()
	} else if let Some(rest) = cwd.strip_prefix("${PLUGIN_ROOT}/") {
		root.join(rest)
	} else if cwd == "${PLUGIN_DATA}" {
		data.to_path_buf()
	} else {
		let rest = cwd.strip_prefix("${PLUGIN_DATA}/")?;
		data.join(rest)
	};
	if resolved
		.components()
		.any(|c| matches!(c, std::path::Component::ParentDir))
	{
		return None;
	}
	Some(resolved)
}

/// Load a plugin's mcp.json into octomind server configs.
///
/// Per spec, an unreadable/mismatched mcp.json disables MCP for the plugin
/// only (returns empty — skills still load), and an invalid individual entry
/// is skipped while its siblings load.
pub fn load_mcp_servers(plugin: &Plugin) -> Vec<McpServerConfig> {
	let data = match plugin_data_dir(&plugin.name) {
		Ok(d) => d,
		Err(e) => {
			crate::log_debug!("plugin '{}': no data dir: {}", plugin.name, e);
			return Vec::new();
		}
	};
	load_mcp_servers_with(plugin, &data)
}

fn load_mcp_servers_with(plugin: &Plugin, data: &Path) -> Vec<McpServerConfig> {
	let path = plugin.root.join("mcp.json");
	if !path.is_file() {
		return Vec::new();
	}
	let content = match std::fs::read_to_string(&path) {
		Ok(c) => c,
		Err(e) => {
			crate::log_debug!("plugin '{}': mcp.json unreadable: {}", plugin.name, e);
			return Vec::new();
		}
	};
	let file: McpFile = match serde_json::from_str(&content) {
		Ok(f) => f,
		Err(e) => {
			crate::log_debug!("plugin '{}': mcp.json invalid: {}", plugin.name, e);
			return Vec::new();
		}
	};
	if file.schema != MCP_SCHEMA {
		crate::log_debug!(
			"plugin '{}': unsupported mcp.json $schema: {}",
			plugin.name,
			file.schema
		);
		return Vec::new();
	}

	// Spec requires PLUGIN_DATA to exist and be writable before servers run.
	if let Err(e) = std::fs::create_dir_all(data) {
		crate::log_debug!("plugin '{}': cannot create data dir: {}", plugin.name, e);
		return Vec::new();
	}

	let root_str = plugin.root.to_string_lossy().into_owned();
	let data_str = data.to_string_lossy().into_owned();

	let mut servers = Vec::new();
	for (name, value) in &file.mcp_servers {
		if name.is_empty() {
			continue;
		}
		let entry_type = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
		let built = match entry_type {
			"stdio" => build_stdio(name, value.clone(), plugin, data, &root_str, &data_str),
			"streamable-http" => build_http(name, value.clone(), plugin),
			// `sse` is deprecated and optional per spec; unknown transports are skipped.
			other => {
				crate::log_debug!(
					"plugin '{}': server '{}' has unsupported transport '{}', skipping",
					plugin.name,
					name,
					other
				);
				None
			}
		};
		if let Some(server) = built {
			servers.push(server);
		}
	}
	servers
}

fn build_stdio(
	name: &str,
	value: serde_json::Value,
	plugin: &Plugin,
	data: &Path,
	root_str: &str,
	data_str: &str,
) -> Option<McpServerConfig> {
	let entry: StdioEntry = match serde_json::from_value(value) {
		Ok(e) => e,
		Err(e) => {
			crate::log_debug!(
				"plugin '{}': server '{}' invalid stdio entry: {}",
				plugin.name,
				name,
				e
			);
			return None;
		}
	};

	// Spec: single bare name or `./`-relative path — never a shell string.
	if entry.command.is_empty() || entry.command.chars().any(char::is_whitespace) {
		crate::log_debug!(
			"plugin '{}': server '{}' command is not a single token, skipping",
			plugin.name,
			name
		);
		return None;
	}
	let command = if let Some(rest) = entry.command.strip_prefix("./") {
		let resolved = plugin.root.join(rest);
		if resolved
			.components()
			.any(|c| matches!(c, std::path::Component::ParentDir))
		{
			crate::log_debug!(
				"plugin '{}': server '{}' command escapes plugin root, skipping",
				plugin.name,
				name
			);
			return None;
		}
		resolved.to_string_lossy().into_owned()
	} else {
		entry.command
	};

	// Spec reserves these keys for the client; an entry declaring them is invalid.
	if entry.env.contains_key("PLUGIN_ROOT") || entry.env.contains_key("PLUGIN_DATA") {
		crate::log_debug!(
			"plugin '{}': server '{}' env declares reserved PLUGIN_ROOT/PLUGIN_DATA, skipping",
			plugin.name,
			name
		);
		return None;
	}

	let cwd = match &entry.cwd {
		Some(c) => match resolve_cwd(c, &plugin.root, data) {
			Some(p) => p,
			None => {
				crate::log_debug!(
					"plugin '{}': server '{}' cwd '{}' not allowed, skipping",
					plugin.name,
					name,
					c
				);
				return None;
			}
		},
		None => plugin.root.clone(),
	};

	let args = entry
		.args
		.iter()
		.map(|a| expand_placeholders(a, root_str, data_str))
		.collect();
	let mut env: HashMap<String, String> = entry
		.env
		.iter()
		.map(|(k, v)| (k.clone(), expand_placeholders(v, root_str, data_str)))
		.collect();
	env.insert("PLUGIN_ROOT".to_string(), root_str.to_string());
	env.insert("PLUGIN_DATA".to_string(), data_str.to_string());

	Some(McpServerConfig::Stdin {
		name: name.to_string(),
		command,
		args,
		timeout_seconds: DEFAULT_MCP_TIMEOUT_SECONDS,
		tools: Vec::new(),
		env,
		cwd: Some(cwd.to_string_lossy().into_owned()),
		auto_bind: None,
	})
}

fn build_http(name: &str, value: serde_json::Value, plugin: &Plugin) -> Option<McpServerConfig> {
	let entry: HttpEntry = match serde_json::from_value(value) {
		Ok(e) => e,
		Err(e) => {
			crate::log_debug!(
				"plugin '{}': server '{}' invalid streamable-http entry: {}",
				plugin.name,
				name,
				e
			);
			return None;
		}
	};

	if !url_allowed(&entry.url) {
		crate::log_debug!(
			"plugin '{}': server '{}' url '{}' rejected (non-loopback must be https)",
			plugin.name,
			name,
			entry.url
		);
		return None;
	}

	Some(McpServerConfig::Http {
		name: name.to_string(),
		url: entry.url,
		timeout_seconds: DEFAULT_MCP_TIMEOUT_SECONDS,
		tools: Vec::new(),
		headers: entry.headers,
		auto_bind: None,
	})
}

/// Spec: absolute HTTP/HTTPS URL; non-loopback must be HTTPS.
// ponytail: loopback = localhost/127.0.0.1/[::1] literals, full 127/8 parsing if ever needed
fn url_allowed(url: &str) -> bool {
	if url.starts_with("https://") {
		return true;
	}
	if let Some(rest) = url.strip_prefix("http://") {
		let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
		// Strip an optional :port. Bracketed IPv6 keeps its own colons.
		let host = if authority.starts_with('[') {
			authority.split(']').next().map(|h| format!("{}]", h))
		} else {
			authority.split(':').next().map(str::to_string)
		}
		.unwrap_or_default();
		return host == "localhost" || host == "127.0.0.1" || host == "[::1]";
	}
	false
}

#[cfg(test)]
#[path = "plugin_inline_tests.rs"]
mod inline_tests;

#[cfg(test)]
#[path = "plugin_tests.rs"]
mod plugin_tests;
