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

#[test]
fn catalog_entry_paths_and_search_text_are_compact() {
	let entry = ArchiveCatalogEntry {
		memory_type: "learning".to_string(),
		file: "note.md".to_string(),
		title: "Postgres Rule".to_string(),
		preview: "Use connection pooling".to_string(),
		tags: vec!["DB".to_string()],
		importance: 0.5,
		created: "2026-01-01T00:00:00Z".to_string(),
	};
	let hot = std::path::Path::new("/hot");
	assert_eq!(
		entry.path(hot),
		hot.join(".archive").join("learning").join("note.md")
	);
	assert_eq!(
		entry.search_text(),
		"postgres rule use connection pooling db"
	);
}

#[test]
fn hard_budget_covers_every_kind_and_falls_back_to_learning() {
	assert_eq!(hard_budget("learning", true), GLOBAL_LEARNING_HARD_TOKENS);
	assert_eq!(
		hard_budget("orientation", true),
		GLOBAL_ORIENTATION_HARD_TOKENS
	);
	assert_eq!(
		hard_budget("experience", true),
		GLOBAL_EXPERIENCE_HARD_TOKENS
	);
	assert_eq!(hard_budget("learning", false), SCOPED_LEARNING_HARD_TOKENS);
	assert_eq!(
		hard_budget("orientation", false),
		SCOPED_ORIENTATION_HARD_TOKENS
	);
	assert_eq!(
		hard_budget("experience", false),
		SCOPED_EXPERIENCE_HARD_TOKENS
	);
	assert_eq!(hard_budget("other", true), GLOBAL_LEARNING_HARD_TOKENS);
	assert_eq!(hard_budget("other", false), SCOPED_LEARNING_HARD_TOKENS);
}

#[test]
fn pair_signal_requires_same_type_and_scope_and_weighs_tags() {
	let mut first = memory("shared durable procedure words", "orientation");
	let mut second = memory("shared durable procedure words", "orientation");
	first.tags = vec!["rust".into()];
	second.tags = vec!["rust".into()];
	let aligned = pair_signal(&first, &second);

	second.tags = vec!["python".into()];
	assert!(pair_signal(&first, &second) < aligned);

	second.memory_type = "learning".to_string();
	assert_eq!(pair_signal(&first, &second), 0.0);
	second.memory_type = "orientation".to_string();
	second.scope = "global".to_string();
	assert_eq!(pair_signal(&first, &second), 0.0);
}

#[test]
fn best_pair_returns_none_without_overlap_above_the_signal_floor() {
	let first = memory("completely unrelated alpha words", "orientation");
	let second = memory("different topic entirely beta gamma", "orientation");
	assert_eq!(best_pair(&[first, second]), None);
	assert_eq!(best_pair(&[]), None);
}

#[test]
fn jaccard_is_zero_for_empty_sets_and_short_tokens_are_dropped() {
	let empty: HashSet<String> = HashSet::new();
	assert_eq!(jaccard(&empty, &empty), 0.0);
	assert_eq!(jaccard(&normalized_words("key value"), &empty), 0.0);
	let words = normalized_words("an the key value pair");
	assert!(words.contains("key"));
	assert!(words.contains("the"), "three-character words are retained");
	assert!(
		!words.contains("an"),
		"words shorter than three are dropped"
	);
}

#[test]
fn valid_experience_shape_requires_word_budget_and_all_headings() {
	let headings =
		"## Objective\n## Durable knowledge\n## Outcome and evidence\n## Reuse conditions";
	let words = (0..200)
		.map(|index| format!("word{index}"))
		.collect::<Vec<_>>()
		.join(" ");
	assert!(valid_experience_shape(&format!("{headings}\n{words}")));
	assert!(!valid_experience_shape(&format!("{headings}\nshort body")));
	assert!(!valid_experience_shape(&format!("## Objective\n{words}")));
}

#[test]
fn source_view_exposes_provenance_fields() {
	let mut item = memory("viewed record", "learning");
	item.evidence = vec!["session://s/message/3".into()];
	item.related = vec!["other-id".into()];
	let view = source_view(&item);
	assert_eq!(view["id"].as_str().unwrap(), item.file_id());
	assert_eq!(view["memory_type"].as_str().unwrap(), "learning");
	assert_eq!(view["outcome"].as_str().unwrap(), "unknown");
	assert_eq!(view["evidence"].as_array().unwrap().len(), 1);
	assert_eq!(view["related"].as_array().unwrap().len(), 1);
}

#[test]
fn retention_utility_weighs_confidence_and_falls_back_to_created() {
	let mut high = memory("confidence check", "learning");
	high.confidence = "high".into();
	high.created = chrono::Utc::now().to_rfc3339();
	let mut medium = high.clone();
	medium.confidence = "medium".into();
	assert!(retention_utility(&high) > retention_utility(&medium));

	let mut undated = high.clone();
	undated.created = "not a date".to_string();
	undated.last_used = String::new();
	assert!(retention_utility(&undated) < retention_utility(&high));
}

#[tokio::test]
async fn archive_record_requires_the_hot_file_and_suffixes_collisions() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let _data = TestDataDir::new();
	let backend = super::super::backend::file::FileBackend;
	let item = memory("collision prone memory", "orientation");

	let missing = archive_record(&item);
	assert!(missing.is_err());

	backend.store(&item).await.unwrap();
	let (_, first_cold) = archive_record(&item).unwrap();
	backend.store(&item).await.unwrap();
	let (_, second_cold) = archive_record(&item).unwrap();
	assert_ne!(first_cold, second_cold);
	assert!(second_cold
		.to_string_lossy()
		.contains(&format!("{}-1.md", item.file_id())));
	let hot_dir = crate::directories::get_learning_dir(&item.role, &item.project).unwrap();
	let catalog = std::fs::read_to_string(hot_dir.join(".archive").join("catalog.jsonl")).unwrap();
	assert_eq!(catalog.lines().count(), 2);
}

