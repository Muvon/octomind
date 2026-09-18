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

//! The distill seam: candidate lessons grounded by the evaluation model in
//! place of the chat verifier, with the chat verifier as the fallback. The
//! extraction call itself always hits the scripted chat stub, so a verifier
//! call the seam should have replaced reads "SCRIPT EXHAUSTED", rejects every
//! candidate, and shows up as nothing stored. `install_fake_evaluation` holds
//! `ENV_LOCK`, so these tests never take it themselves.

use super::*;
use crate::session::chat::test_support::{
	evaluate_counter, fake_provider_config, final_response, install_fake_evaluation, nouls,
	spawn_stub, FakeEvaluationStep,
};
use crate::supervisor::evaluate::{distill_question_id, Seam};
use crate::supervisor::learning::TrajectoryOutcome;
use octolib::evaluation::Question;

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

fn message(role: &str, content: &str) -> crate::session::Message {
	crate::session::Message {
		role: role.to_string(),
		content: content.to_string(),
		..Default::default()
	}
}

fn config(distill: bool) -> Config {
	let mut config = fake_provider_config();
	config.supervisor.enabled = true;
	config.supervisor.model.model = Some("ollama:fake-model".to_string());
	config.supervisor.learning.evolution.enabled = false;
	config.supervisor.evaluate.distill = distill;
	config
}

const BACKOFF_RULE: &str = "For retries in the http client use exponential backoff with jitter";
const SUITE_RULE: &str = "Always run the full test suite before every commit";

const EXTRACTION: &str = r#"<decision>LEARN</decision>
<lesson confidence="high" tags="http" evidence="use exponential backoff with jitter for retries">For retries in the http client use exponential backoff with jitter</lesson>
<lesson confidence="medium" tags="tests" evidence="run the parser tests">Always run the full test suite before every commit</lesson>"#;

fn messages() -> Vec<crate::session::Message> {
	vec![
		message(
			"user",
			"use exponential backoff with jitter for retries, then run the parser tests",
		),
		message(
			"assistant",
			"done: backoff with jitter added, parser tests green",
		),
	]
}

/// Noul answers `l0..ln` with the given probabilities.
fn answers(probabilities: &[f64]) -> FakeEvaluationStep {
	let pairs: Vec<(String, f64)> = probabilities
		.iter()
		.enumerate()
		.map(|(slot, p)| (distill_question_id(slot), *p))
		.collect();
	let borrowed: Vec<(&str, f64)> = pairs.iter().map(|(id, p)| (id.as_str(), *p)).collect();
	FakeEvaluationStep::Answers(nouls(&borrowed))
}

/// Chat calls under `CallKind::Distill`: the extraction call plus, when the
/// seam did not answer, one verifier call.
fn distill_chat_calls() -> u64 {
	crate::supervisor::stats::snapshot()
		.and_then(|s| s.get("distill_calls")?.as_u64())
		.unwrap_or(0)
}

async fn extract(
	config: &Config,
	role: &str,
	project: &str,
	messages: &[crate::session::Message],
	outcome: TrajectoryOutcome,
	stub: Vec<serde_json::Value>,
) -> (usize, Vec<Lesson>) {
	let url = spawn_stub(stub).await;
	std::env::set_var("OLLAMA_API_URL", &url);
	let stored = run_extraction(messages, config, role, project, "distill-session", outcome)
		.await
		.expect("extraction succeeds");
	std::env::remove_var("OLLAMA_API_URL");
	let memories = FileBackend.retrieve_all(role, project).await.unwrap();
	(stored, memories)
}

