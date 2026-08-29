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

#[test]
fn scope_dimensions_are_independent_and_exact() {
	let global = ArtifactScope {
		project: None,
		domain: None,
	};
	assert!(global.matches("octomind", "developer"));
	assert!(global.matches("other", "writer"));

	let domain = ArtifactScope {
		project: None,
		domain: Some("developer".to_string()),
	};
	assert!(domain.matches("octomind", "developer"));
	assert!(!domain.matches("octomind", "writer"));

	let project_domain = ArtifactScope {
		project: Some("octomind".to_string()),
		domain: Some("developer".to_string()),
	};
	assert!(project_domain.matches("octomind", "developer"));
	assert!(!project_domain.matches("octomind", "developer:general"));
	assert!(!project_domain.matches("other", "developer"));
}

#[test]
fn role_variants_share_one_domain() {
	assert_eq!(domain_name("developer:general"), "developer");
	assert_eq!(domain_name("developer:rust"), "developer");
	assert_eq!(domain_name("writer"), "writer");
}

#[test]
fn evolution_is_opt_in() {
	assert!(!EvolutionConfig::default().enabled);
}

#[test]
fn evolution_table_is_required_by_learning_schema() {
	let missing = "enabled = true\nmodel = \"openai:gpt-5-mini\"\n";
	assert!(toml::from_str::<crate::supervisor::learning::LearningConfig>(missing).is_err());
	let present = r#"
enabled = true
model = "openai:gpt-5-mini"
[evolution]
enabled = false
"#;
	let parsed = toml::from_str::<crate::supervisor::learning::LearningConfig>(present).unwrap();
	assert!(!parsed.evolution.enabled);
}

#[serial_test::serial]
#[tokio::test]
async fn lifecycle_requires_shadow_then_verified_trial_and_rolls_back() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let data = tempfile::tempdir().unwrap();
	let previous = std::env::var_os("OCTOMIND_DATA_DIR");
	std::env::set_var("OCTOMIND_DATA_DIR", data.path());
	let id = "evo-lifecycle-test";
	let native = "[[guard]]\nmatch = \"shell\"\nmessage = \"blocked\"\n";
	super::registry::create_record(
		record(id, ArtifactKind::Guard, EvolutionState::Shadow),
		native,
		None,
	)
	.unwrap();

	mark_shadow_match(id);
	assert_eq!(
		get_record(id).unwrap().unwrap().state,
		EvolutionState::Shadow
	);
	mark_shadow_match(id);
	assert_eq!(
		get_record(id).unwrap().unwrap().state,
		EvolutionState::Trial
	);

	for _ in 0..TRIAL_SUCCESSES_REQUIRED {
		mark_behavior_used("session", id);
		reinforce_session("session", 0.05).await;
	}
	assert_eq!(
		get_record(id).unwrap().unwrap().state,
		EvolutionState::Active
	);

	mark_behavior_used("session", id);
	reinforce_session("session", -0.15).await;
	let rolled_back = get_record(id).unwrap().unwrap();
	assert_eq!(rolled_back.state, EvolutionState::Shadow);
	assert!(rolled_back
		.history
		.iter()
		.any(|event| event.event == "rollback"));

	clear_for_session("session");
	if let Some(value) = previous {
		std::env::set_var("OCTOMIND_DATA_DIR", value);
	} else {
		std::env::remove_var("OCTOMIND_DATA_DIR");
	}
}

#[serial_test::serial]
#[tokio::test]
async fn generated_guardrail_keeps_shadow_binding_for_native_runtime() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let data = tempfile::tempdir().unwrap();
	let previous = std::env::var_os("OCTOMIND_DATA_DIR");
	std::env::set_var("OCTOMIND_DATA_DIR", data.path());
	let id = "evo-shadow-binding";
	let native = "[[guard]]\nmatch = \"shell\"\nmessage = \"blocked\"\n";
	let item = record(id, ArtifactKind::Guard, EvolutionState::Shadow);
	super::registry::create_record(item.clone(), native, None).unwrap();
	let generated = generated_guardrails(&[item]).unwrap();
	let binding = generated.guards[0].evolution.as_ref().unwrap();
	assert_eq!(binding.id, id);
	assert!(binding.shadow);

	if let Some(value) = previous {
		std::env::set_var("OCTOMIND_DATA_DIR", value);
	} else {
		std::env::remove_var("OCTOMIND_DATA_DIR");
	}
}

