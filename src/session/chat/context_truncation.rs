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

/// Simple boundary truncation for manual /truncate command
/// Cuts messages until reaching assistant without tool calls OR user message
/// This preserves order and removes tool sequences safely
pub async fn perform_simple_boundary_truncation(
	chat_session: &mut ChatSession,
	_config: &Config,
	current_tokens: usize,
	role: &str,
) -> Result<()> {
	use colored::Colorize;

	// Basic validation
	if chat_session.session.messages.is_empty() {
		return Ok(()); // Nothing to truncate
	}

	// Find system message to preserve
	let system_message = chat_session
		.session
		.messages
		.iter()
		.find(|m| m.role == "system")
		.cloned();

	// SIMPLE LOGIC: Work backwards, keep messages until we need to cut
	// Cut when we hit: assistant with tool calls (to avoid orphaned tools)
	// Keep: user messages, assistant without tool calls
	let mut kept_messages = Vec::new();

	// Work backwards through all messages (skip system)
	for msg in chat_session.session.messages.iter().rev() {
		if msg.role == "system" {
			continue; // Handle system separately
		}

		match msg.role.as_str() {
			"user" => {
				// User messages are safe boundaries - always keep
				kept_messages.push(msg.clone());
			}
			"assistant" => {
				if msg.tool_calls.is_none() {
					// Assistant without tool calls - safe boundary, keep it
					kept_messages.push(msg.clone());
				} else {
					// Assistant with tool calls - STOP here to avoid orphaned tools
					break;
				}
			}
			"tool" => {
				// Tool messages - STOP here, they need their assistant message
				break;
			}
			_ => {
				// Other message types - keep them
				kept_messages.push(msg.clone());
			}
		}
	}

	// Reverse to restore chronological order
	kept_messages.reverse();

	// Build final message list
	let mut final_messages = Vec::new();

	// Add system message first
	if let Some(sys_msg) = system_message {
		final_messages.push(sys_msg);
	}

	// Add initial messages (welcome + instructions) using centralized function
	// Use thread-local if set (ACP/WebSocket), otherwise process cwd
	let current_dir = crate::mcp::get_thread_working_directory();
	if let Ok(initial_messages) =
		crate::session::chat::session::get_initial_messages(_config, role, &current_dir).await
	{
		final_messages.extend(initial_messages);
	}

	// Add kept messages
	final_messages.extend(kept_messages);

	// Update session
	let original_count = chat_session.session.messages.len();
	chat_session.session.messages = final_messages;

	let new_token_count = crate::session::estimate_session_tokens(&chat_session.session.messages);
	let tokens_saved = current_tokens.saturating_sub(new_token_count);
	let new_count = chat_session.session.messages.len();

	// Calculate messages removed (can be negative if messages were added)
	let messages_removed = original_count.saturating_sub(new_count);

	println!(
		"{}",
		format!(
			"Simple boundary truncation complete: {} messages removed, {} tokens saved",
			messages_removed, tokens_saved
		)
		.bright_green()
	);

	// Save the session
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
		..Default::default()
	};
	new_messages.push(summary_msg);

	// Replace session messages with summarized version
	let original_count = chat_session.session.messages.len();
	chat_session.session.messages = new_messages;

	// Reset token tracking for fresh start
	chat_session.session.info.current_non_cached_tokens = 0;
	chat_session.session.info.current_total_tokens = 0;

	// Reset cache checkpoint time
	chat_session.session.info.last_cache_checkpoint_time = std::time::SystemTime::now()
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
			videos: None,
			thinking: None,
			id: None,
		}
	}

	#[test]
	fn test_tool_sequence_identification() {
		let messages = [
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
}