#[tokio::test]
async fn replace_with_consolidation_rolls_back_when_a_source_is_missing() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let _data = TestDataDir::new();
	let backend = super::super::backend::file::FileBackend;
	let first = memory("first consolidation source", "orientation");
	let second = memory("second consolidation source", "orientation");
	let merged = build_consolidated(&[first.clone(), second.clone()], "merged", "merged body");
	backend.store(&first).await.unwrap();
	backend.store(&second).await.unwrap();
	backend.store(&merged).await.unwrap();

	// The second source's hot file disappearing mid-flight must restore the
	// first source and drop the replacement instead of losing data.
	let hot_dir = crate::directories::get_learning_dir(&first.role, &first.project).unwrap();
	std::fs::remove_file(hot_dir.join(format!("{}.md", second.file_id()))).unwrap();

	let replaced = replace_with_consolidation(&backend, &[first.clone(), second.clone()], &merged)
		.await
		.unwrap();
	assert!(!replaced);
	assert!(hot_dir.join(format!("{}.md", first.file_id())).exists());
	assert!(!hot_dir.join(format!("{}.md", merged.file_id())).exists());
	let cold_dir = hot_dir.join(".archive").join("orientation");
	assert!(!cold_dir.join(format!("{}.md", first.file_id())).exists());
}

async fn store_global_orientation(
	index: usize,
	backend: &super::super::backend::file::FileBackend,
) {
	// Grow the record until it holds ~3.3k tokens so three records cross the
	// 8k global hard watermark while any pair stays under the 8k
	// consolidation input cap, regardless of which tokenizer is active.
	let mut body = String::new();
	let mut item = memory(&format!("orientation {index}"), "orientation");
	while memory_tokens(&item) < 3_300 {
		body.push_str("shared durable procedural detail ");
		item.content = format!("orientation {index}: {body}");
	}
	item.scope = "global".to_string();
	item.created = format!("2026-01-0{}T00:00:00Z", index + 1);
	backend.store(&item).await.unwrap();
}

#[serial_test::serial]
#[tokio::test]
async fn orientation_overflow_consolidates_through_a_verified_model_merge() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let _data = TestDataDir::new();
	let previous_url = std::env::var_os("OLLAMA_API_URL");
	let backend = super::super::backend::file::FileBackend;
	for index in 0..3 {
		store_global_orientation(index, &backend).await;
	}
	let before = backend.retrieve_global().await.unwrap();
	assert!(storage_tokens(&before) > GLOBAL_ORIENTATION_HARD_TOKENS);

	let url = crate::session::chat::test_support::spawn_stub(vec![
		crate::session::chat::test_support::final_response(
			r#"{"merge":true,"title":"merged orientation","content":"one merged orientation record"}"#,
		),
		crate::session::chat::test_support::final_response(r#"{"supported":true,"issues":[]}"#),
	])
	.await;
	std::env::set_var("OLLAMA_API_URL", url);
	let mut config = crate::session::chat::test_support::fake_provider_config();
	config.supervisor.model.model = Some("ollama:fake-model".to_string());
	config.supervisor.model.model = Some("ollama:fake-model".to_string());

	let report = maintain(&config, "developer", "project").await.unwrap();
	assert_eq!(report.consolidated, 1);
	assert_eq!(report.archived, 2);
	let hot = backend.retrieve_global().await.unwrap();
	assert_eq!(hot.len(), 2);
	let merged = hot
		.iter()
		.find(|item| item.source.starts_with("retention:"))
		.expect("merged record stored");
	assert_eq!(merged.title, "merged orientation");
	let cold_dir = crate::directories::get_global_learning_dir()
		.unwrap()
		.join(".archive")
		.join("orientation");
	let cold = std::fs::read_dir(cold_dir)
		.unwrap()
		.flatten()
		.filter(|entry| entry.path().extension().is_some_and(|ext| ext == "md"))
		.count();
	assert_eq!(cold, 2);

	if let Some(value) = previous_url {
		std::env::set_var("OLLAMA_API_URL", value);
	} else {
		std::env::remove_var("OLLAMA_API_URL");
	}
}

#[serial_test::serial]
#[tokio::test]
async fn rejected_merge_still_enforces_the_soft_watermark() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let _data = TestDataDir::new();
	let previous_url = std::env::var_os("OLLAMA_API_URL");
	let backend = super::super::backend::file::FileBackend;
	for index in 0..3 {
		store_global_orientation(index, &backend).await;
	}

	let url = crate::session::chat::test_support::spawn_stub(vec![
		crate::session::chat::test_support::final_response(
			r#"{"merge":false,"title":"unused","content":"unused"}"#,
		),
	])
	.await;
	std::env::set_var("OLLAMA_API_URL", url);
	let mut config = crate::session::chat::test_support::fake_provider_config();
	config.supervisor.model.model = Some("ollama:fake-model".to_string());
	config.supervisor.model.model = Some("ollama:fake-model".to_string());

	let report = maintain(&config, "developer", "project").await.unwrap();
	assert_eq!(report.consolidated, 0);
	assert!(report.archived > 0);
	let hot = backend.retrieve_global().await.unwrap();
	assert!(
		storage_tokens(&hot) <= GLOBAL_ORIENTATION_HARD_TOKENS * SOFT_NUMERATOR / SOFT_DENOMINATOR
	);
	assert!(hot
		.iter()
		.all(|item| !item.source.starts_with("retention:")));

	if let Some(value) = previous_url {
		std::env::set_var("OLLAMA_API_URL", value);
	} else {
		std::env::remove_var("OLLAMA_API_URL");
	}
}
