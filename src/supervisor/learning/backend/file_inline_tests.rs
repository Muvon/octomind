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

#[test]
fn test_escaping_roundtrip() {
	// `unescape` is the exact inverse of `escape` on adversarial inputs —
	// backslashes, quotes, the trailing-quote case, newlines, and already-
	// escaped-looking sequences. This is the store/parse cycle that
	// `reinforce` runs on every importance bump, so any asymmetry compounds.
	for s in [
		"plain",
		"has \"quotes\"",
		"back\\slash",
		"trailing quote\\\"",
		"embedded\nnewline",
		"C:\\tmp and a \" quote",
		"\\\\already\\\\escaped",
	] {
		assert_eq!(unescape(&escape(s)), s, "round-trip failed for {s:?}");
	}

	// A full record whose quoted fields contain a quote and a newline
	// survives the store-format -> parse cycle without corruption or the
	// newline truncating the line-based parser.
	let title = "say \"hi\"\nthen bye";
	let content = "path C:\\tmp and a \" quote";
	let project = "weird\\proj\"name";
	let record = format!(
			"---\ntitle: \"{}\"\ncontent: \"{}\"\nmemory_type: learning\nimportance: 0.5\nconfidence: high\ntags: []\nsource: \"s\"\nrole: \"r\"\nproject: \"{}\"\nscope: scoped\ncreated: \"c\"\n---\n",
			escape(title),
			escape(content),
			escape(project),
		);
	let lesson = FileBackend::parse_lesson_file(&record).unwrap();
	assert_eq!(lesson.title, title);
	assert_eq!(lesson.content, content);
	assert_eq!(lesson.project, project);
}

#[test]
fn test_parse_lesson_file_valid() {
	let content = r#"---
content: "Bearer token auth required"
memory_type: learning
importance: 0.8
confidence: high
tags: [auth, api]
source: "test-session"
role: "developer"
project: "octofs"
created: "2026-04-05T14:30:00Z"
related: ["memory-a","memory-b"]
evidence: ["session://test/message/2"]
outcome: failed
last_used: "2026-08-28T00:00:00Z"
use_count: 7
---
"#;
	let lesson = FileBackend::parse_lesson_file(content).unwrap();
	assert_eq!(lesson.content, "Bearer token auth required");
	assert_eq!(lesson.importance, 0.8);
	assert_eq!(lesson.confidence, "high");
	assert_eq!(lesson.tags, vec!["auth", "api"]);
	assert_eq!(lesson.role, "developer");
	assert_eq!(lesson.project, "octofs");
	assert_eq!(lesson.related, vec!["memory-a", "memory-b"]);
	assert_eq!(lesson.evidence, vec!["session://test/message/2"]);
	assert_eq!(lesson.last_used, "2026-08-28T00:00:00Z");
	assert_eq!(lesson.use_count, 7);
	assert_eq!(
		lesson.outcome,
		crate::supervisor::learning::TrajectoryOutcome::Failed
	);
}

#[test]
fn test_parse_lesson_file_missing_frontmatter() {
	let content = "Just some text without frontmatter";
	assert!(FileBackend::parse_lesson_file(content).is_none());
}

#[test]
fn test_parse_lesson_file_empty_content() {
	let content = r#"---
memory_type: learning
importance: 0.5
---
"#;
	// content field is empty -> should return None
	assert!(FileBackend::parse_lesson_file(content).is_none());
}

#[test]
fn test_file_id() {
	let lesson = Lesson {
		content: "Bearer token auth".into(),
		created: "2026-04-05T14:30:00Z".into(),
		..Default::default()
	};
	assert_eq!(lesson.file_id(), "20260405143000-bearer-token-auth");

	let empty = Lesson {
		content: "!!!".into(),
		created: "2026-04-05T14:30:00Z".into(),
		..Default::default()
	};
	// No alphanumerics → slug empty → id is just the timestamp.
	assert_eq!(empty.file_id(), "20260405143000");
}

#[tokio::test]
async fn test_store_and_retrieve_all() {
	let dir = tempfile::tempdir().unwrap();
	let role = "developer";
	let project = "test_proj";

	// Create learning dir manually for test
	let learning_dir = dir.path().join("learning").join(project).join(role);
	std::fs::create_dir_all(&learning_dir).unwrap();

	// Write a lesson file directly
	let content = r#"---
content: "Always use bearer tokens"
memory_type: learning
importance: 0.8
confidence: high
tags: [auth]
source: "test"
role: "developer"
project: "test_proj"
created: "2026-04-05T00:00:00Z"
---
"#;
	std::fs::write(learning_dir.join("20260405-bearer-tokens.md"), content).unwrap();

	// Read it back
	let mut lessons = Vec::new();
	for entry in std::fs::read_dir(&learning_dir).unwrap() {
		let entry = entry.unwrap();
		if let Ok(file_content) = std::fs::read_to_string(entry.path()) {
			if let Some(lesson) = FileBackend::parse_lesson_file(&file_content) {
				lessons.push(lesson);
			}
		}
	}

	assert_eq!(lessons.len(), 1);
	assert_eq!(lessons[0].content, "Always use bearer tokens");
	assert_eq!(lessons[0].confidence, "high");
}

