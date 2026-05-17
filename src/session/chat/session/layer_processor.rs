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

// Layer processing utilities

use super::core::ChatSession;
use crate::config::Config;
use crate::log_info;
use crate::session::chat::layered_response::process_layered_response;
use crate::session::pipelines::PipelineOrchestrator;
use anyhow::Result;
use colored::*;
use tokio::sync::watch;

// Helper function to process layers if enabled
pub async fn process_layers_if_enabled(
	input: &str,
	chat_session: &mut ChatSession,
	config: &Config,
	role: &str,
	first_message_processed: bool,
	operation_rx: watch::Receiver<bool>,
) -> Result<(String, bool, bool)> {
	// Check if role uses pipeline and/or workflow
	let has_pipeline = config
		.role_map
		.get(role)
		.and_then(|r| r.pipeline.as_ref())
		.is_some();
	let has_workflow = config
		.role_map
		.get(role)
		.and_then(|r| r.workflow.as_ref())
		.is_some();

	if (!has_pipeline && !has_workflow) || first_message_processed {
		return Ok((input.to_string(), false, false));
	}

	let mut current_input = input.to_string();

	// Phase 1: Pipeline (deterministic scripts) — runs BEFORE workflow
	if has_pipeline {
		let pipeline_name = config
			.role_map
			.get(role)
			.and_then(|r| r.pipeline.as_ref())
			.unwrap();

		let pipeline_def = config
			.pipelines
			.iter()
			.find(|p| &p.name == pipeline_name)
			.ok_or_else(|| anyhow::anyhow!("Pipeline '{}' not found", pipeline_name))?
			.clone();

		let working_dir = config.get_working_directory();
		let orchestrator = PipelineOrchestrator::new(pipeline_def, pipeline_name.clone());

		log_info!("Running pipeline '{}'", pipeline_name);

		match orchestrator
			.execute(&current_input, &working_dir, role, operation_rx.clone())
			.await
		{
			Ok(output) => {
				log_info!("Pipeline '{}' completed.", pipeline_name);
				current_input = output;
			}
			Err(e) => {
				if crate::session::cancellation::is_cancelled(&e) {
					crate::log_debug!("Pipeline cancelled by user.");
					println!("{}", "Pipeline cancelled.".yellow());
					return Ok((input.to_string(), false, true));
				}
				// Pipeline errors are fatal — non-zero exit code = hard stop
				println!("\n{}: {}", "Pipeline failed".bright_red(), e);
				return Err(e);
			}
		}
	}

	// Phase 2: Workflow (agentic LLM processing) — uses pipeline output as input
	if has_workflow {
		// Track session message count before workflow processing
		let messages_before_workflow = chat_session.session.messages.len();

		// Process using workflow architecture to get improved input
		let workflow_result =
			process_layered_response(&current_input, chat_session, config, role, operation_rx)
				.await;

		match workflow_result {
			Ok(processed_input) => {
				// Check if workflow modified the session
				let messages_after_workflow = chat_session.session.messages.len();
				let workflow_modified_session = messages_after_workflow > messages_before_workflow;

				if workflow_modified_session {
					// Workflow used output_mode append/replace and added messages to session
					log_info!(
						"Workflow modified session ({} messages added).",
						messages_after_workflow - messages_before_workflow
					);
					// Return indication that workflow modified session
					return Ok((processed_input, true, false));
				} else {
					// Workflow didn't modify session (all had output_mode = none)
					// Use the processed input from workflow instead of the original input
					log_info!("Workflow processing complete. Using enhanced input for main model.");
					return Ok((processed_input, false, false));
				}
			}
			Err(e) => {
				// Check if this is a cancellation error - if so, propagate it to main loop
				if crate::session::cancellation::is_cancelled(&e) {
					// This is a cancellation error - handle gracefully and continue session
					crate::log_debug!("Operation cancelled by user.");
					println!("{}", "Continuing with original input.".yellow());

					// CRITICAL FIX: Clean up any partial workflow modifications to session
					// When workflow is cancelled, it might have partially modified the session
					// We need to restore the session to its state before workflow processing
					let messages_after_cancellation = chat_session.session.messages.len();
					if messages_after_cancellation > messages_before_workflow {
						// Remove messages added by workflow before cancellation
						let messages_to_remove =
							messages_after_cancellation - messages_before_workflow;
						for _ in 0..messages_to_remove {
							chat_session.session.messages.pop();
						}
						println!(
							"{}",
							format!(
								"Cleaned up {} messages added by cancelled layers",
								messages_to_remove
							)
							.yellow()
						);
					}

					// Return original input and continue session normally
					return Ok((input.to_string(), false, true));
				}

				// Regular layer processing error - print message and continue with original input
				println!(
					"\n{}: {}",
					"Error processing through layers".bright_red(),
					e
				);
				println!("{}", "Continuing with original input.".yellow());
				return Ok((input.to_string(), false, false));
			}
		}
	}

	// Pipeline ran but no workflow — return pipeline output
	Ok((current_input, false, false))
}
