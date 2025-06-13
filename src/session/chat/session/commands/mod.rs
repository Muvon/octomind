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
mod reduce;
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
		REDUCE_COMMAND => reduce::handle_reduce(session, config).await,
		CACHE_COMMAND => cache::handle_cache(session, config, params).await,
		LIST_COMMAND => list::handle_list(session, params),
		MODEL_COMMAND => model::handle_model(session, config, params),
		SESSION_COMMAND => session::handle_session(session, params),
		MCP_COMMAND => mcp::handle_mcp(config, role, params).await,
		RUN_COMMAND => run::handle_run(session, config, role, params).await,
		IMAGE_COMMAND => image::handle_image(session, params).await,
		_ => Ok(false), // Not a command
	}
}
