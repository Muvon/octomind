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

// Help command handler

use super::super::commands::*;
use crate::config::Config;
use crate::session::chat::command_executor;
use anyhow::Result;
use colored::Colorize;

pub async fn handle_help(config: &Config, role: &str) -> Result<bool> {
	println!("{}", "\nAvailable commands:\n".bright_cyan());
	println!("{} - Show this help message", HELP_COMMAND.cyan());
	println!("{} - Copy last response to clipboard", COPY_COMMAND.cyan());
	println!("{} - Clear the screen", CLEAR_COMMAND.cyan());
	println!("{} - Save the session", SAVE_COMMAND.cyan());
	println!(
		"{} - Manage cache checkpoints: /cache [stats|clear|threshold]",
		CACHE_COMMAND.cyan()
	);
	println!(
		"{} [page] - List all available sessions with pagination (default: page 1)",
		LIST_COMMAND.cyan()
	);
	println!("{} [name] - Switch to another session or create a new one (without name creates fresh session)", SESSION_COMMAND.cyan());
	println!(
		"{} - Display detailed token and cost breakdown for this session",
		INFO_COMMAND.cyan()
	);
	println!(
		"{} - Toggle layered processing architecture on/off",
		LAYERS_COMMAND.cyan()
	);
	println!("{} - Finalize task with memorization, summarization, and auto-commit (resets layered processing for next task)", DONE_COMMAND.cyan());
	println!(
		"{} [level] - Set logging level: none, info, or debug",
		LOGLEVEL_COMMAND.cyan()
	);
	println!(
		"{} - Perform smart context truncation to reduce token usage",
		TRUNCATE_COMMAND.cyan()
	);
	println!(
		"{} - Create intelligent summary of entire conversation using local processing",
		SUMMARIZE_COMMAND.cyan()
	);
	println!(
		"{} <command_name> - Execute a command layer",
		RUN_COMMAND.cyan()
	);
	println!(
		"{} [model] - Show current model or change to a different model (runtime only)",
		MODEL_COMMAND.cyan()
	);
	println!(
		"{} [list|info|full] - Show MCP server status and tools (info is default)",
		MCP_COMMAND.cyan()
	);
	println!(
		"{} - Generate detailed usage report with cost breakdown per request",
		REPORT_COMMAND.cyan()
	);
	println!(
		"{} - Display current session context that would be sent to AI",
		CONTEXT_COMMAND.cyan()
	);
	println!(
		"{} <path_or_url> - Attach image to your next message (supports PNG, JPEG, GIF, WebP, BMP)",
		IMAGE_COMMAND.cyan()
	);
	println!(
		"{} or {} - Exit the session\n",
		EXIT_COMMAND.cyan(),
		QUIT_COMMAND.cyan()
	);

	// Add keyboard shortcuts section
	println!("{}", "Keyboard shortcuts:\n".bright_cyan());
	println!(
		"{} - Insert newline for multi-line input",
		"Ctrl+J".bright_green()
	);
	println!("{} - Accept hint/completion", "Ctrl+E".bright_green());
	println!("{} - Cancel input", "Ctrl+C".bright_green());
	println!("{} - Exit session", "Ctrl+D".bright_green());
	println!();

	// Additional info about caching
	println!("{}", "** About Cache Management **".bright_yellow());
	println!(
		"The system message and tool definitions are automatically cached for supported providers."
	);
	println!("Use '/cache' to mark your last user message for caching.");
	println!("Use '/cache stats' to view detailed cache statistics and efficiency.");
	println!("Use '/cache clear' to remove content cache markers (keeps system/tool caches).");
	println!("Use '/cache threshold' to view auto-cache settings.");
	println!("Supports 2-marker system: when you add a 3rd marker, the first one moves to the new position.");
	println!("Automatic caching triggers based on token threshold (configurable).");
	println!("Cached tokens reduce costs on subsequent requests with the same content.\n");

	// Add information about layered architecture
	println!("{}", "** About Layered Processing **".bright_yellow());
	println!("The layered architecture processes your initial query through multiple AI layers:");
	println!("1. Query Processor: Improves your initial query");
	println!("2. Context Generator: Gathers relevant context information");
	println!("3. Developer: Executes the actual development work");
	println!("The Reducer functionality is available through the /done command.");
	println!("Only the first message in a session uses the full layered architecture.");
	println!("Subsequent messages use direct communication with the developer model.");
	println!("Use the /done command to optimize context, apply EditorConfig formatting to edited files, and restart the layered pipeline.");
	println!("Toggle layered processing with /layers command.\n");

	// Add information about command layers
	println!("{}", "** About Command Layers **".bright_yellow());
	println!("Command layers are specialized AI helpers that can be invoked without affecting the session history.");
	println!("Commands are defined in the [[commands]] section of your configuration file.");
	println!("Example usage: /run estimate - runs the 'estimate' command layer");
	println!(
		"Command layers use the same infrastructure as normal layers but don't store context."
	);
	println!("This allows you to get specialized help without cluttering your conversation.\n");

	// Show available commands for current role
	let available_commands = command_executor::list_available_commands(config, role);
	if available_commands.is_empty() {
		println!("{}", "No command layers configured.".bright_blue());
		println!("Use '/run' to see configuration examples.\n");
	} else {
		println!("{}", "Available command layers:".bright_blue());
		for cmd in &available_commands {
			println!("  {} {}", "/run".cyan(), cmd.bright_yellow());
		}
		println!();
	}

	Ok(false)
}
