// Copyright 2026 Muvon Un Limited
//
// Licensed under the Apache License, Version 2.0 (the "License")

//! Lesson extraction: calls LLM to analyze a session transcript and extract
//! generalizable lessons, then stores them via the configured backend.

use super::backend::create_backend;
use super::Lesson;
use crate::config::Config;
use anyhow::Result;

const EXTRACTION_SYSTEM_PROMPT: &str = r#"# Step 1: Decision
First, decide: does this conversation contain any USER corrections or USER-stated rules?
Output your decision:
<decision>LEARN</decision> or <decision>NONE</decision>

If NONE, stop here. Do not output anything else.

# Step 2: Extract (only if LEARN)
For each lesson, you MUST provide the exact user quote as evidence.
If you cannot quote the user, you do not have a lesson — skip it.

# What qualifies as a lesson
- User correction: user explicitly said something is wrong and stated the fix
- User-stated rule: user declared a project convention, preference, or constraint
- Repeated failure: user corrected the same type of mistake more than once

# What does NOT qualify
- Anything the AI discovered, debugged, or figured out without user input
- One-time implementation details or debugging steps
- Generic knowledge any developer would know
- Anything derivable by reading the codebase
- Successful AI actions that received no user feedback

# Rules
- Max 3 lessons. One strong lesson is better than three weak ones.
- confidence=high: direct user correction ("no, do X instead")
- confidence=medium: user-stated preference without direct correction
- State each lesson as a reusable rule, not a narrative

# Existing Lessons (DO NOT duplicate)
{existing_lessons}

# Output Format
<lesson confidence="high|medium" tags="keyword1,keyword2" evidence="exact user quote here">
Lesson text — what to do or avoid, stated as a rule.
</lesson>"#;

/// Shared extraction core: build transcript, call LLM, parse lessons, store with dedup.
///
/// Used by both `extract_lessons_detached` (fire-and-forget) and any caller that wants
/// awaited extraction. Takes owned data so it works without a `ChatSession` reference.
/// Cost is not tracked against the active session — this is background bookkeeping.
async fn run_extraction(
	messages: &[crate::session::Message],
	config: &Config,
	role: &str,
	project: &str,
	session_name: &str,
) -> Result<usize> {
	let learning = &config.learning;
	if !learning.enabled {
		return Ok(0);
	}

	let backend = create_backend(learning);
	crate::log_debug!(
		"Learning extraction: backend={}, role={}, project={}",
		learning.backend,
		role,
		project
	);

	// Retrieve existing lessons for dedup
	let existing = backend
		.retrieve_all(role, project, config)
		.await
		.unwrap_or_default();
	crate::log_debug!(
		"Learning extraction: {} existing lessons found for dedup",
		existing.len()
	);
	let existing_text = if existing.is_empty() {
		"(none)".to_string()
	} else {
		existing
			.iter()
			.map(|l| format!("- [{}] {}", l.confidence, l.content))
			.collect::<Vec<_>>()
			.join("\n")
	};

	let transcript = build_transcript(messages);
	if transcript.is_empty() {
		return Ok(0);
	}

	let system = EXTRACTION_SYSTEM_PROMPT.replace("{existing_lessons}", &existing_text);
	let response = call_extraction_llm(config, &learning.model, system, transcript).await?;

	// Gate: check <decision> tag first — if NONE, skip parsing entirely
	if !response.contains("<decision>LEARN</decision>") {
		crate::log_debug!("Learning extraction: model decided NONE — nothing to learn");
		return Ok(0);
	}

	let lessons = parse_lesson_tags(&response, role, project, session_name);
	crate::log_debug!(
		"Learning extraction: LLM returned {} lessons with evidence",
		lessons.len()
	);
	if lessons.is_empty() {
		return Ok(0);
	}

	// Store each, with content-based dedup against existing lessons
	let mut stored = 0;
	for lesson in &lessons {
		if is_duplicate(&lesson.content, &existing) {
			crate::log_debug!("Learning skipped (duplicate): {}", lesson.content);
			continue;
		}
		if let Err(e) = backend.store(lesson, config).await {
			crate::log_debug!("Learning store failed: {}", e);
		} else {
			stored += 1;
			crate::log_debug!(
				"Learning stored: [{}] {}",
				lesson.confidence,
				lesson.content
			);
		}
	}

	Ok(stored)
}

