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

//! /skill command — list skills or toggle by name.
//!
//! `/skill`        → list all skills with active status
//! `/skill <name>` → toggle: enable if inactive, disable if active

use super::super::core::ChatSession;
use super::{CommandOutput, CommandResult};
use anyhow::Result;
use serde_json::json;

pub async fn handle_skill(session: &mut ChatSession, params: &[&str]) -> Result<CommandResult> {
	if params.is_empty() {
		return handle_list();
	}

	let name = params[0];

	// Check active status and toggle
	let session_id = crate::session::context::current_session_id();
	let is_active = session_id
		.as_ref()
		.map(|sid| crate::session::context::get_active_skills(sid).contains(&name.to_string()))
		.unwrap_or(false);

	if is_active {
		handle_forget(name).await
	} else {
		handle_use(session, name).await
	}
}

fn handle_list() -> Result<CommandResult> {
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

	let data = json!({
		"subcommand": "list",
		"skills": skills_data,
		"total": all_skills.len(),
		"active_count": active_count,
	});
	Ok(CommandResult::HandledWithOutput(Box::new(
		CommandOutput::Skill { data },
	)))
}

async fn handle_use(session: &mut ChatSession, name: &str) -> Result<CommandResult> {
	let call = crate::mcp::McpToolCall {
		tool_name: "skill".to_string(),
		tool_id: format!("cmd_skill_{}", name),
		parameters: json!({"action": "use_silent", "name": name}),
	};

	match crate::mcp::core::skill::execute_skill_tool(&call).await {
		Ok(_) => {
			if let Some(content) = crate::mcp::core::skill::take_silent_skill_content() {
				let _ = session.add_user_message(&content);
			}
		}
		Err(e) => {
			let data = json!({"subcommand": "error", "message": format!("Error: {}", e)});
			return Ok(CommandResult::HandledWithOutput(Box::new(
				CommandOutput::Skill { data },
			)));
		}
	}

	let data = json!({"subcommand": "use", "name": name});
	Ok(CommandResult::HandledWithOutput(Box::new(
		CommandOutput::Skill { data },
	)))
}

async fn handle_forget(name: &str) -> Result<CommandResult> {
	let call = crate::mcp::McpToolCall {
		tool_name: "skill".to_string(),
		tool_id: format!("cmd_skill_{}", name),
		parameters: json!({"action": "forget", "name": name}),
	};

	if let Err(e) = crate::mcp::core::skill::execute_skill_tool(&call).await {
		let data = json!({"subcommand": "error", "message": format!("Error: {}", e)});
		return Ok(CommandResult::HandledWithOutput(Box::new(
			CommandOutput::Skill { data },
		)));
	}

	let data = json!({"subcommand": "forget", "name": name});
	Ok(CommandResult::HandledWithOutput(Box::new(
		CommandOutput::Skill { data },
	)))
}
