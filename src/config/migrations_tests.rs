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

fn migrate_once(content: &str) -> String {
	plan()
		.migrate(content, DEFAULT_CONFIG_TEMPLATE)
		.expect("fixture must parse")
		.expect("fixture must not already be current")
		.content
}

fn parse(content: &str) -> toml::Value {
	toml::from_str(content).expect("migrated output must be valid TOML")
}

fn user_document(content: &str) -> toml_edit::DocumentMut {
	content.parse().expect("test fixture must parse")
}

fn template_document() -> toml_edit::DocumentMut {
	DEFAULT_CONFIG_TEMPLATE
		.parse()
		.expect("embedded template must parse")
}

#[test]
fn version_of_reports_the_embedded_template_as_current() {
	assert_eq!(
		plan().version_of(DEFAULT_CONFIG_TEMPLATE).unwrap(),
		CURRENT_CONFIG_VERSION
	);
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
			plan()
				.migrate(&once, DEFAULT_CONFIG_TEMPLATE)
				.unwrap()
				.is_none(),
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
fn an_explicit_version_zero_follows_the_same_chain_as_a_missing_stamp() {
	let migration = plan()
		.migrate("version = 0\n", DEFAULT_CONFIG_TEMPLATE)
		.unwrap()
		.expect("explicit v0 must migrate");
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
	assert_eq!(migrated["supervisor"]["model"]["name"].as_str(), Some("google:supervisor"));
	assert_eq!(migrated["compression"]["model"]["name"].as_str(), Some("openai:compression"));
	assert!(migrated["supervisor"]["learning"].get("model").is_none());
	assert!(migrated["supervisor"]["gate"].get("verifier_model").is_none());
	assert!(migrated["supervisor"]["plan"].get("model").is_none());
	assert!(migrated["supervisor"]["condense"].get("model").is_none());
}

#[test]
fn an_empty_document_migrates_to_the_template_supervisor_defaults() {
	let migrated = parse(&migrate_once(""));
	assert_eq!(
		migrated["version"].as_integer(),
		Some(i64::from(CURRENT_CONFIG_VERSION))
	);
	assert_eq!(
		migrated["supervisor"]["model"]["max_tokens"].as_integer(),
		Some(8192)
	);
}

#[test]
fn remove_v10_learning_backends_tolerates_a_non_table_learning_value() {
	let template = template_document();
	let mut document = user_document("[supervisor]\nenabled = true\nlearning = 5\n");
	remove_v10_learning_backends(&mut document, &template).unwrap();
	let supervisor = parse(&document.to_string())["supervisor"].clone();
	assert_eq!(supervisor["enabled"].as_bool(), Some(true));
	assert_eq!(supervisor["learning"].as_integer(), Some(5));
}

#[test]
fn add_v9_adaptive_condense_creates_condense_inside_an_existing_supervisor() {
	let template = template_document();
	let mut document = user_document("[supervisor]\nenabled = true\n");
	add_v9_adaptive_condense(&mut document, &template).unwrap();
	let supervisor = parse(&document.to_string())["supervisor"].clone();
	assert_eq!(supervisor["enabled"].as_bool(), Some(true));
	assert_eq!(supervisor["condense"]["adaptive"].as_bool(), Some(false));
}

#[test]
fn collapse_pressure_levels_carries_a_single_level_threshold() {
	let template = template_document();
	let mut document = user_document(
		"[compression]\n\n[[compression.pressure_levels]]\nthreshold = 45000\ntarget_ratio = 2.0\n",
	);
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
}

#[test]
fn check_and_upgrade_errors_on_a_missing_file() {
	let missing =
		std::env::temp_dir().join(format!("octomind-absent-{}.toml", uuid::Uuid::new_v4()));
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
	assert_eq!(upgraded["supervisor"]["enabled"].as_bool(), Some(true));
	assert_eq!(
		bak_files(dir.path()).len(),
		1,
		"exactly one backup expected"
	);

	// A second force run reports success without rewriting the file.
	let stamped = fs::read_to_string(&path).unwrap();
	force_upgrade_config(&path).expect("already-current file must report success");
	assert_eq!(fs::read_to_string(&path).unwrap(), stamped);
	assert_eq!(bak_files(dir.path()).len(), 1, "no second backup expected");
}
