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

// Materialise a compression decision against the session: drain the chosen
// range, insert the synthetic summary message (with inherited response_id for
// chain continuity), re-inject the most recent user turn, fold knowledge,
// update anchor + token bookkeeping. Pure side-effects on `ChatSession`.

use super::decision::estimate_future_turns;
use super::knowledge::{
	fold_analysis_findings, fold_critical_knowledge, format_compressed_entry_with_context,
	format_compressed_entry_with_pact,
};
use super::schema::{render_pact_summary, render_summary, CompressionSummary};
use crate::log_debug;
use crate::session::chat::file_context;
use crate::session::chat::session::ChatSession;
use crate::session::estimate_tokens;
use anyhow::Result;

// Continuation-wrapper vocabulary lives in `crate::session` so the builder here
// and every reader of the live task (recall, resolve, verify-gate, recitation)
// agree on one spelling.
use crate::session::{CONTINUATION_FALLBACK_INTENT, CONTINUATION_TAG_OPEN};

/// `Message::name` carried by every compression summary inserted into the
/// conversation — this module's conversation summaries and the task summaries
/// from `mcp/core/plan/compression.rs` alike. Structural, so detection never
/// depends on the rendered body text (which gets prefixed with the
/// earlier-requests and plan sections).
pub(super) const COMPRESSION_MESSAGE_NAME: &str = "plan_compression";

/// True if `content` is a synthetic continuation wrapper inserted by a
/// prior compression cycle (not a real user ask). Mirrors the
/// skill-message detection pattern used elsewhere in the session.
pub(super) fn is_continuation_message(content: &str) -> bool {
	content.trim_start().starts_with(CONTINUATION_TAG_OPEN)
}

/// Recover the `<task>…</task>` intent from a prior continuation wrapper.
///
/// A barren re-compaction (autonomous tool loop, no fresh user message in the
/// drain range) leaves the active task living ONLY inside the previous cycle's
/// continuation wrapper. Since that wrapper is excluded from `all_user_msgs`,
/// without this the intent decays to the anchor/instructions. Extracting it
/// here lets the active task propagate across compactions.
///
/// Returns None when `content` isn't a continuation wrapper, has no `<task>`,
/// or carries only the synthetic fallback placeholder (no real intent).
pub(super) fn extract_continuation_task(content: &str) -> Option<String> {
	crate::session::continuation_task(content).map(str::to_string)
}

/// Select the validated active frontier that the model should resume after
/// PACT compression. The exact user request remains separately preserved in
/// the wrapper for task identity, constraints, and completion verification.
/// A pending/tentative/unknown `next_action` is source-attributed and has
/// survived PACT validation; an established/failed/superseded action is not a
/// live frontier. Legacy compression keeps its existing request-as-task path.
fn select_continuation_action(summary: &CompressionSummary, pact_enabled: bool) -> Option<String> {
	if !pact_enabled {
		return None;
	}

	summary
		.folded_units
		.iter()
		.rev()
		.find(|unit| {
			unit.kind == "next_action"
				&& matches!(unit.status.as_str(), "pending" | "tentative" | "unknown")
				&& !unit.text.trim().is_empty()
		})
		.map(|unit| unit.text.trim().to_string())
}

/// Build the continuation wrapper for the trailing user turn after a
/// compressed summary. `request` is the exact most recent real user message;
/// `action` is the validated frontier the work has already advanced to.
/// Keeping them separate prevents a contextual acknowledgement such as
/// "Should work now" from being replayed as a fresh instruction after the
/// summary correctly recorded that the monitor is already running.
///
/// Shape:
/// ```text
/// <continuation>
/// The conversation summary above is the concise record of prior work;
/// its archive is the lossless record. Resume from where the previous
/// turn left off; read the archive rather than guessing an omitted exact
/// detail.
///
/// {plan continuation note, only when a plan is active}
/// <request>{exact user request}</request>
/// <task>{validated resumption action}</task>
/// </continuation>
/// ```
///
/// `plan_active` adds an explicit "continue the active plan" line — without
/// it, a post-compression model re-entering its plan-first protocol calls
/// plan(start), gets steered to reset, and wipes completed-task history.
fn build_continuation_content(
	request: Option<&str>,
	action: Option<&str>,
	plan_active: bool,
) -> String {
	let task_body = action.or(request).unwrap_or(CONTINUATION_FALLBACK_INTENT);
	let request_block = request
		.map(|request| format!("<request>\n{}\n</request>\n", request.trim()))
		.unwrap_or_default();
	let plan_note = if plan_active {
		"An execution plan is already active (shown in the summary above) — continue its current task; never call plan(start) or plan(reset) to re-create it.\n\n"
	} else {
		""
	};
	format!(
		"<continuation>\n\
		The conversation summary above is the concise record of prior work on this task, and its archive points to the lossless transcript. Resume from where the previous turn left off; do not restart or re-discover what is already established. If an exact detail required for the next action is absent, read the archive before acting; never guess. The <request> block preserves the user's exact turn for identity and may already have been acted on; <task> is the validated frontier to resume now.\n\n\
		{}{}<task>\n{}\n</task>\n\
		</continuation>",
		plan_note, request_block, task_body
	)
}

