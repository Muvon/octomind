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

//! Automatic upgrades of `config.toml` when the embedded template's schema
//! version moves ahead of the user's file.
//!
//! The mechanics — version chain, guards, table merging, locking, backup and
//! atomic replace — live in `octolib::utils`; this module only declares
//! octomind's version steps and the CLI-facing entry points.

use anyhow::{Context, Result};
use octolib::utils::config_file;
// `toml_edit` comes from octolib's re-export: the migration `apply` signature
// is a function pointer, so both sides must see the exact same crate.
use octolib::utils::config_migration::{
	ensure_table, merge_missing, required_table, toml_edit, MigrationPlan, VersionMigration,
};
use std::fs;
use std::path::Path;

/// Schema source of truth: the version stamped here is what every config is
/// migrated up to.
const DEFAULT_CONFIG_TEMPLATE: &str = include_str!("../../config-templates/default.toml");

/// Octomind's version chain.
///
/// `with_missing_version(0)` because configs written before the version stamp
/// existed are a real, migratable state rather than a corrupt file.
fn plan() -> MigrationPlan {
	MigrationPlan::new(
		"octomind",
		vec![
			// v0 -> v1 was purely the introduction of the `version` stamp,
			// which the driver writes itself. Nothing else to do.
			VersionMigration {
				from: 0,
				to: 1,
				apply: |_document, _template| Ok(()),
			},
			VersionMigration {
				from: 1,
				to: 2,
				apply: add_delegate_gate,
			},
			VersionMigration {
				from: 2,
				to: 3,
				apply: add_v3_required_fields,
			},
			VersionMigration {
				from: 3,
				to: 4,
				apply: collapse_pressure_levels,
			},
		],
	)
	.with_missing_version(0)
}

/// v2 (octomind 0.40) adds `[supervisor.delegate]` — the handoff quality gate.
///
/// A config that predates `[supervisor]` altogether gets the whole section from
/// the template (octomind requires every field to be present), otherwise only
/// the missing `delegate` keys are filled in; user values always win.
fn add_delegate_gate(
	document: &mut toml_edit::DocumentMut,
	template: &toml_edit::DocumentMut,
) -> Result<()> {
	let template_supervisor = required_table(
		template.as_table(),
		"supervisor",
		"embedded default configuration",
	)?;

	let supervisor = ensure_table(
		document.as_table_mut(),
		template.as_table(),
		"supervisor",
		"user configuration",
	)?;

	merge_missing(supervisor, template_supervisor, "delegate")
}

/// v3 adds required budgets for retained compression findings, Sequential
/// advisories, and the PACT attention controller. Existing values and comments
/// are preserved; only missing keys are copied from the embedded template.
fn add_v3_required_fields(
	document: &mut toml_edit::DocumentMut,
	template: &toml_edit::DocumentMut,
) -> Result<()> {
	let template_compression = required_table(
		template.as_table(),
		"compression",
		"embedded default configuration",
	)?;
	let compression = ensure_table(
		document.as_table_mut(),
		template.as_table(),
		"compression",
		"user configuration",
	)?;

	merge_missing(
		compression,
		template_compression,
		"analysis_findings_max_tokens",
	)?;
	merge_missing(compression, template_compression, "attention")?;
	let template_supervisor = required_table(
		template.as_table(),
		"supervisor",
		"embedded default configuration",
	)?;
	let template_detectors = required_table(
		template_supervisor,
		"detectors",
		"embedded default configuration",
	)?;
	let supervisor = ensure_table(
		document.as_table_mut(),
		template.as_table(),
		"supervisor",
		"user configuration",
	)?;
	let detectors = ensure_table(
		supervisor,
		template_supervisor,
		"detectors",
		"user configuration",
	)?;

	merge_missing(
		detectors,
		template_detectors,
		"sequential_max_steers_per_turn",
	)
}

