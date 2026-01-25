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

// Layered response processing implementation

use super::animation::show_smart_animation;
use crate::config::Config;
use crate::session::chat::session::ChatSession;
use anyhow::Result;
use colored::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

// Process a response using the layered architecture
// Returns the final processed text that should be used as input for the main model
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
				let _ = chat_session.save();
			}
		}
	}

	// Create a task to show loading animation with current cost
	// Use a separate flag for animation to avoid conflicts with user cancellation detection
	let animation_cancel = Arc::new(AtomicBool::new(false));
	let animation_cancel_clone = animation_cancel.clone();
	let current_cost = chat_session.session.info.total_cost;
	let animation_task = tokio::spawn(async move {
		let _ = show_smart_animation(animation_cancel_clone, current_cost).await;
	});

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
				.get_workflow(workflow_name)
				.ok_or_else(|| anyhow::anyhow!("Workflow '{}' not found", workflow_name))?
				.clone();

			// Execute workflow
			let workflow_orchestrator =
				crate::session::workflows::WorkflowOrchestrator::new(workflow_def);
			match workflow_orchestrator
				.execute(
					input,
					&mut chat_session.session,
					config,
					operation_cancelled.clone(),
				)
				.await
			{
				Ok(output) => output,
				Err(e) => {
					// Stop the animation using the separate animation flag
					animation_cancel.store(true, Ordering::SeqCst);
					let _ = animation_task.await;
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

	// Stop the animation using the separate animation flag
	animation_cancel.store(true, Ordering::SeqCst);
	let _ = animation_task.await;

	// Return the processed output from workflow for use in the main model conversation
	// This output already includes the results of any function calls handled by each layer
	Ok(workflow_output)
}