/// Render the session's live background automation state (scheduled entries
/// and running monitors) for embedding into the compressed summary. Without
/// this, compression drains the tool exchanges that created them and the
/// post-compression model re-schedules/re-starts duplicates. Returns None
/// when nothing is scheduled and no monitor is running.
fn render_background_state() -> Option<String> {
	let mut sections = Vec::new();
	if let Some(schedules) = crate::mcp::orchestration::schedule::core::render_pending_entries() {
		sections.push(schedules);
	}
	if let Some(session_id) = crate::session::context::current_session_id() {
		if let Some(monitors) =
			crate::mcp::orchestration::monitor::render_running_monitors(&session_id)
		{
			sections.push(monitors);
		}
	}
	if sections.is_empty() {
		None
	} else {
		Some(sections.join("\n\n"))
	}
}

/// Rebuild the two rolling content-cache boundaries after every compression
/// mutation and reinjection has finished.
///
/// The first marker stays on the unchanged pre-compression anchor so the
/// provider can reuse the longest stable prefix. The second marker is placed
/// on the final message in the newly compacted state. If the structural anchor
/// is the system message (which uses its own cache slot), the generated summary
/// becomes the first *content* marker so both content slots remain useful.
fn align_compression_cache_markers(
	messages: &mut [crate::session::Message],
	anchor_idx: usize,
	summary_idx: usize,
	supports_caching: bool,
) {
	for message in messages
		.iter_mut()
		.filter(|message| message.role != "system")
	{
		message.cached = false;
		message.cache_ttl = None;
	}

	if !supports_caching || messages.is_empty() {
		return;
	}

	let first_idx = match messages.get(anchor_idx) {
		Some(anchor) if anchor.role != "system" => anchor_idx,
		Some(_) if summary_idx < messages.len() => summary_idx,
		_ => return,
	};

	if let Some(first) = messages.get_mut(first_idx) {
		first.cached = true;
		// Only the unchanged preamble boundary gets the long TTL. A generated
		// summary is new content and follows the normal rolling-cache lifetime.
		if first_idx == anchor_idx {
			first.cache_ttl = Some("1h".to_string());
		}
	}

	let final_idx = messages.len() - 1;
	if final_idx != first_idx {
		if let Some(last) = messages.get_mut(final_idx) {
			last.cached = true;
			last.cache_ttl = None;
		}
	}
}

