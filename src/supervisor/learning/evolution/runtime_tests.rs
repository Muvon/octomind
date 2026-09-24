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

use super::super::{
	ArtifactKind, ArtifactScope, EffectClass, EvolutionConfig, EvolutionRecord, EvolutionState,
	Measure, REGISTRY_SCHEMA_VERSION,
};
use super::*;

fn record(id: &str, kind: ArtifactKind, state: EvolutionState) -> EvolutionRecord {
	let now = chrono::Utc::now().to_rfc3339();
	EvolutionRecord {
		schema_version: REGISTRY_SCHEMA_VERSION,
		id: id.to_string(),
		name: format!("evolved-{id}"),
		description: "test behavior".to_string(),
		kind,
		scope: ArtifactScope {
			project: Some("project".to_string()),
			domain: Some("developer".to_string()),
		},
		state,
		effect: if kind == ArtifactKind::Skill {
			EffectClass::Advisory
		} else {
			EffectClass::Effectful
		},
		explicit_authorization: true,
		source_memory_ids: vec!["memory-1".to_string()],
		evidence: vec!["session://s/message/1".to_string()],
		replay_cases: Vec::new(),
		artifact_version: 1,
		parent_version: None,
		superseded_ids: Vec::new(),
		generator_model: "openai:generator".to_string(),
		verifier_model: "google:verifier".to_string(),
		artifact_path: if kind == ArtifactKind::Skill {
			"SKILL.md".to_string()
		} else {
			"guardrail.toml".to_string()
		},
		script_path: None,
		shadow_matches: 0,
		trial_uses: 0,
		successes: 0,
		failures: 0,
		false_triggers: 0,
		control_successes: 0,
		control_failures: 0,
		control_calls: 0,
		treatment_calls: 0,
		measure: None,
		control_score: 0.0,
		treatment_score: 0.0,
		created: now.clone(),
		updated: now,
		promoted: None,
		last_used: None,
		retired: None,
		history: Vec::new(),
	}
}

fn supervisor() -> crate::supervisor::SupervisorConfig {
	let config: crate::config::Config =
		toml::from_str(include_str!("../../../../config-templates/default.toml")).unwrap();
	config.supervisor
}

fn verdict(delta: f64, api_calls: u32) -> TurnVerdict<'static> {
	TurnVerdict {
		delta,
		api_calls,
		request: "",
		answer: "",
	}
}

fn policy() -> EvolutionConfig {
	enabled_config().supervisor.learning.evolution
}

fn enabled_config() -> crate::config::Config {
	let mut config: crate::config::Config =
		toml::from_str(include_str!("../../../../config-templates/default.toml")).unwrap();
	config.supervisor.learning.evolution.enabled = true;
	config
}

#[test]
fn session_accessors_without_session_context_return_empty() {
	init_for_session("developer:general");
	assert!(active_skill_dirs().is_empty());
	assert!(all_skill_bindings().is_empty());
	assert!(skill_binding("anything").is_none());
}

#[serial_test::serial]
#[tokio::test]
async fn registry_failure_disables_generated_behavior_for_session() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let data = tempfile::tempdir().unwrap();
	let project_dir = data.path().join("project");
	std::fs::create_dir_all(&project_dir).unwrap();
	let previous = std::env::var_os("OCTOMIND_DATA_DIR");
	std::env::set_var("OCTOMIND_DATA_DIR", data.path());
	let evolution_dir = crate::directories::get_learning_evolution_dir().unwrap();
	std::fs::write(evolution_dir.join("registry.json"), "not json").unwrap();

	let session_id = "evolution-registry-broken".to_string();
	crate::session::context::with_session_id(session_id.clone(), async {
		crate::session::context::set_session_workdir(&session_id, project_dir);
		crate::session::context::set_session_role(&session_id, "developer:general");
		crate::session::context::set_session_config(&session_id, &enabled_config());
		crate::session::guardrails::init_for_session();
		init_for_session("developer:general");
		assert!(all_skill_bindings().is_empty());
		let rules = crate::session::guardrails::get_rules(&session_id).unwrap();
		assert!(rules.guards.is_empty());
		crate::session::context::cleanup_session(&session_id);
	})
	.await;

	if let Some(value) = previous {
		std::env::set_var("OCTOMIND_DATA_DIR", value);
	} else {
		std::env::remove_var("OCTOMIND_DATA_DIR");
	}
}