#[serial_test::serial]
#[tokio::test]
async fn stored_trial_guard_blocks_with_exact_registry_attribution() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let data = tempfile::tempdir().unwrap();
	let previous = std::env::var_os("OCTOMIND_DATA_DIR");
	std::env::set_var("OCTOMIND_DATA_DIR", data.path());
	let id = "evo-trial-attribution";
	let native = "[[guard]]\nmatch = \"shell\"\nmessage = \"blocked\"\n";
	let item = record(id, ArtifactKind::Guard, EvolutionState::Trial);
	super::registry::create_record(item.clone(), native, None).unwrap();
	let generated = generated_guardrails(&[item]).unwrap();
	let evaluation = crate::config::guardrails::evaluate_guards(
		&generated,
		Some("shell"),
		&serde_json::json!({}),
		&[],
		&std::collections::HashSet::new(),
	);
	let (message, binding) = evaluation.blocked.unwrap();
	assert_eq!(message, "blocked");
	assert_eq!(binding.unwrap().id, id);

	if let Some(value) = previous {
		std::env::set_var("OCTOMIND_DATA_DIR", value);
	} else {
		std::env::remove_var("OCTOMIND_DATA_DIR");
	}
}

#[serial_test::serial]
#[tokio::test]
async fn session_loader_keeps_shadow_skill_observational_and_trial_skill_loadable() {
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
	let shadow_id = "evo-shadow-skill";
	let trial_id = "evo-trial-skill";
	super::registry::create_record(
		record(shadow_id, ArtifactKind::Skill, EvolutionState::Shadow),
		&native(&format!("evolved-{shadow_id}")),
		None,
	)
	.unwrap();
	super::registry::create_record(
		record(trial_id, ArtifactKind::Skill, EvolutionState::Trial),
		&native(&format!("evolved-{trial_id}")),
		None,
	)
	.unwrap();

	let mut config: crate::config::Config =
		toml::from_str(include_str!("../../../../config-templates/default.toml")).unwrap();
	config.supervisor.learning.evolution.enabled = true;
	let session_id = "evolution-loader-session".to_string();
	crate::session::context::with_session_id(session_id.clone(), async {
		crate::session::context::set_session_workdir(&session_id, project_dir);
		crate::session::context::set_session_role(&session_id, "developer:general");
		crate::session::context::set_session_config(&session_id, &config);
		crate::session::guardrails::init_for_session();
		init_for_session("developer:general");
		assert!(
			skill_binding(&format!("evolved-{shadow_id}"))
				.unwrap()
				.shadow
		);
		assert!(
			!skill_binding(&format!("evolved-{trial_id}"))
				.unwrap()
				.shadow
		);
		let active = active_skill_dirs();
		assert_eq!(active.len(), 1);
		assert_eq!(
			active[0]
				.parent()
				.and_then(|path| path.file_name())
				.and_then(|name| name.to_str()),
			Some(trial_id)
		);
		crate::session::context::cleanup_session(&session_id);
	})
	.await;

	if let Some(value) = previous {
		std::env::set_var("OCTOMIND_DATA_DIR", value);
	} else {
		std::env::remove_var("OCTOMIND_DATA_DIR");
	}
}

fn openai_structured_response(value: serde_json::Value) -> serde_json::Value {
	serde_json::json!({
		"id": format!("resp_{}", uuid::Uuid::new_v4()),
		"output": [{
			"type": "message",
			"content": [{"type":"output_text","text":value.to_string()}]
		}],
		"usage": {"input_tokens":20,"output_tokens":20,"total_tokens":40}
	})
}

