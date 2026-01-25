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

// Workflow command handler

use super::super::core::ChatSession;
use super::{CommandOutput, CommandResult};
use crate::config::Config;
use crate::session::workflows::WorkflowOrchestrator;
use anyhow::Result;

pub async fn handle_workflow(
	session: &mut ChatSession,
	config: &Config,
	_role: &str,
	params: &[&str],
	operation_cancelled: tokio::sync::watch::Receiver<bool>,
) -> Result<CommandResult> {
	// Handle /workflow command for executing workflows
	if params.is_empty() {
		// Show available workflows
		let available_workflows: Vec<(String, String)> = config
			.workflows
			.workflows
			.iter()
			.map(|(name, workflow)| (name.clone(), workflow.description.clone()))
			.collect();

		// Debug: print workflows count
		crate::log_debug!("Workflows found: {}", available_workflows.len());
		crate::log_debug!("Workflows config: {:?}", config.workflows);

		return Ok(CommandResult::HandledWithOutput(CommandOutput::Workflow {
			workflow_executed: String::new(),
			data: serde_json::json!({
				"action": "list",
				"workflows": available_workflows,
				"message": if available_workflows.is_empty() { "No workflows configured" } else { "Available workflows" }
			}),
		}));
	}

	let workflow_name = params[0];

	// Check if workflow exists
	if !config.workflows.workflows.contains_key(workflow_name) {
		let available_workflows: Vec<String> = config.workflows.workflows.keys().cloned().collect();
		return Ok(CommandResult::HandledWithOutput(CommandOutput::Workflow {
			workflow_executed: workflow_name.to_string(),
			data: serde_json::json!({
				"action": "execute",
				"success": false,
				"error": format!("Workflow not found: {}", workflow_name),
				"available_workflows": available_workflows
			}),
		}));
	}

	// Get the input for the workflow
	let workflow_input = if params.len() > 1 {
		// Use the provided input after the workflow name
		params[1..].join(" ")
	} else {
		// Use the last user message or a default input
		session
			.session
			.messages
			.iter()
			.rfind(|m| m.role == "user")
			.map(|m| m.content.clone())
			.unwrap_or_else(|| "No recent user input found".to_string())
	};

	// Check spending threshold before executing workflow
	match session.check_spending_threshold(config) {
		Ok(should_continue) => {
			if !should_continue {
				// Spending threshold reached - instant decline for /workflow commands
				return Ok(CommandResult::HandledWithOutput(CommandOutput::Workflow {
					workflow_executed: workflow_name.to_string(),
					data: serde_json::json!({
						"action": "execute",
						"success": false,
						"error": "Workflow execution cancelled due to spending threshold."
					}),
				}));
			}
		}
		Err(e) => {
			// Error checking threshold, log warning and stop execution
			return Ok(CommandResult::HandledWithOutput(CommandOutput::Workflow {
				workflow_executed: workflow_name.to_string(),
				data: serde_json::json!({
					"action": "execute",
					"success": false,
					"error": format!("Error checking spending threshold: {}", e)
				}),
			}));
		}
	}

	// Check request spending threshold before executing workflow
	match session.check_request_spending_threshold(config) {
		Ok(should_continue) => {
			if !should_continue {
				// Request spending threshold exceeded - stop execution
				return Ok(CommandResult::HandledWithOutput(CommandOutput::Workflow {
					workflow_executed: workflow_name.to_string(),
					data: serde_json::json!({
						"action": "execute",
						"success": false,
						"error": "Workflow execution cancelled due to request spending threshold."
					}),
				}));
			}
		}
		Err(e) => {
			// Error checking request threshold, log warning and stop execution
			return Ok(CommandResult::HandledWithOutput(CommandOutput::Workflow {
				workflow_executed: workflow_name.to_string(),
				data: serde_json::json!({
					"action": "execute",
					"success": false,
					"error": format!("Error checking request spending threshold: {}", e)
				}),
			}));
		}
	}

	// Get the workflow definition
	let workflow_def = config
		.workflows
		.workflows
		.get(workflow_name)
		.ok_or_else(|| anyhow::anyhow!("Workflow not found: {}", workflow_name))?
		.clone();

	let workflow_description = workflow_def.description.clone();

	// Execute the workflow
	let orchestrator = WorkflowOrchestrator::new(workflow_def, workflow_name.to_string());
	match orchestrator
		.execute(
			&workflow_input,
			&mut session.session,
			config,
			operation_cancelled,
		)
		.await
	{
		Ok((result, progress)) => Ok(CommandResult::HandledWithOutput(CommandOutput::Workflow {
			workflow_executed: workflow_name.to_string(),
			data: serde_json::json!({
				"action": "execute",
				"success": true,
				"result": result,
				"progress": progress,
				"workflow_description": workflow_description
			}),
		})),
		Err(e) => Ok(CommandResult::HandledWithOutput(CommandOutput::Workflow {
			workflow_executed: workflow_name.to_string(),
			data: serde_json::json!({
				"action": "execute",
				"success": false,
				"error": format!("Workflow execution failed: {}", e)
			}),
		})),
	}
}