#[serial_test::serial]
#[tokio::test]
async fn binding_is_shadow_treats_missing_and_retired_records_as_shadow() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let data = tempfile::tempdir().unwrap();
	let previous = std::env::var_os("OCTOMIND_DATA_DIR");
	std::env::set_var("OCTOMIND_DATA_DIR", data.path());
	let id = "evo-shadow-check";
	super::super::registry::create_record(
		record(id, ArtifactKind::Guard, EvolutionState::Trial),
		"[[guard]]\nmatch = \"shell\"\nmessage = \"blocked\"\n",
		None,
	)
	.unwrap();
	assert!(!binding_is_shadow(id, false));
	assert!(binding_is_shadow(id, true));
	assert!(binding_is_shadow("missing-record", false));

	super::super::registry::mutate_record(id, |item| {
		item.state = EvolutionState::Retired;
		Ok(())
	})
	.unwrap();
	assert!(binding_is_shadow(id, false));

	if let Some(value) = previous {
		std::env::set_var("OCTOMIND_DATA_DIR", value);
	} else {
		std::env::remove_var("OCTOMIND_DATA_DIR");
	}
}

#[serial_test::serial]
#[tokio::test]
async fn generated_guardrails_skip_unreadable_and_unparsable_artifacts() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let data = tempfile::tempdir().unwrap();
	let previous = std::env::var_os("OCTOMIND_DATA_DIR");
	std::env::set_var("OCTOMIND_DATA_DIR", data.path());
	let native = "[[guard]]\nmatch = \"shell\"\nmessage = \"blocked\"\n";
	let valid = record(
		"evo-guard-valid",
		ArtifactKind::Guard,
		EvolutionState::Trial,
	);
	super::super::registry::create_record(valid.clone(), native, None).unwrap();
	let unreadable = record(
		"evo-guard-unreadable",
		ArtifactKind::Guard,
		EvolutionState::Trial,
	);
	super::super::registry::create_record(unreadable.clone(), native, None).unwrap();
	std::fs::remove_file(unreadable.native_path().unwrap()).unwrap();
	let unparsable = record(
		"evo-guard-unparsable",
		ArtifactKind::Guard,
		EvolutionState::Trial,
	);
	super::super::registry::create_record(unparsable.clone(), native, None).unwrap();
	std::fs::write(unparsable.native_path().unwrap(), "not toml {{{").unwrap();

	let valid_id = valid.id.clone();
	let generated = generated_guardrails(&[valid, unreadable, unparsable]).unwrap();
	assert_eq!(generated.guards.len(), 1);
	assert_eq!(generated.guards[0].evolution.as_ref().unwrap().id, valid_id);

	if let Some(value) = previous {
		std::env::set_var("OCTOMIND_DATA_DIR", value);
	} else {
		std::env::remove_var("OCTOMIND_DATA_DIR");
	}
}

