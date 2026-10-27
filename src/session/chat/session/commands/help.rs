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
	// Build list of command names for structured output, aliases beside their command
	let mut commands: Vec<String> = BUILTIN_COMMANDS
		.iter()
		.map(|&(name, ..)| match alias_of(name) {
			Some(alias) => format!("{name} | {alias}"),
			None => name.to_string(),
		})
		.collect();

	// Add custom commands from config
	let (_, _, _, commands_config, _) = config.get_role_config(role);
	if let Some(cmds) = commands_config {
		for cmd in cmds {
			commands.push(format!("/run {}", cmd.name));
		}
	}

	Ok(CommandResult::HandledWithOutput(Box::new(CommandOutput::Help {
		commands,
	})))
}
