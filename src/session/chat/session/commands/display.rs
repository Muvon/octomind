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

//! Display functions for command output in CLI mode
//!
//! This module contains all the formatting logic for displaying command results
//! in the terminal. Commands return strongly-typed CommandOutput enums, and these
//! functions format that output for human-readable CLI display.
//!
//! WebSocket mode sends the raw JSON without using these display functions.

use super::CommandOutput;
use crate::config::Config;
use colored::Colorize;

// Note: Main display routing is now in CommandOutput::display_cli()
// These functions handle the actual formatting

pub fn display_help(output: &CommandOutput, config: &Config) {
	if let CommandOutput::Help { .. } = output {
		use crate::session::chat::commands::*;

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
			"{} - Finalize task with memorization, summarization, and auto-commit",
			DONE_COMMAND.cyan()
		);

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
			"{} <workflow_name> [input] - Execute a workflow",
			WORKFLOW_COMMAND.cyan()
		);
		println!(
			"{} [filter] - Display session context: all, assistant, user, tool, or large",
			CONTEXT_COMMAND.cyan()
		);
		println!(
			"{} [model] - View or change current AI model (runtime only)",
			MODEL_COMMAND.cyan()
		);
		println!(
			"{} [role] - View or change current role (runtime only)",
			ROLE_COMMAND.cyan()
		);
		println!(
			"{} [subcommand] - MCP server management: info (default), list, full, health, dump, validate",
			MCP_COMMAND.cyan()
		);
		println!(
			"{} <path> - Attach image to next message (PNG, JPEG, GIF, WebP, BMP)",
			IMAGE_COMMAND.cyan()
		);
		println!(
			"{} [template_name] - Manage prompt templates",
			PROMPT_COMMAND.cyan()
		);
		println!(
			"{} - Display current plan stored in MCP plan tool",
			PLAN_COMMAND.cyan()
		);
		println!(
			"{} - Generate detailed usage report for this session",
			REPORT_COMMAND.cyan()
		);
		println!(
			"{} | {} - Exit the session",
			EXIT_COMMAND.cyan(),
			QUIT_COMMAND.cyan()
		);

		// Display custom commands if any
		if let Some(commands) = config.commands.as_ref() {
			if !commands.is_empty() {
				println!("\n{}", "Custom Commands:".bright_green());
				for cmd in commands {
					println!(
						"  {} {} - {}",
						"/run".cyan(),
						cmd.name.bright_white(),
						cmd.description
					);
				}
			}
		}

		// Display workflows if any
		if !config.workflows.is_empty() {
			println!("\n{}", "Workflows:".bright_green());
			for workflow in &config.workflows {
				println!(
					"  {} {} - {}",
					"/workflow".cyan(),
					workflow.name.bright_white(),
					workflow.description
				);
			}
		}

		println!();
	}
}

pub fn display_loglevel(output: &CommandOutput) {
	if let CommandOutput::Loglevel {
		old_level: _,
		new_level,
		current_level,
		available_levels,
		changed,
	} = output
	{
		if *changed {
			if let Some(level) = new_level {
				match level.as_str() {
					"none" => {
						println!("{}", "Log level set to NONE (runtime only).".bright_green());
						println!("{}", "No logging will be shown.".bright_yellow());
					}
					"debug" => {
						println!(
							"{}",
							"Log level set to DEBUG (runtime only).".bright_green()
						);
						println!(
							"{}",
							"Detailed debug information will be shown.".bright_yellow()
						);
					}
					_ => {
						println!("{}", "Log level set to INFO (runtime only).".bright_green());
						println!("{}", "Moderate logging will be shown.".bright_yellow());
					}
				}
			}
			println!(
				"{}",
				"Note: This change only affects the current session.".bright_blue()
			);
		} else if let Some(current) = current_level {
			println!(
				"{} {}",
				"Current log level:".bright_cyan(),
				current.bright_white()
			);
			println!();
			println!(
				"{}",
				format!("Available levels: {}", available_levels.join(", ")).bright_yellow()
			);
			println!(
				"{}",
				"Usage: /loglevel <level> (e.g., /loglevel debug)".bright_blue()
			);
		}
		println!();
	}
}