#[test]
fn test_pattern_matching() {
	let lesson = Lesson {
		content: "Bearer token auth is required for API endpoints".into(),
		tags: vec!["auth".into(), "api".into()],
		..Default::default()
	};

	let text = lesson.content.to_lowercase();
	let tags_text = lesson.tags.join(" ").to_lowercase();
	let combined = format!("{} {}", text, tags_text);

	assert!(combined.contains("bearer"));
	assert!(combined.contains("auth"));
	assert!(combined.contains("api"));
	assert!(!combined.contains("database"));
}

// ----------------------------------------------------------------------
// Pure-logic tests for RRF + keyword ranking. These cover the fusion
// math without touching the embedding model.
// ----------------------------------------------------------------------

fn lesson_with(content: &str, tags: &[&str]) -> Lesson {
	Lesson {
		content: content.to_string(),
		tags: tags.iter().map(|s| s.to_string()).collect(),
		..Default::default()
	}
}

#[test]
fn rank_by_keywords_returns_empty_when_no_patterns() {
	let lessons = vec![lesson_with("anything", &[])];
	assert!(rank_by_keywords(&lessons, &[]).is_empty());
}

#[test]
fn rank_by_keywords_excludes_lessons_with_zero_hits() {
	let lessons = vec![
		lesson_with("postgres slow query", &["db"]),
		lesson_with("filesystem read", &["files"]),
	];
	let ranking = rank_by_keywords(&lessons, &["postgres".to_string()]);
	// Only the postgres lesson hits; filesystem lesson is excluded.
	assert_eq!(ranking, vec![0]);
}

#[test]
fn rank_by_keywords_orders_by_hit_count_descending() {
	let lessons = vec![
		lesson_with("postgres", &[]),                // 1 hit
		lesson_with("postgres slow query", &["db"]), // 2 hits (postgres, query)
		lesson_with("just a note", &[]),             // 0 hits — excluded
	];
	let ranking = rank_by_keywords(&lessons, &["postgres".to_string(), "query".to_string()]);
	// Lesson 1 (2 hits) ranks before lesson 0 (1 hit); lesson 2 excluded.
	assert_eq!(ranking, vec![1, 0]);
}

#[test]
fn rank_by_keywords_is_case_insensitive() {
	let lessons = vec![lesson_with("PostgreSQL EXPLAIN ANALYZE", &[])];
	let ranking = rank_by_keywords(&lessons, &["postgresql".to_string()]);
	assert_eq!(ranking, vec![0]);
}

#[test]
fn rank_by_keywords_uses_phrase_terms_as_weak_fallback() {
	let lessons = vec![
		lesson_with(
			"validate callback state and retain the PKCE verifier",
			&["oauth"],
		),
		lesson_with("unrelated callback rendering", &["ui"]),
	];
	let ranking = rank_by_keywords(
		&lessons,
		&["state parameter".to_string(), "oauth callback".to_string()],
	);
	assert_eq!(ranking.first().copied(), Some(0));
}

#[test]
fn semantic_chunks_leave_short_memories_whole_and_split_long_sections() {
	let short = lesson_with("retain the exact provider identity", &["provider"]);
	let short_chunks = semantic_retrieval_chunks(&short, 128);
	assert_eq!(
		short_chunks,
		vec![" retain the exact provider identity provider".to_string()]
	);

	let long = lesson_with(
		&[
			"first focused section ".repeat(40),
			"second independent section ".repeat(40),
		]
		.join("\n"),
		&["memory"],
	);
	let long_chunks = semantic_retrieval_chunks(&long, 32);
	assert!(long_chunks.len() > 2);
	assert!(long_chunks.iter().all(|chunk| chunk.contains("memory")));
}

#[test]
fn importance_rerank_is_bounded_and_neutral_at_half() {
	assert_eq!(importance_factor(-1.0), 0.75);
	assert_eq!(importance_factor(0.5), 1.0);
	assert_eq!(importance_factor(2.0), 1.25);
}

