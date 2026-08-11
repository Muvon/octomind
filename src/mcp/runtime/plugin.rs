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

use crate::config::McpServerConfig;

pub const PLUGIN_SCHEMA: &str = "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json";
pub const MCP_SCHEMA: &str = "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json";

/// Matches the `mcp add` tool default in `runtime/dynamic.rs`.
const DEFAULT_TIMEOUT_SECONDS: u64 = 60;

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
		timeout_seconds: DEFAULT_TIMEOUT_SECONDS,
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

	// Our HTTP transport carries no custom headers; dropping them silently
	// would break auth in confusing ways, so skip the entry instead.
	if !entry.headers.is_empty() {
		crate::log_debug!(
			"plugin '{}': server '{}' declares headers (unsupported), skipping",
			plugin.name,
			name
		);
		return None;
	}

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
		timeout_seconds: DEFAULT_TIMEOUT_SECONDS,
		tools: Vec::new(),
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
mod tests {
	use super::*;

	fn write(path: &Path, content: &str) {
		std::fs::create_dir_all(path.parent().unwrap()).unwrap();
		std::fs::write(path, content).unwrap();
	}

	fn manifest(name: &str) -> String {
		format!(r#"{{"$schema": "{}", "name": "{}"}}"#, PLUGIN_SCHEMA, name)
	}

	// -----------------------------------------------------------------------
	// Manifest validation
	// -----------------------------------------------------------------------

	#[test]
	fn plugin_names_follow_spec_rules() {
		assert!(valid_plugin_name("pdf-tools"));
		assert!(valid_plugin_name("com.example.plugin"));
		assert!(valid_plugin_name("a"));
		assert!(valid_plugin_name(&"a".repeat(64)));

		assert!(!valid_plugin_name(""));
		assert!(!valid_plugin_name(&"a".repeat(65)));
		assert!(!valid_plugin_name("PDF-Tools"));
		assert!(!valid_plugin_name("-pdf"));
		assert!(!valid_plugin_name("pdf-"));
		assert!(!valid_plugin_name(".pdf"));
		assert!(!valid_plugin_name("pdf."));
		assert!(!valid_plugin_name("pdf--tools"));
		assert!(!valid_plugin_name("com..example"));
		assert!(!valid_plugin_name("has space"));
		assert!(!valid_plugin_name("under_score"));
	}

	#[test]
	fn manifest_parses_and_ignores_unknown_fields() {
		let json = format!(
			r#"{{"$schema": "{}", "name": "my-plugin", "version": "1.2.3", "unknown": [1], "extensions": "not-an-object"}}"#,
			PLUGIN_SCHEMA
		);
		assert_eq!(parse_manifest(&json).as_deref(), Some("my-plugin"));
	}

	#[test]
	fn manifest_rejects_bad_schema_name_or_json() {
		let wrong_schema = r#"{"$schema": "https://example.com/other.json", "name": "x"}"#;
		assert!(parse_manifest(wrong_schema).is_none());
		let bad_name = manifest("Bad_Name");
		assert!(parse_manifest(&bad_name).is_none());
		let missing_name = format!(r#"{{"$schema": "{}"}}"#, PLUGIN_SCHEMA);
		assert!(parse_manifest(&missing_name).is_none());
		assert!(parse_manifest("not json").is_none());
	}

	// -----------------------------------------------------------------------
	// Discovery
	// -----------------------------------------------------------------------

	#[test]
	fn scan_finds_valid_plugins_and_skips_invalid_ones() {
		let tmp = tempfile::tempdir().unwrap();
		let dir = tmp.path();

		write(&dir.join("good/plugin.json"), &manifest("good"));
		write(&dir.join("bad/plugin.json"), "not json");
		write(&dir.join("no-manifest/readme.txt"), "hi");
		write(&dir.join("stray-file"), "not a dir");

		let plugins = scan_plugins_in(dir);
		assert_eq!(plugins.len(), 1);
		assert_eq!(plugins[0].name, "good");
		assert_eq!(plugins[0].root, dir.join("good"));
	}

	#[test]
	fn skill_dirs_are_immediate_children_with_skill_md() {
		let tmp = tempfile::tempdir().unwrap();
		let root = tmp.path().join("p");
		write(&root.join("plugin.json"), &manifest("p"));
		write(&root.join("skills/alpha/SKILL.md"), "---\n---");
		write(&root.join("skills/beta/notes.md"), "no skill here");
		// Per spec, nested skills are NOT discovered.
		write(&root.join("skills/beta/nested/SKILL.md"), "---\n---");

		let plugin = Plugin {
			name: "p".to_string(),
			root: root.clone(),
		};
		let dirs = skill_dirs(&plugin);
		assert_eq!(dirs, vec![root.join("skills/alpha")]);
	}

	#[test]
	fn plugin_lookup_from_skill_dir() {
		let tmp = tempfile::tempdir().unwrap();
		let root = tmp.path().join("my-plugin");
		write(&root.join("plugin.json"), &manifest("my-plugin"));
		write(&root.join("skills/alpha/SKILL.md"), "---\n---");

		let found = plugin_for_skill_dir(&root.join("skills/alpha")).unwrap();
		assert_eq!(found.name, "my-plugin");
		assert_eq!(found.root, root);

		// A tap/universal skill dir has no plugin.json two levels up.
		assert!(plugin_for_skill_dir(tmp.path()).is_none());
		assert!(plugin_for_skill_dir(&root.join("skills")).is_none());
	}

	// -----------------------------------------------------------------------
	// mcp.json
	// -----------------------------------------------------------------------

	fn plugin_with_mcp(tmp: &Path, servers_json: &str) -> (Plugin, PathBuf) {
		let root = tmp.join("p");
		write(&root.join("plugin.json"), &manifest("p"));
		write(
			&root.join("mcp.json"),
			&format!(
				r#"{{"$schema": "{}", "mcpServers": {}}}"#,
				MCP_SCHEMA, servers_json
			),
		);
		let data = tmp.join("data");
		(
			Plugin {
				name: "p".to_string(),
				root,
			},
			data,
		)
	}

	#[test]
	fn stdio_entry_maps_with_expansion_and_reserved_env() {
		let tmp = tempfile::tempdir().unwrap();
		let (plugin, data) = plugin_with_mcp(
			tmp.path(),
			r#"{"srv": {"type": "stdio", "command": "python3",
				"args": ["${PLUGIN_ROOT}/scripts/run.py", "--out", "${PLUGIN_DATA}/cache"],
				"env": {"MODE": "${PLUGIN_ROOT}/cfg"}}}"#,
		);
		let servers = load_mcp_servers_with(&plugin, &data);
		assert_eq!(servers.len(), 1);

		let root_str = plugin.root.to_string_lossy();
		let data_str = data.to_string_lossy();
		match &servers[0] {
			McpServerConfig::Stdin {
				name,
				command,
				args,
				env,
				cwd,
				..
			} => {
				assert_eq!(name, "srv");
				assert_eq!(command, "python3");
				assert_eq!(args[0], format!("{}/scripts/run.py", root_str));
				assert_eq!(args[2], format!("{}/cache", data_str));
				assert_eq!(env["MODE"], format!("{}/cfg", root_str));
				assert_eq!(env["PLUGIN_ROOT"], root_str);
				assert_eq!(env["PLUGIN_DATA"], data_str);
				assert_eq!(cwd.as_deref(), Some(root_str.as_ref()));
			}
			other => panic!("expected stdio server, got {:?}", other),
		}
		// The client must create PLUGIN_DATA before servers run.
		assert!(data.is_dir());
	}

