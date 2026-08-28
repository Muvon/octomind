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

//! Env/data-dir-dependent plugin tests: discovery priority + dedupe across
//! sources, the public `load_mcp_servers` wrapper (real PLUGIN_DATA dir),
//! and `resolve_cwd` forms. The inline `mod tests` covers the pure helpers
//! (manifest validation, entry building, URL rules).

use super::*;
use serial_test::serial;

/// Point `OCTOMIND_DATA_DIR` at a fresh tempdir. Tests using it must be
/// `#[serial]` (env is process-global).
struct DataDirGuard {
	previous: Option<std::ffi::OsString>,
	_dir: tempfile::TempDir,
}

impl DataDirGuard {
	fn new() -> Self {
		let previous = std::env::var_os("OCTOMIND_DATA_DIR");
		let dir = tempfile::tempdir().expect("failed to create tempdir");
		std::env::set_var("OCTOMIND_DATA_DIR", dir.path());
		Self {
			previous,
			_dir: dir,
		}
	}
}

impl Drop for DataDirGuard {
	fn drop(&mut self) {
		match self.previous.take() {
			Some(v) => std::env::set_var("OCTOMIND_DATA_DIR", v),
			None => std::env::remove_var("OCTOMIND_DATA_DIR"),
		}
	}
}

fn write(path: &Path, content: &str) {
	std::fs::create_dir_all(path.parent().expect("parent")).expect("create dirs");
	std::fs::write(path, content).expect("write file");
}