#[serial_test::serial]
#[tokio::test]
async fn mark_shadow_match_ignores_non_shadow_records() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let data = tempfile::tempdir().unwrap();
	let previous = std::env::var_os("OCTOMIND_DATA_DIR");
	std::env::set_var("OCTOMIND_DATA_DIR", data.path());
	let id = "evo-shadow-nonshadow";
	super::super::registry::create_record(
		record(id, ArtifactKind::Guard, EvolutionState::Trial),
		"[[guard]]\nmatch = \"shell\"\nmessage = \"blocked\"\n",
		None,
	)
	.unwrap();

	mark_shadow_match(id);
	let stored = super::super::registry::get_record(id).unwrap().unwrap();
	assert_eq!(stored.state, EvolutionState::Trial);
	assert_eq!(stored.shadow_matches, 0);

	if let Some(value) = previous {
		std::env::set_var("OCTOMIND_DATA_DIR", value);
	} else {
		std::env::remove_var("OCTOMIND_DATA_DIR");
	}
}

#[serial_test::serial]
#[tokio::test]
async fn unauthorized_effectful_shadow_stays_shadow_past_threshold() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let data = tempfile::tempdir().unwrap();
	let previous = std::env::var_os("OCTOMIND_DATA_DIR");
	std::env::set_var("OCTOMIND_DATA_DIR", data.path());
	let id = "evo-shadow-unauthorized";
	let mut item = record(id, ArtifactKind::Guard, EvolutionState::Shadow);
	item.effect = EffectClass::Effectful;
	item.explicit_authorization = false;
	super::super::registry::create_record(
		item,
		"[[guard]]\nmatch = \"shell\"\nmessage = \"blocked\"\n",
		None,
	)
	.unwrap();

	mark_shadow_match(id);
	mark_shadow_match(id);
	let stored = super::super::registry::get_record(id).unwrap().unwrap();
	assert_eq!(stored.shadow_matches, 2);
	assert_eq!(stored.state, EvolutionState::Shadow);

	if let Some(value) = previous {
		std::env::set_var("OCTOMIND_DATA_DIR", value);
	} else {
		std::env::remove_var("OCTOMIND_DATA_DIR");
	}
}

#[serial_test::serial]
#[tokio::test]
async fn trial_without_successes_retires_at_use_limit() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let data = tempfile::tempdir().unwrap();
	let previous = std::env::var_os("OCTOMIND_DATA_DIR");
	std::env::set_var("OCTOMIND_DATA_DIR", data.path());
	let id = "evo-trial-inconclusive";
	super::super::registry::create_record(
		record(id, ArtifactKind::Guard, EvolutionState::Trial),
		"[[guard]]\nmatch = \"shell\"\nmessage = \"blocked\"\n",
		None,
	)
	.unwrap();

	let policy = policy();
	for _ in 0..policy.max_trial_uses {
		mark_behavior_used("session", id);
		reinforce_session("session", &verdict(0.0, 1), &supervisor()).await;
	}
	let stored = super::super::registry::get_record(id).unwrap().unwrap();
	assert_eq!(stored.state, EvolutionState::Retired);
	assert_eq!(stored.trial_uses, policy.max_trial_uses);
	assert!(stored
		.history
		.iter()
		.any(|event| event.event == "trial_inconclusive"));

	if let Some(value) = previous {
		std::env::set_var("OCTOMIND_DATA_DIR", value);
	} else {
		std::env::remove_var("OCTOMIND_DATA_DIR");
	}
}

