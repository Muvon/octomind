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

// Command executor for /run commands using layers

use crate::config::Config;
use crate::session::chat::session::ChatSession;
use crate::session::{layers::layer_trait::Layer, layers::LayerProcessor};
use anyhow::Result;
use colored::Colorize;

/// Execute a command layer without storing it in the session history
pub async fn execute_command_layer(
	command_name: &str,
	provided_input: &str,
	chat_session: &mut ChatSession,
	config: &Config,
	role: &str,
	operation_cancelled: tokio::sync::watch::Receiver<bool>,
) -> Result<String> {
	// Get role configuration to check for command layers
	let (_, _, _, commands_config, _) = config.get_role_config(role);

	// Find the command configuration
	let command_config = commands_config
		.and_then(|commands| commands.iter().find(|cmd| cmd.name == command_name))
		.ok_or_else(|| anyhow::anyhow!("Command '{}' not found in configuration", command_name))?;

	println!(
		"{} {}",
		"Executing command:".bright_cyan(),
		command_name.bright_yellow()
	);

	// Log the command execution
	if let Some(session_file) = &chat_session.session.session_file {
		let log_entry = serde_json::json!({
			"type": "COMMAND_EXEC",
			"timestamp": std::time::SystemTime::now()
				.duration_since(std::time::UNIX_EPOCH)
				.unwrap_or_default()
				.as_secs(),
			"command": command_name,
			"role": role,
			"config": {
				"command": command_config.command,
				"workdir": command_config.workdir,
				"input_mode": format!("{:?}", command_config.input_mode)
			}
		});
		let _ = crate::session::append_to_session_file(
			session_file,
			&serde_json::to_string(&log_entry)?,
		);
	}

	// Execute the layer using ACP protocol
	let command_layer = LayerProcessor::new(command_config.clone());

	// Prepare the input according to the command's input_mode
	let processed_input = command_layer.prepare_input(provided_input, &chat_session.session);

	// Log the processed input
	if let Some(session_file) = &chat_session.session.session_file {
		let log_entry = serde_json::json!({
			"type": "COMMAND_INPUT",
			"timestamp": std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.unwrap_or_default()
			.as_secs(),
			"command": command_name,
			"input_length": processed_input.len(),
			"input_mode": format!("{:?}", command_config.input_mode)
		});
		let _ = crate::session::append_to_session_file(
			session_file,
			&serde_json::to_string(&log_entry)?,
		);
	}

	// Execute the layer without affecting the session
	let result = command_layer
		.process(
			&processed_input,
			&chat_session.session,
			config,
			operation_cancelled,
		)
		.await?;

	// Log the command result
	if let Some(session_file) = &chat_session.session.session_file {
		let log_entry = serde_json::json!({
			"type": "COMMAND_RESULT",
			"timestamp": std::time::SystemTime::now()
				.duration_since(std::time::UNIX_EPOCH)
				.unwrap_or_default()
				.as_secs(),
			"command": command_name,
			"output_length": result.outputs.iter().map(|s| s.len()).sum::<usize>(),
		});
		let _ = crate::session::append_to_session_file(
			session_file,
			&serde_json::to_string(&log_entry)?,
		);
	}

	// Handle output_mode to determine how this command's output affects the session
	use crate::session::layers::layer_trait::OutputMode;
	match command_config.output_mode {
		OutputMode::None => {
			// Command output is returned but doesn't modify session (default behavior)
			println!(
				"{}",
				"Output mode: none (command output only)".bright_cyan()
			);
		}
		OutputMode::Append => {
			// Add command output as new assistant message to session
			println!(
				"{}",
				"Output mode: append (adding to session)".bright_cyan()
			);
			// Add all command outputs as messages with configured role
			for output_text in &result.outputs {
				chat_session
					.session
					.add_message(command_config.output_role.as_str(), output_text);
			}

			// Log the append operation for session restoration
			if let Some(session_file) = &chat_session.session.session_file {
				let log_entry = serde_json::json!({
					"type": "OUTPUT_MODE_APPEND",
					"timestamp": std::time::SystemTime::now()
						.duration_since(std::time::UNIX_EPOCH)
						.unwrap_or_default()
						.as_secs(),
					"command": command_name,
					"content_length": result.outputs.iter().map(|s| s.len()).sum::<usize>()
				});
				let _ = crate::session::append_to_session_file(
					session_file,
					&serde_json::to_string(&log_entry)?,
				);
			}

			// Save session to persist the new message
			let _ = chat_session.save();
		}
		OutputMode::Replace => {
			// Replace entire session with this command's output
			println!(
				"{}",
				"Output mode: replace (replacing session content)".bright_cyan()
			);

			// Log the replace operation for session restoration
			if let Some(session_file) = &chat_session.session.session_file {
				let log_entry = serde_json::json!({
					"type": "OUTPUT_MODE_REPLACE",
					"timestamp": std::time::SystemTime::now()
						.duration_since(std::time::UNIX_EPOCH)
						.unwrap_or_default()
						.as_secs(),
					"command": command_name,
					"previous_message_count": chat_session.session.messages.len(),
					"content_length": result.outputs.iter().map(|s| s.len()).sum::<usize>()
				});
				let _ = crate::session::append_to_session_file(
					session_file,
					&serde_json::to_string(&log_entry)?,
				);
			}

			// Find system message to preserve
			let system_message = chat_session
				.session
				.messages
				.iter()
				.find(|m| m.role == "system")
				.cloned();

			// Clear existing messages
			chat_session.session.messages.clear();

			// Build final message list following /truncate pattern
			let mut final_messages = Vec::new();

			// Add system message first
			if let Some(sys_msg) = system_message {
				final_messages.push(sys_msg);
			}
			// Add initial messages (welcome + instructions) using centralized function
			// Use thread-local if set (ACP/WebSocket), otherwise process cwd
			let current_dir = crate::mcp::get_thread_working_directory();
			if let Ok(initial_messages) =
				crate::session::chat::session::get_initial_messages(config, role, &current_dir)
					.await
			{
				final_messages.extend(initial_messages);
			}

			// Add all command outputs with configured role
			for output_text in &result.outputs {
				let output_msg = crate::session::Message {
					role: command_config.output_role.as_str().to_string(),
					content: output_text.clone(),
					timestamp: std::time::SystemTime::now()
						.duration_since(std::time::UNIX_EPOCH)
						.unwrap_or_default()
						.as_secs(),
					cached: false,
					tool_calls: None,
					tool_call_id: None,
					name: None,
					images: None,
					..Default::default()
				};
				final_messages.push(output_msg);
			}

			// Update session with final messages
			chat_session.session.messages = final_messages;

			// Save session to persist the replacement
			let _ = chat_session.save();
		}
		OutputMode::Last => {
			// Add only the last command output as assistant message to session
			println!(
				"{}",
				"Output mode: last (adding last response only to session)".bright_cyan()
			);

			// Add only the last output as message with configured role to session
			if let Some(last_output) = result.outputs.last() {
				chat_session
					.session
					.add_message(command_config.output_role.as_str(), last_output);
			}

			// Log the last append operation for session restoration
			if let Some(session_file) = &chat_session.session.session_file {
				let log_entry = serde_json::json!({
					"type": "OUTPUT_MODE_LAST",
					"timestamp": std::time::SystemTime::now()
						.duration_since(std::time::UNIX_EPOCH)
						.unwrap_or_default()
						.as_secs(),
					"command": command_name,
					"content_length": result.outputs.last().map(|s| s.len()).unwrap_or(0),
					"total_outputs": result.outputs.len()
				});
				let _ = crate::session::append_to_session_file(
					session_file,
					&serde_json::to_string(&log_entry)?,
				);
			}

			// Save session to persist the new message
			let _ = chat_session.save();
		}
		OutputMode::Restart => {
			// Replace entire session with only the last command output (fresh start)
			println!(
				"{}",
				"Output mode: restart (replacing session with last response only)".bright_cyan()
			);

			// Log the restart operation for session restoration
			if let Some(session_file) = &chat_session.session.session_file {
				let log_entry = serde_json::json!({
					"type": "OUTPUT_MODE_RESTART",
					"timestamp": std::time::SystemTime::now()
						.duration_since(std::time::UNIX_EPOCH)
						.unwrap_or_default()
						.as_secs(),
					"command": command_name,
					"previous_message_count": chat_session.session.messages.len(),
					"content_length": result.outputs.last().map(|s| s.len()).unwrap_or(0),
					"total_outputs": result.outputs.len()
				});
				let _ = crate::session::append_to_session_file(
					session_file,
					&serde_json::to_string(&log_entry)?,
				);
			}

			// Clear existing messages and replace with only the last command output
			chat_session.session.messages.clear();
			if let Some(last_output) = result.outputs.last() {
				chat_session
					.session
					.add_message(command_config.output_role.as_str(), last_output);
			}

			// Save session to persist the replacement
			let _ = chat_session.save();
		}
	}

	Ok(result.outputs.last().unwrap_or(&String::new()).clone())
}

