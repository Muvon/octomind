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
use crate::config::CURRENT_CONFIG_VERSION;

/// Backups sitting next to `config_path`. The naming scheme is octolib's, so
/// tests here only ever ask whether a backup was made, never what it's called.
fn backups(config_path: &Path) -> Vec<std::path::PathBuf> {
	let parent = config_path
		.parent()
		.expect("config path must have a parent");
	fs::read_dir(parent)
		.expect("config directory must be readable")
		.map(|entry| entry.expect("directory entry must be readable").path())
		.filter(|path| path.extension().is_some_and(|extension| extension == "bak"))
		.collect()
}

/// The Rust-side constant and the template must never disagree: the
/// constant is what the rest of the codebase compares against.
#[test]
fn template_version_matches_constant() {
	assert_eq!(
		plan().target_version(DEFAULT_CONFIG_TEMPLATE).unwrap(),
		CURRENT_CONFIG_VERSION
	);
}

#[test]
fn current_template_needs_no_migration() {
	assert!(plan()
		.migrate(DEFAULT_CONFIG_TEMPLATE, DEFAULT_CONFIG_TEMPLATE)
		.unwrap()
		.is_none());
}

#[test]
fn config_without_version_is_treated_as_v0() {
	assert_eq!(plan().version_of("log_level = \"info\"\n").unwrap(), 0);
}

#[test]
fn v0_config_gets_stamped_and_upgraded() {
	let migration = plan()
		.migrate("log_level = \"info\"\n", DEFAULT_CONFIG_TEMPLATE)
		.unwrap()
		.expect("v0 must migrate");

	assert_eq!(migration.from_version, 0);
	assert_eq!(migration.to_version, CURRENT_CONFIG_VERSION);

	let migrated: toml::Value = toml::from_str(&migration.content).unwrap();
	assert_eq!(
		migrated["version"].as_integer(),
		Some(i64::from(CURRENT_CONFIG_VERSION))
	);
	assert_eq!(migrated["log_level"].as_str(), Some("info"));
	assert_eq!(
		migrated["compression"]["analysis_findings_max_tokens"].as_integer(),
		Some(6000)
	);
	assert_eq!(
		migrated["compression"]["attention"]["enabled"].as_bool(),
		Some(false)
	);
	assert_eq!(
		migrated["compression"]["attention"]["governance"]["verify_hash"].as_bool(),
		Some(true)
	);
	assert_eq!(
		migrated["supervisor"]["model"]["max_tokens"].as_integer(),
		Some(8192)
	);
	assert!(migrated["supervisor"]["model"]["name"].as_str().is_some());
	assert!(migrated["supervisor"].get("delegate").is_none());
	assert!(migrated["supervisor"].get("detectors").is_none());
}

#[test]
fn v2_gains_attention_in_same_v3_migration_and_preserves_custom_values() {
	let existing = r#"version = 2

[compression]
hints_enabled = true

[compression.attention]
enabled = true
"#;

	let migration = plan()
		.migrate(existing, DEFAULT_CONFIG_TEMPLATE)
		.unwrap()
		.expect("v2 must migrate");
	assert_eq!(migration.to_version, CURRENT_CONFIG_VERSION);
	let migrated: toml::Value = toml::from_str(&migration.content).unwrap();
	assert_eq!(
		migrated["compression"]["attention"]["enabled"].as_bool(),
		Some(true)
	);
	assert_eq!(
		migrated["compression"]["attention"]["validator"].as_bool(),
		Some(true)
	);
	assert_eq!(
		migrated["compression"]["attention"]["governance"]["enabled"].as_bool(),
		Some(true)
	);
}

#[test]
fn partial_unreleased_attention_table_uses_safe_field_defaults() {
	let parsed: crate::config::CompressionAttentionConfig =
		toml::from_str("enabled = true\n").unwrap();
	assert!(parsed.enabled);
	assert!(parsed.validator);
	assert!(parsed.telemetry);
	assert!(parsed.governance.enabled);
	assert!(parsed.governance.verify_hash);
}

