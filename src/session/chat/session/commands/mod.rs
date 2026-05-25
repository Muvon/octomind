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

// Session command processing - refactored into separate modules

mod clear;
mod context;
mod copy;
mod display;
mod done;
pub use done::{handle_done, DoneOutcome};
mod analyze;
mod effort;
mod exit;
mod help;
mod image;
mod info;
mod learning;
mod list;
mod loglevel;
mod mcp;
mod model;
mod plan;
mod prompt;
mod report;
mod role;
mod run;
mod schedule;
mod session;
mod share;
mod skill;
mod utils;
mod video;

use super::super::commands::*;
use super::core::ChatSession;
use crate::config::Config;
use crate::session::chat::tool_display::{
	block_close_err, block_close_ok, block_line, block_open, block_row, key_width,
};
use anyhow::Result;
use colored::Colorize;
use serde::Serialize;

// Strongly-typed command outputs
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "command_type", rename_all = "snake_case")]
pub enum CommandOutput {
	Help {
		commands: Vec<String>,
	},
	Info {
		session_name: String,
		model: String,
		role: String,
		tokens_input: u64,
		tokens_output: u64,
		tokens_used: u64,
		tokens_cached: u64,
		tokens_cache_write: u64,
		tokens_reasoning: u64,
		total_cost: f64,
		cache_savings: f64,
		tokens_per_second: f64,
		avg_tokens_per_compression: f64,
		avg_tokens_per_tool: f64,
		avg_tokens_per_response: f64,
		avg_input_tokens: f64,
		compression_stats: Option<crate::session::CompressionStats>,
		// Cache marker stats (from CacheManager)
		cache_markers_system: u64,
		cache_markers_tool: u64,
		cache_markers_content: u64,
		cache_non_cached_tokens: u64,
	},
	Model {
		old_model: Option<String>,
		new_model: String,
		changed: bool,
		saved: Option<bool>,
		save_error: Option<String>,
	},
	Effort {
		old_effort: Option<String>,
		new_effort: String,
		changed: bool,
		saved: Option<bool>,
		save_error: Option<String>,
	},
	Role {
		old_role: Option<String>,
		new_role: String,
		current_role: Option<String>,
		available_roles: Option<Vec<String>>,
		changed: bool,
		saved: Option<bool>,
		save_error: Option<String>,
	},
	Loglevel {
		old_level: Option<String>,
		new_level: Option<String>,
		current_level: Option<String>,
		available_levels: Vec<String>,
		changed: bool,
	},
	Copy {
		copied: bool,
		length: Option<usize>,
	},
	Clear {
		success: bool,
		message: String,
	},
	Plan {
		has_plan: bool,
		plan: Option<serde_json::Value>,
		display: Option<String>,
		// Critical knowledge entries accumulated from conversation compressions.
		// Empty when the session has never been compressed.
		knowledge: Vec<String>,
	},
	Context {
		filter: String,
		total_messages: usize,
		filtered_messages: Vec<serde_json::Value>,
	},
	Image {
		image_attached: bool,
		path: Option<String>,
		error: Option<String>,
	},
	Video {
		video_attached: bool,
		path: Option<String>,
		error: Option<String>,
	},
	Prompt {
		data: serde_json::Value,
	},
	Done {
		done: bool,
		memorized: Option<bool>,
		summarized: Option<bool>,
		saved: Option<bool>,
	},
	List {
		sessions: Vec<serde_json::Value>,
		total_sessions: usize,
		page: usize,
		total_pages: usize,
		plain_text: Option<String>,
	},
	Run {
		command_executed: String,
		data: serde_json::Value,
	},
	Mcp {
		mcp_command: String,
		data: serde_json::Value,
	},
	Report {
		entries: Vec<serde_json::Value>,
		totals: serde_json::Value,
	},
	Session {
		switched: bool,
		session_name: String,
	},
	Skill {
		data: serde_json::Value,
	},
	Schedule {
		data: serde_json::Value,
	},
	Learning {
		data: serde_json::Value,
	},
	Share {
		id: String,
		url: String,
	},
	Analyze {
		url: String,
		port: u16,
		token: String,
	},
	Error {
		error: String,
		context: Option<serde_json::Value>,
	},
}

impl CommandOutput {
	/// Convert to JSON for WebSocket/API clients
	pub fn to_json(&self) -> serde_json::Value {
		serde_json::to_value(self).unwrap_or_else(|e| {
			serde_json::json!({
				"command_type": "error",
				"error": format!("Failed to serialize command output: {}", e)
			})
		})
	}

