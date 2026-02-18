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

// Run command handler

use super::super::core::ChatSession;
use super::{CommandOutput, CommandResult};
use crate::config::Config;
use crate::session::chat::command_executor;
use anyhow::Result;

pub async fn handle_run(
	session: &mut ChatSession,
	config: &Config,
	role: &str,
	params: &[&str],
	operation_cancelled: tokio::sync::watch::Receiver<bool>,
) -> Result<CommandResult> {
	// Handle /run command for executing command layers
	if params.is_empty() {
		// Show available commands for this role
		let available_commands = command_executor::list_available_commands(config, role);

		return Ok(CommandResult::HandledWithOutput(CommandOutput::Run {
			command_executed: String::new(),
			data: serde_json::json!({
				"action": "list",
				"commands": available_commands,
				"message": if available_commands.is_empty() { "No commands configured" } else { "Available commands" }
			}),
		}));
	}

	let command_name = params[0];

	// Check if command exists
	if !command_executor::command_exists(config, role, command_name) {
		let available_commands = command_executor::list_available_commands(config, role);
		return Ok(CommandResult::HandledWithOutput(CommandOutput::Run {
			command_executed: command_name.to_string(),
			data: serde_json::json!({
				"action": "execute",
				"success": false,
				"error": format!("Command not found: {}", command_name),
				"available_commands": available_commands
			}),
		}));
	}

	// Get the input for the command layer
	// For now, we'll use the last user message or the whole session depending on the input_mode
	// We could also allow passing input as additional parameters
	let command_input = if params.len() > 1 {
		// Use the provided input after the command name
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

	// Check spending threshold before executing command layer
	// For /run commands, any threshold breach results in instant decline and stop
	match session.check_spending_threshold(config) {
		Ok(should_continue) => {
			if !should_continue {
				// Spending threshold reached - instant decline for /run commands
				return Ok(CommandResult::HandledWithOutput(CommandOutput::Run {
					command_executed: command_name.to_string(),
					data: serde_json::json!({
						"action": "execute",
						"success": false,
						"error": "Command execution cancelled due to spending threshold."
					}),
				}));
			}
		}
		Err(e) => {
			// Error checking threshold, log warning and stop execution
			return Ok(CommandResult::HandledWithOutput(CommandOutput::Run {
				command_executed: command_name.to_string(),
				data: serde_json::json!({
					"action": "execute",
					"success": false,
					"error": format!("Error checking spending threshold: {}", e)
				}),
			}));
		}
	}

	// Check request spending threshold before executing command layer
	match session.check_request_spending_threshold(config) {
		Ok(should_continue) => {
			if !should_continue {
				// Request spending threshold exceeded - stop execution
				return Ok(CommandResult::HandledWithOutput(CommandOutput::Run {
					command_executed: command_name.to_string(),
					data: serde_json::json!({
						"action": "execute",
						"success": false,
						"error": "Command execution cancelled due to request spending threshold."
					}),
				}));
			}
		}
		Err(e) => {
			// Error checking request threshold, log warning and stop execution
			return Ok(CommandResult::HandledWithOutput(CommandOutput::Run {
				command_executed: command_name.to_string(),
				data: serde_json::json!({
					"action": "execute",
					"success": false,
					"error": format!("Error checking request spending threshold: {}", e)
				}),
			}));
		}
	}

	// Execute the command layer
	match command_executor::execute_command_layer(
		command_name,
		&command_input,
		session,
		config,
		role,
		operation_cancelled,
	)
	.await
	{
		Ok(result) => Ok(CommandResult::HandledWithOutput(CommandOutput::Run {
			command_executed: command_name.to_string(),
			data: serde_json::json!({
				"action": "execute",
				"success": true,
				"result": result
			}),
		})),
		Err(e) => Ok(CommandResult::HandledWithOutput(CommandOutput::Run {
			command_executed: command_name.to_string(),
			data: serde_json::json!({
				"action": "execute",
				"success": false,
				"error": format!("Command execution failed: {}", e)
			}),
		})),
	}
}
