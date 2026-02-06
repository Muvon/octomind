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

//! Semantic chunking for intelligent conversation compression
//!
//! This module provides EDU-inspired semantic chunking with importance scoring
//! using pure heuristics (no ML dependencies). Works for ANY conversation type:
//! development, creative writing, research, planning, general chat.

use crate::session::{estimate_tokens, Message};

/// Semantic chunk with importance score
#[derive(Debug, Clone)]
pub struct SemanticChunk {
	pub content: String,
	#[allow(dead_code)]
	pub start_idx: usize,
	#[allow(dead_code)]
	pub end_idx: usize,
	pub importance: f64,
	pub chunk_type: ChunkType,
}

/// Universal chunk types (not dev-specific)
#[derive(Debug, Clone, PartialEq)]
pub enum ChunkType {
	Critical,       // Must preserve: errors, decisions, commitments, key facts
	Reference,      // Useful to keep: URLs, file paths, names, dates, numbers
	Context,        // Background info, explanations, reasoning
	Conversational, // Greetings, acknowledgments, filler
}

/// Chunk messages into semantic units with importance scoring
pub fn chunk_messages(messages: &[Message]) -> Vec<SemanticChunk> {
	let mut chunks = Vec::new();

	for (idx, msg) in messages.iter().enumerate() {
		// Skip system messages
		if msg.role == "system" {
			continue;
		}

		// Split message by semantic boundaries
		let segments = split_by_boundaries(&msg.content);

		for segment in segments {
			if segment.trim().is_empty() {
				continue;
			}

			let chunk_type = classify_chunk(&segment, msg);
			let importance = calculate_importance(&segment, &chunk_type, msg);

			chunks.push(SemanticChunk {
				content: segment,
				start_idx: idx,
				end_idx: idx,
				importance,
				chunk_type,
			});
		}
	}

	chunks
}

/// Split text by semantic boundaries (paragraphs, code blocks, tool calls)
fn split_by_boundaries(text: &str) -> Vec<String> {
	let mut segments = Vec::new();
	let mut current = String::new();
	let mut in_code_block = false;

	for line in text.lines() {
		// Code block boundaries
		if line.trim().starts_with("```") {
			if !current.is_empty() {
				segments.push(current.clone());
				current.clear();
			}
			in_code_block = !in_code_block;
			current.push_str(line);
			current.push('\n');
			if !in_code_block {
				segments.push(current.clone());
				current.clear();
			}
			continue;
		}

		// Inside code block - keep together
		if in_code_block {
			current.push_str(line);
			current.push('\n');
			continue;
		}

		// Empty line = paragraph boundary
		if line.trim().is_empty() {
			if !current.is_empty() {
				segments.push(current.clone());
				current.clear();
			}
			continue;
		}

		current.push_str(line);
		current.push('\n');
	}

	if !current.is_empty() {
		segments.push(current);
	}

	segments
}

/// Classify chunk type using universal heuristics
fn classify_chunk(text: &str, msg: &Message) -> ChunkType {
	let lower = text.to_lowercase();

	// CRITICAL: Things that must be preserved
	// - Errors and problems
	if text.contains("error")
		|| text.contains("Error")
		|| text.contains("failed")
		|| text.contains("issue")
		|| text.contains("warning")
		|| text.contains("Warning")
	{
		return ChunkType::Critical;
	}

	// - Explicit decisions and commitments
	if lower.contains("decided")
		|| lower.contains("will do")
		|| lower.contains("agreed")
		|| lower.contains("must")
		|| lower.contains("should not")
		|| lower.contains("don't")
		|| lower.contains("won't")
	{
		return ChunkType::Critical;
	}

	// - Tool calls (actions taken)
	if msg.tool_calls.is_some() {
		return ChunkType::Critical;
	}

	// REFERENCE: Concrete facts to preserve
	// - File paths, URLs, specific names
	if text.contains('/') || text.contains("http") || text.contains("www") {
		return ChunkType::Reference;
	}

	// - Code blocks, commands, specific syntax
	if text.contains("```") || text.contains('`') {
		return ChunkType::Reference;
	}

	// - Numbers, dates, versions (significant amount)
	if text.chars().filter(|c| c.is_numeric()).count() > 2 {
		return ChunkType::Reference;
	}

	// CONVERSATIONAL: Filler that can be dropped
	let trimmed = lower.trim();
	if trimmed.starts_with("ok")
		|| trimmed.starts_with("sure")
		|| trimmed.starts_with("thanks")
		|| trimmed.starts_with("great")
		|| trimmed.starts_with("got it")
		|| trimmed.starts_with("understood")
		|| trimmed.starts_with("yes")
		|| trimmed.starts_with("no problem")
	{
		return ChunkType::Conversational;
	}

	// CONTEXT: Everything else (explanations, reasoning)
	ChunkType::Context
}