/// Build a compact transcript from session messages.
fn build_transcript(messages: &[crate::session::Message]) -> String {
	let mut transcript = String::new();
	for msg in messages {
		if msg.role == "system" {
			continue;
		}
		let role_label = match msg.role.as_str() {
			"user" => "USER",
			"assistant" => "ASSISTANT",
			"tool" => "TOOL",
			_ => continue,
		};

		// Truncate long messages to keep transcript manageable
		let content = if msg.content.len() > 500 {
			format!("{}...[truncated]", {
				let mut end = 500;
				while !msg.content.is_char_boundary(end) {
					end -= 1;
				}
				&msg.content[..end]
			})
		} else {
			msg.content.clone()
		};

		transcript.push_str(&format!("[{}]: {}\n\n", role_label, content));
	}
	transcript
}

/// Parse `<lesson>` tags from LLM response.
fn parse_lesson_tags(response: &str, role: &str, project: &str, source: &str) -> Vec<Lesson> {
	let mut lessons = Vec::new();
	let now = chrono::Utc::now().to_rfc3339();

	// Find all <lesson ...>...</lesson> blocks
	let mut remaining = response;
	while let Some(start) = remaining.find("<lesson") {
		let after_tag = &remaining[start..];
		let Some(close_bracket) = after_tag.find('>') else {
			break;
		};
		let attrs = &after_tag[7..close_bracket]; // between <lesson and >
		let after_open = &after_tag[close_bracket + 1..];
		let Some(end_tag) = after_open.find("</lesson>") else {
			break;
		};
		let content = after_open[..end_tag].trim();

		if !content.is_empty() {
			// Programmatic gate: reject lessons without evidence attribute
			let evidence = extract_attr(attrs, "evidence");
			if evidence.is_none() || evidence.as_ref().is_some_and(|e| e.trim().is_empty()) {
				crate::log_debug!(
					"Learning rejected (no evidence): {}",
					&content[..crate::utils::truncation::floor_char_boundary(content, 80)]
				);
				remaining = &after_open[end_tag + 9..];
				continue;
			}

			let confidence = extract_attr(attrs, "confidence").unwrap_or("medium".into());
			let tags_str = extract_attr(attrs, "tags").unwrap_or_default();
			let tags: Vec<String> = tags_str
				.split(',')
				.map(|t| t.trim().to_string())
				.filter(|t| !t.is_empty())
				.collect();

			let importance = match confidence.as_str() {
				"high" => 0.9,
				_ => 0.6, // medium or anything else
			};

			// Title: first 80 chars of content, trimmed to word boundary
			let title = if content.len() <= 80 {
				content.to_string()
			} else {
				let end = crate::utils::truncation::floor_char_boundary(content, 80);
				let truncated = &content[..end];
				truncated
					.rfind(' ')
					.map(|i| format!("{}...", &truncated[..i]))
					.unwrap_or_else(|| format!("{}...", truncated))
			};

			lessons.push(Lesson {
				content: content.to_string(),
				title,
				memory_type: "learning".into(),
				importance,
				confidence,
				tags,
				source: source.to_string(),
				role: role.to_string(),
				project: project.to_string(),
				created: now.clone(),
			});
		}

		remaining = &after_open[end_tag + 9..]; // skip past </lesson>
	}

	lessons
}

/// Check if a new lesson is a duplicate of an existing one by word overlap.
/// Returns true if >60% of words in the new content match any existing lesson.
fn is_duplicate(new_content: &str, existing: &[Lesson]) -> bool {
	let new_lower = new_content.to_lowercase();
	let new_words: std::collections::HashSet<String> =
		new_lower.split_whitespace().map(String::from).collect();

	if new_words.is_empty() {
		return false;
	}

	for existing_lesson in existing {
		let existing_lower = existing_lesson.content.to_lowercase();
		let existing_words: std::collections::HashSet<String> = existing_lower
			.split_whitespace()
			.map(String::from)
			.collect();
		let overlap = new_words.intersection(&existing_words).count();
		let similarity = overlap as f64 / new_words.len().max(1) as f64;
		if similarity > 0.6 {
			return true;
		}
	}
	false
}

/// Extract an XML attribute value: `key="value"`.
fn extract_attr(attrs: &str, key: &str) -> Option<String> {
	let pattern = format!("{}=\"", key);
	let start = attrs.find(&pattern)? + pattern.len();
	let end = attrs[start..].find('"')? + start;
	Some(attrs[start..end].to_string())
}

