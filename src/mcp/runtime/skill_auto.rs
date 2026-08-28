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
					// Don't silently mark the skill active if its instructions never
					// made it into the session — at least surface the failure.
					if let Err(e) = session.add_system_managed_user_message(&content) {
						crate::log_error!(
							"Failed to inject auto-activated skill '{}': {}",
							name_str,
							e
						);
						continue;
					}
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
/// Minimum non-whitespace character count for user input to drive
/// auto-activation. Short acknowledgments ("try", "ok", "do it", "thanks")
/// carry no real intent — letting them activate skills/capabilities causes
/// expensive MCP server loads on typos and one-word follow-ups (the dominant
/// false-positive class in production logs). Tuned so 2-word intents like
/// "run tests" (8 chars) and "list files" (9 chars) still pass, while
/// "try"/"ok"/"do it"/"fix bug" abstain. The semantic margin gate further
/// filters longer-but-ambiguous inputs.
pub(crate) const MIN_INTENT_NON_WS_CHARS: usize = 8;

/// Returns true when `input` has enough content to justify running
/// auto-activation (both skill rule evaluation and capability semantic
/// matching). Counted on non-whitespace chars after XML stripping so that
/// `<log>…</log>` pastes don't artificially inflate the signal of an
/// otherwise empty user message.
pub(crate) fn intent_has_enough_signal(input: &str) -> bool {
	input.chars().filter(|c| !c.is_whitespace()).count() >= MIN_INTENT_NON_WS_CHARS
}

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

	// Control-plane text is never a user task: supervisor steers/recalls
	// (`<pay-attention>`, `<recall>`), skill blocks, continuation wrappers and
	// `<system-note>` injections are our own messages replayed in the user role.
	// Matching them would auto-inject a skill in response to the supervisor
	// rather than to anything the user asked for. Same predicate the compression
	// and gate paths use, so the three can't drift apart.
	if crate::session::is_system_managed_user_content(content) {
		crate::log_debug!("skill_auto: skipping activation — system-managed content");
		return;
	}

	// Strip XML blocks (skill injections, log pastes, system tags, etc.) so
	// they don't trigger false-positive skill matches.
	let stripped = strip_xml_blocks(content);
	let content: &str = &stripped;

	// Bail before any rule evaluation when the user message is too short to
	// carry intent. Deterministic file/content checks would otherwise fire on
	// "try"/"ok"/"hmm" in any project that happens to have a matching marker
	// file, dragging heavy MCP servers in for what's almost always a typo.
	if !intent_has_enough_signal(content) {
		crate::log_debug!(
			"skill_auto: skipping activation — intent below {} non-ws chars: {:?}",
			MIN_INTENT_NON_WS_CHARS,
			content
		);
		return;
	}

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
				let _ = session.add_system_managed_user_message(&content);
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
					"{} {} {} {}",
					"·".bright_black(),
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
		// Kill the validator if this future is dropped (timeout below) — otherwise
		// each timed-out validation leaks one orphaned script process.
		.kill_on_drop(true)
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

#[cfg(test)]
mod tests {
	use super::*;
	use serial_test::serial;

	#[test]
	fn intent_gate_rejects_short_acknowledgments() {
		// Common chatter that should never drive auto-activation.
		for short in [
			"", " ", "try", "ok", "yes", "no", "hmm", "do it", "thanks!", "what?",
		] {
			assert!(
				!intent_has_enough_signal(short),
				"expected {short:?} to be rejected by intent gate"
			);
		}
	}

	#[test]
	fn intent_gate_accepts_two_word_intents() {
		// Real 2-word intents at the boundary should pass.
		for ok in [
			"run tests",
			"list files",
			"deploy app",
			"build code",
			"show me logs",
			"explain this code to me",
		] {
			assert!(
				intent_has_enough_signal(ok),
				"expected {ok:?} to pass intent gate"
			);
		}
	}

	#[test]
	fn intent_gate_ignores_whitespace_padding() {
		// Pure whitespace and padded short inputs are still rejected.
		assert!(!intent_has_enough_signal("   \n\t  "));
		assert!(!intent_has_enough_signal("  try   "));
		// Whitespace doesn't pad a real intent up to the threshold.
		assert!(!intent_has_enough_signal("a b c"));
	}