pub fn display_model(output: &CommandOutput) {
	if let CommandOutput::Model {
		old_model,
		new_model,
		changed,
		saved,
		save_error,
	} = output
	{
		if *changed {
			if let Some(old) = old_model {
				println!(
					"{} {} → {}",
					"Model changed:".bright_green(),
					old.bright_yellow(),
					new_model.bright_green()
				);
			} else {
				println!(
					"{} {}",
					"Model set to:".bright_green(),
					new_model.bright_white()
				);
			}
			println!(
				"{}",
				"Note: This change only affects the current session.".bright_blue()
			);

			if let Some(false) = saved {
				if let Some(err) = save_error {
					println!(
						"{} {}",
						"Warning: Could not save session:".bright_red(),
						err
					);
				}
			}
		} else {
			println!(
				"{} {}",
				"Current model:".bright_cyan(),
				new_model.bright_white()
			);
			println!();
			println!("{}", "Available models:".bright_yellow());
			println!(
				"{}",
				"  - openrouter:anthropic/claude-sonnet-4".bright_white()
			);
			println!("{}", "  - openai:gpt-4o".bright_white());
			println!();
			println!(
				"{}",
				"Usage: /model <provider:model> (e.g., /model openai:gpt-4o)".bright_blue()
			);
		}
		println!();
	}
}

pub fn display_role(output: &CommandOutput) {
	if let CommandOutput::Role {
		old_role,
		new_role,
		current_role,
		available_roles,
		changed,
		saved,
		save_error,
	} = output
	{
		if *changed {
			if let Some(old) = old_role {
				println!(
					"{} {} → {}",
					"Role switched:".bright_green(),
					old.bright_yellow(),
					new_role.bright_green()
				);
			}
			println!(
				"{}",
				"Note: This change only affects the current session.".bright_blue()
			);

			if let Some(false) = saved {
				if let Some(err) = save_error {
					println!(
						"{} {}",
						"Warning: Could not save session:".bright_red(),
						err
					);
				}
			}
		} else if let Some(current) = current_role {
			println!(
				"{} {}",
				"Current role:".bright_cyan(),
				current.bright_white()
			);

			if let Some(roles) = available_roles {
				println!("\n{}", "Available roles:".bright_cyan());
				for role_name in roles {
					let indicator = if role_name == current { "→" } else { " " };
					println!("  {} {}", indicator, role_name.bright_white());
				}
				println!("\n💡 Usage: {} <role_name>", "/role".bright_green());
			}
		}
		println!();
	}
}

pub fn display_plan(output: &CommandOutput) {
	if let CommandOutput::Plan {
		has_plan,
		plan: _,
		display,
	} = output
	{
		if *has_plan {
			if let Some(display_text) = display {
				println!("{}", display_text);
			}
		} else {
			println!("{}", "No active plan. Use plan(command=\"start\", title=\"...\", tasks=[...]) to create one.".bright_yellow());
			println!(
				"{}",
				"Plans are useful for complex, multi-step tasks that require careful coordination."
					.bright_blue()
			);
			println!();
			println!("For simple tasks, just execute them directly without creating a plan");
		}
	}
}

pub fn display_truncate(output: &CommandOutput) {
	if let CommandOutput::Truncate {
		success,
		tokens_before,
		tokens_after,
		tokens_saved,
	} = output
	{
		if *success {
			println!("{}", "Context truncated successfully.".bright_green());
			println!(
				"{} {} → {}",
				"Tokens:".bright_cyan(),
				tokens_before,
				tokens_after
			);
			if *tokens_saved > 0 {
				println!("{} {}", "Tokens saved:".bright_cyan(), tokens_saved);
			}
		}
		println!();
	}
}

pub fn display_summarize(output: &CommandOutput) {
	if let CommandOutput::Summarize {
		success,
		tokens_before,
		tokens_after,
		tokens_saved,
		..
	} = output
	{
		if *success {
			println!("{}", "Smart summarization completed.".bright_green());
			println!(
				"{} {} → {}",
				"Tokens:".bright_cyan(),
				tokens_before,
				tokens_after
			);
			println!("{} {}", "Tokens saved:".bright_cyan(), tokens_saved);
		}
		println!();
	}
}