/// Apply compression: drain all messages, insert summary, re-inject recent user messages.
/// Pulls structured file contexts and critical knowledge directly from the
/// typed summary — no markdown re-parsing.
#[allow(clippy::too_many_arguments)]
pub(super) async fn apply_compression(
	session: &mut ChatSession,
	start_idx: usize,
	end_idx: usize,
	summary: &CompressionSummary,
	tokens_before: u64,
	current_context_tokens: u64,
	user_tasks_msgs: Vec<String>,
	last_user_message: Option<crate::session::Message>,
	preserved_skills: Vec<crate::session::Message>,
	config: &crate::config::Config,
	pact: Option<&super::attention::PactContext>,
	pact_validation: Option<&super::attention::ValidationReport>,
	force: bool,
) -> Result<()> {
	let continuation_request = last_user_message
		.as_ref()
		.map(|message| message.content.trim().to_string())
		.filter(|request| !request.is_empty());
	let continuation_action = select_continuation_action(summary, pact.is_some());
	let continuation_goal = continuation_action
		.as_deref()
		.or(continuation_request.as_deref())
		.unwrap_or_default()
		.to_string();

	// Fidelity snapshot (pre-drain): the authoritative goal + every explicit
	// constraint across real user turns. Compression is lossy; these are what
	// the post-compression view must still entail (checked at the end).
	//
	// Prefer the actual most recent user message (ground truth) over the
	// AI-generated `original_request`, which can drift stale across
	// compressions when the model fails to detect a user pivot.
	let fidelity_goal = resolve_task_intent(
		&last_user_message,
		&summary.original_request,
		&session.session.messages,
	);
	let fidelity_constraints: Vec<String> = {
		let mut seen = std::collections::BTreeSet::new();
		session
			.session
			.messages
			.iter()
			.filter(|m| crate::session::is_real_user_task_message(m))
			.flat_map(|m| crate::supervisor::recite::extract_constraints(&m.content))
			.filter(|c| seen.insert(c.clone()))
			.collect()
	};

	// PACT commit checks run before ANY live session mutation. Governance is
	// recomputed from the still-live transcript, then the full drain is archived
	// and every stable packet ID is dereferenced back to byte-identical messages.
	// Optional compression aborts on either failure; a forced hard-ceiling
	// compression may proceed without recall only when storage itself is the
	// failing component, because retaining the oversized context can deadlock the
	// session. Governance failure is never bypassed.
	if config.compression.attention.governance.enabled
		&& config.compression.attention.governance.verify_hash
	{
		if let Some(pact) = pact {
			pact.verify_governance(&session.session.messages)?;
		}
	}

	let compression_id = crate::mcp::core::plan::compression::get_compression_id()
		.unwrap_or_else(|| "unknown".to_string());
	let (archive_bundle, archive_fallback_reason) = if let Some(pact) = pact {
		let archive_result = {
			let drained = &session.session.messages[start_idx + 1..=end_idx];
			super::archive::archive_messages_with_index(
				&session.session.info.name,
				&compression_id,
				drained,
				&pact.packets,
			)
			.and_then(|bundle| {
				pact.verify_archive(&bundle, drained)?;
				Ok(bundle)
			})
		};
		match archive_result {
			Ok(bundle) => (Some(bundle), None),
			Err(error) if force => {
				let reason = error.to_string();
				crate::log_error!(
					"PACT archive verification failed under forced compression: {} — exact recall is unavailable for this cycle",
					error
				);
				(None, Some(reason))
			}
			Err(error) => {
				return Err(anyhow::anyhow!(
					"PACT archive verification failed before drain; compression aborted: {error}"
				));
			}
		}
	} else {
		(None, None)
	};
	let legacy_archive_path = if pact.is_none() {
		let drained = &session.session.messages[start_idx + 1..=end_idx];
		super::archive::archive_messages(&session.session.info.name, &compression_id, drained)
	} else {
		None
	};

	// Re-point the session anchor at the goal we just resolved. `recite_note`
	// injects `anchor.intent` mid-turn as "Goal (fixed)", so leaving it on an
	// older task makes the supervisor itself steer the model back to work the
	// user has moved on from — the same stale-task failure compaction just
	// fixed, arriving through a different door.
	// Sign it with the request it was resolved from, so recitation stops once the
	// user asks for something else — the goal only outlives the turn, not the ask.
	if !continuation_goal.trim().is_empty() {
		let intent_task_sig =
			crate::session::latest_real_user_task_content(&session.session.messages)
				.map(crate::session::anchor::task_sig);
		session.session.info.anchor.extend(
			crate::session::anchor::AnchorUpdate {
				intent: Some(continuation_goal.clone()),
				intent_task_sig,
				..Default::default()
			},
			crate::utils::time::now_secs(),
		);
	}

	let pact_live = pact.is_some() && config.compression.attention.enabled;
	// Legacy knowledge fields have no source IDs. Once PACT is live, committing
	// them into runtime stores would create an unvalidated authority channel that
	// can outlive the attributed folded units. Existing pre-PACT stores remain
	// available as unverified attention context, but only validated folds may add
	// new model-authored durable state.
	if !pact_live {
		fold_critical_knowledge(session, config, &summary.critical_knowledge);
	}

	// Accumulate findings in CODE, not by asking the model. Measured over 19
	// compactions of one session the model rewrote `analysis_findings` from
	// scratch every cycle despite the carry-forward instruction — one cycle
	// dropped all 9 prior findings and kept 0, deleting the root cause the
	// agent had already established, which it then re-derived 37 times. The
	// model's list is treated as "what I learned this cycle"; the union is
	// authoritative and is what gets rendered.
	let finding_focus = format!(
		"{}\n{}\n{}",
		summary.original_request, summary.current_task, summary.next_steps
	);
	let accumulated_findings = if pact_live {
		Vec::new()
	} else {
		fold_analysis_findings(session, config, &summary.analysis_findings, &finding_focus).await
	};
	let summary = &CompressionSummary {
		analysis_findings: accumulated_findings,
		..summary.clone()
	};

	// Render the typed summary to the markdown body that gets inserted into
	// the session as the compressed turn. Sections appear only when they
	// carry signal so the body stays terse on early or sparse compressions.
	let summary_body = if pact_live {
		render_pact_summary(summary)
	} else {
		render_summary(summary)
	};

	// File context: structured array → tuple form expected by the legacy
	// renderer. Validate line ranges (start <= end, both > 0); drop invalid
	// entries silently rather than failing compression.
	let file_contexts: Vec<(String, usize, usize)> = summary
		.file_context
		.iter()
		.filter(|fc| fc.start_line > 0 && fc.start_line <= fc.end_line)
		.map(|fc| (fc.filepath.clone(), fc.start_line, fc.end_line))
		.collect();

	let file_context_content = if !file_contexts.is_empty() {
		crate::log_debug!(
			"Compression: AI requested {} file context(s) for continuation",
			file_contexts.len()
		);
		for (filepath, start, end) in &file_contexts {
			crate::log_debug!("  - {} (lines {}-{})", filepath, start, end);
		}
		file_context::generate_file_context_content(&file_contexts)
	} else {
		String::new()
	};

	let base_entry = if let Some(pact) = pact {
		format_compressed_entry_with_pact(
			&summary_body,
			&file_context_content,
			compression_id.clone(),
			archive_bundle.as_ref(),
			pact,
		)
	} else {
		format_compressed_entry_with_context(
			&summary_body,
			&file_context_content,
			compression_id.clone(),
			legacy_archive_path.as_deref(),
		)
	};

	// Prepend the earlier-requests section (last 4 user requests, excluding the
	// appended one). These are raw user messages — not AI-rephrased — so intent
	// is never lost. The heading says "earlier" explicitly: an ambiguous "USER
	// TASKS" list reads as a to-do list, and a post-compaction model will pick
	// the first entry and redo finished work.
	let compressed_entry = if user_tasks_msgs.is_empty() {
		base_entry
	} else {
		let user_tasks = user_tasks_msgs
			.iter()
			.enumerate()
			.map(|(i, msg)| format!("{}. {}", i + 1, msg))
			.collect::<Vec<_>>()
			.join("\n");
		format!(
			"## EARLIER USER REQUESTS (history — already superseded, NOT the active task)\n{}\n\n{}",
			user_tasks, base_entry
		)
	};

	// Append the current active plan (if any) to the summary so the model doesn't have
	// to spend an extra `plan(list)` turn right after compression just to recover state.
	// Absence of a plan → no section injected.
	let plan_display = crate::mcp::core::plan::core::get_current_plan_display().await;
	let plan_active = plan_display.is_ok();
	let compressed_entry = match plan_display {
		Ok(plan_display) => format!(
			"{}\n\nCurrent plan we are working on:\n<plan>\n{}\n</plan>",
			compressed_entry,
			plan_display.trim()
		),
		Err(_) => compressed_entry,
	};

	// Append live background state (scheduled entries, running monitors) so the
	// post-compression model knows they already exist and doesn't re-create
	// duplicates. Absence of state → no section injected.
	let compressed_entry = match render_background_state() {
		Some(state) => format!(
			"{}\n\nActive background automation (already running — do NOT schedule or start it again; manage by the IDs shown):\n<background>\n{}\n</background>",
			compressed_entry, state
		),
		None => compressed_entry,
	};

	let tokens_after = estimate_tokens(&compressed_entry) as u64;

	// CRITICAL: Capture the most recent assistant response_id from the range we're
	// about to drain. The Responses API (OpenAI + OctoHub) chains via this id —
	// the server stores prior turns under it and reconstructs full history from
	// the chain. If we drain every id-bearing assistant and leave the summary
	// without one, the next request finds no `previous_id`, falls into the
	// "initial request" branch of `messages_to_input`, which filters out the
	// summary (role=assistant) entirely. The model then receives only the
	// re-injected user turn with zero context — exactly the "lost YES / plan
	// approval" failure mode. Inheriting the id keeps the server-side chain
	// intact while local view shrinks for token budget.
	//
	// The inherited id must point to a SETTLED completion — one whose stored
	// output did not end with `function_call` items. When the server walks the
	// chain back from an unsettled id, the reconstructed history ends with
	// `assistant_with_tool_calls`, and the next request (whose `input` after
	// compression is a re-injected user message, not the matching tool_results)
	// produces `tool_use → user` upstream, which Anthropic rejects with:
	//   "tool_use ids were found without tool_result blocks immediately after".
	// An assistant message with non-empty `tool_calls` corresponds to a
	// completion whose stored output had `function_call` items, so we skip
	// those when scanning the drained range.
	let inherited_response_id: Option<String> = session.session.messages[start_idx + 1..=end_idx]
		.iter()
		.rev()
		.find(|m| {
			m.role == "assistant"
				&& m.id.is_some()
				&& match m.tool_calls.as_ref() {
					Some(serde_json::Value::Array(arr)) => arr.is_empty(),
					Some(_) => false,
					None => true,
				}
		})
		.and_then(|m| m.id.clone());

	if let Some(ref id) = inherited_response_id {
		log_debug!(
			"Compression: inheriting last assistant response_id={} onto summary to preserve chain continuity",
			id
		);
	} else {
		log_debug!(
			"Compression: no assistant response_id found in drained range; summary will start a fresh chain"
		);
	}

	// COMPRESS-ALL: Drain everything from start_idx+1 to end_idx
	let (messages_removed, _) = session.remove_messages_in_range(start_idx, end_idx)?;

	// Insert the post-compression state first. Cache markers are aligned only
	// after every reinjection (including fidelity repair) has finished, so the
	// second boundary really is the end of the current state.
	let supports_caching = crate::session::model_supports_caching(&session.session.info.model);

	let now = std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.unwrap_or_default()
		.as_secs();

	// Insert preserved active skills FIRST, between the anchor and the summary.
	// Skills carry no cache markers — the two-marker budget is reserved for the
	// stable boundary + final compacted state. Order is
	// preserved relative to each other, matching the user's expectation that
	// active skills sit at the top of the recovered context:
	//   [system, anchor(marker#1), skill1, skill2, …, summary, user(marker#2), …]
	let skill_count = preserved_skills.len();
	for (i, mut skill_msg) in preserved_skills.into_iter().enumerate() {
		// Defensive: clear cache markers so we never blow the 2-marker budget.
		skill_msg.cached = false;
		skill_msg.cache_ttl = None;
		session
			.session
			.messages
			.insert(start_idx + 1 + i, skill_msg);
	}
	if skill_count > 0 {
		log_debug!(
			"Compression: preserved {} active skill message(s) across compression",
			skill_count
		);
	}

	// Summary marker placement is finalized after all reinjections below.
	// The `id` is inherited from the most recent assistant turn in the drained range
	// so the provider can chain via `previous_response_id` on the next API call.
	let summary_msg = crate::session::Message {
		role: "assistant".to_string(),
		content: compressed_entry.clone(),
		timestamp: now,
		cached: false,
		name: Some(COMPRESSION_MESSAGE_NAME.to_string()),
		id: inherited_response_id,
		..Default::default()
	};
	session
		.session
		.messages
		.insert(start_idx + 1 + skill_count, summary_msg);

	// Re-injected continuation message. This is ALWAYS a synthetic
	// <continuation> wrapper, never
	// the raw user message verbatim. The wrapper:
	//   - signals to the model that this is an in-progress task (the
	//     summary above captures completed work), preventing "fresh
	//     start" hallucinations after compression;
	//   - preserves the most recent real user request inside <request> for
	//     runtime task identity, while <task> carries the validated active
	//     frontier so the model does not replay an already-handled follow-up;
	//   - is tagged so the next compression cycle's user-msg filter skips
	//     it (see `is_continuation_message`), keeping USER TASKS sourced
	//     only from real user asks and preventing cross-cycle decay.
	//
	// `last_user_message = None` is only possible on a session with no
	// real user message anywhere (pathological bootstrap-only state); the
	// wrapper falls back to pointing at the summary itself.
	let continuation_msg = crate::session::Message {
		role: "user".to_string(),
		content: build_continuation_content(
			continuation_request.as_deref(),
			continuation_action.as_deref(),
			plan_active,
		),
		timestamp: now,
		cached: false,
		..Default::default()
	};
	session
		.session
		.messages
		.insert(start_idx + 2 + skill_count, continuation_msg);
	log_debug!(
		"Inserted continuation wrapper after compressed summary (USER TASKS: {}, intent_source: {})",
		user_tasks_msgs.len(),
		if continuation_action.is_some() {
			"validated_frontier"
		} else if continuation_request.is_some() {
			"last_user_message"
		} else {
			"summary_fallback"
		}
	);

	// Calculate metrics
	let tokens_saved = tokens_before.saturating_sub(tokens_after);

	let metrics = crate::mcp::core::plan::compression::CompressionMetrics::new(
		messages_removed,
		tokens_saved,
		tokens_before,
	);

	crate::session::chat::cost_tracker::CostTracker::display_compression_result(
		"Conversation",
		&metrics,
	);

	// Track stats
	session.session.info.compression_stats.add_compression(
		crate::session::CompressionKind::Conversation,
		messages_removed,
		tokens_saved,
	);

	// Token-based cooldown: record post-compression context size.
	// Next compression is allowed only after context grows ≥10% above this watermark,
	// preventing futile back-to-back compressions while reacting to actual growth.
	let post_compression_tokens = current_context_tokens.saturating_sub(tokens_saved);
	session.session.info.context_tokens_after_last_compression = post_compression_tokens as usize;
	if config.compression.attention.telemetry {
		if let (Some(pact), Some(report)) = (pact, pact_validation) {
			let telemetry_result = if let Some(bundle) = archive_bundle.as_ref() {
				pact.write_telemetry(bundle, report, summary, post_compression_tokens)
			} else {
				pact.write_degraded_telemetry(
					&session.session.info.name,
					&compression_id,
					report,
					summary,
					post_compression_tokens,
					archive_fallback_reason.as_deref(),
				)
			};
			if let Err(error) = telemetry_result {
				crate::log_error!("PACT telemetry write failed: {}", error);
			}
		}
	}

	// SELF-TUNING: Record checkpoint for incremental growth rate tracking.
	// output_tokens_at_last_compression lets estimate_future_turns measure growth since
	// this compression only, not the inflated lifetime average.
	let estimated_future_turns = estimate_future_turns(session, tokens_saved as f64);
	let api_calls_at_compression = session.session.info.total_api_calls;
	session.session.info.predicted_turns_at_last_compression = estimated_future_turns;
	session.session.info.api_calls_at_last_compression = api_calls_at_compression;
	session.session.info.output_tokens_at_last_compression = session.session.info.output_tokens;

	log_debug!(
		"Compression cooldown set: post_compression_tokens={}, consecutive={}, requires ≥{:.0}% growth before next compression",
		post_compression_tokens,
		session.session.info.consecutive_compressions,
		(0.10 * 2.0_f64.powi(session.session.info.consecutive_compressions as i32)).min(1.0) * 100.0
	);

	// Extend the session anchor so conversation compaction contributes to
	// cross-compaction continuity. Heuristic update: record a marker entry
	// with the metrics; subsequent task compactions (which embed the anchor
	// in their compressed-knowledge messages) surface it in context.
	{
		let now_unix = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.unwrap_or_default()
			.as_secs();
		// The anchor intent is DURABLE — it survives every later compaction and
		// feeds the resolver's session context — so it must not latch onto an
		// elliptical turn ("continue", "yes, do it", "should work now"). PACT's
		// attributed next_action is the already-advanced frontier; legacy mode
		// retains its generated current_task fallback.
		let intent_seed = {
			let current = continuation_action
				.as_deref()
				.unwrap_or_else(|| summary.current_task.trim());
			let resolved = if current.is_empty() {
				resolve_task_intent(
					&last_user_message,
					&summary.original_request,
					&session.session.messages,
				)
			} else {
				current.to_string()
			};
			if !resolved.is_empty() {
				Some(resolved)
			} else if session.session.info.anchor.intent.is_empty() {
				Some("Free-form conversation session".to_string())
			} else {
				None // keep existing intent
			}
		};
		// Sign it with the request it was resolved from, so recitation retires the
		// goal once the user asks for something else. Unsigned intents recite
		// forever: this path fired on the late turns of long sequences, and the
		// agent answered "the re-anchored goal is complete … out of scope" to a
		// brand-new instruction, with zero tool calls.
		let intent_task_sig =
			crate::session::latest_real_user_task_content(&session.session.messages)
				.map(crate::session::anchor::task_sig);
		session.session.info.anchor.extend(
			crate::session::anchor::AnchorUpdate {
				intent: intent_seed,
				intent_task_sig,
				changes_made: vec![format!(
					"Conversation compaction: {} messages folded, {} tokens saved",
					messages_removed, tokens_saved
				)],
				..Default::default()
			},
			now_unix,
		);
	}

	// (dedup state is cleared inside `remove_messages_in_range` — see core.rs.)

	// CRITICAL FIX: Reset token tracking for fresh start after compression
	// This prevents token drift and ensures accurate cache/pricing calculations
	// Mirrors the behavior in context_truncation.rs::perform_smart_full_summarization()
	session.session.info.current_non_cached_tokens = 0;
	session.session.info.current_total_tokens = 0;

	// Reset cache checkpoint time
	session.session.info.last_cache_checkpoint_time = std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.unwrap_or_default()
		.as_secs();

	// COMPACTION FIDELITY: one cheap verifier pass — does the surviving view
	// (summary + plan + anchor intent) still entail the pre-compression goal and
	// every explicit constraint? Whatever was lost is re-injected with full
	// authority. Fail-open inside the check; never blocks compression.
	if config.supervisor.enabled && config.supervisor.gate.enabled {
		let compressed_view = format!(
			"{}\n\nANCHOR INTENT: {}",
			compressed_entry, session.session.info.anchor.intent
		);
		let lost = crate::supervisor::fidelity::check_compaction_fidelity(
			config,
			&fidelity_goal,
			&fidelity_constraints,
			&compressed_view,
		)
		.await;
		if !lost.is_empty() {
			let mut note = String::from(
				"<pay-attention>\n<!-- octomind:compaction_fidelity -->\nThe compression just applied dropped standing requirement(s) that still bind the work. They are re-stated here with full authority — treat each as if the user had just repeated it:\n",
			);
			for item in &lost {
				note.push_str(&format!("- {item}\n"));
			}
			note.push_str("</pay-attention>");
			session.add_system_managed_user_message(&note)?;
			crate::supervisor::notify(&format!(
				"compaction dropped {} requirement(s) — re-injected",
				lost.len()
			));
			crate::log_debug!(
				"Compaction fidelity: {} lost requirement(s) re-injected",
				lost.len()
			);
		}
	}

	let summary_idx = start_idx + 1 + skill_count;
	align_compression_cache_markers(
		&mut session.session.messages,
		start_idx,
		summary_idx,
		supports_caching,
	);

	// Persist the final post-compression state only after skill/fidelity
	// reinjection and cache alignment. The loader clears everything before this
	// marker and rebuilds from this exact snapshot.
	let _ = crate::session::logger::log_compression_point(
		&session.session.info.name,
		"conversation",
		messages_removed,
		tokens_saved,
		&session.session.messages,
	);

	Ok(())
}

