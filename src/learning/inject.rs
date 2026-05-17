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

/// Retrieve relevant lessons and format them for system prompt injection.
///
/// Returns the formatted string to append to the system prompt, or empty string if none.
pub async fn retrieve_and_format(
	config: &Config,
	user_input: &str,
	role: &str,
	project: &str,
	operation_rx: tokio::sync::watch::Receiver<bool>,
) -> String {
	let learning = &config.learning;
	if !learning.enabled {
		return String::new();
	}

	let backend = create_backend(learning);
	crate::log_debug!(
		"Learning retrieval: backend={}, role={}, project={}",
		learning.backend,
		role,
		project
	);

	// Prepare retrieval query via LLM (backend-adaptive)
	let patterns = match prepare_retrieval_query(
		config,
		user_input,
		&learning.backend,
		&learning.model,
		operation_rx,
	)
	.await
	{
		Ok(p) => {
			crate::log_debug!("Learning retrieval patterns: {:?}", p);
			p
		}
		Err(e) => {
			crate::log_debug!("Learning retrieval prep failed: {}", e);
			return String::new();
		}
	};

	// Retrieve from backend — pass both the raw user input (for dense
	// embedding scoring) and the LLM-extracted patterns (for sparse
	// keyword scoring). The file backend fuses both via RRF; the MCP
	// backend hands the patterns to the configured tool.
	let lessons = match backend
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
		Ok(l) => l,
		Err(e) => {
			crate::log_debug!("Learning retrieval failed: {}", e);
			return String::new();
		}
	};

	if lessons.is_empty() {
		crate::log_debug!("Learning retrieval: no matching lessons found");
		return String::new();
	}
	crate::log_debug!(
		"Learning retrieval: {} lessons matched, injecting into context",
		lessons.len()
	);

	// Format for system prompt
	let mut output = String::from("\n\n## Lessons from Past Sessions\n");
	for lesson in &lessons {
		output.push_str(&format!("- [{}] {}\n", lesson.confidence, lesson.content));
	}
	output
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
