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

//! Skill tool — tap-integrated skill discovery and context injection.
//!
//! Skills are reusable instruction packs stored in taps under
//! `<tap>/skills/<skill-name>/SKILL.md`, following the AgentSkills specification
//! (https://agentskills.io/specification).
//!
//! Actions:
//! - `list`   — discover available skills across all taps (filterable, paginated)
//! - `use`    — inject a skill's content into the current session context
//! - `forget` — remove a skill from context (triggers forced compression)

use crate::mcp::{McpFunction, McpToolCall, McpToolResult};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::path::PathBuf;

// Thread-local to pass skill content from execute_use(silent=true) to caller.
thread_local! {
	static LAST_SKILL_CONTENT: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Take the last silently-activated skill's content (if any).
/// Returns None if no silent activation happened or content was already taken.
pub fn take_silent_skill_content() -> Option<String> {
	LAST_SKILL_CONTENT.with(|cell| cell.borrow_mut().take())
}

// ---------------------------------------------------------------------------
// Skill metadata (parsed from SKILL.md frontmatter)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct SkillMeta {
	pub name: String,
	pub description: String,
	pub compatibility: Option<String>,
	pub license: Option<String>,
	pub allowed_tools: Vec<String>,
	pub capabilities: Vec<String>,
	pub domains: Vec<String>,
	/// Declarative activation rules. Empty = manual only.
	pub rules: Vec<Vec<ActivateCheck>>,
}
/// Individual activation check within a group.
#[derive(Debug, Clone)]
pub enum ActivateCheck {
	/// file(Cargo.toml) — file or glob exists in workdir
	File(String),
	/// content(rust) — word-boundary match against user message content
	Content(String),
	/// grep(pattern) or grep(pattern, path_glob) — search inside files
	Grep {
		pattern: String,
		path: Option<String>,
	},
	/// env(VAR) — environment variable is set and non-empty
	/// env(VAR=val) — environment variable equals value
	Env { var: String, value: Option<String> },
	/// match(regex) — regex match against user message content
	Match(String),
}

impl ActivateCheck {
	/// Parse a check from `type(args)` syntax.
	fn parse(s: &str) -> Option<Self> {
		let s = s.trim();
		let open = s.find('(')?;
		let close = s.rfind(')')?;
		if close <= open {
			return None;
		}
		let check_type = &s[..open];
		let args = &s[open + 1..close];

		match check_type {
			"file" => Some(Self::File(args.trim().to_string())),
			"content" => Some(Self::Content(args.trim().to_string())),
			"grep" => {
				if let Some((pattern, path)) = args.split_once(',') {
					Some(Self::Grep {
						pattern: pattern.trim().to_string(),
						path: Some(path.trim().to_string()),
					})
				} else {
					Some(Self::Grep {
						pattern: args.trim().to_string(),
						path: None,
					})
				}
			}
			"env" => {
				if let Some((var, val)) = args.trim().split_once('=') {
					Some(Self::Env {
						var: var.trim().to_string(),
						value: Some(val.trim().to_string()),
					})
				} else {
					Some(Self::Env {
						var: args.trim().to_string(),
						value: None,
					})
				}
			}
			"match" => Some(Self::Match(args.trim().to_string())),
			_ => None,
		}
	}

	/// Evaluate this check against current context.
	pub fn matches(&self, content: &str, workdir: &std::path::Path) -> bool {
		match self {
			Self::File(pattern) => {
				let path = workdir.join(pattern);
				if path.exists() {
					return true;
				}
				// Try as glob
				glob::glob(&workdir.join(pattern).to_string_lossy())
					.map(|mut iter| iter.next().is_some())
					.unwrap_or(false)
			}
			Self::Content(pattern) => match_word_pattern(pattern, content),
			Self::Grep { pattern, path } => grep_workdir(pattern, path.as_deref(), workdir),
			Self::Env { var, value } => match value {
				Some(expected) => std::env::var(var).is_ok_and(|v| v == *expected),
				None => std::env::var(var).is_ok_and(|v| !v.is_empty()),
			},
			Self::Match(pattern) => regex::Regex::new(pattern)
				.map(|re| re.is_match(content))
				.unwrap_or(false),
		}
	}
}

