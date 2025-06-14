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

// Session command processing - refactored into separate modules

mod cache;
mod clear;
mod context;
mod copy;
mod exit;
mod help;
mod image;
mod info;
mod layers;
mod list;
mod loglevel;
mod mcp;
mod model;
mod report;
mod run;
mod save;
mod session;
mod summarize;
mod truncate;
mod utils;

use super::super::commands::*;
use super::core::ChatSession;
use crate::config::Config;
use anyhow::Result;

// Process user commands
pub async fn process_command(
	session: &mut ChatSession,
	input: &str,
	config: &mut Config,
	role: &str,
) -> Result<bool> {
	// Extract command and potential parameters
	let input_parts: Vec<&str> = input.split_whitespace().collect();
	let command = input_parts[0];
	let params = if input_parts.len() > 1 {
		&input_parts[1..]
	} else {
		&[]
	};

	match command {
		EXIT_COMMAND | QUIT_COMMAND => exit::handle_exit(),
		HELP_COMMAND => help::handle_help(config, role).await,
		COPY_COMMAND => copy::handle_copy(&session.last_response),
		CLEAR_COMMAND => clear::handle_clear(),
		SAVE_COMMAND => save::handle_save(session),
		INFO_COMMAND => info::handle_info(session),
		REPORT_COMMAND => report::handle_report(session, config),
		CONTEXT_COMMAND => context::handle_context(session, config),
		LAYERS_COMMAND => layers::handle_layers(session, config, role).await,
		LOGLEVEL_COMMAND => loglevel::handle_loglevel(config, params),
		TRUNCATE_COMMAND => truncate::handle_truncate(session, config).await,
		SUMMARIZE_COMMAND => summarize::handle_summarize(session, config).await,
		CACHE_COMMAND => cache::handle_cache(session, config, params).await,
		LIST_COMMAND => list::handle_list(session, config, params),
		MODEL_COMMAND => model::handle_model(session, config, params),
		SESSION_COMMAND => session::handle_session(session, params),
		MCP_COMMAND => mcp::handle_mcp(config, role, params).await,
		RUN_COMMAND => run::handle_run(session, config, role, params).await,
		IMAGE_COMMAND => image::handle_image(session, params).await,
		_ => handle_unknown_command(command, config, role).await,
	}
}

// Handle unknown commands by showing error and available commands
async fn handle_unknown_command(command: &str, config: &Config, role: &str) -> Result<bool> {
	use colored::Colorize;

	// Show error message
	println!(
		"{}: {}",
		"Unknown command".bright_red(),
		command.bright_yellow()
	);

	// Show available commands
	println!("\n{}", "Available commands:".bright_cyan());

	// Basic session commands
	println!("{} - Show help and available commands", HELP_COMMAND.cyan());
	println!("{} - Display token usage and costs", INFO_COMMAND.cyan());
	println!("{} - Generate detailed usage report", REPORT_COMMAND.cyan());
	println!("{} - Copy last response to clipboard", COPY_COMMAND.cyan());
	println!("{} - Clear the screen", CLEAR_COMMAND.cyan());
	println!("{} - Save the session", SAVE_COMMAND.cyan());
	println!("{} - List all sessions", LIST_COMMAND.cyan());
	println!("{} - Switch to another session", SESSION_COMMAND.cyan());
	println!("{} - Show/change current model", MODEL_COMMAND.cyan());
	println!("{} - Set logging level", LOGLEVEL_COMMAND.cyan());

	// Advanced commands
	println!("{} - Toggle layered processing", LAYERS_COMMAND.cyan());
	println!("{} - Optimize session context", DONE_COMMAND.cyan());
	println!("{} - Smart context truncation", TRUNCATE_COMMAND.cyan());
	println!("{} - Summarize conversation", SUMMARIZE_COMMAND.cyan());
	println!("{} - Manage cache checkpoints", CACHE_COMMAND.cyan());
	println!("{} - Display session context", CONTEXT_COMMAND.cyan());
	println!("{} - Show MCP server status", MCP_COMMAND.cyan());
	println!("{} - Execute command layer", RUN_COMMAND.cyan());
	println!("{} - Attach image to message", IMAGE_COMMAND.cyan());
	println!(
		"{}/{} - Exit the session",
		EXIT_COMMAND.cyan(),
		QUIT_COMMAND.cyan()
	);

	// Show command layers if available
	let available_commands =
		crate::session::chat::command_executor::list_available_commands(config, role);
	if !available_commands.is_empty() {
		println!("\n{}", "Available command layers:".bright_blue());
		for cmd in &available_commands {
			println!("  {} {}", "/run".cyan(), cmd.bright_yellow());
		}
	}

	println!(
		"\n💡 Type {} for detailed help with examples",
		"/help".bright_green()
	);

	Ok(false) // Command was handled, don't exit
}