/// Collect active skill messages from a compression drain range so they can be
/// re-inserted after the summary. Skill messages are user-role entries whose
/// content is wrapped in `<skill name="...">…</skill>` tags.
///
/// Only skills in `active_skill_names` are preserved — a skill the user
/// explicitly forgot (or that was never registered as active) is dropped.
///
/// Duplicate skill names (same skill injected multiple times) are deduped
/// keeping the LAST occurrence in the range, preserving the freshest content.
/// Relative order of distinct skills is preserved (by last-seen position).
pub(super) fn collect_preserved_skills(
	messages: &[crate::session::Message],
	range_start: usize,
	range_end: usize,
	active_skill_names: &[String],
) -> Vec<crate::session::Message> {
	if range_start > range_end || range_end >= messages.len() {
		return Vec::new();
	}

	// Walk the range once, recording the last index per skill name.
	// Using a Vec<(name, idx)> to preserve insertion order of first-seen names
	// while still letting us update the idx to the latest occurrence.
	let mut order: Vec<String> = Vec::new();
	let mut last_idx: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

	for (offset, msg) in messages[range_start..=range_end].iter().enumerate() {
		if msg.role != "user" {
			continue;
		}
		if !crate::mcp::runtime::skill::is_skill_message(&msg.content) {
			continue;
		}
		let name = match crate::mcp::runtime::skill::extract_skill_name(&msg.content) {
			Some(n) => n.to_string(),
			None => continue,
		};
		if !active_skill_names.iter().any(|n| n == &name) {
			continue;
		}
		let idx = range_start + offset;
		if last_idx.insert(name.clone(), idx).is_none() {
			order.push(name);
		}
	}

	order
		.into_iter()
		.filter_map(|name| last_idx.get(&name).map(|&i| messages[i].clone()))
		.collect()
}