/// Word-boundary match: case-insensitive regex with \b boundaries, fallback to contains.
fn match_word_pattern(pattern: &str, text: &str) -> bool {
	let re_pattern = format!(r"(?i)\b{}\b", regex::escape(pattern));
	regex::Regex::new(&re_pattern)
		.map(|re| re.is_match(text))
		.unwrap_or_else(|_| text.to_lowercase().contains(&pattern.to_lowercase()))
}

/// Search file contents in workdir for pattern, respecting .gitignore.
fn grep_workdir(pattern: &str, path_filter: Option<&str>, workdir: &std::path::Path) -> bool {
	let walker = ignore::WalkBuilder::new(workdir)
		.hidden(true)
		.git_ignore(true)
		.build();

	let re = regex::Regex::new(pattern).ok();

	for entry in walker.flatten() {
		if !entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
			continue;
		}
		// Apply path filter if specified
		if let Some(filter) = path_filter {
			let fname = entry.file_name().to_string_lossy();
			if !glob::Pattern::new(filter)
				.map(|p| p.matches(&fname))
				.unwrap_or(false)
			{
				continue;
			}
		}
		// Read and search
		if let Ok(contents) = std::fs::read_to_string(entry.path()) {
			let found = if let Some(ref re) = re {
				re.is_match(&contents)
			} else {
				contents.to_lowercase().contains(&pattern.to_lowercase())
			};
			if found {
				return true;
			}
		}
	}
	false
}

/// Parse the `rule:` line into a list of checks.
/// Format: `file(Cargo.toml) content(rust) grep(fn main, *.rs)`
fn parse_rule_line(line: &str) -> Vec<ActivateCheck> {
	let mut checks = Vec::new();
	let mut rest = line.trim();
	while !rest.is_empty() {
		if rest.find('(').is_some() {
			if let Some(close) = rest.find(')') {
				let check_str = &rest[..=close];
				if let Some(check) = ActivateCheck::parse(check_str) {
					checks.push(check);
				}
				rest = rest[close + 1..].trim_start();
				continue;
			}
		}
		break;
	}
	checks
}

/// Parse a value that may be space-delimited (`git memory`) or YAML-array-like
/// (`["git", "memory"]`). Returns the list of items.
fn parse_space_or_array(value: &str) -> Vec<String> {
	let trimmed = value.trim();
	if trimmed.starts_with('[') && trimmed.ends_with(']') {
		// YAML array syntax: ["git", "memory"] or [git, memory]
		trimmed[1..trimmed.len() - 1]
			.split(',')
			.map(|s| s.trim().trim_matches('"').trim_matches('\'').to_string())
			.filter(|s| !s.is_empty())
			.collect()
	} else {
		// Space-delimited: git memory
		trimmed.split_whitespace().map(|s| s.to_string()).collect()
	}
}

/// Check if a skill directory contains an `activate` script.
pub(crate) fn has_activate_script(skill_dir: &std::path::Path) -> bool {
	skill_dir.join("activate").exists()
}

/// Check if a skill directory contains a `validate` script.
pub(crate) fn has_validate_script(skill_dir: &std::path::Path) -> bool {
	skill_dir.join("validate").exists()
}

/// Check if a session message contains a skill injection by tag.
pub fn is_skill_message(content: &str) -> bool {
	content.trim_start().starts_with("<skill name=\"")
}

/// Extract skill name from a skill-tagged message.
pub fn extract_skill_name(content: &str) -> Option<&str> {
	let trimmed = content.trim_start();
	let after = trimmed.strip_prefix("<skill name=\"")?;
	let end = after.find('"')?;
	Some(&after[..end])
}

/// Strip frontmatter from SKILL.md content, returning only the body.
pub(crate) fn strip_frontmatter(content: &str) -> &str {
	let trimmed = content.trim_start();
	if !trimmed.starts_with("---") {
		return content;
	}
	let after_open = match trimmed.strip_prefix("---") {
		Some(s) => s.trim_start_matches('\n'),
		None => return content,
	};
	match after_open.find("\n---") {
		Some(end) => {
			let after_close = &after_open[end + 4..];
			after_close.trim_start_matches('\n')
		}
		None => content,
	}
}

