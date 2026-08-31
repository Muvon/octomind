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
			VersionMigration {
				from: 4,
				to: 5,
				apply: add_v5_supervisor_fields,
			},
			VersionMigration {
				from: 5,
				to: 6,
				apply: remove_v6_compression_hints,
			},
			VersionMigration {
				from: 6,
				to: 7,
				apply: remove_v7_supervisor_judges,
			},
			VersionMigration {
				from: 7,
				to: 8,
				apply: remove_v8_compression_ignore_cost,
			},
			VersionMigration {
				from: 8,
				to: 9,
				apply: add_v9_adaptive_condense,
			},
			VersionMigration {
				from: 9,
				to: 10,
				apply: remove_v10_learning_backends,
			},
			VersionMigration {
				from: 10,
				to: 11,
				apply: add_v11_learning_evolution,
			},
			VersionMigration {
				from: 11,
				to: 12,
				apply: unify_v12_model_profiles,
			},
		],
	)
	.with_missing_version(0)
}

/// v12 nests one validated model profile under each actual model owner. Main is
/// the baseline; roles inherit it, supervisor (including learning) has one
/// shared override, and compression remains a separate override.
fn unify_v12_model_profiles(
	document: &mut toml_edit::DocumentMut,
	template: &toml_edit::DocumentMut,
) -> Result<()> {
	const PROFILE_FIELDS: [&str; 8] = [
		"reasoning_effort",
		"max_tokens",
		"temperature",
		"top_p",
		"top_k",
		"max_retries",
		"retry_timeout",
		"request_timeout_seconds",
	];

	let template_main = required_table(
		template.as_table(),
		"model",
		"embedded default configuration",
	)?;
	let mut main = toml_edit::Table::new();
	if let Some(value) = document.as_table_mut().remove("model") {
		match value.into_table() {
			Ok(table) => main = table,
			Err(value) => {
				main.insert("name", value);
			}
		}
	}
	for key in PROFILE_FIELDS {
		if let Some(value) = document.as_table_mut().remove(key) {
			main.insert(key, value);
		}
	}
	document
		.as_table_mut()
		.insert("model", toml_edit::Item::Table(main));
	let main = document
		.as_table_mut()
		.get_mut("model")
		.and_then(|item| item.as_table_mut())
		.expect("model table inserted above");
	for key in ["name"].into_iter().chain(PROFILE_FIELDS.into_iter()) {
		merge_missing(main, template_main, key)?;
	}

	if let Some(roles) = document
		.as_table_mut()
		.get_mut("roles")
		.and_then(|item| item.as_array_of_tables_mut())
	{
		for role in roles.iter_mut() {
			let mut profile = toml_edit::Table::new();
			if let Some(value) = role.remove("model") {
				profile.insert("name", value);
			}
			for key in ["temperature", "top_p", "top_k"] {
				if let Some(value) = role.remove(key) {
					profile.insert(key, value);
				}
			}
			if !profile.is_empty() {
				role.insert("model", toml_edit::Item::Table(profile));
			}
		}
	}

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
	let mut supervisor_profile = toml_edit::Table::new();
	if let Some(value) = supervisor.remove("model") {
		match value.into_table() {
			Ok(table) => supervisor_profile = table,
			Err(value) => {
				supervisor_profile.insert("name", value);
			}
		}
	}
	let template_supervisor_profile = required_table(
		template_supervisor,
		"model",
		"embedded default supervisor configuration",
	)?;
	for key in PROFILE_FIELDS {
		if let Some(value) = supervisor.remove(key) {
			supervisor_profile.insert(key, value);
		}
		merge_missing(&mut supervisor_profile, template_supervisor_profile, key)?;
	}
	merge_missing(&mut supervisor_profile, template_supervisor_profile, "name")?;
	supervisor.insert("model", toml_edit::Item::Table(supervisor_profile));

	if let Some(learning) = supervisor
		.get_mut("learning")
		.and_then(|item| item.as_table_mut())
	{
		learning.remove("model");
		for key in PROFILE_FIELDS {
			learning.remove(key);
		}
	}

	if let Some(gate) = supervisor
		.get_mut("gate")
		.and_then(|item| item.as_table_mut())
	{
		gate.remove("verifier_model");
		gate.remove("max_tokens");
	}
	if let Some(plan) = supervisor
		.get_mut("plan")
		.and_then(|item| item.as_table_mut())
	{
		plan.remove("model");
	}
	if let Some(condense) = supervisor
		.get_mut("condense")
		.and_then(|item| item.as_table_mut())
	{
		condense.remove("model");
	}

	let template_compression = required_table(
		template.as_table(),
		"compression",
		"embedded default configuration",
	)?;
	let template_model = required_table(
		template_compression,
		"model",
		"embedded default compression configuration",
	)?;
	let compression = ensure_table(
		document.as_table_mut(),
		template.as_table(),
		"compression",
		"user configuration",
	)?;
	let mut compression_profile = compression
		.remove("decision")
		.and_then(|item| item.into_table().ok())
		.unwrap_or_default();
	if let Some(value) = compression_profile.remove("model") {
		compression_profile.insert("name", value);
	}
	for key in PROFILE_FIELDS {
		merge_missing(&mut compression_profile, template_model, key)?;
	}
	merge_missing(&mut compression_profile, template_model, "name")?;
	compression.insert("model", toml_edit::Item::Table(compression_profile));

	Ok(())
}

