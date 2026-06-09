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

# Scope (REQUIRED on every lesson)
- scope="global": a durable preference about HOW THIS USER WORKS, true in EVERY
  project and role (e.g. "always open a single PR", "never add silent fallbacks",
  "the user runs build/test commands themselves"). Use ONLY when the rule is
  clearly about the user's general way of working — NOT tied to this task, this
  project, or this role.
- scope="scoped" (default): a rule about THIS project, role, or task.
Be conservative: most lessons are "scoped". When unsure, use "scoped".

# Rules
- Max 3 lessons. One strong lesson is better than three weak ones.
- confidence=high: direct user correction ("no, do X instead")
- confidence=medium: user-stated preference without direct correction
- State each lesson as a reusable rule, not a narrative

# Existing Lessons (DO NOT duplicate; refine one only if the user changed their mind)
{existing_lessons}

# Output Format
<lesson scope="global|scoped" confidence="high|medium" tags="keyword1,keyword2" evidence="exact user quote here">
Lesson text — what to do or avoid, stated as a rule.
</lesson>"#;

/// Appended to the extraction prompt when orientation capture is enabled.
const ORIENTATION_SECTION: &str = r#"

# Orientation (separate from lessons — always consider, independent of the decision above)
Capture up to 2 pieces of DURABLE UNDERSTANDING about the subject that took real work
to discover and would save re-exploration next time: architecture, key decisions,
structure, constraints, or non-obvious facts (e.g. "auth is delegated to octolib",
"deploy runs on GitLab not GitHub", "the dataset's date column is epoch milliseconds").
Do NOT capture transient state, exact line numbers, or anything one search recovers.
These need no user quote.
<orientation tags="keyword1,keyword2" confidence="high|medium">
A durable, reusable fact about how the subject works.
</orientation>"#;

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
	let learning = &config.supervisor.learning;
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

	// Retrieve existing lessons (scoped + global) for dedup context and supersede.
	let existing_scoped = backend
		.retrieve_all(role, project, config)
		.await
		.unwrap_or_default();
	let existing_global = backend.retrieve_global(config).await.unwrap_or_default();
	crate::log_debug!(
		"Learning extraction: {} scoped + {} global existing lessons",
		existing_scoped.len(),
		existing_global.len()
	);
	let existing_text = format_existing(&existing_scoped, &existing_global);

	let transcript = build_transcript(messages);
	if transcript.is_empty() {
		return Ok(0);
	}

	let mut system = EXTRACTION_SYSTEM_PROMPT.replace("{existing_lessons}", &existing_text);
	if config.supervisor.orientation.enabled {
		system.push_str(ORIENTATION_SECTION);
	}
	let response = call_extraction_llm(config, &learning.model, system, transcript).await?;

	let mut stored = 0;

	// Orientation: durable subject understanding. Independent of the lesson
	// decision gate; no user evidence required. Deduped vs existing orientation.
	if config.supervisor.orientation.enabled {
		let orientations = parse_orientation_tags(&response, role, project, session_name);
		let existing_or: Vec<Lesson> = existing_scoped
			.iter()
			.filter(|l| l.memory_type == "orientation")
			.cloned()
			.collect();
		for o in &orientations {
			if existing_or
				.iter()
				.any(|e| e.content.trim() == o.content.trim())
			{
				continue;
			}
			if let Some(old) = best_overlap(&o.content, &existing_or) {
				let _ = backend
					.delete(&old.file_id(), &old.role, &old.project, config)
					.await;
			}
			if backend.store(o, config).await.is_ok() {
				stored += 1;
				crate::supervisor::stats::orientation(1);
				crate::log_debug!("Orientation stored: {}", o.content);
			}
		}
	}

	// Lessons: gated by the model's decision; require user evidence. Orientation
	// above is independent, so still return its count even when there are no lessons.
	if !response.contains("<decision>LEARN</decision>") {
		crate::log_debug!("Learning extraction: model decided NONE — no lessons");
		return Ok(stored);
	}

	let lessons = parse_lesson_tags(&response, role, project, session_name);
	crate::log_debug!(
		"Learning extraction: LLM returned {} lessons with evidence",
		lessons.len()
	);
	if lessons.is_empty() {
		return Ok(stored);
	}

	// Store each. Match within the same scope. Identical content is skipped;
	// a refinement (high word overlap) supersedes the stale lesson — delete the
	// old, write the new — so a correction to a previous correction wins instead
	// of being silently dropped.
	// Dedup lessons against existing lessons only (exclude orientation entries
	// that share the same store).
	let existing_lessons_scoped: Vec<Lesson> = existing_scoped
		.iter()
		.filter(|l| l.memory_type != "orientation")
		.cloned()
		.collect();
	for lesson in &lessons {
		let existing = if lesson.scope == "global" {
			&existing_global
		} else {
			&existing_lessons_scoped
		};

		if existing
			.iter()
			.any(|e| e.content.trim() == lesson.content.trim())
		{
			crate::log_debug!("Learning skipped (identical): {}", lesson.content);
			continue;
		}

		if let Some(old) = best_overlap(&lesson.content, existing) {
			if let Err(e) = backend
				.delete(&old.file_id(), &old.role, &old.project, config)
				.await
			{
				crate::log_debug!("Learning supersede delete failed: {}", e);
			} else {
				crate::log_debug!("Learning superseded: {} → {}", old.content, lesson.content);
			}
		}

		if let Err(e) = backend.store(lesson, config).await {
			crate::log_debug!("Learning store failed: {}", e);
		} else {
			stored += 1;
			crate::supervisor::stats::lessons(1);
			crate::log_debug!(
				"Learning stored: [{}/{}] {}",
				lesson.scope,
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
			// Scope is "global" only when the model explicitly says so; anything
			// else (missing, typo, "scoped") falls back to scoped.
			let scope = match extract_attr(attrs, "scope").as_deref() {
				Some("global") => "global".to_string(),
				_ => "scoped".to_string(),
			};
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
				scope,
				created: now.clone(),
			});
		}

		remaining = &after_open[end_tag + 9..]; // skip past </lesson>
	}

	lessons
}

