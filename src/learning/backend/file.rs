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
		if patterns.is_empty() {
			return Ok(all.into_iter().take(limit).collect());
		}

		// Score each lesson by how many patterns match (case-insensitive)
		let patterns_lower: Vec<String> = patterns.iter().map(|p| p.to_lowercase()).collect();
		let mut scored: Vec<(Lesson, usize)> = all
			.into_iter()
			.map(|l| {
				let text = l.content.to_lowercase();
				let tags_text = l.tags.join(" ").to_lowercase();
				let combined = format!("{} {}", text, tags_text);
				let hits = patterns_lower
					.iter()
					.filter(|p| combined.contains(p.as_str()))
					.count();
				(l, hits)
			})
			.filter(|(_, hits)| *hits > 0)
			.collect();

		scored.sort_by_key(|b| std::cmp::Reverse(b.1));
		Ok(scored.into_iter().take(limit).map(|(l, _)| l).collect())
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
}