	#[test]
	fn system_managed_content_is_not_user_intent() {
		// Supervisor steers / recalls, skill replays and continuation wrappers
		// must never drive auto-activation — run_activation returns early on them.
		for synthetic in [
			"<pay-attention>\nYou have made several single-call turns in a row.\n</pay-attention>",
			"<recall>\npast-session lesson\n</recall>",
			"<system-note>\nbackground job finished\n</system-note>",
			"<skill name=\"tap-agent-authoring\" description=\"x\">\nbody\n</skill>",
			"<continuation>\n<task>resume</task>\n</continuation>",
		] {
			assert!(
				crate::session::is_system_managed_user_content(synthetic),
				"expected {synthetic:?} to be classified as system-managed"
			);
		}
		assert!(!crate::session::is_system_managed_user_content(
			"please create an agent manifest for developer:plan"
		));
	}

	// -------------------------------------------------------------------------
	// strip_xml_blocks
	// -------------------------------------------------------------------------

	#[test]
	fn strip_xml_no_tags_returns_borrowed_input() {
		let out = strip_xml_blocks("plain text with no markup");
		assert_eq!(out, "plain text with no markup");
		// Fast path must not allocate.
		assert!(matches!(out, std::borrow::Cow::Borrowed(_)));
	}

	#[test]
	fn strip_xml_removes_block_and_keeps_surrounding_text() {
		assert_eq!(
			strip_xml_blocks("before <log>noise</log> after"),
			"before  after"
		);
		// Multiline blocks are removed whole.
		assert_eq!(
			strip_xml_blocks("head\n<log>\nline1\nline2\n</log>\ntail"),
			"head\n\ntail"
		);
		assert_eq!(strip_xml_blocks("<skill name=\"x\">body</skill>"), "");
	}

	#[test]
	fn strip_xml_removes_multiple_disjoint_blocks() {
		assert_eq!(strip_xml_blocks("a<x>1</x>b<y>2</y>c"), "abc");
	}

	#[test]
	fn strip_xml_removes_outer_block_with_nested_inner_tags() {
		// The outer <a> block swallows the nested <b> block entirely.
		assert_eq!(
			strip_xml_blocks("keep <a>x<b>y</b>z</a> tail"),
			"keep  tail"
		);
	}

	#[test]
	fn strip_xml_keeps_unclosed_tag_verbatim() {
		let out = strip_xml_blocks("hello <b>world");
		assert_eq!(out, "hello <b>world");
		assert!(matches!(out, std::borrow::Cow::Owned(_)));
	}

	#[test]
	fn strip_xml_keeps_non_tag_lt_characters() {
		// Comparisons, empty tags, and "<3" are not block openers.
		assert_eq!(strip_xml_blocks("a < b"), "a < b");
		assert_eq!(strip_xml_blocks("x <> y"), "x <> y");
		assert_eq!(strip_xml_blocks("i <3 u"), "i <3 u");
	}

	#[test]
	fn strip_xml_matches_close_tag_with_attributed_open_tag() {
		// Attributes on the open tag don't break close-tag matching.
		assert_eq!(strip_xml_blocks("<log type=\"err\">boom</log>"), "");
	}

	#[test]
	fn strip_xml_block_content_may_contain_lt() {
		assert_eq!(strip_xml_blocks("<log>a<b and more</log>"), "");
	}

	#[test]
	fn strip_xml_same_name_nesting_stops_at_first_close() {
		// Documents current behavior: the first matching close tag wins, so
		// the trailing "</a>" survives as literal text.
		assert_eq!(strip_xml_blocks("<a>x<a>y</a>z</a>"), "z</a>");
	}

	// -------------------------------------------------------------------------
	// Intent gate
	// -------------------------------------------------------------------------