#[test]
fn v1_keeps_user_values_and_comments_and_sheds_judge_keys() {
	let existing = r#"# keep me
version = 1

[supervisor]
enabled = false
model = "openrouter:custom/model"
claim_check = true
max_consecutive_steers = 5

[supervisor.condense]
enabled = false
tokens_threshold = 1234
model = "openrouter:custom/model"

[supervisor.delegate]
enabled = false
model = "openrouter:custom/model"
max_revisions = 9
"#;

	let migration = plan()
		.migrate(existing, DEFAULT_CONFIG_TEMPLATE)
		.unwrap()
		.expect("v1 must migrate");

	assert_eq!(migration.from_version, 1);
	assert_eq!(migration.to_version, CURRENT_CONFIG_VERSION);
	assert!(migration.content.contains("# keep me"));

	let migrated: toml::Value = toml::from_str(&migration.content).unwrap();
	assert_eq!(
		migrated["version"].as_integer(),
		Some(i64::from(CURRENT_CONFIG_VERSION))
	);
	assert_eq!(migrated["supervisor"]["enabled"].as_bool(), Some(false));
	assert_eq!(
		migrated["supervisor"]["condense"]["tokens_threshold"].as_integer(),
		Some(1234)
	);
	// Dead judge keys are removed, whatever the user had set them to.
	assert!(migrated["supervisor"].get("delegate").is_none());
	assert!(migrated["supervisor"].get("claim_check").is_none());
	assert!(migrated["supervisor"]
		.get("max_consecutive_steers")
		.is_none());
}

#[test]
fn v2_gains_v3_budgets_and_keeps_existing_values() {
	let existing = r#"# keep compression notes
version = 2

[compression]
hints_enabled = false
hints_pressure_threshold = 0.8
hints_min_interval = 9
knowledge_retention = 17

[supervisor.detectors]
sequential_threshold = 3
"#;

	let migration = plan()
		.migrate(existing, DEFAULT_CONFIG_TEMPLATE)
		.unwrap()
		.expect("v2 must migrate");
	let migrated: toml::Value = toml::from_str(&migration.content).unwrap();

	assert_eq!(migration.from_version, 2);
	assert_eq!(migration.to_version, CURRENT_CONFIG_VERSION);
	assert!(migration.content.contains("# keep compression notes"));
	assert!(migrated["compression"].get("hints_enabled").is_none());
	assert!(migrated["compression"]
		.get("hints_pressure_threshold")
		.is_none());
	assert!(migrated["compression"].get("hints_min_interval").is_none());
	assert_eq!(
		migrated["compression"]["knowledge_retention"].as_integer(),
		Some(17)
	);
	assert_eq!(
		migrated["compression"]["analysis_findings_max_tokens"].as_integer(),
		Some(6000)
	);
	assert!(migrated["supervisor"].get("detectors").is_none());
}

#[test]
fn v3_collapses_pressure_levels_into_lowest_threshold() {
	let existing = r#"# keep my notes
version = 3

[compression]
hints_enabled = true

[[compression.pressure_levels]]
threshold = 80000
target_ratio = 2.0

[[compression.pressure_levels]]
threshold = 120000
target_ratio = 4.0
"#;

	let migration = plan()
		.migrate(existing, DEFAULT_CONFIG_TEMPLATE)
		.unwrap()
		.expect("v3 must migrate");

	assert_eq!(migration.from_version, 3);
	assert_eq!(migration.to_version, CURRENT_CONFIG_VERSION);
	assert!(migration.content.contains("# keep my notes"));

	let migrated: toml::Value = toml::from_str(&migration.content).unwrap();
	// The lowest level was where compression became eligible — it carries over.
	assert_eq!(
		migrated["compression"]["threshold"].as_integer(),
		Some(80000)
	);
	assert!(migrated["compression"].get("pressure_levels").is_none());
}

