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
//! - Preserves the active task and exact latest turn boundary for continuity
//! - Reuses existing plan compression infrastructure
//! - Preserves the exact previous-assistant/new-user bridge on fresh user turns

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
// - `ai::prepare_decision` + `ai::run_decision_call` run the LLM round-trip (they build the
//   prompt internally via `prompt::build_compression_prompt`).
// - `apply::{apply_compression, collect_preserved_skills}` materialises the
//   chosen drain range against the session.
// - `decision::{fold_decision, compression_depth, ...}` is the amortization
//   math and the adaptive depth controller driving the should-we-compress gate.
// - `range::{find_compression_range, calculate_range_tokens}` decides which
//   indices to drain and what they cost in tokens.
// Shared with the supervisor: recovery of JSON from a text body when the
// provider does not enforce a response schema.
pub(crate) use ai::extract_json_lenient;
use apply::{apply_compression, collect_preserved_skills, collect_recent_recall_context};
use decision::{
	adaptive_fire_line, at_turn_boundary, autonomous_runway, ceiling_reached, compression_depth,
	context_ceiling, expected_remaining_calls, fold_decision, measured_growth_rate, FoldEconomics,
	MAX_COMPRESSION_RATIO, MIN_COMPRESSION_RATIO,
};
use range::{calculate_range_tokens, find_compression_range_preserving_turn};

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

	// HARD CEILING: unconditional last line of defense, entered one runway
	// margin early. A cooldown may delay a soft fold, but it must never permit
	// an over-window API request. If the fold cannot get below this bound, the
	// caller reports a hard error instead of looping compression or sending an
	// invalid request.
	let ceiling = context_ceiling(session, config);
	let growth = measured_growth_rate(&session.session.info, current_tokens);
	if ceiling_reached(&session.session.info, current_tokens, ceiling) {
		log_debug!(
			"Context ceiling margin reached ({} + {:.0}x{} >= {}) - FORCE triggering deepest compression ({:.0}x)",
			current_tokens,
			growth,
			decision::MIN_RUNWAY_TURNS,
			ceiling,
			MAX_COMPRESSION_RATIO
		);
		return (true, MAX_COMPRESSION_RATIO);
	}

	// ADAPTIVE FIRE LINE: geometric per-turn ladder. Each in-turn fold (or
	// paid decline) doubles the line — threshold, 2x, 4x… capped under the
	// ceiling — so a single long turn earns growing room; a genuine user turn
	// resets it. The runway still paces the amortization gate and fold depth.
	let runway = autonomous_runway(session.session.info.consecutive_compressions);
	let fire_line = adaptive_fire_line(
		config.compression.threshold,
		ceiling,
		session.session.info.context_tokens_after_last_compression,
		growth,
		session.session.info.consecutive_compressions,
	);

	if current_tokens < fire_line && current_tokens < ceiling {
		log_debug!(
			"Below compression fire line (current: {}, fire line: {}, ceiling: {})",
			current_tokens,
			fire_line,
			ceiling
		);
		return (false, MIN_COMPRESSION_RATIO);
	}

	log_debug!(
		"Adaptive compression fire line reached: current={}, fire_line={}, ceiling={}, post={}, growth={:.0}, runway={:.0}",
		current_tokens,
		fire_line,
		ceiling,
		session.session.info.context_tokens_after_last_compression,
		growth,
		runway
	);

	// ADAPTIVE DEPTH: pick the post-compression target from measured dynamics.
	// Pure math over the drain range — no API cost, so it runs before the cost
	// gate and its derived ratio feeds the pricing analysis.
	let (start_idx, end_idx) =
		match find_compression_range_preserving_turn(&session.session.messages, false, true) {
			Ok(range) => range,
			Err(e) => {
				log_debug!("Failed to find compression range: {}", e);
				return (false, MIN_COMPRESSION_RATIO);
			}
		};

	if start_idx >= end_idx {
		log_debug!(
			"Invalid compression range ({} >= {}), skipping compression",
			start_idx,
			end_idx
		);
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

	let Some(adjusted_ratio) = compression_depth(
		current_tokens,
		compressible_tokens,
		fire_line,
		growth,
		runway,
	) else {
		// Even the deepest fold cannot land usefully below the fire line. This is
		// local math (no paid call), so leave the exact compression watermark intact.
		log_debug!(
			"No feasible compression depth (current={}, compressible={}, fire_line={}). Skipping.",
			current_tokens,
			compressible_tokens,
			fire_line
		);
		return (false, MIN_COMPRESSION_RATIO);
	};

	// The fold behind the fire line: free at a genuine turn boundary, otherwise
	// amortized over the calls this session's own pace predicts.
	let compressible = compressible_tokens as f64;
	let target_after = current_tokens as f64 - compressible + compressible / adjusted_ratio;
	// Expected folder output: observed summaries run near the deepest ratio of
	// the drained range (4-10k tokens for 80-180k folds), NOT the configured
	// output cap — costing the fold at the cap overstated it ~3-5x and pinned
	// the mid-turn decision at "wait".
	let summary_cap = config.get_compression_model_profile().max_tokens;
	let summary_tokens = if summary_cap > 0 {
		(compressible / MAX_COMPRESSION_RATIO).min(summary_cap as f64)
	} else {
		compressible / MAX_COMPRESSION_RATIO
	};
	let econ = FoldEconomics::resolve(session, config);
	let fold = fold_decision(
		&session.session.info,
		current_tokens as f64,
		target_after,
		compressible,
		summary_tokens,
		runway,
		econ,
	);
	log_debug!(
		"Fold decision: {} (boundary={}, expected_calls={:.0}, runway={:.0}, current={}, target_after={:.0}, ratio={:.1}x, econ={:?})",
		if fold { "COMPRESS" } else { "WAIT" },
		at_turn_boundary(&session.session.info),
		expected_remaining_calls(&session.session.info),
		runway,
		current_tokens,
		target_after,
		adjusted_ratio,
		econ
	);
	if fold {
		(true, adjusted_ratio)
	} else {
		(false, MIN_COMPRESSION_RATIO)
	}
}

