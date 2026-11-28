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

//! Chain-level and file-level migration tests complementing the inline unit
//! tests: idempotency of already-migrated output, preservation of unknown
//! user keys across the whole version chain, and the CLI entry points
//! (`check_and_upgrade_config` / `force_upgrade_config`) against real files.

use super::*;
use crate::config::{CURRENT_CONFIG_VERSION, DEPRECATED_SAMPLING_KEYS};

/// The whole upgrade, as `check_and_upgrade_config` runs it.
fn migrate_once(content: &str) -> String {
	upgrade(content)
		.expect("fixture must parse")
		.expect("fixture must not already be current")
		.migration
		.content
}

fn parse(content: &str) -> toml::Value {
	toml::from_str(content).expect("migrated output must be valid TOML")
}

fn user_document(content: &str) -> toml_edit::DocumentMut {
	content.parse().expect("test fixture must parse")
}

fn template_document() -> toml_edit::DocumentMut {
	DEFAULT_CONFIG_TEMPLATE.parse().expect("embedded template must parse")
}

#[test]
fn version_of_reports_the_embedded_template_as_current() {
	assert_eq!(
		plan().version_of(DEFAULT_CONFIG_TEMPLATE).unwrap(),
		CURRENT_CONFIG_VERSION
	);
}

#[test]
fn the_default_template_needs_no_upgrade() {
	assert!(upgrade(DEFAULT_CONFIG_TEMPLATE).unwrap().is_none());
}

#[test]
fn migrating_the_output_of_a_migration_is_a_noop() {
	for fixture in [
		"log_level = \"info\"\n",
		"version = 0\n",
		"version = 3\n\n[[compression.pressure_levels]]\nthreshold = 80000\n",
		"version = 9\n\n[supervisor.learning]\nbackend = \"mcp\"\n",
	] {
		let once = migrate_once(fixture);
		assert!(
			upgrade(&once).unwrap().is_none(),
			"migrated output must already be current, got: {once}"
		);
	}
}

#[test]
fn every_version_chain_output_parses_and_keeps_unknown_user_keys() {
	for version in 0..CURRENT_CONFIG_VERSION {
		let existing = format!("version = {version}\n\n[my_custom_section]\nflag = true\n");
		let migrated = parse(&migrate_once(&existing));
		assert_eq!(
			migrated["version"].as_integer(),
			Some(i64::from(CURRENT_CONFIG_VERSION)),
			"version {version} must be stamped up to current"
		);
		assert_eq!(
			migrated["my_custom_section"]["flag"].as_bool(),
			Some(true),
			"version {version} must preserve unknown user sections"
		);
	}
}

#[test]
fn every_start_version_migrates_to_a_current_config_without_the_supervisor_tables() {
	let current = plan().target_version(DEFAULT_CONFIG_TEMPLATE).unwrap();
	for version in 0..current {
		let migrated = migrate_once(&format!("version = {version}\n"));
		let value = parse(&migrated);
		assert_eq!(
			value["version"].as_integer(),
			Some(i64::from(current)),
			"version {version}"
		);
		for table in SUPERVISOR_TABLES {
			assert!(value.get(table).is_none(), "version {version} must lose [{table}]");
		}
		assert!(
			upgrade(&migrated).unwrap().is_none(),
			"version {version}: a second upgrade must find nothing to do"
		);
	}
}

#[test]
fn an_explicit_version_zero_follows_the_same_chain_as_a_missing_stamp() {
	let migration = upgrade("version = 0\n")
		.unwrap()
		.expect("explicit v0 must migrate")
		.migration;
	assert_eq!(migration.from_version, 0);
	assert_eq!(migration.to_version, CURRENT_CONFIG_VERSION);
}