#[serial_test::serial]
#[tokio::test]
async fn promotion_retires_superseded_artifacts_and_notifies_session() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let data = tempfile::tempdir().unwrap();
	let previous = std::env::var_os("OCTOMIND_DATA_DIR");
	std::env::set_var("OCTOMIND_DATA_DIR", data.path());
	let native = "[[guard]]\nmatch = \"shell\"\nmessage = \"blocked\"\n";
	let successor_id = "evo-promote-successor";
	let predecessor_id = "evo-promote-predecessor";
	let policy = policy();
	let mut successor = record(successor_id, ArtifactKind::Guard, EvolutionState::Trial);
	successor.control_failures = policy.min_samples;
	successor.control_calls = policy.min_samples as u64;
	successor.superseded_ids = vec![predecessor_id.to_string()];
	super::super::registry::create_record(successor, native, None).unwrap();
	super::super::registry::create_record(
		record(predecessor_id, ArtifactKind::Guard, EvolutionState::Trial),
		native,
		None,
	)
	.unwrap();

	let session_id = "evolution-promotion-session".to_string();
	crate::session::context::with_session_id(session_id.clone(), async {
		for _ in 0..policy.min_samples {
			mark_behavior_used(&session_id, successor_id);
			reinforce_session(&session_id, &verdict(0.05, 1), &supervisor()).await;
		}
		let promoted = super::super::registry::get_record(successor_id)
			.unwrap()
			.unwrap();
		assert_eq!(promoted.state, EvolutionState::Active);
		assert!(promoted.promoted.is_some());
		let retired = super::super::registry::get_record(predecessor_id)
			.unwrap()
			.unwrap();
		assert_eq!(retired.state, EvolutionState::Retired);
		assert!(retired.history.iter().any(|event| event.event == "retired"));
	})
	.await;

	if let Some(value) = previous {
		std::env::set_var("OCTOMIND_DATA_DIR", value);
	} else {
		std::env::remove_var("OCTOMIND_DATA_DIR");
	}
}

#[serial_test::serial]
#[tokio::test]
async fn reinforce_session_skips_unknown_behavior_ids() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let data = tempfile::tempdir().unwrap();
	let previous = std::env::var_os("OCTOMIND_DATA_DIR");
	std::env::set_var("OCTOMIND_DATA_DIR", data.path());

	mark_behavior_used("session", "no-such-record");
	reinforce_session("session", &verdict(0.05, 1), &supervisor()).await;

	if let Some(value) = previous {
		std::env::set_var("OCTOMIND_DATA_DIR", value);
	} else {
		std::env::remove_var("OCTOMIND_DATA_DIR");
	}
}

#[serial_test::serial]
#[tokio::test]
async fn only_live_skill_dirs_are_exposed_to_the_runtime() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let data = tempfile::tempdir().unwrap();
	let project_dir = data.path().join("project");
	std::fs::create_dir_all(&project_dir).unwrap();
	let previous = std::env::var_os("OCTOMIND_DATA_DIR");
	std::env::set_var("OCTOMIND_DATA_DIR", data.path());
	let native = |name: &str| {
		format!(
			"---\nname: {name}\ndescription: test\ndomains: developer\nrules:\n  - content(schema)\n---\nbody\n"
		)
	};
	let shadow_id = "evo-availability-shadow";
	let trial_id = "evo-availability-trial";
	super::super::registry::create_record(
		record(shadow_id, ArtifactKind::Skill, EvolutionState::Shadow),
		&native(&format!("evolved-{shadow_id}")),
		None,
	)
	.unwrap();
	super::super::registry::create_record(
		record(trial_id, ArtifactKind::Skill, EvolutionState::Trial),
		&native(&format!("evolved-{trial_id}")),
		None,
	)
	.unwrap();

	let session_id = "evolution-availability-session".to_string();
	crate::session::context::with_session_id(session_id.clone(), async {
		crate::session::context::set_session_workdir(&session_id, project_dir);
		crate::session::context::set_session_role(&session_id, "developer:general");
		crate::session::context::set_session_config(&session_id, &enabled_config());
		crate::session::guardrails::init_for_session();
		init_for_session("developer:general");

		// Bindings exist for both skills, but only the trial skill's directory
		// is exposed to the runtime — shadow skills stay observational.
		let trial_binding = skill_binding(&format!("evolved-{trial_id}")).expect("trial binding");
		let shadow_binding =
			skill_binding(&format!("evolved-{shadow_id}")).expect("shadow binding");
		let dirs = active_skill_dirs();
		assert!(dirs.contains(&trial_binding.path));
		assert!(!dirs.contains(&shadow_binding.path));
		crate::session::context::cleanup_session(&session_id);
	})
	.await;

	if let Some(value) = previous {
		std::env::set_var("OCTOMIND_DATA_DIR", value);
	} else {
		std::env::remove_var("OCTOMIND_DATA_DIR");
	}
}

