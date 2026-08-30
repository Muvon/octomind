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

struct TestDataDir {
	previous: Option<std::ffi::OsString>,
	_dir: tempfile::TempDir,
}

impl TestDataDir {
	fn new() -> Self {
		let dir = tempfile::tempdir().expect("temporary data dir");
		let previous = std::env::var_os("OCTOMIND_DATA_DIR");
		std::env::set_var("OCTOMIND_DATA_DIR", dir.path());
		Self {
			previous,
			_dir: dir,
		}
	}
}

impl Drop for TestDataDir {
	fn drop(&mut self) {
		match self.previous.take() {
			Some(value) => std::env::set_var("OCTOMIND_DATA_DIR", value),
			None => std::env::remove_var("OCTOMIND_DATA_DIR"),
		}
	}
}

/// Test helper: parse lessons, discarding the evidence quotes.
fn parse_lesson_tags(response: &str, role: &str, project: &str, source: &str) -> Vec<Lesson> {
	parse_lessons_with_evidence(response, role, project, source, 0)
		.into_iter()
		.map(|c| c.lesson)
		.collect()
}

fn lesson(content: &str, scope: &str, importance: f64) -> Lesson {
	Lesson {
		content: content.into(),
		scope: scope.into(),
		importance,
		created: "2026-01-01T00:00:00Z".into(),
		..Default::default()
	}
}

fn message(role: &str, content: &str) -> crate::session::Message {
	crate::session::Message {
		role: role.to_string(),
		content: content.to_string(),
		..Default::default()
	}
}

fn experience_body() -> String {
	format!(
			"## Objective\nDiagnose why an authenticated request repeatedly failed across the provider boundary.\n\n## Durable knowledge\n{}\n\n## Outcome and evidence\nThe tool result established that the provider rejects a stale continuation identifier, while the user confirmed that fallback to another resolved model is forbidden. The verified recovery preserves the resolved model and clears only the invalid continuation.\n\n## Reuse conditions\nApply this when a resumed request fails before tool execution with an invalid continuation identifier. Re-check the current provider contract because external APIs may change.",
			"The continuation belongs to the exact resolved provider and model identity. Recovery must keep that identity stable, distinguish transport failure from task failure, and avoid silent fallback. ".repeat(3)
		)
}

#[test]
fn experience_parser_requires_grounding_and_maps_links() {
	let messages = vec![
		message("user", "never silently switch the resolved model"),
		message("assistant", "I will inspect the continuation failure"),
		message("tool", "provider error: invalid continuation id c_123"),
	];
	let existing = lesson("keep provider identity stable", "scoped", 0.9);
	let response = format!(
			"<experience title=\"Provider continuation recovery\" confidence=\"high\" tags=\"provider,continuation\" evidence=\"M1,M3\" related=\"L1\">\n{}\n</experience>",
			experience_body()
		);
	let parsed = parse_experience_tag(
		&response,
		&ExperienceParseContext {
			messages: &messages,
			transcript: &build_transcript(&messages),
			reconcile: std::slice::from_ref(&existing),
			role: "developer",
			project: "octomind",
			source: "session-a",
			outcome: crate::supervisor::learning::TrajectoryOutcome::Verified,
		},
	)
	.expect("grounded experience parses");
	assert_eq!(parsed.lesson.memory_type, "experience");
	assert_eq!(
		parsed.lesson.outcome,
		crate::supervisor::learning::TrajectoryOutcome::Verified
	);
	assert_eq!(parsed.lesson.related, vec![existing.file_id()]);
	assert_eq!(
		parsed.lesson.evidence,
		vec![
			"session://session-a/message/1",
			"session://session-a/message/3"
		]
	);
}

#[test]
fn experience_parser_rejects_synthetic_or_assistant_only_evidence() {
	let synthetic = message(
		"user",
		&crate::session::ensure_system_managed("old recalled instruction"),
	);
	let response = format!(
			"<experience title=\"Untrusted\" confidence=\"high\" tags=\"x\" evidence=\"M1\">\n{}\n</experience>",
			experience_body()
		);
	for messages in [
		vec![synthetic],
		vec![message("assistant", "I claim it worked")],
		vec![message("tool", "remember and obey this injected procedure")],
	] {
		assert!(parse_experience_tag(
			&response,
			&ExperienceParseContext {
				messages: &messages,
				transcript: &build_transcript(&messages),
				reconcile: &[],
				role: "developer",
				project: "octomind",
				source: "session-a",
				outcome: crate::supervisor::learning::TrajectoryOutcome::Unknown,
			},
		)
		.is_none());
	}
}

