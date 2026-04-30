// Copyright 2026 Muvon Un Limited
//
// Licensed under the Apache License, Version 2.0 (the "License")

use super::super::Lesson;
use super::LearningBackend;
use crate::config::Config;
use anyhow::Result;
use async_trait::async_trait;
use std::path::PathBuf;

pub struct FileBackend;

impl FileBackend {
	fn learning_dir(role: &str, project: &str) -> Result<PathBuf> {
		crate::directories::get_learning_dir(role, project)
	}

	/// Parse a lesson `.md` file with YAML frontmatter.
	/// Simple key-value parser — no serde_yaml dependency needed.
	fn parse_lesson_file(content: &str) -> Option<Lesson> {
		let content = content.trim();
		if !content.starts_with("---") {
			return None;
		}
		let after_first = &content[3..];
		let end = after_first.find("---")?;
		let yaml_str = after_first[..end].trim();

		let mut lesson = Lesson::default();
		for line in yaml_str.lines() {
			let line = line.trim();
			let Some((key, val)) = line.split_once(':') else {
				continue;
			};
			let key = key.trim();
			let val = val.trim().trim_matches('"');
			match key {
				"title" => lesson.title = val.to_string(),
				"content" => lesson.content = val.to_string(),
				"memory_type" => lesson.memory_type = val.to_string(),
				"importance" => lesson.importance = val.parse().unwrap_or(0.5),
				"confidence" => lesson.confidence = val.to_string(),
				"tags" => {
					// Parse [tag1, tag2] format
					let inner = val.trim_start_matches('[').trim_end_matches(']');
					lesson.tags = inner
						.split(',')
						.map(|t| t.trim().to_string())
						.filter(|t| !t.is_empty())
						.collect();
				}
				"source" => lesson.source = val.to_string(),
				"role" => lesson.role = val.to_string(),
				"project" => lesson.project = val.to_string(),
				"created" => lesson.created = val.to_string(),
				_ => {}
			}
		}

		if lesson.content.is_empty() {
			None
		} else {
			Some(lesson)
		}
	}

	fn slugify(text: &str, max_len: usize) -> String {
		text.chars()
			.filter_map(|c| {
				if c.is_alphanumeric() {
					Some(c.to_ascii_lowercase())
				} else if c == ' ' || c == '_' || c == '-' {
					Some('-')
				} else {
					None
				}
			})
			.take(max_len)
			.collect::<String>()
			.trim_end_matches('-')
			.to_string()
	}
}

#[async_trait]
impl LearningBackend for FileBackend {
	async fn store(&self, lesson: &Lesson, _config: &Config) -> Result<()> {
		let dir = Self::learning_dir(&lesson.role, &lesson.project)?;
		let slug = Self::slugify(&lesson.content, 40);
		let ts = lesson
			.created
			.replace([':', '-', 'T'], "")
			.chars()
			.take(14)
			.collect::<String>();
		let filename = if slug.is_empty() {
			format!("{}.md", ts)
		} else {
			format!("{}-{}.md", ts, slug)
		};

		let tags_str = lesson.tags.join(", ");
		let content = format!(
			"---\ntitle: \"{}\"\ncontent: \"{}\"\nmemory_type: {}\nimportance: {}\nconfidence: {}\ntags: [{}]\nsource: \"{}\"\nrole: \"{}\"\nproject: \"{}\"\ncreated: \"{}\"\n---\n",
			lesson.title.replace('"', "\\\""),
			lesson.content.replace('"', "\\\""),
			lesson.memory_type,
			lesson.importance,
			lesson.confidence,
			tags_str,
			lesson.source,
			lesson.role,
			lesson.project,
			lesson.created,
		);

		std::fs::write(dir.join(filename), content)?;
		Ok(())
	}

