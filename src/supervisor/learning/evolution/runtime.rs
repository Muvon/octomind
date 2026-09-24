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

use super::{
	domain_name, project_name, ArtifactKind, ArtifactScope, EffectClass, EvolutionConfig,
	EvolutionRecord, EvolutionState, Measure,
};
use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::RwLock;

#[derive(Debug, Clone)]
pub struct SkillBinding {
	pub id: String,
	pub shadow: bool,
	pub path: PathBuf,
}

static SESSION_SKILLS: RwLock<Option<HashMap<String, HashMap<String, SkillBinding>>>> =
	RwLock::new(None);
static SESSION_BEHAVIORS: RwLock<Option<HashMap<String, HashSet<String>>>> = RwLock::new(None);
/// Shadow artifacts whose trigger matched since the last verdict — the
/// control arm: the same situations, observed without the artifact applied.
static SESSION_SHADOWS: RwLock<Option<HashMap<String, HashSet<String>>>> = RwLock::new(None);
/// Evolved skills exposed in the session, keyed by id: `true` once a live
/// skill's trigger activated it, `false` once a shadow skill's trigger matched.
/// A live skill stays loaded after activation, so exposure is sticky in both
/// arms — every later verdict in the session is a sample, and neither arm is
/// filtered by the model's own report of what helped.
static SESSION_SKILL_EXPOSURE: RwLock<Option<HashMap<String, HashMap<String, bool>>>> =
	RwLock::new(None);

pub fn init_for_session(role: &str) {
	let Some(session_id) = crate::session::context::current_session_id() else {
		return;
	};
	let enabled = crate::session::context::get_session_config(&session_id)
		.is_some_and(|config| config.supervisor.learning.evolution.enabled);
	if !enabled {
		clear_for_session(&session_id);
		return;
	}
	let workdir = crate::session::context::get_current_workdir(&session_id)
		.or_else(|| std::env::current_dir().ok());
	let project = project_name(workdir.as_deref());
	let domain = domain_name(role);

	let records = match super::registry::list_records() {
		Ok(records) => records,
		Err(error) => {
			crate::log_error!(
				"evolution registry unavailable; generated behavior disabled: {}",
				error
			);
			clear_for_session(&session_id);
			return;
		}
	};
	let matching = records
		.into_iter()
		.filter(|record| record.scope.matches(&project, &domain))
		.filter(|record| {
			matches!(
				record.state,
				EvolutionState::Shadow | EvolutionState::Trial | EvolutionState::Active
			)
		})
		.collect::<Vec<_>>();

	let mut skills = HashMap::new();
	for record in matching
		.iter()
		.filter(|record| record.kind == ArtifactKind::Skill)
	{
		if let Ok(path) = record.artifact_dir() {
			skills.insert(
				record.name.clone(),
				SkillBinding {
					id: record.id.clone(),
					shadow: record.state == EvolutionState::Shadow,
					path,
				},
			);
		}
	}
	{
		let mut guard = SESSION_SKILLS.write().unwrap();
		guard
			.get_or_insert_with(HashMap::new)
			.insert(session_id.clone(), skills);
	}
	{
		let mut guard = SESSION_BEHAVIORS.write().unwrap();
		guard
			.get_or_insert_with(HashMap::new)
			.entry(session_id.clone())
			.or_default();
	}
	{
		let mut guard = SESSION_SHADOWS.write().unwrap();
		guard
			.get_or_insert_with(HashMap::new)
			.entry(session_id.clone())
			.or_default();
	}

	match generated_guardrails(&matching) {
		Ok(generated) => {
			crate::session::guardrails::merge_generated_for_session(&session_id, generated)
		}
		Err(error) => crate::log_error!(
			"generated guardrails unavailable; project guardrails preserved: {}",
			error
		),
	}
}

