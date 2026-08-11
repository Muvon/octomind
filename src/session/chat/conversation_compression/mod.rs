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
pub(crate) mod archive;
mod attention;
mod decision;
mod knowledge;
mod prompt;
mod range;
mod schema;

// Submodule entrypoints used by this orchestrator file:
// - `ai::ask_ai_decision_and_summary` runs the LLM round-trip (it builds the
//   prompt internally via `prompt::build_compression_prompt`).
// - `apply::{apply_compression, collect_preserved_skills}` materialises the
//   chosen drain range against the session.
// - `decision::{calculate_compression_net_benefit, compression_depth, ...}`
//   is the cost/benefit math and the adaptive depth controller driving the
//   should-we-compress gate.
// - `range::{find_compression_range, calculate_range_tokens}` decides which
//   indices to drain and what they cost in tokens.
use ai::ask_ai_decision_and_summary;
use apply::{apply_compression, collect_preserved_skills};
use decision::{
	calculate_compression_net_benefit, compression_depth, context_ceiling, measured_growth_rate,
	session_runway, MAX_COMPRESSION_RATIO, MIN_COMPRESSION_RATIO, MIN_RUNWAY_TURNS,
};
use range::{calculate_range_tokens, find_compression_range};

use crate::config::Config;
use crate::session::chat::get_animation_manager;
use crate::session::chat::session::ChatSession;
use crate::{log_debug, log_info};
use anyhow::Result;