fn measured(control: (u32, u32, u64), treatment: (u32, u32, u64)) -> EvolutionRecord {
	let mut item = record("evo-measured", ArtifactKind::Skill, EvolutionState::Trial);
	(
		item.control_successes,
		item.control_failures,
		item.control_calls,
	) = control;
	(item.successes, item.failures, item.treatment_calls) = treatment;
	item
}

#[test]
fn evidence_waits_for_min_samples_in_both_arms() {
	let policy = policy();
	let below = policy.min_samples - 1;
	assert!(evidence(&measured((below, 0, 3), (3, 0, 3)), &policy).is_none());
	assert!(evidence(&measured((3, 0, 3), (below, 0, 3)), &policy).is_none());
	assert!(evidence(&measured((3, 0, 3), (3, 0, 3)), &policy).is_some());
}

#[test]
fn admission_requires_gain_beyond_noise_paid_for_by_cost() {
	let policy = policy();
	// 0/3 control vs 3/3 treatment at equal cost: a real gain.
	let gain = evidence(&measured((0, 3, 6), (3, 0, 6)), &policy).unwrap();
	assert!(gain.gain > policy.noise_margin);
	assert!(admits(gain, &policy));
	// Same gain at ten times the API calls per turn: the gain does not pay for it.
	let costly = evidence(&measured((0, 3, 6), (3, 0, 60)), &policy).unwrap();
	assert!(!admits(costly, &policy));
	// Equal outcomes and equal cost: nothing measurable to admit.
	let flat = evidence(&measured((3, 0, 6), (3, 0, 6)), &policy).unwrap();
	assert!(!admits(flat, &policy));
	// Equal outcomes at half the API calls: admitted on cost alone.
	let cheaper = evidence(&measured((3, 0, 12), (3, 0, 6)), &policy).unwrap();
	assert!(admits(cheaper, &policy));
}

#[test]
fn retention_keeps_a_small_positive_gain_that_admission_would_not() {
	let policy = policy();
	// 2/4 control vs 3/5 treatment: a small positive gap inside the noise band;
	// one more failure turns it negative.
	let edge = evidence(&measured((2, 2, 4), (3, 2, 5)), &policy).unwrap();
	assert!(edge.gain > 0.0 && edge.gain <= policy.noise_margin);
	assert!(!admits(edge, &policy));
	assert!(sustains(edge, &policy));
	let gone = evidence(&measured((2, 2, 4), (2, 3, 5)), &policy).unwrap();
	assert!(!sustains(gone, &policy));
}

#[serial_test::serial]
#[tokio::test]
async fn trial_below_control_retires_as_regressed() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let data = tempfile::tempdir().unwrap();
	let previous = std::env::var_os("OCTOMIND_DATA_DIR");
	std::env::set_var("OCTOMIND_DATA_DIR", data.path());
	let policy = policy();
	let id = "evo-regressed";
	let mut item = record(id, ArtifactKind::Guard, EvolutionState::Trial);
	item.control_successes = policy.min_samples;
	item.control_calls = policy.min_samples as u64;
	super::super::registry::create_record(
		item,
		"[[guard]]\nmatch = \"shell\"\nmessage = \"blocked\"\n",
		None,
	)
	.unwrap();

	for _ in 0..policy.min_samples {
		mark_behavior_used("session", id);
		reinforce_session("session", &verdict(-0.15, 1), &supervisor()).await;
	}
	let stored = super::super::registry::get_record(id).unwrap().unwrap();
	assert_eq!(stored.state, EvolutionState::Retired);
	assert!(stored
		.history
		.iter()
		.any(|event| event.event == "regressed"));

	if let Some(value) = previous {
		std::env::set_var("OCTOMIND_DATA_DIR", value);
	} else {
		std::env::remove_var("OCTOMIND_DATA_DIR");
	}
}

