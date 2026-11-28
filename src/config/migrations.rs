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

use super::plugins::{DEFAULT_PLUGINS_TOML, DEFAULT_PLUGIN_NAME};
use anyhow::{Context, Result};
use octolib::utils::config_file;
// `toml_edit` comes from octolib's re-export: the migration `apply` signature
// is a function pointer, so both sides must see the exact same crate.
use octolib::utils::config_migration::{
	ensure_table, merge_missing, required_table, toml_edit, Migration, MigrationPlan, VersionMigration,
};
use std::fs;
use std::path::{Path, PathBuf};

/// Schema source of truth: the version stamped here is what every config is
/// migrated up to.
const DEFAULT_CONFIG_TEMPLATE: &str = include_str!("../../config-templates/default.toml");

/// The version whose step removes the built-in supervisor's `[supervisor]` and
/// `[decision]` tables.
const SUPERVISOR_REMOVED_IN_VERSION: u32 = 32;

/// Tables that belonged to the built-in supervisor.
const SUPERVISOR_TABLES: [&str; 2] = ["decision", "supervisor"];

/// Where the upgrade to version 32 saves the supervisor's tables, relative to
/// the configuration directory. A directory of its own, because octomind
/// merges every `*.toml` file that sits beside `config.toml`.
const CARRY_DIR_NAME: &str = "besydr-carry";
const CARRY_FILE_NAME: &str = "supervisor.toml";
const CARRY_FILE_HEADER: &str = "# Written by octomind when it upgraded config.toml to version 32.\n\
# These are the settings of the supervisor that was built into octomind, unchanged.\n\
# Besydr, the plugin that took over that work, reads this file once, on its first run.\n";

/// One step per version, each run against the current template.
///
/// A step that changes nothing once added or tidied keys of `[supervisor]` or
/// `[decision]`: version 32 removes both tables, so nothing is left to prepare
/// in them.
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
				apply: |_document, _template| Ok(()),
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
				apply: |_document, _template| Ok(()),
			},
			VersionMigration {
				from: 5,
				to: 6,
				apply: remove_v6_compression_hints,
			},
			VersionMigration {
				from: 6,
				to: 7,
				apply: |_document, _template| Ok(()),
			},
			VersionMigration {
				from: 7,
				to: 8,
				apply: remove_v8_compression_ignore_cost,
			},
			VersionMigration {
				from: 8,
				to: 9,
				apply: |_document, _template| Ok(()),
			},
			VersionMigration {
				from: 9,
				to: 10,
				apply: |_document, _template| Ok(()),
			},
			VersionMigration {
				from: 10,
				to: 11,
				apply: |_document, _template| Ok(()),
			},
			VersionMigration {
				from: 11,
				to: 12,
				apply: unify_v12_model_profiles,
			},
			VersionMigration {
				from: 12,
				to: 13,
				apply: |_document, _template| Ok(()),
			},
			VersionMigration {
				from: 13,
				to: 14,
				apply: |_document, _template| Ok(()),
			},
			VersionMigration {
				from: 14,
				to: 15,
				apply: |_document, _template| Ok(()),
			},
			VersionMigration {
				from: 15,
				to: 16,
				apply: |_document, _template| Ok(()),
			},
			VersionMigration {
				from: 16,
				to: 17,
				apply: |_document, _template| Ok(()),
			},
			VersionMigration {
				from: 17,
				to: 18,
				apply: |_document, _template| Ok(()),
			},
			VersionMigration {
				from: 18,
				to: 19,
				apply: |_document, _template| Ok(()),
			},
			VersionMigration {
				from: 19,
				to: 20,
				apply: add_v20_timing,
			},
			VersionMigration {
				from: 20,
				to: 21,
				apply: |_document, _template| Ok(()),
			},
			VersionMigration {
				from: 21,
				to: 22,
				apply: |_document, _template| Ok(()),
			},
			VersionMigration {
				from: 22,
				to: 23,
				apply: add_v23_commit_attribution,
			},
			VersionMigration {
				from: 23,
				to: 24,
				apply: |_document, _template| Ok(()),
			},
			VersionMigration {
				from: 24,
				to: 25,
				apply: |_document, _template| Ok(()),
			},
			VersionMigration {
				from: 25,
				to: 26,
				apply: add_v26_model_timeouts_and_transport,
			},
			VersionMigration {
				from: 26,
				to: 27,
				apply: |_document, _template| Ok(()),
			},
			VersionMigration {
				from: 27,
				to: 28,
				apply: add_v28_idle_compaction,
			},
			VersionMigration {
				from: 28,
				to: 29,
				apply: |_document, _template| Ok(()),
			},
			VersionMigration {
				from: 29,
				to: 30,
				apply: rename_v30_chief_tags,
			},
			VersionMigration {
				from: 30,
				to: 31,
				apply: |_document, _template| Ok(()),
			},
			VersionMigration {
				from: 31,
				to: 32,
				apply: replace_v32_supervisor_with_plugin,
			},
			VersionMigration {
				from: 32,
				to: 33,
				apply: add_v33_code_mode,
			},
			VersionMigration {
				from: 33,
				to: 34,
				apply: remove_v34_sampling,
			},
			VersionMigration {
				from: 34,
				to: 35,
				apply: upgrade_v35,
			},
			VersionMigration {
				from: 35,
				to: 36,
				apply: |document, _| remove_v36_agents(document),
			},
		],
	)
	.with_missing_version(0)
}

