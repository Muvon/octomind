// Copyright 2025 Muvon Un Limited
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

// Context truncation functionality to manage token usage

use crate::config::Config;
use crate::log_conditional;
use crate::session::chat::session::ChatSession;
use crate::session::SmartSummarizer;
use anyhow::Result;
use colored::Colorize;
use regex::Regex;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// Message importance scoring for smart truncation
#[derive(Debug, Clone)]
struct MessageImportance {
	total_score: f64, // Combined weighted score
}

impl MessageImportance {
	fn calculate(msg: &crate::session::Message, position: usize, total_messages: usize) -> Self {
		let recency_score = 1.0 - (position as f64 / total_messages as f64);
		let content_type_score = Self::analyze_content_type(&msg.content);
		let reference_score = Self::analyze_references(&msg.content);
		let tool_result_score = Self::analyze_tool_results(msg);
		let file_reference_score = Self::analyze_file_references(&msg.content);

		// Weighted combination - recency is important but not everything
		let total_score = (recency_score * 0.3)
			+ (content_type_score * 0.25)
			+ (reference_score * 0.15)
			+ (tool_result_score * 0.15)
			+ (file_reference_score * 0.15);

		Self { total_score }
	}

	fn analyze_content_type(content: &str) -> f64 {
		let content_lower = content.to_lowercase();

		// High value content indicators
		if content_lower.contains("error")
			|| content_lower.contains("failed")
			|| content_lower.contains("issue")
			|| content_lower.contains("problem")
		{
			return 0.9; // Errors are very important
		}

		if content_lower.contains("solution")
			|| content_lower.contains("fix")
			|| content_lower.contains("resolved")
			|| content_lower.contains("working")
		{
			return 0.85; // Solutions are very important
		}

		if content_lower.contains("decision")
			|| content_lower.contains("choose")
			|| content_lower.contains("implement")
			|| content_lower.contains("approach")
		{
			return 0.8; // Decisions are important
		}

		if content_lower.contains("created")
			|| content_lower.contains("modified")
			|| content_lower.contains("updated")
			|| content_lower.contains("added")
		{
			return 0.75; // File modifications are important
		}

		// Code-related content
		if content.contains("```") || content.contains("fn ") || content.contains("struct ") {
			return 0.7; // Code is moderately important
		}

		0.5 // Default baseline
	}

	fn analyze_references(content: &str) -> f64 {
		// Simple heuristic: content that defines or explains concepts
		let content_lower = content.to_lowercase();

		if content_lower.contains("let me")
			|| content_lower.contains("i'll")
			|| content_lower.contains("here's")
			|| content_lower.contains("this is")
		{
			return 0.7; // Explanatory content
		}

		if content_lower.contains("remember")
			|| content_lower.contains("important")
			|| content_lower.contains("note that")
			|| content_lower.contains("keep in mind")
		{
			return 0.8; // Explicitly marked as important
		}

		0.5 // Default
	}

	fn analyze_tool_results(msg: &crate::session::Message) -> f64 {
		if msg.role != "tool" {
			return 0.5; // Not a tool message
		}

		let content = &msg.content;

		// High-value tool results
		if content.contains("Error:") || content.contains("Failed") {
			return 0.9; // Error outputs are critical
		}

		if content.contains("Successfully") || content.contains("Finished") {
			return 0.7; // Success confirmations are valuable
		}

		// Detect verbose outputs that can be compressed
		if content.len() > 2000 {
			return 0.3; // Very long outputs are often verbose
		}

		if content.lines().count() > 50 {
			return 0.4; // Many lines often indicate verbose output
		}

		0.6 // Default tool result value
	}

	fn analyze_file_references(content: &str) -> f64 {
		let file_patterns = [
			r"src/[^/\s]+\.rs",
			r"[^/\s]+\.toml",
			r"[^/\s]+\.json",
			r"[^/\s]+\.yaml",
			r"[^/\s]+\.md",
			r"/[^/\s]+/[^/\s]+",
		];

		for pattern in &file_patterns {
			if let Ok(regex) = Regex::new(pattern) {
				if regex.is_match(content) {
					return 0.8; // Contains file references
				}
			}
		}

		0.5 // No file references detected
	}
}

/// Smart content compressor for reducing token usage while preserving meaning
struct ContentCompressor;