#[test]
fn ranked_retrieval_expands_forward_links_and_backlinks_once() {
	let mut root = lesson_with("root memory", &[]);
	root.created = "2026-01-01T00:00:01Z".to_string();
	let mut target = lesson_with("target memory", &[]);
	target.created = "2026-01-01T00:00:02Z".to_string();
	let mut backlink = lesson_with("backlink memory", &[]);
	backlink.created = "2026-01-01T00:00:03Z".to_string();
	root.related.push(target.file_id());
	backlink.related.push(root.file_id());
	let all = vec![root, target, backlink];

	let expanded = expand_ranked_with_links(&all, &[(1.0, 0)], 3);
	assert_eq!(
		expanded
			.iter()
			.map(|memory| memory.content.as_str())
			.collect::<Vec<_>>(),
		vec!["root memory", "target memory", "backlink memory"]
	);
}

#[test]
fn rrf_returns_empty_for_empty_inputs() {
	let empty: Vec<&[usize]> = Vec::new();
	assert!(reciprocal_rank_fusion(0, &empty).is_empty());
	assert!(reciprocal_rank_fusion(5, &empty).is_empty());
}

#[test]
fn rrf_single_ranker_preserves_order() {
	// With one ranker, RRF is just rank order with smaller scores
	// further down — fused order should equal input order.
	let r = vec![2usize, 0, 1];
	let fused = reciprocal_rank_fusion(3, &[&r]);
	let order: Vec<usize> = fused.iter().map(|(_, i)| *i).collect();
	assert_eq!(order, vec![2, 0, 1]);
}

#[test]
fn rrf_excludes_items_not_in_any_ranking() {
	// Only items 0 and 2 appear; item 1 should be absent from output.
	let r = vec![0usize, 2];
	let fused = reciprocal_rank_fusion(3, &[&r]);
	let indices: Vec<usize> = fused.iter().map(|(_, i)| *i).collect();
	assert_eq!(indices, vec![0, 2]);
	assert!(!indices.contains(&1));
}

#[test]
fn rrf_promotes_items_appearing_in_multiple_rankings() {
	// Item 0 ranks #2 in keyword and #1 in cosine (mid-rank in both).
	// Item 1 ranks #1 in keyword and not at all in cosine.
	// Item 2 ranks #3 in keyword and #2 in cosine.
	// Item 0 should win because it appears in BOTH rankings —
	// even though item 1 is keyword-#1, missing from cosine drops it.
	let keyword = vec![1usize, 0, 2];
	let cosine = vec![0usize, 2];
	let fused = reciprocal_rank_fusion(3, &[&keyword, &cosine]);
	let top = fused.first().expect("at least one fused result");
	assert_eq!(top.1, 0, "item 0 should win — present in both rankings");
}

#[test]
fn rrf_top_rank_in_both_dominates() {
	// If an item is rank #1 in both, it must score highest.
	let r1 = vec![5usize, 0, 1];
	let r2 = vec![5usize, 1, 0];
	let fused = reciprocal_rank_fusion(6, &[&r1, &r2]);
	assert_eq!(fused.first().unwrap().1, 5);
}

#[test]
fn weighted_rrf_keeps_dense_correction_ahead_of_sparse_stale_match() {
	let sparse = vec![1usize, 0]; // stale phrasing matches first
	let dense = vec![0usize, 2]; // current rule is semantically strongest
	let fused = weighted_reciprocal_rank_fusion(3, &[(&sparse, KEYWORD_RRF_WEIGHT), (&dense, 1.0)]);
	assert_eq!(fused.first().unwrap().1, 0);

	// When embeddings are absent, keyword weighting is uniform and cannot
	// disturb its original exact-match order.
	let sparse_only = weighted_reciprocal_rank_fusion(3, &[(&sparse, 1.0)]);
	assert_eq!(
		sparse_only.iter().map(|item| item.1).collect::<Vec<_>>(),
		sparse
	);
}

#[test]
fn adaptive_keyword_weight_changes_only_conflict_contaminated_queries() {
	let mut lessons = vec![lesson_with("current", &[]), lesson_with("stale", &[])];
	lessons[0].importance = 0.9;
	lessons[1].importance = 0.2;
	assert_eq!(
		adaptive_keyword_weight(&[1, 0], &[0], &lessons),
		KEYWORD_RRF_WEIGHT
	);
	lessons[1].importance = 0.5;
	assert_eq!(adaptive_keyword_weight(&[1, 0], &[0], &lessons), 1.0);
	assert_eq!(adaptive_keyword_weight(&[1, 0], &[], &lessons), 1.0);
}