pub fn clear_for_session(session_id: &str) {
	if let Ok(mut guard) = SESSION_SKILLS.write() {
		if let Some(entries) = guard.as_mut() {
			entries.remove(session_id);
		}
	}
	if let Ok(mut guard) = SESSION_BEHAVIORS.write() {
		if let Some(entries) = guard.as_mut() {
			entries.remove(session_id);
		}
	}
	if let Ok(mut guard) = SESSION_SHADOWS.write() {
		if let Some(entries) = guard.as_mut() {
			entries.remove(session_id);
		}
	}
	if let Ok(mut guard) = SESSION_SKILL_EXPOSURE.write() {
		if let Some(entries) = guard.as_mut() {
			entries.remove(session_id);
		}
	}
}

pub fn active_skill_dirs() -> Vec<PathBuf> {
	let Some(session_id) = crate::session::context::current_session_id() else {
		return Vec::new();
	};
	SESSION_SKILLS
		.read()
		.ok()
		.and_then(|guard| guard.as_ref()?.get(&session_id).cloned())
		.map(|entries| {
			entries
				.into_values()
				.filter(|binding| !binding_is_shadow(&binding.id, binding.shadow))
				.map(|binding| binding.path)
				.collect()
		})
		.unwrap_or_default()
}

pub fn all_skill_bindings() -> Vec<(String, SkillBinding)> {
	let Some(session_id) = crate::session::context::current_session_id() else {
		return Vec::new();
	};
	SESSION_SKILLS
		.read()
		.ok()
		.and_then(|guard| guard.as_ref()?.get(&session_id).cloned())
		.map(|entries| entries.into_iter().collect())
		.unwrap_or_default()
}

pub fn skill_binding(name: &str) -> Option<SkillBinding> {
	let session_id = crate::session::context::current_session_id()?;
	SESSION_SKILLS
		.read()
		.ok()?
		.as_ref()?
		.get(&session_id)?
		.get(name)
		.cloned()
}

/// Snapshots never enable newly promoted behavior mid-task, but a durable
/// downgrade must take effect immediately. A compiled shadow stays shadow;
/// compiled trial/active behavior is suppressed when the registry has since
/// rolled it back, rejected it, or retired it.
pub fn binding_is_shadow(id: &str, compiled_shadow: bool) -> bool {
	compiled_shadow
		|| super::registry::get_record(id)
			.ok()
			.flatten()
			.is_none_or(|record| !record.state.affects_runtime())
}

pub fn generated_guardrails(
	records: &[EvolutionRecord],
) -> Result<crate::config::guardrails::Guardrails> {
	let mut output = crate::config::guardrails::Guardrails::default();
	let user_has_pipe = crate::session::context::current_session_id()
		.and_then(|id| crate::session::guardrails::get_rules(&id))
		.is_some_and(|rules| !rules.pipes.is_empty());
	for record in records
		.iter()
		.filter(|record| record.kind != ArtifactKind::Skill)
	{
		if record.kind == ArtifactKind::Pipe && user_has_pipe {
			crate::log_debug!(
				"generated pipe '{}' disabled because a user-authored pipe is active",
				record.id
			);
			continue;
		}
		let path = match record.native_path() {
			Ok(path) => path,
			Err(error) => {
				crate::log_error!("generated artifact '{}' path failed: {}", record.id, error);
				continue;
			}
		};
		let content = match std::fs::read_to_string(&path) {
			Ok(content) => content,
			Err(error) => {
				crate::log_error!(
					"generated artifact '{}' could not be read: {}",
					record.id,
					error
				);
				continue;
			}
		};
		let parsed = match crate::config::guardrails::Guardrails::parse(&content) {
			Ok(parsed) => parsed,
			Err(error) => {
				crate::log_error!(
					"generated artifact '{}' failed native parsing: {}",
					record.id,
					error
				);
				continue;
			}
		};
		output.append_generated(parsed, &record.id, record.state == EvolutionState::Shadow);
	}
	Ok(output)
}