	#[test]
	fn intent_gate_boundary_is_eight_non_whitespace_chars() {
		assert!(!intent_has_enough_signal("1234567")); // 7 non-ws chars
		assert!(intent_has_enough_signal("12345678")); // 8 non-ws chars
												 // Whitespace never counts toward the threshold.
		assert!(intent_has_enough_signal("a b c d e f g h"));
		assert!(!intent_has_enough_signal("a b c d e f g"));
		// Multibyte chars count as chars, not bytes.
		assert!(!intent_has_enough_signal("日本語です")); // 5 chars
		assert!(intent_has_enough_signal("日本語テストです")); // 8 chars
	}

	// -------------------------------------------------------------------------
	// Skills config
	// -------------------------------------------------------------------------

	#[test]
	fn skills_config_defaults_outside_session() {
		// Outside a session scope the built-in defaults apply.
		let cfg = get_skills_config();
		assert!(cfg.auto_activation);
		assert!(cfg.auto_validation);
		assert_eq!(cfg.activation_timeout, 3);
		assert_eq!(cfg.validation_timeout, 60);
		assert_eq!(cfg.max_retries, 3);
	}

	// -------------------------------------------------------------------------
	// Semantic score precomputation
	// -------------------------------------------------------------------------

	fn semantic_entry(name: &str, phrase: &str) -> PoolEntry {
		PoolEntry {
			name: name.to_string(),
			rules: vec![vec![crate::mcp::runtime::skill::ActivateCheck::Semantic {
				phrase: phrase.to_string(),
				threshold: 0.45,
			}]],
		}
	}

	#[tokio::test]
	async fn semantic_scores_absent_when_no_semantic_checks() {
		let entries = vec![PoolEntry {
			name: "det-only".to_string(),
			rules: vec![vec![crate::mcp::runtime::skill::ActivateCheck::File(
				"Cargo.toml".to_string(),
			)]],
		}];
		// Returns before ever touching the embedding model.
		assert!(compute_semantic_scores("deploy the app", &entries, &[])
			.await
			.is_none());
	}

	#[tokio::test]
	async fn semantic_scores_absent_when_semantic_skills_already_active() {
		let entries = vec![semantic_entry("sem", "deploying")];
		let active = vec!["sem".to_string()];
		assert!(compute_semantic_scores("deploy", &entries, &active)
			.await
			.is_none());
	}

	#[tokio::test]
	async fn semantic_scores_absent_when_embedding_model_not_ready() {
		if crate::embeddings::is_ready() {
			// Another test in this binary may have warmed a locally-cached
			// model; the not-ready branch is then unreachable without a real
			// embed call, so there is nothing deterministic to assert.
			return;
		}
		let entries = vec![semantic_entry("sem", "deploying")];
		assert!(compute_semantic_scores("deploy", &entries, &[])
			.await
			.is_none());
	}

	// -------------------------------------------------------------------------
	// Validator orchestration
	// -------------------------------------------------------------------------

	#[tokio::test]
	async fn validators_return_empty_without_session() {
		let failures = run_validators("assistant text", std::path::Path::new("/tmp")).await;
		assert!(failures.is_empty());
	}

	#[tokio::test]
	#[serial]
	async fn validators_return_empty_when_auto_validation_disabled() {
		let sid = "__skillauto_no_validation".to_string();
		let mut config: crate::config::Config =
			toml::from_str(include_str!("../../../config-templates/default.toml"))
				.expect("parse default config template");
		config.skills.auto_validation = false;
		crate::session::context::set_session_config(&sid, &config);

		let failures = crate::session::context::with_session_id(sid.clone(), async {
			run_validators("assistant text", std::path::Path::new("/tmp")).await
		})
		.await;
		assert!(failures.is_empty());

		crate::session::context::cleanup_session(&sid);
	}

	#[tokio::test]
	async fn validators_return_empty_when_no_active_skills() {
		let sid = "__skillauto_no_skills".to_string();
		let failures = crate::session::context::with_session_id(sid.clone(), async {
			run_validators("assistant text", std::path::Path::new("/tmp")).await
		})
		.await;
		assert!(failures.is_empty());
		crate::session::context::cleanup_session(&sid);
	}