/// Fire-and-forget extraction. Spawns a detached tokio task — caller returns immediately.
///
/// This is the canonical extraction entry point: used by `/done`, `/exit`, Ctrl+D, and
/// auto-compaction. Lessons are extracted and stored in the background; the user is never
/// blocked on the LLM call. Errors are logged at debug level.
pub fn extract_lessons_detached(
	messages: Vec<crate::session::Message>,
	config: Config,
	role: String,
	project: String,
	session_name: String,
) {
	tokio::spawn(async move {
		match run_extraction(&messages, &config, &role, &project, &session_name).await {
			Ok(0) => crate::log_debug!("Learning detached: no lessons extracted"),
			Ok(n) => crate::log_debug!("Learning detached: {} lessons extracted", n),
			Err(e) => crate::log_debug!("Learning detached extraction failed: {}", e),
		}
	});
}

/// LLM call for lesson extraction — no `ChatSession` reference, no cost tracking.
async fn call_extraction_llm(
	config: &Config,
	model: &str,
	system_content: String,
	user_content: String,
) -> Result<String> {
	let now = crate::utils::time::now_secs();
	let messages = vec![
		crate::session::Message {
			role: "system".to_string(),
			content: system_content,
			timestamp: now,
			cached: false,
			cache_ttl: None,
			tool_call_id: None,
			name: None,
			tool_calls: None,
			images: None,
			videos: None,
			thinking: None,
			id: None,
		},
		crate::session::Message {
			role: "user".to_string(),
			content: user_content,
			timestamp: now,
			cached: false,
			cache_ttl: None,
			tool_call_id: None,
			name: None,
			tool_calls: None,
			images: None,
			videos: None,
			thinking: None,
			id: None,
		},
	];

	let params = crate::session::ChatCompletionWithValidationParams::new(
		&messages, model, 0.3, 1.0, 0, 4096, config,
	)
	.with_max_retries(1);

	let response = crate::session::chat_completion_with_validation(params).await?;
	Ok(response.content)
}