/// Parse `<orientation>` tags — durable subject understanding. No evidence
/// required; stored with memory_type = "orientation", always scoped.
fn parse_orientation_tags(response: &str, role: &str, project: &str, source: &str) -> Vec<Lesson> {
	let mut out = Vec::new();
	let now = chrono::Utc::now().to_rfc3339();
	let mut remaining = response;
	while let Some(start) = remaining.find("<orientation") {
		let after_tag = &remaining[start..];
		let Some(close_bracket) = after_tag.find('>') else {
			break;
		};
		let attrs = &after_tag[12..close_bracket]; // between `<orientation` and `>`
		let after_open = &after_tag[close_bracket + 1..];
		let Some(end_tag) = after_open.find("</orientation>") else {
			break;
		};
		let content = after_open[..end_tag].trim();
		if !content.is_empty() {
			let confidence = extract_attr(attrs, "confidence").unwrap_or("medium".into());
			let tags: Vec<String> = extract_attr(attrs, "tags")
				.unwrap_or_default()
				.split(',')
				.map(|t| t.trim().to_string())
				.filter(|t| !t.is_empty())
				.collect();
			let importance = if confidence == "high" { 0.8 } else { 0.55 };
			let title = if content.len() <= 80 {
				content.to_string()
			} else {
				let end = crate::utils::truncation::floor_char_boundary(content, 80);
				format!("{}...", &content[..end])
			};
			out.push(Lesson {
				content: content.to_string(),
				title,
				memory_type: "orientation".into(),
				importance,
				confidence,
				tags,
				source: source.to_string(),
				role: role.to_string(),
				project: project.to_string(),
				scope: "scoped".into(),
				created: now.clone(),
			});
		}
		remaining = &after_open[end_tag + 14..]; // skip past </orientation>
	}
	out
}

/// Word-overlap ratio (0..1): fraction of the new content's words that also
/// appear in the existing content. Case-insensitive, whitespace-tokenized.
fn word_overlap(new_content: &str, existing_content: &str) -> f64 {
	let new_lower = new_content.to_lowercase();
	let new_words: std::collections::HashSet<&str> = new_lower.split_whitespace().collect();
	if new_words.is_empty() {
		return 0.0;
	}
	let existing_lower = existing_content.to_lowercase();
	let existing_words: std::collections::HashSet<&str> =
		existing_lower.split_whitespace().collect();
	let overlap = new_words.intersection(&existing_words).count();
	overlap as f64 / new_words.len() as f64
}