const V35_STOCK_AGENT: &str = r#"
name = "context_gatherer"
description = "Gather detailed context from files and codebase. Reads files, searches code patterns, and provides comprehensive information about specific areas of the codebase for development tasks."
command = "octomind acp context_gatherer"
workdir = "."
"#;

const V36_GUIDANCE: &str = "v36 removed [[agents]]. Move role-based delegation to a tap role and set \
workdir on the run. Arbitrary ACP commands and synchronous agent_* returns have no automatic equivalent. \
Preserve the command and workdir outside the loaded config, migrate its callers, then remove the legacy \
entry and retry. No tap permission is granted automatically.";

fn check_v36_agents(value: &toml::Value) -> Result<()> {
	let Some(item) = value.get("agents") else {
		return Ok(());
	};
	let entries = item
		.as_array()
		.with_context(|| format!("agents must be an array. {V36_GUIDANCE}"))?;
	let stock: toml::Value = toml::from_str(V35_STOCK_AGENT).context("Invalid frozen v35 agent definition")?;
	for (index, entry) in entries.iter().enumerate() {
		if entry != &stock {
			let name = entry.get("name").and_then(toml::Value::as_str).unwrap_or("<unnamed>");
			anyhow::bail!("agents[{index}] ({name:?}) differs from the shipped v35 definition. {V36_GUIDANCE}");
		}
	}
	Ok(())
}

/// Validate the source before deduplication can hide a custom definition.
pub(super) fn strip_v36_agents(value: &mut toml::Value, source: &Path) -> Result<()> {
	if value.get("agents").is_some() {
		check_v36_agents(value).with_context(|| format!("{}: legacy agent configuration", source.display()))?;
		value
			.as_table_mut()
			.expect("a root key belongs to a table")
			.remove("agents");
	}
	Ok(())
}

fn remove_v36_agents(document: &mut toml_edit::DocumentMut) -> Result<()> {
	if document.contains_key("agents") {
		let parsed: toml::Value = toml::from_str(&document.to_string())?;
		check_v36_agents(&parsed)?;
		document.remove("agents");
	}
	Ok(())
}

fn retired_builtin(server: &dyn toml_edit::TableLike) -> bool {
	server.get("name").and_then(toml_edit::Item::as_str) == Some("agent")
		&& server.get("type").and_then(toml_edit::Item::as_str) == Some("builtin")
}

fn retire_v36_server(document: &mut toml_edit::DocumentMut, retire_refs: bool) {
	if let Some(servers) = document
		.get_mut("mcp")
		.and_then(toml_edit::Item::as_table_like_mut)
		.and_then(|mcp| mcp.get_mut("servers"))
	{
		if let Some(tables) = servers.as_array_of_tables_mut() {
			tables.retain(|table| !retired_builtin(table));
			if tables.is_empty() {
				// An empty ArrayOfTables has no serialized TOML spelling.
				*servers = toml_edit::value(toml_edit::Array::new());
			}
		} else if let Some(array) = servers.as_array_mut() {
			array.retain(|v| !v.as_inline_table().is_some_and(|t| retired_builtin(t)));
		}
	}
	if retire_refs {
		for_each_table_of(document.as_table_mut(), "roles", |role| {
			if let Some(refs) = role
				.get_mut("mcp")
				.and_then(toml_edit::Item::as_table_like_mut)
				.and_then(|mcp| mcp.get_mut("server_refs"))
				.and_then(toml_edit::Item::as_array_mut)
			{
				refs.retain(|v| v.as_str() != Some("agent"));
			}
		});
	}
	// Never rewrite user allowlists: [] means unrestricted, not deny-all.
}

/// A whole upgrade: the rewritten configuration, and the supervisor's tables
/// to save for the plugin when the configuration still had them.
struct Upgrade {
	migration: Migration,
	carried_supervisor_settings: Option<String>,
}

/// Upgrade `existing` to the current version. `None` when `existing` is
/// already current.
fn upgrade(existing: &str) -> Result<Option<Upgrade>> {
	let Some(migration) = plan().migrate(existing, DEFAULT_CONFIG_TEMPLATE)? else {
		return Ok(None);
	};
	let carried_supervisor_settings = if migration.from_version < SUPERVISOR_REMOVED_IN_VERSION {
		carried_supervisor_settings(existing)?
	} else {
		None
	};
	Ok(Some(Upgrade {
		migration,
		carried_supervisor_settings,
	}))
}

/// The `[decision]` and `[supervisor]` tables of the configuration being
/// upgraded, as the user has them, as the text of the file the plugin reads,
/// comments included. `None` when the configuration has neither table.
fn carried_supervisor_settings(existing: &str) -> Result<Option<String>> {
	let mut document: toml_edit::DocumentMut = existing.parse().context("Failed to parse the configuration")?;
	let mut carried = toml_edit::DocumentMut::new();
	for table in SUPERVISOR_TABLES {
		if let Some(item) = document.remove(table) {
			carried.insert(table, item);
		}
	}
	if carried.is_empty() {
		return Ok(None);
	}
	Ok(Some(format!("{CARRY_FILE_HEADER}{carried}")))
}

