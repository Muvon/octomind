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
mod done;
pub use done::handle_done;
mod exit;
mod help;
mod image;
mod info;
mod layers;
mod list;
mod loglevel;
mod mcp;
mod model;
mod plan;
mod prompt;
mod report;
mod role;
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

// Command processing result
#[derive(Debug)]
pub enum CommandResult {
	Handled,          // Command was processed successfully, continue session
	Exit,             // Exit the session
	TreatAsUserInput, // This input should be treated as user input, not a command
}

// Process user commands
pub async fn process_command(
	session: &mut ChatSession,
	input: &str,
	config: &mut Config,
	_role: &str, // Original role - now unused, keeping for API compatibility
	operation_cancelled: tokio::sync::watch::Receiver<bool>,
) -> Result<CommandResult> {
	// Extract command and potential parameters
	let input_parts: Vec<&str> = input.split_whitespace().collect();
	let command = input_parts[0];
	let params = if input_parts.len() > 1 {
		&input_parts[1..]
	} else {
		&[]
	};

	// Use current session role instead of original startup role
	let current_role = session.role.clone();

	match command {
		EXIT_COMMAND | QUIT_COMMAND => {
			exit::handle_exit()?;
			Ok(CommandResult::Exit)
		}
		HELP_COMMAND => {
			help::handle_help(config, &current_role).await?;
			Ok(CommandResult::Handled)
		}
		COPY_COMMAND => {
			copy::handle_copy(&session.last_response)?;
			Ok(CommandResult::Handled)
		}
		CLEAR_COMMAND => {
			clear::handle_clear()?;
			Ok(CommandResult::Handled)
		}
		SAVE_COMMAND => {
			save::handle_save(session)?;
			Ok(CommandResult::Handled)
		}
		INFO_COMMAND => {
			info::handle_info(session)?;
			Ok(CommandResult::Handled)
		}
		REPORT_COMMAND => {
			report::handle_report(session, config)?;
			Ok(CommandResult::Handled)
		}
		CONTEXT_COMMAND => {
			context::handle_context(session, config, params)?;
			Ok(CommandResult::Handled)
		}
		LAYERS_COMMAND => {
			layers::handle_layers(session, config, &current_role).await?;
			Ok(CommandResult::Handled)
		}
		LOGLEVEL_COMMAND => {
			loglevel::handle_loglevel(config, params)?;
			Ok(CommandResult::Handled)
		}
		DONE_COMMAND => {
			// /done is handled directly in runner.rs main loop for session lifecycle management
			// This case should not be reached as /done is intercepted before process_command
			unreachable!("/done command should be handled in runner.rs main loop")
		}
		TRUNCATE_COMMAND => {
			truncate::handle_truncate(session, config, &current_role).await?;
			Ok(CommandResult::Handled)
		}
		SUMMARIZE_COMMAND => {
			summarize::handle_summarize(session, config).await?;
			Ok(CommandResult::Handled)
		}
		CACHE_COMMAND => {
			cache::handle_cache(session, config, params).await?;
			Ok(CommandResult::Handled)
		}
		LIST_COMMAND => {
			list::handle_list(session, config, params)?;
			Ok(CommandResult::Handled)
		}
		MODEL_COMMAND => {
			model::handle_model(session, config, params)?;
			Ok(CommandResult::Handled)
		}
		SESSION_COMMAND => {
			session::handle_session(session, params)?;
			Ok(CommandResult::Handled)
		}
		MCP_COMMAND => {
			mcp::handle_mcp(config, &current_role, params).await?;
			Ok(CommandResult::Handled)
		}
		RUN_COMMAND => {
			run::handle_run(session, config, &current_role, params, operation_cancelled).await?;
			Ok(CommandResult::Handled)
		}
		IMAGE_COMMAND => {
			image::handle_image(session, params).await?;
			Ok(CommandResult::Handled)
		}
		ROLE_COMMAND => {
			role::handle_role(session, config, params).await?;
			Ok(CommandResult::Handled)
		}
		PROMPT_COMMAND => {
			prompt::handle_prompt(session, config, &current_role, params).await?;
			Ok(CommandResult::Handled)
		}
		PLAN_COMMAND => {
			plan::handle_plan().await?;
			Ok(CommandResult::Handled)
		}
		_ => {
			// Unknown command - treat as user input instead of showing error
			Ok(CommandResult::TreatAsUserInput)
		}
	}
}
