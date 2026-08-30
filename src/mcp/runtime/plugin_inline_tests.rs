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
	let by_name: HashMap<&str, &McpServerConfig> = servers.iter().map(|s| (s.name(), s)).collect();
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
fn http_entries_enforce_https_and_preserve_headers() {
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
	assert_eq!(names, vec!["authed", "local", "ok"]);
	let authed = servers
		.iter()
		.find(|server| server.name() == "authed")
		.expect("plugin HTTP server with headers must load");
	assert_eq!(
		authed
			.headers()
			.and_then(|headers| headers.get("Authorization"))
			.map(String::as_str),
		Some("Bearer t")
	);
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