/// Resolve the current task intent, preferring ground truth (the actual
/// most recent user message) over the AI-generated `original_request`
/// field, which can drift stale across compressions when the model fails
/// to detect a user pivot.
///
/// Priority: `last_user_message` > `original_request` > latest real user
/// task in surviving messages.
pub(super) fn resolve_task_intent(
	last_user_message: &Option<crate::session::Message>,
	original_request: &str,
	messages: &[crate::session::Message],
) -> String {
	let from_last = last_user_message
		.as_ref()
		.map(|m| m.content.trim().to_string())
		.filter(|s| !s.is_empty());
	from_last
		.or_else(|| {
			let orig = original_request.trim();
			if !orig.is_empty() {
				Some(orig.to_string())
			} else {
				None
			}
		})
		.unwrap_or_else(|| {
			crate::session::latest_real_user_task_content(messages)
				.unwrap_or_default()
				.to_string()
		})
}

#[cfg(test)]
mod apply_tests {
	use super::*;

	fn cache_message(role: &str, content: &str, cached: bool) -> crate::session::Message {
		crate::session::Message {
			role: role.to_string(),
			content: content.to_string(),
			cached,
			cache_ttl: cached.then(|| "stale".to_string()),
			..Default::default()
		}
	}

	fn content_marker_indices(messages: &[crate::session::Message]) -> Vec<usize> {
		messages
			.iter()
			.enumerate()
			.filter(|(_, message)| message.role != "system" && message.cached)
			.map(|(index, _)| index)
			.collect()
	}

