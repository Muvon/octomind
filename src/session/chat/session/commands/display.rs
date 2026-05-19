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
use crate::session::chat::tool_display::{
	block_blank, block_close_err, block_close_ok, block_line, block_open, block_row,
	block_row_text, block_section, block_section_with, key_width,
};
use colored::Colorize;

// Note: Main display routing is now in CommandOutput::display_cli()
// These functions handle the actual formatting

pub fn display_help(output: &CommandOutput, config: &Config) {
	if let CommandOutput::Help { .. } = output {
		use crate::session::chat::commands::*;

		// (command_with_args, description) for the built-in command listing.
		let builtins: &[(&str, &str)] = &[
			(HELP_COMMAND, "Show this help message"),
			(COPY_COMMAND, "Copy last response to clipboard"),
			(CLEAR_COMMAND, "Clear the screen"),
			(LIST_COMMAND, "List all available sessions"),
			(SESSION_COMMAND, "Switch to another session or create one"),
			(INFO_COMMAND, "Detailed token and cost breakdown"),
			(DONE_COMMAND, "Finalize task with memorize/summarize/commit"),
			(LOGLEVEL_COMMAND, "Set logging level: none, info, debug"),
			(RUN_COMMAND, "Execute a command layer"),
			(WORKFLOW_COMMAND, "Execute a workflow"),
			(CONTEXT_COMMAND, "Display session context (filterable)"),
			(MODEL_COMMAND, "View or change current AI model"),
			(EFFORT_COMMAND, "View or change reasoning effort"),
			(ROLE_COMMAND, "View or change current role"),
			(MCP_COMMAND, "MCP server management"),
			(IMAGE_COMMAND, "Attach image to next message"),
			(VIDEO_COMMAND, "Attach video to next message"),
			(PROMPT_COMMAND, "Manage prompt templates"),
			(PLAN_COMMAND, "Display current plan"),
			(SKILL_COMMAND, "List skills or toggle by name"),
			(SCHEDULE_COMMAND, "Schedule a message to be injected later"),
			(LEARNING_COMMAND, "Manage role/project lessons"),
			(REPORT_COMMAND, "Generate detailed usage report"),
			(EXIT_COMMAND, "Exit the session"),
		];

		let custom_cmds: Vec<(String, &str)> = config
			.commands
			.as_ref()
			.map(|cs| {
				cs.iter()
					.map(|c| (format!("/run {}", c.name), c.description.as_str()))
					.collect()
			})
			.unwrap_or_default();
		let workflow_cmds: Vec<(String, &str)> = config
			.workflows
			.iter()
			.map(|w| (format!("/workflow {}", w.name), w.description.as_str()))
			.collect();

		// Column width: pad command names so descriptions align.
		let builtins_width = builtins.iter().map(|(c, _)| c.len()).max().unwrap_or(0);
		let custom_width = custom_cmds.iter().map(|(c, _)| c.len()).max().unwrap_or(0);
		let workflow_width = workflow_cmds
			.iter()
			.map(|(c, _)| c.len())
			.max()
			.unwrap_or(0);
		let pad = builtins_width.max(custom_width).max(workflow_width).min(24);

		block_open("/help", None);
		block_section("commands");
		for (cmd, desc) in builtins {
			block_row(cmd, &desc.dimmed().to_string(), pad);
		}
		if !custom_cmds.is_empty() {
			block_section("custom");
			for (cmd, desc) in &custom_cmds {
				block_row(cmd, &desc.dimmed().to_string(), pad);
			}
		}
		if !workflow_cmds.is_empty() {
			block_section("workflows");
			for (cmd, desc) in &workflow_cmds {
				block_row(cmd, &desc.dimmed().to_string(), pad);
			}
		}
		let total = builtins.len() + custom_cmds.len() + workflow_cmds.len();
		block_close_ok("/help", Some(&format!("{} commands", total)));
		println!();
	}
}