/// Version 32: the built-in supervisor is gone, a plugin does its work.
/// `[supervisor]` and `[decision]` are removed (the upgrade saves them for the
/// plugin first), `max_identical_rounds` moves to the root, and the default
/// plugin entry is added unless the user already has a plugin of that name.
/// A user who had the supervisor switched off gets the entry switched off.
fn replace_v32_supervisor_with_plugin(
	document: &mut toml_edit::DocumentMut,
	template: &toml_edit::DocumentMut,
) -> Result<()> {
	let supervisor = document.remove("supervisor");
	document.remove("decision");
	let supervisor = supervisor.as_ref().and_then(toml_edit::Item::as_table_like);

	// The template's key first, for its comment; then the user's value over it.
	merge_missing(document.as_table_mut(), template.as_table(), "max_identical_rounds")?;
	if let Some(rounds) = supervisor.and_then(|table| table.get("max_identical_rounds")) {
		document["max_identical_rounds"] = rounds.clone();
	}

	let supervisor_was_on = supervisor
		.and_then(|table| table.get("enabled"))
		.and_then(toml_edit::Item::as_bool)
		.unwrap_or(true);
	add_default_plugin(document, supervisor_was_on)
}

fn is_default_plugin(entry: &dyn toml_edit::TableLike) -> bool {
	entry.get("name").and_then(toml_edit::Item::as_str) == Some(DEFAULT_PLUGIN_NAME)
}

/// The entry of [`DEFAULT_PLUGINS_TOML`] as an inline table.
fn default_plugin_entry() -> Result<toml_edit::InlineTable> {
	let defaults: toml_edit::DocumentMut = format!("plugins = {DEFAULT_PLUGINS_TOML}\n")
		.parse()
		.context("The default plugin list does not parse")?;
	let mut entry = defaults
		.get("plugins")
		.and_then(toml_edit::Item::as_array)
		.and_then(|plugins| {
			plugins
				.iter()
				.filter_map(toml_edit::Value::as_inline_table)
				.find(|entry| is_default_plugin(*entry))
		})
		.with_context(|| format!("The default plugin list must have the '{DEFAULT_PLUGIN_NAME}' entry"))?
		.clone();
	// Its line breaks belong to the default list's layout, not to the user's.
	entry.decor_mut().clear();
	Ok(entry)
}

/// Add the default plugin entry to the user's `plugins`, which may be absent,
/// an inline array or `[[plugins]]` tables.
fn add_default_plugin(document: &mut toml_edit::DocumentMut, enabled: bool) -> Result<()> {
	let mut entry = default_plugin_entry()?;
	if !enabled {
		entry.insert("enabled", false.into());
	}

	match document.get_mut("plugins") {
		None => {
			let mut plugins = toml_edit::Array::new();
			plugins.push(entry);
			document["plugins"] = toml_edit::value(plugins);
		}
		Some(toml_edit::Item::Value(toml_edit::Value::Array(plugins))) => {
			let listed = plugins
				.iter()
				.filter_map(toml_edit::Value::as_inline_table)
				.any(|entry| is_default_plugin(entry));
			if !listed {
				plugins.push(entry);
			}
		}
		Some(toml_edit::Item::ArrayOfTables(plugins)) => {
			if !plugins.iter().any(|entry| is_default_plugin(entry)) {
				plugins.push(entry.into_table());
			}
		}
		Some(_) => anyhow::bail!("plugins: an array is required"),
	}
	Ok(())
}

/// Agents renamed when every domain's lead agent became `chief`: `(old tag, new tag)`.
/// `developer:general` and `assistant:general` kept their names and are not listed.
///
/// A record of the v30 config format change only: v35 then rewrites the retired tags, these included.
const V30_RENAMED_TAGS: [(&str, &str); 16] = [
	("assistant:concierge", "assistant:chief"),
	("developer:cto", "developer:chief"),
	("video:producer", "video:chief"),
	("octomind:assistant", "octomind:chief"),
	("octoweb:assistant", "octoweb:chief"),
	("browser:general", "browser:chief"),
	("coach:general", "coach:chief"),
	("devops:general", "devops:chief"),
	("doctor:general", "doctor:chief"),
	("lawyer:general", "lawyer:chief"),
	("tutor:general", "tutor:chief"),
	("vet:general", "vet:chief"),
	("chef:general", "chef:chief"),
	("device:operator", "device:chief"),
	("shopping:advisor", "shopping:chief"),
	("travel:planner", "travel:chief"),
];