#[serial_test::serial]
#[tokio::test]
async fn overlapping_shadows_open_one_trial_at_a_time() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let data = tempfile::tempdir().unwrap();
	let previous = std::env::var_os("OCTOMIND_DATA_DIR");
	std::env::set_var("OCTOMIND_DATA_DIR", data.path());
	let policy = policy();
	let ids = ["evo-overlap-a", "evo-overlap-b"];
	for id in ids {
		super::super::registry::create_record(
			record(id, ArtifactKind::Guard, EvolutionState::Shadow),
			"[[guard]]\nmatch = \"shell\"\nmessage = \"blocked\"\n",
			None,
		)
		.unwrap();
	}

	let session_id = "evolution-overlap-session".to_string();
	crate::session::context::with_session_id(session_id.clone(), async {
		for _ in 0..policy.min_samples {
			for id in ids {
				mark_shadow_match(id);
			}
			reinforce_session(&session_id, &verdict(0.05, 1), &supervisor()).await;
		}
		let states = ids
			.iter()
			.map(|id| {
				super::super::registry::get_record(id)
					.unwrap()
					.unwrap()
					.state
			})
			.collect::<Vec<_>>();
		assert_eq!(
			states
				.iter()
				.filter(|state| **state == EvolutionState::Trial)
				.count(),
			1,
			"{states:?}"
		);
		assert!(states.contains(&EvolutionState::Shadow));
		clear_for_session(&session_id);
	})
	.await;

	if let Some(value) = previous {
		std::env::set_var("OCTOMIND_DATA_DIR", value);
	} else {
		std::env::remove_var("OCTOMIND_DATA_DIR");
	}
}

#[test]
fn graded_evidence_separates_arms_with_equal_verdicts() {
	let policy = policy();
	let mut item = measured((3, 0, 3), (3, 0, 3));
	assert!(!admits(evidence(&item, &policy).unwrap(), &policy));
	item.measure = Some(Measure::Graded);
	item.control_score = 1.2;
	item.treatment_score = 2.7;
	let graded = evidence(&item, &policy).unwrap();
	assert!(graded.gain > policy.noise_margin, "{graded:?}");
	assert!(admits(graded, &policy));
}

#[serial_test::serial]
#[tokio::test]
async fn skill_exposure_is_sticky_in_both_arms() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let data = tempfile::tempdir().unwrap();
	let previous = std::env::var_os("OCTOMIND_DATA_DIR");
	std::env::set_var("OCTOMIND_DATA_DIR", data.path());
	let policy = policy();
	let id = "evo-sticky-skill";
	super::super::registry::create_record(
		record(id, ArtifactKind::Skill, EvolutionState::Shadow),
		"---\nname: evolved-evo-sticky-skill\ndescription: test\nrules:\n  - content(schema)\n---\nbody\n",
		None,
	)
	.unwrap();

	let session_id = "evolution-sticky-session".to_string();
	crate::session::context::with_session_id(session_id.clone(), async {
		// One trigger match, then every verdict turn of the session is a
		// control sample; a turn without a verdict is not.
		mark_shadow_match(id);
		reinforce_session(&session_id, &verdict(0.0, 1), &supervisor()).await;
		for _ in 0..policy.min_samples {
			reinforce_session(&session_id, &verdict(-0.15, 1), &supervisor()).await;
		}
		let trial = super::super::registry::get_record(id).unwrap().unwrap();
		assert_eq!(trial.control_failures, policy.min_samples);
		assert_eq!(trial.state, EvolutionState::Trial);

		// Activation by the trigger makes the same skill treatment from then on.
		mark_skill_activated(&session_id, id);
		for _ in 0..2 {
			reinforce_session(&session_id, &verdict(0.05, 1), &supervisor()).await;
		}
		let treated = super::super::registry::get_record(id).unwrap().unwrap();
		assert_eq!(treated.successes, 2);
		assert_eq!(treated.control_failures, policy.min_samples);
		clear_for_session(&session_id);
	})
	.await;

	if let Some(value) = previous {
		std::env::set_var("OCTOMIND_DATA_DIR", value);
	} else {
		std::env::remove_var("OCTOMIND_DATA_DIR");
	}
}