/// Parse YAML frontmatter from a SKILL.md file.
/// Frontmatter is delimited by `---` lines at the start of the file.
/// Returns None if the file has no valid frontmatter or missing required fields.
pub(crate) fn parse_skill_meta(content: &str) -> Option<SkillMeta> {
	let content = content.trim_start();
	if !content.starts_with("---") {
		return None;
	}

	// Find the closing ---
	let after_open = content.strip_prefix("---")?.trim_start_matches('\n');
	let end = after_open.find("\n---")?;
	let frontmatter = &after_open[..end];

	let mut name = None;
	let mut description = None;
	let mut compatibility = None;
	let mut license = None;
	let mut allowed_tools = Vec::new();
	let mut capabilities = Vec::new();
	let mut domains = Vec::new();
	let mut rules = Vec::new();

	let lines: Vec<&str> = frontmatter.lines().collect();
	let mut i = 0;
	while i < lines.len() {
		let line = lines[i];

		if line.trim() == "rules:" {
			i += 1;
			// Parse indented lines: each `- ` line is one AND-group
			while i < lines.len() {
				let entry_line = lines[i];
				// Stop if not indented (back to top-level)
				if !entry_line.starts_with(' ') && !entry_line.starts_with('\t') {
					break;
				}
				let trimmed = entry_line.trim();
				if let Some(rest) = trimmed.strip_prefix("- ") {
					let checks = parse_rule_line(rest);
					if !checks.is_empty() {
						rules.push(checks);
					}
				}
				i += 1;
			}
			continue;
		}

		// Regular key: value line
		if let Some((key, value)) = line.split_once(':') {
			let key = key.trim();
			let value = value
				.trim()
				.trim_matches('"')
				.trim_matches('\'')
				.to_string();
			match key {
				"name" => name = Some(value),
				"description" => description = Some(value),
				"compatibility" => compatibility = Some(value),
				"license" => license = Some(value),
				"allowed-tools" => {
					allowed_tools = value.split_whitespace().map(|s| s.to_string()).collect();
				}
				"capabilities" => {
					capabilities = parse_space_or_array(&value);
				}
				"domains" => {
					domains = parse_space_or_array(&value);
				}
				_ => {}
			}
		}
		i += 1;
	}

	Some(SkillMeta {
		name: name?,
		description: description?,
		compatibility,
		license,
		allowed_tools,
		capabilities,
		domains,
		rules,
	})
}

// ---------------------------------------------------------------------------
// Resource catalog — Tier 3 (scripts/, references/, assets/)
// ---------------------------------------------------------------------------

/// Scan a skill directory for Tier 3 resources and build a catalog string.
///
/// All resources (scripts/, references/, assets/) are listed with their
/// absolute paths. The AI uses `shell` / `view` to access them on demand.
///
/// Returns an empty string when no resources are found.
pub(crate) fn build_resource_catalog(skill_dir: &std::path::Path) -> String {
	let subdirs = ["scripts", "references", "assets"];

	let mut sections: Vec<String> = Vec::new();

	for subdir_name in &subdirs {
		let subdir = skill_dir.join(subdir_name);
		if !subdir.is_dir() {
			continue;
		}

		let mut entries: Vec<_> = match std::fs::read_dir(&subdir) {
			Ok(e) => e.flatten().collect(),
			Err(_) => continue,
		};
		// Sort for deterministic output
		entries.sort_by_key(|e| e.file_name());

		if entries.is_empty() {
			continue;
		}

		let mut section_lines = vec![format!("**{}/**", subdir_name)];

		for entry in &entries {
			let path = entry.path();
			if !path.is_file() {
				continue;
			}
			let fname = path
				.file_name()
				.map(|n| n.to_string_lossy().to_string())
				.unwrap_or_default();
			section_lines.push(format!("- `{}` — {}", fname, path.display()));
		}

		if section_lines.len() > 1 {
			sections.push(section_lines.join("\n"));
		}
	}

	if sections.is_empty() {
		return String::new();
	}

	format!("\n\n## Skill Resources\n\n{}", sections.join("\n\n"))
}

// ---------------------------------------------------------------------------
// Tool compatibility check
// ---------------------------------------------------------------------------

/// Returns the subset of `allowed_tools` that are NOT available in the current
/// session's tool map. An empty result means the skill is fully compatible.
fn missing_tools(allowed_tools: &[String]) -> Vec<String> {
	allowed_tools
		.iter()
		.filter(|t| crate::mcp::tool_map::get_server_for_tool(t).is_none())
		.cloned()
		.collect()
}