pub fn display_loglevel(output: &CommandOutput) {
	if let CommandOutput::Loglevel {
		old_level,
		new_level,
		current_level,
		available_levels,
		changed,
	} = output
	{
		block_open("/loglevel", None);
		if *changed {
			if let Some(level) = new_level {
				let kw = key_width(["from", "to", "note"]);
				if let Some(old) = old_level {
					block_row("from", &old.bright_yellow().to_string(), kw);
				}
				block_row("to", &level.bright_green().to_string(), kw);
				block_row("note", &"runtime only — not saved".dimmed().to_string(), kw);
				block_close_ok("/loglevel", Some(&format!("set to {}", level)));
			} else {
				block_close_ok("/loglevel", Some("changed"));
			}
		} else if let Some(current) = current_level {
			let kw = key_width(["current", "available"]);
			block_row("current", &current.bright_white().to_string(), kw);
			block_row(
				"available",
				&available_levels.join(", ").dimmed().to_string(),
				kw,
			);
			block_line(
				&"Usage: /loglevel <level> (e.g., /loglevel debug)"
					.dimmed()
					.to_string(),
			);
			block_close_ok("/loglevel", Some(current));
		} else {
			block_close_ok("/loglevel", None);
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
		block_open("/model", None);
		if *changed {
			let kw = key_width(["from", "to", "note", "warning"]);
			if let Some(old) = old_model {
				block_row("from", &old.bright_yellow().to_string(), kw);
				block_row("to", &new_model.bright_green().to_string(), kw);
			} else {
				block_row("set to", &new_model.bright_green().to_string(), kw);
			}
			block_row("note", &"runtime only — not saved".dimmed().to_string(), kw);
			if let Some(false) = saved {
				if let Some(err) = save_error {
					block_row("warning", &err.bright_red().to_string(), kw);
				}
			}
			let suffix = old_model
				.as_ref()
				.map(|o| format!("{} → {}", o, new_model))
				.unwrap_or_else(|| new_model.clone());
			block_close_ok("/model", Some(&suffix));
		} else {
			let kw = key_width(["current"]);
			block_row("current", &new_model.bright_white().to_string(), kw);
			block_line(
				&"Usage: /model <provider:model> (e.g., /model openai:gpt-4o)"
					.dimmed()
					.to_string(),
			);
			block_close_ok("/model", Some(new_model));
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
		block_open("/effort", None);
		if *changed {
			let kw = key_width(["from", "to", "note", "warning"]);
			if let Some(old) = old_effort {
				block_row("from", &old.bright_yellow().to_string(), kw);
				block_row("to", &new_effort.bright_green().to_string(), kw);
			} else {
				block_row("set to", &new_effort.bright_green().to_string(), kw);
			}
			block_row("note", &"runtime only — not saved".dimmed().to_string(), kw);
			if let Some(false) = saved {
				if let Some(err) = save_error {
					block_row("warning", &err.bright_red().to_string(), kw);
				}
			}
			let suffix = old_effort
				.as_ref()
				.map(|o| format!("{} → {}", o, new_effort))
				.unwrap_or_else(|| new_effort.clone());
			block_close_ok("/effort", Some(&suffix));
		} else {
			let kw = key_width(["current", "available"]);
			block_row("current", &new_effort.bright_white().to_string(), kw);
			block_row(
				"available",
				&"low, medium, high, xhigh, max".dimmed().to_string(),
				kw,
			);
			block_line(
				&"Usage: /effort <level> (e.g., /effort high)"
					.dimmed()
					.to_string(),
			);
			block_close_ok("/effort", Some(new_effort));
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
		block_open("/role", None);
		if *changed {
			let kw = key_width(["from", "to", "note", "warning"]);
			if let Some(old) = old_role {
				block_row("from", &old.bright_yellow().to_string(), kw);
				block_row("to", &new_role.bright_green().to_string(), kw);
			} else {
				block_row("set to", &new_role.bright_green().to_string(), kw);
			}
			block_row("note", &"runtime only — not saved".dimmed().to_string(), kw);
			if let Some(false) = saved {
				if let Some(err) = save_error {
					block_row("warning", &err.bright_red().to_string(), kw);
				}
			}
			let suffix = old_role
				.as_ref()
				.map(|o| format!("{} → {}", o, new_role))
				.unwrap_or_else(|| new_role.clone());
			block_close_ok("/role", Some(&suffix));
		} else if let Some(current) = current_role {
			let kw = key_width(["current"]);
			block_row("current", &current.bright_white().to_string(), kw);
			if let Some(roles) = available_roles {
				block_section("available");
				for role_name in roles {
					let marker = if role_name == current {
						"→".bright_green().to_string()
					} else {
						" ".to_string()
					};
					let line = if role_name == current {
						role_name.bright_white().to_string()
					} else {
						role_name.dimmed().to_string()
					};
					block_row_text(&format!("{} {}", marker, line));
				}
			}
			block_line(&"Usage: /role <role_name>".dimmed().to_string());
			block_close_ok("/role", Some(current));
		} else {
			block_close_ok("/role", None);
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
			block_open("/plan", None);
			if let Some(display_text) = display {
				for line in display_text.lines() {
					block_row_text(line);
				}
			}
			block_close_ok("/plan", Some("active"));
		} else {
			block_open("/plan", None);
			block_line(&"No active plan.".bright_yellow().to_string());
			block_line(
				&"Create with plan(command=\"start\", title=\"...\", tasks=[...])"
					.dimmed()
					.to_string(),
			);
			block_close_ok("/plan", Some("empty"));
		}
		println!();
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
		block_open("/info", None);

		// ── session ────────────────────────────────────────────────────
		block_section_with("session", session_name);
		let kw_sess = key_width(["model", "tokens", "breakdown", "cost", "throughput"]);
		block_row("model", &model.bright_white().to_string(), kw_sess);
		let total_tokens = tokens_used + tokens_cached + tokens_cache_write + tokens_reasoning;
		block_row(
			"tokens",
			&format!("{} total", format_number(total_tokens).bright_white()),
			kw_sess,
		);
		let dot = "·".bright_black();
		block_row(
			"breakdown",
			&format!(
				"{} in {} {} out {} {} cache rd {} {} cache wr {} {} reasoning",
				format_number(*tokens_input).bright_blue(),
				dot,
				format_number(*tokens_output).bright_green(),
				dot,
				format_number(*tokens_cached).bright_magenta(),
				dot,
				format_number(*tokens_cache_write).bright_cyan(),
				dot,
				format_number(*tokens_reasoning).white(),
			),
			kw_sess,
		);
		block_row("cost", &format!("${:.5}", total_cost), kw_sess);
		if *tokens_per_second > 0.0 {
			block_row(
				"throughput",
				&format!("{:.1} tok/s", tokens_per_second),
				kw_sess,
			);
		}

		// ── averages ───────────────────────────────────────────────────
		let mut avg_rows: Vec<(&str, String)> = Vec::new();
		if *avg_tokens_per_compression > 0.0 {
			avg_rows.push((
				"per compression",
				format_number(*avg_tokens_per_compression as u64)
					.bright_white()
					.to_string(),
			));
		}
		if *avg_tokens_per_tool > 0.0 {
			avg_rows.push((
				"per tool",
				format_number(*avg_tokens_per_tool as u64)
					.bright_white()
					.to_string(),
			));
		}
		if *avg_tokens_per_response > 0.0 {
			avg_rows.push((
				"per response",
				format_number(*avg_tokens_per_response as u64)
					.bright_white()
					.to_string(),
			));
		}
		if *avg_input_tokens > 0.0 {
			avg_rows.push((
				"per request",
				format_number(*avg_input_tokens as u64)
					.bright_white()
					.to_string(),
			));
		}
		if !avg_rows.is_empty() {
			block_section("averages");
			let kw = key_width(avg_rows.iter().map(|(k, _)| *k));
			for (k, v) in &avg_rows {
				block_row(k, v, kw);
			}
		}

		// ── compression ───────────────────────────────────────────────
		if let Some(stats) = compression_stats {
			block_section("compression");
			let kw = key_width([
				"conversation",
				"messages removed",
				"tokens saved",
				"avg ratio",
			]);
			if stats.conversation_compressions > 0 {
				block_row(
					"conversation",
					&format_number(stats.conversation_compressions as u64)
						.bright_white()
						.to_string(),
					kw,
				);
			}
			block_row(
				"messages removed",
				&format_number(stats.total_messages_removed as u64)
					.bright_green()
					.to_string(),
				kw,
			);
			block_row(
				"tokens saved",
				&format_number(stats.total_tokens_saved)
					.bright_green()
					.to_string(),
				kw,
			);
			let avg_ratio = stats.avg_compression_ratio() * 100.0;
			if avg_ratio > 0.0 {
				block_row("avg ratio", &format!("{:.1}%", avg_ratio), kw);
			}
		}

		// ── cache ──────────────────────────────────────────────────────
		block_section("cache");
		let kw = key_width([
			"system markers",
			"tool markers",
			"content markers",
			"cache read",
			"cache write",
			"non-cached",
		]);
		block_row("system markers", &cache_markers_system.to_string(), kw);
		block_row("tool markers", &cache_markers_tool.to_string(), kw);
		block_row("content markers", &cache_markers_content.to_string(), kw);
		block_row(
			"cache read",
			&format_number(*tokens_cached).bright_magenta().to_string(),
			kw,
		);
		block_row(
			"cache write",
			&format_number(*tokens_cache_write).bright_cyan().to_string(),
			kw,
		);
		block_row(
			"non-cached",
			&format_number(*cache_non_cached_tokens)
				.bright_white()
				.to_string(),
			kw,
		);

		block_close_ok("/info", Some(session_name));
		println!();
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
		block_open("/image", None);
		if *image_attached {
			let kw = key_width(["path", "note"]);
			if let Some(p) = path {
				let display_path = if p == "clipboard" { "clipboard" } else { p };
				block_row("path", &display_path.bright_white().to_string(), kw);
			}
			block_row(
				"note",
				&"will be attached to next message".dimmed().to_string(),
				kw,
			);
			block_close_ok("/image", Some("attached"));
		} else if let Some(err) = error {
			block_close_err("/image", err);
		} else {
			block_section("usage");
			block_row_text("/image <path_or_url>");
			block_section("examples");
			for ex in &[
				"/image screenshot.png",
				"/image /path/to/image.jpg",
				"/image https://example.com/image.png",
			] {
				block_row_text(&ex.dimmed().to_string());
			}
			block_line(
				&"Formats: PNG, JPEG, GIF, WebP, BMP — or clipboard"
					.dimmed()
					.to_string(),
			);
			block_close_ok("/image", Some("usage"));
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
		block_open("/video", None);
		if *video_attached {
			let kw = key_width(["path", "note"]);
			if let Some(p) = path {
				block_row("path", &p.bright_white().to_string(), kw);
			}
			block_row(
				"note",
				&"will be attached to next message".dimmed().to_string(),
				kw,
			);
			block_close_ok("/video", Some("attached"));
		} else if let Some(err) = error {
			block_close_err("/video", err);
		} else {
			block_section("usage");
			block_row_text("/video <path_or_url>");
			block_section("examples");
			for ex in &[
				"/video recording.mp4",
				"/video /path/to/video.mov",
				"/video https://example.com/video.mp4",
			] {
				block_row_text(&ex.dimmed().to_string());
			}
			block_line(
				&"Formats: MP4, MOV, AVI, WebM, MKV, M4V, 3GP — max 100MB"
					.dimmed()
					.to_string(),
			);
			block_close_ok("/video", Some("usage"));
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
						block_open("/prompt", None);
						if prompts.is_empty() {
							block_line(
								&"No prompt templates configured."
									.bright_yellow()
									.to_string(),
							);
							block_line(
								&"Define in the [[prompts]] section of your config."
									.dimmed()
									.to_string(),
							);
							block_close_ok("/prompt", Some("empty"));
						} else {
							block_section("templates");
							let name_width = prompts
								.iter()
								.filter_map(|p| p.get("name").and_then(|v| v.as_str()))
								.map(|n| n.len())
								.max()
								.unwrap_or(0)
								.min(20);
							for prompt in prompts {
								let name =
									prompt.get("name").and_then(|v| v.as_str()).unwrap_or("");
								let description = prompt
									.get("description")
									.and_then(|v| v.as_str())
									.unwrap_or("");
								block_row(name, &description.dimmed().to_string(), name_width);
							}
							block_line(&"Usage: /prompt <template_name>".dimmed().to_string());
							block_close_ok(
								"/prompt",
								Some(&format!("{} template(s)", prompts.len())),
							);
						}
						println!();
					}
				}
				"execute" => {
					block_open("/prompt", None);
					if let Some(true) = data.get("success").and_then(|v| v.as_bool()) {
						if let Some(name) = data.get("prompt_name").and_then(|v| v.as_str()) {
							let kw = key_width(["applied"]);
							block_row("applied", &name.bright_green().to_string(), kw);
							block_close_ok("/prompt", Some(name));
						} else {
							block_close_ok("/prompt", None);
						}
					} else if let Some(error) = data.get("error").and_then(|v| v.as_str()) {
						if let Some(available) =
							data.get("available_prompts").and_then(|v| v.as_array())
						{
							if !available.is_empty() {
								block_section("available");
								for prompt in available {
									if let Some(name) = prompt.as_str() {
										block_row_text(&name.dimmed().to_string());
									}
								}
							}
						}
						block_close_err("/prompt", error);
					}
					println!();
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
			block_open("/done", None);
			let kw = key_width(["memorized", "summarized", "saved"]);
			if let Some(true) = memorized {
				block_row("memorized", &"insights persisted".dimmed().to_string(), kw);
			}
			if let Some(true) = summarized {
				block_row("summarized", &"session compressed".dimmed().to_string(), kw);
			}
			if let Some(true) = saved {
				block_row("saved", &"session written to disk".dimmed().to_string(), kw);
			}
			block_line(
				&"Layered processing reset for next task."
					.bright_cyan()
					.to_string(),
			);
			block_close_ok("/done", Some("task finalized"));
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
						block_open("/run", None);
						if commands.is_empty() {
							block_line(
								&"No command layers configured.".bright_yellow().to_string(),
							);
							block_line(
								&"Define in the global [[commands]] section of your config."
									.dimmed()
									.to_string(),
							);
							block_close_ok("/run", Some("empty"));
						} else {
							block_section("commands");
							for cmd in commands {
								if let Some(name) = cmd.as_str() {
									block_row_text(&format!(
										"{} {}",
										"/run".cyan(),
										name.bright_yellow()
									));
								}
							}
							block_line(&"Usage: /run <command_name>".dimmed().to_string());
							block_close_ok("/run", Some(&format!("{} command(s)", commands.len())));
						}
						println!();
					}
				}
				"execute" => {
					if let Some(false) = data.get("success").and_then(|v| v.as_bool()) {
						block_open("/run", None);
						if let Some(available) =
							data.get("available_commands").and_then(|v| v.as_array())
						{
							if !available.is_empty() {
								block_section("available");
								for cmd in available {
									if let Some(name) = cmd.as_str() {
										block_row_text(&name.dimmed().to_string());
									}
								}
							}
						}
						let error = data
							.get("error")
							.and_then(|v| v.as_str())
							.unwrap_or("unknown error");
						block_close_err("/run", error);
						println!();
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
						block_open("/workflow", None);
						if workflows.is_empty() {
							block_line(&"No workflows configured.".bright_yellow().to_string());
							block_line(
								&"Define in the [workflows] section of your config."
									.dimmed()
									.to_string(),
							);
							block_close_ok("/workflow", Some("empty"));
						} else {
							block_section("workflows");
							let name_width = workflows
								.iter()
								.filter_map(|w| w.get(0).and_then(|v| v.as_str()))
								.map(|n| n.len())
								.max()
								.unwrap_or(0)
								.min(20);
							for workflow in workflows {
								if let (Some(name), Some(desc)) = (
									workflow.get(0).and_then(|v| v.as_str()),
									workflow.get(1).and_then(|v| v.as_str()),
								) {
									block_row(name, &desc.dimmed().to_string(), name_width);
								}
							}
							block_line(
								&"Usage: /workflow <workflow_name> [input]"
									.dimmed()
									.to_string(),
							);
							block_close_ok(
								"/workflow",
								Some(&format!("{} workflow(s)", workflows.len())),
							);
						}
						println!();
					}
				}
				"execute" => {
					if let Some(false) = data.get("success").and_then(|v| v.as_bool()) {
						block_open("/workflow", None);
						if let Some(available) =
							data.get("available_workflows").and_then(|v| v.as_array())
						{
							if !available.is_empty() {
								block_section("available");
								for workflow in available {
									if let Some(name) = workflow.as_str() {
										block_row_text(&name.dimmed().to_string());
									}
								}
							}
						}
						let error = data
							.get("error")
							.and_then(|v| v.as_str())
							.unwrap_or("unknown error");
						block_close_err("/workflow", error);
						println!();
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
				block_open("/mcp", None);
				if let Some(message) = data.get("message").and_then(|v| v.as_str()) {
					block_line(message);
				}
				block_close_ok("/mcp", None);
				println!();
			}
		}
	}
}

/// Format MCP server health into a glyph + label for inline display.
fn mcp_health_display(health: &str) -> String {
	match health {
		"running" => "✓ running".bright_green().to_string(),
		"dead" => "✗ dead".bright_red().to_string(),
		"restarting" => "↻ restarting".bright_yellow().to_string(),
		"failed" => "✗ failed".bright_red().to_string(),
		"unreachable" => "✗ auth failed".bright_red().to_string(),
		other => other.normal().to_string(),
	}
}

fn display_mcp_list(data: &serde_json::Value) {
	block_open("/mcp list", None);
	if let Some(servers) = data.get("servers").and_then(|v| v.as_object()) {
		if servers.is_empty() {
			block_line(&"No tools available.".yellow().to_string());
			block_close_ok("/mcp list", Some("empty"));
		} else {
			let mut total_tools = 0usize;
			for (server_name, tools) in servers {
				block_section(server_name);
				if let Some(tool_array) = tools.as_array() {
					for tool in tool_array {
						if let Some(tool_name) = tool.as_str() {
							block_row_text(&tool_name.bright_white().to_string());
							total_tools += 1;
						}
					}
				}
			}
			block_line(
				&"Use '/mcp info' for descriptions or '/mcp full' for parameters."
					.dimmed()
					.to_string(),
			);
			block_close_ok(
				"/mcp list",
				Some(&format!(
					"{} server(s) · {} tool(s)",
					servers.len(),
					total_tools
				)),
			);
		}
	} else {
		block_close_ok("/mcp list", Some("empty"));
	}
	println!();
}

fn display_mcp_info(data: &serde_json::Value) {
	block_open("/mcp info", None);

	if let Some(message) = data.get("message").and_then(|v| v.as_str()) {
		block_line(&message.yellow().to_string());
		block_close_ok("/mcp info", None);
		println!();
		return;
	}

	// Server status section
	let server_count = data
		.get("servers")
		.and_then(|v| v.as_array())
		.map(|a| a.len())
		.unwrap_or(0);
	if server_count > 0 {
		block_section("servers");
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

				block_row_text(&format!(
					"{}  {}  {}{}",
					name.bright_white().bold(),
					mcp_health_display(health),
					format!("({})", conn_type).dimmed(),
					if restart_count > 0 {
						format!(
							" · restarts: {} · failures: {}",
							restart_count, consecutive_failures
						)
						.dimmed()
						.to_string()
					} else {
						String::new()
					},
				));
				if let Some(tools) = server.get("tools").and_then(|v| v.as_array()) {
					let tool_names: Vec<&str> = tools.iter().filter_map(|t| t.as_str()).collect();
					if !tool_names.is_empty() {
						block_row_text(
							&format!("  configured: {}", tool_names.join(", "))
								.dimmed()
								.to_string(),
						);
					}
				}
			}
		}
	}

	// Tools section
	let mut total_tools = 0usize;
	if let Some(tools) = data.get("tools").and_then(|v| v.as_object()) {
		if tools.is_empty() {
			block_section("tools");
			block_row_text(&"No tools available.".yellow().to_string());
		} else {
			for (server_name, tool_list) in tools {
				block_section(&format!("tools · {}", server_name));
				if let Some(tool_array) = tool_list.as_array() {
					let name_width = tool_array
						.iter()
						.filter_map(|t| t.get("name").and_then(|v| v.as_str()))
						.map(|n| n.len())
						.max()
						.unwrap_or(0)
						.min(24);
					for tool in tool_array {
						let name = tool
							.get("name")
							.and_then(|v| v.as_str())
							.unwrap_or("unknown");
						let desc = tool
							.get("description")
							.and_then(|v| v.as_str())
							.unwrap_or("");
						block_row(name, &desc.dimmed().to_string(), name_width);
						total_tools += 1;
					}
				}
			}
		}
	}

	block_line(
		&"Use '/mcp list' for names only or '/mcp full' for parameters."
			.dimmed()
			.to_string(),
	);
	block_close_ok(
		"/mcp info",
		Some(&format!(
			"{} server(s) · {} tool(s)",
			server_count, total_tools
		)),
	);
	println!();
}