	#[test]
	fn compression_markers_keep_anchor_and_end_after_skill_and_fidelity_reinjection() {
		let mut messages = vec![
			cache_message("system", "system", true),
			cache_message("assistant", "unchanged welcome anchor", false),
			cache_message("user", "<skill name=\"rust\">rules</skill>", true),
			cache_message("assistant", "compressed summary", true),
			cache_message("user", "<continuation>resume</continuation>", true),
			cache_message(
				"user",
				"<pay-attention>fidelity repair</pay-attention>",
				false,
			),
		];

		align_compression_cache_markers(&mut messages, 1, 3, true);

		assert_eq!(content_marker_indices(&messages), vec![1, 5]);
		assert_eq!(messages[1].cache_ttl.as_deref(), Some("1h"));
		assert!(
			!messages[2].cached,
			"re-injected skill is between boundaries"
		);
		assert!(
			!messages[3].cached,
			"summary is covered by the final boundary"
		);
		assert!(
			!messages[4].cached,
			"stale pre-reinjection end marker is cleared"
		);
		assert!(messages[5].cached, "final current state gets marker #2");
	}

	#[test]
	fn compression_with_system_anchor_uses_both_content_marker_slots() {
		let mut messages = vec![
			cache_message("system", "system anchor", true),
			cache_message("assistant", "compressed summary", false),
			cache_message("user", "<continuation>resume</continuation>", false),
		];

		align_compression_cache_markers(&mut messages, 0, 1, true);

		assert!(messages[0].cached, "system cache marker remains intact");
		assert_eq!(content_marker_indices(&messages), vec![1, 2]);
		assert_eq!(messages[1].cache_ttl, None, "new summary uses normal TTL");
	}