#[test]
fn v4_gains_v5_supervisor_fields_and_keeps_user_values() {
	let existing = r#"version = 4

[supervisor.gate]
enabled = false
max_iterations = 7
verifier_model = "openai:custom-verifier"
require_check_after_mutation = false
require_plan_complete = false

[supervisor.plan]
enabled = false
model = "openai:custom-planner"
max_tokens = 1536
trajectory_max_tokens = 3072
"#;

	let migration = plan()
		.migrate(existing, DEFAULT_CONFIG_TEMPLATE)
		.unwrap()
		.expect("v4 must migrate");
	assert_eq!(migration.from_version, 4);
	assert_eq!(migration.to_version, CURRENT_CONFIG_VERSION);

	let migrated: toml::Value = toml::from_str(&migration.content).unwrap();
	assert!(migrated["supervisor"]["gate"]
		.get("verifier_model")
		.is_none());
	assert_eq!(
		migrated["supervisor"]["plan"]["enabled"].as_bool(),
		Some(false)
	);
	assert!(migrated["supervisor"]["plan"].get("model").is_none());
	// Hardcoded budgets and auto-adoption knobs are shed by v7.
	assert!(migrated["supervisor"]["gate"]
		.get("max_iterations")
		.is_none());
	assert!(migrated["supervisor"]["plan"].get("max_tokens").is_none());
	assert!(migrated["supervisor"]["plan"]
		.get("adoption_min_actions")
		.is_none());
}

#[test]
fn v3_without_pressure_levels_takes_template_threshold() {
	let existing = "version = 3\n\n[compression]\nhints_enabled = true\n";

	let migration = plan()
		.migrate(existing, DEFAULT_CONFIG_TEMPLATE)
		.unwrap()
		.expect("v3 must migrate");

	let migrated: toml::Value = toml::from_str(&migration.content).unwrap();
	assert_eq!(
		migrated["compression"]["threshold"].as_integer(),
		Some(70000)
	);
}

#[test]
fn v5_removes_obsolete_compression_hints_and_keeps_compression_values() {
	let existing = r#"version = 5

[compression]
hints_enabled = false
hints_pressure_threshold = 0.8
hints_min_interval = 9
knowledge_retention = 17
threshold = 12345
"#;

	let migration = plan()
		.migrate(existing, DEFAULT_CONFIG_TEMPLATE)
		.unwrap()
		.expect("v5 must migrate");
	assert_eq!(migration.from_version, 5);
	assert_eq!(migration.to_version, CURRENT_CONFIG_VERSION);

	let migrated: toml::Value = toml::from_str(&migration.content).unwrap();
	let compression = migrated["compression"].as_table().unwrap();
	assert!(!compression.contains_key("hints_enabled"));
	assert!(!compression.contains_key("hints_pressure_threshold"));
	assert!(!compression.contains_key("hints_min_interval"));
	assert_eq!(compression["knowledge_retention"].as_integer(), Some(17));
	assert_eq!(compression["threshold"].as_integer(), Some(12345));
}

#[test]
fn v7_drops_compression_ignore_cost() {
	let existing = r#"version = 7

[compression]
threshold = 70000

[compression.decision]
model = "openai:gpt-5-mini"
max_tokens = 16000
ignore_cost = true
"#;
	let migration = plan()
		.migrate(existing, DEFAULT_CONFIG_TEMPLATE)
		.unwrap()
		.expect("v7 must migrate");
	assert_eq!(migration.from_version, 7);
	assert_eq!(migration.to_version, CURRENT_CONFIG_VERSION);
	let migrated: toml::Value = toml::from_str(&migration.content).unwrap();
	let model = migrated["compression"]["model"].as_table().unwrap();
	assert!(!model.contains_key("ignore_cost"));
	assert_eq!(model["max_tokens"].as_integer(), Some(16000));
	assert_eq!(
		migrated["compression"]["threshold"].as_integer(),
		Some(70000)
	);
}

#[test]
fn v8_gains_disabled_adaptive_condense_and_keeps_threshold() {
	let existing = r#"version = 8

[supervisor.condense]
enabled = true
tokens_threshold = 4321
model = "anthropic:custom"
"#;
	let migration = plan()
		.migrate(existing, DEFAULT_CONFIG_TEMPLATE)
		.unwrap()
		.expect("v8 must migrate");
	assert_eq!(migration.from_version, 8);
	assert_eq!(migration.to_version, CURRENT_CONFIG_VERSION);
	let migrated: toml::Value = toml::from_str(&migration.content).unwrap();
	let condense = &migrated["supervisor"]["condense"];
	assert_eq!(condense["adaptive"].as_bool(), Some(false));
	assert_eq!(condense["tokens_threshold"].as_integer(), Some(4321));
	assert!(condense.get("model").is_none());
}