#[serial_test::serial]
#[tokio::test]
async fn structured_models_create_verified_native_candidate_end_to_end() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let data = tempfile::tempdir().unwrap();
	let previous_data = std::env::var_os("OCTOMIND_DATA_DIR");
	let previous_url = std::env::var_os("OPENAI_API_URL");
	let previous_key = std::env::var_os("OPENAI_API_KEY");
	std::env::set_var("OCTOMIND_DATA_DIR", data.path());
	std::env::set_var("OPENAI_API_KEY", "test-key");

	let session_name = "evolution-structured-e2e";
	let lesson = crate::supervisor::learning::Lesson {
		content: "After schema changes run ./scripts/schema-check.".to_string(),
		title: "Run schema check after schema changes".to_string(),
		memory_type: "learning".to_string(),
		importance: 0.9,
		confidence: "high".to_string(),
		tags: vec!["schema".to_string(), "validation".to_string()],
		source: session_name.to_string(),
		role: "developer:general".to_string(),
		project: "project".to_string(),
		scope: "scoped".to_string(),
		created: chrono::Utc::now().to_rfc3339(),
		related: Vec::new(),
		evidence: vec![format!("session://{session_name}/message/1")],
		outcome: crate::supervisor::learning::TrajectoryOutcome::Verified,
		last_used: String::new(),
		use_count: 0,
		storage_path: String::new(),
	};
	let memory_id = lesson.file_id();
	crate::supervisor::learning::backend::FileBackend
		.store(&lesson)
		.await
		.unwrap();

	let proposal = serde_json::json!({
		"decision":"candidate",
		"kind":"validator",
		"name":"schema check",
		"description":"Run the user-requested schema check after writes.",
		"scope_project":"current",
		"scope_domain":"current",
		"explicit_scope_quote":null,
		"activation_rules":[],
		"body":"",
		"match_rule":null,
		"when":[],
		"has":[],
		"message":"",
		"pipe_when":"any",
		"result_regex":null,
		"hook_on":"any",
		"assistant_match":"schema",
		"script_name":"schema-check.sh",
		"script_content":"#!/bin/sh\nexec ./scripts/schema-check\n",
		"effect":"effectful",
		"source_memory_ids":[memory_id],
		"supersedes_artifact_ids":[],
		"replay_cases":[
			{"label":"schema response","input":"schema changed","expected_match":true,"boundary":false},
			{"label":"unrelated response","input":"documentation only","expected_match":false,"boundary":false}
		],
		"explicit_authorization":true
	});
	let verdict = serde_json::json!({"supported":true,"issues":[]});
	let url = crate::session::chat::test_support::spawn_stub(vec![
		openai_structured_response(proposal),
		openai_structured_response(verdict),
	])
	.await;
	std::env::set_var("OPENAI_API_URL", url);

	let mut config: crate::config::Config =
		toml::from_str(include_str!("../../../../config-templates/default.toml")).unwrap();
	config.build_role_map();
	config.supervisor.learning.evolution.enabled = true;
	config.supervisor.learning.model = "openai:gpt-4.1".to_string();
	config.supervisor.gate.verifier_model = "openai:gpt-4.1".to_string();
	let messages = vec![crate::session::Message {
		role: "user".to_string(),
		content: "After schema changes run ./scripts/schema-check.".to_string(),
		timestamp: crate::utils::time::now_secs(),
		cached: false,
		cache_ttl: None,
		tool_call_id: None,
		name: None,
		tool_calls: None,
		images: None,
		videos: None,
		thinking: None,
		id: None,
	}];
	let created = synthesize_after_extraction(
		&messages,
		&config,
		"developer:general",
		"project",
		session_name,
	)
	.await
	.unwrap()
	.expect("candidate created");
	let stored = get_record(&created).unwrap().unwrap();
	assert_eq!(stored.kind, ArtifactKind::Validator);
	assert_eq!(stored.state, EvolutionState::Shadow);
	assert_eq!(stored.scope.project.as_deref(), Some("project"));
	assert_eq!(stored.scope.domain.as_deref(), Some("developer"));
	assert!(stored.explicit_authorization);
	let native = std::fs::read_to_string(stored.native_path().unwrap()).unwrap();
	let parsed = crate::config::guardrails::Guardrails::parse(&native).unwrap();
	assert_eq!(parsed.validators.len(), 1);
	assert!(stored
		.artifact_dir()
		.unwrap()
		.join("schema-check.sh")
		.exists());

	for (key, value) in [
		("OCTOMIND_DATA_DIR", previous_data),
		("OPENAI_API_URL", previous_url),
		("OPENAI_API_KEY", previous_key),
	] {
		if let Some(value) = value {
			std::env::set_var(key, value);
		} else {
			std::env::remove_var(key);
		}
	}
}