fn display_mcp_full(data: &serde_json::Value) {
	block_open("/mcp full", None);

	if let Some(msg) = data.get("message").and_then(|v| v.as_str()) {
		block_line(&msg.yellow().to_string());
		block_close_ok("/mcp full", None);
		println!();
		return;
	}

	// Server status section
	let server_count = data
		.get("servers")
		.and_then(|v| v.as_array())
		.map(|a| a.len())
		.unwrap_or(0);
	if server_count > 0 {
		block_section("servers");
		if let Some(servers) = data.get("servers").and_then(|v| v.as_array()) {
			for server in servers {
				let name = server.get("name").and_then(|v| v.as_str()).unwrap_or("");
				let health = server.get("health").and_then(|v| v.as_str()).unwrap_or("");
				let conn_type = server
					.get("connection_type")
					.and_then(|v| v.as_str())
					.unwrap_or("");
				let restart_count = server
					.get("restart_count")
					.and_then(|v| v.as_u64())
					.unwrap_or(0);
				let failures = server
					.get("consecutive_failures")
					.and_then(|v| v.as_u64())
					.unwrap_or(0);

				block_row_text(&format!(
					"{}  {}  {}{}",
					name.bright_white().bold(),
					mcp_health_display(health),
					format!("({})", conn_type).dimmed(),
					if restart_count > 0 {
						format!(" · restarts: {} · failures: {}", restart_count, failures)
							.dimmed()
							.to_string()
					} else {
						String::new()
					},
				));
				if let Some(tools) = server.get("tools").and_then(|v| v.as_array()) {
					let tool_names: Vec<&str> = tools.iter().filter_map(|t| t.as_str()).collect();
					if !tool_names.is_empty() {
						block_row_text(
							&format!("  configured: {}", tool_names.join(", "))
								.dimmed()
								.to_string(),
						);
					}
				}
			}
		}
	}

	// Tools with parameters
	let mut total_tools = 0usize;
	if let Some(tools_by_server) = data.get("tools").and_then(|v| v.as_object()) {
		if tools_by_server.is_empty() {
			block_section("tools");
			block_row_text(&"No tools available.".yellow().to_string());
		} else {
			for (server_name, tools) in tools_by_server {
				block_section(&format!("tools · {}", server_name));
				if let Some(tools_arr) = tools.as_array() {
					for tool in tools_arr {
						let name = tool.get("name").and_then(|v| v.as_str()).unwrap_or("");
						let desc = tool
							.get("description")
							.and_then(|v| v.as_str())
							.unwrap_or("");
						block_row_text(&name.bright_white().bold().to_string());
						if !desc.is_empty() {
							block_row_text(&format!("  {}", desc.dimmed()));
						}
						if let Some(params) = tool.get("parameters") {
							if let Some(props) =
								params.get("properties").and_then(|v| v.as_object())
							{
								if !props.is_empty() {
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
											"*".bright_red().to_string()
										} else {
											" ".normal().to_string()
										};
										let ptype = param_info
											.get("type")
											.and_then(|v| v.as_str())
											.unwrap_or("any");
										let pdesc = param_info
											.get("description")
											.and_then(|v| v.as_str())
											.unwrap_or("");
										let suffix = if !pdesc.is_empty() {
											format!(" — {}", pdesc).dimmed().to_string()
										} else {
											String::new()
										};
										block_row_text(&format!(
											"  {}{}: {}{}",
											marker,
											param_name.bright_cyan(),
											ptype.yellow(),
											suffix,
										));
										if let Some(enum_vals) =
											param_info.get("enum").and_then(|v| v.as_array())
										{
											let vals: Vec<&str> = enum_vals
												.iter()
												.filter_map(|v| v.as_str())
												.collect();
											if !vals.is_empty() {
												block_row_text(&format!(
													"      options: {}",
													vals.join(", ").bright_black()
												));
											}
										}
										if let Some(default_val) = param_info.get("default") {
											block_row_text(&format!(
												"      default: {}",
												default_val.to_string().bright_black()
											));
										}
									}
								}
							} else if *params != serde_json::json!({}) {
								block_row_text(&format!(
									"  schema: {}",
									params.to_string().dimmed()
								));
							}
						}
						total_tools += 1;
					}
				}
			}
			block_blank();
			block_line(
				&format!("Legend: {} required parameter", "*".bright_red())
					.dimmed()
					.to_string(),
			);
		}
	}

	block_close_ok(
		"/mcp full",
		Some(&format!(
			"{} server(s) · {} tool(s)",
			server_count, total_tools
		)),
	);
	println!();
}

