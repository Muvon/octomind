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

//! Skill auto-activation engine.
//!
//! Scans the tap skill pool for skills with `activate` scripts, filtered by
//! the current agent's domain. Runs scripts on conversation events (`user`,
//! `assistant`, `turn`) to determine which skills should be active.
//!
//! When a skill auto-activates, its required capabilities are auto-loaded
//! (MCP servers enabled) and its content is injected via the inbox.

use std::process::Stdio;
use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;
use tokio::io::AsyncWriteExt;

/// Cached skill pool entry — a skill with declarative activate rules.
#[derive(Debug, Clone)]
struct PoolEntry {
	name: String,
	activate: Vec<super::skill::ActivateEntry>,
}

/// Cached pool of auto-activatable skills, filtered by domain.
struct SkillPool {
	entries: Vec<PoolEntry>,
	_domain: String,
}

static SKILL_POOL: OnceLock<Arc<RwLock<Option<SkillPool>>>> = OnceLock::new();

fn get_pool() -> &'static Arc<RwLock<Option<SkillPool>>> {
	SKILL_POOL.get_or_init(|| Arc::new(RwLock::new(None)))
}

/// Load skills from OCTOMIND_SKILLS env var. Called at session start.
/// Format: comma-delimited skill names, e.g. "programming-rust,git-workflow"
/// On resume: removes stale skill messages and re-injects fresh content.
pub async fn load_env_skills(session: &mut crate::session::chat::session::ChatSession) {
	let env_val = match std::env::var("OCTOMIND_SKILLS") {
		Ok(v) if !v.trim().is_empty() => v,
		_ => return,
	};

	let skill_names: Vec<&str> = env_val
		.split(',')
		.map(|s| s.trim())
		.filter(|s| !s.is_empty())
		.collect();
	if skill_names.is_empty() {
		return;
	}

	// Collect skill IDs already in session (from previous run / resume)
	let existing: std::collections::HashSet<String> = session
		.session
		.messages
		.iter()
		.filter(|m| m.role == "user")
		.filter_map(|m| super::skill::extract_skill_name(&m.content).map(String::from))
		.collect();

	for name in &skill_names {
		if existing.contains(*name) {
			// Already injected from previous session — just register as active
			if let Some(sid) = crate::session::context::current_session_id() {
				crate::session::context::add_active_skill(&sid, name);
			}
			continue;
		}
		let call = crate::mcp::McpToolCall {
			tool_name: "skill".to_string(),
			tool_id: format!("env_{}", name),
			parameters: serde_json::json!({"action": "use_silent", "name": name}),
		};

		match super::skill::execute_skill_tool(&call).await {
			Ok(_) => {
				if let Some(content) = super::skill::take_silent_skill_content() {
					let _ = session.add_user_message(&content);
				}
			}
			Err(e) => {
				eprintln!("OCTOMIND_SKILLS: skill '{}' failed: {}", name, e);
			}
		}
	}
}

/// Initialize the skill pool for the given agent domain (e.g., "developer").
/// Scans all taps for skills with `activate` scripts whose `domains` field
/// includes the given domain.
pub fn init_pool(domain: &str) {
	let taps = match crate::agent::taps::get_taps() {
		Ok(t) => t,
		Err(e) => {
			crate::log_debug!("skill_auto: failed to load taps: {}", e);
			return;
		}
	};

	let mut entries = Vec::new();

	for tap in &taps {
		let skills_dir = match tap.skills_dir() {
			Ok(d) if d.exists() => d,
			_ => continue,
		};

		let dir_entries = match std::fs::read_dir(&skills_dir) {
			Ok(e) => e,
			Err(_) => continue,
		};

		for entry in dir_entries.flatten() {
			let skill_dir = entry.path();
			if !skill_dir.is_dir() {
				continue;
			}

			// Must have SKILL.md with metadata
			let skill_md = skill_dir.join("SKILL.md");
			let content = match std::fs::read_to_string(&skill_md) {
				Ok(c) => c,
				Err(_) => continue,
			};

			let meta = match super::skill::parse_skill_meta(&content) {
				Some(m) => m,
				None => continue,
			};

			// Must have activate rules
			if meta.activate.is_empty() {
				continue;
			}

			// Must have domains that include the current domain
			if meta.domains.is_empty() || !meta.domains.iter().any(|d| d == domain) {
				continue;
			}

			entries.push(PoolEntry {
				name: meta.name,
				activate: meta.activate,
			});
		}
	}

	crate::log_debug!(
		"skill_auto: initialized pool with {} skills for domain '{}'",
		entries.len(),
		domain
	);

	// Clear retry counters from any previous session
	{
		let mut retries = get_retry_tracker().write().unwrap();
		retries.clear();
	}

	let mut pool = get_pool().write().unwrap();
	*pool = Some(SkillPool {
		entries,
		_domain: domain.to_string(),
	});
}