/// v30: domain lead agents were renamed to `chief` (see `V30_RENAMED_TAGS`). Rewrite old tags used
/// as `[taps]` keys and as the root `default`; where the new key already exists it keeps its value
/// and the old key is dropped.
fn rename_v30_chief_tags(document: &mut toml_edit::DocumentMut, _template: &toml_edit::DocumentMut) -> Result<()> {
	if let Some(taps) = document
		.as_table_mut()
		.get_mut("taps")
		.map(|item| item.as_table_like_mut().context("taps must be a table"))
		.transpose()?
	{
		for (old_tag, new_tag) in V30_RENAMED_TAGS {
			if let Some(item) = taps.remove(old_tag) {
				if !taps.contains_key(new_tag) {
					taps.insert(new_tag, item);
				}
			}
		}
	}
	if let Some(default) = document
		.as_table_mut()
		.get_mut("default")
		.and_then(|item| item.as_value_mut())
	{
		if let Some((_, new_tag)) = V30_RENAMED_TAGS
			.iter()
			.find(|(old_tag, _)| default.as_str() == Some(*old_tag))
		{
			// Keep the user's trailing comment on the line.
			let decor = default.decor().clone();
			*default = toml_edit::Value::from(*new_tag);
			*default.decor_mut() = decor;
		}
	}
	Ok(())
}

/// The role names the templates shipped as an example, which would hide the tap agent of the
/// same name once `default` is `assistant`.
const EXAMPLE_ASSISTANT_ROLE: &str = "assistant";

/// Every `assistant` role the default template shipped before v35, in the fields it had once
/// the v34 step removed the sampling keys. Found in the history of the template.
const SHIPPED_ASSISTANT_ROLES: [&str; 6] = [
	// The last form, with its tool servers.
	r#"
name = "assistant"
system = "You are helpful and knowledgeable assistant.\nWorking directory: {{CWD}}\n"
welcome = "Hello! Ready to code. Working in {{CWD}} (Role: {{ROLE}})"
[mcp]
server_refs = ["core", "orchestration", "runtime", "filesystem", "agent"]
allowed_tools = ["core:*", "orchestration:*", "runtime:*", "filesystem:*", "agent:*"]
"#,
	// Before the `orchestration` server was added.
	r#"
name = "assistant"
system = "You are helpful and knowledgeable assistant.\nWorking directory: {{CWD}}\n"
welcome = "Hello! Ready to code. Working in {{CWD}} (Role: {{ROLE}})"
[mcp]
server_refs = ["core", "runtime", "filesystem", "agent"]
allowed_tools = ["core:*", "runtime:*", "filesystem:*", "agent:*"]
"#,
	// Before the `runtime` server was added.
	r#"
name = "assistant"
system = "You are helpful and knowledgeable assistant.\nWorking directory: {{CWD}}\n"
welcome = "Hello! Ready to code. Working in {{CWD}} (Role: {{ROLE}})"
[mcp]
server_refs = ["core", "filesystem", "agent"]
allowed_tools = ["core:*", "filesystem:*", "agent:*"]
"#,
	// The brief form that carried its own `[roles.model]` block.
	r#"
name = "assistant"
system = "You are helpful and knowledgeable assistant.\nWorking directory: {{CWD}}\n"
welcome = "Hello! Ready to code. Working in {{CWD}} (Role: {{ROLE}})"
[model]
name = "octohub:auto"
reasoning_effort = "medium"
max_tokens = 32768
max_retries = 1
retry_timeout = 30
request_timeout_seconds = 300
[mcp]
server_refs = ["core", "orchestration", "runtime", "filesystem", "agent"]
allowed_tools = ["core:*", "orchestration:*", "runtime:*", "filesystem:*", "agent:*"]
"#,
	// The oldest forms: one line of system prompt, no tool servers.
	r#"
name = "assistant"
enable_layers = false
layer_refs = []
system = "You are a helpful assistant."
"#,
	r#"
name = "assistant"
enable_layers = false
max_tokens = 8192
layer_refs = []
system = "You are a helpful assistant."
"#,
];

/// What the migration tells a user whose `assistant` role differs from the shipped example.
const EXAMPLE_ASSISTANT_KEPT_NOTICE: &str = "The local role `assistant` takes priority over the tap agent `assistant`; rename or remove it to use the tap agent.";

/// v35: remove the `assistant` role of the old template, but only when it is exactly one of the
/// shipped forms, compared by parsed fields so whitespace and comments do not matter. A role the
/// user changed stays, with a notice that it hides the tap agent of the same name.
fn drop_v35_example_assistant_role(document: &mut toml_edit::DocumentMut) -> Result<()> {
	let parsed: toml::Value = toml::from_str(&document.to_string()).context("Failed to parse the configuration")?;
	let Some(roles) = parsed.get("roles").and_then(toml::Value::as_array) else {
		return Ok(());
	};
	let Some(index) = roles
		.iter()
		.position(|role| role.get("name").and_then(toml::Value::as_str) == Some(EXAMPLE_ASSISTANT_ROLE))
	else {
		return Ok(());
	};

	// The shipped forms are written with `[mcp]` and `[model]` at the top: a role's own tables.
	let role = &roles[index];
	let mut shipped = false;
	for text in SHIPPED_ASSISTANT_ROLES {
		let variant: toml::Value = toml::from_str(text).context("A shipped role does not parse")?;
		shipped |= variant == *role;
	}
	if !shipped {
		eprintln!("{EXAMPLE_ASSISTANT_KEPT_NOTICE}");
		return Ok(());
	}

	match document.get_mut("roles") {
		Some(toml_edit::Item::ArrayOfTables(tables)) => {
			tables.remove(index);
			if tables.is_empty() {
				document.remove("roles");
			}
		}
		Some(toml_edit::Item::Value(toml_edit::Value::Array(array))) => {
			array.remove(index);
			if array.is_empty() {
				document.remove("roles");
			}
		}
		_ => anyhow::bail!("roles: an array is required"),
	}
	Ok(())
}