	/// Display output in CLI mode
	pub async fn display_cli(&mut self, session: &mut ChatSession, config: &Config) {
		match self {
			Self::Help { .. } => display::display_help(self, config),
			Self::Info { .. } => display::display_info(self),
			Self::Model { .. } => display::display_model(self),
			Self::Effort { .. } => display::display_effort(self),
			Self::Role { .. } => display::display_role(self),
			Self::Loglevel { .. } => display::display_loglevel(self),
			Self::Copy { copied, length } => {
				block_open("/copy", None);
				if *copied {
					if let Some(len) = length {
						let kw = key_width(["copied"]);
						block_row(
							"copied",
							&format!("{} chars to clipboard", len)
								.bright_green()
								.to_string(),
							kw,
						);
						block_close_ok("/copy", Some(&format!("{} chars", len)));
					} else {
						block_close_ok("/copy", Some("clipboard"));
					}
				} else {
					block_line(&"Nothing to copy yet.".yellow().to_string());
					block_close_ok("/copy", Some("empty"));
				}
				println!();
			}
			Self::Clear { message, .. } => {
				print!("\x1B[2J\x1B[1;1H");
				std::io::Write::flush(&mut std::io::stdout()).unwrap_or(());
				block_open("/clear", None);
				if !message.is_empty() {
					block_line(message);
				}
				block_close_ok("/clear", Some("screen reset"));
				println!();
			}
			Self::Plan { .. } => display::display_plan(self),
			Self::Context { .. } => display::display_context(self, session, config).await,
			Self::Image { .. } => display::display_image(self),
			Self::Video { .. } => display::display_video(self),
			Self::Prompt { .. } => display::display_prompt(self),
			Self::Done { .. } => display::display_done(self),
			Self::List { .. } => display::display_list(self, config),

			Self::Run { .. } => display::display_run(self, config, &session.role),
			Self::Mcp { .. } => display::display_mcp(self),
			Self::Report { .. } => display::display_report(self, config),
			Self::Session { .. } => display::display_session(self),
			Self::Skill { .. } => display::display_skill(self),
			Self::Schedule { .. } => display::display_schedule(self),
			Self::Learning { .. } => display::display_learning(self),
			Self::Share { .. } => display::display_share(self),
			Self::Analyze { .. } => display::display_analyze(self),
			Self::Error { error, .. } => {
				block_open("/error", None);
				block_close_err("error", error);
				println!();
			}
		}
	}
}

// Command processing result
#[derive(Debug)]
pub enum CommandResult {
	Handled,                               // Command was processed successfully, continue session
	HandledWithOutput(Box<CommandOutput>), // Command was processed with typed output
	Exit,                                  // Exit the session
	TreatAsUserInput,                      // This input should be treated as user input, not a command
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
		HELP_COMMAND => help::handle_help(config, &current_role).await,
		COPY_COMMAND => copy::handle_copy(&session.last_response),
		CLEAR_COMMAND => clear::handle_clear(),
		INFO_COMMAND => info::handle_info(session, config),
		REPORT_COMMAND => report::handle_report(session, config),

		CONTEXT_COMMAND => context::handle_context(session, params),
		LOGLEVEL_COMMAND => loglevel::handle_loglevel(config, params),
		DONE_COMMAND => {
			// /done is handled directly in runner.rs main loop for session lifecycle management
			// This case should not be reached as /done is intercepted before process_command
			unreachable!("/done command should be handled in runner.rs main loop")
		}
		LIST_COMMAND => list::handle_list(session, config, params),
		MODEL_COMMAND => model::handle_model(session, config, params),
		EFFORT_COMMAND => effort::handle_effort(session, config, params),
		SESSION_COMMAND => session::handle_session(session, params),
		MCP_COMMAND => mcp::handle_mcp(config, &current_role, params).await,
		RUN_COMMAND => {
			run::handle_run(session, config, &current_role, params, operation_cancelled).await
		}

		IMAGE_COMMAND => image::handle_image(session, params).await,
		VIDEO_COMMAND => video::handle_video(session, params).await,
		ROLE_COMMAND => role::handle_role(session, config, params).await,
		PROMPT_COMMAND => prompt::handle_prompt(session, config, &current_role, params).await,
		PLAN_COMMAND => plan::handle_plan(session).await,
		SKILL_COMMAND => skill::handle_skill(session, params).await,
		SCHEDULE_COMMAND => schedule::handle_schedule(input, params).await,
		LEARNING_COMMAND => learning::handle_learning(session, config, params).await,
		SHARE_COMMAND => share::handle_share(session).await,
		ANALYZE_COMMAND => analyze::handle_analyze(session).await,
		_ => {
			// Unknown command - treat as user input instead of showing error
			Ok(CommandResult::TreatAsUserInput)
		}
	}
}