#[test]
fn v9_removes_learning_backend_configuration_and_keeps_learning_values() {
	let existing = r#"version = 9

[supervisor.learning]
enabled = true
model = "alibaba:qwen3.8-flash"
backend = "mcp"

[supervisor.learning.store]
tool = "memorize"
[supervisor.learning.store.field_map]
content = "content"

[supervisor.learning.retrieve]
tool = "remember"
"#;
	let migration = plan()
		.migrate(existing, DEFAULT_CONFIG_TEMPLATE)
		.unwrap()
		.expect("v9 must migrate");
	let migrated: toml::Value = toml::from_str(&migration.content).unwrap();
	let learning = migrated["supervisor"]["learning"].as_table().unwrap();
	assert_eq!(learning["enabled"].as_bool(), Some(true));
	assert!(learning.get("model").is_none());
	for removed in ["backend", "store", "retrieve"] {
		assert!(!learning.contains_key(removed));
	}
}

#[test]
fn v10_adds_disabled_required_learning_evolution_table() {
	let existing = r#"version = 10

[supervisor.learning]
enabled = true
model = "openai:gpt-5-mini"
"#;
	let migration = plan()
		.migrate(existing, DEFAULT_CONFIG_TEMPLATE)
		.unwrap()
		.expect("v10 must migrate");
	assert_eq!(migration.to_version, CURRENT_CONFIG_VERSION);
	let migrated: toml::Value = toml::from_str(&migration.content).unwrap();
	assert_eq!(
		migrated["supervisor"]["learning"]["evolution"]["enabled"].as_bool(),
		Some(false)
	);
	assert!(migrated["supervisor"]["learning"].get("model").is_none());
}

#[test]
fn future_version_is_rejected_rather_than_downgraded() {
	let future = DEFAULT_CONFIG_TEMPLATE.replacen("version = 12", "version = 99", 1);
	let error = plan()
		.migrate(&future, DEFAULT_CONFIG_TEMPLATE)
		.expect_err("a newer config must not be rewritten");
	assert!(error.to_string().contains("newer than this octomind"));
}

#[test]
fn invalid_toml_fails_before_any_write() {
	assert!(plan()
		.migrate("version = 1\n[unclosed\n", DEFAULT_CONFIG_TEMPLATE)
		.is_err());
}

#[test]
fn non_integer_version_is_rejected() {
	assert!(plan()
		.migrate("version = \"1\"\n", DEFAULT_CONFIG_TEMPLATE)
		.is_err());
}

#[test]
fn upgrade_is_idempotent_and_backs_up_once() {
	let dir = std::env::temp_dir().join(format!("octomind-migration-{}", uuid::Uuid::new_v4()));
	fs::create_dir_all(&dir).unwrap();
	let config_path = dir.join("config.toml");
	let original = "version = 1\n\n[supervisor]\nenabled = true\n";
	fs::write(&config_path, original).unwrap();

	assert!(check_and_upgrade_config(&config_path).unwrap());
	let backup = match backups(&config_path).as_slice() {
		[backup] => backup.clone(),
		other => panic!("upgrade should leave exactly one backup, got {other:?}"),
	};
	assert_eq!(fs::read_to_string(&backup).unwrap(), original);

	// Second run must be a no-op: nothing to migrate, backup untouched.
	assert!(!check_and_upgrade_config(&config_path).unwrap());
	assert_eq!(backups(&config_path), vec![backup.clone()]);
	assert_eq!(fs::read_to_string(&backup).unwrap(), original);

	let migrated: toml::Value = toml::from_str(&fs::read_to_string(&config_path).unwrap()).unwrap();
	assert_eq!(
		migrated["version"].as_integer(),
		Some(i64::from(CURRENT_CONFIG_VERSION))
	);

	fs::remove_dir_all(&dir).ok();
}

