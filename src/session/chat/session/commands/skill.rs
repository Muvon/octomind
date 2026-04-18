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

// Skill command handler — /skill [list|active|use|forget]

use super::{CommandOutput, CommandResult};
use anyhow::Result;
use serde_json::json;

pub async fn handle_skill(params: &[&str]) -> Result<CommandResult> {
	let subcommand = if params.is_empty() { "list" } else { params[0] };

	match subcommand {
		"list" => handle_skill_list(params),
		"active" => handle_skill_active(),
		"use" | "enable" => handle_skill_use(params).await,
		"forget" | "disable" => handle_skill_forget(params).await,
		"help" => handle_skill_help(),
		_ => {
			// Treat as /skill use <name> shorthand
			handle_skill_use(&[subcommand]).await
		}
	}
}

fn build_skill_list(filter_active_only: bool, pattern: Option<&str>) -> serde_json::Value {
	let all_skills = crate::mcp::core::skill::find_all_skills_with_details();

	let active_skills = crate::session::context::current_session_id()
		.map(|sid| crate::session::context::get_active_skills(&sid))
		.unwrap_or_default();

	let mut skills_data = Vec::new();
	let mut active_count = 0;

	for (meta, skill_dir) in &all_skills {
		let is_active = active_skills.contains(&meta.name);
		if is_active {
			active_count += 1;
		}

		if filter_active_only && !is_active {
			continue;
		}

		if let Some(pat) = pattern {
			let pat_lower = pat.to_lowercase();
			if !meta.name.to_lowercase().contains(&pat_lower)
				&& !meta.description.to_lowercase().contains(&pat_lower)
			{
				continue;
			}
		}

		let has_activate = crate::mcp::core::skill::has_activate_script(skill_dir);
		let has_validate = crate::mcp::core::skill::has_validate_script(skill_dir);

		let mut scripts = Vec::new();
		if has_activate {
			scripts.push("activate");
		}
		if has_validate {
			scripts.push("validate");
		}

		skills_data.push(json!({
			"name": meta.name,
			"description": meta.description,
			"active": is_active,
			"capabilities": meta.capabilities,
			"domains": meta.domains,
			"scripts": scripts,
		}));
	}

	json!({
		"subcommand": if filter_active_only { "active" } else { "list" },
		"skills": skills_data,
		"total": all_skills.len(),
		"active_count": active_count,
	})
}

fn handle_skill_list(params: &[&str]) -> Result<CommandResult> {
	let pattern = if params.len() > 1 {
		Some(params[1])
	} else {
		None
	};

	let data = build_skill_list(false, pattern);
	Ok(CommandResult::HandledWithOutput(Box::new(
		CommandOutput::Skill { data },
	)))
}

fn handle_skill_active() -> Result<CommandResult> {
	let data = build_skill_list(true, None);
	Ok(CommandResult::HandledWithOutput(Box::new(
		CommandOutput::Skill { data },
	)))
}

async fn handle_skill_use(params: &[&str]) -> Result<CommandResult> {
	let name = if params.len() > 1 {
		params[1]
	} else if !params.is_empty() && params[0] != "use" && params[0] != "enable" {
		params[0]
	} else {
		let data = json!({
			"subcommand": "error",
			"message": "Usage: /skill use <name>",
		});
		return Ok(CommandResult::HandledWithOutput(Box::new(
			CommandOutput::Skill { data },
		)));
	};

	let call = crate::mcp::McpToolCall {
		tool_name: "skill".to_string(),
		tool_id: format!("cmd_skill_use_{}", name),
		parameters: json!({"action": "use", "name": name}),
	};

	let message = match crate::mcp::core::skill::execute_skill_tool(&call).await {
		Ok(result) => result.extract_content(),
		Err(e) => format!("Error: {}", e),
	};

	let data = json!({
		"subcommand": "use",
		"name": name,
		"message": message,
	});
	Ok(CommandResult::HandledWithOutput(Box::new(
		CommandOutput::Skill { data },
	)))
}

async fn handle_skill_forget(params: &[&str]) -> Result<CommandResult> {
	let name = if params.len() > 1 {
		params[1]
	} else {
		let data = json!({
			"subcommand": "error",
			"message": "Usage: /skill forget <name>",
		});
		return Ok(CommandResult::HandledWithOutput(Box::new(
			CommandOutput::Skill { data },
		)));
	};

	let call = crate::mcp::McpToolCall {
		tool_name: "skill".to_string(),
		tool_id: format!("cmd_skill_forget_{}", name),
		parameters: json!({"action": "forget", "name": name}),
	};

	let message = match crate::mcp::core::skill::execute_skill_tool(&call).await {
		Ok(result) => result.extract_content(),
		Err(e) => format!("Error: {}", e),
	};

	let data = json!({
		"subcommand": "forget",
		"name": name,
		"message": message,
	});
	Ok(CommandResult::HandledWithOutput(Box::new(
		CommandOutput::Skill { data },
	)))
}

fn handle_skill_help() -> Result<CommandResult> {
	let data = json!({
		"subcommand": "help",
		"message": "/skill [list|active|use|forget|help]\n  list [pattern]  — show all skills\n  active           — show active skills\n  use <name>       — activate a skill\n  forget <name>    — deactivate a skill",
	});
	Ok(CommandResult::HandledWithOutput(Box::new(
		CommandOutput::Skill { data },
	)))
}