	#[test]
	fn relative_command_resolves_inside_plugin_root() {
		let tmp = tempfile::tempdir().unwrap();
		let (plugin, data) = plugin_with_mcp(
			tmp.path(),
			r#"{"a": {"type": "stdio", "command": "./bin/server"},
				"b": {"type": "stdio", "command": "./../escape"},
				"c": {"type": "stdio", "command": "not a single token"}}"#,
		);
		let servers = load_mcp_servers_with(&plugin, &data);
		assert_eq!(servers.len(), 1);
		assert_eq!(
			servers[0].command().unwrap(),
			plugin.root.join("bin/server").to_string_lossy()
		);
	}

	#[test]
	fn reserved_env_keys_invalidate_only_that_entry() {
		let tmp = tempfile::tempdir().unwrap();
		let (plugin, data) = plugin_with_mcp(
			tmp.path(),
			r#"{"bad": {"type": "stdio", "command": "x", "env": {"PLUGIN_ROOT": "/tmp"}},
				"good": {"type": "stdio", "command": "y"}}"#,
		);
		let servers = load_mcp_servers_with(&plugin, &data);
		assert_eq!(servers.len(), 1);
		assert_eq!(servers[0].name(), "good");
	}

	#[test]
	fn cwd_forms_are_enforced() {
		let tmp = tempfile::tempdir().unwrap();
		let (plugin, data) = plugin_with_mcp(
			tmp.path(),
			r#"{"rel": {"type": "stdio", "command": "x", "cwd": "./sub"},
				"data": {"type": "stdio", "command": "x", "cwd": "${PLUGIN_DATA}/work"},
				"abs": {"type": "stdio", "command": "x", "cwd": "/etc"},
				"escape": {"type": "stdio", "command": "x", "cwd": "./../out"}}"#,
		);
		let servers = load_mcp_servers_with(&plugin, &data);
		let by_name: HashMap<&str, &McpServerConfig> =
			servers.iter().map(|s| (s.name(), s)).collect();
		assert_eq!(servers.len(), 2);
		match by_name["rel"] {
			McpServerConfig::Stdin { cwd, .. } => assert_eq!(
				cwd.as_deref(),
				Some(plugin.root.join("sub").to_string_lossy().as_ref())
			),
			_ => unreachable!(),
		}
		match by_name["data"] {
			McpServerConfig::Stdin { cwd, .. } => assert_eq!(
				cwd.as_deref(),
				Some(data.join("work").to_string_lossy().as_ref())
			),
			_ => unreachable!(),
		}
	}

	#[test]
	fn http_entries_enforce_https_and_reject_headers() {
		let tmp = tempfile::tempdir().unwrap();
		let (plugin, data) = plugin_with_mcp(
			tmp.path(),
			r#"{"ok": {"type": "streamable-http", "url": "https://mcp.example.com/x"},
				"local": {"type": "streamable-http", "url": "http://localhost:3000/mcp"},
				"plain": {"type": "streamable-http", "url": "http://example.com/mcp"},
				"authed": {"type": "streamable-http", "url": "https://x.com", "headers": {"Authorization": "Bearer t"}}}"#,
		);
		let servers = load_mcp_servers_with(&plugin, &data);
		let names: Vec<&str> = servers.iter().map(|s| s.name()).collect();
		assert_eq!(names, vec!["local", "ok"]);
	}

	#[test]
	fn unsupported_transports_and_bad_entries_are_skipped() {
		let tmp = tempfile::tempdir().unwrap();
		let (plugin, data) = plugin_with_mcp(
			tmp.path(),
			r#"{"legacy": {"type": "sse", "url": "https://x.com"},
				"mystery": {"type": "quantum"},
				"untyped": {"command": "x"},
				"broken": {"type": "stdio"},
				"good": {"type": "stdio", "command": "x"}}"#,
		);
		let servers = load_mcp_servers_with(&plugin, &data);
		assert_eq!(servers.len(), 1);
		assert_eq!(servers[0].name(), "good");
	}

	#[test]
	fn bad_mcp_json_disables_mcp_only() {
		let tmp = tempfile::tempdir().unwrap();
		let root = tmp.path().join("p");
		write(&root.join("plugin.json"), &manifest("p"));
		let plugin = Plugin {
			name: "p".to_string(),
			root: root.clone(),
		};
		let data = tmp.path().join("data");

		// Missing file → no servers.
		assert!(load_mcp_servers_with(&plugin, &data).is_empty());

		// Invalid JSON → no servers.
		write(&root.join("mcp.json"), "not json");
		assert!(load_mcp_servers_with(&plugin, &data).is_empty());

		// Wrong $schema → no servers.
		write(
			&root.join("mcp.json"),
			r#"{"$schema": "https://example.com/v9.json", "mcpServers": {}}"#,
		);
		assert!(load_mcp_servers_with(&plugin, &data).is_empty());
	}

	#[test]
	fn url_loopback_rules() {
		assert!(url_allowed("https://anything.example.com/mcp"));
		assert!(url_allowed("http://localhost/mcp"));
		assert!(url_allowed("http://localhost:8080/mcp"));
		assert!(url_allowed("http://127.0.0.1:9000"));
		assert!(url_allowed("http://[::1]:9000/x"));
		assert!(!url_allowed("http://example.com/mcp"));
		assert!(!url_allowed("ftp://example.com"));
		assert!(!url_allowed("mcp.example.com"));
	}

	#[test]
	fn placeholder_expansion_is_textual() {
		assert_eq!(
			expand_placeholders("${PLUGIN_ROOT}/a ${PLUGIN_DATA}/b ${OTHER}", "/r", "/d"),
			"/r/a /d/b ${OTHER}"
		);
	}
}