#[tokio::test]
async fn grounded_lessons_are_kept_and_the_rest_rejected_without_a_chat_verifier() {
	let fake = install_fake_evaluation(vec![answers(&[0.93, 0.11])]).await;
	let _data = TestDataDir::new();
	let calls = distill_chat_calls();
	let applied = evaluate_counter(Seam::Distill, "applied");
	let (stored, memories) = extract(
		&config(true),
		"__distill_seam_role",
		"__distill_seam_project",
		&messages(),
		TrajectoryOutcome::Unknown,
		vec![final_response(EXTRACTION)],
	)
	.await;
	assert_eq!(stored, 1);
	assert_eq!(memories.len(), 1);
	assert_eq!(memories[0].content, BACKOFF_RULE);
	assert_eq!(distill_chat_calls() - calls, 1, "the extraction call only");
	assert_eq!(evaluate_counter(Seam::Distill, "applied") - applied, 1);

	let requests = fake.requests();
	assert_eq!(requests.len(), 1);
	let request = &requests[0];
	assert_eq!(request.max_retries, 0);
	assert_eq!(request.timeout, std::time::Duration::from_secs(5));
	let lessons = request.state["lessons"].as_array().expect("lessons array");
	assert_eq!(lessons.len(), 2);
	assert_eq!(lessons[0]["number"], 1);
	assert_eq!(lessons[0]["rule"], BACKOFF_RULE);
	assert_eq!(
		lessons[0]["evidence"],
		"use exponential backoff with jitter for retries"
	);
	assert_eq!(lessons[1]["number"], 2);
	assert_eq!(lessons[1]["rule"], SUITE_RULE);
	assert_eq!(lessons[1]["evidence"], "run the parser tests");
	let transcript = request.state["transcript"].as_str().expect("transcript");
	assert!(transcript.contains("use exponential backoff with jitter for retries"));
	assert!(transcript.chars().count() <= VERIFY_TRANSCRIPT_CHARS);
	for (slot, prefix) in [(0, "Lesson 1: "), (1, "Lesson 2: ")] {
		match &request.questions[&distill_question_id(slot)] {
			Question::Noul { instructions, .. } => {
				assert!(instructions.as_str().unwrap().starts_with(prefix));
			}
			_ => panic!("distill questions are Nouls"),
		}
	}
}