pub fn display_cache(output: &CommandOutput) {
	if let CommandOutput::Cache {
		cache_command,
		data,
	} = output
	{
		match cache_command.as_str() {
			"check_support" => {
				println!("{}", "This model does not support caching.".bright_yellow());
			}
			"cache_next_message" => {
				println!(
					"{}",
					"The next user message will be marked for caching.".bright_green()
				);
				if let Some(stats) = data.get("statistics") {
					// Format cache statistics
					let system_markers = stats
						.get("system_markers")
						.and_then(|v| v.as_u64())
						.unwrap_or(0);
					let tool_markers = stats
						.get("tool_markers")
						.and_then(|v| v.as_u64())
						.unwrap_or(0);
					let content_markers = stats
						.get("content_markers")
						.and_then(|v| v.as_u64())
						.unwrap_or(0);
					let total_cached = stats
						.get("total_cached_tokens")
						.and_then(|v| v.as_u64())
						.unwrap_or(0);
					let current_non_cached = stats
						.get("current_non_cached_tokens")
						.and_then(|v| v.as_u64())
						.unwrap_or(0);

					println!("\n{}", "Cache Statistics:".bright_cyan());
					println!("  System markers: {}", system_markers);
					println!("  Tool markers: {}", tool_markers);
					println!("  Content markers: {}", content_markers);
					println!("  Total cached tokens: {}", total_cached);
					println!("  Current non-cached tokens: {}", current_non_cached);

				}
			}
			"stats" => {
				if let Some(stats) = data.get("statistics") {
					let system_markers = stats
						.get("system_markers")
						.and_then(|v| v.as_u64())
						.unwrap_or(0);
					let tool_markers = stats
						.get("tool_markers")
						.and_then(|v| v.as_u64())
						.unwrap_or(0);
					let content_markers = stats
						.get("content_markers")
						.and_then(|v| v.as_u64())
						.unwrap_or(0);
					let total_cached = stats
						.get("total_cached_tokens")
						.and_then(|v| v.as_u64())
						.unwrap_or(0);
					let current_non_cached = stats
						.get("current_non_cached_tokens")
						.and_then(|v| v.as_u64())
						.unwrap_or(0);

					println!("{}", "Cache Statistics:".bright_cyan());
					println!("  System markers: {}", system_markers);
					println!("  Tool markers: {}", tool_markers);
					println!("  Content markers: {}", content_markers);
					println!("  Total cached tokens: {}", total_cached);
					println!("  Current non-cached tokens: {}", current_non_cached);


				}
			}
			"clear" => {
				if let Some(cleared) = data.get("cleared_markers").and_then(|v| v.as_u64()) {
					if cleared > 0 {
						println!(
							"{}",
							format!("Cleared {} content cache markers", cleared).bright_green()
						);
					} else {
						println!("{}", "No content cache markers to clear".bright_yellow());
					}
				}
			}
			"threshold" => {
				if let Some(threshold) = data.get("cache_tokens_threshold").and_then(|v| v.as_u64())
				{
					if threshold > 0 {
						println!(
							"{}",
							format!("Current auto-cache threshold: {} tokens", threshold)
								.bright_cyan()
						);
						println!(
							"{}",
							format!(
								"Auto-cache will trigger when non-cached tokens reach {} tokens",
								threshold
							)
							.bright_blue()
						);
					} else {
						println!(
							"{}",
							"Auto-cache is disabled (threshold set to 0)".bright_yellow()
						);
					}
				}

				if let Some(timeout) = data.get("cache_timeout_seconds").and_then(|v| v.as_u64()) {
					if timeout > 0 {
						let timeout_minutes = timeout / 60;
						println!(
							"{}",
							format!(
								"Time-based auto-cache: {} seconds ({} minutes)",
								timeout, timeout_minutes
							)
							.bright_green()
						);
						println!(
							"{}",
							format!(
								"Auto-cache will trigger if {} minutes pass since last checkpoint",
								timeout_minutes
							)
							.bright_blue()
						);
					} else {
						println!("{}", "Time-based auto-cache is disabled".bright_yellow());
					}
				}
			}
			"error" => {
				println!("{}", "Invalid cache command. Usage:".bright_red());
				println!(
					"{}",
					"  /cache - Add cache checkpoint at last user message".cyan()
				);
				println!(
					"{}",
					"  /cache stats - Show detailed cache statistics".cyan()
				);
				println!("{}", "  /cache clear - Clear content cache markers".cyan());
				println!(
					"{}",
					"  /cache threshold - Show auto-cache threshold settings".cyan()
				);
			}
			_ => {}
		}
	}
}