#[test]
fn v11_model_settings_migrate_to_three_nested_ownership_profiles() {
	let old = r#"version = 11
model = "openai:main"
max_tokens = 32000
temperature = 0.4
top_p = 0.8
top_k = 40
max_retries = 2
retry_timeout = 7
request_timeout_seconds = 90
reasoning_effort = "high"

[[roles]]
name = "worker"
model = "anthropic:worker"
temperature = 0.2
top_p = 0.9
top_k = 10
system = "work"
welcome = ""

[supervisor]
enabled = true
model = "google:supervisor"
[supervisor.learning]
enabled = true
model = "openai:old-learning"
[supervisor.learning.evolution]
enabled = false
[supervisor.gate]
enabled = true
verifier_model = "openai:old-verifier"
max_tokens = 1234
[supervisor.plan]
enabled = true
model = "openai:old-plan"
[supervisor.condense]
enabled = true
adaptive = false
tokens_threshold = 5000
model = "openai:old-condense"

[compression]
threshold = 70000
knowledge_retention = 25
analysis_findings_max_tokens = 6000
[compression.decision]
model = "openai:compression"
max_tokens = 16000
temperature = 0.3
top_p = 1.0
top_k = 0
max_retries = 1
retry_timeout = 30
"#;
	let migrated = parse(&migrate_once(old));
	assert_eq!(migrated["model"]["name"].as_str(), Some("openai:main"));
	assert_eq!(migrated["model"]["reasoning_effort"].as_str(), Some("high"));
	assert_eq!(migrated["roles"][0]["model"]["name"].as_str(), Some("anthropic:worker"));
	assert_eq!(
		migrated["compression"]["model"]["name"].as_str(),
		Some("openai:compression")
	);
	assert!(migrated.get("supervisor").is_none());
}

#[test]
fn v34_removes_sampling_from_model_profiles_and_roles_without_disturbing_comments() {
	let old = r#"version = 33

[model]
name = "openai:main" # keep main
temperature = 0.4
top_p = 0.8
top_k = 40
max_tokens = 1234 # keep root field

[compression.model]
name = "openai:compression" # keep compression
temperature = 0.3
top_p = 1.0
top_k = 0
max_tokens = 5678 # keep compression field

[[roles]]
name = "flat"
temperature = 0.2
top_p = 0.9
top_k = 10
system = "flat role" # keep flat role
welcome = ""

[[roles]]
name = "nested"
system = "nested role" # keep nested role
welcome = ""

[roles.model]
name = "anthropic:worker" # keep role model
temperature = 0.1
top_p = 0.7
top_k = 5
max_retries = 2 # keep nested field
"#;
	let migrated = migrate_once(old);
	let value = parse(&migrated);

	for key in DEPRECATED_SAMPLING_KEYS {
		assert!(value["model"].get(key).is_none());
		assert!(value["compression"]["model"].get(key).is_none());
		assert!(value["roles"][0].get(key).is_none());
		assert!(value["roles"][1]["model"].get(key).is_none());
	}
	assert_eq!(value["model"]["max_tokens"].as_integer(), Some(1234));
	assert_eq!(value["compression"]["model"]["max_tokens"].as_integer(), Some(5678));
	assert_eq!(value["roles"][0]["system"].as_str(), Some("flat role"));
	assert_eq!(value["roles"][1]["model"]["max_retries"].as_integer(), Some(2));
	for comment in [
		"# keep main",
		"# keep root field",
		"# keep compression",
		"# keep compression field",
		"# keep flat role",
		"# keep nested role",
		"# keep role model",
		"# keep nested field",
	] {
		assert!(migrated.contains(comment), "missing {comment}");
	}
}

#[test]
fn v34_keeps_sampling_in_unrelated_model_tables() {
	let old = r#"version = 33

[custom.model]
temperature = 25 # keep unrelated sampling
label = "custom" # keep unrelated field
"#;
	let migrated = migrate_once(old);
	let value = parse(&migrated);

	assert_eq!(value["custom"]["model"]["temperature"].as_integer(), Some(25));
	assert_eq!(value["custom"]["model"]["label"].as_str(), Some("custom"));
	assert!(migrated.contains("# keep unrelated sampling"));
	assert!(migrated.contains("# keep unrelated field"));
}

