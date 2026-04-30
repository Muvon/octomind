// Copyright 2026 Muvon Un Limited
//
// Licensed under the Apache License, Version 2.0 (the "License")

pub mod file;
pub mod mcp;

use super::Lesson;
use crate::config::Config;
use anyhow::Result;
use async_trait::async_trait;

/// Backend trait for learning storage and retrieval.
///
/// File backend: hybrid retrieval — LLM-extracted keywords (sparse) +
/// BGE-small cosine over lesson text (dense), fused via Reciprocal Rank
/// Fusion. Both signals are kept because they have non-overlapping
/// failure modes (keyword catches exact API names / error codes;
/// embedding catches paraphrases and semantic neighbors).
/// MCP backend: calls external tools with field mapping; the MCP server
/// owns its own retrieval semantics.
#[async_trait]
pub trait LearningBackend: Send + Sync {
	/// Store a lesson.
	async fn store(&self, lesson: &Lesson, config: &Config) -> Result<()>;

	/// Retrieve lessons relevant to a user request.
	///
	/// `intent` is the raw user input — used for embedding-based dense
	/// scoring. `patterns` are LLM-extracted keywords/phrases — used for
	/// sparse (substring) scoring on the file backend, or as a single
	/// natural-language query on the MCP backend.
	///
	/// File backend fuses both via Reciprocal Rank Fusion. MCP backend
	/// hands the patterns to the configured tool; intent is informational
	/// (the tool owns its own scoring).
	async fn retrieve(
		&self,
		intent: &str,
		patterns: &[String],
		role: &str,
		project: &str,
		limit: usize,
		config: &Config,
	) -> Result<Vec<Lesson>>;

	/// Retrieve ALL lessons for a role/project (used for dedup during extraction).
	async fn retrieve_all(&self, role: &str, project: &str, config: &Config)
		-> Result<Vec<Lesson>>;
}

/// Create a backend based on config.
pub fn create_backend(config: &super::LearningConfig) -> Box<dyn LearningBackend> {
	match config.backend.as_str() {
		"mcp" => Box::new(mcp::McpBackend::new(config)),
		_ => Box::new(file::FileBackend),
	}
}