#[test]
fn sparse_rescue_uses_rank_five_and_prefers_outcome_importance() {
	let mut lessons = vec![
		lesson_with("current callback state verifier", &["oauth"]),
		lesson_with("stale callback state", &["oauth"]),
	];
	lessons[0].importance = 0.9;
	lessons[1].importance = 0.2;
	lessons.extend((0..6).map(|index| lesson_with(&format!("dense {index}"), &[])));
	let sparse = vec![1usize, 0];
	let mut ranked = (2..8)
		.enumerate()
		.map(|(rank, index)| (1.0 - rank as f32 / 10.0, index))
		.chain([(0.01, 1), (0.009, 0)])
		.collect::<Vec<_>>();
	promote_sparse_rescue(&mut ranked, &sparse, &lessons);
	assert_eq!(ranked[4].1, 0);
	assert_ne!(ranked[0].1, 0);
}

#[test]
fn rrf_uses_one_indexed_ranks() {
	// With k=60, item at rank-0 (1-indexed: 1) scores 1/(60+1) = 1/61
	// across one ranker. Item at rank-1 scores 1/62. Verify the math.
	let r = vec![3usize, 7];
	let fused = reciprocal_rank_fusion(8, &[&r]);
	let by_idx: std::collections::HashMap<usize, f32> =
		fused.into_iter().map(|(s, i)| (i, s)).collect();
	let s_first = by_idx[&3];
	let s_second = by_idx[&7];
	let expected_first = 1.0_f32 / (RRF_K + 1.0);
	let expected_second = 1.0_f32 / (RRF_K + 2.0);
	assert!(
		(s_first - expected_first).abs() < 1e-6,
		"rank-1 score should be 1/61 ({}), got {}",
		expected_first,
		s_first
	);
	assert!(
		(s_second - expected_second).abs() < 1e-6,
		"rank-2 score should be 1/62 ({}), got {}",
		expected_second,
		s_second
	);
	assert!(s_first > s_second, "rank-1 must outscore rank-2");
}

