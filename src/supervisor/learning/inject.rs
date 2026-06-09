// Copyright 2026 Muvon Un Limited
//
// Licensed under the Apache License, Version 2.0 (the "License")

//! Retrieval and injection: fetches relevant lessons and injects them
//! into the system prompt at session start.

use super::backend::create_backend;
use crate::config::Config;
use anyhow::Result;

const FILE_RETRIEVAL_PROMPT: &str = r#"# Task
Given the user's request below, output 3-5 search keywords (one per line) to find relevant lessons from past sessions.

# Rules
- Focus on: tool names, error names, domain terms, API names, action verbs.
- One keyword per line, lowercase.
- Output ONLY the keywords — no explanations, no numbering, no punctuation."#;

const MCP_RETRIEVAL_PROMPT: &str = r#"# Task
Given the user's request below, write a single concise semantic search query to find relevant lessons from past sessions.

# Rules
- Natural language, optimized for embedding similarity search.
- Include key domain terms and intent.
- Output ONLY the query — one line, no explanations."#;

/// Retrieve relevant lessons for the current message and format them for
/// injection. Two tiers:
///   - global (user-wide): injected once at session start, ranked by importance,
///     no semantic gating — they apply to every task;
///   - scoped (project×role): retrieved by relevance to the current message.
///
/// `first_call` is true for the first injection of the session (full hybrid
/// retrieval + the one-time global tier); follow-up user messages pass false
/// (embedding-only scoped recall, no LLM call). `injected` holds the contents
/// already injected this session — anything in it is skipped to avoid repeats.
///
/// Returns `(block, new_contents)`: the text to inject (empty if nothing new)
/// and the contents that should be recorded as injected by the caller.
pub async fn retrieve_and_format(
	config: &Config,
	user_input: &str,
	role: &str,
	project: &str,
	first_call: bool,
	injected: &std::collections::HashSet<String>,
	operation_rx: tokio::sync::watch::Receiver<bool>,
) -> (String, Vec<String>) {
	let learning = &config.supervisor.learning;
	if !learning.enabled {
		return (String::new(), Vec::new());
	}

	let backend = create_backend(learning);
	crate::log_debug!(
		"Learning retrieval: backend={}, role={}, project={}, first_call={}",
		learning.backend,
		role,
		project,
		first_call
	);

	let mut candidates: Vec<crate::supervisor::learning::Lesson> = Vec::new();

	// Global tier: durable user-wide preferences. Always relevant, so injected
	// by importance with no semantic query — but only once per session (first
	// call); afterwards they are already recorded in `injected`.
	if first_call {
		match backend.retrieve_global(config).await {
			Ok(g) => candidates.extend(g.into_iter().take(learning.max_inject)),
			Err(e) => crate::log_debug!("Learning: global retrieve failed: {}", e),
		}
	}

	// Scoped tier: contextual lessons retrieved by relevance to this message.
	// First call uses the full hybrid (LLM keywords + embedding); follow-up
	// messages skip the LLM call and use embedding-only recall — free and fast.
	let patterns = if first_call {
		prepare_retrieval_query(
			config,
			user_input,
			&learning.backend,
			&learning.model,
			operation_rx,
		)
		.await
		.unwrap_or_else(|e| {
			crate::log_debug!("Learning retrieval prep failed: {}", e);
			Vec::new()
		})
	} else {
		Vec::new()
	};
	match backend
		.retrieve(
			user_input,
			&patterns,
			role,
			project,
			learning.max_inject,
			config,
		)
		.await
	{
		Ok(s) => candidates.extend(s),
		Err(e) => crate::log_debug!("Learning: scoped retrieve failed: {}", e),
	}

	// Dedup: skip lessons already injected this session and any repeats within
	// this batch (global + scoped can overlap). Identity is the lesson content.
	// Orientation entries (memory_type = "orientation") are split into their own
	// block — injected as working assumptions to verify, never as truth.
	let mut new_contents = Vec::new();
	let mut lesson_block = String::new();
	let mut orient_block = String::new();
	let mut orient_count = 0usize;
	let orient_enabled = config.supervisor.orientation.enabled;
	let orient_max = config.supervisor.orientation.max_inject;
	let mut batch_seen = std::collections::HashSet::new();
	for lesson in &candidates {
		if injected.contains(&lesson.content) || !batch_seen.insert(lesson.content.as_str()) {
			continue;
		}
		if lesson.memory_type == "orientation" {
			if !orient_enabled || orient_count >= orient_max {
				continue;
			}
			orient_block.push_str(&format!("- {}\n", lesson.content));
			orient_count += 1;
		} else {
			lesson_block.push_str(&format!("- [{}] {}\n", lesson.confidence, lesson.content));
		}
		new_contents.push(lesson.content.clone());
	}

	if lesson_block.is_empty() && orient_block.is_empty() {
		crate::log_debug!("Learning retrieval: nothing new to inject");
		return (String::new(), Vec::new());
	}
	crate::log_debug!(
		"Learning retrieval: injecting {} item(s)",
		new_contents.len()
	);

	// Wrap recall in semantic XML so the model can clearly delimit it: lessons are
	// rules; orientation is working assumptions to verify, never truth.
	let mut inner = String::new();
	if !lesson_block.is_empty() {
		inner.push_str("<lessons>\n");
		inner.push_str(&lesson_block);
		inner.push_str("</lessons>\n");
	}
	if !orient_block.is_empty() {
		inner.push_str(
			"<orientation hint=\"working assumptions — verify before relying on them\">\n",
		);
		inner.push_str(&orient_block);
		inner.push_str("</orientation>\n");
	}
	let out = format!("\n\n<recall>\n{inner}</recall>");
	(out, new_contents)
}

/// Call LLM to prepare retrieval patterns/query based on backend type.
async fn prepare_retrieval_query(
	config: &Config,
	user_input: &str,
	backend_type: &str,
	model: &str,
	operation_rx: tokio::sync::watch::Receiver<bool>,
) -> Result<Vec<String>> {
	let system = match backend_type {
		"mcp" => MCP_RETRIEVAL_PROMPT,
		_ => FILE_RETRIEVAL_PROMPT,
	};

	let response = super::extract::call_learning_llm(
		config,
		model,
		system.to_string(),
		user_input.to_string(),
		operation_rx,
	)
	.await?;

	let patterns: Vec<String> = response
		.lines()
		.map(|l| l.trim().to_string())
		.filter(|l| !l.is_empty())
		.collect();

	Ok(patterns)
}
