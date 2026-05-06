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
//! Scans the tap skill pool for skills with declarative rules, filtered by
//! the current agent's domain. Evaluates rules on user input to determine
//! which skills should be active.
//!
//! When a skill auto-activates, its required capabilities are auto-loaded
//! (MCP servers enabled) and its content is injected via the inbox.
//!
//! Validators run only on the final assistant message (end of turn),
//! passing the assistant content to each skill's `validate` script.

use std::process::Stdio;
use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;
use tokio::io::AsyncWriteExt;

/// Cached skill pool entry — a skill with declarative rules.
#[derive(Debug, Clone)]
struct PoolEntry {
	name: String,
	rules: Vec<Vec<super::skill::ActivateCheck>>,
}

/// Cached pool of auto-activatable skills, filtered by domain.
struct SkillPool {
	entries: Vec<PoolEntry>,
}

static SKILL_POOL: OnceLock<Arc<RwLock<Option<SkillPool>>>> = OnceLock::new();

fn get_pool() -> &'static Arc<RwLock<Option<SkillPool>>> {
	SKILL_POOL.get_or_init(|| Arc::new(RwLock::new(None)))
}

/// Load skills from OCTOMIND_SKILLS env var (if set). Called at session start from all five entry points.
///
/// When resuming a session that already had these skills (from previous run or /skill use), we guard against
/// re-injection using the active_skills registry. This prevents duplicate <skill name="..."> messages in the
/// conversation history. The legacy message scan is kept as fallback for restored sessions that may not have
/// populated the registry yet.
///
/// Skills from OCTOMIND_SKILLS are always marked active (even if already present).
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

	let session_id = crate::session::context::current_session_id();

	// Collect skill IDs already in session (from previous run / resume)
	let existing: std::collections::HashSet<String> = session
		.session
		.messages
		.iter()
		.filter(|m| m.role == "user")
		.filter_map(|m| super::skill::extract_skill_name(&m.content).map(String::from))
		.collect();

	for name in &skill_names {
		let name_str = (*name).to_string();

		// Primary guard: if already active in this session (from resume, /skill, or prior load_env_skills), skip injection
		if session_id
			.as_ref()
			.is_some_and(|sid| crate::session::context::has_active_skill(sid, &name_str))
		{
			// Still ensure it is registered (harmless if duplicate)
			if let Some(sid) = &session_id {
				crate::session::context::add_active_skill(sid, &name_str);
				crate::session::context::add_env_skill(sid, &name_str);
			}
			continue;
		}

		if existing.contains(*name) {
			// Legacy path for restored sessions without active registry entry
			if let Some(sid) = &session_id {
				crate::session::context::add_active_skill(sid, &name_str);
				crate::session::context::add_env_skill(sid, &name_str);
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
				// Emit structured event for JSONL/WebSocket consumers
				if let Some(sid) = &session_id {
					crate::mcp::process::send_notification_message(
						crate::websocket::ServerMessage::skill(
							"activate",
							&name_str,
							Some("env(OCTOMIND_SKILLS)".to_string()),
							sid.clone(),
						),
					);
				}
			}
			Err(e) => {
				let suppress = crate::config::with_thread_config(|c| c.output_mode())
					.map(|m| m.should_suppress_cli_output())
					.unwrap_or(false);
				if !suppress {
					eprintln!("OCTOMIND_SKILLS: skill '{}' failed: {}", name, e);
				} else {
					crate::log_debug!("OCTOMIND_SKILLS: skill '{}' failed: {}", name, e);
				}
			}
		}
	}
}