/// Check if we should ask AI about compression
/// Returns (should_compress, target_ratio) tuple
///
/// ADAPTIVE CONTROLLER: one configured fire line, one physical ceiling; depth
/// is computed per cycle from measured session dynamics (see
/// `decision::compression_depth`) instead of a configured ratio ladder.
///
/// CACHE-AWARE: Uses amortized cost analysis to determine if compression is profitable
/// considering cache invalidation costs vs. future savings over estimated remaining turns
pub async fn should_check_compression(session: &mut ChatSession, config: &Config) -> (bool, f64) {
	// UNIFIED TOKEN CALCULATION - Use the single source of truth
	// This ensures consistency with display and all other systems
	let current_tokens = session.get_full_context_tokens(config).await;

	if config.compression.threshold == 0 {
		log_debug!("Compression disabled (compression.threshold = 0)");
		return (false, MIN_COMPRESSION_RATIO);
	}

	// HARD CEILING: the lower of the user's explicit safety limit and the model's
	// physical window. When exceeded, force the deepest compression unconditionally
	// — no cooldown, no cost analysis, no feasibility checks. Last line of defense.
	let ceiling = context_ceiling(session, config);
	if current_tokens >= ceiling {
		log_debug!(
			"Context ceiling exceeded ({} >= {}) - FORCE triggering deepest compression ({:.0}x, bypasses all gates)",
			current_tokens,
			ceiling,
			MAX_COMPRESSION_RATIO
		);
		return (true, MAX_COMPRESSION_RATIO);
	}

	// FIRE LINE: the configured threshold, pulled down when the model window is
	// small so at least MIN_RUNWAY_TURNS turns of measured growth still fit
	// between the fire line and the ceiling.
	let growth = measured_growth_rate(&session.session.info);
	let fire_line = config
		.compression
		.threshold
		.min(ceiling.saturating_sub((growth * MIN_RUNWAY_TURNS) as usize));

	if current_tokens < fire_line {
		log_debug!(
			"Below compression fire line (current: {}, fire line: {}, ceiling: {})",
			current_tokens,
			fire_line,
			ceiling
		);
		return (false, MIN_COMPRESSION_RATIO);
	}

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
			return (false, MIN_COMPRESSION_RATIO);
		}
	}

	log_debug!(
		"Compression cooldown passed: current_tokens={}, tokens_after_last_compression={}, consecutive={}",
		current_tokens,
		tokens_after_last,
		session.session.info.consecutive_compressions
	);

	// ADAPTIVE DEPTH: pick the post-compression target from measured dynamics.
	// Pure math over the drain range — no API cost, so it runs before the cost
	// gate and its derived ratio feeds the pricing analysis.
	let (start_idx, end_idx) = match find_compression_range(&session.session.messages, false) {
		Ok(range) => range,
		Err(e) => {
			log_debug!("Failed to find compression range: {}", e);
			return (false, MIN_COMPRESSION_RATIO);
		}
	};

	if start_idx >= end_idx {
		log_debug!(
			"Invalid compression range ({} >= {}), setting cooldown to prevent re-analysis loop",
			start_idx,
			end_idx
		);
		session.session.info.context_tokens_after_last_compression = current_tokens;
		return (false, MIN_COMPRESSION_RATIO);
	}

	// Count only start_idx+1..=end_idx — the anchor at start_idx is kept
	let compressible_tokens = match calculate_range_tokens(session, start_idx + 1, end_idx) {
		Ok(tokens) => tokens,
		Err(e) => {
			log_debug!("Failed to calculate range tokens: {}", e);
			return (false, MIN_COMPRESSION_RATIO);
		}
	};

	let runway = session_runway(&session.session.info);
	let Some(adjusted_ratio) = compression_depth(
		current_tokens,
		compressible_tokens,
		fire_line,
		ceiling,
		growth,
		runway,
	) else {
		// Even the deepest fold cannot land usefully below the fire line —
		// re-firing immediately would burn a paid call for nothing. Cooldown.
		log_debug!(
			"No feasible compression depth (current={}, compressible={}, fire_line={}). Setting cooldown.",
			current_tokens,
			compressible_tokens,
			fire_line
		);
		session.session.info.context_tokens_after_last_compression = current_tokens;
		return (false, MIN_COMPRESSION_RATIO);
	};

	// CACHE-AWARE DECISION: Calculate if compression is profitable
	let net_benefit =
		calculate_compression_net_benefit(session, config, current_tokens, adjusted_ratio).await;

	if net_benefit > 0.0 {
		log_debug!(
			"Cache-aware analysis: Net benefit ${:.5} → COMPRESS at {:.1}x",
			net_benefit,
			adjusted_ratio
		);
		(true, adjusted_ratio)
	} else {
		log_debug!(
			"Cache-aware analysis: Net benefit ${:.5} → SKIP (would lose money)",
			net_benefit
		);
		(false, MIN_COMPRESSION_RATIO)
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionTrigger {
	/// Normal automatic compression — respects thresholds/cooldowns, preserves all active skills.
	Automatic,
	/// `/done` command — bypasses thresholds and starts the next task without injected skills.
	Done,
}

fn preserves_active_skills(trigger: CompressionTrigger) -> bool {
	matches!(trigger, CompressionTrigger::Automatic)
}

/// Main entry point: check if compression needed and perform if AI decides YES
/// Returns true if compression was performed, false otherwise
/// True when a USER-role message is one of OUR synthetic injections — a skill
/// block, a continuation wrapper, or a `<pay-attention>`/`<recall>`
/// control-plane note (steers, goal recitation, recalled lessons) — rather than a
/// genuine user request. These must NEVER be summarized or captured as USER TASKS:
/// e.g. a steered loop would otherwise turn "<pay-attention> your results were
/// truncated…" into the recorded task and bury the real ask (the bug that ate the
/// work). Centralized + unit-tested so the filter can't silently drift again.
pub(super) fn is_synthetic_user_message(content: &str) -> bool {
	crate::session::is_system_managed_user_content(content)
}

pub async fn check_and_compress_conversation(
	session: &mut ChatSession,
	config: &Config,
	operation_rx: tokio::sync::watch::Receiver<bool>,
	trigger: CompressionTrigger,
) -> Result<bool> {
	let (should_check, computed_ratio) = should_check_compression(session, config).await;

	let force_done = matches!(trigger, CompressionTrigger::Done);

	if !force_done && !should_check {
		return Ok(false);
	}

	// When the context ceiling is exceeded, force compression — AI cannot refuse.
	// The ceiling is the user's explicit safety limit or the model's physical
	// window, whichever is lower; the decision model has no veto here.
	let force_ceiling = {
		let current_tokens = session.get_full_context_tokens(config).await;
		current_tokens >= context_ceiling(session, config)
	};
	let force = force_done || force_ceiling;

	// /done uses the gentlest fixed ratio: it's a task boundary, so there are no
	// session dynamics to project onto the next task. The hard-ceiling force must
	// NOT fall into that branch: should_check_compression already computed the
	// DEEPEST ratio for the ceiling case, and substituting the gentlest one would
	// under-compress a session that is over the safety limit, looping gentle
	// forced compressions. Regular automatic compressions use the computed depth.
	let target_ratio = if force_done {
		MIN_COMPRESSION_RATIO
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
	let (start_idx, end_idx) = find_compression_range(&session.session.messages, force)?;

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
	// Automatic long-running compression keeps active skills because the same
	// task is continuing. `/done` is a task boundary: preserve no injected
	// skills (including env-loaded ones); normal activation can inject whatever
	// the next task actually needs.
	let skill_names_to_preserve: Vec<String> = if preserves_active_skills(trigger) {
		crate::session::context::current_session_id()
			.map(|sid| crate::session::context::get_active_skills(&sid))
			.unwrap_or_default()
	} else {
		Vec::new()
	};
	let preserved_skills = collect_preserved_skills(
		&session.session.messages,
		start_idx + 1,
		end_idx,
		&skill_names_to_preserve,
	);

	// COMPRESS-ALL: Extract user messages BEFORE compression.
	//
	// Two slots feed user intent into the post-compression session:
	//   1. USER TASKS section inside the summary text — older real user
	//      messages (excluding the most recent one), full text, never
	//      truncated. The summary becomes input to the next compression
	//      cycle's AI, so untruncated text is what makes intent durable
	//      across multiple compressions.
	//   2. Re-injected trailing user message right after the summary — a
	//      synthetic <continuation> wrapper around the most recent real
	//      user intent. Signals to the model that this is an in-progress
	//      task, not a fresh start, and supplies the focus ask.
	//
	// The two slots are disjoint by design: the most recent user msg lives
	// ONLY in slot 2, older ones live ONLY in slot 1. No duplication.
	//
	// Filters excluded from `all_user_msgs`:
	//   - skill messages (`<skill name="…">…</skill>`) — preserved
	//     verbatim via `preserved_skills`, never user intent.
	//   - synthetic continuation messages from prior compression cycles
	//     (`apply::is_continuation_message`) — they're conversation
	//     plumbing, not real user asks. Including them would let the
	//     "Please continue."-style degradation chain reappear.
	let user_msg_filter =
		|m: &&crate::session::Message| -> bool { crate::session::is_real_user_task_message(m) };

	let all_user_msgs: Vec<&crate::session::Message> = session.session.messages
		[start_idx + 1..=end_idx]
		.iter()
		.filter(user_msg_filter)
		.collect();

	// FALLBACK: the drained range has no fresh real user message (e.g. a long
	// autonomous tool loop, or a barren re-compaction after the last user ask
	// was already folded into a continuation wrapper). Recover intent in order:
	//   1. The most recent prior <continuation> wrapper's <task> — this is where
	//      the active ask lives after it's been compacted once. Without this the
	//      task DECAYS to the anchor and the model snaps back to the original
	//      request across repeated compactions.
	//   2. The most recent real user message in the surviving prefix
	//      [..=start_idx] (covers a single-turn loop where the anchor IS the
	//      user message).
	let last_user_message: Option<crate::session::Message> = match all_user_msgs.last() {
		Some(m) => Some((*m).clone()),
		None => session.session.messages[start_idx + 1..=end_idx]
			.iter()
			.rev()
			.find(|m| m.role == "user" && apply::is_continuation_message(&m.content))
			.and_then(|m| apply::extract_continuation_task(&m.content))
			.map(|task| crate::session::Message {
				role: "user".to_string(),
				content: task,
				..Default::default()
			})
			.or_else(|| {
				session.session.messages[..=start_idx]
					.iter()
					.rev()
					.find(user_msg_filter)
					.cloned()
			}),
	};

	// USER TASKS: older real user requests, untruncated. The most recent
	// one is excluded — it lives in the continuation wrapper (slot 2)
	// instead. Take up to 4 to bound summary growth while preserving
	// recent task history.
	let user_tasks_msgs: Vec<String> = {
		let exclude_last = if all_user_msgs.len() > 1 {
			&all_user_msgs[..all_user_msgs.len() - 1]
		} else {
			&[][..]
		};
		exclude_last
			.iter()
			.rev()
			.take(4)
			.rev()
			.map(|m| m.content.trim().to_string())
			.collect()
	};

	// Calculate tokens before compression (all messages that will be removed)
	let tokens_before = calculate_range_tokens(session, start_idx + 1, end_idx)?;

	// Skill messages are preserved verbatim (see preserved_skills above) —
	// exclude them from the AI summarizer input so we don't burn tokens
	// paraphrasing instructions we'll re-inject word-for-word.
	//
	// Continuation wrappers from prior compression cycles are also excluded:
	// they're synthetic plumbing, not real user content. The real intent
	// they wrap is already captured in the prior summary's USER TASKS (which
	// IS in the drained range as an assistant message), so dropping the
	// wrapper avoids confusing the summarizer with meta-instructions and
	// prevents recursive "continuation of continuation" phrasing in the
	// new summary text.
	let messages_to_compress: Vec<crate::session::Message> = session.session.messages
		[start_idx + 1..=end_idx]
		.iter()
		.filter(|m| !(m.role == "user" && is_synthetic_user_message(&m.content)))
		.cloned()
		.collect();

	// PACT is built from the exact drain slice, including system-managed runtime
	// events. Those events must remain visible as low-authority triggers without
	// ever being mistaken for the genuine user task. Skills/instructions are
	// excluded structurally by the packet builder and preserved through their
	// existing dedicated paths.
	let pact_started = std::time::Instant::now();
	let compression_stats_before = session.session.info.compression_stats.clone();
	let mut pact = if config.compression.attention.enabled
		|| config.compression.attention.governance.enabled
	{
		Some(
			attention::build(
				session,
				start_idx + 1,
				end_idx,
				target_ratio,
				config.compression.attention.enabled,
				force_done,
			)
			.await?,
		)
	} else {
		None
	};

	// `analysis_findings` is runtime state, while the rendered summary is what
	// survives on disk. Rebuild the store deterministically on resume before the
	// prior summary is stripped from the compressor prompt. Normal live sessions
	// retain the store across user follow-ups, so this branch is resume-only in
	// practice.
	if session.analysis_findings.is_empty() {
		let restored = knowledge::latest_analysis_findings(&session.session.messages);
		if !restored.is_empty() {
			crate::log_debug!(
				"Compression: restored {} analysis findings from latest summary",
				restored.len()
			);
			session.analysis_findings = restored;
		}
	}

	// OPTIMIZATION: Single API call for decision + summary (1-hop instead of 2-hop)
	// Response is schema-validated and arrives as a typed struct.
	let (should_compress, mut summary) = ask_ai_decision_and_summary(
		session,
		config,
		&messages_to_compress,
		config
			.compression
			.attention
			.enabled
			.then_some(())
			.and(pact.as_ref()),
		operation_rx,
		force,
		target_ratio,
	)
	.await?;
	if let Some(pact) = pact.as_mut() {
		let after = &session.session.info.compression_stats;
		pact.record_metrics(attention::PactMetrics {
			controller_and_model_latency_ms: pact_started.elapsed().as_millis() as u64,
			compression_api_time_ms: after
				.api_time_ms
				.saturating_sub(compression_stats_before.api_time_ms),
			compression_input_tokens: after
				.input_tokens
				.saturating_sub(compression_stats_before.input_tokens),
			compression_output_tokens: after
				.output_tokens
				.saturating_sub(compression_stats_before.output_tokens),
			compression_cost: (after.cost - compression_stats_before.cost).max(0.0),
		});
	}

	if !should_compress {
		log_debug!("AI decided compression not beneficial at this point");
		return Ok(false);
	}

	let pact_validation = if let Some(pact) = pact.as_ref() {
		pact.normalize_summary(&mut summary);
		if config.compression.attention.enabled && config.compression.attention.validator {
			// Deterministic repair first: the generative fold is already paid
			// for, so mechanical contract violations (archive-descriptor refs,
			// frontier folded as completed, skipped summarize packets) are
			// fixed instead of rejected. validate_summary stays the strict gate.
			pact.repair_summary(&mut summary);
			match pact.validate_summary(&summary) {
				Ok(report) => Some(report),
				Err(error) if force => {
					let fallback_reason = error.to_string();
					crate::log_error!(
						"PACT validation failed under forced compression: {} — using deterministic pins/frontier and dropping invalid folds",
						error
					);
					pact.sanitize_for_forced_compression(&mut summary);
					let post_fallback = pact.validate_summary(&summary).ok();
					Some(attention::ValidationReport {
						attribution_valid: false,
						fallback_reason: Some(fallback_reason),
						valid_units: post_fallback
							.as_ref()
							.map(|report| report.valid_units)
							.unwrap_or(0),
						referenced_blocks: post_fallback
							.as_ref()
							.map(|report| report.referenced_blocks)
							.unwrap_or(0),
						governance_hash: pact.pinned.governance_hash.clone(),
					})
				}
				Err(error) => {
					log_info!(
						"Compression rejected before drain: PACT attribution/continuity validation failed: {}",
						error
					);
					// COOLDOWN ON REJECTION: without this, the pressure check
					// refires the full (paid) compression call on every turn
					// and gets rejected again — a token-burning loop. Record
					// the current context as the cooldown baseline so the next
					// attempt waits for real growth, exactly like the other
					// skip paths in should_check_compression.
					session.session.info.context_tokens_after_last_compression =
						current_context_tokens as usize;
					session.session.info.consecutive_compressions += 1;
					return Ok(false);
				}
			}
		} else {
			Some(attention::ValidationReport {
				attribution_valid: !config.compression.attention.enabled,
				fallback_reason: config
					.compression
					.attention
					.enabled
					.then(|| "attribution validator disabled by configuration".to_string()),
				valid_units: summary.folded_units.len(),
				referenced_blocks: 0,
				governance_hash: pact.pinned.governance_hash.clone(),
			})
		}
	} else {
		None
	};

	log_info!("AI decided to compress older conversation exchanges");

	// Apply compression with the typed summary
	apply_compression(
		session,
		start_idx,
		end_idx,
		&summary,
		tokens_before,
		current_context_tokens,
		user_tasks_msgs,
		last_user_message,
		preserved_skills,
		config,
		pact.as_ref(),
		pact_validation.as_ref(),
		force,
	)
	.await?;

	// Intermediate learning: extract lessons during auto-compaction if enough user messages.
	// Fire-and-forget — must NOT block compression on a second LLM round-trip.
	if config.supervisor.learning.enabled {
		let user_msg_count = session
			.session
			.messages
			.iter()
			.filter(|m| crate::session::is_real_user_task_message(m))
			.count();
		if user_msg_count >= config.supervisor.learning.min_messages_for_intermediate {
			let role = crate::config::get_thread_role().unwrap_or_default();
			// Mid-session: the process keeps living, dropping the handle is safe.
			let _ = crate::supervisor::learning::extract::spawn_lesson_extraction(
				session, config, role, None,
			);
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
