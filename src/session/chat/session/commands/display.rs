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
			"{} [level] - View or change reasoning effort: low, medium, high, xhigh, max",
			EFFORT_COMMAND.cyan()
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
			"{} <path> - Attach video to next message (MP4, MOV, AVI, WebM, MKV, M4V, 3GP)",
			VIDEO_COMMAND.cyan()
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
			"{} [page|*pattern*|<name>] - List skills or toggle a skill on/off by name",
			SKILL_COMMAND.cyan()
		);
		println!(
			"{} [list|add|remove|edit] - Schedule a message to be injected later (mirrors MCP schedule tool)",
			SCHEDULE_COMMAND.cyan()
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

pub fn display_effort(output: &CommandOutput) {
	if let CommandOutput::Effort {
		old_effort,
		new_effort,
		changed,
		saved,
		save_error,
	} = output
	{
		if *changed {
			if let Some(old) = old_effort {
				println!(
					"{} {} → {}",
					"Reasoning effort changed:".bright_green(),
					old.bright_yellow(),
					new_effort.bright_green()
				);
			} else {
				println!(
					"{} {}",
					"Reasoning effort set to:".bright_green(),
					new_effort.bright_white()
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
				"Current reasoning effort:".bright_cyan(),
				new_effort.bright_white()
			);
			println!();
			println!("{}", "Available levels:".bright_yellow());
			println!("{}", "  - low".bright_white());
			println!("{}", "  - medium".bright_white());
			println!("{}", "  - high".bright_white());
			println!("{}", "  - xhigh".bright_white());
			println!("{}", "  - max".bright_white());
			println!();
			println!(
				"{}",
				"Usage: /effort <level> (e.g., /effort high)".bright_blue()
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

pub fn display_info(output: &CommandOutput) {
	use crate::session::chat::session::utils::format_number;

	if let CommandOutput::Info {
		session_name,
		model,
		tokens_input,
		tokens_output,
		tokens_used,
		tokens_cached,
		tokens_cache_write,
		tokens_reasoning,
		total_cost,
		tokens_per_second,
		avg_tokens_per_compression,
		avg_tokens_per_tool,
		avg_tokens_per_response,
		avg_input_tokens,
		compression_stats,
		cache_markers_system,
		cache_markers_tool,
		cache_markers_content,
		cache_non_cached_tokens,
		..
	} = output
	{
		println!(
			"{}",
			"───────────── Session Information ─────────────".bright_cyan()
		);
		println!(
			"{} {}",
			"Session name:".yellow(),
			session_name.bright_white()
		);
		println!("{} {}", "Main model:".yellow(), model.bright_white());

		let total_tokens = tokens_used + tokens_cached + tokens_cache_write + tokens_reasoning;
		println!(
			"{} {}",
			"Total tokens:".yellow(),
			format_number(total_tokens).bright_white()
		);
		println!(
			"{} {} input, {} output, {} cache read, {} cache write, {} reasoning",
			"Breakdown:".yellow(),
			format_number(*tokens_input).bright_blue(),
			format_number(*tokens_output).bright_green(),
			format_number(*tokens_cached).bright_magenta(),
			format_number(*tokens_cache_write).bright_cyan(),
			format_number(*tokens_reasoning).white(),
		);
		println!("{} ${:.5}", "Total cost:".yellow(), total_cost);
		if *tokens_per_second > 0.0 {
			println!("{} {:.1} tok/s", "Throughput:".yellow(), tokens_per_second);
		}
		println!();
		println!("{}", "───────────── Averages ─────────────".bright_cyan());
		if *avg_tokens_per_compression > 0.0 {
			println!(
				"{} {} tok/compression",
				"Tokens per compression:".yellow(),
				format_number(*avg_tokens_per_compression as u64).bright_white()
			);
		}
		if *avg_tokens_per_tool > 0.0 {
			println!(
				"{} {} tok/tool",
				"Tokens per tool:".yellow(),
				format_number(*avg_tokens_per_tool as u64).bright_white()
			);
		}
		if *avg_tokens_per_response > 0.0 {
			println!(
				"{} {} tok/response",
				"Tokens per response:".yellow(),
				format_number(*avg_tokens_per_response as u64).bright_white()
			);
		}
		if *avg_input_tokens > 0.0 {
			println!(
				"{} {} tok/request",
				"Input per request:".yellow(),
				format_number(*avg_input_tokens as u64).bright_white()
			);
		}

		// Compression stats
		if let Some(stats) = compression_stats {
			println!();
			println!(
				"{}",
				"───────────── Compression Statistics ─────────────".bright_cyan()
			);
			if stats.conversation_compressions > 0 {
				println!(
					"{} {}",
					"Conversation compressions:".yellow(),
					format_number(stats.conversation_compressions as u64).bright_white()
				);
			}
			println!(
				"{} {}",
				"Messages removed:".yellow(),
				format_number(stats.total_messages_removed as u64).bright_green()
			);
			println!(
				"{} {}",
				"Tokens saved:".yellow(),
				format_number(stats.total_tokens_saved).bright_green()
			);
			let avg_ratio = stats.avg_compression_ratio() * 100.0;
			if avg_ratio > 0.0 {
				println!("{} {:.1}%", "Avg compression:".yellow(), avg_ratio);
			}
		}

		// Cache stats
		println!();
		println!(
			"{}",
			"───────────── Cache Statistics ─────────────".bright_cyan()
		);
		println!("{} {}", "System markers:".yellow(), cache_markers_system);
		println!("{} {}", "Tool markers:".yellow(), cache_markers_tool);
		println!("{} {}", "Content markers:".yellow(), cache_markers_content);
		println!(
			"{} {}",
			"Cache read tokens:".yellow(),
			format_number(*tokens_cached).bright_magenta()
		);
		println!(
			"{} {}",
			"Cache write tokens:".yellow(),
			format_number(*tokens_cache_write).bright_cyan()
		);
		println!(
			"{} {}",
			"Non-cached tokens:".yellow(),
			format_number(*cache_non_cached_tokens).bright_white()
		);
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

pub fn display_video(output: &CommandOutput) {
	if let CommandOutput::Video {
		video_attached,
		path,
		error,
	} = output
	{
		if *video_attached {
			println!("{}", "✅ Video attached successfully!".bright_green());
			if let Some(p) = path {
				println!("{} {}", "Path:".bright_cyan(), p.bright_white());
			}
			println!(
				"{}",
				"Your next message will include this video.".bright_cyan()
			);
		} else if let Some(err) = error {
			println!("{}: {}", "❌ Failed to attach video".bright_red(), err);
		} else {
			// Show usage
			println!("{}", "Usage: /video <path_to_video_or_url>".bright_yellow());
			println!("{}", "Examples:".bright_blue());
			println!("{}", "  /video recording.mp4".bright_white());
			println!("{}", "  /video /path/to/video.mov".bright_white());
			println!(
				"{}",
				"  /video https://example.com/video.mp4".bright_white()
			);
			println!(
				"{}",
				"Supported formats: MP4, MOV, AVI, WebM, MKV, M4V, 3GP".bright_blue()
			);
			println!("{}", "💡 Tip: Max file size is 100MB".bright_blue());
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

pub fn display_run(output: &CommandOutput, config: &Config, role: &str) {
	use crate::session::chat::assistant_output::print_assistant_response;

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
server_refs = ["core", "filesystem"]
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
						// Print the result using markdown-aware formatting
						if let Some(result) = data.get("result").and_then(|v| v.as_str()) {
							println!();
							print_assistant_response(result, config, role, &None);
							println!();
						}
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

pub fn display_mcp(output: &CommandOutput) {
	if let CommandOutput::Mcp { data, .. } = output {
		let subcommand = data
			.get("subcommand")
			.and_then(|v| v.as_str())
			.unwrap_or("");

		match subcommand {
			"list" => display_mcp_list(data),
			"info" => display_mcp_info(data),
			"full" => display_mcp_full(data),
			"health" => display_mcp_health(data),
			"dump" => display_mcp_dump(data),
			"validate" => display_mcp_validate(data),
			"invalid" => display_mcp_invalid(data),
			_ => {
				// Fallback for unknown subcommands
				if let Some(message) = data.get("message").and_then(|v| v.as_str()) {
					println!("{}", message);
				}
			}
		}
	}
}

fn display_mcp_list(data: &serde_json::Value) {
	use colored::Colorize;

	println!();
	println!("{}", "Available Tools".bright_cyan().bold());
	println!("{}", "─".repeat(30).dimmed());

	if let Some(servers) = data.get("servers").and_then(|v| v.as_object()) {
		if servers.is_empty() {
			println!("{}", "No tools available.".yellow());
		} else {
			for (server_name, tools) in servers {
				println!();
				println!("  {}", server_name.bright_blue().bold());
				if let Some(tool_array) = tools.as_array() {
					for tool in tool_array {
						if let Some(tool_name) = tool.as_str() {
							println!("    {}", tool_name.bright_white());
						}
					}
				}
			}
		}
	}

	println!();
	println!(
		"{}",
		"Use '/mcp info' for descriptions or '/mcp full' for detailed parameters.".dimmed()
	);
}

fn display_mcp_info(data: &serde_json::Value) {
	use colored::Colorize;

	println!();
	println!("{}", "MCP Server Status".bright_cyan().bold());
	println!("{}", "─".repeat(50).dimmed());

	// Check for "No servers" message
	if let Some(message) = data.get("message").and_then(|v| v.as_str()) {
		println!("{}", message.yellow());
		return;
	}

	// Display server status
	if let Some(servers) = data.get("servers").and_then(|v| v.as_array()) {
		for server in servers {
			let name = server
				.get("name")
				.and_then(|v| v.as_str())
				.unwrap_or("unknown");
			let health = server
				.get("health")
				.and_then(|v| v.as_str())
				.unwrap_or("unknown");
			let conn_type = server
				.get("connection_type")
				.and_then(|v| v.as_str())
				.unwrap_or("unknown");
			let restart_count = server
				.get("restart_count")
				.and_then(|v| v.as_u64())
				.unwrap_or(0);
			let consecutive_failures = server
				.get("consecutive_failures")
				.and_then(|v| v.as_u64())
				.unwrap_or(0);

			let health_display = match health {
				"running" => "✅ Running".green(),
				"dead" => "❌ Dead".red(),
				"restarting" => "🔄 Restarting".yellow(),
				"failed" => "💥 Failed".bright_red(),
				"unreachable" => "🔒 Auth Failed".bright_red(),
				_ => health.normal(),
			};

			println!();
			println!("{}: {}", name.bright_white().bold(), health_display);
			println!("  Type: {}", conn_type);

			if let Some(tools) = server.get("tools").and_then(|v| v.as_array()) {
				if !tools.is_empty() {
					let tool_names: Vec<String> = tools
						.iter()
						.filter_map(|t| t.as_str())
						.map(|s| s.to_string())
						.collect();
					println!("  Configured tools: {}", tool_names.join(", ").dimmed());
				}
			}

			if restart_count > 0 {
				println!("  Restart count: {}", restart_count);
				if consecutive_failures > 0 {
					println!("  Consecutive failures: {}", consecutive_failures);
				}
			}
		}
	}

	// Display tools
	println!();
	println!("{}", "Available Tools".bright_cyan().bold());
	println!("{}", "─".repeat(50).dimmed());

	if let Some(tools) = data.get("tools").and_then(|v| v.as_object()) {
		if tools.is_empty() {
			println!("{}", "No tools available.".yellow());
		} else {
			for (server_name, tool_list) in tools {
				println!();
				println!("  {}", server_name.bright_blue().bold());

				if let Some(tool_array) = tool_list.as_array() {
					for tool in tool_array {
						let name = tool
							.get("name")
							.and_then(|v| v.as_str())
							.unwrap_or("unknown");
						let desc = tool
							.get("description")
							.and_then(|v| v.as_str())
							.unwrap_or("");

						if desc.is_empty() {
							println!("    {}", name.bright_white());
						} else {
							println!("    {} - {}", name.bright_white(), desc.dimmed());
						}
					}
				}
			}
		}
	}

	println!();
	println!(
		"{}",
		"Use '/mcp list' for names only or '/mcp full' for detailed parameters.".dimmed()
	);
}

// Placeholder implementations for remaining subcommands
// These will be implemented once the corresponding handlers are refactored
fn display_mcp_full(data: &serde_json::Value) {
	use colored::Colorize;

	println!();
	println!(
		"{}",
		"MCP Server Status & Tools (Full Details)"
			.bright_cyan()
			.bold()
	);
	println!("{}", "─".repeat(60).dimmed());

	if let Some(msg) = data.get("message").and_then(|v| v.as_str()) {
		println!("{}", msg.yellow());
		return;
	}

	// Server status
	if let Some(servers) = data.get("servers").and_then(|v| v.as_array()) {
		for server in servers {
			let name = server.get("name").and_then(|v| v.as_str()).unwrap_or("");
			let health = server.get("health").and_then(|v| v.as_str()).unwrap_or("");
			let conn_type = server
				.get("connection_type")
				.and_then(|v| v.as_str())
				.unwrap_or("");
			let health_display = match health {
				"running" => "✅ Running".green(),
				"dead" => "❌ Dead".red(),
				"restarting" => "🔄 Restarting".yellow(),
				"failed" => "💥 Failed".bright_red(),
				"unreachable" => "🔒 Auth Failed".bright_red(),
				_ => health.normal(),
			};
			println!();
			println!("{}: {}", name.bright_white().bold(), health_display);
			println!("  Type: {}", conn_type);

			if let Some(tools) = server.get("tools").and_then(|v| v.as_array()) {
				let tool_names: Vec<&str> = tools.iter().filter_map(|t| t.as_str()).collect();
				if !tool_names.is_empty() {
					println!("  Configured tools: {}", tool_names.join(", ").dimmed());
				}
			}

			let restart_count = server
				.get("restart_count")
				.and_then(|v| v.as_u64())
				.unwrap_or(0);
			if restart_count > 0 {
				println!("  Restart count: {}", restart_count);
				let failures = server
					.get("consecutive_failures")
					.and_then(|v| v.as_u64())
					.unwrap_or(0);
				if failures > 0 {
					println!("  Consecutive failures: {}", failures);
				}
			}
		}
	}

	// Tools with full details
	println!();
	println!("{}", "Available Tools (Full Details)".bright_cyan().bold());
	println!("{}", "─".repeat(60).dimmed());

	if let Some(tools_by_server) = data.get("tools").and_then(|v| v.as_object()) {
		if tools_by_server.is_empty() {
			println!("{}", "No tools available.".yellow());
		} else {
			for (server_name, tools) in tools_by_server {
				println!();
				println!("  {}", server_name.bright_blue().bold());

				if let Some(tools_arr) = tools.as_array() {
					for tool in tools_arr {
						let name = tool.get("name").and_then(|v| v.as_str()).unwrap_or("");
						let desc = tool
							.get("description")
							.and_then(|v| v.as_str())
							.unwrap_or("");
						println!("    {}", name.bright_white().bold());
						if !desc.is_empty() {
							println!("      {}", desc.dimmed());
						}

						// Parameters
						if let Some(params) = tool.get("parameters") {
							if let Some(props) =
								params.get("properties").and_then(|v| v.as_object())
							{
								if !props.is_empty() {
									println!("      {}", "Parameters:".bright_green());
									let required: std::collections::HashSet<String> = params
										.get("required")
										.and_then(|r| r.as_array())
										.map(|arr| {
											arr.iter()
												.filter_map(|v| v.as_str())
												.map(|s| s.to_string())
												.collect()
										})
										.unwrap_or_default();

									for (param_name, param_info) in props {
										let marker = if required.contains(param_name) {
											"*".bright_red()
										} else {
											" ".normal()
										};
										let ptype = param_info
											.get("type")
											.and_then(|v| v.as_str())
											.unwrap_or("any");
										let pdesc = param_info
											.get("description")
											.and_then(|v| v.as_str())
											.unwrap_or("");
										println!(
											"        {}{}: {} {}",
											marker,
											param_name.bright_cyan(),
											ptype.yellow(),
											if !pdesc.is_empty() {
												format!("- {}", pdesc).dimmed()
											} else {
												"".normal()
											}
										);
										if let Some(enum_vals) =
											param_info.get("enum").and_then(|v| v.as_array())
										{
											let vals: Vec<&str> = enum_vals
												.iter()
												.filter_map(|v| v.as_str())
												.collect();
											if !vals.is_empty() {
												println!(
													"          {}: {}",
													"options".bright_black(),
													vals.join(", ").bright_black()
												);
											}
										}
										if let Some(default_val) = param_info.get("default") {
											println!(
												"          {}: {}",
												"default".bright_black(),
												default_val.to_string().bright_black()
											);
										}
									}
								}
							} else if *params != serde_json::json!({}) {
								println!(
									"      {}: {}",
									"Schema".bright_green(),
									params.to_string().dimmed()
								);
							}
						}
						println!();
					}
				}
			}
		}
	}

	println!();
	println!("{}", "Legend: ".bright_yellow());
	println!("  {} Required parameter", "*".bright_red());
	println!(
		"  {}",
		"Use '/mcp list' for names only or '/mcp info' for overview.".dimmed()
	);
}

fn display_mcp_health(data: &serde_json::Value) {
	use colored::Colorize;

	println!();
	println!("{}", "MCP Server Health Check".bright_cyan().bold());
	println!("{}", "─".repeat(50).dimmed());

	if let Some(msg) = data.get("message").and_then(|v| v.as_str()) {
		println!("{}", msg.yellow());
		return;
	}

	let monitor_running = data
		.get("monitor_running")
		.and_then(|v| v.as_bool())
		.unwrap_or(false);
	if monitor_running {
		println!("{}", "🔍 Health monitor: RUNNING".bright_green());
	} else {
		println!("{}", "🔍 Health monitor: STOPPED".bright_red());
	}
	println!();

	if let Some(error) = data.get("error").and_then(|v| v.as_str()) {
		println!("{}: {}", "Health check failed".bright_red(), error);
		return;
	}

	println!(
		"{}",
		"Performing health check on all external servers...".bright_blue()
	);

	if let Some(servers) = data.get("servers").and_then(|v| v.as_array()) {
		for server in servers {
			let name = server.get("name").and_then(|v| v.as_str()).unwrap_or("");
			let health = server.get("health").and_then(|v| v.as_str()).unwrap_or("");
			let health_display = match health {
				"running" => "✅ Running".green(),
				"dead" => "❌ Dead".red(),
				"restarting" => "🔄 Restarting".yellow(),
				"failed" => "💥 Failed".bright_red(),
				"unreachable" => "🔒 Auth Failed".bright_red(),
				_ => health.normal(),
			};
			println!("{}: {}", name.bright_white().bold(), health_display);

			let restart_count = server
				.get("restart_count")
				.and_then(|v| v.as_u64())
				.unwrap_or(0);
			if restart_count > 0 {
				println!("  Restart count: {}", restart_count);
				let failures = server
					.get("consecutive_failures")
					.and_then(|v| v.as_u64())
					.unwrap_or(0);
				if failures > 0 {
					println!("  Consecutive failures: {}", failures);
				}
			}

			if let Some(secs) = server.get("last_checked_secs_ago").and_then(|v| v.as_u64()) {
				println!("  Last checked: {}s ago", secs);
			}
		}
	}

	println!();
	println!(
		"{}",
		"Health check completed. Dead servers will be automatically restarted by the health monitor.".bright_blue()
	);
}

fn display_mcp_dump(data: &serde_json::Value) {
	use colored::Colorize;

	println!();
	println!("{}", "Raw MCP Tool Definitions (JSON)".bright_cyan().bold());
	println!("{}", "─".repeat(50).dimmed());

	if let Some(tools) = data.get("tools").and_then(|v| v.as_array()) {
		if tools.is_empty() {
			println!("{}", "No tools available.".yellow());
		} else {
			for (i, tool) in tools.iter().enumerate() {
				let name = tool.get("name").and_then(|v| v.as_str()).unwrap_or("");
				println!();
				println!("{}. {}", i + 1, name.bright_white().bold());
				println!("{}", serde_json::to_string_pretty(tool).unwrap_or_default());
			}
		}
	}

	println!();
	println!(
		"{}",
		"Use this output to debug tool schema validation issues.".dimmed()
	);
}

fn display_mcp_validate(data: &serde_json::Value) {
	use colored::Colorize;

	println!();
	println!("{}", "MCP Tool Schema Validation".bright_cyan().bold());
	println!("{}", "─".repeat(50).dimmed());

	if let Some(tools) = data.get("tools").and_then(|v| v.as_array()) {
		if tools.is_empty() {
			println!("{}", "No tools available to validate.".yellow());
			return;
		}

		for (i, tool) in tools.iter().enumerate() {
			let name = tool.get("name").and_then(|v| v.as_str()).unwrap_or("");
			let valid = tool.get("valid").and_then(|v| v.as_bool()).unwrap_or(false);
			println!();
			println!("{}. Validating {}", i + 1, name.bright_white().bold());

			if valid {
				println!("  {}", "✅ Valid schema".bright_green());
			} else {
				println!("  {}", "❌ Schema issues found:".bright_red());
				if let Some(issues) = tool.get("issues").and_then(|v| v.as_array()) {
					for issue in issues {
						if let Some(s) = issue.as_str() {
							println!("    - {}", s.yellow());
						}
					}
				}
			}
		}

		println!();
		let all_valid = data
			.get("all_valid")
			.and_then(|v| v.as_bool())
			.unwrap_or(false);
		if all_valid {
			println!("{}", "✅ All tool schemas are valid!".bright_green());
		} else {
			println!(
				"{}",
				"❌ Some tool schemas have validation issues.".bright_red()
			);
			println!(
				"{}",
				"These issues may cause API errors with providers like Anthropic.".yellow()
			);
		}
	}
}

fn display_mcp_invalid(_data: &serde_json::Value) {
	use colored::Colorize;

	println!();
	println!("{}", "Invalid MCP subcommand.".bright_red());
	println!();
	println!("{}", "Available subcommands:".bright_cyan());
	println!("  {} - Show tool names only", "/mcp list".cyan());
	println!(
		"  {} - Show server status and tools with descriptions (default)",
		"/mcp info".cyan()
	);
	println!(
		"  {} - Show full details including parameters",
		"/mcp full".cyan()
	);
	println!(
		"  {} - Check server health and attempt restart if needed",
		"/mcp health".cyan()
	);
	println!(
		"  {} - Dump raw tool definitions in JSON format",
		"/mcp dump".cyan()
	);
	println!();
	println!(
		"  {} - Validate tool schema definitions",
		"/mcp validate".cyan()
	);
	println!();
	println!(
		"{}",
		"Usage: /mcp [list|info|full|health|dump|validate]".bright_blue()
	);
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
					task_time: entry.get("task_time")?.as_str()?.to_string(),
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
			total_task_time_ms: totals
				.get("total_task_time_ms")
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

// ---------------------------------------------------------------------------
// /skill display
// ---------------------------------------------------------------------------

pub(super) fn display_schedule(output: &CommandOutput) {
	use colored::Colorize;

	if let CommandOutput::Schedule { data } = output {
		let subcommand = data
			.get("subcommand")
			.and_then(|v| v.as_str())
			.unwrap_or("");

		match subcommand {
			"error" => {
				if let Some(msg) = data.get("message").and_then(|v| v.as_str()) {
					println!("{}", msg.bright_red());
				}
			}
			"help" => {
				println!();
				println!(
					"{}",
					"Schedule — inject a user message later"
						.bright_cyan()
						.bold()
				);
				println!("{}", "─".repeat(50).dimmed());
				println!(
					"  {}                                   list pending entries",
					"/schedule".cyan()
				);
				println!(
					"  {} {}                              list pending entries",
					"/schedule".cyan(),
					"list".yellow()
				);
				println!(
					"  {} {} {}                       cancel an entry",
					"/schedule".cyan(),
					"remove".yellow(),
					"<id>".dimmed()
				);
				println!(
					"  {} {} {}     add a new entry",
					"/schedule".cyan(),
					"add".yellow(),
					"when=\"in 5m\" message=\"...\"".dimmed()
				);
				println!(
					"  {} {} {} {}  update an entry",
					"/schedule".cyan(),
					"edit".yellow(),
					"<id>".dimmed(),
					"[when=...] [message=...]".dimmed()
				);
				println!();
				println!("{}", "Optional add/edit keys:".bright_yellow());
				println!(
					"  {} \"now\"; duration like \"in 5m\", \"in 1h30m\"; time \"15:30\", \"9am\"; or \"2026-03-22 15:30\"",
					"when=".bright_white()
				);
				println!(
					"  {} text injected verbatim as user message when timer fires",
					"message=".bright_white()
				);
				println!(
					"  {} repeat interval — \"10m\", \"1h\", \"1h30m\"; \"none\" in edit clears it",
					"every=".bright_white()
				);
				println!(
					"  {} short label shown in list output",
					"description=".bright_white()
				);
				println!();
				println!(
					"{}",
					"Quote values with spaces: when=\"in 1h 30m\" message='hello world'".dimmed()
				);
			}
			_ => {
				// Default path — list/add/remove/edit all just print the tool's text output.
				let is_error = data
					.get("is_error")
					.and_then(|v| v.as_bool())
					.unwrap_or(false);
				if let Some(msg) = data.get("message").and_then(|v| v.as_str()) {
					if is_error {
						println!("{}", msg.bright_red());
					} else {
						println!("{}", msg);
					}
				}
			}
		}
	}
}

pub(super) fn display_skill(output: &CommandOutput) {
	use colored::Colorize;

	if let CommandOutput::Skill { data } = output {
		let subcommand = data
			.get("subcommand")
			.and_then(|v| v.as_str())
			.unwrap_or("");

		match subcommand {
			"list" => display_skill_list(data),
			"use" => {
				if let Some(name) = data.get("name").and_then(|v| v.as_str()) {
					println!("{} {}", "Enabled skill:".dimmed(), name.bright_green());
				}
			}
			"forget" => {
				if let Some(name) = data.get("name").and_then(|v| v.as_str()) {
					println!("{} {}", "Disabled skill:".dimmed(), name.bright_yellow());
				}
			}
			"error" => {
				if let Some(msg) = data.get("message").and_then(|v| v.as_str()) {
					println!("{}", msg.bright_red());
				}
			}
			_ => {}
		}
	}
}

fn display_skill_list(data: &serde_json::Value) {
	use colored::Colorize;

	let total = data.get("total").and_then(|v| v.as_u64()).unwrap_or(0);
	let active_count = data
		.get("active_count")
		.and_then(|v| v.as_u64())
		.unwrap_or(0);
	let page = data.get("page").and_then(|v| v.as_u64()).unwrap_or(1);
	let total_pages = data
		.get("total_pages")
		.and_then(|v| v.as_u64())
		.unwrap_or(1);
	let pattern = data.get("pattern").and_then(|v| v.as_str()).unwrap_or("");

	println!();
	let header = if pattern.is_empty() {
		format!("Skills ({total} available, {active_count} active)")
	} else {
		format!("Skills matching '{pattern}' ({total} found, {active_count} active)")
	};
	println!("{}", header.bright_cyan().bold());
	println!("{}", "─".repeat(50).dimmed());

	let skills = match data.get("skills").and_then(|v| v.as_array()) {
		Some(s) => s,
		None => {
			println!("{}", "No skills found.".yellow());
			return;
		}
	};

	if skills.is_empty() {
		println!("{}", "No skills found.".yellow());
		println!();
		return;
	}

	for skill in skills {
		let name = skill.get("name").and_then(|v| v.as_str()).unwrap_or("?");
		let desc = skill
			.get("description")
			.and_then(|v| v.as_str())
			.unwrap_or("");
		let is_active = skill
			.get("active")
			.and_then(|v| v.as_bool())
			.unwrap_or(false);
		let capabilities = skill
			.get("capabilities")
			.and_then(|v| v.as_array())
			.map(|a| {
				a.iter()
					.filter_map(|v| v.as_str())
					.collect::<Vec<_>>()
					.join(", ")
			})
			.unwrap_or_default();
		let domains = skill
			.get("domains")
			.and_then(|v| v.as_array())
			.map(|a| {
				a.iter()
					.filter_map(|v| v.as_str())
					.collect::<Vec<_>>()
					.join(", ")
			})
			.unwrap_or_default();
		let scripts = skill
			.get("scripts")
			.and_then(|v| v.as_array())
			.map(|a| {
				a.iter()
					.filter_map(|v| v.as_str())
					.collect::<Vec<_>>()
					.join(" ")
			})
			.unwrap_or_default();

		println!();
		if is_active {
			print!("  {} ", "✓".bright_green().bold());
			print!("{}", name.bright_green().bold());
			println!("  {}", "[active]".bright_green());
		} else {
			print!("    ");
			println!("{}", name.bright_white().bold());
		}

		// Truncate description to ~80 chars (char-safe)
		let desc_display = if desc.chars().count() > 80 {
			format!("{}...", desc.chars().take(77).collect::<String>())
		} else {
			desc.to_string()
		};
		println!("    {}", desc_display.dimmed());

		// Metadata line
		let mut meta_parts = Vec::new();
		if !capabilities.is_empty() {
			meta_parts.push(format!("capabilities: {}", capabilities));
		}
		if !domains.is_empty() {
			meta_parts.push(format!("domains: {}", domains));
		}
		if !scripts.is_empty() {
			meta_parts.push(format!("scripts: {}", scripts));
		}
		if !meta_parts.is_empty() {
			println!("    {}", meta_parts.join(" | ").dimmed());
		}
	}

	println!();

	// Pagination footer
	if total_pages > 1 {
		let mut nav = Vec::new();
		if page > 1 {
			nav.push(format!("/skill {}", page - 1));
		}
		if page < total_pages {
			nav.push(format!("/skill {}", page + 1));
		}
		println!(
			"{}",
			format!("Page {page}/{total_pages}  {}", nav.join("  ")).dimmed()
		);
	}

	println!(
		"{}",
		"Use '/skill <name>' to toggle, '/skill *pattern*' to filter.".dimmed()
	);
}

pub fn display_learning(output: &CommandOutput) {
	let data = match output {
		CommandOutput::Learning { data } => data,
		_ => return,
	};

	let subcommand = data.get("subcommand").and_then(|v| v.as_str()).unwrap_or("");

	match subcommand {
		"list" => {
			let role = data.get("role").and_then(|v| v.as_str()).unwrap_or("?");
			let project = data.get("project").and_then(|v| v.as_str()).unwrap_or("?");
			let total = data.get("total").and_then(|v| v.as_u64()).unwrap_or(0);
			let page = data.get("page").and_then(|v| v.as_u64()).unwrap_or(1);
			let total_pages = data.get("total_pages").and_then(|v| v.as_u64()).unwrap_or(0);
			let pattern = data.get("pattern").and_then(|v| v.as_str());

			let header = if let Some(pat) = pattern {
				format!("Lessons — {}/{} (filter: {})", role, project, pat)
			} else {
				format!("Lessons — {}/{}", role, project)
			};
			println!();
			println!("{}", header.bright_cyan().bold());
			println!("{}", "─".repeat(60).dimmed());

			let lessons = match data.get("lessons").and_then(|v| v.as_array()) {
				Some(l) => l,
				None => {
					println!("{}", "No lessons found.".yellow());
					return;
				}
			};

			if lessons.is_empty() {
				println!("{}", "No lessons found.".yellow());
				println!();
				return;
			}

			for lesson in lessons {
				let index = lesson.get("index").and_then(|v| v.as_u64()).unwrap_or(0);
				let content = lesson.get("content").and_then(|v| v.as_str()).unwrap_or("");
				let importance = lesson.get("importance").and_then(|v| v.as_f64()).unwrap_or(0.5);
				let confidence = lesson.get("confidence").and_then(|v| v.as_str()).unwrap_or("");
				let tags = lesson
					.get("tags")
					.and_then(|v| v.as_array())
					.map(|a| {
						a.iter()
							.filter_map(|v| v.as_str())
							.collect::<Vec<_>>()
							.join(", ")
					})
					.unwrap_or_default();
				let created = lesson.get("created").and_then(|v| v.as_str()).unwrap_or("");

				// Importance indicator: high ≥0.7, medium ≥0.4, low <0.4
				let imp_indicator = if importance >= 0.7 {
					"[high]".bright_yellow()
				} else if importance >= 0.4 {
					"[med] ".normal().dimmed()
				} else {
					"[low] ".normal().dimmed()
				};

				println!();
				print!("  {:>3}. {} ", index, imp_indicator);
				// Truncate content to ~80 chars
				let content_display = if content.chars().count() > 80 {
					format!("{}…", content.chars().take(79).collect::<String>())
				} else {
					content.to_string()
				};
				println!("{}", content_display.bright_white());

				// Metadata line
				let mut meta = Vec::new();
				if !confidence.is_empty() {
					meta.push(format!("confidence: {}", confidence));
				}
				if !tags.is_empty() {
					meta.push(format!("tags: {}", tags));
				}
				if !created.is_empty() {
					// Show just the date portion (first 10 chars of ISO timestamp)
					let date: String = created.chars().take(10).collect();
					meta.push(format!("created: {}", date));
				}
				if !meta.is_empty() {
					println!("       {}", meta.join(" | ").dimmed());
				}
			}

			println!();

			// Pagination footer
			if total_pages > 1 {
				let mut nav = Vec::new();
				if page > 1 {
					nav.push(format!("/learning list {}", page - 1));
				}
				if page < total_pages {
					nav.push(format!("/learning list {}", page + 1));
				}
				println!(
					"{}",
					format!("Page {page}/{total_pages}  {}", nav.join("  ")).dimmed()
				);
			}

			println!(
				"{}",
				format!(
					"{} lesson(s) total  |  /learning delete <n> to remove  |  /learning clear to remove all",
					total
				)
				.dimmed()
			);
		}
		"delete" => {
			let index = data.get("index").and_then(|v| v.as_u64()).unwrap_or(0);
			let preview = data
				.get("content_preview")
				.and_then(|v| v.as_str())
				.unwrap_or("");
			println!(
				"{}",
				format!("✓ Deleted lesson #{}: {}…", index, preview).bright_green()
			);
		}
		"clear" => {
			let deleted = data.get("deleted").and_then(|v| v.as_u64()).unwrap_or(0);
			let errors = data
				.get("errors")
				.and_then(|v| v.as_array())
				.map(|a| a.len())
				.unwrap_or(0);
			if deleted == 0 {
				println!("{}", "No lessons to clear.".yellow());
			} else {
				println!(
					"{}",
					format!("✓ Cleared {} lesson(s).", deleted).bright_green()
				);
				if errors > 0 {
					println!(
						"{}",
						format!("  {} error(s) — some files may not have been removed.", errors)
							.yellow()
					);
				}
			}
		}
		"error" => {
			let msg = data.get("message").and_then(|v| v.as_str()).unwrap_or("unknown error");
			println!("{}", msg.bright_red());
		}
		_ => {}
	}
}