	#[test]
	fn compression_clears_content_markers_for_non_caching_models() {
		let mut messages = vec![
			cache_message("system", "system", true),
			cache_message("assistant", "anchor", true),
			cache_message("assistant", "summary", true),
			cache_message("user", "continuation", true),
		];

		align_compression_cache_markers(&mut messages, 1, 2, false);

		assert!(content_marker_indices(&messages).is_empty());
		assert!(messages[0].cached, "system marker is managed separately");
	}

	#[test]
	fn continuation_detection_ignores_ordinary_messages() {
		assert!(!is_continuation_message("fix the parser"));
		assert!(!is_continuation_message(""));
		// A mention of the tag mid-message is not a wrapper.
		assert!(!is_continuation_message("talk about <continuation> tags"));

		assert!(is_continuation_message("<continuation>\nbody"));
		// Leading whitespace/newlines still count — the wrapper may be re-indented.
		assert!(is_continuation_message("\n  <continuation>\nbody"));
	}

	#[test]
	fn built_wrapper_round_trips_through_the_extractor() {
		let intent = "add retry logic to the uploader";
		let wrapper = build_continuation_content(Some(intent), None, false);
		assert!(is_continuation_message(&wrapper));
		assert_eq!(extract_continuation_task(&wrapper).as_deref(), Some(intent));
		assert!(!wrapper.contains("execution plan is already active"));

		// With an active plan the wrapper gains the continue-the-plan note and
		// the task must still round-trip through the extractor.
		let wrapper = build_continuation_content(Some(intent), None, true);
		assert!(wrapper.contains("execution plan is already active"));
		assert_eq!(extract_continuation_task(&wrapper).as_deref(), Some(intent));
	}