pub fn mark_shadow_match(id: &str) {
	let Some(kind) = super::registry::get_record(id)
		.ok()
		.flatten()
		.filter(|record| record.state == EvolutionState::Shadow)
		.map(|record| record.kind)
	else {
		return;
	};
	let update = super::registry::mutate_record(id, |record| {
		if record.state != EvolutionState::Shadow {
			return Ok(());
		}
		record.shadow_matches = record.shadow_matches.saturating_add(1);
		super::registry::append_history(record, "shadow_match", "native trigger matched");
		Ok(())
	});
	crate::supervisor::stats::evolution("shadow_match");
	if update.is_err() {
		return;
	}
	let Some(session_id) = crate::session::context::current_session_id() else {
		return;
	};
	if kind == ArtifactKind::Skill {
		expose_skill(&session_id, id, false);
		return;
	}
	let mut guard = SESSION_SHADOWS.write().unwrap();
	guard
		.get_or_insert_with(HashMap::new)
		.entry(session_id)
		.or_default()
		.insert(id.to_string());
}

/// A live evolved skill was activated by its own trigger in this session.
pub fn mark_skill_activated(session_id: &str, id: &str) {
	expose_skill(session_id, id, true);
}

fn expose_skill(session_id: &str, id: &str, treated: bool) {
	let mut guard = SESSION_SKILL_EXPOSURE.write().unwrap();
	guard
		.get_or_insert_with(HashMap::new)
		.entry(session_id.to_string())
		.or_default()
		.insert(id.to_string(), treated);
}

pub fn mark_behavior_used(session_id: &str, id: &str) {
	let mut guard = SESSION_BEHAVIORS.write().unwrap();
	guard
		.get_or_insert_with(HashMap::new)
		.entry(session_id.to_string())
		.or_default()
		.insert(id.to_string());
}

/// Laplace-smoothed pass rate of one arm: a few samples stay near 0.5
/// instead of jumping to 0 or 1, so small samples cannot manufacture a large
/// gap. `successes + failures` counts samples under either measure; the hits
/// are the verdict passes or the summed graded outcome.
fn pass_rate(measure: Option<Measure>, successes: u32, failures: u32, score: f64) -> f64 {
	let hits = match measure {
		Some(Measure::Graded) => score,
		_ => successes as f64,
	};
	(hits + 1.0) / ((successes + failures) as f64 + 2.0)
}

/// Treatment measured against the shadow control of the same artifact.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Evidence {
	/// Treatment pass rate minus control pass rate.
	pub gain: f64,
	/// Relative change in API calls per verdict-bearing turn.
	pub cost: f64,
}

/// `None` until both arms hold `min_samples` verdicts.
pub(crate) fn evidence(record: &EvolutionRecord, policy: &EvolutionConfig) -> Option<Evidence> {
	let treated = record.successes + record.failures;
	let control = record.control_successes + record.control_failures;
	if treated < policy.min_samples || control < policy.min_samples || treated == 0 || control == 0
	{
		return None;
	}
	// A turn costs at least one call; flooring the baseline keeps the ratio finite.
	let control_calls = (record.control_calls as f64 / control as f64).max(1.0);
	let treatment_calls = record.treatment_calls as f64 / treated as f64;
	Some(Evidence {
		gain: pass_rate(
			record.measure,
			record.successes,
			record.failures,
			record.treatment_score,
		) - pass_rate(
			record.measure,
			record.control_successes,
			record.control_failures,
			record.control_score,
		),
		cost: (treatment_calls - control_calls) / control_calls,
	})
}

/// Admission clears the noise band at a cost the gain pays for, or holds the
/// pass rate within noise while measurably cutting cost.
pub(crate) fn admits(evidence: Evidence, policy: &EvolutionConfig) -> bool {
	(evidence.gain > policy.noise_margin
		&& evidence.cost <= policy.cost_allowance + policy.cost_per_gain * evidence.gain)
		|| (evidence.gain >= -policy.noise_margin && evidence.cost < -policy.cost_allowance)
}