/// v4 collapses the `[[compression.pressure_levels]]` ladder into the single
/// adaptive `compression.threshold` trigger. The lowest configured level was
/// the point where compression became eligible, so its threshold carries over;
/// depth is computed at runtime now and the ratio ladder is dropped.
fn collapse_pressure_levels(
	document: &mut toml_edit::DocumentMut,
	template: &toml_edit::DocumentMut,
) -> Result<()> {
	let template_compression = required_table(
		template.as_table(),
		"compression",
		"embedded default configuration",
	)?;
	let compression = ensure_table(
		document.as_table_mut(),
		template.as_table(),
		"compression",
		"user configuration",
	)?;

	let lowest_threshold = compression
		.get("pressure_levels")
		.and_then(|item| item.as_array_of_tables())
		.and_then(|levels| {
			levels
				.iter()
				.filter_map(|level| level.get("threshold").and_then(|v| v.as_integer()))
				.min()
		});
	compression.remove("pressure_levels");

	if !compression.contains_key("threshold") {
		if let Some(threshold) = lowest_threshold {
			compression["threshold"] = toml_edit::value(threshold);
		} else {
			merge_missing(compression, template_compression, "threshold")?;
		}
	}
	Ok(())
}

/// Upgrade `config_path` in place when it lags behind the embedded template.
///
/// Returns whether the file was rewritten. The common case — an up-to-date
/// config — takes no lock and touches nothing.
pub fn check_and_upgrade_config(config_path: &Path) -> Result<bool> {
	let content =
		fs::read_to_string(config_path).context("Failed to read config file for version check")?;

	// Cheap pre-check outside the lock; the authoritative one runs under it.
	if plan().migrate(&content, DEFAULT_CONFIG_TEMPLATE)?.is_none() {
		return Ok(false);
	}

	config_file::with_lock(config_path, || upgrade_locked(config_path, false))
}

/// `octomind config --upgrade`: same upgrade, but a missing file is an error
/// and an already-current file reports success instead of staying silent.
pub fn force_upgrade_config(config_path: &Path) -> Result<()> {
	if !config_path.exists() {
		return Err(anyhow::anyhow!(
			"Config file not found: {}",
			config_path.display()
		));
	}

	config_file::with_lock(config_path, || upgrade_locked(config_path, true))?;
	Ok(())
}

/// The migration proper. Must be called holding the config lock: another
/// process may have upgraded the file between our pre-check and here, so the
/// content is re-read rather than passed in.
fn upgrade_locked(config_path: &Path, report_up_to_date: bool) -> Result<bool> {
	let original = fs::read_to_string(config_path).context("Failed to read config file")?;

	let Some(migration) = plan().migrate(&original, DEFAULT_CONFIG_TEMPLATE)? else {
		if report_up_to_date {
			let version = plan().version_of(&original)?;
			println!("✅ Config is already at the latest version ({version})");
		}
		return Ok(false);
	};

	println!(
		"🔄 Upgrading config from version {} to {}...",
		migration.from_version, migration.to_version
	);

	// Never replace the user's file with something that no longer parses.
	toml::from_str::<toml::Value>(&migration.content)
		.context("Migrated config is not valid TOML - aborting upgrade")?;

	let backup_path = config_file::apply_migration(config_path, original.as_bytes(), &migration)?;

	println!(
		"✅ Config upgraded successfully! Backup saved to: {}",
		backup_path.display()
	);

	Ok(true)
}

#[cfg(test)]
mod tests {
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
		assert_eq!(migrated["version"].as_integer(), Some(4));
		assert_eq!(migrated["log_level"].as_str(), Some("info"));
		assert!(migrated["supervisor"]["delegate"]["enabled"]
			.as_bool()
			.is_some());
		assert_eq!(
			migrated["compression"]["analysis_findings_max_tokens"].as_integer(),
			Some(4000)
		);
		assert_eq!(
			migrated["supervisor"]["detectors"]["sequential_max_steers_per_turn"].as_integer(),
			Some(0)
		);
		assert_eq!(
			migrated["compression"]["attention"]["enabled"].as_bool(),
			Some(false)
		);
		assert_eq!(
			migrated["compression"]["attention"]["governance"]["verify_hash"].as_bool(),
			Some(true)
		);
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
		assert_eq!(migration.to_version, 4);
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
	fn v1_gains_delegate_and_keeps_user_values_and_comments() {
		let existing = r#"# keep me
version = 1

[supervisor]
enabled = false
model = "openrouter:custom/model"

[supervisor.condense]
enabled = false
tokens_threshold = 1234
model = "openrouter:custom/model"
"#;

		let migration = plan()
			.migrate(existing, DEFAULT_CONFIG_TEMPLATE)
			.unwrap()
			.expect("v1 must migrate");

		assert_eq!(migration.from_version, 1);
		assert_eq!(migration.to_version, 4);
		assert!(migration.content.contains("# keep me"));
		// The template's documentation comes across with the new section.
		assert!(migration.content.contains("# Delegate gate"));

		let migrated: toml::Value = toml::from_str(&migration.content).unwrap();
		assert_eq!(migrated["version"].as_integer(), Some(4));
		assert_eq!(migrated["supervisor"]["enabled"].as_bool(), Some(false));
		assert_eq!(
			migrated["supervisor"]["condense"]["tokens_threshold"].as_integer(),
			Some(1234)
		);
		assert_eq!(
			migrated["supervisor"]["delegate"]["enabled"].as_bool(),
			Some(true)
		);
		assert!(migrated["supervisor"]["delegate"]["max_revisions"]
			.as_integer()
			.is_some());
	}