	#[test]
	fn pact_continuation_separates_contextual_request_from_validated_frontier() {
		let summary = CompressionSummary {
			folded_units: vec![super::super::schema::FoldedUnit {
				text: "Continue monitoring the 50-case benchmark; monitor mon-debabfb8 is already running."
					.to_string(),
				kind: "next_action".to_string(),
				status: "tentative".to_string(),
				refs: vec!["b:frontier".to_string()],
			}],
			..Default::default()
		};
		let action = select_continuation_action(&summary, true);
		let wrapper = build_continuation_content(Some("Should work now"), action.as_deref(), false);

		assert_eq!(
			extract_continuation_task(&wrapper).as_deref(),
			Some("Should work now"),
			"runtime task identity must remain the exact user request"
		);
		assert!(wrapper.contains(
			"<task>\nContinue monitoring the 50-case benchmark; monitor mon-debabfb8 is already running.\n</task>"
		));
		assert!(!wrapper.contains("<task>\nShould work now\n</task>"));
	}

	#[test]
	fn fallback_wrapper_carries_no_extractable_intent() {
		// Without a real user ask the wrapper holds only the placeholder, which
		// must not propagate as if it were the active task.
		let wrapper = build_continuation_content(None, None, false);
		assert!(wrapper.contains(CONTINUATION_FALLBACK_INTENT));
		assert_eq!(extract_continuation_task(&wrapper), None);
	}

	#[test]
	fn extract_returns_none_for_non_wrappers_and_malformed_tags() {
		assert_eq!(extract_continuation_task("plain user message"), None);
		// Wrapper without a task block.
		assert_eq!(extract_continuation_task("<continuation>\nno task"), None);
		// Unclosed task block.
		assert_eq!(
			extract_continuation_task("<continuation>\n<task>\nhalf"),
			None
		);
		// Empty task block.
		assert_eq!(
			extract_continuation_task("<continuation>\n<task></task>"),
			None
		);
	}

	#[test]
	fn extract_trims_and_keeps_multiline_intent() {
		let wrapper =
			"<continuation>\n<task>\n  first line\n  second line  \n</task>\n</continuation>";
		assert_eq!(
			extract_continuation_task(wrapper).as_deref(),
			Some("first line\n  second line")
		);
	}

	#[test]
	fn extract_handles_multibyte_intent_without_panicking() {
		let intent = "почини парсер 日本語";
		let wrapper = build_continuation_content(Some(intent), None, false);
		assert_eq!(extract_continuation_task(&wrapper).as_deref(), Some(intent));
	}
}