/// Conversation event types for activate/validate scripts.
#[derive(Debug, Clone, Copy)]
pub enum Event {
	/// Real user input (typed, not auto-injected).
	User,
	/// Assistant finished responding, awaiting user.
	Assistant,
	/// Tool execution done, ready for next loop.
	Turn,
}

impl Event {
	fn as_str(&self) -> &'static str {
		match self {
			Event::User => "user",
			Event::Assistant => "assistant",
			Event::Turn => "turn",
		}
	}
}

/// Get the skills config from the current session config.
fn get_skills_config() -> crate::config::SkillsConfig {
	crate::session::context::current_session_id()
		.and_then(|sid| crate::session::context::get_session_config(&sid))
		.map(|cfg| cfg.skills.clone())
		.unwrap_or(crate::config::SkillsConfig {
			auto_activation: true,
			activation_timeout: 3,
			validation_timeout: 60,
			max_retries: 3,
		})
}

/// Run auto-activation for the given event and content.
///
/// Evaluates declarative rules from the skill pool in-process.
/// Any rule matching activates the skill. No process spawns.
pub async fn run_activation(
	event: Event,
	content: &str,
	workdir: &std::path::Path,
	session: &mut crate::session::chat::session::ChatSession,
) {
	let skills_config = get_skills_config();

	if !skills_config.auto_activation {
		return;
	}

	let session_id = match crate::session::context::current_session_id() {
		Some(id) => id,
		None => return,
	};

	let entries = {
		let pool = get_pool().read().unwrap();
		match pool.as_ref() {
			Some(p) => p.entries.clone(),
			None => return,
		}
	};

	if entries.is_empty() {
		return;
	}

	let active_skills = crate::session::context::get_active_skills(&session_id);
	let event_str = event.as_str();

	for entry in &entries {
		if active_skills.contains(&entry.name) {
			continue;
		}

		// Evaluate: any entry matching = activate
		let should_activate = entry.activate.iter().any(|act| {
			act.on.matches(event_str)
				&& act
					.rules
					.iter()
					.any(|check| check.matches(content, workdir))
		});

		if should_activate {
			crate::log_debug!("skill_auto: rule-activated '{}'", entry.name);
			auto_activate_skill(&entry.name, session).await;
		}
	}
}

/// Auto-activate a skill: register + load capabilities + inject content into session.
async fn auto_activate_skill(name: &str, session: &mut crate::session::chat::session::ChatSession) {
	let call = crate::mcp::McpToolCall {
		tool_name: "skill".to_string(),
		tool_id: format!("auto_{}", name),
		parameters: serde_json::json!({
			"action": "use_silent",
			"name": name
		}),
	};

	match super::skill::execute_skill_tool(&call).await {
		Ok(_) => {
			if let Some(content) = super::skill::take_silent_skill_content() {
				let _ = session.add_user_message(&content);
			}
			if std::io::IsTerminal::is_terminal(&std::io::stderr()) {
				use colored::Colorize;
				eprintln!("{} {}", "Using skill:".dimmed(), name.bright_cyan());
			}
		}
		Err(e) => {
			crate::log_debug!("skill_auto: failed to activate '{}': {}", name, e);
		}
	}
}

/// Track validator retry counts per skill. Reset when validation passes,
/// when a skill is deactivated, or when a new session pool is initialized.
static VALIDATOR_RETRIES: OnceLock<Arc<RwLock<std::collections::HashMap<String, u32>>>> =
	OnceLock::new();

fn get_retry_tracker() -> &'static Arc<RwLock<std::collections::HashMap<String, u32>>> {
	VALIDATOR_RETRIES.get_or_init(|| Arc::new(RwLock::new(std::collections::HashMap::new())))
}

/// Clear retry counter for a specific skill.
#[allow(dead_code)]
fn clear_retry_count(skill_name: &str) {
	let mut retries = get_retry_tracker().write().unwrap();
	retries.remove(skill_name);
}

