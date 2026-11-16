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
use crate::config::CURRENT_CONFIG_VERSION;

/// The whole upgrade, as `check_and_upgrade_config` runs it.
fn migrate_once(content: &str) -> String {
	upgrade(content)
		.expect("fixture must parse")
		.expect("fixture must not already be current")
		.migration
		.content
}

/// Only the supervisor-era steps: the output is a version 31 configuration.
fn migrate_to_v31(content: &str) -> String {
	supervisor_era_plan()
		.migrate(content, V31_CONFIG_TEMPLATE)
		.expect("fixture must parse")
		.expect("fixture must not already be at version 31")
		.content
}

fn assert_is_settled_at_v31(migrated: &str) {
	assert_eq!(
		parse(migrated)["version"].as_integer(),
		Some(i64::from(SUPERVISOR_ERA_LAST_VERSION))
	);
	assert!(supervisor_era_plan()
		.migrate(migrated, V31_CONFIG_TEMPLATE)
		.unwrap()
		.is_none());
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

fn v31_template_document() -> toml_edit::DocumentMut {
	V31_CONFIG_TEMPLATE.parse().expect("version 31 template must parse")
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
fn v12_authorizer_migration_defaults_off_and_preserves_explicit_choice() {
	for choice in [None, Some(true), Some(false)] {
		let mut document = v31_template_document();
		document["version"] = toml_edit::value(12);
		let supervisor = document["supervisor"].as_table_mut().unwrap();
		if let Some(enabled) = choice {
			supervisor["authorizer"]["enabled"] = toml_edit::value(enabled);
		} else {
			supervisor.remove("authorizer");
		}
		let migrated = migrate_to_v31(&document.to_string());
		assert_eq!(
			parse(&migrated)["supervisor"]["authorizer"]["enabled"].as_bool(),
			Some(choice.unwrap_or(false))
		);
		assert_is_settled_at_v31(&migrated);
	}
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
	for version in 0..=SUPERVISOR_ERA_LAST_VERSION {
		let migrated = migrate_once(&format!("version = {version}\n"));
		let value = parse(&migrated);
		assert_eq!(
			value["version"].as_integer(),
			Some(i64::from(CURRENT_CONFIG_VERSION)),
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
	let migrated = parse(&migrate_to_v31(old));
	assert_eq!(migrated["model"]["name"].as_str(), Some("openai:main"));
	assert_eq!(migrated["model"]["reasoning_effort"].as_str(), Some("high"));
	assert_eq!(migrated["roles"][0]["model"]["name"].as_str(), Some("anthropic:worker"));
	assert_eq!(
		migrated["supervisor"]["model"]["name"].as_str(),
		Some("google:supervisor")
	);
	assert_eq!(
		migrated["compression"]["model"]["name"].as_str(),
		Some("openai:compression")
	);
	assert!(migrated["supervisor"]["learning"].get("model").is_none());
	assert!(migrated["supervisor"]["gate"].get("verifier_model").is_none());
	assert!(migrated["supervisor"]["plan"].get("model").is_none());
	assert!(migrated["supervisor"]["condense"].get("model").is_none());
}

#[test]
fn an_empty_document_migrates_to_the_template_supervisor_defaults() {
	let migrated = parse(&migrate_to_v31(""));
	assert_eq!(
		migrated["version"].as_integer(),
		Some(i64::from(SUPERVISOR_ERA_LAST_VERSION))
	);
	assert_eq!(migrated["supervisor"]["model"]["max_tokens"].as_integer(), Some(8192));
}

#[test]
fn remove_v10_learning_backends_tolerates_a_non_table_learning_value() {
	let template = v31_template_document();
	let mut document = user_document("[supervisor]\nenabled = true\nlearning = 5\n");
	remove_v10_learning_backends(&mut document, &template).unwrap();
	let supervisor = parse(&document.to_string())["supervisor"].clone();
	assert_eq!(supervisor["enabled"].as_bool(), Some(true));
	assert_eq!(supervisor["learning"].as_integer(), Some(5));
}

#[test]
fn add_v9_adaptive_condense_creates_condense_inside_an_existing_supervisor() {
	let template = v31_template_document();
	let mut document = user_document("[supervisor]\nenabled = true\n");
	add_v9_adaptive_condense(&mut document, &template).unwrap();
	let supervisor = parse(&document.to_string())["supervisor"].clone();
	assert_eq!(supervisor["enabled"].as_bool(), Some(true));
	assert_eq!(supervisor["condense"]["adaptive"].as_bool(), Some(false));
}

#[test]
fn collapse_pressure_levels_carries_a_single_level_threshold() {
	let template = v31_template_document();
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

#[test]
fn v13_evaluate_migration_inserts_seams_off_and_preserves_supervisor_values() {
	let mut document = v31_template_document();
	document["version"] = toml_edit::value(13);
	document.remove("decision");
	let supervisor = document["supervisor"].as_table_mut().unwrap();
	supervisor.remove("evaluate");
	supervisor["enabled"] = toml_edit::value(true);
	supervisor["model"]["name"] = toml_edit::value("openai:custom-supervisor");
	supervisor["authorizer"]["enabled"] = toml_edit::value(true);
	let migrated = migrate_to_v31(&document.to_string());
	let value = parse(&migrated);
	assert_is_settled_at_v31(&migrated);
	let decision = &value["decision"];
	assert_eq!(decision["model"].as_str(), Some("cloudflare:typesafe/jev"));
	for key in ["recall", "skills", "supervisor"] {
		assert_eq!(decision[key].as_bool(), Some(false), "decision.{key}");
	}
	assert_eq!(value["supervisor"]["enabled"].as_bool(), Some(true));
	assert_eq!(
		value["supervisor"]["model"]["name"].as_str(),
		Some("openai:custom-supervisor")
	);
	assert_eq!(value["supervisor"]["authorizer"]["enabled"].as_bool(), Some(true));
}

#[test]
fn v14_evaluate_seams_migration_inserts_condense_and_compression_off_and_preserves_evaluate_values() {
	let mut document = v31_template_document();
	document["version"] = toml_edit::value(14);
	document.remove("decision");
	document["supervisor"]["evaluate"] = toml_edit::Item::Table(toml_edit::Table::new());
	let evaluate = document["supervisor"]["evaluate"].as_table_mut().unwrap();
	evaluate.remove("condense");
	evaluate.remove("compression");
	evaluate["model"] = toml_edit::value("typesafe:jev-latest");
	evaluate["recall"] = toml_edit::value(true);
	evaluate["authorizer"] = toml_edit::value(true);
	let migrated = migrate_to_v31(&document.to_string());
	let value = parse(&migrated);
	assert_is_settled_at_v31(&migrated);
	let decision = &value["decision"];
	assert_eq!(decision["compression"].as_bool(), Some(false));
	assert_eq!(decision["model"].as_str(), Some("typesafe:jev-latest"));
	assert_eq!(decision["recall"].as_bool(), Some(true));
	assert_eq!(decision["skills"].as_bool(), Some(false));
	assert_eq!(decision["supervisor"].as_bool(), Some(true));
}

#[test]
fn v15_evaluate_seams_migration_inserts_distill_plan_and_gate_off_and_preserves_evaluate_values() {
	let mut document = v31_template_document();
	document["version"] = toml_edit::value(15);
	document.remove("decision");
	document["supervisor"]["evaluate"] = toml_edit::Item::Table(toml_edit::Table::new());
	let evaluate = document["supervisor"]["evaluate"].as_table_mut().unwrap();
	evaluate.remove("distill");
	evaluate.remove("plan");
	evaluate.remove("gate");
	evaluate["model"] = toml_edit::value("typesafe:jev-latest");
	evaluate["recall"] = toml_edit::value(true);
	evaluate["condense"] = toml_edit::value(true);
	let migrated = migrate_to_v31(&document.to_string());
	let value = parse(&migrated);
	assert_is_settled_at_v31(&migrated);
	let decision = &value["decision"];
	assert_eq!(decision["model"].as_str(), Some("typesafe:jev-latest"));
	assert_eq!(decision["recall"].as_bool(), Some(true));
	assert_eq!(decision["supervisor"].as_bool(), Some(true));
	assert_eq!(decision["skills"].as_bool(), Some(false));
	assert_eq!(decision["compression"].as_bool(), Some(false));
}

#[test]
fn v16_evaluate_seams_migration_inserts_capabilities_off_and_preserves_evaluate_values() {
	let mut document = v31_template_document();
	document["version"] = toml_edit::value(16);
	document.remove("decision");
	document["supervisor"]["evaluate"] = toml_edit::Item::Table(toml_edit::Table::new());
	let evaluate = document["supervisor"]["evaluate"].as_table_mut().unwrap();
	evaluate.remove("capabilities");
	evaluate["skills"] = toml_edit::value(true);
	evaluate["gate"] = toml_edit::value(true);
	let migrated = migrate_to_v31(&document.to_string());
	let value = parse(&migrated);
	assert_is_settled_at_v31(&migrated);
	let decision = &value["decision"];
	assert_eq!(decision["capabilities"].as_bool(), Some(false));
	assert_eq!(decision["skills"].as_bool(), Some(true));
	assert_eq!(decision["supervisor"].as_bool(), Some(true));
	assert_eq!(decision["recall"].as_bool(), Some(false));
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
fn carried_settings_of_an_older_config_are_the_tables_as_they_are_after_the_steps_to_31() {
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
	assert!(learning.get("backend").is_none(), "step 10 removes the backend");
	assert_eq!(learning["evolution"]["enabled"].as_bool(), Some(false));
	assert!(value.get("decision").is_some(), "step 22 adds the decision table");
	assert!(value.get("version").is_none());
}

#[test]
fn a_v31_config_without_the_supervisor_tables_carries_nothing() {
	let upgraded = upgrade("version = 31\nlog_level = \"info\"\n\n[my_custom_section]\nflag = true\n")
		.unwrap()
		.expect("a version 31 config must upgrade");
	assert!(upgraded.carried_supervisor_settings.is_none());
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
