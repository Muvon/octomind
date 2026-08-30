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

fn proposal(kind: &str) -> Proposal {
	Proposal {
		decision: "candidate".to_string(),
		kind: kind.to_string(),
		name: "schema checks".to_string(),
		description: "Run the project schema checks.".to_string(),
		scope_project: "current".to_string(),
		scope_domain: "current".to_string(),
		explicit_scope_quote: None,
		activation_rules: vec!["content(schema) file(Cargo.toml)".to_string()],
		body: "Follow the verified schema procedure.".to_string(),
		match_rule: Some("filesystem-write".to_string()),
		when: vec!["+filesystem-write".to_string()],
		has: Vec::new(),
		message: "The user reserved this action.".to_string(),
		pipe_when: "any".to_string(),
		result_regex: None,
		hook_on: "any".to_string(),
		assistant_match: None,
		script_name: Some("check.sh".to_string()),
		script_content: Some("#!/bin/sh\nexit 0\n".to_string()),
		effect: "effectful".to_string(),
		source_memory_ids: vec!["memory".to_string()],
		supersedes_artifact_ids: Vec::new(),
		replay_cases: vec![
			super::super::ReplayCase {
				label: "matching schema task".to_string(),
				input: "change the schema".to_string(),
				expected_match: true,
				boundary: false,
			},
			super::super::ReplayCase {
				label: "unrelated task".to_string(),
				input: "write release notes".to_string(),
				expected_match: false,
				boundary: false,
			},
		],
		explicit_authorization: true,
	}
}

fn memory(scope: &str) -> Lesson {
	Lesson {
		content: "After schema changes run the project checker.".to_string(),
		title: "schema check".to_string(),
		memory_type: "learning".to_string(),
		importance: 0.9,
		confidence: "high".to_string(),
		tags: vec!["schema".to_string()],
		source: "session".to_string(),
		role: "developer".to_string(),
		project: "project".to_string(),
		scope: scope.to_string(),
		created: chrono::Utc::now().to_rfc3339(),
		related: Vec::new(),
		evidence: vec!["session://session/message/1".to_string()],
		outcome: crate::supervisor::learning::TrajectoryOutcome::Verified,
		last_used: String::new(),
		use_count: 0,
		storage_path: String::new(),
	}
}

fn user_message(content: &str) -> crate::session::Message {
	crate::session::Message {
		role: "user".to_string(),
		content: content.to_string(),
		timestamp: 1,
		cached: false,
		cache_ttl: None,
		tool_call_id: None,
		name: None,
		tool_calls: None,
		images: None,
		videos: None,
		thinking: None,
		id: None,
	}
}

#[test]
fn scoped_evidence_deterministically_narrows_global_proposal() {
	let mut candidate = proposal("skill");
	candidate.scope_project = "global".to_string();
	candidate.scope_domain = "global".to_string();
	let source = memory("scoped");
	let scope = admitted_scope(
		&candidate,
		&[&source],
		"developer:general",
		"project",
		false,
	);
	assert_eq!(scope.project.as_deref(), Some("project"));
	assert_eq!(scope.domain.as_deref(), Some("developer"));
}

#[test]
fn global_evidence_may_still_be_narrowed_by_proposal() {
	let candidate = proposal("skill");
	let source = memory("global");
	let scope = admitted_scope(
		&candidate,
		&[&source],
		"developer:general",
		"project",
		false,
	);
	assert_eq!(scope.project.as_deref(), Some("project"));
	assert_eq!(scope.domain.as_deref(), Some("developer"));
}

#[test]
fn exact_user_quote_can_authorize_one_global_scope_dimension() {
	let quote = "Apply this to all developer projects.";
	let mut candidate = proposal("skill");
	candidate.scope_project = "global".to_string();
	candidate.explicit_scope_quote = Some(quote.to_string());
	let messages = vec![user_message(quote)];
	assert!(explicit_scope_supported(&candidate, &messages));
	let source = memory("scoped");
	let scope = admitted_scope(&candidate, &[&source], "developer:general", "project", true);
	assert!(scope.project.is_none());
	assert_eq!(scope.domain.as_deref(), Some("developer"));
}

#[test]
fn generated_skill_uses_native_parser_and_existing_rule_dsl() {
	let candidate = proposal("skill");
	let scope = ArtifactScope {
		project: Some("project".to_string()),
		domain: Some("developer".to_string()),
	};
	let (native, script, path) = render_native(
		&candidate,
		ArtifactKind::Skill,
		&scope,
		"evolved-schema-check",
		"evo-id",
	)
	.unwrap();
	assert!(script.is_none());
	assert_eq!(path, "SKILL.md");
	let meta = crate::mcp::runtime::skill::parse_skill_meta(&native).unwrap();
	assert_eq!(meta.domains, vec!["developer"]);
	assert_eq!(meta.rules.len(), 1);
}

#[test]
fn effectful_artifact_fails_closed_without_quote_backed_authorization() {
	let error = validate_native(
		ArtifactKind::Guard,
		"[[guard]]\nmatch = \"shell\"\nmessage = \"no\"\n",
		None,
		EffectClass::Effectful,
		false,
	)
	.unwrap_err();
	assert!(error.to_string().contains("explicit user authorization"));
}

#[test]
fn structured_schema_is_closed_and_requires_supersession_field() {
	let schema = proposal_schema();
	assert_eq!(schema["additionalProperties"], false);
	assert!(schema["required"]
		.as_array()
		.unwrap()
		.contains(&json!("supersedes_artifact_ids")));
}

#[test]
fn unsupported_capability_reference_fails_before_native_storage() {
	let candidate = proposal("guard");
	let error = validate_runtime_references(&candidate, ArtifactKind::Guard, &[], &[]).unwrap_err();
	assert!(error.to_string().contains("unavailable capability"));
	validate_runtime_references(
		&candidate,
		ArtifactKind::Guard,
		&["filesystem-write".to_string()],
		&[],
	)
	.unwrap();
}

#[test]
fn replay_screen_requires_both_matching_and_abstaining_cases() {
	let candidate = proposal("skill");
	validate_replay_cases(&candidate.replay_cases).unwrap();
	let only_positive = candidate
		.replay_cases
		.into_iter()
		.filter(|case| case.expected_match)
		.collect::<Vec<_>>();
	assert!(validate_replay_cases(&only_positive).is_err());
}