pub async fn display_context(
	output: &CommandOutput,
	session: &mut super::super::core::ChatSession,
	config: &Config,
) {
	if let CommandOutput::Context { filter, .. } = output {
		// Display current session context with filtering (CLI output only)
		session
			.display_session_context_filtered(config, filter)
			.await;
	}
}

pub fn display_image(output: &CommandOutput) {
	if let CommandOutput::Image {
		image_attached,
		path,
		error,
	} = output
	{
		if *image_attached {
			println!("{}", "✅ Image attached successfully!".bright_green());
			if let Some(p) = path {
				if p != "clipboard" {
					println!("{} {}", "Path:".bright_cyan(), p.bright_white());
				}
			}
			println!(
				"{}",
				"Your next message will include this image.".bright_cyan()
			);
		} else if let Some(err) = error {
			println!("{}: {}", "❌ Failed to attach image".bright_red(), err);
		} else {
			// Show usage
			println!("{}", "Usage: /image <path_to_image_or_url>".bright_yellow());
			println!("{}", "Examples:".bright_blue());
			println!("{}", "  /image screenshot.png".bright_white());
			println!("{}", "  /image /path/to/image.jpg".bright_white());
			println!(
				"{}",
				"  /image https://example.com/image.png".bright_white()
			);
			println!(
				"{}",
				"Supported formats: PNG, JPEG, GIF, WebP, BMP".bright_blue()
			);
			println!(
				"{}",
				"💡 Tip: Copy an image to clipboard and run /image to auto-attach it".bright_blue()
			);
		}
		println!();
	}
}

pub fn display_prompt(output: &CommandOutput) {
	if let CommandOutput::Prompt { data } = output {
		if let Some(action) = data.get("action").and_then(|v| v.as_str()) {
			match action {
				"list" => {
					if let Some(prompts) = data.get("prompts").and_then(|v| v.as_array()) {
						if prompts.is_empty() {
							println!("{}", "No prompt templates configured.".bright_yellow());
							println!("{}", "Prompt templates can be defined in the [[prompts]] section of your configuration.".bright_blue());
							println!("{}", "Example configuration:".bright_cyan());
							println!(
								"{}",
								r#"[[prompts]]
name = "review"
description = "Request code review"
prompt = "Please review the code above focusing on best practices and security.""#
									.bright_white()
							);
						} else {
							println!("{}", "Available prompt templates:".bright_cyan());
							for prompt in prompts {
								let name =
									prompt.get("name").and_then(|v| v.as_str()).unwrap_or("");
								let description =
									prompt.get("description").and_then(|v| v.as_str());
								if let Some(desc) = description {
									println!(
										"  {} {} - {}",
										"/prompt".cyan(),
										name.bright_yellow(),
										desc.bright_white()
									);
								} else {
									println!("  {} {}", "/prompt".cyan(), name.bright_yellow());
								}
							}
							println!();
							println!("{}", "Usage: /prompt <template_name>".bright_blue());
							println!("{}", "Example: /prompt review".bright_green());
						}
					}
				}
				"execute" => {
					if let Some(true) = data.get("success").and_then(|v| v.as_bool()) {
						if let Some(name) = data.get("prompt_name").and_then(|v| v.as_str()) {
							println!(
								"{} {}",
								"Prompt template applied:".bright_green(),
								name.bright_yellow()
							);
						}
					} else if let Some(error) = data.get("error").and_then(|v| v.as_str()) {
						println!("{}", error.bright_red());
						if let Some(available) =
							data.get("available_prompts").and_then(|v| v.as_array())
						{
							if !available.is_empty() {
								println!("{}", "Available templates:".bright_cyan());
								for prompt in available {
									if let Some(name) = prompt.as_str() {
										println!("  {}", name.bright_yellow());
									}
								}
							}
						}
					}
				}
				_ => {}
			}
		}
	}
}

