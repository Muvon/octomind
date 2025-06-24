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
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

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

/// Identify a complete conversation unit starting from the given index (working backwards)
/// Returns (messages_in_unit, start_index_of_unit) or None if no complete unit can be formed
fn identify_conversation_unit(
	messages: &[crate::session::Message],
	end_idx: usize,
) -> Option<(Vec<crate::session::Message>, usize)> {
	if end_idx >= messages.len() {
		return None;
	}

	let end_msg = &messages[end_idx];

	match end_msg.role.as_str() {
		"user" => {
			// User message is a complete unit by itself
			Some((vec![end_msg.clone()], end_idx))
		}
		"assistant" => {
			if end_msg.tool_calls.is_some() {
				// Assistant message with tool calls - need to find all corresponding tool results
				identify_tool_sequence_unit(messages, end_idx)
			} else {
				// Simple assistant message is a complete unit by itself
				Some((vec![end_msg.clone()], end_idx))
			}
		}
		"tool" => {
			// Tool messages should be part of a tool sequence, not standalone
			// Skip individual tool messages - they should be picked up as part of assistant sequences
			None
		}
		_ => {
			// Other message types (system, etc.) - treat as individual units
			Some((vec![end_msg.clone()], end_idx))
		}
	}
}

/// Identify a complete tool sequence unit (assistant with tool_calls + all tool results)
fn identify_tool_sequence_unit(
	messages: &[crate::session::Message],
	assistant_idx: usize,
) -> Option<(Vec<crate::session::Message>, usize)> {
	let assistant_msg = &messages[assistant_idx];

	// Verify this is an assistant message with tool_calls
	if assistant_msg.role != "assistant" || assistant_msg.tool_calls.is_none() {
		return None;
	}

	// Extract tool_call_ids from the assistant message
	let mut expected_tool_call_ids = Vec::new();
	if let Some(tool_calls_value) = &assistant_msg.tool_calls {
		if let Some(tool_calls_array) = tool_calls_value.as_array() {
			for tool_call in tool_calls_array {
				if let Some(id) = tool_call.get("id").and_then(|v| v.as_str()) {
					expected_tool_call_ids.push(id.to_string());
				}
			}
		}
	}

	if expected_tool_call_ids.is_empty() {
		// Assistant message with tool_calls but no valid IDs - treat as simple assistant message
		return Some((vec![assistant_msg.clone()], assistant_idx));
	}

	// Find all tool messages that belong to this assistant message
	let mut tool_messages = Vec::new();
	let mut found_tool_call_ids = std::collections::HashSet::new();

	// Look forward from the assistant message to find tool results
	for (i, msg) in messages.iter().enumerate().skip(assistant_idx + 1) {
		if msg.role == "tool" {
			if let Some(tool_call_id) = &msg.tool_call_id {
				if expected_tool_call_ids.contains(tool_call_id) {
					tool_messages.push((i, msg.clone()));
					found_tool_call_ids.insert(tool_call_id.clone());
				}
			}
		} else if msg.role == "user" || msg.role == "assistant" {
			// Stop at the next user or assistant message
			break;
		}
	}

	// Check if we found all expected tool results
	let all_tool_results_found = expected_tool_call_ids
		.iter()
		.all(|id| found_tool_call_ids.contains(id));

	if !all_tool_results_found {
		// Incomplete tool sequence - don't include it as a unit
		// This prevents breaking partial tool sequences
		return None;
	}

	// Build the complete tool sequence unit
	let mut unit_messages = vec![assistant_msg.clone()];
	unit_messages.extend(tool_messages.into_iter().map(|(_, msg)| msg));

	Some((unit_messages, assistant_idx))
}