fn display_mcp_health(data: &serde_json::Value) {
	block_open("/mcp health", None);

	if let Some(msg) = data.get("message").and_then(|v| v.as_str()) {
		block_line(&msg.yellow().to_string());
		block_close_ok("/mcp health", None);
		println!();
		return;
	}

	let monitor_running = data
		.get("monitor_running")
		.and_then(|v| v.as_bool())
		.unwrap_or(false);
	let kw = key_width(["monitor"]);
	block_row(
		"monitor",
		&if monitor_running {
			"running".bright_green().to_string()
		} else {
			"stopped".bright_red().to_string()
		},
		kw,
	);

	if let Some(error) = data.get("error").and_then(|v| v.as_str()) {
		block_close_err("/mcp health", error);
		println!();
		return;
	}

	let mut count = 0usize;
	if let Some(servers) = data.get("servers").and_then(|v| v.as_array()) {
		if !servers.is_empty() {
			block_section("servers");
			for server in servers {
				let name = server.get("name").and_then(|v| v.as_str()).unwrap_or("");
				let health = server.get("health").and_then(|v| v.as_str()).unwrap_or("");
				let restart_count = server
					.get("restart_count")
					.and_then(|v| v.as_u64())
					.unwrap_or(0);
				let failures = server
					.get("consecutive_failures")
					.and_then(|v| v.as_u64())
					.unwrap_or(0);
				let last = server.get("last_checked_secs_ago").and_then(|v| v.as_u64());

				let mut extras = Vec::new();
				if restart_count > 0 {
					extras.push(format!("restarts: {}", restart_count));
				}
				if failures > 0 {
					extras.push(format!("failures: {}", failures));
				}
				if let Some(secs) = last {
					extras.push(format!("checked {}s ago", secs));
				}
				let extras_str = if extras.is_empty() {
					String::new()
				} else {
					format!(" · {}", extras.join(" · ")).dimmed().to_string()
				};
				block_row_text(&format!(
					"{}  {}{}",
					name.bright_white().bold(),
					mcp_health_display(health),
					extras_str,
				));
				count += 1;
			}
		}
	}

	block_line(
		&"Dead servers will be automatically restarted by the monitor."
			.dimmed()
			.to_string(),
	);
	block_close_ok("/mcp health", Some(&format!("{} server(s)", count)));
	println!();
}