pub fn display_done(output: &CommandOutput) {
	if let CommandOutput::Done {
		done,
		memorized,
		summarized,
		saved,
	} = output
	{
		if *done {
			println!("{}", "✅ Task finalized successfully!".bright_green());
			if let Some(true) = memorized {
				println!("{}", "  • Insights memorized".bright_blue());
			}
			if let Some(true) = summarized {
				println!("{}", "  • Session summarized".bright_blue());
			}
			if let Some(true) = saved {
				println!("{}", "  • Session saved".bright_blue());
			}
			println!();
			println!(
				"{}",
				"Layered processing reset for next task.".bright_cyan()
			);
		}
		println!();
	}
}

pub fn display_run(output: &CommandOutput) {
	if let CommandOutput::Run {
		command_executed: _,
		data,
	} = output
	{
		if let Some(action) = data.get("action").and_then(|v| v.as_str()) {
			match action {
				"list" => {
					if let Some(commands) = data.get("commands").and_then(|v| v.as_array()) {
						if commands.is_empty() {
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
							for cmd in commands {
								if let Some(name) = cmd.as_str() {
									println!("  {} {}", "/run".cyan(), name.bright_yellow());
								}
							}
							println!();
							println!("{}", "Usage: /run <command_name>".bright_blue());
							println!("{}", "Example: /run estimate".bright_green());
						}
					}
				}
				"execute" => {
					if let Some(false) = data.get("success").and_then(|v| v.as_bool()) {
						if let Some(error) = data.get("error").and_then(|v| v.as_str()) {
							println!("{}", error.bright_red());
						}
						if let Some(available) =
							data.get("available_commands").and_then(|v| v.as_array())
						{
							if !available.is_empty() {
								println!("{}", "Available commands:".bright_cyan());
								for cmd in available {
									if let Some(name) = cmd.as_str() {
										println!("  {}", name.bright_yellow());
									}
								}
							}
						}
					} else if let Some(true) = data.get("success").and_then(|v| v.as_bool()) {
						// Result is already printed by the command executor (print_assistant_response in handle_run)
						// No additional output needed here
					}
				}
				_ => {}
			}
		}
	}
}

pub fn display_workflow(output: &CommandOutput, _config: &Config) {
	if let CommandOutput::Workflow {
		workflow_executed: _,
		data,
	} = output
	{
		if let Some(action) = data.get("action").and_then(|v| v.as_str()) {
			match action {
				"list" => {
					if let Some(workflows) = data.get("workflows").and_then(|v| v.as_array()) {
						if workflows.is_empty() {
							println!("{}", "No workflows configured.".bright_yellow());
							println!("{}", "Workflows can be defined in the [workflows] section of your configuration.".bright_blue());
							println!("{}", "Example configuration:".bright_cyan());
							println!(
								"{}",
								r#"[[workflows]]
name = "developer_workflow"
description = "Two-stage workflow: refine task, then research context"

[[workflows.steps]]
name = "refine"
type = "once"
layer = "task_refiner"

[[workflows.steps]]
name = "research"
type = "once"
layer = "task_researcher""#
									.bright_white()
							);
						} else {
							println!("{}", "Available workflows:".bright_cyan());
							for workflow in workflows {
								if let (Some(name), Some(desc)) = (
									workflow.get(0).and_then(|v| v.as_str()),
									workflow.get(1).and_then(|v| v.as_str()),
								) {
									println!(
										"  {} {} - {}",
										"/workflow".cyan(),
										name.bright_yellow(),
										desc.bright_white()
									);
								}
							}
							println!();
							println!(
								"{}",
								"Usage: /workflow <workflow_name> [input]".bright_blue()
							);
							println!("{}", "Example: /workflow developer_workflow Implement user authentication".bright_green());
						}
					}
				}
				"execute" => {
					if let Some(false) = data.get("success").and_then(|v| v.as_bool()) {
						// Display errors only
						if let Some(error) = data.get("error").and_then(|v| v.as_str()) {
							println!("{}", error.bright_red());
						}
						if let Some(available) =
							data.get("available_workflows").and_then(|v| v.as_array())
						{
							if !available.is_empty() {
								println!("{}", "Available workflows:".bright_cyan());
								for workflow in available {
									if let Some(name) = workflow.as_str() {
										println!("  {}", name.bright_yellow());
									}
								}
							}
						}
					}
					// Success case: all output already displayed in real-time, nothing to show
				}

				_ => {}
			}
		}
	}
}