	#[test]
	fn partially_configured_delegate_keeps_user_keys() {
		let existing = r#"version = 1

[supervisor]
enabled = true

[supervisor.delegate]
enabled = false
"#;

		let migration = plan()
			.migrate(existing, DEFAULT_CONFIG_TEMPLATE)
			.unwrap()
			.expect("v1 must migrate");
		let migrated: toml::Value = toml::from_str(&migration.content).unwrap();

		assert_eq!(
			migrated["supervisor"]["delegate"]["enabled"].as_bool(),
			Some(false)
		);
		// Keys the user never set are filled from the template.
		assert!(migrated["supervisor"]["delegate"]["model"]
			.as_str()
			.is_some());
		assert!(migrated["supervisor"]["delegate"]["max_revisions"]
			.as_integer()
			.is_some());
	}

	#[test]
	fn fully_configured_delegate_is_untouched() {
		let existing = r#"version = 1

[supervisor]
enabled = true

[supervisor.delegate]
enabled = false
model = "openrouter:custom/model"
max_revisions = 9
"#;

		let migration = plan()
			.migrate(existing, DEFAULT_CONFIG_TEMPLATE)
			.unwrap()
			.expect("v1 must migrate to stamp the version");
		let migrated: toml::Value = toml::from_str(&migration.content).unwrap();

		assert_eq!(
			migrated["supervisor"]["delegate"]["model"].as_str(),
			Some("openrouter:custom/model")
		);
		assert_eq!(
			migrated["supervisor"]["delegate"]["max_revisions"].as_integer(),
			Some(9)
		);
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
		assert_eq!(migration.to_version, 4);
		assert!(migration.content.contains("# keep compression notes"));
		assert_eq!(
			migrated["compression"]["hints_enabled"].as_bool(),
			Some(false)
		);
		assert_eq!(
			migrated["compression"]["knowledge_retention"].as_integer(),
			Some(17)
		);
		assert_eq!(
			migrated["compression"]["analysis_findings_max_tokens"].as_integer(),
			Some(4000)
		);
		assert_eq!(
			migrated["supervisor"]["detectors"]["sequential_threshold"].as_integer(),
			Some(3)
		);
		assert_eq!(
			migrated["supervisor"]["detectors"]["sequential_max_steers_per_turn"].as_integer(),
			Some(0)
		);
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
		assert_eq!(migration.to_version, 4);
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
	fn v3_without_pressure_levels_takes_template_threshold() {
		let existing = "version = 3\n\n[compression]\nhints_enabled = true\n";

		let migration = plan()
			.migrate(existing, DEFAULT_CONFIG_TEMPLATE)
			.unwrap()
			.expect("v3 must migrate");

		let migrated: toml::Value = toml::from_str(&migration.content).unwrap();
		assert_eq!(
			migrated["compression"]["threshold"].as_integer(),
			Some(90000)
		);
	}

	#[test]
	fn future_version_is_rejected_rather_than_downgraded() {
		let future = DEFAULT_CONFIG_TEMPLATE.replacen("version = 4", "version = 99", 1);
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

		let migrated: toml::Value =
			toml::from_str(&fs::read_to_string(&config_path).unwrap()).unwrap();
		assert_eq!(migrated["version"].as_integer(), Some(4));

		fs::remove_dir_all(&dir).ok();
	}

	#[test]
	fn force_upgrade_errors_when_config_is_missing() {
		let missing =
			std::env::temp_dir().join(format!("octomind-absent-{}.toml", uuid::Uuid::new_v4()));
		assert!(force_upgrade_config(&missing).is_err());
	}
}
