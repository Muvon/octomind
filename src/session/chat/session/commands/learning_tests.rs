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
//! throwaway role scope. `clear` is deliberately NOT exercised: its lesson
//! walk includes the global tier, so on a developer machine it would wipe
//! real user-wide lessons.

use super::*;
use crate::supervisor::learning::backend::create_backend;
use crate::supervisor::learning::Lesson;

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

fn test_config() -> Config {
	let mut config: Config =
		toml::from_str(include_str!("../../../../../config-templates/default.toml"))
			.expect("parse default config template");
	config.build_role_map();
	config
}

fn test_session() -> ChatSession {
	let mut session = ChatSession::for_tests(Vec::new());
	session.role = ROLE.to_string();
	session
}

async fn store(content: &str, config: &Config) {
	let backend = create_backend(&config.supervisor.learning);
	backend
		.store(
			&Lesson {
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
			},
			config,
		)
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

#[tokio::test]
async fn test_learning_list_and_delete_lifecycle() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let _data = TestDataDir::new();
	cleanup();
	let config = test_config();
	let mut session = test_session();
	store("first learning-cmd lesson about widgets", &config).await;
	store("second learning-cmd lesson about gadgets", &config).await;

	// Bare /learning lists both (global-tier entries from the machine may
	// follow ours, so assert containment, not exact counts).
	let data = learning_data(
		handle_learning(&mut session, &config, &[])
			.await
			.expect("list dispatches"),
	);
	assert_eq!(data["subcommand"], "list");
	let listed = data["lessons"].to_string();
	assert!(listed.contains("first learning-cmd lesson"), "{listed}");
	assert!(listed.contains("second learning-cmd lesson"), "{listed}");
	let shown = learning_data(
		handle_learning(&mut session, &config, &["show", "1"])
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
		handle_learning(&mut session, &config, &["list", "*widgets*"])
			.await
			.expect("filtered list dispatches"),
	);
	let listed = data["lessons"].to_string();
	assert!(listed.contains("widgets"), "{listed}");
	assert!(!listed.contains("gadgets"), "{listed}");

	// Scoped lessons sort before the global tier, so index 1 is ours
	let data = learning_data(
		handle_learning(&mut session, &config, &["delete", "1"])
			.await
			.expect("delete dispatches"),
	);
	assert_eq!(data["subcommand"], "delete", "delete failed: {data}");

	let data = learning_data(
		handle_learning(&mut session, &config, &[])
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

	cleanup();
}

#[tokio::test]
async fn test_learning_error_arms() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let _data = TestDataDir::new();
	let config = test_config();
	let mut session = test_session();

	// delete without an index → usage error
	let data = learning_data(
		handle_learning(&mut session, &config, &["delete"])
			.await
			.expect("dispatches"),
	);
	assert_eq!(data["subcommand"], "error");

	// non-numeric and zero indices → validation error
	for bad in ["abc", "0"] {
		let data = learning_data(
			handle_learning(&mut session, &config, &["delete", bad])
				.await
				.expect("dispatches"),
		);
		assert_eq!(data["subcommand"], "error", "index {bad}: {data}");
	}

	// far out-of-range index → out-of-range error
	let data = learning_data(
		handle_learning(&mut session, &config, &["delete", "999999"])
			.await
			.expect("dispatches"),
	);
	assert_eq!(data["subcommand"], "error");
	let data = learning_data(
		handle_learning(&mut session, &config, &["show", "999999"])
			.await
			.expect("dispatches"),
	);
	assert_eq!(data["subcommand"], "error");

	// unknown subcommand → usage error
	let data = learning_data(
		handle_learning(&mut session, &config, &["frobnicate"])
			.await
			.expect("dispatches"),
	);
	assert_eq!(data["subcommand"], "error");
}