/// Find the existing lesson most similar to `new_content` above the 0.6
/// overlap threshold — the candidate to supersede. None if nothing is close.
fn best_overlap<'a>(new_content: &str, existing: &'a [Lesson]) -> Option<&'a Lesson> {
	existing
		.iter()
		.map(|l| (word_overlap(new_content, &l.content), l))
		.filter(|(s, _)| *s > 0.6)
		.max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
		.map(|(_, l)| l)
}

/// Format existing lessons (global + scoped) for the extraction prompt's
/// dedup context, so the model neither duplicates nor wrongly re-scopes them.
fn format_existing(scoped: &[Lesson], global: &[Lesson]) -> String {
	let fmt = |ls: &[Lesson]| {
		ls.iter()
			.map(|l| format!("- [{}] {}", l.confidence, l.content))
			.collect::<Vec<_>>()
			.join("\n")
	};
	let mut out = String::new();
	if !global.is_empty() {
		out.push_str("## Global (user-wide)\n");
		out.push_str(&fmt(global));
		out.push('\n');
	}
	if !scoped.is_empty() {
		out.push_str("## This project/role\n");
		out.push_str(&fmt(scoped));
		out.push('\n');
	}
	if out.is_empty() {
		"(none)".to_string()
	} else {
		out
	}
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

/// Higher-level convenience wrapper that consolidates the common pre-call prep
/// shared by /done, /exit, Ctrl+D and auto-compaction:
///
/// - early-return when `config.supervisor.learning.enabled` is false (matches existing site gates),
/// - derive `project` from the supplied `current_dir` (or process cwd when `None`),
/// - snapshot `session.messages` for the detached task.
///
/// Pass `current_dir = Some(...)` from interactive sessions that thread the
/// thread-local session cwd; pass `None` to fall back to `std::env::current_dir()`
/// (auto-compaction / `/done` path).
pub fn spawn_lesson_extraction(
	session: &crate::session::chat::session::ChatSession,
	config: &Config,
	role: String,
	current_dir: Option<&std::path::Path>,
) {
	if !config.supervisor.learning.enabled {
		return;
	}
	if session.gate_failed {
		crate::log_debug!("Distill skipped: trajectory failed verify-gate");
		return;
	}
	let owned_cwd;
	let resolved_dir: Option<&std::path::Path> = match current_dir {
		Some(p) => Some(p),
		None => {
			owned_cwd = std::env::current_dir().ok();
			owned_cwd.as_deref()
		}
	};
	let project = resolved_dir
		.and_then(|p| p.file_name())
		.and_then(|n| n.to_str())
		.map(String::from)
		.unwrap_or_else(|| "unknown".to_string());
	extract_lessons_detached(
		session.session.messages.clone(),
		config.clone(),
		role,
		project,
		session.session.info.name.clone(),
	);
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
	if let Some(usage) = &response.exchange.usage {
		crate::supervisor::stats::record_call(
			usage.input_tokens,
			usage.output_tokens,
			usage.cost.unwrap_or(0.0),
		);
	}
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
	if let Some(usage) = &response.exchange.usage {
		crate::supervisor::stats::record_call(
			usage.input_tokens,
			usage.output_tokens,
			usage.cost.unwrap_or(0.0),
		);
	}
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
	fn test_best_overlap_finds_refinement() {
		let existing = vec![Lesson {
			content: "Bearer token auth is required for all API endpoints".into(),
			..Default::default()
		}];
		// High overlap → returns the stale lesson to supersede.
		assert!(best_overlap(
			"Bearer token auth is required for all octofs API endpoints",
			&existing
		)
		.is_some());
	}

	#[test]
	fn test_best_overlap_none_when_unrelated() {
		let existing = vec![Lesson {
			content: "Bearer token auth is required for all API endpoints".into(),
			..Default::default()
		}];
		assert!(best_overlap("Use custom error types instead of anyhow", &existing).is_none());
	}

	#[test]
	fn test_parse_lesson_tags_scope() {
		let response = r#"<decision>LEARN</decision>
<lesson scope="global" confidence="high" tags="style" evidence="always single PR">
Always open a single PR
</lesson>
<lesson confidence="medium" tags="proj" evidence="use X here">
This project uses X
</lesson>"#;
		let lessons = parse_lesson_tags(response, "dev", "proj", "src");
		assert_eq!(lessons.len(), 2);
		assert_eq!(lessons[0].scope, "global");
		// scope omitted → defaults to scoped.
		assert_eq!(lessons[1].scope, "scoped");
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