/// v35: the retired community agent tags name their current agents, the example `assistant`
/// role is gone, and `[registry]` names the built-in tap and its ref.
fn upgrade_v35(document: &mut toml_edit::DocumentMut, template: &toml_edit::DocumentMut) -> Result<()> {
	for notice in rewrite_v35_retired_agent_tags(document)? {
		eprintln!("{notice}");
	}
	drop_v35_example_assistant_role(document)?;
	let template_registry = required_table(template.as_table(), "registry", "embedded config")?;
	let registry = ensure_table(document.as_table_mut(), template.as_table(), "registry", "user config")?;
	for key in ["cache_ttl_hours", "tap", "ref"] {
		merge_missing(registry, template_registry, key)?;
	}
	Ok(())
}

/// The community agent tags that were retired, one `old tag<TAB>current agent` row each. A tag
/// of a private tap (`acme:reviewer`) is not in it and is never rewritten.
const RETIRED_AGENT_TAGS: &str = include_str!("retired_agent_tags.tsv");

/// The agent that replaced the retired tag `tag`; `None` for every other tag.
fn current_agent_tag(tag: &str) -> Option<&'static str> {
	RETIRED_AGENT_TAGS
		.lines()
		.filter_map(|row| row.split_once('\t'))
		.find_map(|(retired, current)| (retired == tag).then_some(current))
}

/// Say that the retired `tag` is now `current`, once per tag.
fn note_rewrite(notices: &mut Vec<String>, tag: &str, current: &str) {
	let notice = format!("Agent tag '{tag}' is now '{current}'.");
	if !notices.contains(&notice) {
		notices.push(notice);
	}
}

/// Rewrite the retired tags of the string list `key` of `owner` to their current agents, keeping
/// the order and one entry of each agent.
fn rewrite_tag_list(owner: &mut dyn toml_edit::TableLike, key: &str, notices: &mut Vec<String>) {
	let Some(list) = owner.get_mut(key).and_then(toml_edit::Item::as_array_mut) else {
		return;
	};
	if !list
		.iter()
		.filter_map(toml_edit::Value::as_str)
		.any(|tag| current_agent_tag(tag).is_some())
	{
		return;
	}
	let mut rewritten: Vec<String> = Vec::new();
	for tag in list.iter().filter_map(toml_edit::Value::as_str) {
		let current = current_agent_tag(tag).inspect(|current| note_rewrite(notices, tag, current));
		let tag = current.unwrap_or(tag).to_string();
		if !rewritten.contains(&tag) {
			rewritten.push(tag);
		}
	}
	list.clear();
	list.extend(rewritten);
}

/// Apply `change` to every table of the array `key` in `owner`, whether it is written as
/// `[[key]]` tables or as an inline array.
fn for_each_table_of(
	owner: &mut dyn toml_edit::TableLike,
	key: &str,
	mut change: impl FnMut(&mut dyn toml_edit::TableLike),
) {
	let Some(entries) = owner.get_mut(key) else {
		return;
	};
	if let Some(tables) = entries.as_array_of_tables_mut() {
		tables.iter_mut().for_each(|table| change(table));
	} else if let Some(array) = entries.as_array_mut() {
		array
			.iter_mut()
			.filter_map(toml_edit::Value::as_inline_table_mut)
			.for_each(|table| change(table));
	}
}

fn item_text(item: &toml_edit::Item) -> String {
	item.as_value()
		.map_or_else(|| item.to_string(), |value| value.to_string().trim().to_string())
}

