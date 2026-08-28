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

//! Retrieval-and-active-pack tests against the real file backend under a
//! throwaway role/project scope. Follow-up-call retrieval is LLM-free
//! (keyword/recency ranking), so most of the flow runs with no stub; the
//! first-call keyword query rides the scripted fake provider.

use super::*;
use crate::session::chat::test_support::{
	fake_provider_config, final_response, spawn_stub, ENV_LOCK,
};
use crate::supervisor::learning::backend::create_backend;
use crate::supervisor::learning::Lesson;

const ROLE: &str = "__inject_test_role";

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

fn cleanup(proj: &str) {
	if let Ok(dir) = crate::directories::get_learning_dir(ROLE, proj) {
		let _ = std::fs::remove_dir_all(dir);
	}
}

fn lesson(proj: &str, content: &str, memory_type: &str, tags: &[&str]) -> Lesson {
	Lesson {
		content: content.to_string(),
		title: String::new(),
		memory_type: memory_type.to_string(),
		importance: 0.8,
		confidence: "high".to_string(),
		tags: tags.iter().map(|t| t.to_string()).collect(),
		source: "inject-test".to_string(),
		role: ROLE.to_string(),
		project: proj.to_string(),
		scope: "scoped".to_string(),
		created: chrono::Utc::now().to_rfc3339(),
		..Default::default()
	}
}

/// Keep the sender alive across the call — a dropped sender reads as a
/// cancelled operation to the LLM-call cancellation wrapper.
fn cancel_pair() -> (
	tokio::sync::watch::Sender<bool>,
	tokio::sync::watch::Receiver<bool>,
) {
	tokio::sync::watch::channel(false)
}

#[tokio::test]
async fn test_followup_retrieval_injects_and_dedupes() {
	let _guard = ENV_LOCK.lock().await;
	let _data = TestDataDir::new();
	let proj = "__inject_test_proj_followup";
	cleanup(proj);
	let config = fake_provider_config();
	let backend = create_backend(&config.supervisor.learning);
	backend
		.store(
			&lesson(
				proj,
				"always run the test suite on the dev box",
				"learning",
				&["testing", "box"],
			),
			&config,
		)
		.await
		.expect("store lesson");
	backend
		.store(
			&lesson(
				proj,
				"the build uses a cargo workspace",
				"orientation",
				&["build"],
			),
			&config,
		)
		.await
		.expect("store orientation");

	// Follow-up call (first_call=false): no LLM. An empty
	// user input takes the deterministic embedding-free branch (plain
	// recency listing); a non-empty one would need the MiniLM warmup, which
	// only other tests happen to trigger — never depend on that here.
	let (_tx1, rx1) = cancel_pair();
	let (text, injected_now) = retrieve_and_format(&config, "", ROLE, proj, false, rx1).await;
	let dir = crate::directories::get_learning_dir(ROLE, proj).expect("dir");
	let files: Vec<String> = std::fs::read_dir(&dir)
		.map(|entries| {
			entries
				.filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().to_string()))
				.collect()
		})
		.unwrap_or_default();
	let all = backend
		.retrieve_all(ROLE, proj, &config)
		.await
		.unwrap_or_default();
	assert!(
		text.contains("always run the test suite on the dev box"),
		"lesson missing from recall block:\ntext={text:?}\nstore dir {dir:?} files={files:?}\nretrieve_all={}",
		all.len()
	);
	assert!(
		text.contains("<active_memory_pack "),
		"missing active memory wrapper:\n{text}"
	);
	assert!(
		text.contains("<orientation"),
		"orientation block missing:\n{text}"
	);
	assert!(!injected_now.is_empty());

	// A replacement pack may deliberately select the same relevant memory again;
	// it replaces the previous pack instead of accumulating beside it.
	let (_tx2, rx2) = cancel_pair();
	let (text2, selected2) = retrieve_and_format(&config, "", ROLE, proj, false, rx2).await;
	assert!(text2.contains("always run the test suite on the dev box"));
	assert!(!selected2.is_empty());
	assert!(
		crate::session::estimate_tokens(&text2) <= MAX_MEMORY_PACK_TOKENS,
		"active pack exceeded token budget"
	);

	cleanup(proj);
}

