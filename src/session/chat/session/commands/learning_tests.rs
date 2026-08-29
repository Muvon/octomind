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

//! Tests for the `/learning` command against the real file backend under a
//! throwaway role scope. `clear` is scope-local and must never touch the global
//! tier.

use super::*;
use crate::supervisor::learning::backend::FileBackend;
use crate::supervisor::learning::Lesson;
use serial_test::serial;

const ROLE: &str = "__learning_cmd_role";

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

fn project() -> String {
	std::env::current_dir()
		.ok()
		.and_then(|d| d.file_name().map(|n| n.to_string_lossy().to_string()))
		.unwrap_or_default()
}

fn cleanup() {
	if let Ok(dir) = crate::directories::get_learning_dir(ROLE, &project()) {
		let _ = std::fs::remove_dir_all(dir);
	}
}

fn test_session() -> ChatSession {
	let mut session = ChatSession::for_tests(Vec::new());
	session.role = ROLE.to_string();
	session
}

async fn store(content: &str) {
	let backend = FileBackend;
	backend
		.store(&Lesson {
			content: content.to_string(),
			title: String::new(),
			memory_type: "learning".to_string(),
			importance: 0.7,
			confidence: "high".to_string(),
			tags: vec!["cmd-test".to_string()],
			source: "learning-cmd-test".to_string(),
			role: ROLE.to_string(),
			project: project(),
			scope: "scoped".to_string(),
			created: chrono::Utc::now().to_rfc3339(),
			..Default::default()
		})
		.await
		.expect("store lesson");
}

fn learning_data(result: CommandResult) -> serde_json::Value {
	match result {
		CommandResult::HandledWithOutput(output) => match *output {
			CommandOutput::Learning { data } => data,
			other => panic!("expected Learning output, got {other:?}"),
		},
		other => panic!("expected HandledWithOutput, got {other:?}"),
	}
}

#[serial]
#[tokio::test]
async fn test_learning_list_and_delete_lifecycle() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let _data = TestDataDir::new();
	cleanup();
	let mut session = test_session();
	store("first learning-cmd lesson about widgets").await;
	store("second learning-cmd lesson about gadgets").await;

	// Bare /learning lists both (global-tier entries from the machine may
	// follow ours, so assert containment, not exact counts).
	let data = learning_data(
		handle_learning(&mut session, &[])
			.await
			.expect("list dispatches"),
	);
	assert_eq!(data["subcommand"], "list");
	assert_eq!(data["storage"]["hot_items"], 2);
	assert_eq!(data["storage"]["cold_items"], 0);
	assert_eq!(data["storage"]["by_type"]["learning"]["hot"], 2);
	let listed = data["lessons"].to_string();
	assert!(listed.contains("first learning-cmd lesson"), "{listed}");
	assert!(listed.contains("second learning-cmd lesson"), "{listed}");
	let shown = learning_data(
		handle_learning(&mut session, &["show", "1"])
			.await
			.expect("show dispatches"),
	);
	assert_eq!(shown["subcommand"], "show");
	assert!(shown["content"]
		.as_str()
		.unwrap_or_default()
		.contains("learning-cmd"));
	assert!(shown["path"]
		.as_str()
		.is_some_and(|path| path.ends_with(".md")));

	// Glob filter narrows to the matching lesson
	let data = learning_data(
		handle_learning(&mut session, &["list", "*widgets*"])
			.await
			.expect("filtered list dispatches"),
	);
	let listed = data["lessons"].to_string();
	assert!(listed.contains("widgets"), "{listed}");
	assert!(!listed.contains("gadgets"), "{listed}");

	// Scoped lessons sort before the global tier, so index 1 is ours
	let data = learning_data(
		handle_learning(&mut session, &["delete", "1"])
			.await
			.expect("delete dispatches"),
	);
	assert_eq!(data["subcommand"], "delete", "delete failed: {data}");

	let data = learning_data(
		handle_learning(&mut session, &[])
			.await
			.expect("list after delete"),
	);
	// Listing order is retrieval-ranked, not insertion order — index 1 was
	// one of the two, so exactly one must survive.
	let listed = data["lessons"].to_string();
	let survivors = ["first learning-cmd lesson", "second learning-cmd lesson"]
		.iter()
		.filter(|s| listed.contains(*s))
		.count();
	assert_eq!(survivors, 1, "exactly one lesson must remain: {listed}");
	let backend = FileBackend;
	backend
		.store(&Lesson {
			content: "global rule must survive scoped clear".to_string(),
			scope: "global".to_string(),
			created: chrono::Utc::now().to_rfc3339(),
			..Default::default()
		})
		.await
		.expect("store global rule");

	let cleared = learning_data(
		handle_learning(&mut session, &["clear"])
			.await
			.expect("clear dispatches"),
	);
	assert_eq!(cleared["subcommand"], "clear");
	assert_eq!(cleared["deleted"], 1);
	let globals = backend.retrieve_global().await.expect("retrieve globals");
	assert!(globals
		.iter()
		.any(|memory| memory.content == "global rule must survive scoped clear"));

	cleanup();
}