impl ContentCompressor {
	/// Compress message content intelligently based on content type
	fn compress_message(
		msg: &crate::session::Message,
		importance: &MessageImportance,
	) -> crate::session::Message {
		let mut compressed_msg = msg.clone();

		// Don't compress high-importance messages as aggressively
		if importance.total_score > 0.7 {
			compressed_msg.content = Self::light_compression(&msg.content);
		} else if importance.total_score > 0.4 {
			compressed_msg.content = Self::medium_compression(&msg.content);
		} else {
			compressed_msg.content = Self::heavy_compression(&msg.content);
		}

		compressed_msg
	}

	fn light_compression(content: &str) -> String {
		// Only compress very verbose parts
		Self::compress_file_paths(content)
	}

	fn medium_compression(content: &str) -> String {
		let mut result = Self::compress_file_paths(content);
		result = Self::compress_code_blocks(&result);
		result = Self::compress_repetitive_patterns(&result);
		result
	}

	fn heavy_compression(content: &str) -> String {
		let mut result = Self::compress_file_paths(content);
		result = Self::compress_code_blocks(&result);
		result = Self::compress_repetitive_patterns(&result);
		result = Self::compress_verbose_outputs(&result);
		result
	}

	fn compress_file_paths(content: &str) -> String {
		// Replace full file paths with references
		let patterns = [
			(r"src/([^/\s]+/)*([^/\s]+\.rs)", "[Rust file: $2]"),
			(r"([^/\s]+\.toml)", "[Config: $1]"),
			(r"([^/\s]+\.json)", "[JSON: $1]"),
			(r"([^/\s]+\.yaml)", "[YAML: $1]"),
			(r"([^/\s]+\.md)", "[Doc: $1]"),
		];

		let mut result = content.to_string();
		for (pattern, replacement) in &patterns {
			if let Ok(regex) = Regex::new(pattern) {
				result = regex.replace_all(&result, *replacement).to_string();
			}
		}
		result
	}

	fn compress_code_blocks(content: &str) -> String {
		// Compress large code blocks to summaries
		if let Ok(regex) = Regex::new(r"```[\s\S]*?```") {
			regex
				.replace_all(content, |caps: &regex::Captures| {
					let block = &caps[0];
					if block.len() > 500 {
						if block.contains("fn ") {
							"[Code block: function definitions]".to_string()
						} else if block.contains("struct ") {
							"[Code block: struct definitions]".to_string()
						} else if block.contains("impl ") {
							"[Code block: implementations]".to_string()
						} else {
							"[Code block: truncated for brevity]".to_string()
						}
					} else {
						block.to_string() // Keep shorter code blocks
					}
				})
				.to_string()
		} else {
			content.to_string()
		}
	}

	fn compress_repetitive_patterns(content: &str) -> String {
		// Compress repetitive patterns like multiple similar lines
		let lines: Vec<&str> = content.lines().collect();
		if lines.len() <= 10 {
			return content.to_string(); // Don't compress short content
		}

		let mut compressed_lines = Vec::new();
		let mut i = 0;

		while i < lines.len() {
			let current_line = lines[i];
			let mut repeat_count = 1;

			// Count consecutive similar lines
			while i + repeat_count < lines.len()
				&& Self::lines_similar(current_line, lines[i + repeat_count])
			{
				repeat_count += 1;
			}

			if repeat_count >= 3 {
				compressed_lines.push(current_line.to_string());
				compressed_lines.push(format!(
					"[... {} similar lines omitted ...]",
					repeat_count - 1
				));
				i += repeat_count;
			} else {
				compressed_lines.push(current_line.to_string());
				i += 1;
			}
		}

		compressed_lines.join("\n")
	}

	fn compress_verbose_outputs(content: &str) -> String {
		// Aggressively compress very long outputs
		if content.len() > 3000 {
			let lines: Vec<&str> = content.lines().collect();
			if lines.len() > 100 {
				let start_lines = &lines[..20];
				let end_lines = &lines[lines.len() - 10..];
				format!(
					"{}\n[... {} lines omitted for brevity ...]\n{}",
					start_lines.join("\n"),
					lines.len() - 30,
					end_lines.join("\n")
				)
			} else {
				// Just truncate very long single lines
				if content.len() > 5000 {
					format!(
						"{}...[truncated {} chars]",
						&content[..2000],
						content.len() - 2000
					)
				} else {
					content.to_string()
				}
			}
		} else {
			content.to_string()
		}
	}