fn display_mcp_dump(data: &serde_json::Value) {
	block_open("/mcp dump", None);
	if let Some(tools) = data.get("tools").and_then(|v| v.as_array()) {
		if tools.is_empty() {
			block_line(&"No tools available.".yellow().to_string());
			block_close_ok("/mcp dump", Some("empty"));
		} else {
			for (i, tool) in tools.iter().enumerate() {
				let name = tool.get("name").and_then(|v| v.as_str()).unwrap_or("");
				block_section(&format!("{}. {}", i + 1, name));
				let json = serde_json::to_string_pretty(tool).unwrap_or_default();
				for line in json.lines() {
					block_row_text(line);
				}
			}
			block_close_ok("/mcp dump", Some(&format!("{} tool(s)", tools.len())));
		}
	} else {
		block_close_ok("/mcp dump", Some("empty"));
	}
	println!();
}

fn display_mcp_validate(data: &serde_json::Value) {
	block_open("/mcp validate", None);

	let tools = match data.get("tools").and_then(|v| v.as_array()) {
		Some(t) if !t.is_empty() => t,
		_ => {
			block_line(&"No tools available to validate.".yellow().to_string());
			block_close_ok("/mcp validate", Some("empty"));
			println!();
			return;
		}
	};

	let mut valid_count = 0usize;
	let mut invalid_count = 0usize;
	for tool in tools {
		let name = tool.get("name").and_then(|v| v.as_str()).unwrap_or("");
		let valid = tool.get("valid").and_then(|v| v.as_bool()).unwrap_or(false);
		if valid {
			block_row_text(&format!("{} {}", "✓".bright_green(), name.bright_white()));
			valid_count += 1;
		} else {
			block_row_text(&format!("{} {}", "✗".bright_red(), name.bright_red()));
			if let Some(issues) = tool.get("issues").and_then(|v| v.as_array()) {
				for issue in issues {
					if let Some(s) = issue.as_str() {
						block_row_text(&format!("    {} {}", "-".dimmed(), s.yellow()));
					}
				}
			}
			invalid_count += 1;
		}
	}

	let all_valid = data
		.get("all_valid")
		.and_then(|v| v.as_bool())
		.unwrap_or(false);
	if all_valid {
		block_close_ok(
			"/mcp validate",
			Some(&format!("all {} schema(s) valid", valid_count)),
		);
	} else {
		block_close_err(
			"/mcp validate",
			&format!("{} invalid, {} valid", invalid_count, valid_count),
		);
	}
	println!();
}