#[test]
fn v34_removes_sampling_from_inline_roles() {
	let old = concat!(
		"version = 33\n",
		"roles = [{ name = \"legacy\", system = \"x\", welcome = \"\", ",
		"temperature = 0.3, top_p = 0.8, top_k = 20, ",
		"model = { name = \"openai:worker\", temperature = 0.2, top_p = 0.7, ",
		"top_k = 10, max_tokens = 1234 } }]\n",
	);
	let value = parse(&migrate_once(old));
	let role = &value["roles"][0];

	for key in DEPRECATED_SAMPLING_KEYS {
		assert!(role.get(key).is_none());
		assert!(role["model"].get(key).is_none());
	}
	assert_eq!(role["name"].as_str(), Some("legacy"));
	assert_eq!(role["system"].as_str(), Some("x"));
	assert_eq!(role["welcome"].as_str(), Some(""));
	assert_eq!(role["model"]["name"].as_str(), Some("openai:worker"));
	assert_eq!(role["model"]["max_tokens"].as_integer(), Some(1234));
}

#[test]
fn v34_removes_sampling_from_inline_root_and_compression_models() {
	let old = concat!(
		"version = 33\n",
		"model = { name = \"openai:main\", temperature = 0.4, top_p = 0.5, top_k = 40, max_tokens = 1234 }\n",
		"compression = { model = { name = \"openai:compression\", temperature = 0.3, ",
		"top_p = 0.6, top_k = 20, max_tokens = 5678 }, threshold = 0.9 }\n",
	);
	let value = parse(&migrate_once(old));

	for key in DEPRECATED_SAMPLING_KEYS {
		assert!(value["model"].get(key).is_none());
		assert!(value["compression"]["model"].get(key).is_none());
	}
	assert_eq!(value["model"]["name"].as_str(), Some("openai:main"));
	assert_eq!(value["model"]["max_tokens"].as_integer(), Some(1234));
	assert_eq!(
		value["compression"]["model"]["name"].as_str(),
		Some("openai:compression")
	);
	assert_eq!(value["compression"]["model"]["max_tokens"].as_integer(), Some(5678));
	assert_eq!(value["compression"]["threshold"].as_float(), Some(0.9));
}

#[test]
fn collapse_pressure_levels_carries_a_single_level_threshold() {
	let template = template_document();
	let mut document =
		user_document("[compression]\n\n[[compression.pressure_levels]]\nthreshold = 45000\ntarget_ratio = 2.0\n");
	collapse_pressure_levels(&mut document, &template).unwrap();
	let compression = parse(&document.to_string())["compression"].clone();
	assert_eq!(compression["threshold"].as_integer(), Some(45000));
	assert!(compression.get("pressure_levels").is_none());
}

fn temp_config(content: &str) -> (std::path::PathBuf, tempfile::TempDir) {
	let dir = tempfile::tempdir().expect("tempdir");
	let path = dir.path().join("config.toml");
	fs::write(&path, content).expect("write fixture");
	(path, dir)
}

fn bak_files(dir: &Path) -> Vec<std::path::PathBuf> {
	fs::read_dir(dir)
		.expect("config directory must be readable")
		.map(|entry| entry.expect("directory entry must be readable").path())
		.filter(|path| path.extension().is_some_and(|extension| extension == "bak"))
		.collect()
}

#[test]
fn check_and_upgrade_leaves_an_up_to_date_file_untouched() {
	let (path, dir) = temp_config(DEFAULT_CONFIG_TEMPLATE);
	let before = fs::read_to_string(&path).unwrap();
	assert!(!check_and_upgrade_config(&path).unwrap());
	assert_eq!(fs::read_to_string(&path).unwrap(), before);
	assert!(
		bak_files(dir.path()).is_empty(),
		"an up-to-date config must not be backed up"
	);
	assert!(
		!dir.path().join(CARRY_DIR_NAME).exists(),
		"an up-to-date config must not write carried settings"
	);
}

#[test]
fn check_and_upgrade_errors_on_a_missing_file() {
	let missing = std::env::temp_dir().join(format!("octomind-absent-{}.toml", uuid::Uuid::new_v4()));
	assert!(check_and_upgrade_config(&missing).is_err());
}

