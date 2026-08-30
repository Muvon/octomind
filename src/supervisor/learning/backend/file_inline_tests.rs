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