/// v11 adds the opt-in grounded behavior-evolution stage beneath learning.
/// The table is required after migration so runtime behavior never depends on
/// a silent serde fallback; existing users remain disabled until they opt in.
fn add_v11_learning_evolution(
	document: &mut toml_edit::DocumentMut,
	template: &toml_edit::DocumentMut,
) -> Result<()> {
	let template_supervisor = required_table(
		template.as_table(),
		"supervisor",
		"embedded default configuration",
	)?;
	let template_learning = required_table(
		template_supervisor,
		"learning",
		"embedded default supervisor configuration",
	)?;
	let supervisor = ensure_table(
		document.as_table_mut(),
		template.as_table(),
		"supervisor",
		"user configuration",
	)?;
	let learning = ensure_table(
		supervisor,
		template_supervisor,
		"learning",
		"user supervisor configuration",
	)?;
	merge_missing(learning, template_learning, "evolution")
}

/// v10 makes supervisor learning file-authoritative. The alternate MCP adapter
/// and its field maps could not preserve the verified memory schema, outcome
/// feedback, graph identity, or bounded retention lifecycle.
fn remove_v10_learning_backends(
	document: &mut toml_edit::DocumentMut,
	_template: &toml_edit::DocumentMut,
) -> Result<()> {
	let Some(learning) = document
		.as_table_mut()
		.get_mut("supervisor")
		.and_then(|item| item.as_table_mut())
		.and_then(|supervisor| supervisor.get_mut("learning"))
		.and_then(|item| item.as_table_mut())
	else {
		return Ok(());
	};
	for key in ["backend", "store", "retrieve"] {
		learning.remove(key);
	}
	Ok(())
}

/// v9 adds the opt-in runtime-only adaptive condenser trigger. Existing
/// configurations remain on the exact fixed-threshold behavior.
fn add_v9_adaptive_condense(
	document: &mut toml_edit::DocumentMut,
	template: &toml_edit::DocumentMut,
) -> Result<()> {
	let template_condense = required_table(
		required_table(
			template.as_table(),
			"supervisor",
			"embedded default configuration",
		)?,
		"condense",
		"embedded default supervisor configuration",
	)?;
	let supervisor = ensure_table(
		document.as_table_mut(),
		template.as_table(),
		"supervisor",
		"user configuration",
	)?;
	let condense = ensure_table(
		supervisor,
		required_table(
			template.as_table(),
			"supervisor",
			"embedded default configuration",
		)?,
		"condense",
		"user supervisor configuration",
	)?;
	merge_missing(condense, template_condense, "adaptive")
}

/// v8 removes `compression.decision.ignore_cost`. The dollar gate it switched
/// off is gone: a fold is now amortized over the session's own pace in price
/// ratios, and the decision call's spend is always tracked.
fn remove_v8_compression_ignore_cost(
	document: &mut toml_edit::DocumentMut,
	_template: &toml_edit::DocumentMut,
) -> Result<()> {
	if let Some(decision) = document
		.as_table_mut()
		.get_mut("compression")
		.and_then(|item| item.as_table_mut())
		.and_then(|compression| compression.get_mut("decision"))
		.and_then(|item| item.as_table_mut())
	{
		decision.remove("ignore_cost");
	}
	Ok(())
}