#[serial_test::serial]
#[tokio::test]
async fn concurrent_registry_writers_preserve_every_record() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let data = tempfile::tempdir().unwrap();
	let previous = std::env::var_os("OCTOMIND_DATA_DIR");
	std::env::set_var("OCTOMIND_DATA_DIR", data.path());
	let writers = (0..8)
		.map(|index| {
			std::thread::spawn(move || {
				let id = format!("evo-concurrent-{index}");
				super::registry::create_record(
					record(&id, ArtifactKind::Guard, EvolutionState::Shadow),
					"[[guard]]\nmatch = \"shell\"\nmessage = \"blocked\"\n",
					None,
				)
			})
		})
		.collect::<Vec<_>>();
	for writer in writers {
		writer.join().unwrap().unwrap();
	}
	let records = list_records().unwrap();
	assert_eq!(records.len(), 8);
	for index in 0..8 {
		assert!(records
			.iter()
			.any(|item| item.id == format!("evo-concurrent-{index}")));
	}

	if let Some(value) = previous {
		std::env::set_var("OCTOMIND_DATA_DIR", value);
	} else {
		std::env::remove_var("OCTOMIND_DATA_DIR");
	}
}

#[serial_test::serial]
#[tokio::test]
async fn disabled_evolution_loads_no_generated_behavior() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let data = tempfile::tempdir().unwrap();
	let project_dir = data.path().join("project");
	std::fs::create_dir_all(&project_dir).unwrap();
	let previous = std::env::var_os("OCTOMIND_DATA_DIR");
	std::env::set_var("OCTOMIND_DATA_DIR", data.path());
	super::registry::create_record(
		record("evo-disabled", ArtifactKind::Guard, EvolutionState::Active),
		"[[guard]]\nmatch = \"shell\"\nmessage = \"generated\"\n",
		None,
	)
	.unwrap();
	let mut config: crate::config::Config =
		toml::from_str(include_str!("../../../../config-templates/default.toml")).unwrap();
	config.supervisor.learning.evolution.enabled = false;
	let session_id = "evolution-disabled-session".to_string();
	crate::session::context::with_session_id(session_id.clone(), async {
		crate::session::context::set_session_workdir(&session_id, project_dir);
		crate::session::context::set_session_role(&session_id, "developer:general");
		crate::session::context::set_session_config(&session_id, &config);
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
async fn user_authored_pipe_prevents_generated_pipe_conflict() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let data = tempfile::tempdir().unwrap();
	let project_dir = data.path().join("project");
	std::fs::create_dir_all(project_dir.join(".agents")).unwrap();
	std::fs::write(
		project_dir.join(".agents/guardrails.toml"),
		"[[pipe]]\nname = \"user-pipe\"\ncommand = \"/bin/cat\"\nmatch = \"schema\"\n",
	)
	.unwrap();
	let previous = std::env::var_os("OCTOMIND_DATA_DIR");
	std::env::set_var("OCTOMIND_DATA_DIR", data.path());
	let mut generated = record("evo-pipe", ArtifactKind::Pipe, EvolutionState::Active);
	generated.script_path = Some("generated.sh".to_string());
	super::registry::create_record(
		generated,
		"[[pipe]]\nname = \"generated-pipe\"\ncommand = \"/bin/cat\"\nmatch = \"schema\"\n",
		None,
	)
	.unwrap();
	let mut config: crate::config::Config =
		toml::from_str(include_str!("../../../../config-templates/default.toml")).unwrap();
	config.supervisor.learning.evolution.enabled = true;
	let session_id = "evolution-pipe-conflict".to_string();
	crate::session::context::with_session_id(session_id.clone(), async {
		crate::session::context::set_session_workdir(&session_id, project_dir);
		crate::session::context::set_session_role(&session_id, "developer:general");
		crate::session::context::set_session_config(&session_id, &config);
		crate::session::guardrails::init_for_session();
		init_for_session("developer:general");
		let rules = crate::session::guardrails::get_rules(&session_id).unwrap();
		assert_eq!(rules.pipes.len(), 1);
		assert_eq!(rules.pipes[0].name, "user-pipe");
		assert!(rules.pipes[0].evolution.is_none());
		crate::session::context::cleanup_session(&session_id);
	})
	.await;

	if let Some(value) = previous {
		std::env::set_var("OCTOMIND_DATA_DIR", value);
	} else {
		std::env::remove_var("OCTOMIND_DATA_DIR");
	}
}

#[cfg(unix)]
#[serial_test::serial]
#[tokio::test]
async fn generated_pipe_hook_and_validator_share_native_shadow_and_trial_runtime() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let data = tempfile::tempdir().unwrap();
	let project_dir = data.path().join("project");
	std::fs::create_dir_all(&project_dir).unwrap();
	let previous = std::env::var_os("OCTOMIND_DATA_DIR");
	std::env::set_var("OCTOMIND_DATA_DIR", data.path());
	let evolution_dir = crate::directories::get_learning_evolution_dir().unwrap();
	let mut items = Vec::new();

	for (kind, label) in [
		(ArtifactKind::Pipe, "pipe"),
		(ArtifactKind::Hook, "hook"),
		(ArtifactKind::Validator, "validator"),
	] {
		for (state, state_label) in [
			(EvolutionState::Shadow, "shadow"),
			(EvolutionState::Trial, "trial"),
		] {
			let id = format!("evo-{label}-{state_label}");
			let script_name = format!("{label}-{state_label}.sh");
			let marker = data.path().join(format!("{label}-{state_label}.ran"));
			let script_path = evolution_dir.join(&id).join("artifact").join(&script_name);
			let native = match kind {
				ArtifactKind::Pipe => format!(
					"[[pipe]]\nname = \"{id}\"\ncommand = \"{}\"\nmatch = \"schema\"\n",
					script_path.display()
				),
				ArtifactKind::Hook => format!(
					"[[hook]]\non = \"any\"\nscript = \"{}\"\n",
					script_path.display()
				),
				ArtifactKind::Validator => format!(
					"[[validator]]\nname = \"{id}\"\nmatch = \"done\"\nscript = \"{}\"\n",
					script_path.display()
				),
				_ => unreachable!(),
			};
			let script = if kind == ArtifactKind::Pipe {
				format!(
					"#!/bin/sh\ninput=$(cat)\ntouch '{}'\nprintf '%s' \"$input\"\n",
					marker.display()
				)
			} else {
				format!("#!/bin/sh\ntouch '{}'\nexit 0\n", marker.display())
			};
			let mut item = record(&id, kind, state);
			item.script_path = Some(script_name.clone());
			super::registry::create_record(
				item.clone(),
				&native,
				Some(&GeneratedScript {
					file_name: script_name,
					content: script,
				}),
			)
			.unwrap();
			items.push((item, marker));
		}
	}

	let mut config: crate::config::Config =
		toml::from_str(include_str!("../../../../config-templates/default.toml")).unwrap();
	config.supervisor.learning.evolution.enabled = true;
	let session_id = "evolution-native-phases".to_string();
	crate::session::context::with_session_id(session_id.clone(), async {
		crate::session::context::set_session_workdir(&session_id, project_dir);
		crate::session::context::set_session_role(&session_id, "developer:general");
		crate::session::context::set_session_config(&session_id, &config);
		crate::session::guardrails::init_for_session();
		let records = items
			.iter()
			.map(|(record, _)| record.clone())
			.collect::<Vec<_>>();
		let generated = generated_guardrails(&records).unwrap();
		crate::session::guardrails::merge_generated_for_session(&session_id, generated);

		let piped = crate::session::pipe::run_pipe(
			&session_id,
			"developer:general",
			"schema changed",
			false,
		)
		.await
		.unwrap();
		assert_eq!(piped.as_deref(), Some("schema changed"));
		let call = crate::mcp::McpToolCall {
			tool_name: "unknown-test-tool".to_string(),
			tool_id: "call-1".to_string(),
			parameters: serde_json::json!({}),
		};
		let result = crate::mcp::McpToolResult::success(
			call.tool_name.clone(),
			call.tool_id.clone(),
			"ok".to_string(),
		);
		crate::session::hooks::run_hooks(&session_id, &config, &[call], &[result], &[false]).await;
		crate::session::hooks::run_turn_validators(&session_id, "developer:general", "done").await;

		for (item, marker) in &items {
			if item.state == EvolutionState::Shadow {
				assert!(!marker.exists(), "shadow {} executed", item.id);
				assert_eq!(get_record(&item.id).unwrap().unwrap().shadow_matches, 1);
			} else {
				assert!(marker.exists(), "trial {} did not execute", item.id);
			}
		}
		reinforce_session(&session_id, 0.05).await;
		for (item, _) in &items {
			if item.state == EvolutionState::Trial {
				assert_eq!(get_record(&item.id).unwrap().unwrap().successes, 1);
			}
		}
		crate::session::context::cleanup_session(&session_id);
	})
	.await;

	if let Some(value) = previous {
		std::env::set_var("OCTOMIND_DATA_DIR", value);
	} else {
		std::env::remove_var("OCTOMIND_DATA_DIR");
	}
}