/// Calculate importance score with temporal decay
fn calculate_importance(text: &str, chunk_type: &ChunkType, msg: &Message) -> f64 {
	let mut score = match chunk_type {
		ChunkType::Critical => 10.0,      // Always preserve
		ChunkType::Reference => 7.0,      // High priority
		ChunkType::Context => 4.0,        // Medium priority
		ChunkType::Conversational => 1.0, // Low priority
	};

	// Boost for tool calls (actions taken)
	if msg.tool_calls.is_some() {
		score += 5.0;
	}

	// Boost for user messages (user intent is critical)
	if msg.role == "user" {
		score += 2.0;
	}

	// Boost for questions (need context for answers)
	if text.contains('?') {
		score += 1.5;
	}

	// Temporal decay (24-hour half-life)
	let age_hours = calculate_age_hours(msg.timestamp);
	score *= (-age_hours / 24.0).exp();

	score
}

/// Calculate message age in hours
fn calculate_age_hours(timestamp: u64) -> f64 {
	let now = std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.unwrap_or_default()
		.as_secs();

	let age_seconds = now.saturating_sub(timestamp);
	age_seconds as f64 / 3600.0
}

/// Select top chunks within token budget
pub fn select_chunks_within_budget(
	chunks: &[SemanticChunk],
	target_tokens: usize,
) -> Vec<SemanticChunk> {
	let mut sorted = chunks.to_vec();
	sorted.sort_by(|a, b| {
		b.importance
			.partial_cmp(&a.importance)
			.unwrap_or(std::cmp::Ordering::Equal)
	});

	let mut selected = Vec::new();
	let mut total_tokens = 0;

	for chunk in sorted {
		let chunk_tokens = estimate_tokens(&chunk.content);
		if total_tokens + chunk_tokens <= target_tokens {
			selected.push(chunk);
			total_tokens += chunk_tokens;
		}
	}

	selected
}

#[cfg(test)]
mod tests {
	use super::*;

	fn create_test_message(role: &str, content: &str) -> Message {
		Message {
			role: role.to_string(),
			content: content.to_string(),
			timestamp: std::time::SystemTime::now()
				.duration_since(std::time::UNIX_EPOCH)
				.unwrap_or_default()
				.as_secs(),
			cached: false,
			tool_call_id: None,
			name: None,
			tool_calls: None,
			images: None,
			thinking: None,
			id: None,
		}
	}

	#[test]
	fn test_classify_critical() {
		let msg = create_test_message("assistant", "Error: file not found");
		assert_eq!(
			classify_chunk("Error: file not found", &msg),
			ChunkType::Critical
		);

		let msg = create_test_message("user", "We decided to use Rust");
		assert_eq!(
			classify_chunk("We decided to use Rust", &msg),
			ChunkType::Critical
		);
	}

	#[test]
	fn test_classify_reference() {
		let msg = create_test_message("assistant", "Check src/main.rs");
		assert_eq!(
			classify_chunk("Check src/main.rs", &msg),
			ChunkType::Reference
		);

		let msg = create_test_message("assistant", "Visit https://example.com");
		assert_eq!(
			classify_chunk("Visit https://example.com", &msg),
			ChunkType::Reference
		);
	}

	#[test]
	fn test_classify_conversational() {
		let msg = create_test_message("user", "ok");
		assert_eq!(classify_chunk("ok", &msg), ChunkType::Conversational);

		let msg = create_test_message("user", "thanks!");
		assert_eq!(classify_chunk("thanks!", &msg), ChunkType::Conversational);
	}

	#[test]
	fn test_importance_scoring() {
		let msg = create_test_message("user", "This is important");
		let score = calculate_importance("This is important", &ChunkType::Critical, &msg);
		assert!(score > 10.0); // Critical + user boost
	}

	#[test]
	fn test_chunk_selection() {
		let chunks = vec![
			SemanticChunk {
				content: "Critical info".to_string(),
				start_idx: 0,
				end_idx: 0,
				importance: 10.0,
				chunk_type: ChunkType::Critical,
			},
			SemanticChunk {
				content: "Context info".to_string(),
				start_idx: 1,
				end_idx: 1,
				importance: 5.0,
				chunk_type: ChunkType::Context,
			},
		];

		// Select with very small budget - should only get highest importance
		let selected = select_chunks_within_budget(&chunks, 5);
		assert!(!selected.is_empty());
		assert_eq!(selected[0].importance, 10.0);
	}
}
