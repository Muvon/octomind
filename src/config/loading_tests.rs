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
	write_file(tmp.path(), "a-first.toml", "model = \"ollama:from-a\"\n");
	let config = Config::load_from_path(tmp.path()).expect("directory must load");
	assert_eq!(config.model, "ollama:from-a");
}

#[test]
fn regular_files_merge_in_alphabetical_order() {
	let tmp = tempfile::tempdir().expect("tempdir");
	write_file(tmp.path(), "config.toml", &template_toml());
	write_file(tmp.path(), "a.toml", "model = \"ollama:a\"\n");
	write_file(tmp.path(), "z.toml", "model = \"ollama:z\"\n");
	let config = Config::load_from_path(tmp.path()).expect("directory must load");
	assert_eq!(config.model, "ollama:z");
}

#[test]
fn mcp_extension_files_load_after_every_regular_file() {
	// "mcp-a.toml" sorts before "z.toml" alphabetically, but the documented
	// contract loads mcp-*.toml overrides last, so its field must win.
	let tmp = tempfile::tempdir().expect("tempdir");
	write_file(tmp.path(), "config.toml", &template_toml());
	write_file(tmp.path(), "z.toml", "model = \"ollama:z\"\n");
	write_file(tmp.path(), "mcp-a.toml", "model = \"ollama:mcp-a\"\n");
	let config = Config::load_from_path(tmp.path()).expect("directory must load");
	assert_eq!(config.model, "ollama:mcp-a");
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
		"[compression.model]\nmax_tokens = 999\n",
	);
	let config = Config::load_from_path(tmp.path()).expect("directory must load");
	assert_eq!(config.get_compression_model_profile().max_tokens, 999);
	assert!(
		!config.get_compression_model_profile().model.is_empty(),
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
	write_file(
		tmp.path(),
		"override.toml",
		"model = \"ollama:env-override\"\n",
	);
	std::env::set_var("OCTOMIND_CONFIG_PATH", tmp.path().join("config.toml"));
	let loaded = Config::load();
	std::env::remove_var("OCTOMIND_CONFIG_PATH");
	let config = loaded.expect("load via env override");
	assert_eq!(config.model, "ollama:env-override");
}

// ---------------------------------------------------------------------------
// merge_toml_values: an override table may introduce keys the base lacks.
// ---------------------------------------------------------------------------

#[test]
fn deep_merge_introduces_keys_the_base_table_lacks() {
	let base: toml::Value = toml::from_str("[a]\nx = 1\n").expect("base");
	let over: toml::Value = toml::from_str("[a]\ny = 2\n").expect("override");
	let merged = merge_toml_values(&base, &over);
	assert_eq!(merged["a"]["x"], toml::Value::Integer(1));
	assert_eq!(merged["a"]["y"], toml::Value::Integer(2));
}

#[test]
fn duplicate_server_names_across_files_keep_the_last_definition() {
	let tmp = tempfile::tempdir().expect("tempdir");
	write_file(tmp.path(), "config.toml", &template_toml());
	// zz- sorts after config.toml, so this definition is the later one.
	write_file(
		tmp.path(),
		"zz-override.toml",
		"[[mcp.servers]]\nname = \"core\"\ntype = \"builtin\"\ntools = []\ntimeout_seconds = 99\n",
	);
	let config = Config::load_from_path(tmp.path()).expect("duplicate-name files still load");
	let cores: Vec<_> = config
		.mcp
		.servers
		.iter()
		.filter(|s| s.name() == "core")
		.collect();
	assert_eq!(cores.len(), 1, "same-name servers dedup to one entry");
	assert_eq!(
		cores[0].timeout_seconds(),
		99,
		"the later file's definition wins"
	);
}

// ---------------------------------------------------------------------------
// Config::load(): first-run bootstrap and env override, serial because they
// mutate OCTOMIND_CONFIG_PATH.
// ---------------------------------------------------------------------------

#[test]
#[serial_test::serial]
fn load_injects_the_default_template_when_the_directory_is_missing() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let path = tmp.path().join("fresh/nested/config.toml");
	std::env::set_var("OCTOMIND_CONFIG_PATH", &path);
	let config = Config::load();
	std::env::remove_var("OCTOMIND_CONFIG_PATH");

	let config = config.expect("missing directory bootstraps a default config");
	assert!(!config.model.is_empty());
	assert!(
		path.exists(),
		"the bootstrap writes the template for the next run"
	);
}

#[test]
#[serial_test::serial]
fn load_injects_the_default_template_in_an_empty_directory() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let path = tmp.path().join("config.toml");
	std::env::set_var("OCTOMIND_CONFIG_PATH", &path);
	let config = Config::load();
	std::env::remove_var("OCTOMIND_CONFIG_PATH");

	let config = config.expect("empty directory bootstraps a default config");
	assert!(!config.model.is_empty());
	assert!(path.exists());
}

#[test]
#[serial_test::serial]
fn load_uses_sibling_toml_files_when_config_toml_is_absent() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let customized = template_toml().replace("max_tokens = 32768", "max_tokens = 1234");
	assert!(
		customized.contains("max_tokens = 1234"),
		"the template anchor was replaced"
	);
	write_file(tmp.path(), "other.toml", &customized);
	let path = tmp.path().join("config.toml");
	std::env::set_var("OCTOMIND_CONFIG_PATH", &path);
	let config = Config::load();
	std::env::remove_var("OCTOMIND_CONFIG_PATH");

	let config = config.expect("sibling toml files are merged when config.toml is absent");
	assert_eq!(config.max_tokens, 1234);
}