#[serial_test::serial]
#[tokio::test]
async fn graded_samples_use_evaluation_outcome_and_drop_inapplicable_turns() {
	use crate::session::chat::test_support::{install_fake_evaluation, nouls, FakeEvaluationStep};
	let fake = install_fake_evaluation(vec![
		FakeEvaluationStep::Answers(nouls(&[("outcome", 0.3), ("a0", 0.9)])),
		FakeEvaluationStep::Answers(nouls(&[("outcome", 0.9), ("a0", 0.2)])),
	])
	.await;
	let data = tempfile::tempdir().unwrap();
	let previous = std::env::var_os("OCTOMIND_DATA_DIR");
	std::env::set_var("OCTOMIND_DATA_DIR", data.path());
	let graded_id = "evo-graded";
	let verdict_id = "evo-verdict-locked";
	let native = "[[guard]]\nmatch = \"shell\"\nmessage = \"blocked\"\n";
	super::super::registry::create_record(
		record(graded_id, ArtifactKind::Guard, EvolutionState::Shadow),
		native,
		None,
	)
	.unwrap();
	// Already holds ungraded samples: turning the seam on must not regrade it.
	let mut locked = record(verdict_id, ArtifactKind::Guard, EvolutionState::Shadow);
	locked.control_successes = 1;
	super::super::registry::create_record(locked, native, None).unwrap();
	let mut supervisor = supervisor();
	supervisor.enabled = true;
	supervisor.evaluate.evolution = true;
	let turn = TurnVerdict {
		delta: 0.05,
		api_calls: 2,
		request: "fix the failing build",
		answer: "fixed; cargo build passes",
	};

	let session_id = "evolution-graded-session".to_string();
	crate::session::context::with_session_id(session_id.clone(), async {
		for _ in 0..3 {
			mark_shadow_match(graded_id);
			mark_shadow_match(verdict_id);
			reinforce_session(&session_id, &turn, &supervisor).await;
		}
		clear_for_session(&session_id);
	})
	.await;

	// Turn 1 counts at the graded outcome, turn 2 is inapplicable, turn 3
	// has no evaluation answer (the fake has no step left): both dropped.
	let graded = super::super::registry::get_record(graded_id)
		.unwrap()
		.unwrap();
	assert_eq!(graded.measure, Some(Measure::Graded));
	assert_eq!(graded.control_successes, 1);
	assert!((graded.control_score - 0.3).abs() < 1e-9);
	assert_eq!(graded.false_triggers, 1);
	let locked = super::super::registry::get_record(verdict_id)
		.unwrap()
		.unwrap();
	assert_eq!(locked.measure, Some(Measure::Verdict));
	// 1 prior + 2 plain verdicts reach the baseline; the trial then opens.
	assert_eq!(locked.control_successes, policy().min_samples);
	assert_eq!(locked.state, EvolutionState::Trial);

	// Only the graded artifact is put to the evaluation, with the turn's text.
	let requests = fake.requests.lock().unwrap();
	assert_eq!(requests.len(), 3);
	let state = requests[0].state.to_string();
	assert!(state.contains("fix the failing build") && state.contains("cargo build passes"));
	assert!(state.contains("evolved-evo-graded") && !state.contains("evolved-evo-verdict-locked"));
	drop(requests);

	if let Some(value) = previous {
		std::env::set_var("OCTOMIND_DATA_DIR", value);
	} else {
		std::env::remove_var("OCTOMIND_DATA_DIR");
	}
}