// ---------------------------------------------------------------------------
// Skill discovery across all taps
// ---------------------------------------------------------------------------

/// Scan all active taps for skills. Returns (meta, skill_dir) pairs.
/// Skills live at `<tap>/skills/<skill-name>/SKILL.md`.
/// Public alias for use by `/skill` command.
pub fn find_all_skills_with_details() -> Vec<(SkillMeta, PathBuf)> {
	find_all_skills()
}

fn find_all_skills() -> Vec<(SkillMeta, PathBuf)> {
	let taps = match crate::agent::taps::get_taps() {
		Ok(t) => t,
		Err(e) => {
			crate::log_debug!("skill: failed to load taps: {}", e);
			return Vec::new();
		}
	};

	let mut skills = Vec::new();
	let mut seen_names = std::collections::HashSet::new();

	for tap in &taps {
		let skills_dir = match tap.skills_dir() {
			Ok(d) => d,
			Err(_) => continue,
		};

		if !skills_dir.exists() {
			continue;
		}

		let entries = match std::fs::read_dir(&skills_dir) {
			Ok(e) => e,
			Err(_) => continue,
		};

		for entry in entries.flatten() {
			let skill_dir = entry.path();
			if !skill_dir.is_dir() {
				continue;
			}

			let skill_md = skill_dir.join("SKILL.md");
			if !skill_md.exists() {
				continue;
			}

			let content = match std::fs::read_to_string(&skill_md) {
				Ok(c) => c,
				Err(_) => continue,
			};

			if let Some(meta) = parse_skill_meta(&content) {
				if seen_names.insert(meta.name.clone()) {
					skills.push((meta, skill_dir));
				}
			}
		}
	}

	skills
}

/// Find a specific skill by name across all taps.
/// Returns (meta, skill_dir, full_content) — reads SKILL.md only once.
/// Public alias for `/skill` command.
pub fn find_skill_by_name_pub(name: &str) -> Option<(SkillMeta, PathBuf, String)> {
	find_skill_by_name(name)
}

fn find_skill_by_name(name: &str) -> Option<(SkillMeta, PathBuf, String)> {
	let taps = match crate::agent::taps::get_taps() {
		Ok(t) => t,
		Err(e) => {
			crate::log_debug!("skill: failed to get taps: {}", e);
			return None;
		}
	};

	for tap in &taps {
		let skills_dir = match tap.skills_dir() {
			Ok(d) => d,
			Err(_) => continue,
		};

		let skill_dir = skills_dir.join(name);
		if !skill_dir.is_dir() {
			continue;
		}

		let skill_md = skill_dir.join("SKILL.md");
		let content = match std::fs::read_to_string(&skill_md) {
			Ok(c) => c,
			Err(_) => continue,
		};

		if let Some(meta) = parse_skill_meta(&content) {
			if meta.name == name {
				return Some((meta, skill_dir, content));
			}
		}
	}

	None
}

// ---------------------------------------------------------------------------
// MCP tool definition
// ---------------------------------------------------------------------------

pub fn get_skill_function() -> McpFunction {
	McpFunction {
		name: "skill".to_string(),
		description: r#"Manage skills from taps. Skills are reusable instruction packs that inject domain knowledge into context.

**Actions:**
- `list`   — discover available skills across all taps. Supports optional `pattern` (substring filter on name/description), `offset`, and `limit` (default 20).
- `use`    — inject a skill's full content into the current session context. The skill instructions become immediately active.
- `forget` — remove a skill from context. Triggers conversation compression to clean up the injected content.

**Workflow:**
1. `skill(action="list")` to explore what's available
2. `skill(action="use", name="skill-name")` to activate a skill
3. `skill(action="forget", name="skill-name")` when the skill is no longer needed"#.to_string(),
		parameters: json!({
			"type": "object",
			"properties": {
				"action": {
					"type": "string",
					"enum": ["list", "use", "forget"],
					"description": "Action to perform: list (discover skills), use (inject skill into context), forget (remove skill from context)"
				},
				"name": {
					"type": "string",
					"description": "Skill name (required for use and forget actions)"
				},
				"pattern": {
					"type": "string",
					"description": "Optional substring filter applied to skill name and description (for list action)"
				},
				"offset": {
					"type": "integer",
					"description": "Pagination offset for list action (default: 0)"
				},
				"limit": {
					"type": "integer",
					"description": "Maximum number of skills to return for list action (default: 20)"
				}
			},
			"required": ["action"]
		}),
	}
}

