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

//! Path-based config load/save round trips against the shipped template in
//! a tempdir — the exact flow `--config <path>` and the setters use.

use super::*;

#[test]
fn test_load_from_path_roundtrip() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let path = tmp.path().join("config.toml");
	std::fs::write(&path, include_str!("../../config-templates/default.toml"))
		.expect("write template");

	let mut config = Config::load_from_path(&path).expect("load template from path");
	assert!(!config.model.is_empty());
	assert!(!config.roles.is_empty());

	// Mutate, save to a new path, reload — the change must survive.
	config.model = "ollama:roundtrip-model".to_string();
	let out = tmp.path().join("saved.toml");
	config.save_to_path(&out).expect("save to path");
	let reloaded = Config::load_from_path(&out).expect("reload saved config");
	assert_eq!(reloaded.model, "ollama:roundtrip-model");

	// The clean copy used for saving parses back too
	let clean = reloaded.create_clean_copy_for_saving();
	let serialized = toml::to_string(&clean).expect("serialize clean copy");
	let reparsed: Config = toml::from_str(&serialized).expect("reparse clean copy");
	assert_eq!(reparsed.model, "ollama:roundtrip-model");
}

#[test]
fn test_load_from_path_failures() {
	let tmp = tempfile::tempdir().expect("tempdir");

	// Missing file
	assert!(Config::load_from_path(&tmp.path().join("absent.toml")).is_err());

	// Present but not valid config TOML
	let bad = tmp.path().join("bad.toml");
	std::fs::write(&bad, "this = [is not : valid").expect("write bad file");
	assert!(Config::load_from_path(&bad).is_err());
}

// --- multi-file directory merging -------------------------------------

fn template_toml() -> String {
	include_str!("../../config-templates/default.toml").to_string()
}

fn write_file(dir: &std::path::Path, name: &str, content: &str) {
	std::fs::write(dir.join(name), content).expect("write fixture");
}

#[test]
fn config_toml_is_the_base_even_when_other_files_sort_earlier() {
	let tmp = tempfile::tempdir().expect("tempdir");
	write_file(tmp.path(), "config.toml", &template_toml());
	write_file(tmp.path(), "a-first.toml", "model = \"from-a\"\n");
	let config = Config::load_from_path(tmp.path()).expect("directory must load");
	assert_eq!(config.model, "from-a");
}

#[test]
fn regular_files_merge_in_alphabetical_order() {
	let tmp = tempfile::tempdir().expect("tempdir");
	write_file(tmp.path(), "config.toml", &template_toml());
	write_file(tmp.path(), "a.toml", "model = \"a\"\n");
	write_file(tmp.path(), "z.toml", "model = \"z\"\n");
	let config = Config::load_from_path(tmp.path()).expect("directory must load");
	assert_eq!(config.model, "z");
}

#[test]
fn mcp_extension_files_load_after_every_regular_file() {
	// "mcp-a.toml" sorts before "z.toml" alphabetically, but the documented
	// contract loads mcp-*.toml overrides last, so its field must win.
	let tmp = tempfile::tempdir().expect("tempdir");
	write_file(tmp.path(), "config.toml", &template_toml());
	write_file(tmp.path(), "z.toml", "model = \"z\"\n");
	write_file(tmp.path(), "mcp-a.toml", "model = \"mcp-a\"\n");
	let config = Config::load_from_path(tmp.path()).expect("directory must load");
	assert_eq!(config.model, "mcp-a");
}

#[test]
fn mcp_extension_files_override_same_named_servers_from_mcp_toml() {
	let tmp = tempfile::tempdir().expect("tempdir");
	write_file(tmp.path(), "config.toml", &template_toml());
	write_file(
		tmp.path(),
		"mcp.toml",
		"\n[[mcp.servers]]\nname = \"dup\"\ntype = \"stdio\"\ncommand = \"first\"\nargs = []\ntimeout_seconds = 30\ntools = []\n",
	);
	write_file(
		tmp.path(),
		"mcp-dup.toml",
		"\n[[mcp.servers]]\nname = \"dup\"\ntype = \"stdio\"\ncommand = \"second\"\nargs = []\ntimeout_seconds = 30\ntools = []\n",
	);
	let config = Config::load_from_path(tmp.path()).expect("directory must load");
	let dups: Vec<_> = config
		.mcp
		.servers
		.iter()
		.filter(|server| server.name() == "dup")
		.collect();
	assert_eq!(dups.len(), 1, "same-named servers must dedup to one entry");
	assert_eq!(
		dups[0].command(),
		Some("second"),
		"the mcp-*.toml entry must win"
	);
}

