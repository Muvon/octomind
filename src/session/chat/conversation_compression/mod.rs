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

//! Conversation compression - AI-driven automatic compression for normal conversations
//!
//! This module provides automatic compression of older conversation exchanges while preserving
//! recent context. It reuses the plan compression logic but applies it to regular conversations.
//!
//! Key features:
//! - AI decides when compression is beneficial (self-reflection)
//! - Preserves last 4 turns (2 exchanges) uncompressed for context continuity
//! - Reuses existing plan compression infrastructure
//! - Triggered BEFORE user message is added to avoid breaking conversation flow

mod ai;
mod apply;
mod decision;
mod knowledge;
mod prompt;
mod range;

// Submodule entrypoints used by this orchestrator file:
// - `ai::ask_ai_decision_and_summary` runs the LLM round-trip (it builds the
//   prompt internally via `prompt::build_compression_prompt`).
// - `apply::{apply_compression, collect_preserved_skills}` materialises the
//   chosen drain range against the session.
// - `decision::{calculate_compression_net_benefit, calculate_adaptive_compression_ratio}`
//   is the cost/benefit math driving the should-we-compress gate.
// - `range::{find_compression_range, calculate_range_tokens}` decides which
//   indices to drain and what they cost in tokens.
use ai::ask_ai_decision_and_summary;
use apply::{apply_compression, collect_preserved_skills};
use decision::{calculate_adaptive_compression_ratio, calculate_compression_net_benefit};
use range::{calculate_range_tokens, find_compression_range};

use crate::config::Config;
use crate::session::chat::get_animation_manager;
use crate::session::chat::session::ChatSession;
use crate::{log_debug, log_info};
use anyhow::Result;