/// Initialize the skill pool for the given agent domain (e.g., "developer").
/// Scans all taps for skills with declarative rules whose `domains` field
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
	let mut seen_names = std::collections::HashSet::new();

	// 1. Tap skills (highest priority)
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

			// Must have rules
			if meta.rules.is_empty() {
				continue;
			}

			// Must have domains that include the current domain
			if meta.domains.is_empty() || !meta.domains.iter().any(|d| d == domain) {
				continue;
			}

			if seen_names.insert(meta.name.clone()) {
				entries.push(PoolEntry {
					name: meta.name,
					rules: meta.rules,
				});
			}
		}
	}

	// 2. Universal skill dirs (npx skills) — fallback after taps
	let workdir = crate::mcp::workdir::get_thread_working_directory();
	for dir in super::skill::universal_skill_dirs(&workdir) {
		let dir_entries = match std::fs::read_dir(&dir) {
			Ok(e) => e,
			Err(_) => continue,
		};

		for entry in dir_entries.flatten() {
			let skill_dir = entry.path();
			if !skill_dir.is_dir() {
				continue;
			}

			let skill_md = skill_dir.join("SKILL.md");
			let content = match std::fs::read_to_string(&skill_md) {
				Ok(c) => c,
				Err(_) => continue,
			};

			let meta = match super::skill::parse_skill_meta(&content) {
				Some(m) => m,
				None => continue,
			};

			if meta.rules.is_empty() {
				continue;
			}

			if meta.domains.is_empty() || !meta.domains.iter().any(|d| d == domain) {
				continue;
			}

			if seen_names.insert(meta.name.clone()) {
				entries.push(PoolEntry {
					name: meta.name,
					rules: meta.rules,
				});
			}
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
	*pool = Some(SkillPool { entries });
}

/// Get the skills config from the current session config.
fn get_skills_config() -> crate::config::SkillsConfig {
	crate::session::context::current_session_id()
		.and_then(|sid| crate::session::context::get_session_config(&sid))
		.map(|cfg| cfg.skills.clone())
		.unwrap_or(crate::config::SkillsConfig {
			auto_activation: true,
			auto_validation: true,
			activation_timeout: 3,
			validation_timeout: 60,
			max_retries: 3,
		})
}

/// Run auto-activation for the given content.
///
/// Evaluates declarative rules from the skill pool in-process.
/// Strip XML-style blocks (`<tag>...</tag>`) from a string so that injected
/// context (system tags, skill blocks, log pastes, etc.) does not influence
/// skill auto-activation matching.  Only the plain user-written text remains.
pub(crate) fn strip_xml_blocks(input: &str) -> std::borrow::Cow<'_, str> {
	// Fast path: no '<' at all.
	if !input.contains('<') {
		return std::borrow::Cow::Borrowed(input);
	}

	let mut out = String::with_capacity(input.len());
	let mut rest = input;
	while let Some(open_start) = rest.find('<') {
		// Collect the tag name (letters, digits, hyphens, underscores).
		let after_lt = &rest[open_start + 1..];
		let tag_end = after_lt
			.find(|c: char| !c.is_alphanumeric() && c != '-' && c != '_')
			.unwrap_or(after_lt.len());
		let tag = &after_lt[..tag_end];

		if tag.is_empty() {
			// Not a real tag — keep the '<' and advance past it.
			out.push_str(&rest[..open_start + 1]);
			rest = &rest[open_start + 1..];
			continue;
		}

		// Look for the matching closing tag.
		let close_tag = format!("</{tag}>");
		if let Some(close_pos) = rest.find(&close_tag) {
			// Emit text before the opening '<', skip the entire block.
			out.push_str(&rest[..open_start]);
			rest = &rest[close_pos + close_tag.len()..];
		} else {
			// No closing tag found — keep everything up to and including '<'.
			out.push_str(&rest[..open_start + 1]);
			rest = &rest[open_start + 1..];
		}
	}
	out.push_str(rest);
	std::borrow::Cow::Owned(out)
}