	fn lines_similar(line1: &str, line2: &str) -> bool {
		// Check if two lines are similar enough to be considered repetitive
		let l1 = line1.trim();
		let l2 = line2.trim();

		// Exact match
		if l1 == l2 {
			return true;
		}

		// Similar patterns (same prefix/suffix)
		if l1.len() > 20 && l2.len() > 20 {
			let prefix_len = l1
				.chars()
				.zip(l2.chars())
				.take_while(|(a, b)| a == b)
				.count();
			if prefix_len > l1.len() * 7 / 10 {
				// 70% similarity
				return true;
			}
		}

		false
	}
}

// Perform smart context truncation when token limit is approaching
pub async fn check_and_truncate_context(
	chat_session: &mut ChatSession,
	config: &Config,
	_role: &str,
	_operation_cancelled: Arc<AtomicBool>,
) -> Result<()> {
	// Check if auto truncation is enabled in config
	if !config.enable_auto_truncation {
		return Ok(());
	}

	// Estimate current token usage
	let current_tokens = crate::session::estimate_message_tokens(&chat_session.session.messages);

	// If we're under the threshold, nothing to do
	if current_tokens < config.max_request_tokens_threshold {
		return Ok(());
	}

	// Delegate to the core truncation logic
	perform_smart_truncation(chat_session, config, current_tokens).await
}