#[test]
fn parse_string_list_prefers_json_and_falls_back_to_brackets() {
	assert_eq!(
		parse_string_list(r#"["a","b"]"#),
		vec!["a".to_string(), "b".to_string()]
	);
	assert_eq!(
		parse_string_list(r#"[a, "b"]"#),
		vec!["a".to_string(), "b".to_string()]
	);
	assert!(parse_string_list("[]").is_empty());
	assert!(parse_string_list("").is_empty());
}

#[test]
fn parse_lesson_file_tolerates_bad_scalars_and_unknown_keys() {
	let content = r#"---
content: "keeps the record alive"
importance: high
confidence: medium
outcome: bogus
use_count: many
unknown_key: ignored
not a key value line
related: not json
---
"#;
	let lesson = FileBackend::parse_lesson_file(content).unwrap();
	assert_eq!(lesson.importance, 0.5);
	assert_eq!(
		lesson.outcome,
		crate::supervisor::learning::TrajectoryOutcome::Unknown
	);
	assert_eq!(lesson.use_count, 0);
	assert_eq!(lesson.related, vec!["not json".to_string()]);

	// No closing frontmatter delimiter -> nothing to parse.
	assert!(FileBackend::parse_lesson_file("---\ncontent: \"x\"\n").is_none());
}

#[test]
fn read_lessons_sorted_skips_invalid_files_and_orders_by_importance() {
	let dir = tempfile::tempdir().unwrap();
	let record = |importance: &str| {
		format!("---\ncontent: \"record {importance}\"\nimportance: {importance}\n---\n")
	};
	std::fs::write(dir.path().join("low.md"), record("0.3")).unwrap();
	std::fs::write(dir.path().join("high.md"), record("0.9")).unwrap();
	std::fs::write(dir.path().join("mid.md"), record("0.6")).unwrap();
	std::fs::write(dir.path().join("broken.md"), "not a lesson").unwrap();
	std::fs::write(dir.path().join("ignored.txt"), record("1.0")).unwrap();

	let lessons = FileBackend::read_lessons_sorted(dir.path());
	assert_eq!(
		lessons
			.iter()
			.map(|lesson| lesson.content.as_str())
			.collect::<Vec<_>>(),
		vec!["record 0.9", "record 0.6", "record 0.3"]
	);
	assert!(lessons[0].storage_path.ends_with("high.md"));

	// A missing directory is an empty scope, not an error.
	assert!(FileBackend::read_lessons_sorted(&dir.path().join("absent")).is_empty());
}

fn write_catalog(dir: &std::path::Path, rows: &[serde_json::Value]) {
	let catalog = dir.join(".archive").join("catalog.jsonl");
	std::fs::create_dir_all(catalog.parent().unwrap()).unwrap();
	let body = rows
		.iter()
		.map(|row| row.to_string())
		.collect::<Vec<_>>()
		.join("\n");
	std::fs::write(catalog, body).unwrap();
}

fn catalog_row(memory_type: &str, file: &str, title: &str, importance: f64) -> serde_json::Value {
	serde_json::json!({
		"memory_type": memory_type,
		"file": file,
		"title": title,
		"preview": title,
		"tags": [],
		"importance": importance,
		"created": "2026-01-01T00:00:00Z"
	})
}

fn write_archived_lesson(dir: &std::path::Path, memory_type: &str, file: &str, content: &str) {
	let path = dir.join(".archive").join(memory_type).join(file);
	std::fs::create_dir_all(path.parent().unwrap()).unwrap();
	std::fs::write(
		&path,
		format!("---\ncontent: \"{content}\"\nimportance: 0.5\n---\n"),
	)
	.unwrap();
}

#[test]
fn read_archived_walks_type_dirs_and_skips_non_markdown() {
	let dir = tempfile::tempdir().unwrap();
	write_archived_lesson(dir.path(), "learning", "a.md", "archived learning");
	write_archived_lesson(dir.path(), "orientation", "b.md", "archived orientation");
	write_archived_lesson(dir.path(), "learning", "note.txt", "ignored");
	std::fs::write(
		dir.path().join(".archive").join("learning").join("bad.md"),
		"garbage",
	)
	.unwrap();

	let lessons = FileBackend::read_archived(dir.path());
	assert_eq!(lessons.len(), 2);
	assert!(lessons
		.iter()
		.any(|lesson| lesson.content == "archived learning"));
	assert!(lessons
		.iter()
		.any(|lesson| lesson.content == "archived orientation"));

	// No archive at all -> empty.
	let empty = tempfile::tempdir().unwrap();
	assert!(FileBackend::read_archived(empty.path()).is_empty());
}

#[test]
fn retrieve_archived_guards_limit_terms_and_ranking() {
	let dir = tempfile::tempdir().unwrap();
	write_archived_lesson(dir.path(), "learning", "pg.md", "postgres migration notes");
	write_archived_lesson(dir.path(), "learning", "oauth.md", "oauth callback state");
	write_catalog(
		dir.path(),
		&[
			catalog_row("learning", "pg.md", "postgres migration notes", 0.5),
			catalog_row("learning", "oauth.md", "oauth callback state", 0.9),
			catalog_row("learning", "pg.md", "postgres migration notes", 0.5),
			catalog_row("learning", "missing.md", "postgres gone", 0.9),
			serde_json::json!({"broken": true}),
		],
	);

	assert!(FileBackend::retrieve_archived(dir.path(), &[], "", 2).is_empty());
	assert!(
		FileBackend::retrieve_archived(dir.path(), &["postgres".to_string()], "", 0).is_empty()
	);

	let no_catalog = tempfile::tempdir().unwrap();
	assert!(
		FileBackend::retrieve_archived(no_catalog.path(), &["postgres".to_string()], "", 2)
			.is_empty()
	);

	// One generic intent term never pages noise; two independent terms do.
	assert!(FileBackend::retrieve_archived(dir.path(), &[], "oauth", 2).is_empty());
	let intent_hits = FileBackend::retrieve_archived(dir.path(), &[], "oauth callback", 2);
	assert_eq!(intent_hits.len(), 1);
	assert_eq!(intent_hits[0].content, "oauth callback state");

	// Exact patterns outrank intent-only hits; duplicate rows dedup to one file.
	let ranked =
		FileBackend::retrieve_archived(dir.path(), &["postgres".to_string()], "oauth callback", 2);
	assert_eq!(ranked.len(), 2);
	assert_eq!(ranked[0].content, "postgres migration notes");
	assert_eq!(ranked[1].content, "oauth callback state");
}

#[test]
fn find_archived_by_content_prefers_the_latest_row_and_returns_none_when_absent() {
	let dir = tempfile::tempdir().unwrap();
	write_archived_lesson(dir.path(), "learning", "old.md", "same durable content");
	write_archived_lesson(dir.path(), "learning", "new.md", "same durable content");
	write_catalog(
		dir.path(),
		&[
			catalog_row("learning", "old.md", "old title", 0.5),
			catalog_row("learning", "new.md", "new title", 0.5),
		],
	);
	let found = FileBackend::find_archived_by_content(dir.path(), "same durable content");
	assert_eq!(
		found.unwrap().storage_path,
		dir.path()
			.join(".archive")
			.join("learning")
			.join("new.md")
			.display()
			.to_string()
	);

	assert!(FileBackend::find_archived_by_content(dir.path(), "absent content").is_none());
	let no_catalog = tempfile::tempdir().unwrap();
	assert!(
		FileBackend::find_archived_by_content(no_catalog.path(), "same durable content").is_none()
	);
}

#[test]
fn reactivate_archived_ignores_hot_storage_paths() {
	let mut item = lesson_with("hot entry", &[]);
	item.storage_path = "learning/project/developer/hot.md".to_string();
	assert!(FileBackend::reactivate_archived(&mut item).is_ok());
	assert_eq!(item.storage_path, "learning/project/developer/hot.md");

	item.storage_path = String::new();
	assert!(FileBackend::reactivate_archived(&mut item).is_ok());
}

#[test]
fn pool_normalize_mean_pools_and_renormalizes_multiple_chunks() {
	let single: Vec<f32> = vec![0.6, 0.8];
	assert_eq!(pool_normalize(&[single.as_slice()]), single);

	let pooled = pool_normalize(&[&[1.0, 0.0], &[0.0, 1.0]]);
	assert!((pooled[0] - pooled[1]).abs() < 1e-6);
	assert!((pooled[0].hypot(pooled[1]) - 1.0).abs() < 1e-6);

	// All-zero chunks stay zero instead of dividing by zero.
	assert_eq!(pool_normalize(&[&[0.0, 0.0], &[0.0, 0.0]]), vec![0.0, 0.0]);
}

#[test]
fn recency_factor_handles_invalid_and_future_dates() {
	let fresh = recency_factor(&chrono::Utc::now().to_rfc3339());
	assert!((fresh - 1.0).abs() < 1e-6);
	assert_eq!(recency_factor("not a date"), 0.0);
	assert_eq!(recency_factor(""), 0.0);
	let future = recency_factor("2999-01-01T00:00:00Z");
	assert!((future - 1.0).abs() < 1e-6);
	let old = recency_factor("2020-01-01T00:00:00Z");
	assert!(old > 0.0 && old < fresh);
}

#[test]
fn semantic_retrieval_chunks_falls_back_to_metadata_for_empty_content() {
	let mut lesson = lesson_with("", &[]);
	lesson.title = "metadata only".to_string();
	lesson.tags = vec!["tagged".to_string()];
	assert_eq!(
		semantic_retrieval_chunks(&lesson, 128),
		vec!["metadata only tagged".to_string()]
	);
}

#[test]
fn unescape_preserves_unknown_escapes_and_trailing_backslash() {
	assert_eq!(unescape(r"a\xb"), r"a\xb");
	assert_eq!(unescape("trailing\\"), "trailing\\");
	assert_eq!(unescape(r#"\"\n\\"#), "\"\n\\");
}

#[serial_test::serial]
#[tokio::test]
async fn store_retrieve_delete_and_has_lessons_across_scopes() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let data = tempfile::tempdir().unwrap();
	let previous = std::env::var_os("OCTOMIND_DATA_DIR");
	std::env::set_var("OCTOMIND_DATA_DIR", data.path());
	let backend = FileBackend;

	let mut scoped = lesson_with("scoped bearer token rule", &["auth"]);
	scoped.role = "developer".to_string();
	scoped.project = "project".to_string();
	scoped.created = "2026-01-01T00:00:00Z".to_string();
	scoped.importance = 0.8;
	backend.store(&scoped).await.unwrap();
	let mut global = lesson_with("global editor style rule", &["style"]);
	global.scope = "global".to_string();
	global.created = "2026-01-02T00:00:00Z".to_string();
	backend.store(&global).await.unwrap();

	assert_eq!(
		backend
			.retrieve_all("developer", "project")
			.await
			.unwrap()
			.len(),
		1
	);
	assert_eq!(backend.retrieve_global().await.unwrap().len(), 1);
	assert!(backend.has_lessons("developer", "project").await);
	assert!(!backend.has_lessons("writer", "other").await);
	assert!(backend
		.retrieve("", &[], "writer", "other", 5)
		.await
		.unwrap()
		.is_empty());

	// Empty query short-circuits to the importance-ordered head.
	let head = backend
		.retrieve("", &[], "developer", "project", 5)
		.await
		.unwrap();
	assert_eq!(head.len(), 1);
	assert_eq!(head[0].content, "scoped bearer token rule");

	// Keyword retrieval ranks the matching hot record without embeddings.
	let ranked = backend
		.retrieve(
			"auth intent",
			&["bearer".to_string()],
			"developer",
			"project",
			5,
		)
		.await
		.unwrap();
	assert_eq!(ranked.len(), 1);
	assert_eq!(ranked[0].content, "scoped bearer token rule");

	// Global ids delete from the shared dir; unknown ids fail loudly.
	backend
		.delete(&global.file_id(), "developer", "project")
		.await
		.unwrap();
	assert!(backend.retrieve_global().await.unwrap().is_empty());
	assert!(backend
		.delete("missing-id", "developer", "project")
		.await
		.is_err());

	// An archive-only scope still reports lessons via the catalog.
	crate::supervisor::learning::retention::archive_record(&scoped).unwrap();
	assert!(backend.has_lessons("developer", "project").await);

	if let Some(value) = previous {
		std::env::set_var("OCTOMIND_DATA_DIR", value);
	} else {
		std::env::remove_var("OCTOMIND_DATA_DIR");
	}
}

#[serial_test::serial]
#[tokio::test]
async fn reinforce_cold_archives_floor_importance_and_ignores_unknown_content() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let data = tempfile::tempdir().unwrap();
	let previous = std::env::var_os("OCTOMIND_DATA_DIR");
	std::env::set_var("OCTOMIND_DATA_DIR", data.path());
	let backend = FileBackend;

	let mut weak = lesson_with("barely useful rule", &[]);
	weak.role = "developer".to_string();
	weak.project = "project".to_string();
	weak.created = "2026-01-01T00:00:00Z".to_string();
	weak.importance = 0.05;
	backend.store(&weak).await.unwrap();

	backend
		.reinforce("content that exists nowhere", "developer", "project", 0.1)
		.await
		.unwrap();

	backend
		.reinforce(&weak.content, "developer", "project", -0.1)
		.await
		.unwrap();
	let hot = backend.retrieve_all("developer", "project").await.unwrap();
	assert!(hot.is_empty());
	let cold = FileBackend::read_archived(
		&crate::directories::get_learning_dir("developer", "project").unwrap(),
	);
	assert_eq!(cold.len(), 1);
	assert_eq!(cold[0].importance, 0.0);

	if let Some(value) = previous {
		std::env::set_var("OCTOMIND_DATA_DIR", value);
	} else {
		std::env::remove_var("OCTOMIND_DATA_DIR");
	}
}

#[serial_test::serial]
#[tokio::test]
async fn prune_stale_archives_only_stale_weak_entries() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let data = tempfile::tempdir().unwrap();
	let previous = std::env::var_os("OCTOMIND_DATA_DIR");
	std::env::set_var("OCTOMIND_DATA_DIR", data.path());
	let backend = FileBackend;

	let lesson = |content: &str, importance: f64, created: &str| {
		let mut item = lesson_with(content, &[]);
		item.role = "developer".to_string();
		item.project = "project".to_string();
		item.importance = importance;
		item.created = created.to_string();
		item
	};
	backend
		.store(&lesson("stale weak rule", 0.3, "2020-01-01T00:00:00Z"))
		.await
		.unwrap();
	backend
		.store(&lesson(
			"fresh weak rule",
			0.3,
			&chrono::Utc::now().to_rfc3339(),
		))
		.await
		.unwrap();
	backend
		.store(&lesson("stale strong rule", 0.9, "2020-01-01T00:00:00Z"))
		.await
		.unwrap();
	backend
		.store(&lesson("undated weak rule", 0.3, "not a date"))
		.await
		.unwrap();

	backend
		.prune_stale("developer", "project", 0)
		.await
		.unwrap();
	assert_eq!(
		backend
			.retrieve_all("developer", "project")
			.await
			.unwrap()
			.len(),
		4
	);

	backend
		.prune_stale("developer", "project", 365)
		.await
		.unwrap();
	let hot = backend.retrieve_all("developer", "project").await.unwrap();
	assert_eq!(hot.len(), 3);
	assert!(hot.iter().all(|item| item.content != "stale weak rule"));

	if let Some(value) = previous {
		std::env::set_var("OCTOMIND_DATA_DIR", value);
	} else {
		std::env::remove_var("OCTOMIND_DATA_DIR");
	}
}

#[test]
fn expand_ranked_with_links_returns_empty_at_zero_limit() {
	let all = vec![lesson_with("root memory", &[])];
	assert!(expand_ranked_with_links(&all, &[(1.0, 0)], 0).is_empty());
}

#[serial_test::serial]
#[tokio::test]
async fn reinforce_reactivates_archived_entries_and_bumps_them_in_place() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let data = tempfile::tempdir().unwrap();
	let previous = std::env::var_os("OCTOMIND_DATA_DIR");
	std::env::set_var("OCTOMIND_DATA_DIR", data.path());
	let backend = FileBackend;

	// Scoped entry: archived, then reinforced above the floor — it must move
	// back from .archive to the hot dir with bumped importance and use stats.
	let mut scoped = lesson_with("archived scoped rule about retries", &[]);
	scoped.role = "developer".to_string();
	scoped.project = "project".to_string();
	scoped.title = scoped.content.clone();
	scoped.created = "2026-01-01T00:00:00Z".to_string();
	scoped.importance = 0.3;
	backend.store(&scoped).await.unwrap();
	crate::supervisor::learning::retention::archive_record(&scoped).unwrap();
	assert!(backend
		.retrieve_all("developer", "project")
		.await
		.unwrap()
		.is_empty());

	backend
		.reinforce(&scoped.content, "developer", "project", 0.3)
		.await
		.unwrap();
	let hot = backend.retrieve_all("developer", "project").await.unwrap();
	assert_eq!(hot.len(), 1);
	assert_eq!(hot[0].content, "archived scoped rule about retries");
	assert!((hot[0].importance - 0.6).abs() < 1e-9);
	assert_eq!(hot[0].use_count, 1);
	assert!(!hot[0].last_used.is_empty());

	// Global entry: found through the global-archive fallback and reactivated
	// into the shared global dir.
	let mut global = lesson_with("archived global rule about logging", &[]);
	global.scope = "global".to_string();
	global.title = global.content.clone();
	global.created = "2026-01-02T00:00:00Z".to_string();
	global.importance = 0.3;
	backend.store(&global).await.unwrap();
	crate::supervisor::learning::retention::archive_record(&global).unwrap();

	backend
		.reinforce(&global.content, "developer", "project", 0.3)
		.await
		.unwrap();
	let hot_global = backend.retrieve_global().await.unwrap();
	assert_eq!(hot_global.len(), 1);
	assert_eq!(hot_global[0].content, "archived global rule about logging");
	assert!((hot_global[0].importance - 0.6).abs() < 1e-9);

	// A scoped id deletes from the scoped dir on the first candidate path.
	backend
		.delete(&scoped.file_id(), "developer", "project")
		.await
		.unwrap();
	assert!(backend
		.retrieve_all("developer", "project")
		.await
		.unwrap()
		.is_empty());
	assert_eq!(backend.retrieve_global().await.unwrap().len(), 1);

	if let Some(value) = previous {
		std::env::set_var("OCTOMIND_DATA_DIR", value);
	} else {
		std::env::remove_var("OCTOMIND_DATA_DIR");
	}
}