/// Check if we should ask AI about compression
/// Returns (should_compress, target_ratio) tuple
///
/// CACHE-AWARE: Uses amortized cost analysis to determine if compression is profitable
/// considering cache invalidation costs vs. future savings over estimated remaining turns
pub async fn should_check_compression(session: &mut ChatSession, config: &Config) -> (bool, f64) {
	// UNIFIED TOKEN CALCULATION - Use the single source of truth
	// This ensures consistency with display and all other systems
	let current_tokens = session.get_full_context_tokens(config).await;

	// HARD CEILING: max_session_tokens_threshold is the user's explicit safety limit.
	// When set and exceeded, force compression unconditionally — no cooldown, no cost
	// analysis, no "won't bring below threshold" checks. This is the last line of defense.
	if config.max_session_tokens_threshold > 0
		&& current_tokens >= config.max_session_tokens_threshold
	{
		let ratio = config
			.compression
			.pressure_levels
			.iter()
			.map(|l| l.target_ratio)
			.fold(2.0_f64, f64::max);
		log_debug!(
			"Max session token threshold exceeded ({} >= {}) - FORCE triggering compression with ratio {:.1}x (bypasses all gates)",
			current_tokens,
			config.max_session_tokens_threshold,
			ratio
		);
		return (true, ratio);
	}

	// Check if we have any pressure levels configured
	if config.compression.pressure_levels.is_empty() {
		log_debug!("No pressure levels configured - compression disabled");
		return (false, 2.0);
	}

	log_debug!(
		"Compression check: current_tokens={}, thresholds={:?}",
		current_tokens,
		config
			.compression
			.pressure_levels
			.iter()
			.map(|l| l.threshold)
			.collect::<Vec<_>>()
	);

	// RATIO SELECTION: Find the highest matched pressure level, then escalate
	// upward based on consecutive compressions (without user interaction).
	// Escalation clamps at the last level — never wraps back to a lighter level.
	// This prevents infinite loops when compress-all drops context hard and it
	// grows back to the same threshold repeatedly.
	let num_levels = config.compression.pressure_levels.len();
	if num_levels == 0 {
		log_debug!("No pressure levels configured - compression disabled");
		return (false, 2.0);
	}

	// Find the index of the highest threshold that current_tokens exceeds.
	let matched_idx = config
		.compression
		.pressure_levels
		.iter()
		.enumerate()
		.filter(|(_, l)| current_tokens >= l.threshold)
		.max_by(|(_, a), (_, b)| a.threshold.cmp(&b.threshold))
		.map(|(i, _)| i);

	let (matched_idx, level) = match matched_idx {
		Some(base_idx) => {
			// Escalate UP from the matched level; clamp at the last level so we
			// never de-escalate back to a lighter ratio under sustained pressure.
			let n = session.session.info.consecutive_compressions as usize;
			let escalated_idx = (base_idx + n).min(num_levels - 1);
			(base_idx, &config.compression.pressure_levels[escalated_idx])
		}
		None => {
			log_debug!(
				"No threshold exceeded (current: {}, lowest threshold: {})",
				current_tokens,
				config
					.compression
					.pressure_levels
					.first()
					.map(|l| l.threshold)
					.unwrap_or(0)
			);
			return (false, 2.0);
		}
	};

	// ADAPTIVE COMPRESSION RATIO: Adjust based on session patterns
	let adjusted_ratio = calculate_adaptive_compression_ratio(session, level.target_ratio);

	log_debug!(
		"✓ Threshold exceeded! Context tokens: {} → base compression: {:.1}x → adaptive: {:.1}x (matched threshold: {}, escalated level: {})",
		current_tokens,
		level.target_ratio,
		adjusted_ratio,
		config.compression.pressure_levels[matched_idx].threshold,
		level.threshold
	);

	// EXPONENTIAL COOLDOWN: Each consecutive compression (without a user message)
	// doubles the required token growth before re-compression is allowed.
	// 1st: 10%, 2nd: 20%, 3rd: 40%, 4th+: 80-100%.
	// This prevents futile loops while still allowing compression when context genuinely grows.
	let tokens_after_last = session.session.info.context_tokens_after_last_compression;

	if tokens_after_last > 0 {
		let n = session.session.info.consecutive_compressions;
		// 0.10 * 2^n, capped at 1.0 (i.e. require 100% growth = context must double)
		let growth_factor = (0.10 * 2.0_f64.powi(n as i32)).min(1.0);
		let min_tokens_for_recompression =
			(tokens_after_last as f64 * (1.0 + growth_factor)) as usize;
		if current_tokens < min_tokens_for_recompression {
			let actual_growth_pct =
				((current_tokens as f64 / tokens_after_last as f64 - 1.0) * 100.0) as i32;
			log_debug!(
				"Exponential cooldown active (n={}): need {:.0}% growth, have {}% (current={}, required={}, base={})",
				n,
				growth_factor * 100.0,
				actual_growth_pct,
				current_tokens,
				min_tokens_for_recompression,
				tokens_after_last
			);
			return (false, 2.0);
		}
	}

	log_debug!(
		"Compression cooldown passed: current_tokens={}, tokens_after_last_compression={}, consecutive={}",
		current_tokens,
		tokens_after_last,
		session.session.info.consecutive_compressions
	);

	// CACHE-AWARE DECISION: Calculate if compression is profitable
	let net_benefit =
		calculate_compression_net_benefit(session, config, current_tokens, adjusted_ratio).await;

	if net_benefit > 0.0 {
		// Verify compression will actually reduce context meaningfully
		let (start_idx, end_idx) = match find_compression_range(
			&session.session.messages,
			session.first_prompt_idx,
			false,
		) {
			Ok(range) => range,
			Err(e) => {
				log_debug!("Failed to find compression range: {}", e);
				return (false, 2.0);
			}
		};

		if start_idx >= end_idx {
			log_debug!(
				"Invalid compression range ({} >= {}), setting cooldown to prevent re-analysis loop",
				start_idx,
				end_idx
			);
			session.session.info.context_tokens_after_last_compression = current_tokens;
			return (false, 2.0);
		}

		// Count only start_idx+1..=end_idx — the anchor at start_idx is kept
		let compressible_tokens = match calculate_range_tokens(session, start_idx + 1, end_idx) {
			Ok(tokens) => tokens,
			Err(e) => {
				log_debug!("Failed to calculate range tokens: {}", e);
				return (false, 2.0);
			}
		};

		let estimated_compressed_size = (compressible_tokens as f64 / adjusted_ratio) as u64;
		let estimated_after_compression = (current_tokens as u64)
			.saturating_sub(compressible_tokens)
			.saturating_add(estimated_compressed_size);

		// Use the matched (trigger) threshold for the feasibility check, not the
		// escalated level's threshold. The goal is to drop below the threshold that
		// actually fired — the escalated level may have a higher threshold and would
		// incorrectly pass contexts that compression cannot meaningfully reduce.
		let trigger_threshold = config.compression.pressure_levels[matched_idx].threshold as u64;
		if estimated_after_compression >= trigger_threshold {
			log_debug!(
				"Compression won't bring context below trigger threshold: {} → {} (threshold: {}). Compressible: {} → {}. Setting cooldown.",
				current_tokens,
				estimated_after_compression,
				trigger_threshold,
				compressible_tokens,
				estimated_compressed_size
			);
			session.session.info.context_tokens_after_last_compression = current_tokens;
			return (false, 2.0);
		}

		log_debug!(
			"Cache-aware analysis: Net benefit ${:.5} → COMPRESS (will reduce {} → {} tokens, below threshold {})",
			net_benefit,
			current_tokens,
			estimated_after_compression,
			trigger_threshold
		);
		(true, adjusted_ratio)
	} else {
		log_debug!(
			"Cache-aware analysis: Net benefit ${:.5} → SKIP (would lose money)",
			net_benefit
		);
		(false, 2.0)
	}
}