/// Refuse an API call only when the fully materialized context remains above
/// its usable bound after compression. This is the escape hatch for an
/// infeasible fold (for example, an enormous protected current turn): retrying
/// compression would destroy fresh summaries, while sending the request would
/// violate the model window.
pub async fn ensure_context_within_ceiling(
	session: &mut ChatSession,
	config: &Config,
) -> Result<()> {
	let current_tokens = session.get_full_context_tokens(config).await;
	let ceiling = context_ceiling(session, config);
	if current_tokens > ceiling {
		return Err(anyhow::anyhow!(
			"context remains above the usable ceiling after compression ({} > {} tokens); shorten the current request or increase the configured/model context limit",
			current_tokens,
			ceiling
		));
	}
	Ok(())
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

/// A background fold in flight: the paid decision+summary call runs in a
/// spawned task while the agent keeps working. Everything the apply step needs
/// was captured at spawn time; the fingerprint pins the exact drained range so
/// the summary is only ever applied to the messages it was computed from
/// (mid-turn mutations only append, but this makes the invariant checked, not
/// assumed).
pub struct FoldJob {
	handle: tokio::task::JoinHandle<
		Result<(
			schema::CompressionSummary,
			Option<crate::providers::TokenUsage>,
		)>,
	>,
	ctx: FoldContext,
}

struct FoldContext {
	start_idx: usize,
	end_idx: usize,
	fingerprint: u64,
	tokens_before: u64,
	current_context_tokens: u64,
	user_tasks_msgs: Vec<String>,
	last_user_message: Option<crate::session::Message>,
	previous_assistant_response: Option<String>,
	preserved_skills: Vec<crate::session::Message>,
	recalled_context: Vec<String>,
	pact: Option<attention::PactContext>,
	preserve_recent_user_bridge: bool,
	started: std::time::Instant,
}

/// Content identity of the drained range. Excludes mutable presentation state
/// (cache markers) — only what the summary was computed from.
fn fold_fingerprint(messages: &[crate::session::Message], start_idx: usize, end_idx: usize) -> u64 {
	use std::hash::{Hash, Hasher};
	let mut hasher = std::collections::hash_map::DefaultHasher::new();
	(end_idx - start_idx).hash(&mut hasher);
	for message in &messages[start_idx + 1..=end_idx] {
		message.role.hash(&mut hasher);
		message.name.hash(&mut hasher);
		message.tool_call_id.hash(&mut hasher);
		message.content.hash(&mut hasher);
	}
	hasher.finish()
}

/// A background attempt that produced nothing to apply: hold unforced attempts
/// for one runway of calls. Retrying on the very next round turned a slow or
/// broken folder into a session-long crawl (every call paid the failing fold
/// again, ~10 minutes each on a 300s request timeout with one retry).
fn note_fold_failure(session: &mut ChatSession) {
	let runway = autonomous_runway(session.session.info.consecutive_compressions) as usize;
	session.fold_cooldown_until_call = session.session.info.total_api_calls + runway;
	log_info!(
		"Background fold produced nothing to apply — next unforced attempt after {} calls",
		runway
	);
}

/// Join a finished (or force-awaited) background fold and apply it. Returns
/// true when the fold was applied. A failed call or a fingerprint mismatch
/// discards the job and starts the failure cooldown (usage still recorded
/// where known). `force` is the ceiling-margin wait: the summary exists and
/// the window is nearly full, so the decision model's veto no longer applies.
/// `force_done` preserves `/done`'s task-boundary bookkeeping when that command
/// collects a fold which was originally started in the background.
async fn collect_fold_job(
	session: &mut ChatSession,
	config: &Config,
	job: FoldJob,
	force: bool,
	force_done: bool,
) -> Result<bool> {
	let FoldJob { handle, ctx } = job;
	let outcome = match handle.await {
		Ok(outcome) => outcome,
		Err(join_error) => {
			log_debug!("Background fold task failed to join: {}", join_error);
			note_fold_failure(session);
			return Ok(false);
		}
	};
	let (summary, usage) = match outcome {
		Ok(result) => result,
		Err(error) => {
			if crate::session::cancellation::is_cancelled(&error) {
				log_debug!("Background fold cancelled");
			} else {
				log_info!("Background fold call failed, continuing session: {}", error);
			}
			note_fold_failure(session);
			return Ok(false);
		}
	};
	if ctx.end_idx >= session.session.messages.len()
		|| fold_fingerprint(&session.session.messages, ctx.start_idx, ctx.end_idx)
			!= ctx.fingerprint
	{
		ai::record_decision_usage(session, usage.as_ref());
		log_info!(
			"Background fold discarded: the drained range changed while the summary was being written"
		);
		note_fold_failure(session);
		return Ok(false);
	}
	finish_fold(session, config, ctx, summary, usage, force, force_done).await
}

/// Everything after the decision call: usage/metrics accounting, the veto and
/// PACT validation, the drain itself, and the runway bookkeeping. Shared by
/// the inline (forced) path and background collection.
async fn finish_fold(
	session: &mut ChatSession,
	config: &Config,
	mut ctx: FoldContext,
	mut summary: schema::CompressionSummary,
	usage: Option<crate::providers::TokenUsage>,
	force: bool,
	force_done: bool,
) -> Result<bool> {
	ai::record_decision_usage(session, usage.as_ref());
	if let Some(pact) = ctx.pact.as_mut() {
		pact.record_metrics(attention::PactMetrics {
			controller_and_model_latency_ms: ctx.started.elapsed().as_millis() as u64,
			compression_api_time_ms: usage.as_ref().and_then(|u| u.request_time_ms).unwrap_or(0),
			compression_input_tokens: usage.as_ref().map(|u| u.input_tokens).unwrap_or(0),
			compression_output_tokens: usage.as_ref().map(|u| u.output_tokens).unwrap_or(0),
			compression_cost: usage.as_ref().and_then(|u| u.cost).unwrap_or(0.0),
		});
	}
	let should_compress = ai::evaluate_decision(&summary, force, ctx.pact.is_some());

	if !should_compress {
		log_debug!("AI decided compression not beneficial at this point");
		session.session.info.consecutive_compressions += 1;
		return Ok(false);
	}

	let pact_validation = if let Some(pact) = ctx.pact.as_ref() {
		pact.normalize_summary(&mut summary);
		if config.compression.attention.enabled && config.compression.attention.validator {
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
	// Capture learning input before apply_compression replaces the raw turns
	// with one assistant summary. The extraction itself starts only after a
	// successful apply, so a rejected/failed fold never teaches from a state
	// transition that did not happen.
	let learning_snapshot = if !force_done && config.supervisor.learning.enabled {
		let user_msg_count = session
			.session
			.messages
			.iter()
			.filter(|message| crate::session::is_real_user_task_message(message))
			.count();
		(user_msg_count >= crate::supervisor::learning::MIN_MESSAGES_FOR_INTERMEDIATE).then(|| {
			(
				session.session.messages.clone(),
				session.session.info.name.clone(),
				session.learning_outcome,
			)
		})
	} else {
		None
	};

	let preserve_bridge = ctx.preserve_recent_user_bridge
		&& session.session.messages[ctx.end_idx + 1..]
			.iter()
			.any(crate::session::is_real_user_task_message);
	apply_compression(
		session,
		ctx.start_idx,
		ctx.end_idx,
		&summary,
		ctx.tokens_before,
		ctx.current_context_tokens,
		ctx.user_tasks_msgs,
		ctx.last_user_message,
		ctx.previous_assistant_response,
		ctx.preserved_skills,
		ctx.recalled_context,
		config,
		ctx.pact.as_ref(),
		pact_validation.as_ref(),
		force,
		preserve_bridge,
	)
	.await?;

	if let Some((messages, session_name, outcome)) = learning_snapshot {
		let role = crate::config::get_thread_role().unwrap_or_default();
		let _ = crate::supervisor::learning::extract::spawn_lesson_extraction_snapshot(
			messages,
			config,
			role,
			None,
			session_name,
			outcome,
		);
	}

	if force_done {
		session.session.info.consecutive_compressions = 0;
		log_debug!("/done compression: autonomous runway reset for new task phase");
	} else {
		session.session.info.consecutive_compressions += 1;
		log_debug!(
			"Adaptive runway: consecutive_compressions={} (next runway {:.0} calls)",
			session.session.info.consecutive_compressions,
			autonomous_runway(session.session.info.consecutive_compressions)
		);
	}

	Ok(true)
}

/// Turn end with a background fold parked: apply it when it has finished so
/// the persisted state is the compacted one — replace, never auto-continue
/// (no agent call happens here; the summary was already paid for). A fold
/// still running stays parked and is collected at the next round: waiting
/// here charged every turn end the full fold latency (up to 20 minutes
/// measured) for a summary the next turn's boundary check would produce
/// again anyway.
pub async fn settle_pending_fold(session: &mut ChatSession, config: &Config) -> Result<bool> {
	let finished = session
		.fold_job
		.as_ref()
		.is_some_and(|job| job.handle.is_finished());
	if !finished {
		return Ok(false);
	}
	let job = session.fold_job.take().expect("checked above");
	log_debug!("Turn finished with a completed background fold — applying before save");
	collect_fold_job(session, config, job, false, false).await
}

/// Inside the ceiling margin (see `decision::ceiling_reached`): folds are
/// forced and inline here, and a failed one is a hard error for the request —
/// the next rounds would cross the window, and retrying the same failing fold
/// every round is the crawl this replaces.
pub async fn within_ceiling_margin(session: &mut ChatSession, config: &Config) -> bool {
	let current = session.get_full_context_tokens(config).await;
	ceiling_reached(
		&session.session.info,
		current,
		context_ceiling(session, config),
	)
}

pub async fn check_and_compress_conversation(
	session: &mut ChatSession,
	config: &Config,
	operation_rx: tokio::sync::watch::Receiver<bool>,
	trigger: CompressionTrigger,
) -> Result<bool> {
	let force_done = matches!(trigger, CompressionTrigger::Done);

	// A background fold in flight: collect it when finished (or when a forced
	// trigger cannot wait), otherwise leave it running and skip fresh checks —
	// one fold at a time.
	if session.fold_job.is_some() {
		let finished = session
			.fold_job
			.as_ref()
			.is_some_and(|job| job.handle.is_finished());
		// Never run detached NEAR the window: within a few calls of the
		// ceiling, block on the in-flight fold rather than risk the next round
		// overshooting the model window. `ensure_context_within_ceiling` stays
		// the hard error behind this.
		let must_wait = force_done || within_ceiling_margin(session, config).await;
		if finished || must_wait {
			let job = session.fold_job.take().expect("checked above");
			if collect_fold_job(session, config, job, must_wait, force_done).await? {
				return Ok(true);
			}
		// Declined or discarded: fall through — a forced trigger still folds.
		} else {
			return Ok(false);
		}
	}

	let (should_check, computed_ratio) = should_check_compression(session, config).await;

	if !force_done && !should_check {
		return Ok(false);
	}

	// Inside the ceiling margin, force compression — inline, deepest ratio,
	// and the AI cannot refuse. The ceiling is the user's explicit safety
	// limit or the model's physical window, whichever is lower.
	let force = force_done || within_ceiling_margin(session, config).await;

	if !force && session.session.info.total_api_calls < session.fold_cooldown_until_call {
		log_debug!(
			"Fold cooldown after a failed background attempt: {} calls remaining",
			session.fold_cooldown_until_call - session.session.info.total_api_calls
		);
		return Ok(false);
	}

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
	let preserve_recent_user_bridge = !force_done;
	let (start_idx, end_idx) = find_compression_range_preserving_turn(
		&session.session.messages,
		force,
		preserve_recent_user_bridge,
	)?;

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

	// Recall grace window rides the same task-continuity gate as skills: an
	// automatic fold continues the task, so freshly recalled blocks stay live;
	// `/done` is a task boundary and pins nothing.
	let recalled_context = if preserves_active_skills(trigger) {
		collect_recent_recall_context(&session.session.messages, start_idx + 1, end_idx)
	} else {
		Vec::new()
	};

	// COMPRESS-ALL: Extract user messages BEFORE compression.
	//
	// Two paths feed user intent into the post-compression session:
	//   1. USER TASKS section inside the summary text — older real user
	//      messages, full text, never
	//      truncated. The summary becomes input to the next compression
	//      cycle's AI, so untruncated text is what makes intent durable
	//      across multiple compressions.
	//   2. The current turn is either kept structurally as the exact previous
	//      assistant/new-user pair, or carried across a later autonomous fold in
	//      a continuation envelope containing both exact bodies.
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
	let latest_real_user_idx = session
		.session
		.messages
		.iter()
		.rposition(crate::session::is_real_user_task_message);
	let last_user_message: Option<crate::session::Message> = latest_real_user_idx
		.and_then(|idx| session.session.messages.get(idx).cloned())
		.or_else(|| {
			session.session.messages[start_idx + 1..=end_idx]
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
				})
		});
	let previous_assistant_response = latest_real_user_idx
		.and_then(|user_idx| {
			session.session.messages[..user_idx]
				.iter()
				.rev()
				.find(|message| message.role == "assistant")
				.map(|message| message.content.clone())
		})
		.or_else(|| {
			session.session.messages[start_idx + 1..=end_idx]
				.iter()
				.rev()
				.find(|message| {
					message.role == "user" && apply::is_continuation_message(&message.content)
				})
				.and_then(|message| apply::extract_previous_assistant_response(&message.content))
		});

	// USER TASKS: drained real user requests, untruncated. Exclude the latest
	// only when it was drained and will be carried by the continuation envelope;
	// a structurally preserved latest request is outside this list already.
	let user_tasks_msgs: Vec<String> = {
		let latest_user_is_drained = latest_real_user_idx.is_some_and(|idx| idx <= end_idx);
		let exclude_last = if latest_user_is_drained && !all_user_msgs.is_empty() {
			&all_user_msgs[..all_user_msgs.len() - 1]
		} else {
			&all_user_msgs[..]
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
	let pact = if config.compression.attention.enabled
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

	let ctx = FoldContext {
		start_idx,
		end_idx,
		fingerprint: fold_fingerprint(&session.session.messages, start_idx, end_idx),
		tokens_before,
		current_context_tokens,
		user_tasks_msgs,
		last_user_message,
		previous_assistant_response,
		preserved_skills,
		recalled_context,
		pact,
		preserve_recent_user_bridge,
		started: pact_started,
	};

	// Unforced folds run in the background: the paid decision+summary call is
	// the slow part (minutes on big transcripts), and nothing about it needs
	// the live session once the prompt is built. The agent keeps working; the
	// summary is applied at the next round boundary. Forced folds (ceiling,
	// /done) cannot proceed without the result and stay inline.
	if !force {
		let prepared = ai::prepare_decision(
			session,
			config,
			&messages_to_compress,
			ctx.pact.as_ref(),
			false,
			target_ratio,
		)?;
		let config_for_task = config.clone();
		let task_rx = operation_rx.clone();
		let handle = tokio::spawn(async move {
			ai::run_decision_call(
				&config_for_task,
				prepared.system_content,
				prepared.user_content,
				prepared.schema,
				task_rx,
			)
			.await
		});
		session.fold_job = Some(FoldJob { handle, ctx });
		crate::supervisor::notify("compaction started in background");
		log_debug!("Compression decision spawned in background");
		return Ok(false);
	}

	let prepared = ai::prepare_decision(
		session,
		config,
		&messages_to_compress,
		ctx.pact.as_ref(),
		force,
		target_ratio,
	)?;
	let (summary, usage) = ai::run_decision_call(
		config,
		prepared.system_content,
		prepared.user_content,
		prepared.schema,
		operation_rx,
	)
	.await?;
	finish_fold(session, config, ctx, summary, usage, force, force_done).await
}

#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "gate_tests.rs"]
mod gate_tests;

#[cfg(test)]
#[path = "compression_e2e_tests.rs"]
mod compression_e2e_tests;