fn display_mcp_invalid(_data: &serde_json::Value) {
	block_open("/mcp", None);
	block_line(&"Invalid subcommand.".bright_red().to_string());
	block_section("subcommands");
	let entries: &[(&str, &str)] = &[
		("list", "Show tool names only"),
		("info", "Show server status and tools (default)"),
		("full", "Show full details including parameters"),
		("health", "Check server health and attempt restart"),
		("dump", "Dump raw tool definitions in JSON"),
		("validate", "Validate tool schema definitions"),
	];
	let kw = key_width(entries.iter().map(|(k, _)| *k));
	for (name, desc) in entries {
		block_row(name, &desc.dimmed().to_string(), kw);
	}
	block_close_err("/mcp", "unknown subcommand");
	println!();
}

pub fn display_report(output: &CommandOutput, _config: &Config) {
	use crate::session::chat::formatting::format_duration;

	if let CommandOutput::Report { entries, totals } = output {
		block_open("/report", None);

		if entries.is_empty() {
			block_line(&"No requests recorded yet.".yellow().to_string());
			block_close_ok("/report", Some("empty"));
			println!();
			return;
		}

		// Per-request entries — each becomes one section.
		for (i, entry) in entries.iter().enumerate() {
			let user_request = entry
				.get("user_request")
				.and_then(|v| v.as_str())
				.unwrap_or("");
			let cost = entry.get("cost").and_then(|v| v.as_str()).unwrap_or("$0");
			let tool_calls = entry
				.get("tool_calls")
				.and_then(|v| v.as_u64())
				.unwrap_or(0);
			let tools_used = entry
				.get("tools_used")
				.and_then(|v| v.as_str())
				.unwrap_or("");
			let task_time = entry
				.get("task_time")
				.and_then(|v| v.as_str())
				.unwrap_or("");
			let ai_time = entry.get("ai_time").and_then(|v| v.as_str()).unwrap_or("");
			let processing_time = entry
				.get("processing_time")
				.and_then(|v| v.as_str())
				.unwrap_or("");

			block_section(&format!("#{}", i + 1));
			let kw = key_width([
				"request",
				"cost",
				"tools",
				"task time",
				"ai time",
				"processing",
			]);
			let request_preview: String = if user_request.chars().count() > 80 {
				format!("{}…", user_request.chars().take(79).collect::<String>())
			} else {
				user_request.to_string()
			};
			block_row("request", &request_preview.bright_white().to_string(), kw);
			block_row("cost", &cost.bright_yellow().to_string(), kw);
			block_row(
				"tools",
				&format!(
					"{} call(s){}",
					tool_calls,
					if tools_used.is_empty() {
						String::new()
					} else {
						format!(" · {}", tools_used).dimmed().to_string()
					}
				),
				kw,
			);
			if !task_time.is_empty() {
				block_row("task time", task_time, kw);
			}
			if !ai_time.is_empty() {
				block_row("ai time", ai_time, kw);
			}
			if !processing_time.is_empty() {
				block_row("processing", processing_time, kw);
			}
		}

		// Totals section.
		block_section("totals");
		let total_cost = totals
			.get("total_cost")
			.and_then(|v| v.as_f64())
			.unwrap_or(0.0);
		let total_tool_calls = totals
			.get("total_tool_calls")
			.and_then(|v| v.as_u64())
			.unwrap_or(0);
		let total_task = totals
			.get("total_task_time_ms")
			.and_then(|v| v.as_u64())
			.unwrap_or(0);
		let total_ai = totals
			.get("total_ai_time_ms")
			.and_then(|v| v.as_u64())
			.unwrap_or(0);
		let total_proc = totals
			.get("total_processing_time_ms")
			.and_then(|v| v.as_u64())
			.unwrap_or(0);
		let kw = key_width([
			"requests",
			"cost",
			"tools",
			"task time",
			"ai time",
			"processing",
		]);
		block_row("requests", &entries.len().to_string(), kw);
		block_row(
			"cost",
			&format!("${:.5}", total_cost).bright_yellow().to_string(),
			kw,
		);
		block_row("tools", &total_tool_calls.to_string(), kw);
		if total_task > 0 {
			block_row("task time", &format_duration(total_task), kw);
		}
		if total_ai > 0 {
			block_row("ai time", &format_duration(total_ai), kw);
		}
		if total_proc > 0 {
			block_row("processing", &format_duration(total_proc), kw);
		}

		block_close_ok(
			"/report",
			Some(&format!(
				"{} request(s) · ${:.5}",
				entries.len(),
				total_cost
			)),
		);
		println!();
	}
}

pub fn display_session(output: &CommandOutput) {
	if let CommandOutput::Session {
		switched,
		session_name,
	} = output
	{
		block_open("/session", None);
		if *switched {
			let kw = key_width(["new"]);
			block_row("new", &session_name.bright_green().to_string(), kw);
			block_close_ok("/session", Some(session_name));
		} else {
			block_line(&"You are already in this session.".blue().to_string());
			block_close_ok("/session", Some(session_name));
		}
		println!();
	}
}

