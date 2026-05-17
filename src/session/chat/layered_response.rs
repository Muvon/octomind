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

// Layered response processing implementation

use crate::config::Config;
use crate::mcp::get_available_functions;
use crate::session::chat::get_animation_manager;
use crate::session::chat::session::ChatSession;
use crate::session::estimate_full_context_tokens;
use anyhow::Result;
use colored::*;
// Process a response using the layered architecture
pub async fn process_layered_response(
	input: &str,
	chat_session: &mut ChatSession,
	config: &Config,
	role: &str,
	operation_cancelled: tokio::sync::watch::Receiver<bool>,
) -> Result<String> {
	// Ensure system message is cached before processing with layers
	// This is important because system messages contain all the function definitions
	// and developer context needed for the layered processing
	let mut system_message_cached = false;

	// Check if system message is already cached
	for msg in &chat_session.session.messages {
		if msg.role == "system" && msg.cached {
			system_message_cached = true;
			break;
		}
	}

	// If system message not already cached, add a cache checkpoint
	if !system_message_cached {
		if let Ok(cached) = chat_session.session.add_cache_checkpoint(true) {
			if cached && crate::session::model_supports_caching(&chat_session.model) {
				println!(
					"{}",
					"System message has been automatically marked for caching to save tokens."
						.yellow()
				);
				// Save the session to ensure the cached status is persisted
				if let Err(e) = chat_session.save() {
					crate::log_debug!("session save failed: {}", e);
				}
			}
		}
	}

	// Use AnimationManager for animation
	let animation_manager = get_animation_manager();
	let current_cost = chat_session.session.info.total_cost;
	let max_threshold = config.max_session_tokens_threshold;

	// Calculate actual current context tokens for percentage display
	let tools = get_available_functions(config).await;
	let current_context_tokens =
		estimate_full_context_tokens(&chat_session.session.messages, Some(&tools)) as u64;

	animation_manager
		.start_with_params(current_cost, current_context_tokens, max_threshold)
		.await;

	// Display status message BEFORE processing starts - cleaner flow
	if config.get_log_level().is_debug_enabled() {
		println!("{}", "Using workflow processing with model-specific caching - only supported models will use caching".bright_cyan());
	} else {
		println!("{}", "Using workflow processing".bright_cyan());
	}

	// Process through workflows if configured, otherwise pass through unchanged
	// Each workflow step operates on its own session context and passes output to next step
	//
	// IMPORTANT: Each layer within workflow handles its own function calls internally
	// using the process method in processor.rs
	//
	// ANIMATION: We show the animation during workflow processing
	let workflow_output: String = if let Some(role_data) = config.role_map.get(role) {
		if let Some(workflow_name) = &role_data.workflow {
			// Get workflow definition
			let workflow_def = config
				.workflows
				.iter()
				.find(|w| &w.name == workflow_name)
				.ok_or_else(|| anyhow::anyhow!("Workflow '{}' not found", workflow_name))?
				.clone();

			// Execute workflow
			let workflow_orchestrator = crate::session::workflows::WorkflowOrchestrator::new(
				workflow_def,
				workflow_name.to_string(),
			);
			match workflow_orchestrator
				.execute(
					input,
					&mut chat_session.session,
					config,
					operation_cancelled.clone(),
				)
				.await
			{
				Ok((output, _progress)) => output, // Ignore progress in layered response
				Err(e) => {
					// Stop the animation
					animation_manager.stop_current().await;
					return Err(e);
				}
			}
		} else {
			// No workflow configured - pass through unchanged
			input.to_string()
		}
	} else {
		// Role not found - pass through unchanged
		input.to_string()
	};

	// Stop the animation
	animation_manager.stop_current().await;

	// Return the processed output from workflow for use in the main model conversation
	// This output already includes the results of any function calls handled by each layer
	Ok(workflow_output)
}