	#[tokio::test]
	#[serial]
	async fn validators_return_empty_when_active_skill_has_no_validate_script() {
		let sid = "__skillauto_no_script".to_string();
		crate::session::context::add_active_skill(&sid, "skillauto-no-such-skill");

		let failures = crate::session::context::with_session_id(sid.clone(), async {
			run_validators("assistant text", std::path::Path::new("/tmp")).await
		})
		.await;
		// No installed tap carries that skill, so nothing is scheduled.
		assert!(failures.is_empty());

		crate::session::context::cleanup_session(&sid);
	}

	// -------------------------------------------------------------------------
	// Validate script subprocess contract
	// -------------------------------------------------------------------------

	// Callers all run `#!/bin/sh` scripts and are `#[cfg(unix)]`-gated below;
	// the helper itself stays cross-platform (chmod is Unix-only) so Windows
	// test builds compile it without a dead-code warning.
	#[cfg_attr(not(unix), allow(dead_code))]
	fn write_script(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
		let path = dir.join("validate");
		std::fs::write(&path, body).expect("write validate script");
		#[cfg(unix)]
		{
			use std::os::unix::fs::PermissionsExt;
			let mut perms = std::fs::metadata(&path)
				.expect("script metadata")
				.permissions();
			perms.set_mode(0o755);
			std::fs::set_permissions(&path, perms).expect("make script executable");
		}
		path
	}

	#[cfg(unix)]
	#[tokio::test]
	async fn validate_script_exit_zero_with_no_output_is_ok() {
		let dir = tempfile::tempdir().expect("tempdir");
		let script = write_script(dir.path(), "#!/bin/sh\nexit 0\n");
		let (code, output) =
			run_validate_script(&script, "content", dir.path(), Duration::from_secs(10))
				.await
				.expect("script runs");
		assert_eq!(code, 0);
		assert_eq!(output, "");
	}

	#[cfg(unix)]
	#[tokio::test]
	async fn validate_script_failure_captures_stderr() {
		let dir = tempfile::tempdir().expect("tempdir");
		let script = write_script(dir.path(), "#!/bin/sh\necho 'boom' >&2\nexit 1\n");
		let (code, output) =
			run_validate_script(&script, "content", dir.path(), Duration::from_secs(10))
				.await
				.expect("script runs");
		assert_eq!(code, 1);
		assert_eq!(output, "boom\n");
	}

	#[cfg(unix)]
	#[tokio::test]
	async fn validate_script_uses_stdout_when_stderr_empty() {
		let dir = tempfile::tempdir().expect("tempdir");
		let script = write_script(dir.path(), "#!/bin/sh\necho 'stdout noise'\nexit 2\n");
		let (code, output) =
			run_validate_script(&script, "content", dir.path(), Duration::from_secs(10))
				.await
				.expect("script runs");
		assert_eq!(code, 2);
		assert_eq!(output, "stdout noise\n");
	}

	#[cfg(unix)]
	#[tokio::test]
	async fn validate_script_receives_content_on_stdin() {
		let dir = tempfile::tempdir().expect("tempdir");
		let script = write_script(dir.path(), "#!/bin/sh\ncat >&2\nexit 1\n");
		let (code, output) = run_validate_script(
			&script,
			"ASSISTANT-BODY",
			dir.path(),
			Duration::from_secs(10),
		)
		.await
		.expect("script runs");
		assert_eq!(code, 1);
		assert_eq!(output, "ASSISTANT-BODY");
	}

	#[cfg(unix)]
	#[tokio::test]
	async fn validate_script_receives_assistant_role_arg() {
		let dir = tempfile::tempdir().expect("tempdir");
		let script = write_script(dir.path(), "#!/bin/sh\necho \"$1\" >&2\nexit 3\n");
		let (code, output) =
			run_validate_script(&script, "content", dir.path(), Duration::from_secs(10))
				.await
				.expect("script runs");
		assert_eq!(code, 3);
		assert_eq!(output, "assistant\n");
	}