#[test]
fn force_upgrade_errors_when_config_is_missing() {
	let missing =
		std::env::temp_dir().join(format!("octomind-absent-{}.toml", uuid::Uuid::new_v4()));
	assert!(force_upgrade_config(&missing).is_err());
}
// --- plan() structure -------------------------------------------------

/// The chain must be walkable from every declared version up to the
/// current one. A missing step, a non-advancing step, or an overshooting
/// step anywhere in 0..CURRENT makes one of these iterations fail, so this
/// pins the from/to sequencing without reaching into MigrationPlan's
/// private step list.
#[test]
fn every_declared_version_migrates_up_to_current() {
	for version in 0..CURRENT_CONFIG_VERSION {
		let existing = format!("version = {version}\n");
		let migration = plan()
			.migrate(&existing, DEFAULT_CONFIG_TEMPLATE)
			.unwrap_or_else(|e| panic!("version {version} must migrate: {e}"))
			.unwrap_or_else(|| panic!("version {version} must not already be current"));
		assert_eq!(migration.from_version, version);
		assert_eq!(migration.to_version, CURRENT_CONFIG_VERSION);
	}
}

#[test]
fn a_sparse_config_at_the_current_version_is_left_untouched() {
	// At the target version migrate short-circuits before any step runs,
	// so even a document missing every migrated field is not rewritten.
	assert!(plan()
		.migrate(
			&format!("version = {}\n", CURRENT_CONFIG_VERSION),
			DEFAULT_CONFIG_TEMPLATE
		)
		.unwrap()
		.is_none());
}

// --- individual steps, called directly on DocumentMut -----------------

fn template_document() -> toml_edit::DocumentMut {
	DEFAULT_CONFIG_TEMPLATE
		.parse()
		.expect("embedded template must parse")
}

fn user_document(content: &str) -> toml_edit::DocumentMut {
	content.parse().expect("test fixture must parse")
}

fn roundtrip(document: &toml_edit::DocumentMut) -> toml::Value {
	toml::from_str(&document.to_string()).expect("migrated document must be valid TOML")
}

#[test]
fn additive_steps_are_noops_on_an_already_current_document() {
	let template = template_document();
	let mut document = template.clone();
	add_delegate_gate(&mut document, &template).unwrap();
	add_v3_required_fields(&mut document, &template).unwrap();
	collapse_pressure_levels(&mut document, &template).unwrap();
	add_v5_supervisor_fields(&mut document, &template).unwrap();
	add_v9_adaptive_condense(&mut document, &template).unwrap();
	assert_eq!(document.to_string(), template.to_string());
}

#[test]
fn removal_steps_leave_an_already_current_document_untouched() {
	let template = template_document();
	let mut document = template.clone();
	remove_v6_compression_hints(&mut document, &template).unwrap();
	remove_v7_supervisor_judges(&mut document, &template).unwrap();
	remove_v8_compression_ignore_cost(&mut document, &template).unwrap();
	assert_eq!(document.to_string(), template.to_string());
}

#[test]
fn add_delegate_gate_creates_supervisor_from_template_on_empty_document() {
	let template = template_document();
	let mut document = toml_edit::DocumentMut::new();
	add_delegate_gate(&mut document, &template).unwrap();

	let supervisor = roundtrip(&document)["supervisor"].clone();
	assert!(supervisor["enabled"].as_bool().is_some());
	assert!(supervisor["gate"].is_table());
}

#[test]
fn add_delegate_gate_preserves_an_existing_supervisor_section() {
	let template = template_document();
	let mut document =
		user_document("[supervisor]\nenabled = false\nmodel = \"openrouter:custom/model\"\n");
	add_delegate_gate(&mut document, &template).unwrap();

	let supervisor = roundtrip(&document)["supervisor"].clone();
	assert_eq!(supervisor["enabled"].as_bool(), Some(false));
	assert_eq!(
		supervisor["model"].as_str(),
		Some("openrouter:custom/model")
	);
}

