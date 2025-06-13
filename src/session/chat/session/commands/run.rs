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
use crate::config::Config;
use crate::session::chat::assistant_output::print_assistant_response;
use crate::session::chat::command_executor;
use anyhow::Result;
use colored::Colorize;

pub async fn handle_run(
	session: &mut ChatSession,
	config: &Config,
	role: &str,
	params: &[&str],
) -> Result<bool> {
	// Handle /run command for executing command layers
	if params.is_empty() {
		// Show available commands for this role
		let available_commands = command_executor::list_available_commands(config, role);
		if available_commands.is_empty() {
			println!("{}", "No command layers configured.".bright_yellow());
			println!("{}", "Command layers can be defined in the global [[commands]] section of your configuration.".bright_blue());
			println!("{}", "Example configuration:".bright_cyan());
			println!(
				"{}",
				r#"[[commands]]
name = "estimate"
model = "openrouter:openai/gpt-4.1-mini"
system_prompt = "You are a project estimation expert. Analyze the work done and provide estimates."
temperature = 0.2
input_mode = "Last"

[commands.mcp]
server_refs = ["developer", "filesystem"]
allowed_tools = []"#
					.bright_white()
			);
		} else {
			println!("{}", "Available command layers:".bright_cyan());
			for cmd in &available_commands {
				println!("  {} {}", "/run".cyan(), cmd.bright_yellow());
			}
			println!();
			println!("{}", "Usage: /run <command_name>".bright_blue());
			println!("{}", "Example: /run estimate".bright_green());
		}
		return Ok(false);
	}

	let command_name = params[0];

	// Check if command exists
	if !command_executor::command_exists(config, role, command_name) {
		let available_commands = command_executor::list_available_commands(config, role);
		println!(
			"{} {}",
			"Command not found:".bright_red(),
			command_name.bright_yellow()
		);
		if !available_commands.is_empty() {
			println!("{}", "Available commands:".bright_cyan());
			for cmd in &available_commands {
				println!("  {}", cmd.bright_yellow());
			}
		}
		return Ok(false);
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
			.filter(|m| m.role == "user")
			.next_back()
			.map(|m| m.content.clone())
			.unwrap_or_else(|| "No recent user input found".to_string())
	};

	// Execute the command layer
	println!();
	let operation_cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
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
		Ok(result) => {
			println!();
			println!("{}", "Command result:".bright_green());
			// Use markdown-aware printing for command results
			print_assistant_response(&result, config, role);
			println!();
		}
		Err(e) => {
			println!("{} {}", "Command execution failed:".bright_red(), e);
		}
	}

	Ok(false)
}
