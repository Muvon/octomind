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
	ArtifactKind, ArtifactScope, EffectClass, EvolutionRecord, EvolutionState,
	REGISTRY_SCHEMA_VERSION,
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
		created: now.clone(),
		updated: now,
		promoted: None,
		last_used: None,
		retired: None,
		history: Vec::new(),
	}
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

	for _ in 0..TRIAL_MAX_USES {
		mark_behavior_used("session", id);
		reinforce_session("session", 0.0).await;
	}
	let stored = super::super::registry::get_record(id).unwrap().unwrap();
	assert_eq!(stored.state, EvolutionState::Retired);
	assert_eq!(stored.trial_uses, TRIAL_MAX_USES);
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
	let mut successor = record(successor_id, ArtifactKind::Guard, EvolutionState::Trial);
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
		for _ in 0..TRIAL_SUCCESSES_REQUIRED {
			mark_behavior_used(&session_id, successor_id);
			reinforce_session(&session_id, 0.05).await;
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
	reinforce_session("session", 0.05).await;

	if let Some(value) = previous {
		std::env::set_var("OCTOMIND_DATA_DIR", value);
	} else {
		std::env::remove_var("OCTOMIND_DATA_DIR");
	}
}

#[serial_test::serial]
#[tokio::test]
async fn behavior_available_requires_runtime_affecting_binding() {
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
		assert!(behavior_available(&session_id, trial_id));
		assert!(!behavior_available(&session_id, shadow_id));
		assert!(!behavior_available("other-session", trial_id));

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