#[test]
#[serial_test::serial]
fn load_honors_a_relative_single_component_config_path() {
	// A bare filename has parent "" — it must resolve to the current
	// directory, not be treated as a missing directory and overwritten.
	let name = format!("loadtest-{}.toml", std::process::id());
	std::fs::write(&name, template_toml()).expect("write relative config");
	std::env::set_var("OCTOMIND_CONFIG_PATH", &name);
	let config = Config::load();
	std::env::remove_var("OCTOMIND_CONFIG_PATH");
	let _ = std::fs::remove_file(&name);

	let config = config.expect("relative path loads from the current directory");
	assert!(!config.model.is_empty());
}

// ---------------------------------------------------------------------------
// Setters: save / update_and_save / update_specific_field.
// ---------------------------------------------------------------------------

#[test]
fn save_writes_to_the_stored_path_and_creates_parents() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let mut config = Config::load_from_path(&{
		let p = tmp.path().join("config.toml");
		std::fs::write(&p, template_toml()).expect("write template");
		p
	})
	.expect("load template");
	let out = tmp.path().join("made/up/dir/config.toml");
	config.config_path = Some(out.clone());
	config.save().expect("save creates parents and writes");
	assert!(out.exists());
}

#[test]
fn update_and_save_persists_the_mutation() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let path = tmp.path().join("config.toml");
	std::fs::write(&path, template_toml()).expect("write template");
	let mut config = Config::load_from_path(&path).expect("load");
	config
		.update_and_save(|c| c.supervisor.enabled = true)
		.expect("update and save");
	let on_disk = std::fs::read_to_string(&path).expect("read back");
	assert!(
		on_disk.contains("enabled = true"),
		"the mutation reached the file"
	);
	assert!(
		config.supervisor.enabled,
		"the in-memory copy is updated too"
	);
}

#[test]
fn update_specific_field_fails_strictly_without_a_config_file() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let mut config = Config::load_from_path(&{
		let p = tmp.path().join("config.toml");
		std::fs::write(&p, template_toml()).expect("write template");
		p
	})
	.expect("load");
	config.config_path = Some(tmp.path().join("absent.toml"));
	let error = config
		.update_specific_field(|c| c.supervisor.enabled = true)
		.expect_err("strict mode refuses a missing file");
	assert!(error.to_string().contains("No configuration file found"));
}

// ---------------------------------------------------------------------------
// merge_agent_toml: additive servers/roles, override elsewhere.
// ---------------------------------------------------------------------------

#[test]
fn merge_agent_toml_adds_servers_and_roles_and_overrides_scalars() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let base_path = tmp.path().join("config.toml");
	std::fs::write(&base_path, template_toml()).expect("write template");
	let base = Config::load_from_path(&base_path).expect("load base");
	let base_model = base.model.clone();
	let base_server_count = base.mcp.servers.len();
	let base_role_count = base.roles.len();

	let agent_toml = r#"
model = "agent-override-model"

[[mcp.servers]]
name = "agent-extra-server"
type = "builtin"
tools = []
timeout_seconds = 30

[[roles]]
name = "agent-extra-role"
model = "ollama:agent"
temperature = 0.3
top_p = 0.7
top_k = 20
system = "agent extra role system prompt"
welcome = "agent extra role welcome"
"#;
	let merged = merge_agent_toml(&base, agent_toml).expect("merge agent manifest");

	assert_eq!(merged.model, "agent-override-model", "scalars override");
	assert_eq!(
		merged.mcp.servers.len(),
		base_server_count + 1,
		"servers concatenate"
	);
	assert!(
		merged
			.mcp
			.servers
			.iter()
			.any(|s| s.name() == "agent-extra-server"),
		"the agent's server is present"
	);
	assert_eq!(merged.roles.len(), base_role_count + 1, "roles concatenate");
	assert!(
		merged.roles.iter().any(|r| r.name == "agent-extra-role"),
		"the agent's role is present"
	);
	assert_eq!(base.model, base_model, "the base config is not mutated");
}

#[test]
fn merge_agent_toml_skips_servers_and_roles_the_base_already_has() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let base_path = tmp.path().join("config.toml");
	std::fs::write(&base_path, template_toml()).expect("write template");
	let base = Config::load_from_path(&base_path).expect("load base");
	let existing_server = base.mcp.servers[0].name().to_string();
	let existing_role = base.roles[0].name.clone();
	let servers = base.mcp.servers.len();
	let roles = base.roles.len();

	let agent_toml = format!(
		r#"
[[mcp.servers]]
name = "{existing_server}"
type = "builtin"
tools = []

[[roles]]
name = "{existing_role}"
model = "ollama:dup"
"#
	);
	let merged = merge_agent_toml(&base, &agent_toml).expect("merge");
	assert_eq!(
		merged.mcp.servers.len(),
		servers,
		"a same-named server is not duplicated"
	);
	assert_eq!(
		merged.roles.len(),
		roles,
		"a same-named role is not duplicated"
	);
}
