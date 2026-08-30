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

fn memory(content: &str, memory_type: &str) -> Lesson {
	Lesson {
		content: content.to_string(),
		title: content.to_string(),
		memory_type: memory_type.to_string(),
		role: "developer".to_string(),
		project: "project".to_string(),
		created: "2026-01-01T00:00:00Z".to_string(),
		..Default::default()
	}
}

#[test]
fn pair_selection_is_only_a_review_signal_and_respects_outcome() {
	let mut first = memory(
		"Provider continuation recovery keeps the resolved provider identity stable",
		"experience",
	);
	first.tags = vec!["provider".into(), "continuation".into()];
	first.outcome = TrajectoryOutcome::Verified;
	let mut related = memory(
		"Recover provider continuation by preserving the resolved provider and model identity",
		"experience",
	);
	related.tags = first.tags.clone();
	related.outcome = TrajectoryOutcome::Verified;
	let mut conflicting_outcome = related.clone();
	conflicting_outcome.outcome = TrajectoryOutcome::Failed;
	assert!(pair_signal(&first, &related) > MIN_PAIR_SIGNAL);
	assert_eq!(pair_signal(&first, &conflicting_outcome), 0.0);
	assert_eq!(best_pair(&[first, related]), Some((0, 1)));
}

#[test]
fn consolidated_record_never_inflates_trust_and_keeps_provenance() {
	let mut first = memory("first durable source", "orientation");
	first.importance = 0.8;
	first.confidence = "high".into();
	first.evidence = vec!["session://one/message/1".into()];
	first.use_count = 3;
	let mut second = memory("second durable source", "orientation");
	second.importance = 0.55;
	second.confidence = "medium".into();
	second.evidence = vec!["session://two/message/2".into()];
	second.use_count = 4;
	let merged = build_consolidated(&[first.clone(), second.clone()], "merged", "body");
	assert_eq!(merged.importance, 0.55);
	assert_eq!(merged.confidence, "medium");
	assert_eq!(merged.use_count, 7);
	assert!(merged.related.contains(&first.file_id()));
	assert!(merged.related.contains(&second.file_id()));
	assert_eq!(merged.evidence.len(), 2);
}

#[test]
fn retention_utility_rewards_proven_use_without_overriding_truth_credit() {
	let mut unused = memory("unused", "learning");
	unused.importance = 0.6;
	let mut used = unused.clone();
	used.content = "used".into();
	used.use_count = 10;
	used.last_used = chrono::Utc::now().to_rfc3339();
	assert!(retention_utility(&used) > retention_utility(&unused));
}

#[tokio::test]
async fn cold_archive_is_lossless_and_leaves_the_hot_scan() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let _data = TestDataDir::new();
	let item = memory("archived durable memory", "orientation");
	let backend = super::super::backend::file::FileBackend;
	backend.store(&item).await.unwrap();
	let hot_dir = crate::directories::get_learning_dir(&item.role, &item.project).unwrap();
	let hot = hot_dir.join(format!("{}.md", item.file_id()));

	let (moved_from, cold) = archive_record(&item).unwrap();
	assert_eq!(moved_from, hot);
	assert!(!hot.exists());
	assert!(cold.exists());
	let recalled = super::super::backend::file::FileBackend::retrieve_archived(
		&hot_dir,
		&["archived".to_string()],
		"",
		2,
	);
	assert_eq!(recalled.len(), 1);
	assert_eq!(recalled[0].content, item.content);
	assert_eq!(recalled[0].storage_path, cold.display().to_string());

	backend
		.reinforce(&item.content, &item.role, &item.project, 0.0)
		.await
		.unwrap();
	assert!(hot.exists());
	assert!(!cold.exists());
	let promoted = backend
		.retrieve_all(&item.role, &item.project)
		.await
		.unwrap();
	assert_eq!(promoted.len(), 1);
	assert_eq!(promoted[0].use_count, 1);
}

#[tokio::test]
async fn short_rules_obey_hard_budget_without_synthetic_merge_or_deletion() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let _data = TestDataDir::new();
	let config = crate::session::chat::test_support::fake_provider_config();
	let backend = super::super::backend::file::FileBackend;
	for index in 0..24 {
		let mut item = memory(
			&format!(
				"rule {index}: {}",
				"preserve this grounded constraint ".repeat(180)
			),
			"learning",
		);
		item.created = format!("2026-01-01T00:00:{index:02}Z");
		backend.store(&item).await.unwrap();
	}
	let before = backend.retrieve_all("developer", "project").await.unwrap();
	assert!(storage_tokens(&before) > SCOPED_LEARNING_HARD_TOKENS);

	let report = maintain(&config, "developer", "project").await.unwrap();
	assert_eq!(report.consolidated, 0);
	assert!(report.archived > 0);
	let hot = backend.retrieve_all("developer", "project").await.unwrap();
	assert!(
		storage_tokens(&hot) <= SCOPED_LEARNING_HARD_TOKENS * SOFT_NUMERATOR / SOFT_DENOMINATOR
	);
	let archive = crate::directories::get_learning_dir("developer", "project")
		.unwrap()
		.join(".archive")
		.join("learning");
	let cold_files = std::fs::read_dir(archive)
		.unwrap()
		.flatten()
		.filter(|entry| entry.path().extension().is_some_and(|ext| ext == "md"))
		.count();
	assert_eq!(hot.len() + cold_files, before.len());
}
