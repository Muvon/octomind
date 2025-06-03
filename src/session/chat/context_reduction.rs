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

/// Process context reduction (simplified version without external dependencies)
pub async fn perform_context_reduction(
	chat_session: &mut ChatSession,
	config: &Config,
	operation_cancelled: Arc<AtomicBool>
) -> Result<()> {
	println!("{}", "Performing context reduction and optimization...".cyan());

	// Create a task to show loading animation with current cost
	let animation_cancel = operation_cancelled.clone();
	let current_cost = chat_session.session.info.total_cost;
	let animation_task = tokio::spawn(async move {
		let _ = show_loading_animation(animation_cancel, current_cost).await;
	});

	// Extract elements from the session to create an optimized version
	// First get the original user request and the current context
	let user_message = chat_session.session.messages.iter()
		.find(|m| m.role == "user")
		.map(|m| m.content.clone())
		.unwrap_or_else(|| "No original query found".to_string());

	// Get the last assistant message as the context
	let assistant_message = chat_session.session.messages.iter()
		.filter(|m| m.role == "assistant")
		.next_back()
		.map(|m| m.content.clone())
		.unwrap_or_else(|| "No assistant response found".to_string());

	// Create a message for the reducer to summarize everything
	let reducer_input = format!("Original request: {}\n\nDeveloper solution: {}",
		user_message, assistant_message);

	// Create messages for the chat completion API
	let mut messages = Vec::new();

	// System message with reducer-specific prompt
	let system_prompt = r#"You are a context reduction specialist. Your task is to create a concise, comprehensive summary that preserves the essential information from a developer conversation.

INSTRUCTIONS:
1. Extract the key problem/request from the original user message
2. Summarize the main solution provided by the developer
3. Include important technical details, file paths, and code changes
4. Preserve any critical context that might be needed for future development
5. Remove redundant explanations and verbose descriptions
6. Keep the summary focused and actionable

Format your response as a clear, structured summary that maintains the technical context while reducing verbosity."#;

	messages.push(crate::session::Message {
		role: "system".to_string(),
		content: system_prompt.to_string(),
		timestamp: std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.unwrap_or_default()
			.as_secs(),
		cached: false,
		tool_call_id: None,
		name: None,
		tool_calls: None,
	});

	// Add user message with the context to reduce
	messages.push(crate::session::Message {
		role: "user".to_string(),
		content: reducer_input,
		timestamp: std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.unwrap_or_default()
			.as_secs(),
		cached: false,
		tool_call_id: None,
		name: None,
		tool_calls: None,
	});

	// Use the same model as the chat session for consistency
	let reducer_model = &chat_session.model;

	// Call the model using the unified provider system
	println!("{} {}", "Using model for context reduction:".bright_blue(), reducer_model.bright_yellow());
	let reduction_result = crate::session::chat_completion_with_provider(
		&messages,
		reducer_model,
		0.7, // Moderate temperature
		config
	).await;

	// Stop the animation
	operation_cancelled.store(true, Ordering::SeqCst);
	let _ = animation_task.await;

	match reduction_result {
		Ok(response) => {
			// Extract the reduced content from the response
			let reduced_content = response.content;

			// Log restoration point before clearing session
			let user_message_content = chat_session.session.messages.iter()
				.find(|m| m.role == "user")
				.map(|m| m.content.clone())
				.unwrap_or_else(|| "No user message found".to_string());

			let _ = crate::session::logger::log_restoration_point(&chat_session.session.info.name, &user_message_content, &reduced_content);

			// Log restoration point to session file as well for complete restoration capability
			if let Some(session_file) = &chat_session.session.session_file {
				let restoration_data = serde_json::json!({
					"user_message": user_message_content,
					"assistant_response": reduced_content,
					"timestamp": std::time::SystemTime::now()
						.duration_since(std::time::UNIX_EPOCH)
						.unwrap_or_default()
						.as_secs()
				});
				let restoration_json = serde_json::to_string(&restoration_data)?;
				let _ = crate::session::append_to_session_file(session_file, &format!("RESTORATION_POINT: {}", restoration_json));
			}

			println!("{}", "Context reduction complete".bright_green());
			println!("{}", reduced_content.bright_blue());

			// Clear the session while preserving key elements
			let system_message = chat_session.session.messages.iter()
				.find(|m| m.role == "system")
				.cloned();

			// Store user message if it exists to prevent warning
			let user_message = chat_session.session.messages.iter()
				.find(|m| m.role == "user")
				.cloned();

			// Find and preserve any tool calls from the session
			let tool_call_messages = chat_session.session.messages.iter()
				.filter(|m| m.content.contains("tool_call_id") && m.content.contains("\"role\":\"tool\""))
				.cloned()
				.collect::<Vec<_>>();

			chat_session.session.messages.clear();

			// Restore system message
			if let Some(system) = system_message {
				chat_session.session.messages.push(system);
			}

			// Add back the user message to prevent 'warning: user message not found' issues
			if let Some(user) = user_message {
				chat_session.session.messages.push(user);
			}

			// Restore any tool call messages to maintain tool context
			for tool_msg in tool_call_messages {
				chat_session.session.messages.push(tool_msg);
			}

			// Add the reduced content as a cached context for next iteration
			chat_session.session.add_message("assistant", &reduced_content);
			let last_index = chat_session.session.messages.len() - 1;
			chat_session.session.messages[last_index].cached = true;

			// CRITICAL: Reset cache markers and recalibrate token tracking
			// Reset token counters for fresh context start
			chat_session.session.current_non_cached_tokens = 0;
			chat_session.session.current_total_tokens = 0;
			
			// Update cache checkpoint time
			chat_session.session.last_cache_checkpoint_time = std::time::SystemTime::now()
				.duration_since(std::time::UNIX_EPOCH)
				.unwrap_or_default()
				.as_secs();

			// Save stats for the reduction
			if let Some(usage) = &response.exchange.usage {
				let cost = usage.cost.unwrap_or(0.0);
				if cost > 0.0 {
					println!("{}", format!("Context reduction cost: ${:.5}", cost).bright_magenta());

					// Add the stats to the session
					chat_session.session.add_layer_stats(
						"context_optimization",
						reducer_model,
						usage.prompt_tokens,
						usage.completion_tokens,
						cost
					);

					// Update the overall cost in the session
					chat_session.session.info.total_cost += cost;
					chat_session.estimated_cost = chat_session.session.info.total_cost;
				}
			}

			println!("{}", "Session context optimized for next interaction".bright_green());
			println!("{}", "The next message will be processed through the full layered pipeline.".bright_cyan());

			// Save the updated session
			chat_session.save()?;

			Ok(())
		},
		Err(e) => {
			println!("{}: {}", "Error during context reduction".bright_red(), e);
			Err(anyhow::anyhow!("Context reduction failed: {}", e))
		}
	}
}
