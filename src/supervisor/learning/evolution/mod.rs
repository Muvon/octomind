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

//! Grounded behavior evolution.
//!
//! Learning remains the evidence authority. This module turns newly grounded
//! memories into native skill or guardrail candidates, manages their
//! candidate/shadow/trial/active lifecycle, and exposes matching artifacts to
//! the existing runtimes. Generated text is never executed directly.

mod registry;
mod runtime;
mod synthesize;

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[cfg(test)]
pub(crate) use registry::create_record;
pub use registry::{get_record, list_records, mutate_record};
pub use runtime::{
	active_skill_dirs, all_skill_bindings, behavior_available, binding_is_shadow,
	clear_for_session, generated_guardrails, init_for_session, mark_behavior_used,
	mark_shadow_match, reinforce_session, skill_binding, SkillBinding,
};

/// The only user-facing evolution knob. Models, evidence thresholds, trial
/// limits, and storage are inherited or fixed to avoid a second config system.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EvolutionConfig {
	pub enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
	Skill,
	Pipe,
	Guard,
	Hook,
	Validator,
}

impl ArtifactKind {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Skill => "skill",
			Self::Pipe => "pipe",
			Self::Guard => "guard",
			Self::Hook => "hook",
			Self::Validator => "validator",
		}
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvolutionState {
	Candidate,
	Shadow,
	Trial,
	Active,
	Rejected,
	Retired,
}

impl EvolutionState {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Candidate => "candidate",
			Self::Shadow => "shadow",
			Self::Trial => "trial",
			Self::Active => "active",
			Self::Rejected => "rejected",
			Self::Retired => "retired",
		}
	}

	pub fn affects_runtime(self) -> bool {
		matches!(self, Self::Trial | Self::Active)
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectClass {
	Advisory,
	Observational,
	Effectful,
}

/// Two independent scope dimensions. `None` means all projects/domains.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactScope {
	pub project: Option<String>,
	pub domain: Option<String>,
}

impl ArtifactScope {
	pub fn matches(&self, project: &str, domain: &str) -> bool {
		self.project.as_deref().is_none_or(|value| value == project)
			&& self.domain.as_deref().is_none_or(|value| value == domain)
	}
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeneratedScript {
	pub file_name: String,
	pub content: String,
}

/// Synthetic trigger case proposed with an artifact. These screen trigger
/// breadth and boundaries but never count as live utility or outcome credit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayCase {
	pub label: String,
	pub input: String,
	pub expected_match: bool,
	pub boundary: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEvent {
	pub at: String,
	pub event: String,
	pub detail: String,
}

/// Durable registry record. The generated native artifact is stored beside
/// this metadata; `artifact_path` is relative to the record directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvolutionRecord {
	pub schema_version: u32,
	pub id: String,
	pub name: String,
	pub description: String,
	pub kind: ArtifactKind,
	pub scope: ArtifactScope,
	pub state: EvolutionState,
	pub effect: EffectClass,
	pub explicit_authorization: bool,
	pub source_memory_ids: Vec<String>,
	pub evidence: Vec<String>,
	#[serde(default)]
	pub replay_cases: Vec<ReplayCase>,
	pub artifact_version: u32,
	pub parent_version: Option<String>,
	pub superseded_ids: Vec<String>,
	pub generator_model: String,
	pub verifier_model: String,
	pub artifact_path: String,
	pub script_path: Option<String>,
	pub shadow_matches: u32,
	pub trial_uses: u32,
	pub successes: u32,
	pub failures: u32,
	pub false_triggers: u32,
	pub created: String,
	pub updated: String,
	pub promoted: Option<String>,
	pub last_used: Option<String>,
	pub retired: Option<String>,
	pub history: Vec<HistoryEvent>,
}

impl EvolutionRecord {
	pub fn artifact_dir(&self) -> anyhow::Result<PathBuf> {
		Ok(crate::directories::get_learning_evolution_dir()?
			.join(&self.id)
			.join("artifact"))
	}

	pub fn native_path(&self) -> anyhow::Result<PathBuf> {
		Ok(self.artifact_dir()?.join(&self.artifact_path))
	}
}

pub const REGISTRY_SCHEMA_VERSION: u32 = 1;
pub const SHADOW_MATCHES_REQUIRED: u32 = 2;
pub const TRIAL_SUCCESSES_REQUIRED: u32 = 2;
pub const TRIAL_FAILURE_LIMIT: u32 = 1;
pub const TRIAL_MAX_USES: u32 = 4;

/// Existing learning project identity: current working-directory basename.
/// Keeping this in one function prevents evolution from inventing a parallel
/// scope key while the broader learning store still uses this contract.
pub fn project_name(current_dir: Option<&Path>) -> String {
	let owned;
	let dir = match current_dir {
		Some(path) => Some(path),
		None => {
			owned = std::env::current_dir().ok();
			owned.as_deref()
		}
	};
	dir.and_then(Path::file_name)
		.and_then(|name| name.to_str())
		.map(String::from)
		.unwrap_or_else(|| "unknown".to_string())
}

pub fn domain_name(role: &str) -> String {
	role.split(':').next().unwrap_or(role).to_string()
}

/// Called only by the canonical extraction worker after it stored new memory.
pub async fn synthesize_after_extraction(
	messages: &[crate::session::Message],
	config: &crate::config::Config,
	role: &str,
	project: &str,
	session_name: &str,
) -> anyhow::Result<Option<String>> {
	if !config.supervisor.learning.evolution.enabled {
		return Ok(None);
	}
	synthesize::synthesize(messages, config, role, project, session_name).await
}

/// Human/structured command representation of a record without exposing raw
/// script bodies in list output.
pub fn record_summary(record: &EvolutionRecord) -> serde_json::Value {
	serde_json::json!({
		"id": record.id,
		"name": record.name,
		"description": record.description,
		"kind": record.kind.as_str(),
		"state": record.state.as_str(),
		"scope": record.scope,
		"effect": record.effect,
		"authorized": record.explicit_authorization,
		"artifact_version": record.artifact_version,
		"shadow_matches": record.shadow_matches,
		"trial_uses": record.trial_uses,
		"successes": record.successes,
		"failures": record.failures,
		"false_triggers": record.false_triggers,
		"replay_cases": record.replay_cases.len(),
		"created": record.created,
		"updated": record.updated,
	})
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