// ---------------------------------------------------------------------------
// Tool handler
// ---------------------------------------------------------------------------

pub async fn execute_skill_tool(call: &McpToolCall) -> Result<McpToolResult, String> {
	let action = match call.parameters.get("action") {
		Some(Value::String(a)) if !a.trim().is_empty() => a.clone(),
		Some(_) => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"action must be a string".to_string(),
			))
		}
		None => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"missing required parameter: action".to_string(),
			))
		}
	};

	match action.as_str() {
		"list" => execute_list(call),
		"use" => execute_use(call, false).await,
		"use_silent" => execute_use(call, true).await,
		"forget" => execute_forget(call),
		other => Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!(
				"unknown action '{}'. Valid actions: list, use, forget",
				other
			),
		)),
	}
}

fn execute_list(call: &McpToolCall) -> Result<McpToolResult, String> {
	let pattern = match call.parameters.get("pattern") {
		Some(Value::String(p)) => Some(p.to_lowercase()),
		_ => None,
	};

	let offset = match call.parameters.get("offset") {
		Some(Value::Number(n)) => n.as_u64().unwrap_or(0) as usize,
		_ => 0,
	};

	let limit = match call.parameters.get("limit") {
		Some(Value::Number(n)) => n.as_u64().unwrap_or(20) as usize,
		_ => 20,
	};

	let all_skills = find_all_skills();

	// Filter by pattern (case-insensitive substring on name + description)
	let filtered: Vec<_> = all_skills
		.iter()
		.filter(|(meta, _)| {
			if let Some(ref pat) = pattern {
				meta.name.to_lowercase().contains(pat.as_str())
					|| meta.description.to_lowercase().contains(pat.as_str())
			} else {
				true
			}
		})
		.collect();

	let total = filtered.len();

	if total == 0 {
		let msg = if pattern.is_some() {
			"No skills found matching the pattern.".to_string()
		} else {
			"No skills found. Add skills to your tap under skills/<name>/SKILL.md".to_string()
		};
		return Ok(McpToolResult::success(
			call.tool_name.clone(),
			call.tool_id.clone(),
			msg,
		));
	}

	// Paginate
	let page: Vec<_> = filtered.iter().skip(offset).take(limit).collect();
	let page_len = page.len();

	// Get active skills for current session to mark them
	let active_skills = crate::session::context::current_session_id()
		.map(|sid| crate::session::context::get_active_skills(&sid))
		.unwrap_or_default();

	// Format as table
	let mut lines = vec![format!(
		"Found {} skill(s){}:",
		total,
		if pattern.is_some() {
			" matching pattern"
		} else {
			""
		}
	)];
	lines.push(String::new());

	for (meta, _) in page {
		let active_marker = if active_skills.contains(&meta.name) {
			" ✓ [active]"
		} else {
			""
		};
		// Check tool compatibility for this skill
		let unavailable = missing_tools(&meta.allowed_tools);
		let compat_marker = if unavailable.is_empty() {
			String::new()
		} else {
			format!(" ⚠️ [missing tools: {}]", unavailable.join(", "))
		};
		lines.push(format!(
			"**{}**{}{}",
			meta.name, active_marker, compat_marker
		));
		lines.push(format!("  {}", meta.description));
		if let Some(ref compat) = meta.compatibility {
			lines.push(format!("  Compatibility: {}", compat));
		}
		lines.push(String::new());
	}

	if offset + limit < total {
		lines.push(format!(
			"Showing {}-{} of {}. Use offset={} to see more.",
			offset + 1,
			offset + page_len,
			total,
			offset + limit
		));
	}

	Ok(McpToolResult::success(
		call.tool_name.clone(),
		call.tool_id.clone(),
		lines.join("\n"),
	))
}

