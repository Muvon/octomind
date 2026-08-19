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

//! Retrieval-and-injection tests against the real file backend under a
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

	// Follow-up call (first_call=false): no LLM, no global tier. An empty
	// user input takes the deterministic embedding-free branch (plain
	// recency listing); a non-empty one would need the MiniLM warmup, which
	// only other tests happen to trigger — never depend on that here.
	let (_tx1, rx1) = cancel_pair();
	let (text, injected_now) = retrieve_and_format(
		&config,
		"",
		ROLE,
		proj,
		false,
		&std::collections::HashSet::new(),
		rx1,
	)
	.await;
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
	assert!(text.contains("<recall>"), "missing recall wrapper:\n{text}");
	assert!(
		text.contains("<orientation"),
		"orientation block missing:\n{text}"
	);
	assert!(!injected_now.is_empty());

	// Same call with everything already injected: nothing new to say.
	let already: std::collections::HashSet<String> = injected_now.into_iter().collect();
	let (_tx2, rx2) = cancel_pair();
	let (text2, injected2) =
		retrieve_and_format(&config, "", ROLE, proj, false, &already, rx2).await;
	assert!(text2.is_empty(), "re-injection must be suppressed: {text2}");
	assert!(injected2.is_empty());

	cleanup(proj);
}

#[tokio::test]
async fn test_first_call_retrieval_uses_keyword_query() {
	let _guard = ENV_LOCK.lock().await;
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
		&std::collections::HashSet::new(),
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
		&std::collections::HashSet::new(),
		rx4,
	)
	.await;
	assert!(text.is_empty());
	assert!(injected.is_empty());
}
