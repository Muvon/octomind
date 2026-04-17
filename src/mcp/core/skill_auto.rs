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

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;
use tokio::io::AsyncWriteExt;

/// Cached skill pool entry — a skill with an `activate` script.
#[derive(Debug, Clone)]
struct PoolEntry {
	name: String,
	skill_dir: PathBuf,
	_domains: Vec<String>,
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

			// Must have activate script
			if !super::skill::has_activate_script(&skill_dir) {
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

			// Must have domains that include the current domain
			if meta.domains.is_empty() || !meta.domains.iter().any(|d| d == domain) {
				continue;
			}

			entries.push(PoolEntry {
				name: meta.name,
				skill_dir,
				_domains: meta.domains,
			});
		}
	}

	crate::log_debug!(
		"skill_auto: initialized pool with {} skills for domain '{}'",
		entries.len(),
		domain
	);

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

/// Get the skills config from the current session, falling back to defaults.
fn get_skills_config() -> crate::config::SkillsConfig {
	crate::session::context::current_session_id()
		.and_then(|sid| crate::session::context::get_session_config(&sid))
		.map(|cfg| cfg.skills.clone())
		.unwrap_or_default()
}

/// Run auto-activation for the given event and content.
///
/// Executes `activate` scripts from the skill pool in parallel.
/// Skills returning exit 0 are activated, non-zero are deactivated.
/// Respects `[skills]` config: `auto_activation` flag and `activation_timeout`.
pub async fn run_activation(event: Event, content: &str, workdir: &std::path::Path) {
	let skills_config = get_skills_config();

	// Check if auto-activation is enabled
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
	let timeout = if skills_config.activation_timeout == 0 {
		Duration::from_secs(3600) // 0 = effectively unlimited (1h)
	} else {
		Duration::from_secs(skills_config.activation_timeout)
	};

	// Run all activate scripts in parallel
	let mut tasks = Vec::new();
	for entry in &entries {
		let script_path = entry.skill_dir.join("activate");
		let event_str = event.as_str().to_string();
		let content = content.to_string();
		let workdir = workdir.to_path_buf();
		let name = entry.name.clone();

		tasks.push(tokio::spawn(async move {
			let result = run_script(&script_path, &event_str, &content, &workdir, timeout).await;
			(name, result)
		}));
	}

	// Collect results
	let mut to_activate = Vec::new();
	let mut to_deactivate = Vec::new();

	for task in tasks {
		match task.await {
			Ok((name, Ok(exit_code))) => {
				let is_active = active_skills.contains(&name);
				if exit_code == 0 && !is_active {
					to_activate.push(name);
				} else if exit_code != 0 && is_active {
					to_deactivate.push(name);
				}
			}
			Ok((name, Err(e))) => {
				crate::log_debug!("skill_auto: '{}' activate script error: {}", name, e);
			}
			Err(e) => {
				crate::log_debug!("skill_auto: task join error: {}", e);
			}
		}
	}

	// Activate new skills
	for name in &to_activate {
		crate::log_debug!("skill_auto: auto-activating '{}'", name);
		auto_activate_skill(name, &session_id).await;
	}

	// Deactivate skills
	for name in &to_deactivate {
		crate::log_debug!("skill_auto: auto-deactivating '{}'", name);
		auto_deactivate_skill(name, &session_id);
	}
}

/// Run a script with event type as argv[1] and content on stdin.
/// Returns the exit code.
async fn run_script(
	script_path: &std::path::Path,
	event: &str,
	content: &str,
	workdir: &std::path::Path,
	timeout: Duration,
) -> anyhow::Result<i32> {
	let mut child = tokio::process::Command::new(script_path)
		.arg(event)
		.current_dir(workdir)
		.stdin(Stdio::piped())
		.stdout(Stdio::null())
		.stderr(Stdio::null())
		.spawn()
		.map_err(|e| anyhow::anyhow!("Failed to spawn {}: {}", script_path.display(), e))?;

	// Write content to stdin
	if let Some(mut stdin) = child.stdin.take() {
		let _ = stdin.write_all(content.as_bytes()).await;
		drop(stdin);
	}

	// Wait with timeout
	match tokio::time::timeout(timeout, child.wait()).await {
		Ok(Ok(status)) => Ok(status.code().unwrap_or(1)),
		Ok(Err(e)) => Err(anyhow::anyhow!("Script wait error: {}", e)),
		Err(_) => {
			let _ = child.kill().await;
			Err(anyhow::anyhow!("Script timed out"))
		}
	}
}

/// Auto-activate a skill: inject content via inbox + load capabilities.
async fn auto_activate_skill(name: &str, _session_id: &str) {
	// Use the existing skill activation logic by constructing a synthetic tool call
	let call = crate::mcp::McpToolCall {
		tool_name: "skill".to_string(),
		tool_id: format!("auto_{}", name),
		parameters: serde_json::json!({
			"action": "use",
			"name": name
		}),
	};

	match super::skill::execute_skill_tool(&call).await {
		Ok(result) => {
			crate::log_debug!(
				"skill_auto: activated '{}': {}",
				name,
				result.extract_content()
			);
		}
		Err(e) => {
			crate::log_debug!("skill_auto: failed to activate '{}': {}", name, e);
		}
	}
}

/// Track validator retry counts per skill. Reset when validation passes.
static VALIDATOR_RETRIES: OnceLock<Arc<RwLock<std::collections::HashMap<String, u32>>>> =
	OnceLock::new();

fn get_retry_tracker() -> &'static Arc<RwLock<std::collections::HashMap<String, u32>>> {
	VALIDATOR_RETRIES.get_or_init(|| Arc::new(RwLock::new(std::collections::HashMap::new())))
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

/// Auto-deactivate a skill: forget + compress.
fn auto_deactivate_skill(name: &str, _session_id: &str) {
	let sid = crate::session::context::current_session_id().unwrap_or_default();
	let n = name.to_string();
	if crate::session::context::has_active_skill(&sid, &n) {
		crate::session::context::remove_active_skill(&sid, &n);
		crate::session::context::request_skill_compression(&sid);
		crate::log_debug!("skill_auto: deactivated '{}'", name);
	}
}
