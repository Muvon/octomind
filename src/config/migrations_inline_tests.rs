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
	let parent = config_path.parent().expect("config path must have a parent");
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
	assert_eq!(migrated["compression"]["attention"]["enabled"].as_bool(), Some(false));
	assert_eq!(
		migrated["compression"]["attention"]["governance"]["verify_hash"].as_bool(),
		Some(true)
	);
	assert!(migrated.get("supervisor").is_none());
	assert!(migrated.get("decision").is_none());
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
	assert_eq!(migrated["compression"]["attention"]["enabled"].as_bool(), Some(true));
	assert_eq!(migrated["compression"]["attention"]["validator"].as_bool(), Some(true));
	assert_eq!(
		migrated["compression"]["attention"]["governance"]["enabled"].as_bool(),
		Some(true)
	);
}

#[test]
fn partial_unreleased_attention_table_uses_safe_field_defaults() {
	let parsed: crate::config::CompressionAttentionConfig = toml::from_str("enabled = true\n").unwrap();
	assert!(parsed.enabled);
	assert!(parsed.validator);
	assert!(parsed.telemetry);
	assert!(parsed.governance.enabled);
	assert!(parsed.governance.verify_hash);
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
	assert!(migrated["compression"].get("hints_pressure_threshold").is_none());
	assert!(migrated["compression"].get("hints_min_interval").is_none());
	assert_eq!(migrated["compression"]["knowledge_retention"].as_integer(), Some(17));
	assert_eq!(
		migrated["compression"]["analysis_findings_max_tokens"].as_integer(),
		Some(6000)
	);
	assert!(migrated.get("supervisor").is_none());
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
	assert_eq!(migrated["compression"]["threshold"].as_integer(), Some(80000));
	assert!(migrated["compression"].get("pressure_levels").is_none());
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
		migrated["compression"]["threshold"].as_integer().unwrap(),
		template_document()["compression"]["threshold"].as_integer().unwrap()
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
	assert_eq!(migrated["compression"]["threshold"].as_integer(), Some(70000));
}

#[test]
fn future_version_is_rejected_rather_than_downgraded() {
	let future = DEFAULT_CONFIG_TEMPLATE.replacen(&format!("version = {CURRENT_CONFIG_VERSION}"), "version = 99", 1);
	let error = upgrade(&future).err().expect("a newer config must not be rewritten");
	assert!(error.to_string().contains("newer than this octomind"));
}

#[test]
fn invalid_toml_fails_before_any_write() {
	assert!(upgrade("version = 1\n[unclosed\n").is_err());
}

#[test]
fn non_integer_version_is_rejected() {
	assert!(upgrade("version = \"1\"\n").is_err());
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
	let missing = std::env::temp_dir().join(format!("octomind-absent-{}.toml", uuid::Uuid::new_v4()));
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
		let migration = upgrade(&existing)
			.unwrap_or_else(|e| panic!("version {version} must migrate: {e}"))
			.unwrap_or_else(|| panic!("version {version} must not already be current"))
			.migration;
		assert_eq!(migration.from_version, version);
		assert_eq!(migration.to_version, CURRENT_CONFIG_VERSION);
	}
}

#[test]
fn a_sparse_config_at_the_current_version_is_left_untouched() {
	// At the target version migrate short-circuits before any step runs,
	// so even a document missing every migrated field is not rewritten.
	assert!(upgrade(&format!("version = {}\n", CURRENT_CONFIG_VERSION))
		.unwrap()
		.is_none());
}

// --- individual steps, called directly on DocumentMut -----------------

fn template_document() -> toml_edit::DocumentMut {
	DEFAULT_CONFIG_TEMPLATE.parse().expect("embedded template must parse")
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
	add_v3_required_fields(&mut document, &template).unwrap();
	collapse_pressure_levels(&mut document, &template).unwrap();
	assert_eq!(document.to_string(), template.to_string());
}