#[serial_test::serial]
#[tokio::test]
async fn retrieve_pages_cold_archive_when_hot_scope_is_empty() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let data = tempfile::tempdir().unwrap();
	let previous = std::env::var_os("OCTOMIND_DATA_DIR");
	std::env::set_var("OCTOMIND_DATA_DIR", data.path());
	let backend = FileBackend;

	let mut item = lesson_with("cold only rule about migrations", &[]);
	item.role = "developer".to_string();
	item.project = "project".to_string();
	item.title = item.content.clone();
	item.created = "2026-01-01T00:00:00Z".to_string();
	backend.store(&item).await.unwrap();
	crate::supervisor::learning::retention::archive_record(&item).unwrap();

	// Hot scope is empty but the catalog still pages the exact cold record.
	let recalled = backend
		.retrieve("", &["migrations".to_string()], "developer", "project", 5)
		.await
		.unwrap();
	assert_eq!(recalled.len(), 1);
	assert_eq!(recalled[0].content, "cold only rule about migrations");
	assert!(recalled[0].storage_path.contains(".archive"));

	if let Some(value) = previous {
		std::env::set_var("OCTOMIND_DATA_DIR", value);
	} else {
		std::env::remove_var("OCTOMIND_DATA_DIR");
	}
}

#[serial_test::serial]
#[tokio::test]
async fn retrieve_archived_global_pages_the_shared_archive() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let data = tempfile::tempdir().unwrap();
	let previous = std::env::var_os("OCTOMIND_DATA_DIR");
	std::env::set_var("OCTOMIND_DATA_DIR", data.path());
	let backend = FileBackend;

	let mut item = lesson_with("global cold rule about deployments", &[]);
	item.scope = "global".to_string();
	item.title = item.content.clone();
	item.created = "2026-01-01T00:00:00Z".to_string();
	backend.store(&item).await.unwrap();
	crate::supervisor::learning::retention::archive_record(&item).unwrap();

	let recalled = backend
		.retrieve_archived_global("", &["deployments".to_string()], 5)
		.await
		.unwrap();
	assert_eq!(recalled.len(), 1);
	assert_eq!(recalled[0].content, "global cold rule about deployments");

	if let Some(value) = previous {
		std::env::set_var("OCTOMIND_DATA_DIR", value);
	} else {
		std::env::remove_var("OCTOMIND_DATA_DIR");
	}
}