#[serial]
#[tokio::test]
async fn test_learning_error_arms() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let _data = TestDataDir::new();
	let mut session = test_session();

	// delete without an index → usage error
	let data = learning_data(
		handle_learning(&mut session, &["delete"])
			.await
			.expect("dispatches"),
	);
	assert_eq!(data["subcommand"], "error");

	// non-numeric and zero indices → validation error
	for bad in ["abc", "0"] {
		let data = learning_data(
			handle_learning(&mut session, &["delete", bad])
				.await
				.expect("dispatches"),
		);
		assert_eq!(data["subcommand"], "error", "index {bad}: {data}");
	}

	// far out-of-range index → out-of-range error
	let data = learning_data(
		handle_learning(&mut session, &["delete", "999999"])
			.await
			.expect("dispatches"),
	);
	assert_eq!(data["subcommand"], "error");
	let data = learning_data(
		handle_learning(&mut session, &["show", "999999"])
			.await
			.expect("dispatches"),
	);
	assert_eq!(data["subcommand"], "error");

	// unknown subcommand → usage error
	let data = learning_data(
		handle_learning(&mut session, &["frobnicate"])
			.await
			.expect("dispatches"),
	);
	assert_eq!(data["subcommand"], "error");
}

#[serial]
#[tokio::test]
async fn test_learning_storage_summary_counts_cold_memory() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let _data = TestDataDir::new();
	let backend = FileBackend;
	let memory = Lesson {
		content: "cold retention summary memory".to_string(),
		memory_type: "orientation".to_string(),
		role: ROLE.to_string(),
		project: project(),
		created: chrono::Utc::now().to_rfc3339(),
		..Default::default()
	};
	backend.store(&memory).await.unwrap();
	crate::supervisor::learning::retention::archive_record(&memory).unwrap();

	let hot = all_lessons(ROLE, &project()).await.unwrap();
	let summary = learning_storage_summary(ROLE, &project(), &hot).unwrap();
	assert_eq!(summary["hot_items"], 0);
	assert_eq!(summary["cold_items"], 1);
	assert_eq!(summary["by_type"]["orientation"]["cold"], 1);
	assert!(summary["cold_tokens"].as_u64().unwrap_or_default() > 0);
}

#[serial]
#[tokio::test]
async fn test_learning_evolution_command_lifecycle() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let _data = TestDataDir::new();
	let now = chrono::Utc::now().to_rfc3339();
	let id = "evo-command-test";
	crate::supervisor::learning::evolution::create_record(
		crate::supervisor::learning::evolution::EvolutionRecord {
			schema_version: crate::supervisor::learning::evolution::REGISTRY_SCHEMA_VERSION,
			id: id.to_string(),
			name: "evolved-command-test".to_string(),
			description: "command lifecycle test".to_string(),
			kind: crate::supervisor::learning::evolution::ArtifactKind::Guard,
			scope: crate::supervisor::learning::evolution::ArtifactScope {
				project: Some(project()),
				domain: Some(ROLE.to_string()),
			},
			state: crate::supervisor::learning::evolution::EvolutionState::Shadow,
			effect: crate::supervisor::learning::evolution::EffectClass::Effectful,
			explicit_authorization: true,
			source_memory_ids: vec!["memory".to_string()],
			evidence: vec!["session://s/message/1".to_string()],
			artifact_version: 1,
			parent_version: None,
			superseded_ids: Vec::new(),
			generator_model: "openai:generator".to_string(),
			verifier_model: "google:verifier".to_string(),
			artifact_path: "guardrail.toml".to_string(),
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
		},
		"[[guard]]\nmatch = \"shell\"\nmessage = \"blocked\"\n",
		None,
	)
	.unwrap();
	let mut session = test_session();
	let listed = learning_data(handle_learning(&mut session, &["evolution"]).await.unwrap());
	assert_eq!(listed["subcommand"], "evolution_list");
	assert_eq!(listed["total"], 1);
	assert_eq!(listed["records"][0]["id"], id);

	let shown = learning_data(
		handle_learning(&mut session, &["evolution", "show", id])
			.await
			.unwrap(),
	);
	assert_eq!(shown["subcommand"], "evolution_show");
	assert!(shown["native_artifact"]
		.as_str()
		.unwrap_or_default()
		.contains("[[guard]]"));

	let approved = learning_data(
		handle_learning(&mut session, &["evolution", "approve", id])
			.await
			.unwrap(),
	);
	assert_eq!(approved["record"]["state"], "trial");
	let rolled_back = learning_data(
		handle_learning(&mut session, &["evolution", "rollback", id])
			.await
			.unwrap(),
	);
	assert_eq!(rolled_back["record"]["state"], "shadow");
	let rejected = learning_data(
		handle_learning(&mut session, &["evolution", "reject", id])
			.await
			.unwrap(),
	);
	assert_eq!(rejected["record"]["state"], "rejected");
}