/// Retention is admission with the margin relaxed to zero: once active, an
/// artifact stays while it still beats its control at all, so one unlucky
/// verdict near the promotion edge does not flap it out.
pub(crate) fn sustains(evidence: Evidence, policy: &EvolutionConfig) -> bool {
	(evidence.gain > 0.0
		&& evidence.cost <= policy.cost_allowance + policy.cost_per_gain * evidence.gain)
		|| (evidence.gain >= -policy.noise_margin && evidence.cost < -policy.cost_allowance)
}

fn describe(evidence: Evidence) -> String {
	format!(
		"pass rate {:+.2} vs shadow control, API calls {:+.0}%",
		evidence.gain,
		evidence.cost * 100.0
	)
}

/// One turn's outcome as the runtime saw it. `delta` carries the verify-gate
/// verdict by sign (zero = no verdict); `request` and `answer` are read only
/// by the evaluation seam.
pub struct TurnVerdict<'a> {
	pub delta: f64,
	pub api_calls: u32,
	pub request: &'a str,
	pub answer: &'a str,
}

/// The evaluation seam's reading of one turn.
struct TurnGrade {
	outcome: f64,
	applies: HashMap<String, bool>,
}

/// One sample for one artifact, or `None` when this turn does not count for
/// it. The first sample fixes the measure: graded while the seam is on,
/// verdict otherwise (and always for artifacts that already hold ungraded
/// samples). A graded artifact skips turns the evaluation could not answer
/// and turns where it judged the artifact inapplicable.
fn sample_score(
	record: &mut EvolutionRecord,
	passed: bool,
	seam_on: bool,
	grade: Option<&TurnGrade>,
) -> Option<f64> {
	let sampled =
		record.successes + record.failures + record.control_successes + record.control_failures;
	let measure = *record.measure.get_or_insert(if seam_on && sampled == 0 {
		Measure::Graded
	} else {
		Measure::Verdict
	});
	match measure {
		Measure::Verdict => Some(if passed { 1.0 } else { 0.0 }),
		Measure::Graded => {
			let grade = grade?;
			if !grade.applies.get(&record.id).copied().unwrap_or(false) {
				record.false_triggers = record.false_triggers.saturating_add(1);
				return None;
			}
			Some(grade.outcome)
		}
	}
}

/// One evaluation call per verdict turn that touched an artifact which is, or
/// will become, graded. `None` keeps graded artifacts from sampling this turn.
async fn grade_turn(
	config: &crate::supervisor::SupervisorConfig,
	turn: &TurnVerdict<'_>,
	ids: &[&String],
) -> Option<TurnGrade> {
	use crate::supervisor::evaluate::{self, Seam};
	let records = super::registry::list_records().ok()?;
	let touched = ids
		.iter()
		.filter_map(|id| records.iter().find(|record| &&record.id == id))
		.filter(|record| {
			record.measure == Some(Measure::Graded)
				|| (record.measure.is_none()
					&& record.successes
						+ record.failures + record.control_successes
						+ record.control_failures
						== 0)
		})
		.collect::<Vec<_>>();
	if touched.is_empty() {
		return None;
	}
	let state = serde_json::json!({
		"request": crate::session::truncate_to_tokens(turn.request, evaluate::EVOLUTION_REQUEST_TOKENS),
		"final_answer": turn.answer,
		"behaviors": touched
			.iter()
			.enumerate()
			.map(|(index, record)| serde_json::json!({
				"index": index,
				"name": record.name,
				"description": record.description,
			}))
			.collect::<Vec<_>>(),
	});
	let answers = evaluate::run(
		config,
		Seam::Evolution,
		state,
		evaluate::evolution_questions(touched.len()),
	)
	.await?;
	crate::supervisor::stats::evaluate_applied(Seam::Evolution, 1);
	Some(TurnGrade {
		outcome: evaluate::probability(&answers, evaluate::EVOLUTION_OUTCOME_ID),
		applies: touched
			.iter()
			.enumerate()
			.map(|(slot, record)| {
				(
					record.id.clone(),
					evaluate::probability(&answers, &evaluate::evolution_question_id(slot))
						>= evaluate::EVOLUTION_APPLIES_AT,
				)
			})
			.collect(),
	})
}