/// List all available command layers for the current role
pub fn list_available_commands(config: &Config, role: &str) -> Vec<String> {
	let (_, _, _, commands_config, _) = config.get_role_config(role);

	commands_config
		.map(|commands| commands.iter().map(|cmd| cmd.name.clone()).collect())
		.unwrap_or_else(Vec::new)
}

/// Check if a command exists for the current role
pub fn command_exists(config: &Config, role: &str, command_name: &str) -> bool {
	let (_, _, _, commands_config, _) = config.get_role_config(role);

	commands_config
		.map(|commands| commands.iter().any(|cmd| cmd.name == command_name))
		.unwrap_or(false)
}

/// Get help text for command layers
pub fn get_command_help(config: &Config, role: &str) -> String {
	let (_, _, _, commands_config, _) = config.get_role_config(role);

	if let Some(commands) = commands_config {
		if commands.is_empty() {
			"No command layers configured.".to_string()
		} else {
			let mut help_text = String::from("Available command layers:\n");
			for command in commands {
				help_text.push_str(&format!(
					"  /run {} - {}\n",
					command.name, command.description
				));
			}
			help_text.push_str("\nUsage: /run <command_name>\nExample: /run reduce");
			help_text
		}
	} else {
		"No command layers configured.".to_string()
	}
}