#[test]
fn add_v3_required_fields_fills_gaps_without_overwriting() {
	let template = template_document();
	let mut document =
		user_document("[compression]\nthreshold = 12345\nanalysis_findings_max_tokens = 99\n");
	add_v3_required_fields(&mut document, &template).unwrap();

	let compression = roundtrip(&document)["compression"].clone();
	assert_eq!(compression["threshold"].as_integer(), Some(12345));
	assert_eq!(
		compression["analysis_findings_max_tokens"].as_integer(),
		Some(99)
	);
	assert_eq!(compression["attention"]["enabled"].as_bool(), Some(false));
}

#[test]
fn add_v3_required_fields_creates_compression_whole_on_empty_document() {
	let template = template_document();
	let mut document = toml_edit::DocumentMut::new();
	add_v3_required_fields(&mut document, &template).unwrap();

	let compression = roundtrip(&document)["compression"].clone();
	assert_eq!(
		compression["analysis_findings_max_tokens"].as_integer(),
		Some(6000)
	);
	assert!(compression["attention"].is_table());
}

#[test]
fn collapse_pressure_levels_keeps_an_existing_threshold_and_drops_the_ladder() {
	let template = template_document();
	let mut document = user_document(
			"[compression]\nthreshold = 50000\n\n[[compression.pressure_levels]]\nthreshold = 80000\ntarget_ratio = 2.0\n",
		);
	collapse_pressure_levels(&mut document, &template).unwrap();

	let compression = roundtrip(&document)["compression"].clone();
	assert_eq!(compression["threshold"].as_integer(), Some(50000));
	assert!(compression.get("pressure_levels").is_none());
}

#[test]
fn collapse_pressure_levels_takes_the_lowest_threshold_wherever_it_sits() {
	let template = template_document();
	let mut document = user_document(
			"[compression]\n\n[[compression.pressure_levels]]\nthreshold = 120000\n\n[[compression.pressure_levels]]\nthreshold = 60000\n\n[[compression.pressure_levels]]\nthreshold = 90000\n",
		);
	collapse_pressure_levels(&mut document, &template).unwrap();

	let compression = roundtrip(&document)["compression"].clone();
	assert_eq!(compression["threshold"].as_integer(), Some(60000));
	assert!(compression.get("pressure_levels").is_none());
}

#[test]
fn collapse_pressure_levels_ignores_levels_without_an_integer_threshold() {
	let template = template_document();
	let mut document = user_document(
			"[compression]\n\n[[compression.pressure_levels]]\ntarget_ratio = 2.0\n\n[[compression.pressure_levels]]\nthreshold = \"high\"\n\n[[compression.pressure_levels]]\nthreshold = 95000\n",
		);
	collapse_pressure_levels(&mut document, &template).unwrap();

	let compression = roundtrip(&document)["compression"].clone();
	assert_eq!(compression["threshold"].as_integer(), Some(95000));
}

#[test]
fn collapse_pressure_levels_falls_back_to_the_template_without_usable_levels() {
	let template = template_document();
	let mut document = user_document("[compression]\nknowledge_retention = 17\n");
	collapse_pressure_levels(&mut document, &template).unwrap();

	let compression = roundtrip(&document)["compression"].clone();
	assert_eq!(compression["threshold"].as_integer(), Some(70000));
	assert_eq!(compression["knowledge_retention"].as_integer(), Some(17));
}

#[test]
fn add_v5_supervisor_fields_merges_gate_and_plan_into_a_partial_supervisor() {
	let template = template_document();
	let mut document = user_document("[supervisor]\nenabled = false\n");
	add_v5_supervisor_fields(&mut document, &template).unwrap();

	let supervisor = roundtrip(&document)["supervisor"].clone();
	assert_eq!(supervisor["enabled"].as_bool(), Some(false));
	assert_eq!(supervisor["gate"]["enabled"].as_bool(), Some(true));
	assert_eq!(supervisor["plan"]["enabled"].as_bool(), Some(true));
}

#[test]
fn add_v5_supervisor_fields_never_overwrites_custom_gate_values() {
	let template = template_document();
	let mut document = user_document(
		"[supervisor.gate]\nenabled = false\nverifier_model = \"openai:custom-verifier\"\n",
	);
	add_v5_supervisor_fields(&mut document, &template).unwrap();

	let gate = roundtrip(&document)["supervisor"]["gate"].clone();
	assert_eq!(gate["enabled"].as_bool(), Some(false));
	assert_eq!(
		gate["verifier_model"].as_str(),
		Some("openai:custom-verifier")
	);
	assert!(gate.get("max_tokens").is_none());
}