// Perform smart context truncation without checking auto-truncation settings
pub async fn perform_smart_truncation(
	chat_session: &mut ChatSession,
	config: &Config,
	current_tokens: usize,
) -> Result<()> {
	// Basic validation
	if chat_session.session.messages.is_empty() {
		return Ok(()); // Nothing to truncate
	}

	log_conditional!(
		debug: format!("\nℹ️  Message history exceeds configured token limit ({} > {})\nApplying enhanced safe boundary truncation with intelligent summarization.",
			current_tokens, config.max_request_tokens_threshold).bright_blue(),
		default: "Applying enhanced safe boundary truncation with intelligent summarization".bright_blue()
	);

	// SAFE BOUNDARY TRUNCATION STRATEGY:
	// 1. Always preserve system message
	// 2. Identify complete conversation units (user messages, assistant responses, tool sequences)
	// 3. Work backwards from most recent, keeping complete units
	// 4. Truncate only at safe boundaries between complete conversation exchanges

	// Step 1: Extract system message
	let mut system_message = None;
	let mut non_system_messages = Vec::new();

	for msg in &chat_session.session.messages {
		if msg.role == "system" {
			system_message = Some(msg.clone());
		} else {
			non_system_messages.push(msg.clone());
		}
	}

	if non_system_messages.is_empty() {
		return Ok(()); // Only system message, nothing to truncate
	}

	// Step 2: Calculate available token budget
	let system_tokens = system_message
		.as_ref()
		.map(|msg| crate::session::estimate_tokens(&msg.content))
		.unwrap_or(0);

	let available_tokens = config
		.max_request_tokens_threshold
		.saturating_sub(system_tokens);
	let target_tokens = (available_tokens as f64 * 0.85) as usize; // 85% of available tokens

	// Step 3: Identify conversation units by working backwards
	let mut conversation_units = Vec::new();
	let mut i = non_system_messages.len();

	while i > 0 {
		// Look for complete conversation units working backwards
		let unit = identify_conversation_unit(&non_system_messages, i - 1);

		if let Some((unit_messages, start_idx)) = unit {
			conversation_units.push((unit_messages, start_idx));
			i = start_idx; // Move to the message before this unit
		} else {
			i -= 1; // Skip this message if it can't form a complete unit
		}
	}

	// Reverse to get chronological order
	conversation_units.reverse();

	// Step 4: Select complete units that fit within token budget
	let mut selected_units = Vec::new();
	let mut current_token_count = 0usize;

	// Always try to keep the most recent units
	for (unit_messages, start_idx) in conversation_units.iter().rev() {
		let unit_tokens: usize = unit_messages
			.iter()
			.map(|msg| crate::session::estimate_tokens(&msg.content))
			.sum();

		if current_token_count + unit_tokens <= target_tokens {
			// This unit fits, add it
			selected_units.push((unit_messages.clone(), *start_idx));
			current_token_count += unit_tokens;
		} else {
			// This unit doesn't fit, we found our truncation boundary
			break;
		}
	}

	// Reverse selected units to maintain chronological order (oldest first)
	selected_units.reverse();

	// Build final selected messages in correct chronological order
	let mut selected_messages = Vec::new();
	for (unit_messages, _) in selected_units {
		selected_messages.extend(unit_messages);
	}

	// Messages are already in correct chronological order from unit processing
	// No need to re-sort as it can break tool sequence ordering

	// Step 5: Build the new truncated message list
	let mut truncated_messages = Vec::new();

	// Add system message first if available
	if let Some(sys_msg) = system_message {
		truncated_messages.push(sys_msg);
	}

	// Add intelligent summary if we removed messages
	if selected_messages.len() < non_system_messages.len() {
		let removed_count = non_system_messages.len() - selected_messages.len();

		// Collect removed messages for summarization
		let removed_messages: Vec<crate::session::Message> = non_system_messages
			.iter()
			.filter(|msg| {
				// Check if this message was NOT selected (i.e., it was removed)
				!selected_messages.iter().any(|selected| {
					selected.content == msg.content
						&& selected.role == msg.role
						&& selected.timestamp == msg.timestamp
				})
			})
			.cloned()
			.collect();

		// Generate intelligent summary of removed content
		let summary_content = if !removed_messages.is_empty() {
			let summarizer = SmartSummarizer::new();
			match summarizer.summarize_messages(&removed_messages) {
				Ok(summary) if !summary.trim().is_empty() => {
					format!(
						"📋 **Context Summary** (Removed {} messages at safe boundary)\n\n{}\n\n---\nTruncation preserved complete conversation exchanges to maintain tool sequence integrity.",
						removed_count, summary
					)
				}
				_ => {
					// Fallback to simple message if summarization fails or is empty
					format!(
						"📋 **Context Truncated** (Removed {} messages at safe boundary)\n\nTruncation preserved complete conversation exchanges to maintain tool sequence integrity.",
						removed_count
					)
				}
			}
		} else {
			// Fallback if no removed messages to summarize
			format!(
				"📋 **Context Truncated** (Removed {} messages at safe boundary)\n\nTruncation preserved complete conversation exchanges to maintain tool sequence integrity.",
				removed_count
			)
		};

		let summary_message = crate::session::Message {
			role: "assistant".to_string(),
			content: summary_content,
			timestamp: std::time::SystemTime::now()
				.duration_since(std::time::UNIX_EPOCH)
				.unwrap_or_default()
				.as_secs(),
			cached: false,
			tool_calls: None,
			tool_call_id: None,
			name: None,
			images: None,
		};

		truncated_messages.push(summary_message);
	}

	// Add selected messages
	truncated_messages.extend(selected_messages);

	// Step 6: Update session and report results
	chat_session.session.messages = truncated_messages;

	let new_token_count = crate::session::estimate_message_tokens(&chat_session.session.messages);
	let tokens_saved = current_tokens.saturating_sub(new_token_count);

	log_conditional!(
		debug: format!("Enhanced safe boundary truncation complete: {} tokens removed, new context size: {} tokens.",
			tokens_saved, new_token_count).bright_green(),
		default: format!("Reduced context size by {} tokens with intelligent summarization", tokens_saved).bright_green()
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
		images: None,
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

#[cfg(test)]
mod tests {
	use crate::session::Message;
	use serde_json::json;

	fn create_test_message(
		role: &str,
		content: &str,
		tool_calls: Option<serde_json::Value>,
		tool_call_id: Option<String>,
		name: Option<String>,
	) -> Message {
		Message {
			role: role.to_string(),
			content: content.to_string(),
			timestamp: 0,
			cached: false,
			tool_call_id,
			name,
			tool_calls,
			images: None,
		}
	}

	#[test]
	fn test_tool_sequence_identification() {
		let messages = vec![
			create_test_message("user", "Hello", None, None, None),
			create_test_message(
				"assistant",
				"I'll help you",
				Some(
					json!([{"id": "call_123", "type": "function", "function": {"name": "test_tool"}}]),
				),
				None,
				None,
			),
			create_test_message(
				"tool",
				"Tool result 1",
				None,
				Some("call_123".to_string()),
				Some("test_tool".to_string()),
			),
			create_test_message("assistant", "Based on the result...", None, None, None),
			create_test_message(
				"assistant",
				"Let me use another tool",
				Some(
					json!([{"id": "call_456", "type": "function", "function": {"name": "another_tool"}}]),
				),
				None,
				None,
			),
			create_test_message(
				"tool",
				"Tool result 2",
				None,
				Some("call_456".to_string()),
				Some("another_tool".to_string()),
			),
		];

		// Build tool call map
		let mut tool_call_map: std::collections::HashMap<String, usize> =
			std::collections::HashMap::new();
		for (i, msg) in messages.iter().enumerate() {
			if msg.role == "assistant" && msg.tool_calls.is_some() {
				if let Some(tool_calls_value) = &msg.tool_calls {
					if let Some(tool_calls_array) = tool_calls_value.as_array() {
						for tool_call in tool_calls_array {
							if let Some(id) = tool_call.get("id").and_then(|v| v.as_str()) {
								tool_call_map.insert(id.to_string(), i);
							}
						}
					}
				}
			}
		}

		// Verify tool call mapping
		assert_eq!(tool_call_map.get("call_123"), Some(&1));
		assert_eq!(tool_call_map.get("call_456"), Some(&4));

		// Build tool sequences
		let mut tool_sequences: Vec<(Vec<usize>, usize)> = Vec::new();
		let mut processed_assistants: std::collections::HashSet<usize> =
			std::collections::HashSet::new();

		for (i, msg) in messages.iter().enumerate() {
			if msg.role == "assistant"
				&& msg.tool_calls.is_some()
				&& !processed_assistants.contains(&i)
			{
				let mut sequence_indices = vec![i];
				processed_assistants.insert(i);

				// Find all tool messages that belong to this assistant's tool calls
				for (j, tool_msg) in messages.iter().enumerate() {
					if tool_msg.role == "tool" {
						if let Some(tool_call_id) = &tool_msg.tool_call_id {
							if tool_call_map.get(tool_call_id) == Some(&i) {
								sequence_indices.push(j);
							}
						}
					}
				}

				sequence_indices.sort();
				tool_sequences.push((sequence_indices, 0)); // Token count not important for this test
			}
		}

		// Verify sequences
		assert_eq!(tool_sequences.len(), 2);
		assert_eq!(tool_sequences[0].0, vec![1, 2]); // Assistant at index 1, tool at index 2
		assert_eq!(tool_sequences[1].0, vec![4, 5]); // Assistant at index 4, tool at index 5
	}

	#[test]
	fn test_partial_tool_results_removal() {
		let mut messages = vec![
			create_test_message(
				"assistant",
				"I'll use multiple tools",
				Some(json!([
					{"id": "call_123", "type": "function", "function": {"name": "tool1"}},
					{"id": "call_456", "type": "function", "function": {"name": "tool2"}}
				])),
				None,
				None,
			),
			create_test_message(
				"tool",
				"Tool result 1",
				None,
				Some("call_123".to_string()),
				Some("tool1".to_string()),
			),
			// Missing tool result for call_456 - this should cause assistant message removal
		];

		// Build preserved tool call map
		let mut preserved_tool_call_map: std::collections::HashMap<String, bool> =
			std::collections::HashMap::new();
		for msg in &messages {
			if msg.role == "assistant" && msg.tool_calls.is_some() {
				if let Some(tool_calls_value) = &msg.tool_calls {
					if let Some(tool_calls_array) = tool_calls_value.as_array() {
						for tool_call in tool_calls_array {
							if let Some(id) = tool_call.get("id").and_then(|v| v.as_str()) {
								preserved_tool_call_map.insert(id.to_string(), true);
							}
						}
					}
				}
			}
		}

		// Remove assistant messages with incomplete tool results
		let mut i = 0;
		while i < messages.len() {
			let msg = &messages[i];

			if msg.role == "assistant" && msg.tool_calls.is_some() {
				let mut all_tool_results_present = true;

				// Check if ALL tool results for this assistant message are preserved
				if let Some(tool_calls_value) = &msg.tool_calls {
					if let Some(tool_calls_array) = tool_calls_value.as_array() {
						for tool_call in tool_calls_array {
							if let Some(id) = tool_call.get("id").and_then(|v| v.as_str()) {
								// Look for tool messages with this tool_call_id
								let mut found_tool_result = false;
								for tool_msg in &messages {
									if tool_msg.role == "tool"
										&& tool_msg.tool_call_id.as_ref() == Some(&id.to_string())
									{
										found_tool_result = true;
										break;
									}
								}
								// If any tool call doesn't have its result, mark as incomplete
								if !found_tool_result {
									all_tool_results_present = false;
									break;
								}
							}
						}
					}
				}

				// If this assistant message has tool_calls but ANY tool results are missing, remove it
				if !all_tool_results_present {
					messages.remove(i);
					continue; // Don't increment i since we removed an element
				}
			}
			i += 1;
		}

		// Should have removed the assistant message because call_456 has no result
		// Only the orphaned tool result for call_123 should remain
		assert_eq!(messages.len(), 1);
		assert_eq!(messages[0].role, "tool");
		assert_eq!(messages[0].tool_call_id, Some("call_123".to_string()));
	}

	#[test]
	fn test_orphan_detection() {
		let mut messages = vec![
			create_test_message(
				"assistant",
				"I'll help you",
				Some(
					json!([{"id": "call_123", "type": "function", "function": {"name": "test_tool"}}]),
				),
				None,
				None,
			),
			create_test_message(
				"tool",
				"Tool result 1",
				None,
				Some("call_123".to_string()),
				Some("test_tool".to_string()),
			),
			create_test_message(
				"tool",
				"Orphaned tool result",
				None,
				Some("call_999".to_string()),
				Some("missing_tool".to_string()),
			), // This should be removed
		];

		// Build preserved tool call map
		let mut preserved_tool_call_map: std::collections::HashMap<String, bool> =
			std::collections::HashMap::new();
		for msg in &messages {
			if msg.role == "assistant" && msg.tool_calls.is_some() {
				if let Some(tool_calls_value) = &msg.tool_calls {
					if let Some(tool_calls_array) = tool_calls_value.as_array() {
						for tool_call in tool_calls_array {
							if let Some(id) = tool_call.get("id").and_then(|v| v.as_str()) {
								preserved_tool_call_map.insert(id.to_string(), true);
							}
						}
					}
				}
			}
		}

		// Remove orphaned tool messages
		let mut i = 0;
		while i < messages.len() {
			let msg = &messages[i];

			if msg.role == "tool" {
				let mut is_orphaned = true;

				if let Some(tool_call_id) = &msg.tool_call_id {
					if preserved_tool_call_map.contains_key(tool_call_id) {
						is_orphaned = false;
					}
				}

				if is_orphaned {
					messages.remove(i);
					continue;
				}
			}
			i += 1;
		}

		// Should have removed the orphaned tool message
		assert_eq!(messages.len(), 2);
		assert_eq!(messages[0].role, "assistant");
		assert_eq!(messages[1].role, "tool");
		assert_eq!(messages[1].tool_call_id, Some("call_123".to_string()));
	}

	#[test]
	fn test_tool_sequence_ordering() {
		// Test that tool sequences maintain correct ordering
		let assistant_with_tools = create_test_message(
			"assistant",
			"I'll use a tool",
			Some(
				json!([{"id": "call_123", "type": "function", "function": {"name": "test_tool"}}]),
			),
			None,
			None,
		);

		let tool_result = create_test_message(
			"tool",
			"Tool result",
			None,
			Some("call_123".to_string()),
			Some("test_tool".to_string()),
		);

		// Test identify_tool_sequence_unit
		let messages = vec![assistant_with_tools.clone(), tool_result.clone()];
		let unit = super::identify_tool_sequence_unit(&messages, 0);

		assert!(unit.is_some());
		let (unit_messages, start_idx) = unit.unwrap();
		assert_eq!(start_idx, 0);
		assert_eq!(unit_messages.len(), 2);
		assert_eq!(unit_messages[0].role, "assistant");
		assert_eq!(unit_messages[1].role, "tool");
		assert_eq!(unit_messages[1].tool_call_id, Some("call_123".to_string()));
	}
}
