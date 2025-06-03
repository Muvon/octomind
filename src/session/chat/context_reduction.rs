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

// Context reduction for session optimization

use crate::config::Config;
use crate::session::chat::session::ChatSession;
use colored::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use anyhow::Result;
use super::animation::show_loading_animation;

/// Process context reduction - smart truncation with summarization
/// Uses same model and session flow, then keeps only the summarized context
pub async fn perform_context_reduction(
	chat_session: &mut ChatSession,
	config: &Config,
	operation_cancelled: Arc<AtomicBool>
) -> Result<()> {
	println!("{}", "Summarizing conversation context...".cyan());

	// Build conversation history for summarization (exclude system message)
	let conversation_history = chat_session.session.messages.iter()
		.filter(|m| m.role != "system")
		.map(|m| format!("{}: {}", m.role.to_uppercase(), m.content))
		.collect::<Vec<_>>()
		.join("\n\n");

	if conversation_history.is_empty() {
		println!("{}", "No conversation to summarize".yellow());
		return Ok(());
	}

	// Create summarization prompt as a user message
	let summarization_prompt = format!(
		"Please create a concise summary of our conversation that preserves all important technical details, decisions made, files modified, and context needed for future development. Focus on actionable information and key outcomes.\n\nConversation to summarize:\n{}",
		conversation_history
	);

	// Add the summarization request as a regular user message to the session
	chat_session.add_user_message(&summarization_prompt)?;

	// Create a task to show loading animation with current cost
	let animation_cancel = operation_cancelled.clone();
	let current_cost = chat_session.session.info.total_cost;
	let animation_task = tokio::spawn(async move {
		let _ = show_loading_animation(animation_cancel, current_cost).await;
	});

	// Call the same model using the normal session flow
	let api_result = crate::session::chat_completion_with_provider(
		&chat_session.session.messages,
		&chat_session.model,
		chat_session.temperature,
		config
	).await;

	// Stop the animation
	operation_cancelled.store(true, Ordering::SeqCst);
	let _ = animation_task.await;

	match api_result {
		Ok(response) => {
			let summary_content = response.content;

			// Log restoration point for recovery
			let _ = crate::session::logger::log_restoration_point(
				&chat_session.session.info.name, 
				"Context summarization", 
				&summary_content
			);

			// Log to session file as well
			if let Some(session_file) = &chat_session.session.session_file {
				let restoration_data = serde_json::json!({
					"type": "context_reduction",
					"summary": summary_content,
					"original_message_count": chat_session.session.messages.len(),
					"timestamp": std::time::SystemTime::now()
						.duration_since(std::time::UNIX_EPOCH)
						.unwrap_or_default()
						.as_secs()
				});
				let restoration_json = serde_json::to_string(&restoration_data)?;
				let _ = crate::session::append_to_session_file(session_file, &format!("RESTORATION_POINT: {}", restoration_json));
			}

			println!("{}", "Context summarization complete".bright_green());
			println!("{}", summary_content.bright_blue());

			// SMART TRUNCATION: Keep only system message + summary as assistant message
			let system_message = chat_session.session.messages.iter()
				.find(|m| m.role == "system")
				.cloned();

			// Clear all messages
			chat_session.session.messages.clear();

			// Restore system message
			if let Some(system) = system_message {
				chat_session.session.messages.push(system);
			}

			// Add the summary as an assistant message (this is our new context)
			chat_session.session.add_message("assistant", &summary_content);
			let last_index = chat_session.session.messages.len() - 1;
			chat_session.session.messages[last_index].cached = true; // Mark for caching

			// Reset token tracking for fresh start
			chat_session.session.current_non_cached_tokens = 0;
			chat_session.session.current_total_tokens = 0;
			
			// Update cache checkpoint time
			chat_session.session.last_cache_checkpoint_time = std::time::SystemTime::now()
				.duration_since(std::time::UNIX_EPOCH)
				.unwrap_or_default()
				.as_secs();

			// Update session stats
			if let Some(usage) = &response.exchange.usage {
				let cost = usage.cost.unwrap_or(0.0);
				if cost > 0.0 {
					println!("{}", format!("Summarization cost: ${:.5}", cost).bright_magenta());

					// Add the stats to the session
					chat_session.session.add_layer_stats(
						"context_summarization",
						&chat_session.model,
						usage.prompt_tokens,
						usage.completion_tokens,
						cost
					);

					// Update the overall cost in the session
					chat_session.session.info.total_cost += cost;
					chat_session.estimated_cost = chat_session.session.info.total_cost;
				}
			}

			println!("{}", "Session context reduced to essential summary".bright_green());
			println!("{}", "You can now continue the conversation with optimized context".bright_cyan());

			// Save the updated session
			chat_session.save()?;

			Ok(())
		},
		Err(e) => {
			// Remove the summarization prompt since it failed
			if let Some(last_msg) = chat_session.session.messages.last() {
				if last_msg.role == "user" && last_msg.content.contains("Please create a concise summary") {
					chat_session.session.messages.pop();
				}
			}
			
			println!("{}: {}", "Error during context summarization".bright_red(), e);
			Err(anyhow::anyhow!("Context summarization failed: {}", e))
		}
	}
}