#[test]
fn remove_v6_compression_hints_drops_only_the_hint_keys() {
	let template = template_document();
	let mut document = user_document(
			"[compression]\nhints_enabled = true\nhints_pressure_threshold = 0.8\nhints_min_interval = 9\nthreshold = 70000\n",
		);
	remove_v6_compression_hints(&mut document, &template).unwrap();

	let compression = roundtrip(&document)["compression"].clone();
	let table = compression.as_table().unwrap();
	assert!(!table.contains_key("hints_enabled"));
	assert!(!table.contains_key("hints_pressure_threshold"));
	assert!(!table.contains_key("hints_min_interval"));
	assert_eq!(compression["threshold"].as_integer(), Some(70000));
}

#[test]
fn remove_v6_compression_hints_is_a_noop_without_a_compression_table() {
	let template = template_document();
	let mut document = toml_edit::DocumentMut::new();
	remove_v6_compression_hints(&mut document, &template).unwrap();
	assert!(document.as_table().is_empty());
}

#[test]
fn remove_v7_supervisor_judges_strips_dead_keys_at_every_level() {
	let template = template_document();
	let mut document = user_document(
		r#"[supervisor]
enabled = true
claim_check = true
max_consecutive_steers = 5
orientation = "strict"
recite = true

[supervisor.detectors]
sequential_threshold = 3

[supervisor.delegate]
enabled = false

[supervisor.gate]
enabled = true
verifier_model = "openai:custom-verifier"
max_tokens = 1024
max_iterations = 7
require_check_after_mutation = false
require_plan_complete = false

[supervisor.plan]
enabled = false
model = "openai:custom-planner"
max_tokens = 1536
trajectory_max_tokens = 3072
adoption_min_actions = 3
adoption_min_distinct_actions = 2

[supervisor.learning]
min_messages_for_intermediate = 5
max_inject = 4
"#,
	);
	remove_v7_supervisor_judges(&mut document, &template).unwrap();

	let supervisor = roundtrip(&document)["supervisor"].clone();
	for key in [
		"claim_check",
		"max_consecutive_steers",
		"orientation",
		"detectors",
		"recite",
		"delegate",
	] {
		assert!(supervisor.get(key).is_none(), "{key} must be removed");
	}
	assert_eq!(supervisor["enabled"].as_bool(), Some(true));

	let gate = supervisor["gate"].clone();
	assert_eq!(gate["enabled"].as_bool(), Some(true));
	assert_eq!(
		gate["verifier_model"].as_str(),
		Some("openai:custom-verifier")
	);
	assert_eq!(gate["max_tokens"].as_integer(), Some(1024));
	for key in [
		"max_iterations",
		"require_check_after_mutation",
		"require_plan_complete",
	] {
		assert!(gate.get(key).is_none(), "gate.{key} must be removed");
	}

	let plan = supervisor["plan"].clone();
	assert_eq!(plan["enabled"].as_bool(), Some(false));
	assert_eq!(plan["model"].as_str(), Some("openai:custom-planner"));
	for key in [
		"max_tokens",
		"trajectory_max_tokens",
		"adoption_min_actions",
		"adoption_min_distinct_actions",
	] {
		assert!(plan.get(key).is_none(), "plan.{key} must be removed");
	}

	let learning = supervisor["learning"].as_table().unwrap();
	assert!(learning.is_empty(), "both learning knobs are dead keys");
}

#[test]
fn remove_v7_supervisor_judges_is_a_noop_without_supervisor() {
	let template = template_document();
	let mut document = user_document("[compression]\nthreshold = 70000\n");
	remove_v7_supervisor_judges(&mut document, &template).unwrap();
	assert!(document.as_table().contains_key("compression"));
	assert!(!document.as_table().contains_key("supervisor"));
}