	#[cfg(unix)]
	#[tokio::test]
	async fn validate_script_runs_in_workdir() {
		let dir = tempfile::tempdir().expect("tempdir");
		let script = write_script(dir.path(), "#!/bin/sh\npwd >&2\nexit 1\n");
		let (_code, output) =
			run_validate_script(&script, "content", dir.path(), Duration::from_secs(10))
				.await
				.expect("script runs");
		// `pwd` reports the physical path; canonicalize the tempdir to match.
		let canonical = std::fs::canonicalize(dir.path()).expect("canonicalize workdir");
		assert_eq!(output.trim(), canonical.to_str().expect("utf8 workdir"));
	}

	#[cfg(unix)]
	#[tokio::test]
	async fn validate_script_times_out() {
		let dir = tempfile::tempdir().expect("tempdir");
		let script = write_script(dir.path(), "#!/bin/sh\nsleep 5\n");
		let err = run_validate_script(&script, "content", dir.path(), Duration::from_millis(200))
			.await
			.expect_err("script must time out");
		assert!(err.to_string().contains("Validator timed out"));
	}

	#[tokio::test]
	async fn validate_script_missing_script_is_error() {
		let err = run_validate_script(
			std::path::Path::new("/nonexistent/skillauto-validate"),
			"content",
			std::path::Path::new("/tmp"),
			Duration::from_secs(10),
		)
		.await
		.expect_err("spawn must fail");
		assert!(err.to_string().contains("Failed to spawn"));
	}

	// -------------------------------------------------------------------------
	// Retry tracker and pool init
	// -------------------------------------------------------------------------

	#[test]
	#[serial]
	fn retry_tracker_counts_and_resets() {
		let tracker = get_retry_tracker();
		tracker.write().unwrap().clear();
		tracker
			.write()
			.unwrap()
			.insert("__skillauto_retry".to_string(), 2);
		assert_eq!(
			tracker.read().unwrap().get("__skillauto_retry").copied(),
			Some(2)
		);
		tracker.write().unwrap().remove("__skillauto_retry");
		assert!(tracker.read().unwrap().get("__skillauto_retry").is_none());
		tracker.write().unwrap().clear();
	}

	/// Point `OCTOMIND_DATA_DIR` at a fresh tempdir so tap enumeration sees an
	/// empty tap set. Tests using it must be `#[serial]` (env is process-global).
	struct DataDirGuard {
		previous: Option<std::ffi::OsString>,
		_dir: tempfile::TempDir,
	}

	impl DataDirGuard {
		fn new() -> Self {
			let previous = std::env::var_os("OCTOMIND_DATA_DIR");
			let dir = tempfile::tempdir().expect("failed to create tempdir");
			std::env::set_var("OCTOMIND_DATA_DIR", dir.path());
			Self {
				previous,
				_dir: dir,
			}
		}
	}

	impl Drop for DataDirGuard {
		fn drop(&mut self) {
			match self.previous.take() {
				Some(v) => std::env::set_var("OCTOMIND_DATA_DIR", v),
				None => std::env::remove_var("OCTOMIND_DATA_DIR"),
			}
		}
	}

	#[test]
	#[serial]
	fn init_pool_with_fresh_data_dir_yields_empty_pool_and_clears_retries() {
		let _guard = DataDirGuard::new();

		// Seed a stale retry counter — init_pool must clear it (new session pool).
		get_retry_tracker()
			.write()
			.unwrap()
			.insert("__skillauto_stale".to_string(), 9);

		init_pool("skillauto-test-domain");

		{
			let pool = get_pool().read().unwrap();
			let pool = pool.as_ref().expect("pool initialized");
			assert!(
				pool.entries.is_empty(),
				"no taps in the fresh data dir, so no entries"
			);
		}
		assert!(
			get_retry_tracker()
				.read()
				.unwrap()
				.get("__skillauto_stale")
				.is_none(),
			"init_pool resets retry counters"
		);

		// Restore pre-test global state for other tests in this binary.
		*get_pool().write().unwrap() = None;
		get_retry_tracker().write().unwrap().clear();
	}
}

#[cfg(test)]
#[path = "skill_auto_tests.rs"]
mod skill_auto_tests;