#[tokio::test]
async fn test_first_call_retrieval_uses_keyword_query() {
	let _guard = ENV_LOCK.lock().await;
	let _data = TestDataDir::new();
	let proj = "__inject_test_proj_first";
	cleanup(proj);
	let mut config = fake_provider_config();
	config.supervisor.learning.model = "ollama:fake-model".to_string();
	let backend = create_backend(&config.supervisor.learning);
	backend
		.store(
			&lesson(
				proj,
				"prefer rsync over scp for box deployments",
				"learning",
				&["deploy", "rsync"],
			),
			&config,
		)
		.await
		.expect("store lesson");

	// First call: the keyword-query model call happens against the stub.
	let url = spawn_stub(vec![final_response("deploy\nrsync\nbox\n")]).await;
	std::env::set_var("OLLAMA_API_URL", &url);

	let (_tx3, rx3) = cancel_pair();
	let (text, _) = retrieve_and_format(
		&config,
		"deploy the service to the box",
		ROLE,
		proj,
		true,
		rx3,
	)
	.await;
	assert!(
		text.contains("prefer rsync over scp"),
		"scoped lesson missing on first call:\n{text}"
	);

	std::env::remove_var("OLLAMA_API_URL");
	cleanup(proj);
}

#[tokio::test]
async fn test_disabled_learning_injects_nothing() {
	let mut config = fake_provider_config();
	config.supervisor.learning.enabled = false;
	let (_tx4, rx4) = cancel_pair();
	let (text, injected) = retrieve_and_format(
		&config,
		"anything",
		ROLE,
		"__inject_test_proj_disabled",
		true,
		rx4,
	)
	.await;
	assert!(text.is_empty());
	assert!(injected.is_empty());
}

#[tokio::test]
async fn test_active_pack_is_token_bounded_with_stable_pack_ids() {
	let _guard = ENV_LOCK.lock().await;
	let _data = TestDataDir::new();
	let proj = "__inject_test_proj_budget";
	cleanup(proj);
	let config = fake_provider_config();
	let backend = create_backend(&config.supervisor.learning);
	for i in 0..20 {
		backend
			.store(
				&lesson(
					proj,
					&format!("memory {i}: {}", "long reusable context ".repeat(120)),
					"learning",
					&["memory"],
				),
				&config,
			)
			.await
			.expect("store lesson");
	}

	let (_tx, rx) = cancel_pair();
	let (text, selected) = retrieve_and_format(&config, "", ROLE, proj, false, rx).await;
	assert!(!selected.is_empty());
	assert!(selected.len() < 20, "long memories must be budget-pruned");
	assert!(crate::session::estimate_tokens(&text) <= MAX_MEMORY_PACK_TOKENS);
	for (index, memory) in selected.iter().enumerate() {
		assert_eq!(memory.id, format!("M{}", index + 1));
		assert!(text.contains(&format!("[{} ", memory.id)));
	}

	cleanup(proj);
}

#[tokio::test]
async fn test_long_experience_injects_card_with_full_file_reference() {
	let _guard = ENV_LOCK.lock().await;
	let _data = TestDataDir::new();
	let proj = "__inject_test_proj_experience";
	cleanup(proj);
	let config = fake_provider_config();
	let backend = create_backend(&config.supervisor.learning);
	let mut experience = lesson(
		proj,
		&format!(
			"## Objective\nRecover provider continuation safely.\n\n## Durable knowledge\n{}\n\n## Outcome and evidence\nVerified by the provider error and successful retry.\n\n## Reuse conditions\nUse only for invalid continuation failures.",
			"Keep the resolved provider identity stable and clear only the invalid continuation. ".repeat(180)
		),
		"experience",
		&["provider", "continuation"],
	);
	experience.title = "Provider continuation recovery".to_string();
	experience.outcome = crate::supervisor::learning::TrajectoryOutcome::Verified;
	experience.related = vec!["related-memory-id".to_string()];
	experience.evidence = vec!["session://s/message/3".to_string()];
	backend
		.store(&experience, &config)
		.await
		.expect("store experience");

	let (_tx, rx) = cancel_pair();
	let (text, selected) = retrieve_and_format(&config, "", ROLE, proj, false, rx).await;
	assert!(text.contains("<experiences>"));
	assert!(text.contains("Provider continuation recovery"));
	assert!(text.contains("full memory:"));
	assert!(text.contains("related-memory-id"));
	assert!(text.contains("session://s/message/3"));
	assert!(selected
		.iter()
		.any(|memory| memory.content == experience.content));
	assert!(crate::session::estimate_tokens(&text) <= MAX_MEMORY_PACK_TOKENS);

	cleanup(proj);
}

#[test]
fn retrieval_rewrite_validation_accepts_keywords_and_rejects_answers() {
	let valid = validate_retrieval_patterns(
		"oauth callback\npkce verifier\nstate parameter\nsession fixation\ncsrf protection",
	)
	.expect("valid keyword rewrite");
	assert_eq!(valid.len(), 5);
	assert!(validate_retrieval_patterns(
		"Good morning!\nThis is the answer to the user's request.\ntranslation\nicelandic\ngreeting\nlanguage"
	)
	.is_err());
	assert!(validate_retrieval_patterns("one\ntwo").is_err());
}