/// v35: rewrite the retired community agent tags to the agents that replaced them, in the same
/// places as v30: the root `default` and the `[taps]` keys, and in the role filter of a plugin and
/// `auto_bind` of an MCP server. Any other tag, `:` or not, stays as written. Where a `[taps]` key
/// of the current agent exists already it keeps its value; where several retired keys name one
/// agent, the first in file order is kept. One line per rewritten tag says what changed, and one
/// line per dropped key with a different model names the kept key and the dropped ones.
fn rewrite_v35_retired_agent_tags(document: &mut toml_edit::DocumentMut) -> Result<Vec<String>> {
	let mut notices = Vec::new();
	if let Some(taps) = document
		.as_table_mut()
		.get_mut("taps")
		.map(|item| item.as_table_like_mut().context("taps must be a table"))
		.transpose()?
	{
		let keys: Vec<String> = taps.iter().map(|(key, _)| key.to_string()).collect();
		let mut dropped: Vec<(&str, Vec<&str>)> = Vec::new();
		for key in &keys {
			let Some(current) = current_agent_tag(key) else {
				continue;
			};
			note_rewrite(&mut notices, key, current);
			let item = taps.remove(key).context("the retired key is in taps")?;
			match taps.get(current).map(item_text) {
				None => {
					taps.insert(current, item);
				}
				Some(kept_value) if kept_value != item_text(&item) => {
					match dropped.iter_mut().find(|(kept, _)| *kept == current) {
						Some((_, keys)) => keys.push(key.as_str()),
						None => dropped.push((current, vec![key.as_str()])),
					}
				}
				Some(_) => {}
			}
		}
		for (kept, keys) in dropped {
			let kept_value = taps.get(kept).map(item_text).unwrap_or_default();
			notices.push(format!(
				"[taps]: kept '{kept}' = {kept_value}; dropped {} with a different model.",
				keys.iter().map(|key| format!("'{key}'")).collect::<Vec<_>>().join(", ")
			));
		}
	}
	if let Some(default) = document
		.as_table_mut()
		.get_mut("default")
		.and_then(|item| item.as_value_mut())
	{
		if let Some((tag, current)) = default
			.as_str()
			.and_then(|tag| current_agent_tag(tag).map(|current| (tag.to_string(), current)))
		{
			note_rewrite(&mut notices, &tag, current);
			// Keep the user's trailing comment on the line.
			let decor = default.decor().clone();
			*default = toml_edit::Value::from(current);
			*default.decor_mut() = decor;
		}
	}
	for_each_table_of(document.as_table_mut(), "plugins", |plugin| {
		rewrite_tag_list(plugin, "roles", &mut notices)
	});
	if let Some(mcp) = document
		.as_table_mut()
		.get_mut("mcp")
		.and_then(toml_edit::Item::as_table_like_mut)
	{
		for_each_table_of(mcp, "servers", |server| {
			rewrite_tag_list(server, "auto_bind", &mut notices)
		});
	}
	Ok(notices)
}

/// Add the `mcp.code_mode` switch (on: external tools go through the `code` tool); an existing value is kept.
fn add_v33_code_mode(document: &mut toml_edit::DocumentMut, template: &toml_edit::DocumentMut) -> Result<()> {
	let template_mcp = required_table(template.as_table(), "mcp", "embedded config")?;
	let mcp = ensure_table(document.as_table_mut(), template.as_table(), "mcp", "user config")?;
	merge_missing(mcp, template_mcp, "code_mode")
}

fn remove_v34_sampling(document: &mut toml_edit::DocumentMut, _template: &toml_edit::DocumentMut) -> Result<()> {
	remove_sampling_from_model_tables(document.as_table_mut());
	Ok(())
}

fn remove_sampling_from_model_tables(table: &mut toml_edit::Table) {
	remove_model_sampling(table);

	if let Some(compression) = table
		.get_mut("compression")
		.and_then(toml_edit::Item::as_table_like_mut)
	{
		remove_model_sampling(compression);
	}

	if let Some(roles) = table.get_mut("roles") {
		if let Some(roles) = roles.as_array_of_tables_mut() {
			for role in roles.iter_mut() {
				remove_role_sampling(role);
			}
		} else if let Some(roles) = roles.as_array_mut() {
			for role in roles.iter_mut().filter_map(toml_edit::Value::as_inline_table_mut) {
				remove_role_sampling(role);
			}
		}
	}
}

fn remove_role_sampling(role: &mut dyn toml_edit::TableLike) {
	remove_sampling_keys(role);
	remove_model_sampling(role);
}

fn remove_model_sampling(owner: &mut dyn toml_edit::TableLike) {
	if let Some(model) = owner.get_mut("model").and_then(toml_edit::Item::as_table_like_mut) {
		remove_sampling_keys(model);
	}
}

fn remove_sampling_keys(table: &mut dyn toml_edit::TableLike) {
	for key in super::DEPRECATED_SAMPLING_KEYS {
		table.remove(key);
	}
}

/// Add the required `compression.idle` switch (on); an existing value is kept.
fn add_v28_idle_compaction(document: &mut toml_edit::DocumentMut, template: &toml_edit::DocumentMut) -> Result<()> {
	let template_compression = required_table(template.as_table(), "compression", "embedded config")?;
	let compression = ensure_table(
		document.as_table_mut(),
		template.as_table(),
		"compression",
		"user config",
	)?;
	merge_missing(compression, template_compression, "idle")
}

fn add_v26_model_timeouts_and_transport(
	document: &mut toml_edit::DocumentMut,
	template: &toml_edit::DocumentMut,
) -> Result<()> {
	let template_model = required_table(template.as_table(), "model", "embedded config")?;
	let model = ensure_table(document.as_table_mut(), template.as_table(), "model", "user config")?;
	for key in ["stream_timeout_seconds", "request_transport"] {
		merge_missing(model, template_model, key)?;
	}
	Ok(())
}

/// Add the required root `commit_attribution` trailer; an existing value is kept.
fn add_v23_commit_attribution(document: &mut toml_edit::DocumentMut, template: &toml_edit::DocumentMut) -> Result<()> {
	merge_missing(document.as_table_mut(), template.as_table(), "commit_attribution")
}

/// Add the required `[timing]` calibration section for `/report`.
fn add_v20_timing(document: &mut toml_edit::DocumentMut, template: &toml_edit::DocumentMut) -> Result<()> {
	merge_missing(document.as_table_mut(), template.as_table(), "timing")
}