/// Call the learning LLM (cheap model) for extraction or retrieval prep.
pub(crate) async fn call_learning_llm(
	config: &Config,
	model: &str,
	system_content: String,
	user_content: String,
	operation_rx: tokio::sync::watch::Receiver<bool>,
) -> Result<String> {
	let now = crate::utils::time::now_secs();
	let messages = vec![
		crate::session::Message {
			role: "system".to_string(),
			content: system_content,
			timestamp: now,
			cached: false,
			cache_ttl: None,
			tool_call_id: None,
			name: None,
			tool_calls: None,
			images: None,
			videos: None,
			thinking: None,
			id: None,
		},
		crate::session::Message {
			role: "user".to_string(),
			content: user_content,
			timestamp: now,
			cached: false,
			cache_ttl: None,
			tool_call_id: None,
			name: None,
			tool_calls: None,
			images: None,
			videos: None,
			thinking: None,
			id: None,
		},
	];

	let params = crate::session::ChatCompletionWithValidationParams::new(
		&messages, model, 0.3,  // low temperature for structured output
		1.0,  // top_p
		0,    // top_k (0 = default)
		4096, // max_tokens
		config,
	)
	.with_max_retries(1)
	.with_full_context_tokens(true)
	.with_cancellation_token(operation_rx);

	let response = crate::session::chat_completion_with_validation(params).await?;
	Ok(response.content)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_parse_lesson_tags_single() {
		let response = r#"Some preamble text.
<lesson confidence="high" tags="auth,api" evidence="use bearer tokens not basic auth">
Bearer token auth is required for all endpoints
</lesson>
Some trailing text."#;

		let lessons = parse_lesson_tags(response, "developer", "octofs", "test-session");
		assert_eq!(lessons.len(), 1);
		assert_eq!(
			lessons[0].content,
			"Bearer token auth is required for all endpoints"
		);
		assert_eq!(lessons[0].confidence, "high");
		assert_eq!(lessons[0].importance, 0.9);
		assert_eq!(lessons[0].tags, vec!["auth", "api"]);
		assert_eq!(lessons[0].role, "developer");
		assert_eq!(lessons[0].project, "octofs");
	}

	#[test]
	fn test_parse_lesson_tags_multiple() {
		let response = r#"
<lesson confidence="high" tags="error" evidence="no, use custom error types">
Use custom error types not anyhow
</lesson>
<lesson confidence="medium" tags="style" evidence="I prefer single PRs">
User prefers single PRs
</lesson>"#;

		let lessons = parse_lesson_tags(response, "dev", "proj", "src");
		assert_eq!(lessons.len(), 2);
		assert_eq!(lessons[0].confidence, "high");
		assert_eq!(lessons[0].importance, 0.9);
		assert_eq!(lessons[1].confidence, "medium");
		assert_eq!(lessons[1].importance, 0.6);
	}

	#[test]
	fn test_parse_lesson_tags_empty_content_skipped() {
		let response = r#"<lesson confidence="high" tags="x" evidence="some quote">
</lesson>"#;
		let lessons = parse_lesson_tags(response, "dev", "proj", "src");
		assert_eq!(lessons.len(), 0);
	}

	#[test]
	fn test_parse_lesson_tags_no_evidence_rejected() {
		let response = r#"<lesson confidence="high" tags="x">
This lesson has no evidence attribute and should be rejected
</lesson>"#;
		let lessons = parse_lesson_tags(response, "dev", "proj", "src");
		assert_eq!(lessons.len(), 0);
	}

	#[test]
	fn test_parse_lesson_tags_no_lessons() {
		let response = "No lessons to extract from this session.";
		let lessons = parse_lesson_tags(response, "dev", "proj", "src");
		assert_eq!(lessons.len(), 0);
	}

	#[test]
	fn test_parse_lesson_tags_missing_confidence_defaults_medium() {
		let response = r#"<lesson tags="test" evidence="user said something">
Some lesson without confidence attr
</lesson>"#;
		let lessons = parse_lesson_tags(response, "dev", "proj", "src");
		assert_eq!(lessons.len(), 1);
		assert_eq!(lessons[0].confidence, "medium");
		assert_eq!(lessons[0].importance, 0.6);
	}

	#[test]
	fn test_is_duplicate_high_overlap() {
		let existing = vec![Lesson {
			content: "Bearer token auth is required for all API endpoints".into(),
			..Default::default()
		}];
		assert!(is_duplicate(
			"Bearer token auth is required for all octofs API endpoints",
			&existing
		));
	}

	#[test]
	fn test_is_duplicate_no_overlap() {
		let existing = vec![Lesson {
			content: "Bearer token auth is required for all API endpoints".into(),
			..Default::default()
		}];
		assert!(!is_duplicate(
			"Use custom error types instead of anyhow",
			&existing
		));
	}

	#[test]
	fn test_extract_attr() {
		assert_eq!(
			extract_attr(r#" confidence="high" tags="a,b""#, "confidence"),
			Some("high".into())
		);
		assert_eq!(
			extract_attr(r#" confidence="high" tags="a,b""#, "tags"),
			Some("a,b".into())
		);
		assert_eq!(extract_attr(r#" confidence="high""#, "missing"), None);
	}

	#[test]
	fn test_build_transcript() {
		let messages = vec![
			crate::session::Message {
				role: "system".into(),
				content: "You are helpful".into(),
				timestamp: 0,
				cached: false,
				cache_ttl: None,
				tool_call_id: None,
				name: None,
				tool_calls: None,
				images: None,
				videos: None,
				thinking: None,
				id: None,
			},
			crate::session::Message {
				role: "user".into(),
				content: "Fix the auth bug".into(),
				timestamp: 0,
				cached: false,
				cache_ttl: None,
				tool_call_id: None,
				name: None,
				tool_calls: None,
				images: None,
				videos: None,
				thinking: None,
				id: None,
			},
			crate::session::Message {
				role: "assistant".into(),
				content: "I'll fix it".into(),
				timestamp: 0,
				cached: false,
				cache_ttl: None,
				tool_call_id: None,
				name: None,
				tool_calls: None,
				images: None,
				videos: None,
				thinking: None,
				id: None,
			},
		];
		let transcript = build_transcript(&messages);
		assert!(!transcript.contains("system"));
		assert!(!transcript.contains("You are helpful"));
		assert!(transcript.contains("[USER]: Fix the auth bug"));
		assert!(transcript.contains("[ASSISTANT]: I'll fix it"));
	}
}