/// Run validators from all active skills for the given event.
///
/// Returns a list of validation failures (skill_name, stderr) that should be
/// fed back to the LLM as error messages. Respects `[skills]` config:
/// `validation_timeout` and `max_retries`.
pub async fn run_validators(
	event: Event,
	content: &str,
	workdir: &std::path::Path,
) -> Vec<(String, String)> {
	let skills_config = get_skills_config();
	let session_id = match crate::session::context::current_session_id() {
		Some(id) => id,
		None => return Vec::new(),
	};

	let active_skills = crate::session::context::get_active_skills(&session_id);
	if active_skills.is_empty() {
		return Vec::new();
	}

	let timeout = if skills_config.validation_timeout == 0 {
		Duration::from_secs(3600) // 0 = effectively unlimited (1h)
	} else {
		Duration::from_secs(skills_config.validation_timeout)
	};
	let max_retries = skills_config.max_retries;

	// Find validate scripts for active skills
	let taps = match crate::agent::taps::get_taps() {
		Ok(t) => t,
		Err(_) => return Vec::new(),
	};

	let mut tasks = Vec::new();
	let retry_tracker = get_retry_tracker();

	for skill_name in &active_skills {
		// Check retry cap before even running the script
		if max_retries > 0 {
			let retries = retry_tracker.read().unwrap();
			if let Some(&count) = retries.get(skill_name) {
				if count >= max_retries {
					crate::log_debug!(
						"skill_auto: validator '{}' exceeded max_retries ({}), skipping",
						skill_name,
						max_retries
					);
					continue;
				}
			}
		}

		// Find the skill's validate script across taps
		for tap in &taps {
			let skills_dir = match tap.skills_dir() {
				Ok(d) if d.exists() => d,
				_ => continue,
			};

			let skill_dir = skills_dir.join(skill_name);
			if !skill_dir.is_dir() {
				continue;
			}

			let validate_script = skill_dir.join("validate");
			if !validate_script.exists() {
				break; // skill found but no validate script
			}

			let event_str = event.as_str().to_string();
			let content = content.to_string();
			let workdir = workdir.to_path_buf();
			let name = skill_name.clone();

			tasks.push(tokio::spawn(async move {
				let result =
					run_validate_script(&validate_script, &event_str, &content, &workdir, timeout)
						.await;
				(name, result)
			}));

			break; // found the skill, stop searching taps
		}
	}

	let mut failures = Vec::new();

	for task in tasks {
		match task.await {
			Ok((name, Ok((exit_code, stderr)))) => {
				if exit_code != 0 && !stderr.is_empty() {
					// Increment retry counter
					let mut retries = retry_tracker.write().unwrap();
					let count = retries.entry(name.clone()).or_insert(0);
					*count += 1;
					failures.push((name, stderr));
				} else if exit_code == 0 {
					// Validation passed — reset retry counter
					let mut retries = retry_tracker.write().unwrap();
					retries.remove(&name);
				}
			}
			Ok((name, Err(e))) => {
				crate::log_debug!("skill_auto: '{}' validate script error: {}", name, e);
			}
			Err(e) => {
				crate::log_debug!("skill_auto: validator task join error: {}", e);
			}
		}
	}

	failures
}

/// Run a validate script. Returns (exit_code, stderr).
async fn run_validate_script(
	script_path: &std::path::Path,
	event: &str,
	content: &str,
	workdir: &std::path::Path,
	timeout: Duration,
) -> anyhow::Result<(i32, String)> {
	let mut child = tokio::process::Command::new(script_path)
		.arg(event)
		.current_dir(workdir)
		.stdin(Stdio::piped())
		.stdout(Stdio::piped())
		.stderr(Stdio::piped())
		.spawn()
		.map_err(|e| anyhow::anyhow!("Failed to spawn {}: {}", script_path.display(), e))?;

	// Write content to stdin
	if let Some(mut stdin) = child.stdin.take() {
		let _ = stdin.write_all(content.as_bytes()).await;
		drop(stdin);
	}

	// Wait with timeout
	match tokio::time::timeout(timeout, child.wait_with_output()).await {
		Ok(Ok(output)) => {
			let exit_code = output.status.code().unwrap_or(1);
			let stderr = String::from_utf8_lossy(&output.stderr).to_string();
			// Also capture stdout as part of the error if stderr is empty
			let error_output = if stderr.trim().is_empty() {
				String::from_utf8_lossy(&output.stdout).to_string()
			} else {
				stderr
			};
			Ok((exit_code, error_output))
		}
		Ok(Err(e)) => Err(anyhow::anyhow!("Script wait error: {}", e)),
		Err(_) => Err(anyhow::anyhow!("Validator timed out")),
	}
}

/// Auto-deactivate a skill: forget + compress + clear retry counter.
#[allow(dead_code)]
fn auto_deactivate_skill(name: &str, _session_id: &str) {
	let sid = crate::session::context::current_session_id().unwrap_or_default();
	let n = name.to_string();
	if crate::session::context::has_active_skill(&sid, &n) {
		crate::session::context::remove_active_skill(&sid, &n);
		crate::session::context::request_skill_compression(&sid);
		clear_retry_count(name);
		crate::log_debug!("skill_auto: deactivated '{}'", name);
	}
}