// Perform smart context truncation without checking auto-truncation settings
pub async fn perform_smart_truncation(
	chat_session: &mut ChatSession,
	config: &Config,
	current_tokens: usize,
) -> Result<()> {
	// We need to truncate - inform the user with minimal info
	log_conditional!(
		debug: format!("\nℹ️  Message history exceeds configured token limit ({} > {})\nApplying enhanced smart truncation to reduce context size.",
			current_tokens, config.max_request_tokens_threshold).bright_blue(),
		default: "Applying enhanced smart truncation to reduce token usage".bright_blue()
	);

	// ENHANCED SMART TRUNCATION STRATEGY:
	// 1. Always keep system message
	// 2. Calculate importance scores for all messages
	// 3. Apply intelligent content compression before selection
	// 4. Keep recent conversation with complete tool sequences
	// 5. Prioritize high-importance messages regardless of position
	// 6. Preserve file modification context and technical decisions

	let mut system_message = None;
	let mut preserved_messages = Vec::new();

	// Extract system message
	for msg in &chat_session.session.messages {
		if msg.role == "system" {
			system_message = Some(msg.clone());
			break;
		}
	}

	let non_system_messages: Vec<_> = chat_session
		.session
		.messages
		.iter()
		.filter(|msg| msg.role != "system")
		.collect();

	if !non_system_messages.is_empty() {
		// PHASE 1: Calculate importance scores for all messages
		let mut message_scores: Vec<(usize, MessageImportance)> = non_system_messages
			.iter()
			.enumerate()
			.map(|(i, msg)| {
				let importance = MessageImportance::calculate(msg, i, non_system_messages.len());
				(i, importance)
			})
			.collect();

		// PHASE 2: Apply intelligent content compression to reduce token usage
		let mut compressed_messages: Vec<crate::session::Message> = Vec::new();
		let mut compression_savings = 0usize;

		for (i, (_, importance)) in message_scores.iter().enumerate() {
			let original_msg = non_system_messages[i];
			let compressed_msg = ContentCompressor::compress_message(original_msg, importance);

			let original_tokens = crate::session::estimate_tokens(&original_msg.content);
			let compressed_tokens = crate::session::estimate_tokens(&compressed_msg.content);
			compression_savings += original_tokens.saturating_sub(compressed_tokens);

			compressed_messages.push(compressed_msg);
		}

		log_conditional!(
			debug: format!("Content compression saved {} tokens", compression_savings).bright_green(),
			default: format!("Applied content compression, saved {} tokens", compression_savings).bright_green()
		);

		// Update current tokens after compression (for potential future use)
		let _current_tokens_after_compression = current_tokens.saturating_sub(compression_savings);

		// Calculate how many messages we can keep based on token budget
		let system_tokens = system_message
			.as_ref()
			.map(|msg| crate::session::estimate_tokens(&msg.content))
			.unwrap_or(0);

		let available_tokens = config
			.max_request_tokens_threshold
			.saturating_sub(system_tokens);
		let target_tokens = (available_tokens as f64 * 0.85) as usize; // Increased from 80% to 85% due to compression

		// PHASE 3: Smart message selection based on importance and constraints
		// Sort by importance score (descending) while preserving original indices
		message_scores.sort_by(|a, b| {
			b.1.total_score
				.partial_cmp(&a.1.total_score)
				.unwrap_or(std::cmp::Ordering::Equal)
		});

		let mut selected_messages = Vec::new();
		let mut current_token_count = 0usize;
		let mut selected_indices = std::collections::HashSet::new();

		// First pass: Select high-importance messages
		for (original_index, importance) in &message_scores {
			if importance.total_score > 0.7 {
				// High importance threshold
				let msg = &compressed_messages[*original_index];
				let msg_tokens = crate::session::estimate_tokens(&msg.content);

				if current_token_count + msg_tokens <= target_tokens {
					selected_messages.push((*original_index, msg.clone()));
					selected_indices.insert(*original_index);
					current_token_count += msg_tokens;
				}
			}
		}

		// Second pass: Fill remaining space with recent messages and tool sequences
		// Work backwards from most recent, respecting tool sequences
		let mut i = compressed_messages.len();
		let mut incomplete_tool_sequence = false;

		while i > 0 && current_token_count < target_tokens {
			i -= 1;

			if selected_indices.contains(&i) {
				continue; // Already selected in first pass
			}

			let msg = &compressed_messages[i];
			let msg_tokens = crate::session::estimate_tokens(&msg.content);

			// Check if adding this message would exceed our budget
			if current_token_count + msg_tokens > target_tokens && !selected_messages.is_empty() {
				break;
			}

			// Tool sequence preservation logic (enhanced from original)
			match msg.role.as_str() {
				"tool" => {
					// Always include tool messages to avoid breaking sequences
					selected_messages.push((i, msg.clone()));
					selected_indices.insert(i);
					current_token_count += msg_tokens;
					incomplete_tool_sequence = true;
				}
				"assistant" => {
					// Include assistant message
					selected_messages.push((i, msg.clone()));
					selected_indices.insert(i);
					current_token_count += msg_tokens;

					// If this assistant message has tool_calls, we have a complete sequence
					if msg.tool_calls.is_some() && incomplete_tool_sequence {
						incomplete_tool_sequence = false;
					}
				}
				"user" => {
					// User messages are good natural breakpoints
					// Include if we have a complete tool sequence or no tool sequence
					if !incomplete_tool_sequence {
						selected_messages.push((i, msg.clone()));
						selected_indices.insert(i);
						current_token_count += msg_tokens;
					} else {
						// If we have incomplete tool sequence, we need to include this user message too
						// to maintain context, but check token budget
						if current_token_count + msg_tokens <= target_tokens {
							selected_messages.push((i, msg.clone()));
							selected_indices.insert(i);
							current_token_count += msg_tokens;
						} else {
							// Can't fit, but we need to break cleanly
							break;
						}
					}
				}
				_ => {
					// Other message types - include if important or recent enough
					let (_, importance) =
						&message_scores.iter().find(|(idx, _)| *idx == i).unwrap();
					if importance.total_score > 0.5 || i >= compressed_messages.len() - 10 {
						// Recent or important
						if current_token_count + msg_tokens <= target_tokens {
							selected_messages.push((i, msg.clone()));
							selected_indices.insert(i);
							current_token_count += msg_tokens;
						}
					}
				}
			}
		}

		// Sort selected messages by original index to maintain chronological order
		selected_messages.sort_by_key(|(index, _)| *index);
		preserved_messages = selected_messages.into_iter().map(|(_, msg)| msg).collect();

		log_conditional!(
			debug: format!("Enhanced smart truncation: preserving {} of {} messages ({} tokens, {} saved by compression)",
				preserved_messages.len(), non_system_messages.len(), current_token_count, compression_savings).bright_blue(),
			default: format!("Preserving {} recent messages with intelligent compression", preserved_messages.len()).bright_blue()
		);
	}

	// Build the new truncated message list
	let mut truncated_messages = Vec::new();

	// Add system message first if available
	if let Some(sys_msg) = system_message {
		truncated_messages.push(sys_msg);
	}

	// Add context note only if we actually removed messages
	if preserved_messages.len() < non_system_messages.len() {
		let removed_count = non_system_messages.len() - preserved_messages.len();

		// Get the messages that were removed for summarization
		let removed_messages: Vec<_> = non_system_messages
			.iter()
			.take(removed_count)
			.cloned()
			.cloned()
			.collect();

		// Create smart summary of removed messages
		let summarizer = SmartSummarizer::new();
		let removed_summary = match summarizer.summarize_messages(&removed_messages) {
			Ok(summary) => summary,
			Err(e) => {
				log_conditional!(
					debug: format!("Failed to summarize removed messages: {}", e).bright_yellow(),
					default: "Failed to create summary of removed messages".bright_yellow()
				);
				format!("Removed {} older messages", removed_count)
			}
		};

		let context_note = format!(
			"[Smart truncation applied: {} older messages removed and summarized below]\n\n--- Summary of Removed Context ---\n{}\n--- End Summary ---",
			removed_count, removed_summary
		);

		let summary_msg = crate::session::Message {
			role: "assistant".to_string(),
			content: context_note,
			timestamp: std::time::SystemTime::now()
				.duration_since(std::time::UNIX_EPOCH)
				.unwrap_or_default()
				.as_secs(),
			cached: false,
			tool_call_id: None,
			name: None,
			tool_calls: None,
		};
		truncated_messages.push(summary_msg);
	}

	// Add preserved messages
	truncated_messages.extend(preserved_messages);

	// Replace session messages with truncated version
	chat_session.session.messages = truncated_messages;

	// Calculate and report savings
	let new_token_count = crate::session::estimate_message_tokens(&chat_session.session.messages);
	let tokens_saved = current_tokens.saturating_sub(new_token_count);

	log_conditional!(
		debug: format!("Smart truncation complete: {} tokens removed, new context size: {} tokens.",
			tokens_saved, new_token_count).bright_green(),
		default: format!("Reduced context size by {} tokens", tokens_saved).bright_green()
	);

	// Save the session with truncated messages
	chat_session.save()?;

	Ok(())
}