/// Credit one turn to the artifacts touched since the last one: used live
/// artifacts are the treatment arm, matched shadow artifacts the control arm,
/// and exposed evolved skills join their arm on every verdict turn.
pub async fn reinforce_session(
	session_id: &str,
	turn: &TurnVerdict<'_>,
	config: &crate::supervisor::SupervisorConfig,
) {
	let policy = &config.learning.evolution;
	let mut used = {
		let mut guard = SESSION_BEHAVIORS.write().unwrap();
		guard
			.as_mut()
			.and_then(|entries| entries.get_mut(session_id))
			.map(std::mem::take)
			.unwrap_or_default()
	};
	let mut shadowed = {
		let mut guard = SESSION_SHADOWS.write().unwrap();
		guard
			.as_mut()
			.and_then(|entries| entries.get_mut(session_id))
			.map(std::mem::take)
			.unwrap_or_default()
	};
	let passed = if turn.delta > 0.0 {
		Some(true)
	} else if turn.delta < 0.0 {
		Some(false)
	} else {
		None
	};
	if passed.is_some() {
		let exposed = SESSION_SKILL_EXPOSURE
			.read()
			.ok()
			.and_then(|guard| guard.as_ref()?.get(session_id).cloned())
			.unwrap_or_default();
		for (id, treated) in exposed {
			if treated {
				used.insert(id);
			} else {
				shadowed.insert(id);
			}
		}
	}
	let seam_on =
		crate::supervisor::evaluate::enabled(config, crate::supervisor::evaluate::Seam::Evolution);
	let grade = match passed {
		Some(_) if seam_on && !(used.is_empty() && shadowed.is_empty()) => {
			let ids = used.iter().chain(shadowed.iter()).collect::<Vec<_>>();
			grade_turn(config, turn, &ids).await
		}
		_ => None,
	};
	if let Some(passed) = passed {
		record_control(
			shadowed,
			passed,
			turn.api_calls,
			policy,
			seam_on,
			grade.as_ref(),
		);
	}
	for id in used {
		let result = super::registry::mutate_record(&id, |record| {
			record.last_used = Some(chrono::Utc::now().to_rfc3339());
			if record.state == EvolutionState::Trial {
				record.trial_uses = record.trial_uses.saturating_add(1);
			}
			if let Some(passed) = passed {
				if let Some(score) = sample_score(record, passed, seam_on, grade.as_ref()) {
					if passed {
						record.successes = record.successes.saturating_add(1);
					} else {
						record.failures = record.failures.saturating_add(1);
					}
					record.treatment_score += score;
					record.treatment_calls =
						record.treatment_calls.saturating_add(turn.api_calls as u64);
					super::registry::append_history(
						record,
						if passed { "success" } else { "failure" },
						format!(
							"outcome credit {}, score {:.2}, {} API calls",
							turn.delta, score, turn.api_calls
						),
					);
				}
			}
			let measured = evidence(record, policy);
			match record.state {
				EvolutionState::Trial => match measured {
					Some(evidence) if admits(evidence, policy) => {
						record.state = EvolutionState::Active;
						record.promoted = Some(chrono::Utc::now().to_rfc3339());
						super::registry::append_history(record, "promoted", describe(evidence));
					}
					Some(evidence) if evidence.gain < -policy.noise_margin => {
						record.state = EvolutionState::Retired;
						record.retired = Some(chrono::Utc::now().to_rfc3339());
						super::registry::append_history(record, "regressed", describe(evidence));
					}
					_ if record.trial_uses >= policy.max_trial_uses => {
						record.state = EvolutionState::Retired;
						record.retired = Some(chrono::Utc::now().to_rfc3339());
						super::registry::append_history(
							record,
							"trial_inconclusive",
							measured.map_or_else(
								|| "bounded trial ended without enough verdicts".to_string(),
								describe,
							),
						);
					}
					_ => {}
				},
				EvolutionState::Active => {
					if let Some(evidence) = measured.filter(|evidence| !sustains(*evidence, policy))
					{
						record.state = EvolutionState::Retired;
						record.retired = Some(chrono::Utc::now().to_rfc3339());
						super::registry::append_history(record, "pruned", describe(evidence));
					}
				}
				_ => {}
			}
			Ok(())
		});
		if let Ok(record) = result {
			let event = record.history.last().map(|event| event.event.as_str());
			if event == Some("promoted") {
				for old_id in &record.superseded_ids {
					let _ = super::registry::mutate_record(old_id, |old| {
						old.state = EvolutionState::Retired;
						old.retired = Some(chrono::Utc::now().to_rfc3339());
						super::registry::append_history(
							old,
							"retired",
							format!("superseded by promoted {}", record.id),
						);
						Ok(())
					});
				}
			}
			if let Some(action @ ("promoted" | "regressed" | "pruned" | "trial_inconclusive")) =
				event
			{
				emit_lifecycle(&record, action);
			}
		}
	}
}