/// v12 nests one validated model profile under each actual model owner. Main is
/// the baseline; roles inherit it, and compression remains a separate override.
fn unify_v12_model_profiles(document: &mut toml_edit::DocumentMut, template: &toml_edit::DocumentMut) -> Result<()> {
	const PROFILE_FIELDS: [&str; 6] = [
		"reasoning_effort",
		"max_tokens",
		"max_retries",
		"retry_timeout",
		"request_timeout_seconds",
		"stream_timeout_seconds",
	];

	let template_main = required_table(template.as_table(), "model", "embedded default configuration")?;
	let mut main = toml_edit::Table::new();
	if let Some(value) = document.as_table_mut().remove("model") {
		match value.into_table() {
			Ok(table) => main = table,
			Err(value) => {
				main.insert("name", value);
			}
		}
	}
	for key in PROFILE_FIELDS.into_iter().chain(super::DEPRECATED_SAMPLING_KEYS) {
		if let Some(value) = document.as_table_mut().remove(key) {
			main.insert(key, value);
		}
	}
	document.as_table_mut().insert("model", toml_edit::Item::Table(main));
	let main = document
		.as_table_mut()
		.get_mut("model")
		.and_then(|item| item.as_table_mut())
		.expect("model table inserted above");
	for key in ["name"].into_iter().chain(PROFILE_FIELDS) {
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
			for key in super::DEPRECATED_SAMPLING_KEYS {
				if let Some(value) = role.remove(key) {
					profile.insert(key, value);
				}
			}
			if !profile.is_empty() {
				role.insert("model", toml_edit::Item::Table(profile));
			}
		}
	}

	let template_compression = required_table(template.as_table(), "compression", "embedded default configuration")?;
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

/// v3 adds required budgets for retained compression findings and the PACT
/// attention controller. Existing values and comments are preserved; only
/// missing keys are copied from the embedded template. (Its detector-advisory
/// keys were removed again in v7.)
fn add_v3_required_fields(document: &mut toml_edit::DocumentMut, template: &toml_edit::DocumentMut) -> Result<()> {
	let template_compression = required_table(template.as_table(), "compression", "embedded default configuration")?;
	let compression = ensure_table(
		document.as_table_mut(),
		template.as_table(),
		"compression",
		"user configuration",
	)?;

	merge_missing(compression, template_compression, "analysis_findings_max_tokens")?;
	merge_missing(compression, template_compression, "attention")
}

/// v4 collapses the `[[compression.pressure_levels]]` ladder into the single
/// adaptive `compression.threshold` trigger. The lowest configured level was
/// the point where compression became eligible, so its threshold carries over;
/// depth is computed at runtime now and the ratio ladder is dropped.
fn collapse_pressure_levels(document: &mut toml_edit::DocumentMut, template: &toml_edit::DocumentMut) -> Result<()> {
	let template_compression = required_table(template.as_table(), "compression", "embedded default configuration")?;
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
	let content = fs::read_to_string(config_path).context("Failed to read config file for version check")?;

	// Pre-check outside the lock; the authoritative one runs under it.
	if plan().version_of(&content)? == plan().target_version(DEFAULT_CONFIG_TEMPLATE)? {
		return Ok(false);
	}

	config_file::with_lock(config_path, || upgrade_locked(config_path, false))
}

/// `octomind config --upgrade`: same upgrade, but a missing file is an error
/// and an already-current file reports success instead of staying silent.
pub fn force_upgrade_config(config_path: &Path) -> Result<()> {
	if !config_path.exists() {
		return Err(anyhow::anyhow!("Config file not found: {}", config_path.display()));
	}

	config_file::with_lock(config_path, || upgrade_locked(config_path, true))?;
	Ok(())
}

/// The migration proper. Must be called holding the config lock: another
/// process may have upgraded the file between our pre-check and here, so the
/// content is re-read rather than passed in.
fn upgrade_locked(config_path: &Path, report_up_to_date: bool) -> Result<bool> {
	let original = fs::read_to_string(config_path).context("Failed to read config file")?;

	let Some(upgrade) = upgrade(&original).with_context(|| format!("Upgrading {}", config_path.display()))? else {
		if report_up_to_date {
			preflight_v36(config_path, &original, None)?;
			let version = plan().version_of(&original)?;
			eprintln!("✅ Config is already at the latest version ({version})");
		}
		return Ok(false);
	};
	let mut migration = upgrade.migration;
	let siblings = preflight_v36(config_path, &original, Some(&mut migration))?;

	// stderr, never stdout: an outdated config is upgraded during startup, and
	// ACP/MCP stdio modes carry JSON-RPC on stdout.
	eprintln!(
		"🔄 Upgrading config from version {} to {}...",
		migration.from_version, migration.to_version
	);

	// Never replace the user's file with something that no longer parses.
	toml::from_str::<toml::Value>(&migration.content)
		.context("Migrated config is not valid TOML - aborting upgrade")?;

	// Before the config is replaced: if the process dies in between, the old
	// config is still there and the next start writes the same file again.
	if let Some(settings) = &upgrade.carried_supervisor_settings {
		let carry_path = save_supervisor_settings(config_path, settings)?;
		eprintln!(
			"ℹ️ The built-in supervisor's settings were saved to {} for the Besydr plugin",
			carry_path.display()
		);
	}

	for (path, before, change) in siblings {
		publish_checked(&path, &before, &change)?;
	}
	let backup_path = publish_checked(config_path, &original, &migration)?;

	eprintln!(
		"✅ Config upgraded successfully! Backup saved to: {}",
		backup_path.display()
	);

	Ok(true)
}