async fn execute_use(call: &McpToolCall, silent: bool) -> Result<McpToolResult, String> {
	let name = match call.parameters.get("name") {
		Some(Value::String(n)) if !n.trim().is_empty() => n.clone(),
		Some(_) => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"name must be a non-empty string".to_string(),
			))
		}
		None => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"missing required parameter: name".to_string(),
			))
		}
	};

	let session_id = match crate::session::context::current_session_id() {
		Some(id) => id,
		None => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"skill use requires an active session".to_string(),
			))
		}
	};

	// Already active — inform but don't re-inject
	if crate::session::context::has_active_skill(&session_id, &name) {
		return Ok(McpToolResult::success(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!(
				"Skill '{}' is already active in this session. Use forget to remove it first.",
				name
			),
		));
	}

	// Find the skill across taps (reads SKILL.md once)
	let (meta, skill_dir, content) = match find_skill_by_name(&name) {
		Some(s) => s,
		None => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				format!(
					"Skill '{}' not found. Use skill(action=\"list\") to see available skills.",
					name
				),
			))
		}
	};

	// Tier 3: append resource catalog (scripts/, references/, assets/)
	let resources = build_resource_catalog(&skill_dir);

	// Auto-load required capabilities — resolves to MCP servers and enables them.
	// Uses refcounting so shared capabilities are only offloaded when the last skill forgets.
	let mut cap_messages = Vec::new();
	let mut servers_loaded_by_this_skill = Vec::new();
	if !meta.capabilities.is_empty() {
		let session_config = crate::session::context::current_session_id()
			.and_then(|sid| crate::session::context::get_session_config(&sid));
		let overrides = session_config
			.as_ref()
			.map(|cfg| cfg.capabilities.clone())
			.unwrap_or_default();

		// Collect config-level server names to avoid touching domain-owned servers
		let config_server_names: std::collections::HashSet<String> = session_config
			.as_ref()
			.map(|cfg| {
				cfg.mcp
					.servers
					.iter()
					.map(|s| s.name().to_string())
					.collect()
			})
			.unwrap_or_default();

		for cap_name in &meta.capabilities {
			match crate::agent::registry::parse_capability_toml(cap_name, &overrides) {
				Ok(resolved) => {
					let mut loaded_servers = Vec::new();
					for server_config in resolved.mcp_servers {
						let server_name = server_config.name().to_string();

						// Domain-level server (loaded at session init) — skip, not our responsibility
						if config_server_names.contains(&server_name) {
							crate::log_debug!(
								"skill: capability '{}' server '{}' already config-level, skipping",
								cap_name,
								server_name
							);
							loaded_servers.push(server_name);
							continue;
						}

						// Already in dynamic registry and enabled — just bump refcount
						if let Some((_cfg, true)) = crate::session::context::current_session_id()
							.and_then(|sid| {
								crate::session::context::get_dynamic_server_for_session(
									&sid,
									&server_name,
								)
							}) {
							crate::session::context::increment_capability_refcount(
								&session_id,
								&server_name,
							);
							servers_loaded_by_this_skill.push(server_name.clone());
							loaded_servers.push(server_name);
							continue;
						}

						// New server — register + enable + refcount=1
						if let Err(e) =
							crate::mcp::core::dynamic::register_server(server_config.clone())
						{
							crate::log_debug!(
								"skill: capability '{}' server '{}' register: {}",
								cap_name,
								server_name,
								e
							);
						}
						match crate::mcp::core::dynamic::enable_server(&server_name, None).await {
							Ok(_) => {
								crate::session::context::increment_capability_refcount(
									&session_id,
									&server_name,
								);
								servers_loaded_by_this_skill.push(server_name.clone());
								loaded_servers.push(server_name);
							}
							Err(e) => {
								crate::log_debug!(
									"skill: capability '{}' server '{}' enable failed: {}",
									cap_name,
									server_name,
									e
								);
							}
						}
					}
					if !loaded_servers.is_empty() {
						cap_messages.push(format!(
							"Loaded capability '{}' (servers: {})",
							cap_name,
							loaded_servers.join(", ")
						));
					}
				}
				Err(e) => {
					crate::log_debug!("skill: capability '{}' resolution failed: {}", cap_name, e);
					cap_messages.push(format!("⚠️ Capability '{}' not found: {}", cap_name, e));
				}
			}
		}
	}

	// Record which servers this skill loaded for proper offloading on forget
	crate::session::context::set_skill_capability_servers(
		&session_id,
		&name,
		servers_loaded_by_this_skill,
	);

	// Tool compatibility: warn if required tools are still missing after capability loading
	let unavailable = missing_tools(&meta.allowed_tools);
	let tool_warning = if unavailable.is_empty() {
		String::new()
	} else {
		format!(
			"\n\n⚠️ Some tools still unavailable after capability loading: {}",
			unavailable.join(", ")
		)
	};

	// Register as active
	crate::session::context::add_active_skill(&session_id, &name);

	// Inject skill body wrapped in tags for detection on session resume.
	let body = strip_frontmatter(&content);
	let description = meta.description.replace('"', "&quot;");
	let mut injection_content = format!(
		"<skill name=\"{}\" description=\"{}\">\n{}",
		name, description, body
	);
	if !resources.is_empty() {
		injection_content.push_str(&resources);
	}
	injection_content.push_str("\n</skill>");

	if silent {
		// Silent mode (env loading, /skill command): store content for the caller to inject
		// into session messages directly. We stash it in a thread-local so the caller can
		// retrieve it without re-reading the file.
		LAST_SKILL_CONTENT.with(|cell| {
			*cell.borrow_mut() = Some(injection_content);
		});
		crate::log_debug!(
			"skill: silently activated '{}' in session {}",
			name,
			session_id
		);
	} else {
		// Normal mode (AI-initiated): push to inbox for the main loop to process.
		crate::session::inbox::push_inbox_message(crate::session::inbox::InboxMessage {
			source: crate::session::inbox::InboxSource::Skill { name: name.clone() },
			content: injection_content,
		});
		crate::log_debug!(
			"skill: queued '{}' for injection in session {}",
			name,
			session_id
		);
	}

	// Return short confirmation — the actual content is injected as a system message
	let mut msg = format!("Skill '{}' is now active.", name);
	for cap_msg in &cap_messages {
		msg.push_str(&format!("\n{}", cap_msg));
	}
	if !tool_warning.is_empty() {
		msg.push_str(&tool_warning);
	}

	Ok(McpToolResult::success(
		call.tool_name.clone(),
		call.tool_id.clone(),
		msg,
	))
}