#[test]
fn server_arrays_concatenate_across_files() {
	let tmp = tempfile::tempdir().expect("tempdir");
	write_file(tmp.path(), "config.toml", &template_toml());
	write_file(
		tmp.path(),
		"extra-servers.toml",
		"\n[[mcp.servers]]\nname = \"alpha-extra\"\ntype = \"stdio\"\ncommand = \"alpha\"\nargs = []\ntimeout_seconds = 30\ntools = []\n\n[[mcp.servers]]\nname = \"beta-extra\"\ntype = \"stdio\"\ncommand = \"beta\"\nargs = []\ntimeout_seconds = 30\ntools = []\n",
	);
	let config = Config::load_from_path(tmp.path()).expect("directory must load");
	let names: Vec<&str> = config.mcp.servers.iter().map(|s| s.name()).collect();
	assert!(
		names.contains(&"core"),
		"template servers must survive: {names:?}"
	);
	assert!(
		names.contains(&"alpha-extra"),
		"added servers must stack: {names:?}"
	);
	assert!(
		names.contains(&"beta-extra"),
		"added servers must stack: {names:?}"
	);
}

#[test]
fn scalar_arrays_replace_rather_than_concatenate() {
	let tmp = tempfile::tempdir().expect("tempdir");
	write_file(tmp.path(), "config.toml", &template_toml());
	write_file(tmp.path(), "b1.toml", "[mcp]\nallowed_tools = [\"one\"]\n");
	write_file(tmp.path(), "b2.toml", "[mcp]\nallowed_tools = [\"two\"]\n");
	let config = Config::load_from_path(tmp.path()).expect("directory must load");
	assert_eq!(
		config.mcp.allowed_tools,
		vec!["two"],
		"scalar arrays replace"
	);
}

#[test]
fn tables_deep_merge_across_files() {
	let tmp = tempfile::tempdir().expect("tempdir");
	write_file(tmp.path(), "config.toml", &template_toml());
	write_file(
		tmp.path(),
		"decision.toml",
		"[compression.decision]\nmax_tokens = 999\n",
	);
	let config = Config::load_from_path(tmp.path()).expect("directory must load");
	assert_eq!(config.compression.decision.max_tokens, 999);
	assert!(
		!config.compression.decision.model.is_empty(),
		"sibling keys survive"
	);
	assert!(
		config.compression.threshold > 0,
		"parent table keys survive"
	);
}

#[test]
fn malformed_toml_in_any_file_fails_the_directory_load() {
	let tmp = tempfile::tempdir().expect("tempdir");
	write_file(tmp.path(), "config.toml", &template_toml());
	write_file(tmp.path(), "broken.toml", "this = [is not : valid");
	let error = Config::load_from_path(tmp.path()).unwrap_err().to_string();
	assert!(error.contains("broken.toml"), "got: {error}");
}

#[test]
fn a_directory_without_toml_files_is_an_error() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let error = Config::load_from_path(tmp.path()).unwrap_err().to_string();
	assert!(error.contains("No TOML files found"), "got: {error}");
}

#[test]
fn merged_config_missing_required_fields_is_rejected() {
	let tmp = tempfile::tempdir().expect("tempdir");
	write_file(tmp.path(), "only.toml", "just_a_key = 1\n");
	let error = Config::load_from_path(tmp.path()).unwrap_err().to_string();
	assert!(
		error.contains("Failed to parse merged TOML configuration"),
		"got: {error}"
	);
}

#[test]
fn non_toml_files_and_subdirectories_are_ignored() {
	let tmp = tempfile::tempdir().expect("tempdir");
	write_file(tmp.path(), "config.toml", &template_toml());
	write_file(tmp.path(), "notes.txt", "not config");
	let subdir = tmp.path().join("subdir");
	std::fs::create_dir(&subdir).expect("create subdir");
	write_file(&subdir, "nested.toml", "model = \"from-subdir\"\n");
	let config = Config::load_from_path(tmp.path()).expect("directory must load");
	assert_ne!(config.model, "from-subdir", "merge must not recurse");
}

#[test]
fn load_from_path_on_a_directory_points_config_path_at_config_toml() {
	let tmp = tempfile::tempdir().expect("tempdir");
	write_file(tmp.path(), "config.toml", &template_toml());
	let config = Config::load_from_path(tmp.path()).expect("directory must load");
	assert_eq!(config.config_path, Some(tmp.path().join("config.toml")));
}

#[test]
fn update_specific_field_persists_changes_to_the_configured_path() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let path = tmp.path().join("config.toml");
	write_file(tmp.path(), "config.toml", &template_toml());
	let mut config = Config::load_from_path(&path).expect("load");
	config
		.update_specific_field(|c| c.model = "ollama:updated".to_string())
		.expect("update specific field");
	assert_eq!(config.model, "ollama:updated", "memory must see the change");
	let reloaded = Config::load_from_path(&path).expect("reload");
	assert_eq!(reloaded.model, "ollama:updated", "disk must see the change");
}

#[test]
#[serial_test::serial]
fn load_honors_the_octomind_config_path_env_override() {
	let tmp = tempfile::tempdir().expect("tempdir");
	write_file(tmp.path(), "config.toml", &template_toml());
	write_file(tmp.path(), "override.toml", "model = \"env-override\"\n");
	std::env::set_var("OCTOMIND_CONFIG_PATH", tmp.path().join("config.toml"));
	let loaded = Config::load();
	std::env::remove_var("OCTOMIND_CONFIG_PATH");
	let config = loaded.expect("load via env override");
	assert_eq!(config.model, "env-override");
}
