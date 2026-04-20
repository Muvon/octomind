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

//! /skill command — list/filter skills or toggle by name.
//!
//! `/skill`            → list all skills (active first, then alphabetical), page 1
//! `/skill 2`          → list page 2
//! `/skill *pattern*`  → filter skills by glob pattern
//! `/skill <name>`     → toggle: enable if inactive, disable if active

use super::super::core::ChatSession;
use super::{CommandOutput, CommandResult};
use anyhow::Result;
use serde_json::json;

const SKILLS_PER_PAGE: usize = 15;

pub async fn handle_skill(session: &mut ChatSession, params: &[&str]) -> Result<CommandResult> {
	if params.is_empty() {
		return handle_list(None, 1);
	}

	let arg = params[0];

	// Numeric argument → page number
	if let Ok(page) = arg.parse::<usize>() {
		return if page == 0 {
			Ok(CommandResult::HandledWithOutput(Box::new(
				CommandOutput::Error {
					error: "Page number must be a positive integer".to_string(),
					context: None,
				},
			)))
		} else {
			handle_list(None, page)
		};
	}

	// Contains `*` → glob pattern filter
	if arg.contains('*') {
		return handle_list(Some(arg), 1);
	}

	// Otherwise → toggle skill by exact name
	let all_skills = crate::mcp::core::skill::find_all_skills_with_details();
	if !all_skills.iter().any(|(m, _)| m.name == arg) {
		let data = json!({"subcommand": "error", "message": format!("Skill '{}' not found.", arg)});
		return Ok(CommandResult::HandledWithOutput(Box::new(
			CommandOutput::Skill { data },
		)));
	}

	let session_id = crate::session::context::current_session_id();
	let is_active = session_id
		.as_ref()
		.map(|sid| crate::session::context::get_active_skills(sid).contains(&arg.to_string()))
		.unwrap_or(false);

	if is_active {
		handle_forget(arg).await
	} else {
		handle_use(session, arg).await
	}
}

fn handle_list(pattern: Option<&str>, page: usize) -> Result<CommandResult> {
	let all_skills = crate::mcp::core::skill::find_all_skills_with_details();
	let active_skills = crate::session::context::current_session_id()
		.map(|sid| crate::session::context::get_active_skills(&sid))
		.unwrap_or_default();

	// Build skill entries with active flag
	let mut entries: Vec<(serde_json::Value, bool)> = Vec::new();
	for (meta, skill_dir) in &all_skills {
		// Apply glob filter if present
		if let Some(pat) = pattern {
			if !glob_match(pat, &meta.name) && !glob_match(pat, &meta.description) {
				continue;
			}
		}

		let is_active = active_skills.contains(&meta.name);
		let has_activate = crate::mcp::core::skill::has_activate_script(skill_dir);
		let has_validate = crate::mcp::core::skill::has_validate_script(skill_dir);
		let mut scripts = Vec::new();
		if has_activate {
			scripts.push("activate");
		}
		if has_validate {
			scripts.push("validate");
		}

		entries.push((
			json!({
				"name": meta.name,
				"description": meta.description,
				"active": is_active,
				"capabilities": meta.capabilities,
				"domains": meta.domains,
				"scripts": scripts,
			}),
			is_active,
		));
	}

	// Sort: active first, then alphabetical by name within each group
	entries.sort_by(|(a, a_active), (b, b_active)| {
		b_active.cmp(a_active).then_with(|| {
			let a_name = a.get("name").and_then(|v| v.as_str()).unwrap_or("");
			let b_name = b.get("name").and_then(|v| v.as_str()).unwrap_or("");
			a_name.cmp(b_name)
		})
	});

	let total = entries.len();
	let active_count = entries.iter().filter(|(_, active)| *active).count();
	let total_pages = if total == 0 {
		0
	} else {
		total.div_ceil(SKILLS_PER_PAGE)
	};

	if page > total_pages && total > 0 {
		return Ok(CommandResult::HandledWithOutput(Box::new(
			CommandOutput::Error {
				error: format!("Page {} not found. Total pages: {}", page, total_pages),
				context: Some(json!({"page": page, "total_pages": total_pages})),
			},
		)));
	}

	// Paginate
	let start = (page - 1) * SKILLS_PER_PAGE;
	let end = std::cmp::min(start + SKILLS_PER_PAGE, total);
	let page_skills: Vec<serde_json::Value> =
		entries[start..end].iter().map(|(v, _)| v.clone()).collect();

	let data = json!({
		"subcommand": "list",
		"skills": page_skills,
		"total": total,
		"active_count": active_count,
		"page": page,
		"total_pages": total_pages,
		"pattern": pattern,
	});
	Ok(CommandResult::HandledWithOutput(Box::new(
		CommandOutput::Skill { data },
	)))
}

/// Simple glob matching: `*` matches any sequence of characters.
/// Case-insensitive. Supports leading/trailing/middle wildcards.
fn glob_match(pattern: &str, text: &str) -> bool {
	let pattern = pattern.to_lowercase();
	let text = text.to_lowercase();

	let parts: Vec<&str> = pattern.split('*').collect();
	if parts.len() == 1 {
		// No wildcard — exact match
		return text == pattern;
	}

	let mut pos = 0;
	for (i, part) in parts.iter().enumerate() {
		if part.is_empty() {
			continue;
		}
		match text[pos..].find(part) {
			Some(idx) => {
				// First part must match at start if pattern doesn't start with *
				if i == 0 && idx != 0 {
					return false;
				}
				pos += idx + part.len();
			}
			None => return false,
		}
	}

	// Last part must match at end if pattern doesn't end with *
	if !pattern.ends_with('*') {
		if let Some(last) = parts.last() {
			if !last.is_empty() {
				return text.ends_with(last);
			}
		}
	}

	true
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

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_glob_match() {
		// Trailing wildcard
		assert!(glob_match("git*", "github"));
		assert!(glob_match("git*", "git"));
		assert!(!glob_match("git*", "fugit"));

		// Leading wildcard
		assert!(glob_match("*hub", "github"));
		assert!(!glob_match("*hub", "hubris"));

		// Both sides
		assert!(glob_match("*it*", "github"));
		assert!(glob_match("*it*", "git"));
		assert!(!glob_match("*xyz*", "github"));

		// Case insensitive
		assert!(glob_match("*GIT*", "github"));
		assert!(glob_match("Git*", "github"));

		// Exact (no wildcard)
		assert!(glob_match("git", "git"));
		assert!(!glob_match("git", "github"));
	}
}