/// The version that rewrites the retired community agent tags (see `RETIRED_AGENT_TAGS`).
const RETIRED_AGENT_TAGS_VERSION: u32 = 35;

type FragmentChange = (PathBuf, String, Migration);

fn publish_checked(path: &Path, before: &str, change: &Migration) -> Result<PathBuf> {
	if fs::read_to_string(path)? != before {
		anyhow::bail!("{} changed during migration; retry", path.display());
	}
	config_file::apply_migration(path, before.as_bytes(), change)
		.with_context(|| format!("Upgrading {}", path.display()))
}

/// Inspect all originals first. Removing a winning builtin must never reveal a
/// previously shadowed executable; a winning external override keeps its refs.
fn preflight_v36(config_path: &Path, original: &str, primary: Option<&mut Migration>) -> Result<Vec<FragmentChange>> {
	let mut files = super::loading::config_files(config_file::parent_directory(config_path)?)?;
	if !files.iter().any(|p| p.file_name() == config_path.file_name()) {
		files.insert(0, config_path.to_owned());
	}
	let mut sources = Vec::new();
	let mut external = None;
	let mut winner = None;
	for path in files {
		let main = path.file_name() == config_path.file_name();
		let text = if main {
			original.to_owned()
		} else {
			fs::read_to_string(&path).with_context(|| format!("Reading {}", path.display()))?
		};
		let value: toml::Value = toml::from_str(&text).with_context(|| format!("Parsing {}", path.display()))?;
		check_v36_agents(&value).with_context(|| format!("{}: legacy agent configuration", path.display()))?;
		let mut has_builtin = false;
		if let Some(servers) = value
			.get("mcp")
			.and_then(|m| m.get("servers"))
			.and_then(toml::Value::as_array)
		{
			for server in servers {
				if server.get("name").and_then(toml::Value::as_str) == Some("agent") {
					let builtin = server.get("type").and_then(toml::Value::as_str) == Some("builtin");
					has_builtin |= builtin;
					if !builtin {
						external = Some(path.clone());
					}
					winner = Some((path.clone(), builtin));
				}
			}
		}
		sources.push((path, text, main, has_builtin));
	}
	let Some(primary) = primary else {
		return Ok(Vec::new());
	};
	let retire_refs = matches!(&winner, Some((_, true)));
	if let (Some(hidden), Some((active, true))) = (external, &winner) {
		anyhow::bail!(
			"{}: removing builtin 'agent' would activate a shadowed server from {}. \
			 Rename the external server and review its references before retrying.",
			active.display(),
			hidden.display()
		);
	}

	// Publish reference-only siblings before removing the declarations they reference.
	sources.sort_by_key(|(_, _, _, has_builtin)| *has_builtin);
	let mut changes = Vec::new();
	for (path, text, main, _) in sources {
		let input = if main { &primary.content } else { &text };
		let mut document: toml_edit::DocumentMut = input.parse()?;
		if !main {
			if primary.from_version < RETIRED_AGENT_TAGS_VERSION {
				for notice in rewrite_v35_retired_agent_tags(&mut document)? {
					eprintln!("{}: {notice}", path.display());
				}
			}
			remove_v36_agents(&mut document)?;
		}
		retire_v36_server(&mut document, retire_refs);
		let content = document.to_string();
		if content == *input {
			continue;
		}
		let _: toml::Value =
			toml::from_str(&content).with_context(|| format!("Invalid migrated TOML for {}", path.display()))?;
		if main {
			primary.content = content;
		} else {
			let change = Migration {
				content,
				from_version: primary.from_version,
				to_version: primary.to_version,
			};
			changes.push((path, text, change));
		}
	}
	for path in changes
		.iter()
		.map(|(p, _, _)| p.as_path())
		.chain(std::iter::once(config_path))
	{
		let meta = fs::symlink_metadata(path)?;
		if meta.is_symlink() || meta.permissions().readonly() {
			anyhow::bail!(
				"{}: not replacing a symlink or read-only config; migrate its writable source",
				path.display()
			);
		}
	}
	Ok(changes)
}

/// Write the supervisor's tables where the plugin looks for them on its first run.
fn save_supervisor_settings(config_path: &Path, settings: &str) -> Result<PathBuf> {
	let carry_dir = config_path
		.parent()
		.context("The config file has no parent directory")?
		.join(CARRY_DIR_NAME);
	fs::create_dir_all(&carry_dir).with_context(|| format!("Failed to create {}", carry_dir.display()))?;
	let carry_path = carry_dir.join(CARRY_FILE_NAME);
	fs::write(&carry_path, settings).with_context(|| format!("Failed to write {}", carry_path.display()))?;
	Ok(carry_path)
}

#[cfg(test)]
#[path = "migrations_inline_tests.rs"]
mod inline_tests;

#[cfg(test)]
#[path = "migrations_tests.rs"]
mod migrations_tests;