fn execute_forget(call: &McpToolCall) -> Result<McpToolResult, String> {
	let name = match call.parameters.get("name") {
		Some(Value::String(n)) if !n.trim().is_empty() => n.clone(),
		Some(_) => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"name must be a non-empty string".to_string(),
			))
		}
		None => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"missing required parameter: name".to_string(),
			))
		}
	};

	let session_id = match crate::session::context::current_session_id() {
		Some(id) => id,
		None => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"skill forget requires an active session".to_string(),
			))
		}
	};

	if !crate::session::context::has_active_skill(&session_id, &name) {
		return Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!(
				"Skill '{}' is not currently active. Use skill(action=\"list\") to see active skills.",
				name
			),
		));
	}

	crate::session::context::remove_active_skill(&session_id, &name);

	// Offload capability servers this skill loaded (refcount-aware)
	let servers = crate::session::context::take_skill_capability_servers(&session_id, &name);
	let mut offloaded = Vec::new();
	for server_name in &servers {
		let remaining =
			crate::session::context::decrement_capability_refcount(&session_id, server_name);
		if remaining == 0 {
			if let Err(e) = crate::mcp::core::dynamic::disable_server(server_name) {
				crate::log_debug!("skill: offload disable '{}': {}", server_name, e);
			}
			crate::mcp::core::dynamic::remove_server(server_name);
			offloaded.push(server_name.clone());
		}
	}
	if !offloaded.is_empty() {
		crate::log_debug!(
			"skill: forgot '{}' — offloaded servers: {}",
			name,
			offloaded.join(", ")
		);
	}

	// Signal the session to run forced compression so the injected skill content is cleaned up
	crate::session::context::request_skill_compression(&session_id);

	crate::log_debug!("skill: forgot '{}' from session {}", name, session_id);

	let msg = if offloaded.is_empty() {
		format!(
			"Skill '{}' removed from context. Conversation will be compressed to clean up injected content.",
			name
		)
	} else {
		format!(
			"Skill '{}' removed from context (offloaded servers: {}). Conversation will be compressed.",
			name,
			offloaded.join(", ")
		)
	};

	Ok(McpToolResult::success(
		call.tool_name.clone(),
		call.tool_id.clone(),
		msg,
	))
}