/// Perform smart full context summarization using external crate
/// This replaces the entire conversation with an intelligent summary
pub async fn perform_smart_full_summarization(
	chat_session: &mut ChatSession,
	_config: &Config,
) -> Result<()> {
	log_conditional!(
		debug: "Performing smart full context summarization...".bright_blue(),
		default: "Summarizing conversation...".bright_blue()
	);

	// Extract system message
	let system_message = chat_session
		.session
		.messages
		.iter()
		.find(|m| m.role == "system")
		.cloned();

	// Get all non-system messages for summarization
	let conversation_messages: Vec<_> = chat_session
		.session
		.messages
		.iter()
		.filter(|m| m.role != "system")
		.cloned()
		.collect();

	if conversation_messages.is_empty() {
		log_conditional!(
			debug: "No conversation messages to summarize".bright_yellow(),
			default: "No conversation to summarize".bright_yellow()
		);
		return Ok(());
	}

	// Create smart summary of entire conversation
	let summarizer = SmartSummarizer::new();
	let conversation_summary = match summarizer.summarize_messages(&conversation_messages) {
		Ok(summary) => summary,
		Err(e) => {
			log_conditional!(
				debug: format!("Failed to summarize conversation: {}", e).bright_red(),
				default: "Failed to create conversation summary".bright_red()
			);
			return Err(anyhow::anyhow!("Summarization failed: {}", e));
		}
	};

	// Build new message list with summary
	let mut new_messages = Vec::new();

	// Add system message first if available
	if let Some(sys_msg) = system_message {
		new_messages.push(sys_msg);
	}

	// Add comprehensive summary as assistant message
	let summary_note = format!(
		"--- Conversation Summary ---\n{}\n--- End Summary ---\n\nConversation has been summarized. You can continue from here.",
		conversation_summary
	);

	let summary_msg = crate::session::Message {
		role: "assistant".to_string(),
		content: summary_note,
		timestamp: std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.unwrap_or_default()
			.as_secs(),
		cached: true, // Mark for caching
		tool_call_id: None,
		name: None,
		tool_calls: None,
	};
	new_messages.push(summary_msg);

	// Replace session messages with summarized version
	let original_count = chat_session.session.messages.len();
	chat_session.session.messages = new_messages;

	// Reset token tracking for fresh start
	chat_session.session.current_non_cached_tokens = 0;
	chat_session.session.current_total_tokens = 0;

	// Update cache checkpoint time
	chat_session.session.last_cache_checkpoint_time = std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.unwrap_or_default()
		.as_secs();

	log_conditional!(
		debug: format!("Full summarization complete: {} messages replaced with summary", original_count).bright_green(),
		default: "Conversation summarized successfully".bright_green()
	);

	// Save the updated session
	chat_session.save()?;

	Ok(())
}