pub enum CompressionTrigger {
	/// Normal automatic compression — respects thresholds/cooldowns, preserves all active skills.
	Automatic,
	/// `/done` command — bypasses thresholds, preserves only env-loaded skills (OCTOMIND_SKILLS).
	Done,
}

/// Main entry point: check if compression needed and perform if AI decides YES
/// Returns true if compression was performed, false otherwise
pub async fn check_and_compress_conversation(
	session: &mut ChatSession,
	config: &Config,
	operation_rx: tokio::sync::watch::Receiver<bool>,
	trigger: CompressionTrigger,
) -> Result<bool> {
	let (should_check, computed_ratio) = should_check_compression(session, config).await;

	let force = matches!(trigger, CompressionTrigger::Done);

	if !force && !should_check {
		return Ok(false);
	}

	// When max_session_tokens_threshold is exceeded, force compression — AI cannot refuse.
	// This is the user's explicit safety ceiling; the decision model has no veto here.
	let force = force
		|| (config.max_session_tokens_threshold > 0 && {
			let current_tokens = session.get_full_context_tokens(config).await;
			current_tokens >= config.max_session_tokens_threshold
		});

	// When force=true (/done or skill-forget), use fixed level 1 pressure ratio (no adaptive adjustment).
	// Regular automatic compressions use the adaptive ratio from should_check_compression.
	let target_ratio = if force {
		config
			.compression
			.pressure_levels
			.first()
			.map(|l| l.target_ratio)
			.unwrap_or(2.0)
	} else {
		computed_ratio
	};

	// Check for cancellation before starting compression (which involves an API call)
	if *operation_rx.borrow() {
		return Err(anyhow::Error::new(crate::session::cancellation::Cancelled));
	}

	// Show animation immediately to avoid perceived lag during decision/summary call
	let animation_manager = get_animation_manager();
	let current_cost = session.session.info.total_cost;
	let max_threshold = config.max_session_tokens_threshold;

	// UNIFIED TOKEN CALCULATION - Use the single source of truth
	let current_context_tokens = session.get_full_context_tokens(config).await as u64;
	animation_manager
		.start_with_params(current_cost, current_context_tokens, max_threshold)
		.await;

	// Surface the phase on the spinner — compression can take several seconds
	// (decision model + summary call). RAII guard guarantees clear_phase
	// runs on every exit path (success, `return`, or `?` propagation).
	animation_manager
		.set_phase("Compressing conversation…")
		.await;
	struct PhaseGuard<'a>(&'a crate::session::chat::animation_manager::AnimationManager);
	impl Drop for PhaseGuard<'_> {
		fn drop(&mut self) {
			self.0.clear_phase();
		}
	}
	let _phase_guard = PhaseGuard(animation_manager);

	log_debug!("Compression check triggered - asking AI for decision and summary in one call");

	// OPTIMIZATION: Do semantic chunking BEFORE AI call (local, no API cost)
	// This allows us to send context chunks to AI in the same call as decision
	let (start_idx, end_idx) =
		find_compression_range(&session.session.messages, session.first_prompt_idx, force)?;

	// end_idx is already safe from find_compression_range

	if start_idx >= end_idx {
		log_debug!("No messages to compress (range invalid)");
		return Ok(false);
	}

	// SKILL PRESERVATION: skill injections land as user-role messages with
	// content wrapped in <skill name="..."> tags (see add_user_message in
	// skill_auto::load_env_skills and skill::execute_use → inbox). If they
	// fall inside the drain range they get wiped by compression, and the AI
	// loses the domain guidance that was active. Extract them here so
	// apply_compression can re-insert them between the anchor and the summary.
	//
	// When trigger=Done (/done), preserve ONLY env-loaded skills (OCTOMIND_SKILLS).
	// Auto-activated skills are context-dependent and should re-activate if
	// the context still matches after the summary.
	//
	// When trigger=Automatic or SkillForget, preserve all active skills.
	let skill_names_to_preserve: Vec<String> = if matches!(trigger, CompressionTrigger::Done) {
		crate::session::context::current_session_id()
			.map(|sid| crate::session::context::get_env_skills(&sid))
			.unwrap_or_default()
	} else {
		crate::session::context::current_session_id()
			.map(|sid| crate::session::context::get_active_skills(&sid))
			.unwrap_or_default()
	};
	let preserved_skills = collect_preserved_skills(
		&session.session.messages,
		start_idx + 1,
		end_idx,
		&skill_names_to_preserve,
	);

	// COMPRESS-ALL: Extract user messages BEFORE compression.
	// - Last user message → re-injected as raw session message after summary
	// - Last 4 user messages (excluding the appended one) → USER TASKS section in summary
	// No intersection: the appended message is NOT in USER TASKS.
	// Skill messages are filtered out — they're preserved verbatim via
	// preserved_skills and must never show up as "user tasks" or get
	// re-injected as the raw user prompt after the summary.
	let all_user_msgs: Vec<&crate::session::Message> = session.session.messages
		[start_idx + 1..=end_idx]
		.iter()
		.filter(|m| {
			m.role == "user"
				&& !m.content.trim().is_empty()
				&& !crate::mcp::core::skill::is_skill_message(&m.content)
		})
		.collect();

	// Last user message for raw re-injection after summary
	let last_user_message = all_user_msgs.last().cloned().cloned();

	// Last 4 user messages EXCLUDING the appended one → USER TASKS in summary
	let user_tasks_msgs: Vec<String> = {
		let exclude_last = if all_user_msgs.len() > 1 {
			&all_user_msgs[..all_user_msgs.len() - 1]
		} else {
			&[]
		};
		exclude_last
			.iter()
			.rev()
			.take(4)
			.rev()
			.map(|m| {
				let content = m.content.trim();
				if content.len() > 200 {
					format!(
						"{}…",
						&content[..content
							.char_indices()
							.take_while(|&(i, _)| i <= 200)
							.last()
							.map(|(i, _)| i)
							.unwrap_or(200)]
					)
				} else {
					content.to_string()
				}
			})
			.collect()
	};

	// Calculate tokens before compression (all messages that will be removed)
	let tokens_before = calculate_range_tokens(session, start_idx + 1, end_idx)?;

	// Skill messages are preserved verbatim (see preserved_skills above) —
	// exclude them from the AI summarizer input so we don't burn tokens
	// paraphrasing instructions we'll re-inject word-for-word.
	let messages_to_compress: Vec<crate::session::Message> = session.session.messages
		[start_idx + 1..=end_idx]
		.iter()
		.filter(|m| !(m.role == "user" && crate::mcp::core::skill::is_skill_message(&m.content)))
		.cloned()
		.collect();

	// OPTIMIZATION: Single API call for decision + summary (1-hop instead of 2-hop)
	let (should_compress, context_summary) = ask_ai_decision_and_summary(
		session,
		config,
		&messages_to_compress,
		operation_rx,
		force,
		target_ratio,
	)
	.await?;

	if !should_compress {
		log_debug!("AI decided compression not beneficial at this point");
		return Ok(false);
	}

	log_info!("AI decided to compress older conversation exchanges");

	// Apply compression with the summary we got from AI
	apply_compression(
		session,
		start_idx,
		end_idx,
		&context_summary,
		tokens_before,
		current_context_tokens,
		user_tasks_msgs,
		last_user_message,
		preserved_skills,
	)
	.await?;

	// Intermediate learning: extract lessons during auto-compaction if enough user messages.
	// Fire-and-forget — must NOT block compression on a second LLM round-trip.
	if config.learning.enabled {
		let user_msg_count = session
			.session
			.messages
			.iter()
			.filter(|m| m.role == "user")
			.count();
		if user_msg_count >= config.learning.min_messages_for_intermediate {
			let role = crate::config::get_thread_role().unwrap_or_default();
			crate::learning::extract::spawn_lesson_extraction(session, config, role, None);
		}
	}

	if force {
		// /done resets cooldown — treat as fresh session phase boundary.
		session.session.info.consecutive_compressions = 0;
		session.session.info.context_tokens_after_last_compression = 0;
		log_debug!("Forced compression: cooldown counters reset (fresh session phase)");
	} else {
		// EXPONENTIAL COOLDOWN: Increment consecutive compressions counter.
		// Each consecutive compression (without a user message) doubles the required
		// token growth before the next compression is allowed.
		// Resets to 0 on every new user message (see main_loop.rs).
		session.session.info.consecutive_compressions += 1;
		log_debug!(
			"Exponential cooldown: consecutive_compressions now {} (next requires {:.0}% growth)",
			session.session.info.consecutive_compressions,
			(0.10 * 2.0_f64.powi(session.session.info.consecutive_compressions as i32)).min(1.0)
				* 100.0
		);
	}

	// PhaseGuard above clears the phase on drop — no manual call needed.
	Ok(true)
}

#[cfg(test)]
mod tests;