#[tokio::test]
async fn an_unavailable_evaluation_runs_the_chat_verifier_once_with_every_candidate() {
	let fake =
		install_fake_evaluation(vec![FakeEvaluationStep::MissingKey("CLOUDFLARE_API_KEY")]).await;
	let _data = TestDataDir::new();
	let calls = distill_chat_calls();
	let unavailable = evaluate_counter(Seam::Distill, "unavailable");
	let applied = evaluate_counter(Seam::Distill, "applied");
	let (stored, memories) = extract(
		&config(true),
		"__distill_fallback_role",
		"__distill_fallback_project",
		&messages(),
		TrajectoryOutcome::Unknown,
		vec![
			final_response(EXTRACTION),
			final_response(r#"{"unsupported":[2]}"#),
		],
	)
	.await;
	assert_eq!(stored, 1);
	assert_eq!(memories[0].content, BACKOFF_RULE);
	assert_eq!(
		distill_chat_calls() - calls,
		2,
		"extraction plus one verifier call"
	);
	assert_eq!(
		evaluate_counter(Seam::Distill, "unavailable") - unavailable,
		1
	);
	assert_eq!(evaluate_counter(Seam::Distill, "applied") - applied, 0);
	assert_eq!(fake.requests().len(), 1);
}

#[tokio::test]
async fn the_seam_off_makes_no_evaluation_request_and_keeps_the_chat_verifier() {
	let fake = install_fake_evaluation(vec![answers(&[0.9, 0.9])]).await;
	let _data = TestDataDir::new();
	let calls = distill_chat_calls();
	let (stored, _) = extract(
		&config(false),
		"__distill_off_role",
		"__distill_off_project",
		&messages(),
		TrajectoryOutcome::Unknown,
		vec![
			final_response(EXTRACTION),
			final_response(r#"{"unsupported":[]}"#),
		],
	)
	.await;
	assert_eq!(stored, 2);
	assert!(fake.requests().is_empty());
	assert_eq!(distill_chat_calls() - calls, 2);
}

fn experience_body() -> String {
	format!(
		"## Objective\nDiagnose why an authenticated request repeatedly failed across the provider boundary.\n\n## Durable knowledge\n{}\n\n## Outcome and evidence\nThe tool result established that the provider rejects a stale continuation identifier, while the user confirmed that fallback to another resolved model is forbidden. The verified recovery preserves the resolved model and clears only the invalid continuation.\n\n## Reuse conditions\nApply this when a resumed request fails before tool execution with an invalid continuation identifier. Re-check the current provider contract because external APIs may change.",
		"The continuation belongs to the exact resolved provider and model identity. Recovery must keep that identity stable, distinguish transport failure from task failure, and avoid silent fallback. ".repeat(3)
	)
}

/// Stored fields the seam must leave exactly as the chat verifier would.
fn stored_fields(memories: &[Lesson]) -> Vec<(String, String, String, String, String)> {
	let mut fields: Vec<_> = memories
		.iter()
		.map(|m| {
			(
				m.memory_type.clone(),
				m.content.clone(),
				format!("{:.2}", m.importance),
				m.confidence.clone(),
				m.scope.clone(),
			)
		})
		.collect();
	fields.sort();
	fields
}

#[tokio::test]
async fn experiences_keep_the_chat_verifier_and_kept_lessons_store_the_same_fields() {
	let fake = install_fake_evaluation(vec![answers(&[0.9, 0.9])]).await;
	let _data = TestDataDir::new();
	// A tool turn plus a verified outcome opens the experience value gate.
	let mut messages = messages();
	messages.push(message(
		"tool",
		&"provider error: invalid continuation id c_123. diagnostic evidence confirms it. "
			.repeat(60),
	));
	let experience = format!(
		"<experience title=\"Provider continuation recovery\" confidence=\"high\" tags=\"provider\" evidence=\"M1,M3\">\n{}\n</experience>",
		experience_body()
	);
	let superseding = EXTRACTION.replacen("tags=\"http\"", "tags=\"http\" supersedes=\"L1\"", 1);
	let old = |role: &str, project: &str| Lesson {
		content: "Retries use a fixed one second delay".into(),
		memory_type: "learning".into(),
		scope: "scoped".into(),
		importance: 0.9,
		role: role.into(),
		project: project.into(),
		created: "2026-01-01T00:00:00Z".into(),
		..Default::default()
	};
	for (role, project) in [
		("__distill_same_on_role", "__distill_same_on_project"),
		("__distill_same_off_role", "__distill_same_off_project"),
	] {
		FileBackend.store(&old(role, project)).await.unwrap();
	}

	// Seam on: extraction, experience extraction, experience verdict — and no
	// lesson verifier call, or the exhausted script would reject both lessons.
	let on = extract(
		&config(true),
		"__distill_same_on_role",
		"__distill_same_on_project",
		&messages,
		TrajectoryOutcome::Verified,
		vec![
			final_response(&superseding),
			final_response(&experience),
			final_response(r#"{"supported":true}"#),
		],
	)
	.await;
	let off = extract(
		&config(false),
		"__distill_same_off_role",
		"__distill_same_off_project",
		&messages,
		TrajectoryOutcome::Verified,
		vec![
			final_response(&superseding),
			final_response(&experience),
			final_response(r#"{"supported":true}"#),
			final_response(r#"{"unsupported":[]}"#),
		],
	)
	.await;
	assert_eq!(on.0, 3, "two lessons and one experience");
	assert_eq!(off.0, 3);
	assert_eq!(fake.requests().len(), 1, "the seam scored the on-run only");
	assert_eq!(stored_fields(&on.1), stored_fields(&off.1));
	assert!(on.1.iter().any(|m| m.memory_type == "experience"));
	assert!(
		on.1.iter()
			.all(|m| m.content != "Retries use a fixed one second delay"),
		"the superseded lesson is gone with the seam on"
	);
	assert!(off
		.1
		.iter()
		.all(|m| m.content != "Retries use a fixed one second delay"));
}