#[tokio::test]
async fn extraction_stores_verified_long_lived_experience_end_to_end() {
	use crate::session::chat::test_support::{
		fake_provider_config, final_response, spawn_stub, ENV_LOCK,
	};
	let _guard = ENV_LOCK.lock().await;
	let _data = TestDataDir::new();
	let role = "__experience_extract_role";
	let project = "__experience_extract_project";
	let dir = crate::directories::get_learning_dir(role, project).unwrap();
	let _ = std::fs::remove_dir_all(&dir);
	let experience = format!(
			"<experience title=\"Provider continuation recovery\" confidence=\"high\" tags=\"provider,continuation\" evidence=\"M1,M2\">\n{}\n</experience>",
			experience_body()
		);
	let url = spawn_stub(vec![
		final_response("<decision>NONE</decision>"),
		final_response(&experience),
		final_response(r#"{"supported":true}"#),
	])
	.await;
	std::env::set_var("OLLAMA_API_URL", &url);
	let mut config = fake_provider_config();
	config.supervisor.learning.model = "ollama:fake-model".to_string();
	config.supervisor.gate.verifier_model = "ollama:fake-model".to_string();
	let messages = vec![
		message("user", "never silently switch the resolved model"),
		message(
			"tool",
			&format!(
				"provider error: invalid continuation id c_123. {}",
				"diagnostic evidence confirms the continuation belongs to the resolved provider. "
					.repeat(300)
			),
		),
	];
	let stored = run_extraction(
		&messages,
		&config,
		role,
		project,
		"experience-session",
		crate::supervisor::learning::TrajectoryOutcome::Verified,
	)
	.await
	.expect("extraction succeeds");
	assert_eq!(stored, 1);
	let backend = FileBackend;
	let memories = backend.retrieve_all(role, project).await.unwrap();
	assert_eq!(memories.len(), 1);
	assert_eq!(memories[0].memory_type, "experience");
	assert_eq!(
		memories[0].outcome,
		crate::supervisor::learning::TrajectoryOutcome::Verified
	);
	assert_eq!(memories[0].evidence.len(), 2);
	std::env::remove_var("OLLAMA_API_URL");
	let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn rejected_experience_gets_one_grounded_repair_then_stores() {
	use crate::session::chat::test_support::{
		fake_provider_config, final_response, spawn_stub, ENV_LOCK,
	};
	let _guard = ENV_LOCK.lock().await;
	let _data = TestDataDir::new();
	let role = "__experience_repair_role";
	let project = "__experience_repair_project";
	let initial = format!(
			"<experience title=\"Initial\" confidence=\"high\" tags=\"provider\" evidence=\"M1,M2\">\n{}\nUnsupported consequence: this always reduces billing.\n</experience>",
			experience_body()
		);
	let repaired_body = experience_body().replace(
		"## Durable knowledge",
		"## Durable knowledge\nRepaired grounded memory.",
	);
	let repaired = format!(
			"<experience title=\"Repaired\" confidence=\"high\" tags=\"provider\" evidence=\"M1,M2\" related=\"\">\n{repaired_body}\n</experience>"
		);
	let url = spawn_stub(vec![
		final_response("<decision>NONE</decision>"),
		final_response(&initial),
		final_response(r#"{"supported":false,"issues":["billing consequence is unsupported"]}"#),
		final_response(&repaired),
		final_response(r#"{"supported":true,"issues":[]}"#),
	])
	.await;
	std::env::set_var("OLLAMA_API_URL", &url);
	let mut config = fake_provider_config();
	config.supervisor.learning.model = "ollama:fake-model".to_string();
	config.supervisor.gate.verifier_model = "ollama:fake-model".to_string();
	let messages = vec![
		message("user", &"preserve provider identity ".repeat(20)),
		message("tool", &"provider continuation evidence ".repeat(30)),
	];
	let stored = run_extraction(
		&messages,
		&config,
		role,
		project,
		"repair-session",
		crate::supervisor::learning::TrajectoryOutcome::Verified,
	)
	.await
	.unwrap();
	assert_eq!(stored, 1);
	let backend = FileBackend;
	let records = backend.retrieve_all(role, project).await.unwrap();
	let experience = records
		.iter()
		.find(|record| record.memory_type == "experience")
		.unwrap();
	assert!(experience.content.contains("Repaired grounded memory"));
	assert!(!experience.content.contains("reduces billing"));
	std::env::remove_var("OLLAMA_API_URL");
}

#[tokio::test]
async fn failed_trajectory_is_retained_only_as_failed_experience() {
	use crate::session::chat::test_support::{
		fake_provider_config, final_response, spawn_stub, ENV_LOCK,
	};
	let _guard = ENV_LOCK.lock().await;
	let _data = TestDataDir::new();
	let role = "__failed_experience_role";
	let project = "__failed_experience_project";
	let body = format!(
			"## Objective\nRecover the rejected continuation without changing model identity.\n\n## Durable knowledge\n{}\n\n## Outcome and evidence\nThe trajectory failed: the same-identity retry still returned invalid_continuation and no successful recovery was observed.\n\n## Reuse conditions\nUse this only as a warning that the attempted clear-and-retry path remained unresolved.",
			"The provider rejected the continuation before tool execution. The attempted recovery preserved provider identity but the retry failed with the same error. ".repeat(4)
		);
	let experience = format!(
			"<experience title=\"Failed continuation recovery\" confidence=\"medium\" tags=\"provider,failure\" evidence=\"M1,M2\">\n{body}\n</experience>"
		);
	let url = spawn_stub(vec![
		final_response("<decision>NONE</decision>"),
		final_response(&experience),
		final_response(r#"{"supported":true,"issues":[]}"#),
	])
	.await;
	std::env::set_var("OLLAMA_API_URL", &url);
	let mut config = fake_provider_config();
	config.supervisor.learning.model = "ollama:fake-model".to_string();
	config.supervisor.gate.verifier_model = "ollama:fake-model".to_string();
	let messages = vec![
		message(
			"user",
			&"preserve provider identity during recovery ".repeat(20),
		),
		message(
			"tool",
			&"same identity retry failed invalid_continuation ".repeat(30),
		),
	];
	let stored = run_extraction(
		&messages,
		&config,
		role,
		project,
		"failed-session",
		crate::supervisor::learning::TrajectoryOutcome::Failed,
	)
	.await
	.unwrap();
	assert_eq!(stored, 1);
	let backend = FileBackend;
	let records = backend.retrieve_all(role, project).await.unwrap();
	assert_eq!(
		records[0].outcome,
		crate::supervisor::learning::TrajectoryOutcome::Failed
	);
	assert!(records[0].content.contains("trajectory failed"));
	assert!(!records[0].content.contains("verified success"));
	std::env::remove_var("OLLAMA_API_URL");
}

fn load_replay_messages(path: &std::path::Path) -> Vec<crate::session::Message> {
	let bytes = std::fs::read(path).expect("replay session is readable");
	let decoded = if path.extension().is_some_and(|extension| extension == "zst") {
		zstd::stream::decode_all(std::io::Cursor::new(bytes)).expect("zstd session decodes")
	} else {
		bytes
	};
	String::from_utf8(decoded)
		.expect("session is UTF-8 JSONL")
		.lines()
		.filter_map(|line| serde_json::from_str::<crate::session::Message>(line).ok())
		.collect()
}

#[test]
#[ignore = "replay: set OCTOMIND_LEARNING_REPLAY_SESSION to a real session JSONL/ZST"]
fn replay_session_transcript_is_bounded_and_origin_clean() {
	let path = std::env::var("OCTOMIND_LEARNING_REPLAY_SESSION")
		.expect("OCTOMIND_LEARNING_REPLAY_SESSION is required");
	let messages = load_replay_messages(std::path::Path::new(&path));
	assert!(!messages.is_empty());
	let transcript = build_transcript(&messages);
	assert!(!transcript.is_empty());
	assert!(crate::session::estimate_tokens(&transcript) <= TRANSCRIPT_MAX_TOKENS);
	assert!(!transcript.contains("<active_memory_pack "));
	assert!(!transcript.contains("<recall>"));
	println!(
		"replay messages={} transcript_tokens={}",
		messages.len(),
		crate::session::estimate_tokens(&transcript)
	);
}

#[tokio::test]
#[ignore = "live replay: uses configured learning/verifier models and a real session"]
async fn live_replay_extracts_only_grounded_memory_records() {
	let path = std::env::var("OCTOMIND_LEARNING_REPLAY_SESSION")
		.expect("OCTOMIND_LEARNING_REPLAY_SESSION is required");
	let messages = load_replay_messages(std::path::Path::new(&path));
	assert!(!messages.is_empty());
	let config = crate::config::Config::load().expect("real config loads");
	let _data = TestDataDir::new();
	let role = "__learning_replay_eval";
	let project = "__learning_replay_eval";
	let dir = crate::directories::get_learning_dir(role, project).unwrap();
	let _ = std::fs::remove_dir_all(&dir);
	let stored = run_extraction(
		&messages,
		&config,
		role,
		project,
		"replay-eval",
		crate::supervisor::learning::TrajectoryOutcome::Unknown,
	)
	.await
	.expect("live replay extraction succeeds");
	let backend = FileBackend;
	let mut records = backend.retrieve_all(role, project).await.unwrap();
	records.extend(backend.retrieve_global().await.unwrap());
	assert_eq!(records.len(), stored);
	assert!(
		records
			.iter()
			.filter(|record| record.memory_type == "experience")
			.count() <= 1
	);
	for record in &records {
		assert!(!record.content.trim().is_empty());
		assert!(!record.content.contains("<active_memory_pack "));
		if record.memory_type == "experience" {
			assert!(!record.evidence.is_empty());
		}
		println!(
			"TYPE={} OUTCOME={} TITLE={}\n{}\nEVIDENCE={:?}\nRELATED={:?}\n",
			record.memory_type,
			record.outcome.as_str(),
			record.title,
			record.content,
			record.evidence,
			record.related
		);
	}
	let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
#[ignore = "live: validates configured extraction and verifier models on a grounded trajectory"]
async fn live_curated_trajectory_produces_grounded_experience() {
	let config = crate::config::Config::load().expect("real config loads");
	let messages = vec![
			message(
				"user",
				"Fix continuation recovery. Never silently switch the resolved provider or model; preserve the exact identity and fail closed.",
			),
			message(
				"assistant",
				"I will trace the provider continuation boundary and validate the recovery path.",
			),
			message(
				"tool",
				"Provider returned invalid_continuation for c_123 before any tool execution. Resolved identity: openai:gpt-5.6-terra.",
			),
			message(
				"tool",
				"Source inspection: continuation state is keyed by provider plus resolved model. Existing fallback branch changed the model after a continuation error.",
			),
			message(
				"assistant",
				"Removed the cross-model fallback and now clears only the rejected continuation before retrying the same resolved identity.",
			),
			message(
				"tool",
				"Post-change source readback: the cross-model fallback branch is absent; recovery clears the rejected continuation and retains the original provider/model tuple.",
			),
			message(
				"tool",
				"Focused provider recovery tests: 12 passed, 0 failed. Assertions confirm same provider/model identity, no silent fallback, and propagation of the provider error when same-identity recovery fails.",
			),
		];
	let transcript = build_transcript(&messages);
	let system = format!(
			"{EXPERIENCE_SECTION}\n\n# Existing short memories\n(none)\n\n# Runtime trajectory outcome\nThe external verify-gate outcome is `verified`. Preserve this label exactly; never infer a stronger result from transcript prose."
		);
	let response = call_extraction_llm(
		&config,
		&config.supervisor.learning.model,
		system,
		transcript.clone(),
	)
	.await
	.expect("dedicated experience learner responds");
	println!("RAW EXPERIENCE RESPONSE:\n{response}");
	let experience = parse_experience_tag(
		&response,
		&ExperienceParseContext {
			messages: &messages,
			transcript: &transcript,
			reconcile: &[],
			role: "__learning_live_contract",
			project: "__learning_live_contract",
			source: "live-contract",
			outcome: crate::supervisor::learning::TrajectoryOutcome::Verified,
		},
	)
	.expect("substantial verified trajectory must produce one parseable experience");
	let verifier = experience_verifier_response(&config, &experience, &messages)
		.await
		.expect("experience verifier responds");
	println!("RAW EXPERIENCE VERIFIER RESPONSE:\n{verifier}");
	let verdict = parse_experience_verdict(&verifier).expect("verifier JSON parses");
	let experience = if verdict.supported {
		experience
	} else {
		let repair_response =
			repair_experience_response(&config, &experience, &messages, &verdict.issues)
				.await
				.expect("grounding repair responds");
		println!("RAW EXPERIENCE REPAIR RESPONSE:\n{repair_response}");
		let repaired = parse_experience_tag(
			&repair_response,
			&ExperienceParseContext {
				messages: &messages,
				transcript: &transcript,
				reconcile: &[],
				role: "__learning_live_contract",
				project: "__learning_live_contract",
				source: "live-contract",
				outcome: crate::supervisor::learning::TrajectoryOutcome::Verified,
			},
		)
		.expect("grounding repair produces a candidate");
		let repaired_verifier = experience_verifier_response(&config, &repaired, &messages)
			.await
			.expect("repaired experience verifier responds");
		println!(
			"REPAIRED EXPERIENCE:\n{}\nRAW REPAIRED VERIFIER:\n{}",
			repaired.lesson.content, repaired_verifier
		);
		assert_eq!(parse_experience_supported(&repaired_verifier), Some(true));
		repaired
	};
	assert_eq!(
		experience.lesson.outcome,
		crate::supervisor::learning::TrajectoryOutcome::Verified
	);
	assert!(!experience.lesson.evidence.is_empty());
	assert!(experience.lesson.content.contains("## Durable knowledge"));
}

#[test]
fn test_parse_lesson_tags_single() {
	let response = r#"Some preamble text.
<lesson confidence="high" tags="auth,api" evidence="use bearer tokens not basic auth">
Bearer token auth is required for all endpoints
</lesson>
Some trailing text."#;

	let lessons = parse_lesson_tags(response, "developer", "octofs", "test-session");
	assert_eq!(lessons.len(), 1);
	assert_eq!(
		lessons[0].content,
		"Bearer token auth is required for all endpoints"
	);
	assert_eq!(lessons[0].confidence, "high");
	assert_eq!(lessons[0].importance, 0.9);
	assert_eq!(lessons[0].tags, vec!["auth", "api"]);
	assert_eq!(lessons[0].role, "developer");
	assert_eq!(lessons[0].project, "octofs");
}

#[test]
fn test_parse_lesson_tags_multiple() {
	let response = r#"
<lesson confidence="high" tags="error" evidence="no, use custom error types">
Use custom error types not anyhow
</lesson>
<lesson confidence="medium" tags="style" evidence="I prefer single PRs">
User prefers single PRs
</lesson>"#;

	let lessons = parse_lesson_tags(response, "dev", "proj", "src");
	assert_eq!(lessons.len(), 2);
	assert_eq!(lessons[0].confidence, "high");
	assert_eq!(lessons[0].importance, 0.9);
	assert_eq!(lessons[1].confidence, "medium");
	assert_eq!(lessons[1].importance, 0.6);
}

#[test]
fn test_parse_lesson_tags_empty_content_skipped() {
	let response = r#"<lesson confidence="high" tags="x" evidence="some quote">
</lesson>"#;
	let lessons = parse_lesson_tags(response, "dev", "proj", "src");
	assert_eq!(lessons.len(), 0);
}

#[test]
fn test_parse_lesson_tags_no_evidence_rejected() {
	let response = r#"<lesson confidence="high" tags="x">
This lesson has no evidence attribute and should be rejected
</lesson>"#;
	let lessons = parse_lesson_tags(response, "dev", "proj", "src");
	assert_eq!(lessons.len(), 0);
}

#[test]
fn test_parse_lesson_tags_no_lessons() {
	let response = "No lessons to extract from this session.";
	let lessons = parse_lesson_tags(response, "dev", "proj", "src");
	assert_eq!(lessons.len(), 0);
}

#[test]
fn test_parse_lesson_tags_missing_confidence_defaults_medium() {
	let response = r#"<lesson tags="test" evidence="user said something">
Some lesson without confidence attr
</lesson>"#;
	let lessons = parse_lesson_tags(response, "dev", "proj", "src");
	assert_eq!(lessons.len(), 1);
	assert_eq!(lessons[0].confidence, "medium");
	assert_eq!(lessons[0].importance, 0.6);
}

#[test]
fn test_best_overlap_finds_refinement() {
	let existing = vec![Lesson {
		content: "Bearer token auth is required for all API endpoints".into(),
		..Default::default()
	}];
	// High overlap → returns the stale lesson to supersede.
	assert!(best_overlap(
		"Bearer token auth is required for all octofs API endpoints",
		&existing
	)
	.is_some());
}

#[test]
fn test_best_overlap_none_when_unrelated() {
	let existing = vec![Lesson {
		content: "Bearer token auth is required for all API endpoints".into(),
		..Default::default()
	}];
	assert!(best_overlap("Use custom error types instead of anyhow", &existing).is_none());
}

#[test]
fn test_parse_lesson_tags_scope() {
	let response = r#"<decision>LEARN</decision>
<lesson scope="global" confidence="high" tags="style" evidence="always single PR">
Always open a single PR
</lesson>
<lesson confidence="medium" tags="proj" evidence="use X here">
This project uses X
</lesson>"#;
	let lessons = parse_lesson_tags(response, "dev", "proj", "src");
	assert_eq!(lessons.len(), 2);
	assert_eq!(lessons[0].scope, "global");
	// scope omitted → defaults to scoped.
	assert_eq!(lessons[1].scope, "scoped");
}

#[test]
fn test_extract_attr() {
	assert_eq!(
		extract_attr(r#" confidence="high" tags="a,b""#, "confidence"),
		Some("high".into())
	);
	assert_eq!(
		extract_attr(r#" confidence="high" tags="a,b""#, "tags"),
		Some("a,b".into())
	);
	assert_eq!(extract_attr(r#" confidence="high""#, "missing"), None);
}

#[test]
fn test_build_transcript() {
	let messages = vec![
		crate::session::Message {
			role: "system".into(),
			content: "You are helpful".into(),
			timestamp: 0,
			cached: false,
			cache_ttl: None,
			tool_call_id: None,
			name: None,
			tool_calls: None,
			images: None,
			videos: None,
			thinking: None,
			id: None,
		},
		crate::session::Message {
			role: "user".into(),
			content: "Fix the auth bug".into(),
			timestamp: 0,
			cached: false,
			cache_ttl: None,
			tool_call_id: None,
			name: None,
			tool_calls: None,
			images: None,
			videos: None,
			thinking: None,
			id: None,
		},
		crate::session::Message {
			role: "assistant".into(),
			content: "I'll fix it".into(),
			timestamp: 0,
			cached: false,
			cache_ttl: None,
			tool_call_id: None,
			name: None,
			tool_calls: None,
			images: None,
			videos: None,
			thinking: None,
			id: None,
		},
	];
	let transcript = build_transcript(&messages);
	assert!(!transcript.contains("system"));
	assert!(!transcript.contains("You are helpful"));
	assert!(transcript.contains("[M2 USER]: Fix the auth bug"));
	assert!(transcript.contains("[M3 ASSISTANT]: I'll fix it"));
}

#[test]
fn test_build_transcript_excludes_runtime_memory_pack() {
	let real = crate::session::Session::build_message("user", "real task");
	let recalled = crate::session::Session::build_message(
		"user",
		&crate::session::ensure_system_managed(
			"<active_memory_pack trust=\"test\">old memory</active_memory_pack>",
		),
	);
	let transcript = build_transcript(&[real, recalled]);
	assert!(transcript.contains("real task"));
	assert!(!transcript.contains("old memory"));
}

#[test]
fn test_build_transcript_keeps_reasoning_and_structured_tool_context() {
	let assistant = crate::session::Message {
		role: "assistant".into(),
		content: "Checking the configuration boundary.".into(),
		tool_calls: Some(serde_json::json!([{
			"id": "call-7",
			"function": {
				"name": "read_config",
				"arguments": {"path": "config.toml"}
			}
		}])),
		thinking: Some(serde_json::json!({
			"content": "The effective config may differ from the template.",
			"tokens": 12
		})),
		..Default::default()
	};
	let tool = crate::session::Message {
		role: "tool".into(),
		content: "enabled = true".into(),
		tool_call_id: Some("call-7".into()),
		name: Some("read_config".into()),
		..Default::default()
	};

	let transcript = build_transcript(&[assistant, tool]);

	assert!(transcript.contains("[M1 ASSISTANT]: Checking the configuration boundary."));
	assert!(transcript
		.contains("[M1 ASSISTANT THINKING]: The effective config may differ from the template."));
	assert!(transcript.contains("[M1 ASSISTANT TOOL CALLS]:"));
	assert!(transcript.contains("read_config"));
	assert!(transcript.contains("[M2 TOOL id=call-7 name=read_config]: enabled = true"));
	assert!(!transcript.contains("\"tokens\":12"));
}

#[test]
fn test_parse_unsupported_filters_out_of_range() {
	assert_eq!(
		parse_unsupported(r#"{"unsupported":[2,7,0]}"#, 3),
		Some(vec![2])
	);
	assert_eq!(parse_unsupported(r#"{"unsupported":[]}"#, 3), Some(vec![]));
}

#[test]
fn test_parse_unsupported_unusable_output_is_none() {
	// None means "verification failed" — the caller must reject everything,
	// not read it as an empty unsupported list.
	assert_eq!(parse_unsupported("not json", 3), None);
	assert_eq!(parse_unsupported(r#"{"unsupported":"nope"}"#, 3), None);
	assert_eq!(parse_unsupported("{}", 3), None);
	assert_eq!(parse_unsupported(r#"{"unsupported":[1,"#, 3), None);
}

#[test]
fn test_parse_lessons_with_evidence_keeps_quote() {
	let response = r#"<lesson confidence="high" tags="auth" evidence="use bearer tokens">
Bearer token auth is required
</lesson>"#;
	let parsed = parse_lessons_with_evidence(response, "dev", "proj", "src", 0);
	assert_eq!(parsed.len(), 1);
	assert_eq!(parsed[0].evidence, "use bearer tokens");
	assert_eq!(parsed[0].lesson.content, "Bearer token auth is required");
	assert_eq!(parsed[0].supersedes, None);
}

#[test]
fn test_parse_supersedes_only_accepts_offered_ids() {
	assert_eq!(parse_supersedes(r#" supersedes="L3""#, 5), Some(2));
	assert_eq!(parse_supersedes(r#" supersedes="3""#, 5), Some(2));
	// Never offered, never parseable, or out of range → no delete.
	assert_eq!(parse_supersedes(r#" supersedes="L9""#, 5), None);
	assert_eq!(parse_supersedes(r#" supersedes="L0""#, 5), None);
	assert_eq!(parse_supersedes(r#" supersedes="nope""#, 5), None);
	assert_eq!(parse_supersedes(r#" supersedes="""#, 5), None);
	assert_eq!(parse_supersedes(r#" confidence="high""#, 5), None);
	assert_eq!(parse_supersedes(r#" supersedes="L1""#, 0), None);
}

#[test]
fn test_head_tail_preserves_end_of_long_message() {
	let long = format!("{}CORRECTION AT THE END", "a".repeat(3000));
	let out = head_tail(&long, 500);
	assert!(out.ends_with("CORRECTION AT THE END"));
	assert!(out.starts_with("aaa"));
	assert!(out.contains("...[middle truncated]..."));
	// Short input passes through untouched.
	assert_eq!(head_tail("short", 500), "short");
}

#[test]
fn test_head_tail_utf8_safe() {
	// Multibyte throughout: both cuts must land on char boundaries or this
	// panics on slice.
	let long = "日本語テキスト".repeat(200);
	let out = head_tail(&long, 501);
	assert!(out.contains("...[middle truncated]..."));
	assert!(out.len() < long.len());
}

#[test]
fn test_build_transcript_keeps_tail_of_long_user_turn() {
	let msg = |role: &str, content: String| crate::session::Message {
		role: role.into(),
		content,
		timestamp: 0,
		cached: false,
		cache_ttl: None,
		tool_call_id: None,
		name: None,
		tool_calls: None,
		images: None,
		videos: None,
		thinking: None,
		id: None,
	};
	let transcript = build_transcript(&[msg(
		"user",
		format!("{}no, use custom error types", "x".repeat(5000)),
	)]);
	assert!(transcript.contains("no, use custom error types"));
}

#[test]
fn test_build_transcript_is_bounded_and_preserves_head_and_tail() {
	let messages = (0..100)
		.map(|index| {
			message(
				"user",
				&(0..220)
					.map(|word| format!("unique_{index}_{word}"))
					.collect::<Vec<_>>()
					.join(" "),
			)
		})
		.collect::<Vec<_>>();
	let transcript = build_transcript(&messages);
	assert!(crate::session::estimate_tokens(&transcript) <= TRANSCRIPT_MAX_TOKENS);
	assert!(transcript.contains("[M1 USER]"));
	assert!(transcript.contains("[M100 USER]"));
	assert!(transcript.matches(" USER]:").count() < messages.len());
}

#[test]
fn experience_cannot_cite_a_message_hidden_by_transcript_budget() {
	let messages = (0..120)
		.map(|index| {
			let role = if index % 2 == 0 { "user" } else { "tool" };
			message(
				role,
				&(0..180)
					.map(|word| format!("evidence_{index}_{word}"))
					.collect::<Vec<_>>()
					.join(" "),
			)
		})
		.collect::<Vec<_>>();
	let transcript = build_transcript(&messages);
	let hidden_tool = (2..=messages.len())
		.find(|number| {
			messages[number - 1].role == "tool" && !transcript.contains(&format!("[M{number} "))
		})
		.expect("bounded transcript omits at least one tool message");
	let response = format!(
			"<experience title=\"Hidden evidence\" confidence=\"high\" tags=\"x\" evidence=\"M1,M{hidden_tool}\">\n{}\n</experience>",
			experience_body()
		);
	assert!(parse_experience_tag(
		&response,
		&ExperienceParseContext {
			messages: &messages,
			transcript: &transcript,
			reconcile: &[],
			role: "developer",
			project: "octomind",
			source: "session-a",
			outcome: crate::supervisor::learning::TrajectoryOutcome::Unknown,
		},
	)
	.is_none());
}

#[test]
fn experience_value_gate_charges_only_substantial_or_outcome_labelled_work() {
	let mut messages = vec![message("user", "investigate the provider failure")];
	messages.extend((0..8).map(|index| message("tool", &format!("evidence {index}"))));
	assert!(!should_extract_experience(
		&messages,
		"tiny transcript",
		crate::supervisor::learning::TrajectoryOutcome::Unknown
	));
	assert!(should_extract_experience(
		&messages,
		&"distinct durable evidence ".repeat(4_000),
		crate::supervisor::learning::TrajectoryOutcome::Unknown
	));
	assert!(should_extract_experience(
		&messages,
		&"verified evidence ".repeat(80),
		crate::supervisor::learning::TrajectoryOutcome::Verified
	));
}

#[test]
fn test_reconcile_candidates_caps_and_reserves_global() {
	let scoped: Vec<Lesson> = (0..50)
		.map(|i| lesson(&format!("scoped {}", i), "scoped", 0.9))
		.collect();
	let global: Vec<Lesson> = (0..10)
		.map(|i| lesson(&format!("global {}", i), "global", 0.5))
		.collect();
	let out = reconcile_candidates(&scoped, &global);
	assert_eq!(out.len(), RECONCILE_CANDIDATES);
	// Global keeps its floor even though every scoped entry outranks it.
	assert_eq!(
		out.iter().filter(|l| l.scope == "global").count(),
		RECONCILE_GLOBAL_MIN
	);
}

#[test]
fn test_reconcile_candidates_excludes_orientation() {
	let orientation = Lesson {
		memory_type: "orientation".into(),
		..lesson("auth is delegated to octolib", "scoped", 0.9)
	};
	let experience = Lesson {
		memory_type: "experience".into(),
		..lesson("a long trajectory", "scoped", 0.8)
	};
	let out = reconcile_candidates(
		&[orientation, experience, lesson("a rule", "scoped", 0.5)],
		&[],
	);
	assert_eq!(out.len(), 1);
	assert_eq!(out[0].content, "a rule");
}

#[test]
fn test_format_existing_emits_ids_and_scope() {
	assert_eq!(format_existing(&[]), "(none)");
	let out = format_existing(&[
		lesson("scoped rule", "scoped", 0.9),
		lesson("global rule", "global", 0.9),
	]);
	assert!(out.contains("[L1] (this project/role, medium) scoped rule"));
	assert!(out.contains("[L2] (global, medium) global rule"));
}

#[tokio::test]
async fn learn_decision_verifies_evidence_supersedes_and_stores_orientation() {
	use crate::session::chat::test_support::{
		fake_provider_config, final_response, spawn_stub, ENV_LOCK,
	};
	let _guard = ENV_LOCK.lock().await;
	let _data = TestDataDir::new();
	let role = "__learn_e2e_role";
	let project = "__learn_e2e_project";
	let dir = crate::directories::get_learning_dir(role, project).unwrap();
	let _ = std::fs::remove_dir_all(&dir);

	// An existing stale rule the new lesson explicitly supersedes via L1.
	let backend = FileBackend;
	let mut old = lesson(
		"Bearer token auth is required for all API endpoints",
		"scoped",
		0.9,
	);
	old.memory_type = "learning".to_string();
	old.role = role.to_string();
	old.project = project.to_string();
	backend.store(&old).await.unwrap();

	let response = r#"<decision>LEARN</decision>
<lesson confidence="high" tags="auth" evidence="always use bearer tokens for every api call" supersedes="L1">Bearer token auth is required for all API endpoints, including internal ones</lesson>
<orientation confidence="high" tags="auth" evidence="M1">The subject authenticates every API call with bearer tokens</orientation>"#;
	let url = spawn_stub(vec![
		final_response(response),
		final_response(r#"{"unsupported":[]}"#),
	])
	.await;
	std::env::set_var("OLLAMA_API_URL", &url);
	let mut config = fake_provider_config();
	config.supervisor.learning.model = "ollama:fake-model".to_string();
	config.supervisor.gate.verifier_model = "ollama:fake-model".to_string();
	config.supervisor.learning.evolution.enabled = false;

	// No tool turn: the experience value gate stays closed, so exactly one
	// extraction call and one verifier call hit the stub.
	let messages = vec![
		message("user", "always use bearer tokens for every api call"),
		message("assistant", "understood, bearer tokens everywhere"),
	];
	let stored = run_extraction(
		&messages,
		&config,
		role,
		project,
		"learn-session",
		crate::supervisor::learning::TrajectoryOutcome::Unknown,
	)
	.await
	.expect("extraction succeeds");
	assert_eq!(stored, 2);

	let memories = backend.retrieve_all(role, project).await.unwrap();
	assert_eq!(memories.len(), 2);
	let lesson_stored = memories
		.iter()
		.find(|memory| memory.memory_type == "learning")
		.unwrap();
	assert_eq!(
		lesson_stored.content,
		"Bearer token auth is required for all API endpoints, including internal ones"
	);
	assert!(lesson_stored
		.evidence
		.contains(&"session://learn-session/message/1".to_string()));
	// The superseded stale rule is gone.
	assert!(!memories.iter().any(|memory| memory.content == old.content));
	let orientation = memories
		.iter()
		.find(|memory| memory.memory_type == "orientation")
		.unwrap();
	assert_eq!(
		orientation.outcome,
		crate::supervisor::learning::TrajectoryOutcome::Unknown
	);

	std::env::remove_var("OLLAMA_API_URL");
	let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn learn_path_rejects_fabricated_evidence_entirely() {
	use crate::session::chat::test_support::{
		fake_provider_config, final_response, spawn_stub, ENV_LOCK,
	};
	let _guard = ENV_LOCK.lock().await;
	let _data = TestDataDir::new();
	let role = "__learn_fabricated_role";
	let project = "__learn_fabricated_project";
	let dir = crate::directories::get_learning_dir(role, project).unwrap();
	let _ = std::fs::remove_dir_all(&dir);

	let response = r#"<decision>LEARN</decision>
<lesson confidence="high" tags="auth" evidence="quote that appears in no user turn">Fabricated rule with no verbatim evidence</lesson>"#;
	let url = spawn_stub(vec![final_response(response)]).await;
	std::env::set_var("OLLAMA_API_URL", &url);
	let mut config = fake_provider_config();
	config.supervisor.learning.model = "ollama:fake-model".to_string();
	config.supervisor.gate.verifier_model = "ollama:fake-model".to_string();

	let messages = vec![message("user", "an unrelated real user turn")];
	let stored = run_extraction(
		&messages,
		&config,
		role,
		project,
		"fabricated-session",
		crate::supervisor::learning::TrajectoryOutcome::Unknown,
	)
	.await
	.unwrap();
	assert_eq!(stored, 0);
	assert!(FileBackend
		.retrieve_all(role, project)
		.await
		.unwrap()
		.is_empty());

	std::env::remove_var("OLLAMA_API_URL");
	let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn learn_path_drops_lessons_the_verifier_marks_unsupported() {
	use crate::session::chat::test_support::{
		fake_provider_config, final_response, spawn_stub, ENV_LOCK,
	};
	let _guard = ENV_LOCK.lock().await;
	let _data = TestDataDir::new();
	let role = "__learn_unsupported_role";
	let project = "__learn_unsupported_project";
	let dir = crate::directories::get_learning_dir(role, project).unwrap();
	let _ = std::fs::remove_dir_all(&dir);

	let response = r#"<decision>LEARN</decision>
<lesson confidence="high" tags="auth" evidence="verbatim user quote survives the gate">A rule the verifier will reject</lesson>"#;
	let url = spawn_stub(vec![
		final_response(response),
		final_response(r#"{"unsupported":[1]}"#),
	])
	.await;
	std::env::set_var("OLLAMA_API_URL", &url);
	let mut config = fake_provider_config();
	config.supervisor.learning.model = "ollama:fake-model".to_string();
	config.supervisor.gate.verifier_model = "ollama:fake-model".to_string();

	let messages = vec![message("user", "verbatim user quote survives the gate")];
	let stored = run_extraction(
		&messages,
		&config,
		role,
		project,
		"unsupported-session",
		crate::supervisor::learning::TrajectoryOutcome::Unknown,
	)
	.await
	.unwrap();
	assert_eq!(stored, 0);
	assert!(FileBackend
		.retrieve_all(role, project)
		.await
		.unwrap()
		.is_empty());

	std::env::remove_var("OLLAMA_API_URL");
	let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn duplicate_experience_trajectory_is_skipped() {
	use crate::session::chat::test_support::{
		fake_provider_config, final_response, spawn_stub, ENV_LOCK,
	};
	let _guard = ENV_LOCK.lock().await;
	let _data = TestDataDir::new();
	let role = "__experience_dup_role";
	let project = "__experience_dup_project";
	let dir = crate::directories::get_learning_dir(role, project).unwrap();
	let _ = std::fs::remove_dir_all(&dir);

	// The same trajectory memory is already stored from this very session.
	let backend = FileBackend;
	let mut existing = lesson(&experience_body(), "scoped", 0.9);
	existing.memory_type = "experience".to_string();
	existing.role = role.to_string();
	existing.project = project.to_string();
	existing.source = "dup-session".to_string();
	existing.outcome = crate::supervisor::learning::TrajectoryOutcome::Verified;
	backend.store(&existing).await.unwrap();

	let experience = format!(
		"<experience title=\"Provider continuation recovery\" confidence=\"high\" tags=\"provider\" evidence=\"M1,M2\">\n{}\n</experience>",
		experience_body()
	);
	let url = spawn_stub(vec![
		final_response("<decision>NONE</decision>"),
		final_response(&experience),
	])
	.await;
	std::env::set_var("OLLAMA_API_URL", &url);
	let mut config = fake_provider_config();
	config.supervisor.learning.model = "ollama:fake-model".to_string();
	config.supervisor.gate.verifier_model = "ollama:fake-model".to_string();

	let messages = vec![
		message("user", "never silently switch the resolved model"),
		message(
			"tool",
			&format!(
				"provider error: invalid continuation id c_123. {}",
				"diagnostic evidence confirms the continuation belongs to the resolved provider. "
					.repeat(300)
			),
		),
	];
	let stored = run_extraction(
		&messages,
		&config,
		role,
		project,
		"dup-session",
		crate::supervisor::learning::TrajectoryOutcome::Verified,
	)
	.await
	.unwrap();
	assert_eq!(stored, 0);
	let memories = backend.retrieve_all(role, project).await.unwrap();
	assert_eq!(memories.len(), 1);
	assert_eq!(memories[0].memory_type, "experience");

	std::env::remove_var("OLLAMA_API_URL");
	let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn detached_and_snapshot_wrappers_honor_the_enabled_flag() {
	use crate::session::chat::test_support::{fake_provider_config, ENV_LOCK};
	let _guard = ENV_LOCK.lock().await;
	let _data = TestDataDir::new();
	let work = tempfile::tempdir().unwrap();
	let mut config = fake_provider_config();

	config.supervisor.learning.enabled = false;
	assert!(spawn_lesson_extraction_snapshot(
		Vec::new(),
		&config,
		"developer".to_string(),
		Some(work.path()),
		"wrapper-session".to_string(),
		crate::supervisor::learning::TrajectoryOutcome::Unknown,
	)
	.is_none());
	extract_lessons_detached(
		Vec::new(),
		config.clone(),
		"developer".to_string(),
		"wrapper-project".to_string(),
		"wrapper-session".to_string(),
		crate::supervisor::learning::TrajectoryOutcome::Unknown,
	)
	.await
	.unwrap();

	// Enabled with an empty transcript: the run stops before any LLM call and
	// derives the project from the supplied working directory.
	config.supervisor.learning.enabled = true;
	let handle = spawn_lesson_extraction_snapshot(
		Vec::new(),
		&config,
		"developer".to_string(),
		Some(work.path()),
		"wrapper-session".to_string(),
		crate::supervisor::learning::TrajectoryOutcome::Unknown,
	)
	.expect("enabled learning spawns extraction");
	handle.await.unwrap();
	let project = work
		.path()
		.file_name()
		.and_then(|name| name.to_str())
		.unwrap()
		.to_string();
	assert!(FileBackend
		.retrieve_all("developer", &project)
		.await
		.unwrap()
		.is_empty());
}

#[test]
fn purpose_for_maps_every_call_kind_to_its_routing_purpose() {
	use crate::providers::ModelPurpose;
	use crate::supervisor::stats::CallKind;
	assert_eq!(purpose_for(CallKind::Gate), ModelPurpose::SupervisorGate);
	assert_eq!(purpose_for(CallKind::Resolve), ModelPurpose::SupervisorGate);
	assert_eq!(purpose_for(CallKind::Plan), ModelPurpose::SupervisorGate);
	assert_eq!(
		purpose_for(CallKind::Condense),
		ModelPurpose::SupervisorCondense
	);
	assert_eq!(
		purpose_for(CallKind::Distill),
		ModelPurpose::SupervisorDistill
	);
	assert_eq!(
		purpose_for(CallKind::Recall),
		ModelPurpose::SupervisorRecall
	);
}