/// Record one control sample per matched shadow artifact. Once the baseline
/// holds `min_samples` samples the artifact opens its live trial — unless
/// another trial already runs in an overlapping scope, because two trials in
/// one session share every verdict and neither could be credited.
fn record_control(
	shadowed: HashSet<String>,
	passed: bool,
	turn_calls: u32,
	policy: &EvolutionConfig,
	seam_on: bool,
	grade: Option<&TurnGrade>,
) {
	if shadowed.is_empty() {
		return;
	}
	let mut open_trials: Vec<ArtifactScope> = match super::registry::list_records() {
		Ok(records) => records
			.into_iter()
			.filter(|record| record.state == EvolutionState::Trial)
			.map(|record| record.scope)
			.collect(),
		Err(error) => {
			crate::log_error!(
				"evolution registry unavailable; control verdict dropped: {}",
				error
			);
			return;
		}
	};
	for id in shadowed {
		let mut opened = false;
		let result = super::registry::mutate_record(&id, |record| {
			if record.state != EvolutionState::Shadow {
				return Ok(());
			}
			let Some(score) = sample_score(record, passed, seam_on, grade) else {
				return Ok(());
			};
			if passed {
				record.control_successes = record.control_successes.saturating_add(1);
			} else {
				record.control_failures = record.control_failures.saturating_add(1);
			}
			record.control_score += score;
			record.control_calls = record.control_calls.saturating_add(turn_calls as u64);
			let control = record.control_successes + record.control_failures;
			if control >= policy.min_samples
				&& (record.effect != EffectClass::Effectful || record.explicit_authorization)
				&& !open_trials
					.iter()
					.any(|scope| scope.overlaps(&record.scope))
			{
				record.state = EvolutionState::Trial;
				opened = true;
				super::registry::append_history(
					record,
					"trial",
					format!(
						"shadow control baseline: {} samples, pass rate {:.2}",
						control,
						pass_rate(
							record.measure,
							record.control_successes,
							record.control_failures,
							record.control_score,
						)
					),
				);
			}
			Ok(())
		});
		if let Ok(record) = result {
			if opened {
				open_trials.push(record.scope.clone());
				emit_lifecycle(&record, "trial");
			}
		}
	}
}

pub(super) fn emit_lifecycle(record: &EvolutionRecord, action: &str) {
	crate::supervisor::stats::evolution(action);
	crate::supervisor::notify(&format!(
		"evolution {}: {} ({}, {}/{})",
		action,
		record.name,
		record.kind.as_str(),
		record.scope.project.as_deref().unwrap_or("*"),
		record.scope.domain.as_deref().unwrap_or("*")
	));
	if let Some(session_id) = crate::session::context::current_session_id() {
		crate::mcp::process::send_notification_message(crate::websocket::ServerMessage::evolution(
			action,
			&record.id,
			&record.name,
			record.kind.as_str(),
			record.state.as_str(),
			serde_json::to_value(&record.scope).unwrap_or_default(),
			session_id,
		));
	}
}

#[cfg(test)]
#[path = "runtime_tests.rs"]
mod tests;
