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

// Command executor for /run commands using layers

use crate::config::Config;
use crate::session::chat::format_number;
use crate::session::chat::session::ChatSession;
use crate::session::{layers::layer_trait::Layer, layers::GenericLayer};
use anyhow::Result;
use colored::Colorize;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// Execute a command layer without storing it in the session history
pub async fn execute_command_layer(
	command_name: &str,
	provided_input: &str,
	chat_session: &mut ChatSession,
	config: &Config,
	role: &str,
	operation_cancelled: Arc<AtomicBool>,
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
			"model": command_config.get_effective_model(&chat_session.session.info.model),
			"temperature": command_config.temperature,
			"input_mode": format!("{:?}", command_config.input_mode),
			"mcp_enabled": !command_config.mcp.server_refs.is_empty()
		}
		});
		let _ = crate::session::append_to_session_file(
			session_file,
			&serde_json::to_string(&log_entry)?,
		);
	}

	// Create a generic layer with the command configuration
	let command_layer = GenericLayer::new(command_config.clone());

	// Prepare the input according to the command's input_mode
	// CRITICAL FIX: Always use prepare_input to respect the input_mode setting
	// The input_mode determines what context the command should receive:
	// - "last": Get the last assistant response from session
	// - "all": Get all conversation context
	// - "summary": Get a summarized version
	let processed_input = match command_config.input_mode {
		crate::session::layers::layer_trait::InputMode::Last => {
			// For "Last" mode, always use prepare_input to get the last assistant response
			// If explicit input is provided, it will be combined with the last assistant context
			command_layer.prepare_input(provided_input, &chat_session.session)
		}
		crate::session::layers::layer_trait::InputMode::All => {
			// For "All" mode, use prepare_input to format the full conversation context
			command_layer.prepare_input(provided_input, &chat_session.session)
		}
		crate::session::layers::layer_trait::InputMode::Summary => {
			// For "Summary" mode, use prepare_input to generate a summary
			command_layer.prepare_input(provided_input, &chat_session.session)
		}
	};

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
			"output_length": result.output.len(),
			"usage": result.token_usage
		});
		let _ = crate::session::append_to_session_file(
			session_file,
			&serde_json::to_string(&log_entry)?,
		);
	}

	// Add command statistics to the session
	if let Some(usage) = &result.token_usage {
		let effective_model = command_config.get_effective_model(&chat_session.session.info.model);
		let cost = usage.cost.unwrap_or(0.0);

		// Add the stats to the session with a special prefix for commands
		chat_session.session.add_layer_stats(
			&format!("command:{}", command_name),
			&effective_model,
			usage.prompt_tokens,
			usage.output_tokens,
			cost,
		);

		// Save the session to persist the statistics
		let _ = chat_session.save();

		// Display information about the command execution
		println!(
			"{} {} prompt, {} completion tokens",
			"Command usage:".bright_blue(),
			format_number(usage.prompt_tokens).bright_green(),
			format_number(usage.output_tokens).bright_green()
		);

		if cost > 0.0 {
			println!(
				"{} ${:.5}",
				"Command cost:".bright_blue(),
				cost.to_string().bright_magenta()
			);
		}
	}

	Ok(result.output)
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
	let available_commands = list_available_commands(config, role);

	if available_commands.is_empty() {
		"No command layers configured.".to_string()
	} else {
		format!(
			"Available command layers: {}\nUsage: /run <command_name>\nExample: /run estimate",
			available_commands.join(", ")
		)
	}
}