#[test]
fn force_upgrade_stamps_an_outdated_file_and_is_quiet_on_current_ones() {
	let (path, dir) = temp_config("version = 2\n\n[supervisor]\nenabled = true\n");
	force_upgrade_config(&path).expect("force upgrade must succeed");

	let upgraded = parse(&fs::read_to_string(&path).unwrap());
	assert_eq!(
		upgraded["version"].as_integer(),
		Some(i64::from(CURRENT_CONFIG_VERSION))
	);
	assert!(upgraded.get("supervisor").is_none());
	assert_eq!(upgraded["plugins"][0]["name"].as_str(), Some(DEFAULT_PLUGIN_NAME));
	assert!(upgraded["plugins"][0].get("enabled").is_none());
	assert_eq!(bak_files(dir.path()).len(), 1, "exactly one backup expected");

	let carried = fs::read_to_string(dir.path().join(CARRY_DIR_NAME).join(CARRY_FILE_NAME)).unwrap();
	assert_eq!(parse(&carried)["supervisor"]["enabled"].as_bool(), Some(true));

	// A second force run reports success without rewriting the file.
	let stamped = fs::read_to_string(&path).unwrap();
	force_upgrade_config(&path).expect("already-current file must report success");
	assert_eq!(fs::read_to_string(&path).unwrap(), stamped);
	assert_eq!(bak_files(dir.path()).len(), 1, "no second backup expected");
}

#[test]
fn v19_timing_migration_adds_the_section_and_keeps_calibrated_values() {
	let mut document = template_document();
	document["version"] = toml_edit::value(19);
	let timing = document["timing"].as_table_mut().unwrap();
	timing.remove("wait_weight");
	timing["review_weight"] = toml_edit::value(3.0);
	let migrated = migrate_once(&document.to_string());
	let config: crate::config::Config = toml::from_str(&migrated).unwrap();
	assert_eq!(config.version, CURRENT_CONFIG_VERSION);
	assert_eq!(config.timing.wait_weight, 0.25);
	assert_eq!(config.timing.review_weight, 3.0);

	document.remove("timing");
	let config: crate::config::Config = toml::from_str(&migrate_once(&document.to_string())).unwrap();
	assert_eq!(config.timing.review_weight, 2.0);
}

const V31_WITH_SUPERVISOR_TABLES: &str = "# my config\nversion = 31\nlog_level = \"info\"\n\n# my decision notes\n[decision]\nmodel = \"typesafe:jev-latest\" # my model\nrecall = true\n\n# my supervisor notes\n[supervisor]\nenabled = false # off for now\nmax_identical_rounds = 4\n\n[my_custom_section]\nflag = true\n";

#[test]
fn carried_settings_of_a_v31_config_hold_both_tables_with_the_users_values_and_comments() {
	let carried = upgrade(V31_WITH_SUPERVISOR_TABLES)
		.unwrap()
		.expect("a version 31 config must upgrade")
		.carried_supervisor_settings
		.expect("both tables must be carried");

	assert!(carried.starts_with(CARRY_FILE_HEADER));
	let value = parse(&carried);
	assert_eq!(value["decision"]["model"].as_str(), Some("typesafe:jev-latest"));
	assert_eq!(value["decision"]["recall"].as_bool(), Some(true));
	assert_eq!(value["supervisor"]["enabled"].as_bool(), Some(false));
	assert_eq!(value["supervisor"]["max_identical_rounds"].as_integer(), Some(4));
	assert!(value.get("my_custom_section").is_none());
	for comment in [
		"# my decision notes",
		"# my model",
		"# my supervisor notes",
		"# off for now",
	] {
		assert!(carried.contains(comment), "{comment} must be carried");
	}
}

#[test]
fn carried_settings_of_an_older_config_are_the_users_tables_as_they_are_in_the_file() {
	let upgraded = upgrade("version = 9\n\n[supervisor.learning]\nenabled = true\nbackend = \"mcp\"\n")
		.unwrap()
		.expect("a version 9 config must upgrade");
	assert_eq!(upgraded.migration.from_version, 9);
	let carried = upgraded
		.carried_supervisor_settings
		.expect("the supervisor table must be carried");

	assert!(carried.starts_with(CARRY_FILE_HEADER));
	let value = parse(&carried);
	let learning = &value["supervisor"]["learning"];
	assert_eq!(learning["enabled"].as_bool(), Some(true));
	assert_eq!(
		learning["backend"].as_str(),
		Some("mcp"),
		"no step is applied to the carried tables"
	);
	assert!(learning.get("evolution").is_none());
	assert!(value.get("decision").is_none());
	assert!(value.get("version").is_none());
}