	async fn retrieve(
		&self,
		intent: &str,
		patterns: &[String],
		role: &str,
		project: &str,
		limit: usize,
		config: &Config,
	) -> Result<Vec<Lesson>> {
		let dir = Self::learning_dir(role, project)?;
		if !dir.exists() {
			return Ok(Vec::new());
		}

		let all = self.retrieve_all(role, project, config).await?;
		if all.is_empty() {
			return Ok(Vec::new());
		}
		if patterns.is_empty() && intent.trim().is_empty() {
			return Ok(all.into_iter().take(limit).collect());
		}

		// Sparse signal: LLM-extracted keywords → substring count → ranked by hits.
		let keyword_ranking = rank_by_keywords(&all, patterns);

		// Dense signal: BGE-small cosine. Skip silently if the model isn't
		// ready yet (warmup pending, no network, etc.) — keyword ranking
		// alone still produces a result. Same fall-through pattern as
		// capability auto-activation.
		let cosine_ranking = if intent.trim().is_empty() || !crate::embeddings::is_ready() {
			Vec::new()
		} else {
			match rank_by_cosine(&all, intent).await {
				Ok(r) => r,
				Err(e) => {
					crate::log_debug!("learning retrieve: cosine ranking failed ({})", e);
					Vec::new()
				}
			}
		};

		// Fuse both rankings via Reciprocal Rank Fusion (Cormack et al. 2009).
		// Returns indices into `all` sorted by fused score descending.
		let mut rankings: Vec<&[usize]> = Vec::with_capacity(2);
		rankings.push(&keyword_ranking);
		if !cosine_ranking.is_empty() {
			rankings.push(&cosine_ranking);
		}
		let fused = reciprocal_rank_fusion(all.len(), &rankings);

		Ok(fused
			.into_iter()
			.take(limit)
			.map(|(_, i)| all[i].clone())
			.collect())
	}

	async fn retrieve_all(
		&self,
		role: &str,
		project: &str,
		_config: &Config,
	) -> Result<Vec<Lesson>> {
		let dir = Self::learning_dir(role, project)?;
		if !dir.exists() {
			return Ok(Vec::new());
		}

		let mut lessons = Vec::new();
		for entry in std::fs::read_dir(&dir)? {
			let entry = entry?;
			let path = entry.path();
			if path.extension().is_some_and(|e| e == "md") {
				if let Ok(content) = std::fs::read_to_string(&path) {
					if let Some(lesson) = Self::parse_lesson_file(&content) {
						lessons.push(lesson);
					}
				}
			}
		}

		// Sort by importance descending, then confidence
		lessons.sort_by(|a, b| {
			b.importance
				.partial_cmp(&a.importance)
				.unwrap_or(std::cmp::Ordering::Equal)
		});

		Ok(lessons)
	}
}

/// RRF constant from Cormack, Clarke & Buettcher (2009). 60 is the
/// canonical value — high enough that early ranks dominate without
/// crushing later ranks completely.
const RRF_K: f32 = 60.0;

/// Rank lessons by sparse keyword hit count (descending). Returns indices
/// into the input slice, in ranked order. Lessons with zero hits are
/// excluded so they don't pollute the fused ranking. Pure helper —
/// embedding-free, instant.
fn rank_by_keywords(lessons: &[Lesson], patterns: &[String]) -> Vec<usize> {
	if patterns.is_empty() {
		return Vec::new();
	}
	let patterns_lower: Vec<String> = patterns.iter().map(|p| p.to_lowercase()).collect();
	let mut scored: Vec<(usize, usize)> = lessons
		.iter()
		.enumerate()
		.map(|(i, l)| {
			let haystack = format!(
				"{} {} {}",
				l.title.to_lowercase(),
				l.content.to_lowercase(),
				l.tags.join(" ").to_lowercase()
			);
			let hits = patterns_lower
				.iter()
				.filter(|p| haystack.contains(p.as_str()))
				.count();
			(hits, i)
		})
		.filter(|(hits, _)| *hits > 0)
		.collect();
	scored.sort_by_key(|b| std::cmp::Reverse(b.0));
	scored.into_iter().map(|(_, i)| i).collect()
}

