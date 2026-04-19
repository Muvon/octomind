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

// Help command handler

use super::super::commands::*;
use super::{CommandOutput, CommandResult};
use crate::config::Config;
use anyhow::Result;

pub async fn handle_help(config: &Config, role: &str) -> Result<CommandResult> {
	// Build list of command names for structured output
	let mut commands = Vec::new();

	commands.push(HELP_COMMAND.to_string());
	commands.push(COPY_COMMAND.to_string());
	commands.push(CLEAR_COMMAND.to_string());
	commands.push(LIST_COMMAND.to_string());
	commands.push(SESSION_COMMAND.to_string());
	commands.push(INFO_COMMAND.to_string());
	commands.push(DONE_COMMAND.to_string());
	commands.push(WORKFLOW_COMMAND.to_string());

	commands.push(LOGLEVEL_COMMAND.to_string());

	commands.push(TRUNCATE_COMMAND.to_string());
	commands.push(SUMMARIZE_COMMAND.to_string());
	commands.push(RUN_COMMAND.to_string());
	commands.push(CONTEXT_COMMAND.to_string());
	commands.push(MODEL_COMMAND.to_string());
	commands.push(ROLE_COMMAND.to_string());
	commands.push(MCP_COMMAND.to_string());
	commands.push(IMAGE_COMMAND.to_string());
	commands.push(VIDEO_COMMAND.to_string());
	commands.push(PROMPT_COMMAND.to_string());
	commands.push(PLAN_COMMAND.to_string());
	commands.push(SKILL_COMMAND.to_string());
	commands.push(REPORT_COMMAND.to_string());
	commands.push(format!("{} | {}", EXIT_COMMAND, QUIT_COMMAND));

	// Add custom commands from config
	let (_, _, _, commands_config, _) = config.get_role_config(role);
	if let Some(cmds) = commands_config {
		for cmd in cmds {
			commands.push(format!("/run {}", cmd.name));
		}
	}

	Ok(CommandResult::HandledWithOutput(Box::new(
		CommandOutput::Help { commands },
	)))
}