pub fn display_mcp(_output: &CommandOutput) {
	// MCP commands have complex output, handled by mcp.rs
}

pub fn display_report(output: &CommandOutput, config: &Config) {
	if let CommandOutput::Report { entries, totals } = output {
		// Reconstruct SessionReport from the entries and totals
		let report_entries: Vec<crate::session::report::ReportEntry> = entries
			.iter()
			.filter_map(|entry| {
				Some(crate::session::report::ReportEntry {
					user_request: entry.get("user_request")?.as_str()?.to_string(),
					cost: entry.get("cost")?.as_str()?.to_string(),
					tool_calls: entry.get("tool_calls")?.as_u64()? as u32,
					tools_used: entry.get("tools_used")?.as_str()?.to_string(),
					human_time: entry.get("human_time")?.as_str()?.to_string(),
					ai_time: entry.get("ai_time")?.as_str()?.to_string(),
					processing_time: entry.get("processing_time")?.as_str()?.to_string(),
				})
			})
			.collect();

		let report_totals = crate::session::report::ReportTotals {
			total_cost: totals
				.get("total_cost")
				.and_then(|v| v.as_f64())
				.unwrap_or(0.0),
			total_tool_calls: totals
				.get("total_tool_calls")
				.and_then(|v| v.as_u64())
				.unwrap_or(0) as u32,
			total_human_time_ms: totals
				.get("total_human_time_ms")
				.and_then(|v| v.as_u64())
				.unwrap_or(0),
			total_ai_time_ms: totals
				.get("total_ai_time_ms")
				.and_then(|v| v.as_u64())
				.unwrap_or(0),
			total_processing_time_ms: totals
				.get("total_processing_time_ms")
				.and_then(|v| v.as_u64())
				.unwrap_or(0),
			total_requests: report_entries.len() as u32,
		};

		let report = crate::session::report::SessionReport {
			entries: report_entries,
			totals: report_totals,
		};

		// Display using existing report display logic
		report.display(config);
	}
}

pub fn display_session(output: &CommandOutput) {
	if let CommandOutput::Session {
		switched,
		session_name,
	} = output
	{
		if *switched {
			println!(
				"{} {}",
				"Creating new session:".bright_green(),
				session_name.bright_white()
			);
		} else {
			println!("{}", "You are already in this session.".blue());
		}
	}
}

pub fn display_list(output: &CommandOutput, config: &Config) {
	if let CommandOutput::List {
		plain_text: Some(markdown_content),
		..
	} = output
	{
		// Render using markdown renderer if enabled
		if config.enable_markdown_rendering {
			let theme = config.markdown_theme.parse().unwrap_or_default();
			let renderer = crate::session::chat::markdown::MarkdownRenderer::with_theme(theme);
			match renderer.render_and_print(markdown_content) {
				Ok(_) => {
					// Successfully rendered as markdown
				}
				Err(_) => {
					// Fallback to plain text if markdown rendering fails
					display_plain_list(markdown_content);
				}
			}
		} else {
			// Use plain text rendering
			display_plain_list(markdown_content);
		}
	}
}

/// Display markdown as plain text (fallback for list command)
fn display_plain_list(markdown_content: &str) {
	// Convert markdown to plain text for fallback
	let plain_text = markdown_content
		.replace("# ", "")
		.replace("## ", "")
		.replace("**", "")
		.replace("*", "");
	println!("{}", plain_text);
}