pub fn display_list(output: &CommandOutput, _config: &Config) {
	if let CommandOutput::List {
		sessions,
		total_sessions,
		page,
		total_pages,
		plain_text,
	} = output
	{
		block_open("/list", None);

		if sessions.is_empty() {
			block_line(&"No sessions found.".yellow().to_string());
			block_close_ok("/list", Some("empty"));
			println!();
			return;
		}

		// Each session = one section. Pull fields from the JSON entries directly so
		// we don't depend on the markdown-rendered fallback path.
		for entry in sessions {
			let name = entry.get("name").and_then(|v| v.as_str()).unwrap_or("?");
			let model = entry.get("model").and_then(|v| v.as_str()).unwrap_or("");
			let messages = entry.get("messages").and_then(|v| v.as_u64()).unwrap_or(0);
			let tokens = entry
				.get("total_tokens")
				.and_then(|v| v.as_u64())
				.unwrap_or(0);
			let cost = entry
				.get("total_cost")
				.and_then(|v| v.as_f64())
				.unwrap_or(0.0);
			let updated = entry
				.get("updated_at")
				.and_then(|v| v.as_str())
				.unwrap_or("");

			block_section_with("session", name);
			let kw = key_width(["model", "messages", "tokens", "cost", "updated"]);
			if !model.is_empty() {
				block_row("model", &model.dimmed().to_string(), kw);
			}
			block_row("messages", &messages.to_string(), kw);
			if tokens > 0 {
				block_row(
					"tokens",
					&crate::session::chat::format_number(tokens)
						.bright_white()
						.to_string(),
					kw,
				);
			}
			if cost > 0.0 {
				block_row(
					"cost",
					&format!("${:.5}", cost).bright_yellow().to_string(),
					kw,
				);
			}
			if !updated.is_empty() {
				block_row("updated", &updated.dimmed().to_string(), kw);
			}
		}

		if *total_pages > 1 {
			let mut nav = Vec::new();
			if *page > 1 {
				nav.push(format!("/list {}", page - 1));
			}
			if *page < *total_pages {
				nav.push(format!("/list {}", page + 1));
			}
			block_line(
				&format!("Page {}/{}  {}", page, total_pages, nav.join("  "))
					.dimmed()
					.to_string(),
			);
		}

		// Suppress unused warning when plain_text is None.
		let _ = plain_text;

		block_close_ok(
			"/list",
			Some(&format!(
				"{} session(s) · page {}/{}",
				total_sessions, page, total_pages
			)),
		);
		println!();
	}
}

// ---------------------------------------------------------------------------
// /skill display
// ---------------------------------------------------------------------------

pub(super) fn display_schedule(output: &CommandOutput) {
	if let CommandOutput::Schedule { data } = output {
		let subcommand = data
			.get("subcommand")
			.and_then(|v| v.as_str())
			.unwrap_or("");

		match subcommand {
			"error" => {
				block_open("/schedule", None);
				let msg = data
					.get("message")
					.and_then(|v| v.as_str())
					.unwrap_or("unknown error");
				block_close_err("/schedule", msg);
				println!();
			}
			"help" => {
				block_open("/schedule", Some("inject a user message later"));
				block_section("usage");
				let entries: &[(&str, &str)] = &[
					("/schedule", "list pending entries"),
					("/schedule list", "list pending entries"),
					("/schedule remove <id>", "cancel an entry"),
					("/schedule add when=… message=…", "add a new entry"),
					(
						"/schedule edit <id> [when=…] [message=…]",
						"update an entry",
					),
				];
				let cmd_width = entries
					.iter()
					.map(|(c, _)| c.len())
					.max()
					.unwrap_or(0)
					.min(40);
				for (cmd, desc) in entries {
					block_row(cmd, &desc.dimmed().to_string(), cmd_width);
				}
				block_section("keys (add/edit)");
				let kws: &[(&str, &str)] = &[
					(
						"when",
						"\"now\"; \"in 5m\", \"in 1h30m\"; \"15:30\", \"9am\"; \"2026-03-22 15:30\"",
					),
					("message", "text injected verbatim when timer fires"),
					(
						"every",
						"repeat interval — \"10m\", \"1h\", \"1h30m\"; \"none\" clears",
					),
					("description", "short label shown in list output"),
				];
				let key_w = key_width(kws.iter().map(|(k, _)| *k));
				for (k, v) in kws {
					block_row(k, &v.dimmed().to_string(), key_w);
				}
				block_line(
					&"Quote values with spaces: when=\"in 1h 30m\" message='hello world'"
						.dimmed()
						.to_string(),
				);
				block_close_ok("/schedule", Some("help"));
				println!();
			}
			_ => {
				let is_error = data
					.get("is_error")
					.and_then(|v| v.as_bool())
					.unwrap_or(false);
				let msg = data.get("message").and_then(|v| v.as_str()).unwrap_or("");
				block_open("/schedule", None);
				for line in msg.lines() {
					block_row_text(line);
				}
				if is_error {
					block_close_err("/schedule", "failed");
				} else {
					block_close_ok("/schedule", None);
				}
				println!();
			}
		}
	}
}

pub(super) fn display_skill(output: &CommandOutput) {
	if let CommandOutput::Skill { data } = output {
		let subcommand = data
			.get("subcommand")
			.and_then(|v| v.as_str())
			.unwrap_or("");

		match subcommand {
			"list" => display_skill_list(data),
			"use" => {
				if let Some(name) = data.get("name").and_then(|v| v.as_str()) {
					block_open("/skill", None);
					let kw = key_width(["enabled"]);
					block_row("enabled", &name.bright_green().to_string(), kw);
					block_close_ok("/skill", Some(name));
					println!();
				}
			}
			"forget" => {
				if let Some(name) = data.get("name").and_then(|v| v.as_str()) {
					block_open("/skill", None);
					let kw = key_width(["disabled"]);
					block_row("disabled", &name.bright_yellow().to_string(), kw);
					block_close_ok("/skill", Some(name));
					println!();
				}
			}
			"error" => {
				block_open("/skill", None);
				let msg = data
					.get("message")
					.and_then(|v| v.as_str())
					.unwrap_or("unknown error");
				block_close_err("/skill", msg);
				println!();
			}
			_ => {}
		}
	}
}