#[test]
fn a_v31_config_without_the_supervisor_tables_carries_nothing() {
	let upgraded = upgrade("version = 31\nlog_level = \"info\"\n\n[my_custom_section]\nflag = true\n")
		.unwrap()
		.expect("a version 31 config must upgrade");
	assert!(upgraded.carried_supervisor_settings.is_none());
}

#[cfg(unix)]
#[test]
fn a_failed_sibling_rewrite_leaves_the_config_at_its_old_version_so_the_upgrade_runs_again() {
	use std::os::unix::fs::PermissionsExt;
	let old_config = "version = 34\ndefault = \"content:editor\"\n";
	let (path, dir) = temp_config(old_config);
	let sibling = dir.path().join("mcp-extra.toml");
	fs::write(&sibling, "auto_bind = [\"content:editor\"]\n").unwrap();
	fs::set_permissions(&sibling, fs::Permissions::from_mode(0o444)).unwrap();
	if fs::OpenOptions::new().write(true).open(&sibling).is_ok() {
		return; // running as root: the file cannot be made unwritable
	}

	assert!(check_and_upgrade_config(&path).is_err());
	assert_eq!(fs::read_to_string(&path).unwrap(), old_config, "main file untouched");

	fs::set_permissions(&sibling, fs::Permissions::from_mode(0o644)).unwrap();
	assert!(check_and_upgrade_config(&path).unwrap());
	assert_eq!(
		parse(&fs::read_to_string(&path).unwrap())["default"].as_str(),
		Some("writer")
	);
	assert!(fs::read_to_string(&sibling).unwrap().contains("writer"));
}

#[test]
fn a_config_already_at_the_current_version_gives_no_upgrade_and_no_carried_settings() {
	let stamped = format!("version = {CURRENT_CONFIG_VERSION}\n\n[supervisor]\nenabled = true\n");
	assert!(upgrade(&stamped).unwrap().is_none());
}

#[test]
fn check_and_upgrade_saves_the_supervisor_tables_and_upgrades_a_v31_file_once() {
	let (path, dir) = temp_config(V31_WITH_SUPERVISOR_TABLES);
	assert!(check_and_upgrade_config(&path).unwrap());

	let upgraded = parse(&fs::read_to_string(&path).unwrap());
	assert_eq!(
		upgraded["version"].as_integer(),
		Some(i64::from(CURRENT_CONFIG_VERSION))
	);
	assert!(upgraded.get("supervisor").is_none());
	assert!(upgraded.get("decision").is_none());
	assert_eq!(upgraded["max_identical_rounds"].as_integer(), Some(4));
	assert_eq!(upgraded["plugins"][0]["enabled"].as_bool(), Some(false));
	assert_eq!(bak_files(dir.path()).len(), 1, "exactly one backup expected");

	let carry_path = dir.path().join(CARRY_DIR_NAME).join(CARRY_FILE_NAME);
	let carried = fs::read_to_string(&carry_path).unwrap();
	assert!(carried.starts_with(CARRY_FILE_HEADER));
	let carried_value = parse(&carried);
	assert_eq!(carried_value["decision"]["recall"].as_bool(), Some(true));
	assert_eq!(carried_value["supervisor"]["enabled"].as_bool(), Some(false));

	let config_after_first_run = fs::read_to_string(&path).unwrap();
	assert!(!check_and_upgrade_config(&path).unwrap());
	assert_eq!(fs::read_to_string(&path).unwrap(), config_after_first_run);
	assert_eq!(fs::read_to_string(&carry_path).unwrap(), carried);
	assert_eq!(bak_files(dir.path()).len(), 1, "no second backup expected");
}

#[test]
fn check_and_upgrade_writes_no_carry_file_when_the_config_had_no_supervisor_tables() {
	let (path, dir) = temp_config("version = 31\nlog_level = \"info\"\n");
	assert!(check_and_upgrade_config(&path).unwrap());
	assert!(!dir.path().join(CARRY_DIR_NAME).exists());
	assert_eq!(bak_files(dir.path()).len(), 1);
}