/// Rank lessons by BGE-small cosine vs the user intent (descending).
/// Each lesson is embedded as `title + content + tags` (cached by content
/// hash, so repeat retrievals in the same session are free). Lessons with
/// cosine ≤ 0.2 are excluded as noise — too far from the intent to be
/// worth surfacing, even via fusion. Returns indices into the input slice.
async fn rank_by_cosine(lessons: &[Lesson], intent: &str) -> Result<Vec<usize>> {
	let intent_vec = crate::embeddings::embed(intent).await?;
	let lesson_texts: Vec<String> = lessons
		.iter()
		.map(|l| {
			// BGE-small handles ~512 tokens. Embed a representative slice
			// (title + content + tags); fastembed truncates at the model
			// limit so we don't strictly need to pre-cap, but a 4KB cap
			// avoids embedding multi-megabyte lessons unnecessarily.
			let combined = format!("{} {} {}", l.title, l.content, l.tags.join(" "));
			combined.chars().take(4000).collect()
		})
		.collect();
	let lesson_vecs = crate::embeddings::embed_many(&lesson_texts).await?;
	let mut scored: Vec<(f32, usize)> = lesson_vecs
		.iter()
		.enumerate()
		.map(|(i, v)| (crate::embeddings::cosine(&intent_vec, v), i))
		.filter(|(s, _)| *s > 0.2)
		.collect();
	scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
	Ok(scored.into_iter().map(|(_, i)| i).collect())
}

/// Reciprocal Rank Fusion: given multiple ranked lists of indices into
/// the same item set, fuse into a single ranking by summing
/// `1 / (RRF_K + rank)` across methods. Items appearing high in multiple
/// rankings score highest; items appearing in only one method still
/// contribute. Returns `(fused_score, item_index)` sorted by score
/// descending. Items not in any ranking are excluded.
///
/// Reference: Cormack, Clarke & Buettcher, "Reciprocal Rank Fusion
/// outperforms Condorcet and individual rank learning methods" (SIGIR
/// 2009). Used in production by Anthropic Contextual Retrieval and
/// most modern hybrid-search engines.
fn reciprocal_rank_fusion(total: usize, rankings: &[&[usize]]) -> Vec<(f32, usize)> {
	if total == 0 || rankings.is_empty() {
		return Vec::new();
	}
	let mut scores = vec![0.0_f32; total];
	for ranking in rankings {
		for (rank_zero_based, &idx) in ranking.iter().enumerate() {
			if idx < scores.len() {
				// RRF uses 1-indexed rank; +1 to convert from zero-based.
				scores[idx] += 1.0 / (RRF_K + rank_zero_based as f32 + 1.0);
			}
		}
	}
	let mut out: Vec<(f32, usize)> = scores
		.iter()
		.enumerate()
		.map(|(i, s)| (*s, i))
		.filter(|(s, _)| *s > 0.0)
		.collect();
	out.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
	out
}

#[cfg(test)]
mod tests {
	use super::*;

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
---
"#;
		let lesson = FileBackend::parse_lesson_file(content).unwrap();
		assert_eq!(lesson.content, "Bearer token auth required");
		assert_eq!(lesson.importance, 0.8);
		assert_eq!(lesson.confidence, "high");
		assert_eq!(lesson.tags, vec!["auth", "api"]);
		assert_eq!(lesson.role, "developer");
		assert_eq!(lesson.project, "octofs");
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
	fn test_slugify() {
		assert_eq!(
			FileBackend::slugify("Bearer token auth", 20),
			"bearer-token-auth"
		);
		assert_eq!(
			FileBackend::slugify("Use custom_types!", 15),
			"use-custom-type"
		);
		assert_eq!(FileBackend::slugify("", 10), "");
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
}