fn display_skill_list(data: &serde_json::Value) {
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

	let subtitle = if pattern.is_empty() {
		format!("{} available · {} active", total, active_count)
	} else {
		format!(
			"filter '{}' · {} found · {} active",
			pattern, total, active_count
		)
	};
	block_open("/skill", Some(&subtitle));

	let skills = match data.get("skills").and_then(|v| v.as_array()) {
		Some(s) if !s.is_empty() => s,
		_ => {
			block_line(&"No skills found.".yellow().to_string());
			block_close_ok("/skill", Some("empty"));
			println!();
			return;
		}
	};

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

		// Section header: `name` (with active marker as suffix value).
		if is_active {
			block_section_with(name, "active");
		} else {
			block_section(&name.bright_white().to_string());
		}

		// Description on indented line(s), truncated.
		let desc_display = if desc.chars().count() > 80 {
			format!("{}…", desc.chars().take(79).collect::<String>())
		} else {
			desc.to_string()
		};
		if !desc_display.is_empty() {
			block_row_text(&desc_display.dimmed().to_string());
		}

		let mut meta = Vec::new();
		if !capabilities.is_empty() {
			meta.push(format!("capabilities: {}", capabilities));
		}
		if !domains.is_empty() {
			meta.push(format!("domains: {}", domains));
		}
		if !scripts.is_empty() {
			meta.push(format!("scripts: {}", scripts));
		}
		if !meta.is_empty() {
			block_row_text(&meta.join(" | ").dimmed().to_string());
		}
	}

	if total_pages > 1 {
		let mut nav = Vec::new();
		if page > 1 {
			nav.push(format!("/skill {}", page - 1));
		}
		if page < total_pages {
			nav.push(format!("/skill {}", page + 1));
		}
		block_line(
			&format!("Page {}/{}  {}", page, total_pages, nav.join("  "))
				.dimmed()
				.to_string(),
		);
	}
	block_line(
		&"Use '/skill <name>' to toggle, '/skill *pattern*' to filter."
			.dimmed()
			.to_string(),
	);
	block_close_ok(
		"/skill",
		Some(&format!("{} skill(s) · {} active", total, active_count)),
	);
	println!();
}

pub fn display_learning(output: &CommandOutput) {
	let data = match output {
		CommandOutput::Learning { data } => data,
		_ => return,
	};

	let subcommand = data
		.get("subcommand")
		.and_then(|v| v.as_str())
		.unwrap_or("");

	match subcommand {
		"list" => {
			let role = data.get("role").and_then(|v| v.as_str()).unwrap_or("?");
			let project = data.get("project").and_then(|v| v.as_str()).unwrap_or("?");
			let total = data.get("total").and_then(|v| v.as_u64()).unwrap_or(0);
			let page = data.get("page").and_then(|v| v.as_u64()).unwrap_or(1);
			let total_pages = data
				.get("total_pages")
				.and_then(|v| v.as_u64())
				.unwrap_or(0);
			let pattern = data.get("pattern").and_then(|v| v.as_str());

			let subtitle = if let Some(pat) = pattern {
				format!("{}/{} · filter '{}'", role, project, pat)
			} else {
				format!("{}/{}", role, project)
			};
			block_open("/learning", Some(&subtitle));

			let lessons = match data.get("lessons").and_then(|v| v.as_array()) {
				Some(l) if !l.is_empty() => l,
				_ => {
					block_line(&"No lessons found.".yellow().to_string());
					block_close_ok("/learning", Some("empty"));
					println!();
					return;
				}
			};

			for lesson in lessons {
				let index = lesson.get("index").and_then(|v| v.as_u64()).unwrap_or(0);
				let content = lesson.get("content").and_then(|v| v.as_str()).unwrap_or("");
				let importance = lesson
					.get("importance")
					.and_then(|v| v.as_f64())
					.unwrap_or(0.5);
				let confidence = lesson
					.get("confidence")
					.and_then(|v| v.as_str())
					.unwrap_or("");
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

				let imp_indicator = if importance >= 0.7 {
					"[high]".bright_yellow().to_string()
				} else if importance >= 0.4 {
					"[med] ".dimmed().to_string()
				} else {
					"[low] ".dimmed().to_string()
				};

				let content_display = if content.chars().count() > 80 {
					format!("{}…", content.chars().take(79).collect::<String>())
				} else {
					content.to_string()
				};

				block_section(&format!("#{} {}", index, imp_indicator));
				block_row_text(&content_display.bright_white().to_string());

				let mut meta = Vec::new();
				if !confidence.is_empty() {
					meta.push(format!("confidence: {}", confidence));
				}
				if !tags.is_empty() {
					meta.push(format!("tags: {}", tags));
				}
				if !created.is_empty() {
					let date: String = created.chars().take(10).collect();
					meta.push(format!("created: {}", date));
				}
				if !meta.is_empty() {
					block_row_text(&meta.join(" | ").dimmed().to_string());
				}
			}

			if total_pages > 1 {
				let mut nav = Vec::new();
				if page > 1 {
					nav.push(format!("/learning list {}", page - 1));
				}
				if page < total_pages {
					nav.push(format!("/learning list {}", page + 1));
				}
				block_line(
					&format!("Page {}/{}  {}", page, total_pages, nav.join("  "))
						.dimmed()
						.to_string(),
				);
			}
			block_line(
				&"/learning delete <n> · /learning clear"
					.dimmed()
					.to_string(),
			);
			block_close_ok("/learning", Some(&format!("{} lesson(s)", total)));
			println!();
		}
		"delete" => {
			let index = data.get("index").and_then(|v| v.as_u64()).unwrap_or(0);
			let preview = data
				.get("content_preview")
				.and_then(|v| v.as_str())
				.unwrap_or("");
			block_open("/learning", None);
			let kw = key_width(["deleted"]);
			block_row(
				"deleted",
				&format!("#{}: {}…", index, preview)
					.bright_green()
					.to_string(),
				kw,
			);
			block_close_ok("/learning", Some(&format!("deleted #{}", index)));
			println!();
		}
		"clear" => {
			let deleted = data.get("deleted").and_then(|v| v.as_u64()).unwrap_or(0);
			let errors = data
				.get("errors")
				.and_then(|v| v.as_array())
				.map(|a| a.len())
				.unwrap_or(0);
			block_open("/learning", None);
			if deleted == 0 {
				block_line(&"No lessons to clear.".yellow().to_string());
				block_close_ok("/learning", Some("empty"));
			} else {
				let kw = key_width(["cleared", "warnings"]);
				block_row(
					"cleared",
					&format!("{} lesson(s)", deleted).bright_green().to_string(),
					kw,
				);
				if errors > 0 {
					block_row(
						"warnings",
						&format!("{} file(s) could not be removed", errors)
							.yellow()
							.to_string(),
						kw,
					);
				}
				block_close_ok("/learning", Some(&format!("cleared {}", deleted)));
			}
			println!();
		}
		"error" => {
			let msg = data
				.get("message")
				.and_then(|v| v.as_str())
				.unwrap_or("unknown error");
			block_open("/learning", None);
			block_close_err("/learning", msg);
			println!();
		}
		_ => {}
	}
}