#[test]
fn remove_v8_compression_ignore_cost_drops_only_that_key() {
	let template = template_document();
	let mut document = user_document(
			"[compression.decision]\nmodel = \"openai:gpt-5-mini\"\nmax_tokens = 16000\nignore_cost = true\n",
		);
	remove_v8_compression_ignore_cost(&mut document, &template).unwrap();

	let decision = roundtrip(&document)["compression"]["decision"].clone();
	assert!(!decision.as_table().unwrap().contains_key("ignore_cost"));
	assert_eq!(decision["model"].as_str(), Some("openai:gpt-5-mini"));
	assert_eq!(decision["max_tokens"].as_integer(), Some(16000));
}

#[test]
fn remove_v8_compression_ignore_cost_tolerates_missing_or_malformed_sections() {
	let template = template_document();

	// no [compression] table at all
	let mut document = toml_edit::DocumentMut::new();
	remove_v8_compression_ignore_cost(&mut document, &template).unwrap();
	assert!(document.as_table().is_empty());

	// [compression] without a decision table
	let mut document = user_document("[compression]\nthreshold = 70000\n");
	remove_v8_compression_ignore_cost(&mut document, &template).unwrap();
	assert_eq!(
		roundtrip(&document)["compression"]["threshold"].as_integer(),
		Some(70000)
	);

	// compression present but not a table — left exactly as-is
	let mut document = user_document("compression = 5\n");
	remove_v8_compression_ignore_cost(&mut document, &template).unwrap();
	assert_eq!(roundtrip(&document)["compression"].as_integer(), Some(5));
}

#[test]
fn add_v9_adaptive_condense_adds_the_switch_without_touching_existing_values() {
	let template = template_document();
	let mut document = user_document(
			"[supervisor.condense]\nenabled = true\ntokens_threshold = 4321\nmodel = \"anthropic:custom\"\n",
		);
	add_v9_adaptive_condense(&mut document, &template).unwrap();

	let condense = roundtrip(&document)["supervisor"]["condense"].clone();
	assert_eq!(condense["adaptive"].as_bool(), Some(false));
	assert_eq!(condense["enabled"].as_bool(), Some(true));
	assert_eq!(condense["tokens_threshold"].as_integer(), Some(4321));
	assert_eq!(condense["model"].as_str(), Some("anthropic:custom"));
}

#[test]
fn add_v9_adaptive_condense_never_overwrites_an_existing_adaptive_value() {
	let template = template_document();
	let mut document =
		user_document("[supervisor.condense]\nadaptive = true\ntokens_threshold = 1000\n");
	add_v9_adaptive_condense(&mut document, &template).unwrap();
	// Running the step twice must stay a no-op for user-set values.
	add_v9_adaptive_condense(&mut document, &template).unwrap();

	let condense = roundtrip(&document)["supervisor"]["condense"].clone();
	assert_eq!(condense["adaptive"].as_bool(), Some(true));
}

#[test]
fn add_v9_adaptive_condense_builds_the_full_chain_on_an_empty_document() {
	let template = template_document();
	let mut document = toml_edit::DocumentMut::new();
	add_v9_adaptive_condense(&mut document, &template).unwrap();

	let condense = roundtrip(&document)["supervisor"]["condense"].clone();
	assert_eq!(condense["adaptive"].as_bool(), Some(false));
	assert!(condense["tokens_threshold"].as_integer().is_some());
}

#[test]
fn remove_v10_learning_backends_tolerates_missing_sections() {
	let template = template_document();
	let mut empty = toml_edit::DocumentMut::new();
	remove_v10_learning_backends(&mut empty, &template).unwrap();
	assert!(empty.as_table().is_empty());

	let mut document = user_document(
		"[supervisor.learning]\nenabled = true\nbackend = \"mcp\"\nstore = \"bad\"\n",
	);
	remove_v10_learning_backends(&mut document, &template).unwrap();
	let learning = roundtrip(&document)["supervisor"]["learning"]
		.as_table()
		.unwrap()
		.clone();
	assert_eq!(learning["enabled"].as_bool(), Some(true));
	assert!(!learning.contains_key("backend"));
	assert!(!learning.contains_key("store"));
}
