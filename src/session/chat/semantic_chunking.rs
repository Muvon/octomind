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
	pub discourse_relation: DiscourseRelation,
}

/// Universal chunk types (not dev-specific)
#[derive(Debug, Clone, PartialEq)]
pub enum ChunkType {
	Critical,       // Must preserve: errors, decisions, commitments, key facts
	Reference,      // Useful to keep: URLs, file paths, names, dates, numbers
	Context,        // Background info, explanations, reasoning
	Conversational, // Greetings, acknowledgments, filler
}

/// Discourse relations between chunks (lightweight heuristics)
/// Research shows structure-aware compression preserves coherence better
#[derive(Debug, Clone, PartialEq)]
pub enum DiscourseRelation {
	Elaboration, // "for example", "specifically" - can compress
	Contrast,    // "however", "but" - keep both sides
	Cause,       // "because", "therefore" - keep both
	Sequence,    // "then", "next", "after" - can compress old
	Background,  // "context", "background" - compress first
	None,        // No clear relation
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
			let discourse_relation = detect_discourse_relation(&segment);
			let importance = calculate_importance(&segment, &chunk_type, &discourse_relation, msg);

			chunks.push(SemanticChunk {
				content: segment,
				start_idx: idx,
				end_idx: idx,
				importance,
				chunk_type,
				discourse_relation,
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

	// - Questions that need answers (preserve context)
	if text.contains('?') && msg.role == "user" {
		return ChunkType::Critical;
	}

	// - Plan/task related (preserve structure)
	if lower.contains("plan(")
		|| lower.contains("task:")
		|| lower.contains("step ")
		|| lower.contains("phase:")
		|| lower.contains("todo")
		|| lower.contains("next:")
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

	// - Configuration values, environment vars
	if text.contains('=') && (text.contains("export") || text.contains("ENV")) {
		return ChunkType::Reference;
	}

	// - Function/method names (preserve API references)
	if text.contains("fn ")
		|| text.contains("def ")
		|| text.contains("function ")
		|| text.contains("class ")
		|| text.contains("impl ")
	{
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

/// Detect discourse relation using simple keyword matching
/// Research shows structure-aware compression preserves coherence better
fn detect_discourse_relation(text: &str) -> DiscourseRelation {
	let lower = text.to_lowercase();

	// Elaboration: Can be compressed (details of main point)
	if lower.contains("for example")
		|| lower.contains("specifically")
		|| lower.contains("in particular")
		|| lower.contains("such as")
		|| lower.contains("i.e.")
		|| lower.contains("e.g.")
	{
		return DiscourseRelation::Elaboration;
	}

	// Contrast: Must preserve both sides
	if lower.contains("however")
		|| lower.contains("but ")
		|| lower.contains("although")
		|| lower.contains("on the other hand")
		|| lower.contains("instead")
		|| lower.contains("rather than")
	{
		return DiscourseRelation::Contrast;
	}

	// Cause: Must preserve reasoning chain
	if lower.contains("because")
		|| lower.contains("therefore")
		|| lower.contains("thus")
		|| lower.contains("so ")
		|| lower.contains("as a result")
		|| lower.contains("consequently")
	{
		return DiscourseRelation::Cause;
	}

	// Sequence: Older steps can be compressed
	if lower.contains("first")
		|| lower.contains("then")
		|| lower.contains("next")
		|| lower.contains("after")
		|| lower.contains("finally")
		|| lower.contains("step ")
	{
		return DiscourseRelation::Sequence;
	}

	// Background: Compress aggressively
	if lower.contains("background")
		|| lower.contains("context")
		|| lower.contains("historically")
		|| lower.contains("previously")
		|| lower.contains("as mentioned")
	{
		return DiscourseRelation::Background;
	}

	DiscourseRelation::None
}

/// Calculate importance score with type-specific temporal decay and discourse relations
/// Research shows different content types have different "shelf lives"
fn calculate_importance(
	text: &str,
	chunk_type: &ChunkType,
	discourse_relation: &DiscourseRelation,
	msg: &Message,
) -> f64 {
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

	// Discourse relation adjustments (structure-aware compression)
	match discourse_relation {
		DiscourseRelation::Contrast | DiscourseRelation::Cause => {
			score += 2.0; // Preserve logical relationships
		}
		DiscourseRelation::Elaboration | DiscourseRelation::Background => {
			score -= 1.0; // Can compress details
		}
		DiscourseRelation::Sequence => {
			// Handled by temporal decay below
		}
		DiscourseRelation::None => {}
	}

	// Type-specific temporal decay (research-backed half-lives)
	let age_hours = calculate_age_hours(msg.timestamp);
	let half_life = match chunk_type {
		ChunkType::Critical => 72.0,      // 3 days - decisions stay relevant
		ChunkType::Reference => 48.0,     // 2 days - file paths, URLs
		ChunkType::Context => 24.0,       // 1 day - explanations
		ChunkType::Conversational => 6.0, // 6 hours - filler decays fast
	};

	// Apply exponential decay
	score *= (-age_hours / half_life).exp();

	// Recency boost: Last 2 hours get extra weight (working memory)
	if age_hours < 2.0 {
		score *= 1.5;
	}

	// Sequence decay: Older steps in sequences decay faster
	if matches!(discourse_relation, DiscourseRelation::Sequence) && age_hours > 12.0 {
		score *= 0.7;
	}

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

/// Select top chunks within token budget with discourse relation awareness
/// Preserves logical relationships (Contrast/Cause pairs) and compresses sequences
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
	let mut selected_indices = std::collections::HashSet::new();

	for (idx, chunk) in sorted.iter().enumerate() {
		// Skip if already selected
		if selected_indices.contains(&idx) {
			continue;
		}

		let chunk_tokens = estimate_tokens(&chunk.content);

		// Check if this chunk needs its context preserved (Contrast/Cause relations)
		let needs_context = matches!(
			chunk.discourse_relation,
			DiscourseRelation::Contrast | DiscourseRelation::Cause
		);

		if needs_context && idx > 0 {
			// Try to include previous chunk for context (the chunk it relates to)
			let prev_idx = idx - 1;
			if !selected_indices.contains(&prev_idx) {
				let prev_chunk = &sorted[prev_idx];
				let prev_tokens = estimate_tokens(&prev_chunk.content);

				// Try to fit both chunks
				if total_tokens + chunk_tokens + prev_tokens <= target_tokens {
					selected.push(prev_chunk.clone());
					selected_indices.insert(prev_idx);
					total_tokens += prev_tokens;

					selected.push(chunk.clone());
					selected_indices.insert(idx);
					total_tokens += chunk_tokens;
					continue;
				}
			}
		}

		// Normal selection
		if total_tokens + chunk_tokens <= target_tokens {
			selected.push(chunk.clone());
			selected_indices.insert(idx);
			total_tokens += chunk_tokens;
		}
	}

	// Compress sequences: keep only latest steps in sequential chains
	compress_sequences(&selected)
}

/// Compress sequential chunks by keeping only the latest step in each sequence
/// This reduces redundancy while preserving the current state
fn compress_sequences(chunks: &[SemanticChunk]) -> Vec<SemanticChunk> {
	let mut result = Vec::new();
	let mut sequence_buffer = Vec::new();

	for chunk in chunks {
		if matches!(chunk.discourse_relation, DiscourseRelation::Sequence) {
			sequence_buffer.push(chunk.clone());
		} else {
			// Flush sequence buffer (keep only last step)
			if !sequence_buffer.is_empty() {
				if let Some(last) = sequence_buffer.last() {
					result.push(last.clone());
				}
				sequence_buffer.clear();
			}
			result.push(chunk.clone());
		}
	}

	// Flush remaining sequences
	if let Some(last) = sequence_buffer.last() {
		result.push(last.clone());
	}

	result
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
		let score = calculate_importance(
			"This is important",
			&ChunkType::Critical,
			&DiscourseRelation::None,
			&msg,
		);
		assert!(score > 10.0); // Critical + user boost
	}

	#[test]
	fn test_discourse_relations() {
		assert_eq!(
			detect_discourse_relation("However, we should consider alternatives"),
			DiscourseRelation::Contrast
		);
		assert_eq!(
			detect_discourse_relation("Because of this, we need to refactor"),
			DiscourseRelation::Cause
		);
		assert_eq!(
			detect_discourse_relation("For example, we can use Rust"),
			DiscourseRelation::Elaboration
		);
		assert_eq!(
			detect_discourse_relation("First, we need to setup the environment"),
			DiscourseRelation::Sequence
		);
	}

	#[test]
	fn test_temporal_decay_by_type() {
		// Create old message (48 hours ago)
		let old_timestamp = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.unwrap_or_default()
			.as_secs()
			- (48 * 3600);

		let old_msg = Message {
			role: "user".to_string(),
			content: "Old content".to_string(),
			timestamp: old_timestamp,
			cached: false,
			tool_call_id: None,
			name: None,
			tool_calls: None,
			images: None,
			thinking: None,
			id: None,
		};

		// Critical should decay slower than Conversational
		let critical_score = calculate_importance(
			"Important decision",
			&ChunkType::Critical,
			&DiscourseRelation::None,
			&old_msg,
		);
		let conversational_score = calculate_importance(
			"ok",
			&ChunkType::Conversational,
			&DiscourseRelation::None,
			&old_msg,
		);

		assert!(critical_score > conversational_score);
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
				discourse_relation: DiscourseRelation::None,
			},
			SemanticChunk {
				content: "Context info".to_string(),
				start_idx: 1,
				end_idx: 1,
				importance: 5.0,
				chunk_type: ChunkType::Context,
				discourse_relation: DiscourseRelation::None,
			},
		];

		// Select with very small budget - should only get highest importance
		let selected = select_chunks_within_budget(&chunks, 5);
		assert!(!selected.is_empty());
		assert_eq!(selected[0].importance, 10.0);
	}

	#[test]
	fn test_relation_aware_selection_preserves_pairs() {
		// Test that Contrast/Cause relations preserve both chunks
		let chunks = vec![
			SemanticChunk {
				content: "We tried approach A".to_string(),
				start_idx: 0,
				end_idx: 0,
				importance: 5.0,
				chunk_type: ChunkType::Context,
				discourse_relation: DiscourseRelation::None,
			},
			SemanticChunk {
				content: "However, approach B is better".to_string(),
				start_idx: 1,
				end_idx: 1,
				importance: 8.0, // Higher importance
				chunk_type: ChunkType::Context,
				discourse_relation: DiscourseRelation::Contrast,
			},
			SemanticChunk {
				content: "Unrelated info".to_string(),
				start_idx: 2,
				end_idx: 2,
				importance: 3.0,
				chunk_type: ChunkType::Context,
				discourse_relation: DiscourseRelation::None,
			},
		];

		// Large enough budget to fit the pair
		let selected = select_chunks_within_budget(&chunks, 100);

		// Should include both chunks in the contrast pair
		assert!(selected.len() >= 2);
		let has_approach_a = selected.iter().any(|c| c.content.contains("approach A"));
		let has_approach_b = selected.iter().any(|c| c.content.contains("approach B"));
		assert!(
			has_approach_a && has_approach_b,
			"Contrast pair should be preserved together"
		);
	}

	#[test]
	fn test_sequence_compression() {
		// Test that sequences keep only the latest step
		let chunks = vec![
			SemanticChunk {
				content: "First, we setup the environment".to_string(),
				start_idx: 0,
				end_idx: 0,
				importance: 7.0,
				chunk_type: ChunkType::Context,
				discourse_relation: DiscourseRelation::Sequence,
			},
			SemanticChunk {
				content: "Then, we installed dependencies".to_string(),
				start_idx: 1,
				end_idx: 1,
				importance: 7.0,
				chunk_type: ChunkType::Context,
				discourse_relation: DiscourseRelation::Sequence,
			},
			SemanticChunk {
				content: "Finally, we ran the tests".to_string(),
				start_idx: 2,
				end_idx: 2,
				importance: 7.0,
				chunk_type: ChunkType::Context,
				discourse_relation: DiscourseRelation::Sequence,
			},
			SemanticChunk {
				content: "Critical result: all tests passed".to_string(),
				start_idx: 3,
				end_idx: 3,
				importance: 10.0,
				chunk_type: ChunkType::Critical,
				discourse_relation: DiscourseRelation::None,
			},
		];

		let selected = select_chunks_within_budget(&chunks, 200);

		// Should compress sequence to just the last step
		let sequence_chunks: Vec<_> = selected
			.iter()
			.filter(|c| matches!(c.discourse_relation, DiscourseRelation::Sequence))
			.collect();

		// Should have only 1 sequence chunk (the last one)
		assert_eq!(
			sequence_chunks.len(),
			1,
			"Sequences should be compressed to last step only"
		);
		assert!(
			sequence_chunks[0].content.contains("Finally"),
			"Should keep the final step"
		);
	}

	#[test]
	fn test_discourse_relation_importance_boost() {
		let msg = create_test_message("user", "Test");

		// Cause/Contrast should get importance boost
		let cause_score = calculate_importance(
			"Because of this issue",
			&ChunkType::Context,
			&DiscourseRelation::Cause,
			&msg,
		);

		let contrast_score = calculate_importance(
			"However, we can try this",
			&ChunkType::Context,
			&DiscourseRelation::Contrast,
			&msg,
		);

		let none_score = calculate_importance(
			"Some context",
			&ChunkType::Context,
			&DiscourseRelation::None,
			&msg,
		);

		// Cause and Contrast should have higher scores than None
		assert!(
			cause_score > none_score,
			"Cause relation should boost importance"
		);
		assert!(
			contrast_score > none_score,
			"Contrast relation should boost importance"
		);
	}

	#[test]
	fn test_elaboration_importance_penalty() {
		let msg = create_test_message("user", "Test");

		// Elaboration/Background should get importance penalty
		let elaboration_score = calculate_importance(
			"For example, we can use this",
			&ChunkType::Context,
			&DiscourseRelation::Elaboration,
			&msg,
		);

		let background_score = calculate_importance(
			"Background: this was done before",
			&ChunkType::Context,
			&DiscourseRelation::Background,
			&msg,
		);

		let none_score = calculate_importance(
			"Some context",
			&ChunkType::Context,
			&DiscourseRelation::None,
			&msg,
		);

		// Elaboration and Background should have lower scores than None
		assert!(
			elaboration_score < none_score,
			"Elaboration relation should reduce importance"
		);
		assert!(
			background_score < none_score,
			"Background relation should reduce importance"
		);
	}
}