/// v7 removes the supervisor's judge mechanics and their knobs: claim_check,
/// the steer circuit-breaker, the delegate gate, per-knob detectors (now fixed
/// constants), recitation/orientation switches (now always on with learning),
/// hardcoded gate/plan budgets, and plan auto-adoption. The mechanics were
/// deleted from the runtime; this keeps user configs free of dead keys.
fn remove_v7_supervisor_judges(
	document: &mut toml_edit::DocumentMut,
	_template: &toml_edit::DocumentMut,
) -> Result<()> {
	let Some(supervisor) = document
		.as_table_mut()
		.get_mut("supervisor")
		.and_then(|item| item.as_table_mut())
	else {
		return Ok(());
	};
	for key in [
		"claim_check",
		"max_consecutive_steers",
		"orientation",
		"detectors",
		"recite",
		"delegate",
	] {
		supervisor.remove(key);
	}
	if let Some(gate) = supervisor
		.get_mut("gate")
		.and_then(|item| item.as_table_mut())
	{
		for key in [
			"max_iterations",
			"require_check_after_mutation",
			"require_plan_complete",
		] {
			gate.remove(key);
		}
	}
	if let Some(plan) = supervisor
		.get_mut("plan")
		.and_then(|item| item.as_table_mut())
	{
		for key in [
			"max_tokens",
			"trajectory_max_tokens",
			"adoption_min_actions",
			"adoption_min_distinct_actions",
		] {
			plan.remove(key);
		}
	}
	if let Some(learning) = supervisor
		.get_mut("learning")
		.and_then(|item| item.as_table_mut())
	{
		for key in ["min_messages_for_intermediate", "max_inject"] {
			learning.remove(key);
		}
	}
	Ok(())
}

/// v6 removes the obsolete terminal `/plan next` hint. Plan state is owned by
/// the supervisor sidecar, and `/plan` is now a read-only display command.
fn remove_v6_compression_hints(
	document: &mut toml_edit::DocumentMut,
	_template: &toml_edit::DocumentMut,
) -> Result<()> {
	if let Some(compression) = document
		.as_table_mut()
		.get_mut("compression")
		.and_then(|item| item.as_table_mut())
	{
		compression.remove("hints_enabled");
		compression.remove("hints_pressure_threshold");
		compression.remove("hints_min_interval");
	}
	Ok(())
}

/// v5 adds the configurable verifier budget, the external plan manager, and
/// the re-read advisory threshold. Existing supervisor settings and comments
/// are preserved; only missing keys are copied from the embedded template.
fn add_v5_supervisor_fields(
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

	merge_missing(supervisor, template_supervisor, "gate")?;
	merge_missing(supervisor, template_supervisor, "plan")
}

/// v2 (octomind 0.40) originally added the delegate gate; the gate is gone
/// (v7 removes its keys), so this step now only guarantees a `[supervisor]`
/// section exists — a config predating it gets the whole section from the
/// template (octomind requires every field to be present).
fn add_delegate_gate(
	document: &mut toml_edit::DocumentMut,
	template: &toml_edit::DocumentMut,
) -> Result<()> {
	ensure_table(
		document.as_table_mut(),
		template.as_table(),
		"supervisor",
		"user configuration",
	)?;
	Ok(())
}

/// v3 adds required budgets for retained compression findings and the PACT
/// attention controller. Existing values and comments are preserved; only
/// missing keys are copied from the embedded template. (Its detector-advisory
/// keys were removed again in v7.)
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
	merge_missing(compression, template_compression, "attention")
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
			eprintln!("✅ Config is already at the latest version ({version})");
		}
		return Ok(false);
	};

	// stderr, never stdout: an outdated config is upgraded during startup, and
	// ACP/MCP stdio modes carry JSON-RPC on stdout.
	eprintln!(
		"🔄 Upgrading config from version {} to {}...",
		migration.from_version, migration.to_version
	);

	// Never replace the user's file with something that no longer parses.
	toml::from_str::<toml::Value>(&migration.content)
		.context("Migrated config is not valid TOML - aborting upgrade")?;

	let backup_path = config_file::apply_migration(config_path, original.as_bytes(), &migration)?;

	eprintln!(
		"✅ Config upgraded successfully! Backup saved to: {}",
		backup_path.display()
	);

	Ok(true)
}

#[cfg(test)]
#[path = "migrations_inline_tests.rs"]
mod inline_tests;

#[cfg(test)]
#[path = "migrations_tests.rs"]
mod migrations_tests;
