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

use super::{EvolutionRecord, GeneratedScript, REGISTRY_SCHEMA_VERSION};
use anyhow::{Context, Result};
use octolib::utils::config_file;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Registry {
	#[serde(default = "schema_version")]
	schema_version: u32,
	#[serde(default)]
	records: Vec<EvolutionRecord>,
}

const fn schema_version() -> u32 {
	REGISTRY_SCHEMA_VERSION
}

impl Default for Registry {
	fn default() -> Self {
		Self {
			schema_version: REGISTRY_SCHEMA_VERSION,
			records: Vec::new(),
		}
	}
}

fn registry_path() -> Result<PathBuf> {
	Ok(crate::directories::get_learning_evolution_dir()?.join("registry.json"))
}

fn load_from(path: &Path) -> Result<Registry> {
	let content = match fs::read_to_string(path) {
		Ok(content) => content,
		Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
			return Ok(Registry::default());
		}
		Err(error) => return Err(error.into()),
	};
	let registry: Registry = serde_json::from_str(&content)
		.with_context(|| format!("invalid evolution registry {}", path.display()))?;
	if registry.schema_version != REGISTRY_SCHEMA_VERSION {
		anyhow::bail!(
			"unsupported evolution registry schema {} (expected {})",
			registry.schema_version,
			REGISTRY_SCHEMA_VERSION
		);
	}
	Ok(registry)
}

fn persist(path: &Path, registry: &Registry) -> Result<()> {
	let bytes = serde_json::to_vec_pretty(registry)?;
	config_file::atomic_write(path, &bytes, None)?;
	for record in &registry.records {
		let record_path = crate::directories::get_learning_evolution_dir()?
			.join(&record.id)
			.join("record.json");
		config_file::atomic_write(&record_path, &serde_json::to_vec_pretty(record)?, None)?;
	}
	Ok(())
}

pub fn list_records() -> Result<Vec<EvolutionRecord>> {
	load_from(&registry_path()?).map(|registry| registry.records)
}

pub fn get_record(id: &str) -> Result<Option<EvolutionRecord>> {
	Ok(list_records()?.into_iter().find(|record| record.id == id))
}

pub fn create_record(
	record: EvolutionRecord,
	native_content: &str,
	script: Option<&GeneratedScript>,
) -> Result<()> {
	validate_relative_file(&record.artifact_path)?;
	if let Some(script) = script {
		validate_relative_file(&script.file_name)?;
	}
	let path = registry_path()?;
	config_file::with_lock(&path, || {
		let mut registry = load_from(&path)?;
		if registry
			.records
			.iter()
			.any(|existing| existing.id == record.id)
		{
			anyhow::bail!("evolution record '{}' already exists", record.id);
		}
		if registry.records.iter().any(|existing| {
			existing.name == record.name
				&& existing.scope == record.scope
				&& !matches!(
					existing.state,
					super::EvolutionState::Rejected | super::EvolutionState::Retired
				)
		}) {
			anyhow::bail!(
				"an evolution artifact named '{}' already exists in this scope",
				record.name
			);
		}

		let artifact_dir = crate::directories::get_learning_evolution_dir()?
			.join(&record.id)
			.join("artifact");
		fs::create_dir_all(&artifact_dir)?;
		config_file::atomic_write(
			&artifact_dir.join(&record.artifact_path),
			native_content.as_bytes(),
			None,
		)?;
		if let Some(script) = script {
			let script_path = artifact_dir.join(&script.file_name);
			config_file::atomic_write(&script_path, script.content.as_bytes(), None)?;
			make_executable(&script_path)?;
		}
		registry.records.push(record);
		persist(&path, &registry)
	})
}

pub fn mutate_record(
	id: &str,
	operation: impl FnOnce(&mut EvolutionRecord) -> Result<()>,
) -> Result<EvolutionRecord> {
	let path = registry_path()?;
	config_file::with_lock(&path, || {
		let mut registry = load_from(&path)?;
		let record = registry
			.records
			.iter_mut()
			.find(|record| record.id == id)
			.ok_or_else(|| anyhow::anyhow!("evolution record '{}' not found", id))?;
		operation(record)?;
		record.updated = chrono::Utc::now().to_rfc3339();
		let updated = record.clone();
		persist(&path, &registry)?;
		Ok(updated)
	})
}

pub fn append_history(record: &mut EvolutionRecord, event: &str, detail: impl Into<String>) {
	record.history.push(super::HistoryEvent {
		at: chrono::Utc::now().to_rfc3339(),
		event: event.to_string(),
		detail: detail.into(),
	});
}

fn validate_relative_file(value: &str) -> Result<()> {
	let path = Path::new(value);
	if value.trim().is_empty()
		|| path.is_absolute()
		|| path.components().count() != 1
		|| value == "."
		|| value == ".."
	{
		anyhow::bail!("invalid generated artifact file name '{}'", value);
	}
	Ok(())
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
	use std::os::unix::fs::PermissionsExt;
	let mut permissions = fs::metadata(path)?.permissions();
	permissions.set_mode(0o700);
	fs::set_permissions(path, permissions)?;
	Ok(())
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<()> {
	Ok(())
}