/// Any AND-group matching activates the skill. No process spawns.
pub async fn run_activation(
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

	// Strip XML blocks (skill injections, log pastes, system tags, etc.) so
	// they don't trigger false-positive skill matches.
	let stripped = strip_xml_blocks(content);
	let content: &str = &stripped;

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

	let session_name = session.session.info.name.clone();

	// Pre-compute semantic similarity scores once per evaluation cycle so
	// the rule loop stays sync. Embeds the user message + every unique
	// `semantic(phrase)` argument from inactive skills in one batch, then
	// builds a phrase → cosine table that `ActivateCheck::matches` reads.
	// Returns None when no semantic checks exist or the model isn't ready
	// — those Semantic rules then evaluate to false silently.
	let semantic_scores = compute_semantic_scores(content, &entries, &active_skills).await;
	let semantic_ref = semantic_scores.as_ref();

	// Bucket each skill into one of three outcomes:
	//   - deterministic match: a fully-non-semantic AND-group matched
	//     (file/content/grep/match/env/bin/session/workdir). These are
	//     hand-authored, precise — fire unconditionally, no margin gate.
	//   - semantic candidate: only semantic-bearing groups matched. The
	//     skill enters a winner-take-all selection where the top scorer
	//     must beat #2 by SEMANTIC_MARGIN to fire. Prevents the avalanche
	//     where ambiguous prompts ("rewrite my landing page text") clear
	//     the floor for many marketing/copy skills at once.
	//   - no match: skipped silently.
	let mut deterministic: Vec<(String, String)> = Vec::new();
	let mut semantic_candidates: Vec<(f32, String, String)> = Vec::new();

	for entry in &entries {
		if active_skills.contains(&entry.name) {
			continue;
		}

		let mut det_trigger: Option<String> = None;
		let mut sem_best: Option<(f32, String)> = None;

		for group in &entry.rules {
			if !group
				.iter()
				.all(|check| check.matches(content, workdir, &session_name, semantic_ref))
			{
				continue;
			}

			let trigger = group
				.iter()
				.map(|c| c.to_string())
				.collect::<Vec<_>>()
				.join(" ");

			let has_semantic = group
				.iter()
				.any(|c| matches!(c, super::skill::ActivateCheck::Semantic { .. }));

			if !has_semantic {
				det_trigger = Some(trigger);
				break;
			}

			let group_score = group
				.iter()
				.filter_map(|c| match c {
					super::skill::ActivateCheck::Semantic { phrase, .. } => {
						semantic_ref.and_then(|s| s.get(phrase)).copied()
					}
					_ => None,
				})
				.fold(f32::NEG_INFINITY, f32::max);

			let group_score = if group_score.is_finite() {
				group_score
			} else {
				0.0
			};

			match &sem_best {
				Some((best, _)) if group_score <= *best => {}
				_ => sem_best = Some((group_score, trigger)),
			}
		}

		if let Some(trigger) = det_trigger {
			deterministic.push((entry.name.clone(), trigger));
		} else if let Some((score, trigger)) = sem_best {
			semantic_candidates.push((score, entry.name.clone(), trigger));
		} else {
			crate::log_debug!("skill_auto: no rule matched for '{}'", entry.name);
		}
	}

	for (name, trigger) in &deterministic {
		crate::log_debug!("skill_auto: activated '{}' via [{}]", name, trigger);
		auto_activate_skill(name, trigger, session).await;
	}

	semantic_candidates.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
	if let Some((top1, name, trigger)) = semantic_candidates.first().cloned() {
		let top2 = semantic_candidates.get(1).map(|x| x.0).unwrap_or(0.0);
		if top1 - top2 >= super::skill::SEMANTIC_MARGIN {
			crate::log_debug!(
				"skill_auto: activated '{}' via [{}] (semantic top1={:.3}, top2={:.3}, margin ok)",
				name,
				trigger,
				top1,
				top2
			);
			auto_activate_skill(&name, &trigger, session).await;
		} else {
			crate::log_debug!(
				"skill_auto: {} semantic candidate(s) abstained — top1={:.3} top2={:.3} gap {:.3} < {} (winner: '{}')",
				semantic_candidates.len(),
				top1,
				top2,
				top1 - top2,
				super::skill::SEMANTIC_MARGIN,
				name
			);
		}
	}
}

/// Pre-compute cosine similarity for every `semantic(phrase)` rule across
/// inactive skills. Embeds the user message once and batch-embeds all
/// unique phrases (one network/CPU pass), then builds `phrase → cosine`.
///
/// Returns `None` when:
/// - No `Semantic` checks exist anywhere in the inactive pool (skip entirely)
/// - The embedding model isn't ready yet (warmup pending, no network)
/// - Embedding fails for any reason
///
/// In all cases, downstream `ActivateCheck::matches` treats `Semantic` as
/// `false` — same fall-through pattern as capability auto-activation, so
/// non-semantic rules in the same skill still fire correctly.
async fn compute_semantic_scores(
	content: &str,
	entries: &[PoolEntry],
	active_skills: &[String],
) -> Option<std::collections::HashMap<String, f32>> {
	use std::collections::{HashMap, HashSet};

	let mut phrases: HashSet<String> = HashSet::new();
	for entry in entries {
		if active_skills.iter().any(|n| n == &entry.name) {
			continue;
		}
		for group in &entry.rules {
			for check in group {
				if let super::skill::ActivateCheck::Semantic { phrase, .. } = check {
					phrases.insert(phrase.clone());
				}
			}
		}
	}
	if phrases.is_empty() {
		return None;
	}

	if !crate::embeddings::is_ready() {
		crate::log_debug!(
			"skill_auto: embedding model not ready, semantic({} phrase{}) check{} will evaluate false",
			phrases.len(),
			if phrases.len() == 1 { "" } else { "s" },
			if phrases.len() == 1 { "" } else { "s" }
		);
		return None;
	}

	let content_vec = match crate::embeddings::embed(content).await {
		Ok(v) => v,
		Err(e) => {
			crate::log_debug!("skill_auto: failed to embed user message ({})", e);
			return None;
		}
	};

	let phrase_list: Vec<String> = phrases.into_iter().collect();
	let phrase_vecs = match crate::embeddings::embed_many(&phrase_list).await {
		Ok(v) => v,
		Err(e) => {
			crate::log_debug!("skill_auto: failed to embed semantic phrases ({})", e);
			return None;
		}
	};

	let mut scores: HashMap<String, f32> = HashMap::with_capacity(phrase_list.len());
	for (phrase, vec) in phrase_list.iter().zip(phrase_vecs.iter()) {
		let cosine = crate::embeddings::cosine(&content_vec, vec);
		scores.insert(phrase.clone(), cosine);
	}
	Some(scores)
}