fn manifest(name: &str) -> String {
	format!(r#"{{"$schema": "{}", "name": "{}"}}"#, PLUGIN_SCHEMA, name)
}

/// The default tap's `plugins/` dir inside the current data dir.
fn default_tap_plugins_dir() -> PathBuf {
	let dir = crate::directories::get_octomind_data_dir()
		.expect("data dir")
		.join("taps")
		.join("muvon")
		.join("octomind-tap")
		.join("plugins");
	std::fs::create_dir_all(&dir).expect("create tap plugins dir");
	dir
}

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

// ---------------------------------------------------------------------------
// cwd resolution forms
// ---------------------------------------------------------------------------

#[test]
fn resolve_cwd_accepts_all_spec_forms() {
	let root = Path::new("/plugins/p");
	let data = Path::new("/data/p");
	assert_eq!(resolve_cwd("./sub", root, data), Some(root.join("sub")));
	assert_eq!(
		resolve_cwd("${PLUGIN_ROOT}", root, data),
		Some(root.to_path_buf())
	);
	assert_eq!(
		resolve_cwd("${PLUGIN_ROOT}/x", root, data),
		Some(root.join("x"))
	);
	assert_eq!(
		resolve_cwd("${PLUGIN_DATA}", root, data),
		Some(data.to_path_buf())
	);
	assert_eq!(
		resolve_cwd("${PLUGIN_DATA}/y", root, data),
		Some(data.join("y"))
	);
}

#[test]
fn resolve_cwd_rejects_escapes_and_unprefixed_forms() {
	let root = Path::new("/plugins/p");
	let data = Path::new("/data/p");
	assert_eq!(resolve_cwd("./../out", root, data), None);
	assert_eq!(resolve_cwd("${PLUGIN_DATA}/../escape", root, data), None);
	assert_eq!(
		resolve_cwd("/etc", root, data),
		None,
		"absolute paths not allowed"
	);
	assert_eq!(
		resolve_cwd("relative", root, data),
		None,
		"bare relative not allowed"
	);
}

// ---------------------------------------------------------------------------
// Data dir + public loader
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn plugin_data_dir_is_namespaced_under_data_dir() {
	let _guard = DataDirGuard::new();
	let dir = plugin_data_dir("my-plugin").expect("data dir");
	let data = crate::directories::get_octomind_data_dir().expect("data dir");
	assert_eq!(dir, data.join("plugin-data").join("my-plugin"));
}

#[test]
#[serial]
fn load_mcp_servers_uses_real_data_dir_and_creates_it() {
	let _guard = DataDirGuard::new();
	let tmp = tempfile::tempdir().expect("tempdir");
	let (plugin, _data) =
		plugin_with_mcp(tmp.path(), r#"{"srv": {"type": "stdio", "command": "x"}}"#);

	let servers = load_mcp_servers(&plugin);
	assert_eq!(servers.len(), 1);

	let expected_data = crate::directories::get_octomind_data_dir()
		.expect("data dir")
		.join("plugin-data")
		.join("p");
	assert!(
		expected_data.is_dir(),
		"PLUGIN_DATA created before servers run"
	);
	match &servers[0] {
		McpServerConfig::Stdin {
			env,
			timeout_seconds,
			tools,
			..
		} => {
			assert_eq!(
				env.get("PLUGIN_DATA").map(String::as_str),
				Some(expected_data.to_string_lossy().as_ref())
			);
			assert_eq!(*timeout_seconds, DEFAULT_MCP_TIMEOUT_SECONDS);
			assert!(tools.is_empty());
		}
		other => panic!("expected stdio server, got {:?}", other),
	}
}

// ---------------------------------------------------------------------------
// Discovery across sources
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn plugin_parent_dirs_order_taps_then_project() {
	let _guard = DataDirGuard::new();
	let tap_plugins = default_tap_plugins_dir();

	let work = tempfile::tempdir().expect("tempdir");
	let project = work.path().join(".agents").join("plugins");
	std::fs::create_dir_all(&project).expect("create project plugins dir");

	let dirs = plugin_parent_dirs(work.path());
	assert_eq!(dirs.first(), Some(&tap_plugins), "taps come first");
	assert!(dirs.contains(&project), "project dir included when present");
}

#[test]
#[serial]
fn find_plugins_dedupes_by_name_with_tap_priority() {
	let _guard = DataDirGuard::new();
	let tap_plugins = default_tap_plugins_dir();
	write(
		&tap_plugins.join("octomind-dup/plugin.json"),
		&manifest("octomind-dup"),
	);
	write(
		&tap_plugins.join("octomind-tap-only/plugin.json"),
		&manifest("octomind-tap-only"),
	);

	let work = tempfile::tempdir().expect("tempdir");
	let project = work.path().join(".agents").join("plugins");
	write(
		&project.join("octomind-dup/plugin.json"),
		&manifest("octomind-dup"),
	);
	write(
		&project.join("octomind-project-only/plugin.json"),
		&manifest("octomind-project-only"),
	);

	crate::mcp::workdir::set_session_working_directory(work.path().to_path_buf());

	let plugins = find_plugins();
	let dup: Vec<&Plugin> = plugins
		.iter()
		.filter(|p| p.name == "octomind-dup")
		.collect();
	assert_eq!(dup.len(), 1, "deduplicated by manifest name");
	assert_eq!(
		dup[0].root,
		tap_plugins.join("octomind-dup"),
		"tap source wins over project"
	);

	let names: Vec<&str> = plugins.iter().map(|p| p.name.as_str()).collect();
	assert!(names.contains(&"octomind-tap-only"));
	assert!(names.contains(&"octomind-project-only"));
}

#[test]
fn plugin_lookup_rejects_invalid_manifest() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let root = tmp.path().join("bad");
	write(&root.join("plugin.json"), "not json");
	write(&root.join("skills/alpha/SKILL.md"), "---\n---");
	assert!(plugin_for_skill_dir(&root.join("skills/alpha")).is_none());
}

// ---------------------------------------------------------------------------
// mcp.json entry edge cases
// ---------------------------------------------------------------------------

#[test]
fn empty_server_name_is_skipped_but_siblings_load() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let (plugin, data) = plugin_with_mcp(
		tmp.path(),
		r#"{"": {"type": "stdio", "command": "x"}, "ok": {"type": "stdio", "command": "y"}}"#,
	);
	let servers = load_mcp_servers_with(&plugin, &data);
	assert_eq!(servers.len(), 1);
	assert_eq!(servers[0].name(), "ok");
}

#[test]
fn empty_mcp_servers_map_yields_no_servers() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let (plugin, data) = plugin_with_mcp(tmp.path(), "{}");
	assert!(load_mcp_servers_with(&plugin, &data).is_empty());
}

#[test]
fn http_entry_missing_url_is_skipped() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let (plugin, data) = plugin_with_mcp(tmp.path(), r#"{"h": {"type": "streamable-http"}}"#);
	assert!(load_mcp_servers_with(&plugin, &data).is_empty());
}
