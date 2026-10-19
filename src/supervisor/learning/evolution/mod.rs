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
//! memories into native skill, guardrail, or guidance candidates, manages their
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
	active_skill_dirs, all_skill_bindings, applied_skill_names, binding_is_shadow,
	clear_for_session, deliver_guidance, generated_guardrails, init_for_session,
	mark_behavior_used, mark_shadow_match, mark_skill_activated, reinforce_session, skill_binding,
	SkillBinding, TurnVerdict,
};

/// The only user-facing evolution knob. Models, evidence thresholds, trial
/// limits, and storage are inherited or fixed to avoid a second config system.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EvolutionConfig {
	pub enabled: bool,
	/// Verify-gate verdicts required in each arm (concurrent control,
	/// treatment) before the two pass rates are compared.
	pub min_samples: u32,
	/// Smallest pass-rate gain worth promoting, and the non-inferiority bound
	/// a cheaper behavior must hold.
	pub noise_margin: f64,
	/// Relative increase in API calls per turn tolerated at a negligible gain,
	/// and the reduction that counts as "cheaper" at an unchanged pass rate.
	pub cost_allowance: f64,
	/// Additional relative API-call increase allowed per unit of pass-rate gain.
	pub cost_per_gain: f64,
	/// Live uses after which a trial without a decision retires as inconclusive.
	pub max_trial_uses: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
	Skill,
	Pipe,
	Guard,
	Hook,
	Validator,
	/// An always-on rule for its scope, delivered once per session: the
	/// cheapest fix for a recurring misstep, which a triggered skill cannot
	/// express because it has no separating trigger.
	Guidance,
}

impl ArtifactKind {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Skill => "skill",
			Self::Pipe => "pipe",
			Self::Guard => "guard",
			Self::Hook => "hook",
			Self::Validator => "validator",
			Self::Guidance => "guidance",
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
/// How an artifact's samples are scored. Fixed by its first sample so the two
/// arms of one comparison are never measured differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Measure {
	/// The verify-gate verdict: 1 on pass, 0 otherwise.
	Verdict,
	/// The evaluation seam's probability that the turn fulfilled the request,
	/// on turns where it also judged the artifact applicable.
	Graded,
}

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

	/// Whether some session can bind both scopes at once.
	pub fn overlaps(&self, other: &ArtifactScope) -> bool {
		fn compatible(left: Option<&str>, right: Option<&str>) -> bool {
			left.is_none() || right.is_none() || left == right
		}
		compatible(self.project.as_deref(), other.project.as_deref())
			&& compatible(self.domain.as_deref(), other.domain.as_deref())
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
	/// Verify-gate passes in the control arm: shadow-phase matches before the
	/// trial, then trial sessions randomized to control. Reset when the trial
	/// opens so the comparison uses concurrent samples only.
	#[serde(default)]
	pub control_successes: u32,
	#[serde(default)]
	pub control_failures: u32,
	/// API calls summed over the control turns above.
	#[serde(default)]
	pub control_calls: u64,
	/// API calls summed over the verdict-bearing live uses (`successes` + `failures`).
	#[serde(default)]
	pub treatment_calls: u64,
	/// Sum and sum of squares of ln(API calls) per control turn: the cost test
	/// compares geometric means, robust to the heavy tail of per-turn calls.
	#[serde(default)]
	pub control_log_calls: f64,
	#[serde(default)]
	pub control_log_calls_sq: f64,
	/// Sum and sum of squares of ln(API calls) per treatment turn.
	#[serde(default)]
	pub treatment_log_calls: f64,
	#[serde(default)]
	pub treatment_log_calls_sq: f64,
	/// Unset until the first sample of either arm.
	#[serde(default)]
	pub measure: Option<Measure>,
	/// Graded outcome summed over the control samples (unused under `Verdict`).
	#[serde(default)]
	pub control_score: f64,
	/// Graded outcome summed over the treatment samples (unused under `Verdict`).
	#[serde(default)]
	pub treatment_score: f64,
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

	/// Drop the live arm's evidence: a rolled-back or re-validated behavior
	/// starts its next trial from no treatment samples.
	pub fn clear_treatment_arm(&mut self) {
		self.trial_uses = 0;
		self.successes = 0;
		self.failures = 0;
		self.treatment_calls = 0;
		self.treatment_score = 0.0;
		self.treatment_log_calls = 0.0;
		self.treatment_log_calls_sq = 0.0;
	}

	/// Drop the control arm's evidence: a trial compares against concurrent
	/// control samples only, never against an earlier period.
	pub fn clear_control_arm(&mut self) {
		self.control_successes = 0;
		self.control_failures = 0;
		self.control_calls = 0;
		self.control_score = 0.0;
		self.control_log_calls = 0.0;
		self.control_log_calls_sq = 0.0;
	}
}

/// Version 2 replaced sequential shadow-then-trial comparison with randomized
/// concurrent arms and a posterior test; version 1 trials and promotions were
/// decided by the old rule and are re-validated on load (see the registry).
pub const REGISTRY_SCHEMA_VERSION: u32 = 2;
/// Cross-store synthesis scans and embeds the whole hot store, so detached
/// extraction runs it at most once per interval; the command bypasses it.
const STORE_SYNTHESIS_INTERVAL: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);
const STORE_SYNTHESIS_STAMP: &str = "store-synthesis.stamp";

