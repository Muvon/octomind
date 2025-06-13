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

// Reduce command handler

use super::super::core::ChatSession;
use super::utils::format_number;
use crate::config::Config;
use anyhow::Result;
use colored::Colorize;

pub async fn handle_reduce(session: &mut ChatSession, config: &Config) -> Result<bool> {
	// Perform session history reduction using the configured reducer layer
	println!(
		"{}",
		"🔄 Reducing session history using configured reducer layer...".bright_cyan()
	);

	// Estimate current token usage
	let current_tokens = crate::session::estimate_message_tokens(&session.session.messages);
	println!(
		"{}",
		format!(
			"Current context size: {} tokens",
			format_number(current_tokens as u64)
		)
		.bright_blue()
	);

	// Use the reduction logic
	match handle_reduce_internal(session, config).await {
		Ok(()) => {
			// Calculate new token count after reduction
			let new_tokens = crate::session::estimate_message_tokens(&session.session.messages);
			let tokens_saved = current_tokens.saturating_sub(new_tokens);

			println!(
                "{}",
                format!(
                    "✅ Session reduced from {} messages to {} message. {} tokens saved, new context size: {} tokens",
                    // Calculate original message count (we don't have it, so estimate)
                    (current_tokens / 100).max(1), // rough estimate
                    session.session.messages.len() - 1, // -1 for system message
                    format_number(tokens_saved as u64),
                    format_number(new_tokens as u64)
                )
                .bright_green()
            );
		}
		Err(e) => {
			println!("{}: {}", "Session reduction failed".bright_red(), e);
		}
	}

	Ok(false)
}

// Internal reduce handler function
async fn handle_reduce_internal(session: &mut ChatSession, config: &Config) -> anyhow::Result<()> {
	// Collect all session messages into a single input string for the reducer
	let mut input_content = String::new();
	input_content.push_str("Session History Summary Request:\n\n");

	for (i, message) in session.session.messages.iter().enumerate() {
		match message.role.as_str() {
			"system" => {
				// Skip system messages in the input - they're not part of conversation history
				continue;
			}
			"user" => {
				input_content.push_str(&format!(
					"[Message {}] User: \"{}\"\n",
					i + 1,
					message.content
				));
			}
			"assistant" => {
				input_content.push_str(&format!(
					"[Message {}] Assistant: \"{}\"\n",
					i + 1,
					message.content
				));
			}
			"tool" => {
				input_content.push_str(&format!(
					"[Message {}] Tool Result: \"{}\"\n",
					i + 1,
					message.content
				));
			}
			_ => {
				input_content.push_str(&format!(
					"[Message {}] {}: \"{}\"\n",
					i + 1,
					message.role,
					message.content
				));
			}
		}
	}

	input_content.push_str("\nPlease provide a concise summary preserving key context, decisions, and important technical information.");

	// Create a filtered orchestrator with only the reducer layer
	let orchestrator = {
		// Search ALL configured layers for the reducer (not just role-enabled layers)
		let reducer_layers: Vec<_> = config
			.layers
			.as_ref()
			.map(|layers| {
				layers
					.iter()
					.filter(|layer| layer.name == "reducer")
					.cloned()
					.collect()
			})
			.unwrap_or_default();

		if reducer_layers.is_empty() {
			return Err(anyhow::anyhow!("No 'reducer' layer found in configuration. Please add a reducer layer to your config."));
		}

		// Create orchestrator with only the reducer layer
		let mut layers: Vec<Box<dyn crate::session::layers::Layer + Send + Sync>> = Vec::new();
		for mut layer_config in reducer_layers {
			// Process and cache the system prompt for this layer
			let project_dir =
				std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
			layer_config
				.process_and_cache_system_prompt(&project_dir)
				.await;
			layers.push(Box::new(crate::session::layers::GenericLayer::new(
				layer_config,
			)));
		}

		crate::session::layers::LayeredOrchestrator { layers }
	};

	// Create a temporary session for the reducer layer
	let mut temp_session = crate::session::Session::new(
		"temp_reducer".to_string(),
		config.get_effective_model().to_string(),
		"temp".to_string(),
	);

	// Process through the reducer layer
	let reduced_content = orchestrator
		.process(
			&input_content,
			&mut temp_session,
			config,
			std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
		)
		.await?;

	// Clear all messages except system message
	session.session.messages.retain(|msg| msg.role == "system");

	// Add the reduced content as a single user message
	session.add_user_message(&reduced_content)?;

	// Save the session with the reduced content
	session.save()?;

	Ok(())
}