/// Auto-activate a skill: register + load capabilities + inject content into session.
async fn auto_activate_skill(
	name: &str,
	trigger: &str,
	session: &mut crate::session::chat::session::ChatSession,
) {
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

			// Emit structured event for JSONL/WebSocket consumers
			if let Some(sid) = crate::session::context::current_session_id() {
				crate::mcp::process::send_notification_message(
					crate::websocket::ServerMessage::skill(
						"activate",
						name,
						Some(trigger.to_string()),
						sid,
					),
				);
			}

			// Plain-text print: only when not suppressing CLI output (i.e. skip for jsonl/websocket)
			let suppress = crate::config::with_thread_config(|c| c.output_mode())
				.map(|m| m.should_suppress_cli_output())
				.unwrap_or(false);
			if !suppress && std::io::IsTerminal::is_terminal(&std::io::stderr()) {
				use colored::Colorize;
				eprintln!(
					"{} {} {}",
					"Using skill:".dimmed(),
					name.bright_cyan(),
					format!("[{}]", trigger).dimmed()
				);
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

/// Run validators from all active skills on the final assistant message.
///
/// Returns a list of validation failures (skill_name, stderr) that should be
/// fed back to the LLM as error messages. Respects `[skills]` config:
/// `validation_timeout` and `max_retries`.
pub async fn run_validators(content: &str, workdir: &std::path::Path) -> Vec<(String, String)> {
	let skills_config = get_skills_config();

	if !skills_config.auto_validation {
		return Vec::new();
	}

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
	// Names of skills whose validators we actually scheduled — used for the
	// animation phase label so the user sees exactly what's being validated.
	let mut scheduled_names: Vec<String> = Vec::new();

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

			let content = content.to_string();
			let workdir = workdir.to_path_buf();
			let name = skill_name.clone();
			scheduled_names.push(skill_name.clone());

			tasks.push(tokio::spawn(async move {
				let result =
					run_validate_script(&validate_script, &content, &workdir, timeout).await;
				(name, result)
			}));

			break; // found the skill, stop searching taps
		}
	}

	// Nothing to run — skip the phase overhead entirely.
	if tasks.is_empty() {
		return Vec::new();
	}

	// Show "Validating (skill1, skill2)…" on the spinner while validators run.
	// No-op in non-interactive modes; safe to always call. Cleared unconditionally
	// below so a panic in a task can't leave the phase sticky.
	let phase_label = format!("Validating ({})…", scheduled_names.join(", "));
	crate::session::chat::animation_manager::get_animation_manager()
		.set_phase(&phase_label)
		.await;

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

	// Restore the standard "Working …" message regardless of outcome.
	crate::session::chat::animation_manager::get_animation_manager().clear_phase();

	failures
}

/// Run a validate script. Passes `"assistant"` as the first argument and
/// the assistant message content on stdin. Returns (exit_code, stderr).
async fn run_validate_script(
	script_path: &std::path::Path,
	content: &str,
	workdir: &std::path::Path,
	timeout: Duration,
) -> anyhow::Result<(i32, String)> {
	let mut child = tokio::process::Command::new(script_path)
		.arg("assistant")
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