/// Existing learning project identity: the basename of the repository root that
/// contains the working directory, else the directory's own basename. Keeping
/// this in one function prevents evolution from inventing a parallel
/// scope key while the broader learning store still uses this contract.
/// Without an explicit dir it is the session's workdir — the one recall keys
/// on — not the process cwd: an ACP/WebSocket daemon serves sessions rooted
/// elsewhere, and storing under the daemon's cwd strands every lesson.
/// A session started in a subdirectory (`repo/src`) keys on the repository, so
/// its lessons are the repository's lessons rather than a store named `src`.
pub fn project_name(current_dir: Option<&Path>) -> String {
	let owned;
	let dir = match current_dir {
		Some(path) => path,
		None => {
			owned = crate::mcp::get_thread_working_directory();
			owned.as_path()
		}
	};
	project_root(dir)
		.file_name()
		.and_then(|name| name.to_str())
		.map(String::from)
		.unwrap_or_else(|| "unknown".to_string())
}

/// Nearest ancestor holding `.git` (a directory, or the file a worktree or
/// submodule uses). The walk stops below the home directory: a dotfiles
/// repository at `~` must not fold every unversioned project into one key.
fn project_root(dir: &Path) -> &Path {
	let home = dirs::home_dir();
	dir.ancestors()
		.take_while(|ancestor| home.as_deref() != Some(*ancestor))
		.find(|ancestor| ancestor.join(".git").exists())
		.unwrap_or(dir)
}

pub fn domain_name(role: &str) -> String {
	role.split(':').next().unwrap_or(role).to_string()
}

/// Existing artifacts applied in a session, read from the markers their
/// delivery leaves: a guidance note's per-rule tag, an evolved skill's
/// injection. Shadow behavior is never delivered, so a marker means treatment.
/// Pipes, guards, hooks, and validators leave no marker and are not listed.
pub fn live_in_session(
	messages: &[crate::session::Message],
	records: &[EvolutionRecord],
) -> Vec<String> {
	records
		.iter()
		.filter(|record| {
			let marker = match record.kind {
				ArtifactKind::Guidance => format!("<guidance id=\"{}\">", record.id),
				ArtifactKind::Skill => format!("<skill name=\"{}\"", record.name),
				_ => return false,
			};
			messages
				.iter()
				.any(|message| message.role == "user" && message.content.contains(&marker))
		})
		.map(|record| record.id.clone())
		.collect()
}

/// Called only by the canonical extraction worker after it stored new memory.
pub async fn synthesize_after_extraction(
	messages: &[crate::session::Message],
	config: &crate::config::Config,
	role: &str,
	project: &str,
	session_name: &str,
) -> anyhow::Result<Vec<String>> {
	if !config.supervisor.learning.evolution.enabled {
		return Ok(Vec::new());
	}
	synthesize::synthesize(messages, config, role, project, session_name).await
}

/// Called by the canonical extraction worker after maintenance. Stamps before
/// running so a concurrent extraction cannot start a second scan.
pub async fn synthesize_store_if_due(
	config: &crate::config::Config,
	role: &str,
	project: &str,
) -> anyhow::Result<Vec<String>> {
	if !config.supervisor.learning.evolution.enabled {
		return Ok(Vec::new());
	}
	let stamp = store_synthesis_stamp()?;
	let fresh = std::fs::metadata(&stamp)
		.and_then(|metadata| metadata.modified())
		.ok()
		.and_then(|modified| modified.elapsed().ok())
		.is_some_and(|age| age < STORE_SYNTHESIS_INTERVAL);
	if fresh {
		return Ok(Vec::new());
	}
	std::fs::write(&stamp, chrono::Utc::now().to_rfc3339())?;
	retire_stale()?;
	synthesize::synthesize_store(config, role, project).await
}

/// Retire live-lifecycle artifacts with no activity (trigger match or use)
/// for `DECAY_DAYS`: an artifact must keep earning its place in the harness.
fn retire_stale() -> anyhow::Result<()> {
	let horizon = chrono::Duration::days(super::DECAY_DAYS as i64);
	let now = chrono::Utc::now();
	for record in registry::list_records()? {
		let idle = chrono::DateTime::parse_from_rfc3339(&record.updated)
			.is_ok_and(|updated| now - updated.with_timezone(&chrono::Utc) > horizon);
		if !idle
			|| !matches!(
				record.state,
				EvolutionState::Shadow | EvolutionState::Trial | EvolutionState::Active
			) {
			continue;
		}
		let retired = registry::mutate_record(&record.id, |record| {
			record.state = EvolutionState::Retired;
			record.retired = Some(chrono::Utc::now().to_rfc3339());
			registry::append_history(
				record,
				"stale",
				format!("no trigger match or use for {} days", super::DECAY_DAYS),
			);
			Ok(())
		})?;
		runtime::emit_lifecycle(&retired, "stale");
	}
	Ok(())
}

/// `/learning evolution distill`: run the cross-store pass now.
pub async fn synthesize_store_now(
	config: &crate::config::Config,
	role: &str,
	project: &str,
) -> anyhow::Result<Vec<String>> {
	if !config.supervisor.learning.evolution.enabled {
		anyhow::bail!("[supervisor.learning.evolution] is disabled");
	}
	std::fs::write(store_synthesis_stamp()?, chrono::Utc::now().to_rfc3339())?;
	synthesize::synthesize_store(config, role, project).await
}

fn store_synthesis_stamp() -> anyhow::Result<PathBuf> {
	Ok(crate::directories::get_learning_evolution_dir()?.join(STORE_SYNTHESIS_STAMP))
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
		"control_successes": record.control_successes,
		"control_failures": record.control_failures,
		"measure": record.measure,
		"reason": record.history.last().map(|event| event.detail.as_str()),
		"replay_cases": record.replay_cases.len(),
		"created": record.created,
		"updated": record.updated,
	})
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