#[test]
fn removal_steps_leave_an_already_current_document_untouched() {
	let template = template_document();
	let mut document = template.clone();
	remove_v6_compression_hints(&mut document, &template).unwrap();
	remove_v8_compression_ignore_cost(&mut document, &template).unwrap();
	assert_eq!(document.to_string(), template.to_string());
}

#[test]
fn add_v3_required_fields_fills_gaps_without_overwriting() {
	let template = template_document();
	let mut document = user_document("[compression]\nthreshold = 12345\nanalysis_findings_max_tokens = 99\n");
	add_v3_required_fields(&mut document, &template).unwrap();

	let compression = roundtrip(&document)["compression"].clone();
	assert_eq!(compression["threshold"].as_integer(), Some(12345));
	assert_eq!(compression["analysis_findings_max_tokens"].as_integer(), Some(99));
	assert_eq!(compression["attention"]["enabled"].as_bool(), Some(false));
}

#[test]
fn add_v3_required_fields_creates_compression_whole_on_empty_document() {
	let template = template_document();
	let mut document = toml_edit::DocumentMut::new();
	add_v3_required_fields(&mut document, &template).unwrap();

	let compression = roundtrip(&document)["compression"].clone();
	assert_eq!(compression["analysis_findings_max_tokens"].as_integer(), Some(6000));
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
	assert_eq!(
		compression["threshold"].as_integer().unwrap(),
		template["compression"]["threshold"].as_integer().unwrap()
	);
	assert_eq!(compression["knowledge_retention"].as_integer(), Some(17));
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
fn v27_gains_idle_compaction_on_and_keeps_an_explicit_choice() {
	let migrate = |existing: &str| -> toml::Value {
		let migration = plan()
			.migrate(existing, DEFAULT_CONFIG_TEMPLATE)
			.unwrap()
			.expect("v27 must migrate");
		assert_eq!(migration.from_version, 27);
		assert_eq!(migration.to_version, CURRENT_CONFIG_VERSION);
		toml::from_str(&migration.content).unwrap()
	};

	let gained = migrate("version = 27\n\n[compression]\nthreshold = 90000\n");
	assert_eq!(gained["compression"]["idle"].as_bool(), Some(true));
	assert_eq!(gained["compression"]["threshold"].as_integer(), Some(90000));

	let kept = migrate("version = 27\n\n[compression]\nthreshold = 90000\nidle = false\n");
	assert_eq!(kept["compression"]["idle"].as_bool(), Some(false));
}

#[test]
fn v23_adds_commit_attribution_and_keeps_an_existing_value() {
	let migration = plan()
		.migrate("version = 22\n", DEFAULT_CONFIG_TEMPLATE)
		.unwrap()
		.unwrap();
	let value: toml::Value = toml::from_str(&migration.content).unwrap();
	assert_eq!(migration.from_version, 22);
	assert_eq!(
		value["commit_attribution"].as_str(),
		Some("Co-Authored-By: Octomind <345424151+octomindat3am@users.noreply.github.com>")
	);

	let migration = plan()
		.migrate("version = 22\ncommit_attribution = \"\"\n", DEFAULT_CONFIG_TEMPLATE)
		.unwrap()
		.unwrap();
	let value: toml::Value = toml::from_str(&migration.content).unwrap();
	assert_eq!(value["commit_attribution"].as_str(), Some(""));
}

#[test]
fn v30_renames_old_agent_tags_and_v35_then_rewrites_them_to_the_current_agents() {
	let input = "version = 29\ndefault = \"developer:cto\" # lead\nlog_level = \"info\"\n\n[taps]\n\"developer:cto\" = \"openai:gpt-5\"\n\"travel:planner\" = \"openai:gpt-4o\"\n\"assistant:concierge\" = \"openai:gpt-4.1\"\ndoctor = \"openai:o3-mini\"\n\"acme:reviewer\" = \"openai:o3\"\n";
	let migration = plan().migrate(input, DEFAULT_CONFIG_TEMPLATE).unwrap().unwrap();
	let value: toml::Value = toml::from_str(&migration.content).unwrap();
	assert_eq!(migration.from_version, 29);
	assert_eq!(migration.to_version, CURRENT_CONFIG_VERSION);
	assert_eq!(value["default"].as_str(), Some("developer"));
	assert!(migration.content.contains("default = \"developer\" # lead"));
	let taps = value["taps"].as_table().unwrap();
	assert_eq!(taps.len(), 5);
	assert_eq!(taps["developer"].as_str(), Some("openai:gpt-5"));
	assert_eq!(taps["travel-agent"].as_str(), Some("openai:gpt-4o"));
	assert_eq!(taps["assistant"].as_str(), Some("openai:gpt-4.1"));
	assert_eq!(taps["doctor"].as_str(), Some("openai:o3-mini"));
	assert_eq!(taps["acme:reviewer"].as_str(), Some("openai:o3"), "a private tag stays");
	assert_eq!(value["log_level"].as_str(), Some("info"));
}

#[test]
fn v30_keeps_the_new_taps_key_when_both_old_and_new_exist() {
	let mut document = user_document("[taps]\n\"video:producer\" = \"old-model\"\n\"video:chief\" = \"new-model\"\n");
	rename_v30_chief_tags(&mut document, &template_document()).unwrap();
	let value = roundtrip(&document);
	let taps = value["taps"].as_table().unwrap();
	assert_eq!(taps.len(), 1);
	assert_eq!(taps["video:chief"].as_str(), Some("new-model"));
}

#[test]
fn v30_leaves_a_config_without_old_tags_unchanged() {
	let input = "default = \"assistant:general\"\n\n# models\n[taps]\n\"developer:general\" = \"openai:o3\"\n";
	let mut document = user_document(input);
	rename_v30_chief_tags(&mut document, &template_document()).unwrap();
	assert_eq!(document.to_string(), input);
}

fn migrate_v35(existing: &str) -> toml::Value {
	let migration = plan().migrate(existing, DEFAULT_CONFIG_TEMPLATE).unwrap().unwrap();
	assert_eq!(migration.from_version, 34);
	assert_eq!(migration.to_version, CURRENT_CONFIG_VERSION);
	toml::from_str(&migration.content).unwrap()
}

#[test]
fn v35_rewrites_retired_tags_in_the_default_and_the_taps_keys() {
	let input = "version = 34\ndefault = \"content:editor\" # lead\n\n[taps]\n\"home:realty\" = \"openai:o3\"\n\"doctor:blood\" = \"openai:gpt-4o\"\n\"acme:reviewer\" = \"openai:gpt-4.1\"\nassistant = \"openai:gpt-4.1-mini\"\n";
	let migration = plan().migrate(input, DEFAULT_CONFIG_TEMPLATE).unwrap().unwrap();
	let value: toml::Value = toml::from_str(&migration.content).unwrap();
	assert_eq!(value["default"].as_str(), Some("writer"));
	assert!(migration.content.contains("default = \"writer\" # lead"));
	let taps = value["taps"].as_table().unwrap();
	assert_eq!(taps.len(), 4);
	assert_eq!(taps["financial-planner"].as_str(), Some("openai:o3"));
	assert_eq!(taps["doctor"].as_str(), Some("openai:gpt-4o"));
	assert_eq!(taps["acme:reviewer"].as_str(), Some("openai:gpt-4.1"));
	assert_eq!(taps["assistant"].as_str(), Some("openai:gpt-4.1-mini"));
}

#[test]
fn v35_leaves_a_tag_that_is_not_retired_as_written() {
	let input = "version = 34\ndefault = \"acme:reviewer\"\n\n[taps]\n\"developer:rust\" = \"openai:o3\"\n";
	let value = migrate_v35(input);
	assert_eq!(value["default"].as_str(), Some("acme:reviewer"));
	assert_eq!(value["taps"]["developer:rust"].as_str(), Some("openai:o3"));
}

#[test]
fn v35_collapses_retired_taps_keys_of_one_agent_by_existing_key_then_file_order() {
	let value = migrate_v35(concat!(
		"version = 34\n[taps]\n",
		"\"academic:writer\" = \"first\"\n\"academic:chief\" = \"second\"\nacademic = \"existing\"\n",
		"\"coach:resume\" = \"first\"\n\"coach:career\" = \"second\"\n",
	));
	let taps = value["taps"].as_table().unwrap();
	assert_eq!(taps.len(), 2);
	assert_eq!(taps["academic"].as_str(), Some("existing"));
	assert_eq!(taps["coach"].as_str(), Some("first"));
}

#[test]
fn v35_leaves_names_that_are_not_retired_alone() {
	let input = "version = 34\ndefault = \"assistant\"\n\n# models\n[taps]\ndeveloper = \"openai:o3\"\n";
	let migration = plan().migrate(input, DEFAULT_CONFIG_TEMPLATE).unwrap().unwrap();
	assert!(migration.content.starts_with(&format!(
		"version = {CURRENT_CONFIG_VERSION}\ndefault = \"assistant\"\n"
	)));
	assert!(migration
		.content
		.contains("# models\n[taps]\ndeveloper = \"openai:o3\"\n"));
}

const EXAMPLE_ASSISTANT_ROLE_TOML: &str = r#"
# my copy of the example
[[roles]]
name   = "assistant"
system = """
You are helpful and knowledgeable assistant.
Working directory: {{CWD}}
"""
welcome = "Hello! Ready to code. Working in {{CWD}} (Role: {{ROLE}})"

[roles.mcp]
server_refs = ["core","orchestration","runtime","filesystem","agent"]
allowed_tools = ["core:*", "orchestration:*", "runtime:*", "filesystem:*", "agent:*"]
"#;

fn role_names(value: &toml::Value) -> Vec<String> {
	value
		.get("roles")
		.and_then(toml::Value::as_array)
		.map(|roles| {
			roles
				.iter()
				.filter_map(|role| role["name"].as_str().map(str::to_string))
				.collect()
		})
		.unwrap_or_default()
}

#[test]
fn v35_rewrites_plugin_role_filters_and_server_auto_bind_without_duplicates() {
	let input = concat!(
		"version = 34\n",
		"plugins = [{ name = \"p\", type = \"stdio\", roles = [\"developer:general\", \"developer:brief\", \"doctor\", \"acme:reviewer\"] }]\n",
		"[mcp]\ncode_mode = true\n",
		"[[mcp.servers]]\nname = \"s\"\ntype = \"stdio\"\ncommand = \"c\"\nauto_bind = [\"coach:career\", \"coach\", \"acme:reviewer\"]\n",
	);
	let value = migrate_v35(input);
	let roles = value["plugins"][0]["roles"].as_array().unwrap();
	let roles: Vec<&str> = roles.iter().filter_map(toml::Value::as_str).collect();
	assert_eq!(roles, ["developer", "doctor", "acme:reviewer"]);
	let bound = value["mcp"]["servers"][0]["auto_bind"].as_array().unwrap();
	let bound: Vec<&str> = bound.iter().filter_map(toml::Value::as_str).collect();
	assert_eq!(bound, ["coach", "acme:reviewer"]);
}

#[test]
fn v35_notices_name_each_rewritten_tag_and_the_keys_it_dropped() {
	let mut document = user_document(concat!(
		"default = \"content:editor\"\n[taps]\n\"academic:writer\" = \"first\"\n\"academic:chief\" = \"second\"\n",
		"\"coach:resume\" = \"same\"\n\"coach:career\" = \"same\"\n",
	));
	let notices = rewrite_v35_retired_agent_tags(&mut document).unwrap();
	assert!(
		notices.contains(&"Agent tag 'content:editor' is now 'writer'.".to_string()),
		"{notices:?}"
	);
	assert!(
		notices.contains(&"Agent tag 'academic:chief' is now 'academic'.".to_string()),
		"{notices:?}"
	);
	let dropped: Vec<&String> = notices.iter().filter(|notice| notice.contains("dropped")).collect();
	assert_eq!(dropped.len(), 1, "keys with equal values lose nothing: {notices:?}");
	assert!(dropped[0].contains("kept 'academic' = \"first\""), "{}", dropped[0]);
	assert!(dropped[0].contains("'academic:chief'"), "{}", dropped[0]);
}

#[test]
fn every_retired_tag_row_is_a_two_part_tag_and_a_current_name() {
	let rows: Vec<(&str, &str)> = RETIRED_AGENT_TAGS
		.lines()
		.filter_map(|row| row.split_once('\t'))
		.collect();
	assert_eq!(rows.len(), RETIRED_AGENT_TAGS.lines().count());
	assert_eq!(rows.len(), 203);
	for (retired, current) in rows {
		assert_eq!(retired.matches(':').count(), 1, "{retired}");
		assert!(!current.contains(':') && !current.is_empty(), "{current}");
		assert_eq!(current_agent_tag(retired), Some(current));
	}
}

#[test]
fn v35_adds_the_registry_fields_the_config_requires() {
	let value = migrate_v35("version = 34\n[registry]\ncache_ttl_hours = 5\n");
	assert_eq!(value["registry"]["cache_ttl_hours"].as_integer(), Some(5));
	assert_eq!(value["registry"]["tap"].as_str(), Some("muvon/tap"));
	assert!(value["registry"]["ref"].as_str().is_some());
	let value = migrate_v35("version = 34\n");
	assert_eq!(value["registry"]["cache_ttl_hours"].as_integer(), Some(24));
}

#[test]
fn v35_removes_the_untouched_example_assistant_role_whatever_its_layout() {
	let input = format!("version = 34\n{EXAMPLE_ASSISTANT_ROLE_TOML}\n[[roles]]\nname = \"mine\"\nsystem = \"x\"\n");
	let value = migrate_v35(&input);
	assert_eq!(role_names(&value), ["mine"]);
}

#[test]
fn v35_removes_an_older_shipped_form_of_the_example_role() {
	let input = concat!(
		"version = 34\n[[roles]]\nname = \"assistant\"\n",
		"system = \"You are helpful and knowledgeable assistant.\\nWorking directory: {{CWD}}\\n\"\n",
		"welcome = \"Hello! Ready to code. Working in {{CWD}} (Role: {{ROLE}})\"\n",
		"[roles.mcp]\nserver_refs = [\"core\", \"filesystem\", \"agent\"]\n",
		"allowed_tools = [\"core:*\", \"filesystem:*\", \"agent:*\"]\n",
	);
	let value = migrate_v35(input);
	assert!(role_names(&value).is_empty());
	assert!(value.get("roles").is_none(), "an emptied list is dropped");
}

#[test]
fn v35_keeps_an_assistant_role_the_user_changed() {
	let input = format!(
		"version = 34\n{}",
		EXAMPLE_ASSISTANT_ROLE_TOML.replace("helpful and knowledgeable", "terse")
	);
	let value = migrate_v35(&input);
	assert_eq!(role_names(&value), ["assistant"]);
	assert!(value["roles"][0]["system"].as_str().unwrap().contains("terse"));
}

#[test]
fn v35_leaves_a_config_without_an_assistant_role_alone() {
	let input = "version = 34\n\n[[roles]]\nname = \"mine\"\nsystem = \"x\"\n";
	let value = migrate_v35(input);
	assert_eq!(role_names(&value), ["mine"]);
	let value = migrate_v35("version = 34\n");
	assert!(value.get("roles").is_none());
}

#[test]
fn v26_adds_disabled_stream_timeout_and_preserves_explicit_limits() {
	for stream_setting in [None, Some(0), Some(90)] {
		let mut document = template_document();
		document["version"] = toml_edit::value(25);
		document["model"]["request_timeout_seconds"] = toml_edit::value(600);
		match stream_setting {
			Some(value) => document["model"]["stream_timeout_seconds"] = toml_edit::value(value),
			None => {
				document["model"]
					.as_table_mut()
					.unwrap()
					.remove("stream_timeout_seconds");
			}
		}
		let migration = upgrade(&document.to_string()).unwrap().unwrap().migration;
		let config: crate::config::Config = toml::from_str(&migration.content).unwrap();
		assert_eq!(config.version, CURRENT_CONFIG_VERSION);
		assert_eq!(config.request_timeout_seconds, 600);
		assert_eq!(config.stream_timeout_seconds, stream_setting.unwrap_or(0) as u64);
	}
}

#[test]
fn v32_removes_the_supervisor_and_decision_tables_and_keeps_everything_else() {
	let existing = "# my config\nversion = 31\nlog_level = \"info\" # quiet\n\n[decision]\nrecall = true\n\n[supervisor]\nenabled = true\n\n# keep this table\n[my_custom_section]\nflag = true\n";
	let migration = plan().migrate(existing, DEFAULT_CONFIG_TEMPLATE).unwrap().unwrap();
	let value: toml::Value = toml::from_str(&migration.content).unwrap();

	assert_eq!(migration.from_version, 31);
	assert_eq!(migration.to_version, CURRENT_CONFIG_VERSION);
	assert_eq!(value["version"].as_integer(), Some(i64::from(CURRENT_CONFIG_VERSION)));
	assert!(value.get("supervisor").is_none());
	assert!(value.get("decision").is_none());
	assert_eq!(value["log_level"].as_str(), Some("info"));
	assert_eq!(value["my_custom_section"]["flag"].as_bool(), Some(true));
	for comment in ["# my config", "# quiet", "# keep this table"] {
		assert!(migration.content.contains(comment), "{comment} must survive");
	}
}

/// Version 31 -> 32 on its own, against the current template.
fn migrate_v32(existing: &str) -> toml::Value {
	let migration = plan()
		.migrate(existing, DEFAULT_CONFIG_TEMPLATE)
		.unwrap()
		.expect("a version 31 fixture must migrate");
	toml::from_str(&migration.content).unwrap()
}

fn template_max_identical_rounds() -> i64 {
	toml::from_str::<toml::Value>(DEFAULT_CONFIG_TEMPLATE).unwrap()["max_identical_rounds"]
		.as_integer()
		.unwrap()
}

/// The `besydr` entry of the default plugin list, as the step must add it.
fn default_plugin_value() -> toml::Value {
	let defaults: toml::Value = toml::from_str(&format!("plugins = {DEFAULT_PLUGINS_TOML}\n")).unwrap();
	defaults["plugins"][0].clone()
}

#[test]
fn v32_moves_the_users_max_identical_rounds_to_the_root() {
	for (existing, expected) in [
		("version = 31\n\n[supervisor]\nmax_identical_rounds = 4\n", 4),
		("version = 31\n\n[supervisor]\nmax_identical_rounds = 0\n", 0),
	] {
		let value = migrate_v32(existing);
		assert_eq!(value["max_identical_rounds"].as_integer(), Some(expected), "{existing}");
		assert!(value.get("supervisor").is_none());
	}
}

#[test]
fn v32_gives_max_identical_rounds_the_template_value_when_the_user_had_none() {
	for existing in ["version = 31\n", "version = 31\n\n[supervisor]\nenabled = true\n"] {
		let value = migrate_v32(existing);
		assert_eq!(
			value["max_identical_rounds"].as_integer(),
			Some(template_max_identical_rounds()),
			"{existing}"
		);
	}
}

#[test]
fn v32_adds_the_default_plugin_entry_when_plugins_is_absent() {
	let value = migrate_v32("version = 31\n");
	assert_eq!(value["plugins"], toml::Value::Array(vec![default_plugin_value()]));
}

#[test]
fn v32_appends_the_default_plugin_entry_to_an_inline_plugins_array() {
	let existing = "version = 31\nplugins = [{ name = \"mine\", type = \"stdio\", command = \"prog\" }]\n";
	let mine = toml::from_str::<toml::Value>(existing).unwrap()["plugins"][0].clone();
	let value = migrate_v32(existing);
	assert_eq!(value["plugins"], toml::Value::Array(vec![mine, default_plugin_value()]));
}

#[test]
fn v32_appends_the_default_plugin_entry_to_plugins_tables() {
	let existing = "version = 31\n\n[[plugins]]\nname = \"mine\"\ntype = \"stdio\"\ncommand = \"prog\"\n";
	let mine = toml::from_str::<toml::Value>(existing).unwrap()["plugins"][0].clone();
	let value = migrate_v32(existing);
	assert_eq!(value["plugins"], toml::Value::Array(vec![mine, default_plugin_value()]));
}

#[test]
fn v32_leaves_an_existing_besydr_entry_alone_even_when_the_supervisor_was_off() {
	for existing in [
		"version = 31\nplugins = [{ name = \"besydr\", type = \"stdio\", command = \"my-besydr\" }]\n\n[supervisor]\nenabled = false\n",
		"version = 31\n\n[supervisor]\nenabled = false\n\n[[plugins]]\nname = \"besydr\"\ntype = \"stdio\"\ncommand = \"my-besydr\"\n",
	] {
		let users_plugins = toml::from_str::<toml::Value>(existing).unwrap()["plugins"].clone();
		let value = migrate_v32(existing);
		assert_eq!(value["plugins"], users_plugins, "{existing}");
	}
}

#[test]
fn v32_rejects_plugins_of_another_type() {
	for existing in [
		"version = 31\nplugins = 5\n",
		"version = 31\nplugins = \"besydr\"\n",
		"version = 31\n\n[plugins]\nname = \"besydr\"\n",
	] {
		assert!(plan().migrate(existing, DEFAULT_CONFIG_TEMPLATE).is_err(), "{existing}");
	}
}

#[test]
fn v32_switches_the_added_entry_off_only_when_the_supervisor_was_off() {
	for (supervisor_table, expected) in [
		("[supervisor]\nenabled = false\n", Some(false)),
		("[supervisor]\nenabled = true\n", None),
		("[supervisor]\nmax_identical_rounds = 3\n", None),
		("", None),
	] {
		let value = migrate_v32(&format!("version = 31\n\n{supervisor_table}"));
		let entry = &value["plugins"][0];
		assert_eq!(entry["name"].as_str(), Some("besydr"));
		assert_eq!(
			entry.get("enabled").and_then(toml::Value::as_bool),
			expected,
			"{supervisor_table:?}"
		);
	}
}

#[test]
fn v26_adds_websocket_request_transport_and_preserves_explicit_choice() {
	use crate::config::RequestTransportConfig;

	for (transport_setting, expected) in [
		(None, RequestTransportConfig::WebSocket),
		(Some("websocket"), RequestTransportConfig::WebSocket),
		(Some("http"), RequestTransportConfig::Http),
	] {
		let mut document = template_document();
		document["version"] = toml_edit::value(25);
		match transport_setting {
			Some(value) => document["model"]["request_transport"] = toml_edit::value(value),
			None => {
				document["model"].as_table_mut().unwrap().remove("request_transport");
			}
		}
		let migration = upgrade(&document.to_string()).unwrap().unwrap().migration;
		let config: crate::config::Config = toml::from_str(&migration.content).unwrap();
		assert_eq!(config.version, CURRENT_CONFIG_VERSION);
		assert_eq!(config.request_transport, expected);
		assert_eq!(config.get_compression_model_profile().request_transport, expected);
	}
}
