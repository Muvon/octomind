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

use super::super::response::{process_response, ResponseProcessingParams};
use super::super::CostTracker;
use super::core::ChatSession;
use super::error_utils::display_rate_limit_info;
use crate::config::Config;
use crate::session::chat_completion_with_validation;
use crate::session::ChatCompletionWithValidationParams;
use anyhow::Result;
use colored::*;
use tokio::sync::watch;

use crate::session::output::{OutputMode, OutputSink};

const PREGATE_MARKER: &str = "octomind:pre_gate_unverified_mutation";

const CONTINUE_NOTE: &str = "<pay-attention>\n<!-- octomind:pre_gate_unfinished_handback -->\nYour last message ended the turn while your own status was still in progress and no action was taken \u{2014} that is a promise, not a result. Continue the work now. When it is genuinely finished, report done; if you cannot proceed, report blocked or need_input with the reason.\n</pay-attention>";
const PREGATE_NOTE: &str = "<pay-attention>\n<!-- octomind:pre_gate_unverified_mutation -->\nYou may only report done after a verification has actually passed. You reported done with state changes still unverified, so that claim isn't trustworthy yet. Run the check appropriate to this work (for example, inspect the resulting state, exercise the changed behavior, or use a domain-specific validator), watch the result, and report the actual outcome: pass, fail, or — if no meaningful check exists — what you inspected and why that is sufficient. Base the report on the observed result, not on what you expect.\n</pay-attention>";

fn latest_real_user_turn_start(messages: &[crate::session::Message]) -> usize {
	crate::session::latest_task_turn_index(messages).unwrap_or(messages.len())
}

fn claims_user_task_completion(
	completion_gate_eligible: bool,
	self_report: Option<crate::supervisor::detect::SelfReport>,
	has_mutations: bool,
) -> bool {
	completion_gate_eligible
		&& (self_report == Some(crate::supervisor::detect::SelfReport::Done)
			|| (self_report.is_none() && has_mutations))
}

/// Read the admission-time task resolution used by both verifier-backed and
/// trusted no-gate completion. A missing/mismatched cache stays conservative,
/// so an unrelated `done` cannot retire an older plan merely because it is open.
fn completion_task(
	chat_session: &ChatSession,
	task: &str,
	live_plan: &str,
) -> crate::supervisor::resolve::ResolvedTask {
	match chat_session.gate_task.clone() {
		Some(resolved) if resolved.original_request == task => resolved,
		_ => {
			// Missing/mismatched state is uncertain. Treat the live plan as
			// pre-existing so it cannot gain authority from missing metadata.
			let mut resolved =
				crate::supervisor::resolve::ResolvedTask::self_contained(task.to_string());
			resolved.plan_at_turn_start = live_plan.to_string();
			resolved
		}
	}
}

/// Separator between the parts of a re-run turn's answer. Self-describing so the
/// verifier reads the parts as one deliverable (see the gate prompt).
const ANSWER_PART_SEPARATOR: &str = "\n\n--- (continued after supervisor feedback) ---\n\n";

/// The turn's answer: the turn-answer ledger's finals — one per supervisor
/// re-run pass, oldest first. First-class session state, never a context-window
/// query: mid-turn compression rewrites the live message list, and the judged
/// deliverable must not shrink because the context was compacted.
///
/// The gate must judge these as ONE deliverable. A re-run triggered by a narrow
/// correction gets a narrow reply ("the link is grounded, the report stands"),
/// because that is what the advisory asked for; judging only that last message
/// throws away the actual deliverable and fails a correct turn for "not
/// delivering" what an earlier part of the same turn already delivered.
///
/// `max_tokens` is the gate's configured budget (`supervisor.gate.max_tokens`).
fn current_turn_answer(turn_answers: &[String], max_tokens: usize) -> String {
	// Fill the budget newest-first (an amendment is the most recent state), then
	// restore chronological order so "later parts amend earlier ones" holds.
	let mut kept: Vec<&str> = Vec::new();
	let mut used = 0usize;
	for part in turn_answers.iter().rev() {
		let tokens = crate::session::estimate_tokens(part);
		if !kept.is_empty() && used + tokens > max_tokens {
			break;
		}
		used += tokens;
		kept.push(part.as_str());
	}
	kept.reverse();
	crate::session::truncate_to_tokens(&kept.join(ANSWER_PART_SEPARATOR), max_tokens)
}

/// Apply the verify-gate's verdict only to active-pack entries the specialist
/// reported materially using. Exposure alone earns no positive or negative
/// credit. Clears the pack references and used-ID set either way.
async fn reinforce_recalled(chat_session: &mut ChatSession, config: &Config, delta: f64) {
	let refs = std::mem::take(&mut chat_session.recalled_refs);
	let used = std::mem::take(&mut chat_session.used_memory_ids);
	if refs.is_empty() || used.is_empty() {
		return;
	}
	let backend = crate::supervisor::learning::backend::create_backend(&config.supervisor.learning);
	for (id, content, role, project) in &refs {
		if !used.contains(id) {
			continue;
		}
		let applied = backend
			.reinforce(content, role, project, delta, config)
			.await;
		if applied.is_ok() && delta != 0.0 {
			crate::supervisor::stats::memory_credit(delta > 0.0);
		}
	}
}

// Helper function to execute API call and process response
pub async fn execute_api_call_and_process_response<S: OutputSink>(
	chat_session: &mut ChatSession,
	config: &Config,
	role: &str,
	operation_rx: watch::Receiver<bool>,
	mode: OutputMode,
	sink: S,
) -> Result<()> {
	let model = chat_session.model.clone();
	let temperature = chat_session.temperature;
	let config_clone = config.clone();

	// Calculate animation parameters
	let current_cost = chat_session.session.info.total_cost;
	let max_threshold = config.max_session_tokens_threshold;
	let current_context_tokens = chat_session.get_full_context_tokens(config).await as u64;

	// Clone operation_rx for response processing
	let operation_rx_for_response = operation_rx.clone();

	// CRITICAL FIX: Check spending threshold BEFORE starting animation
	// This prevents animation from covering the Y/N prompt
	if mode.is_interactive() {
		match chat_session.check_spending_threshold(config) {
			Ok(should_continue) => {
				if !should_continue {
					// User chose not to continue due to spending threshold
					return Ok(());
				}
			}
			Err(e) => {
				// Error checking threshold, log and continue
				println!(
					"{}: {}",
					"Warning: Error checking spending threshold".bright_yellow(),
					e
				);
			}
		}

		// Check request spending threshold
		match chat_session.check_request_spending_threshold(config) {
			Ok(should_continue) => {
				if !should_continue {
					// Request spending threshold exceeded - stop execution
					return Ok(());
				}
			}
			Err(e) => {
				// Error checking request threshold, log and continue
				println!(
					"{}: {}",
					"Warning: Error checking request spending threshold".bright_yellow(),
					e
				);
			}
		}
	}

	// Update animation state with current cost/context values
	// Animation was already started early in main_loop to cover pre-processing
	use crate::session::chat::get_animation_manager;
	let animation_manager = get_animation_manager();
	let anim_state = animation_manager.get_state();
	anim_state.update_cost(current_cost);
	anim_state.update_context_tokens(current_context_tokens);
	anim_state.update_max_threshold(max_threshold);

	// CRITICAL: Connect session cancellation to animation for INSTANT Ctrl+C response
	animation_manager.set_cancel_receiver(operation_rx.clone());

	// Build the runtime-only active memory pack. Two triggers:
	//   - first call of the session → full hybrid scoped recall;
	//   - a new user message (pending_recall) → embedding-only scoped recall.
	// Tool follow-up rounds keep the same pack without retrieving again.
	if !config.supervisor.learning.enabled && chat_session.active_memory_pack.is_some() {
		chat_session.clear_active_memory_pack();
	}
	let mut memory_pack_rebuilt = false;
	if config.supervisor.learning.enabled
		&& (!chat_session.learning_injected || chat_session.pending_recall)
	{
		let first_call = !chat_session.learning_injected;
		chat_session.learning_injected = true;
		chat_session.pending_recall = false;
		crate::log_debug!("Learning injection triggered (first_call={})", first_call);
		let current_dir = crate::mcp::get_thread_working_directory();
		let project = current_dir
			.file_name()
			.and_then(|n| n.to_str())
			.unwrap_or("unknown")
			.to_string();
		// Most recent user message drives query-based scoped retrieval.
		let user_input =
			crate::session::latest_real_user_task_content(&chat_session.session.messages)
				.unwrap_or_default()
				.to_string();
		animation_manager.set_phase("Recalling lessons …").await;
		let (block, selected) = crate::supervisor::learning::inject::retrieve_and_format(
			config,
			&user_input,
			role,
			&project,
			first_call,
			operation_rx.clone(),
		)
		.await;
		animation_manager.clear_phase();
		chat_session.recalled_refs = selected
			.iter()
			.map(|memory| {
				(
					memory.id.clone(),
					memory.content.clone(),
					role.to_string(),
					project.clone(),
				)
			})
			.collect();
		chat_session.set_active_memory_pack((!block.is_empty()).then_some(block));
		memory_pack_rebuilt = true;
	}

	// Supervisor: inject any queued steer note (advisory re-anchor) at the safe
	// pre-request point — same message-ordering guarantees as recall above.
	if let Some(note) = chat_session.steer_pending.take() {
		chat_session.add_system_managed_user_message(&note)?;
		crate::log_debug!("Supervisor steer injected");
	}

	// Supervisor: goal and execution-boundary recitation. Once the session has
	// compacted, the durable goal lives only in the mid-transcript summary, where
	// attention is weak. Re-emit a tiny goal block here — at the tail, in the
	// recency window — and crucially BEFORE the cache-marker advance below, so the
	// cached prefix stays intact (the recited block lands after it each turn).
	let effective_verification_policy = chat_session.session.info.verification_policy.effective(
		chat_session
			.gate_task
			.as_ref()
			.is_some_and(|task| task.forbids_verification),
	);
	if config.supervisor.enabled {
		// Prefer the live plan checklist (refreshed every turn from plan storage)
		// over the anchor's stale next_steps snapshot for the recency-slot block.
		// A plan untouched since before the latest user message carries a
		// staleness marker: re-anchoring the model on a plan the user may have
		// superseded is how a session ends up chasing a dead step.
		let plan_checklist = crate::mcp::core::plan::render_plan_checklist_with_staleness(
			crate::session::latest_task_timestamp(&chat_session.session.messages),
		);
		// The live request. Its prohibitions are recited verbatim (models abandon
		// those first as attention decays), and its signature decides whether the
		// anchor's goal is still the one being worked on — the user may have asked
		// for something else since the last compaction wrote that goal.
		let live_task =
			crate::session::latest_real_user_task_content(&chat_session.session.messages);
		let constraints = crate::supervisor::recite::active_constraints(
			&chat_session.session.messages,
			chat_session
				.gate_task
				.as_ref()
				.map(|task| task.resolved_request.as_str()),
		);
		if let Some(note) = crate::supervisor::recite::recite_note(
			&chat_session.session.info.anchor,
			plan_checklist.as_deref(),
			&constraints,
			live_task.map(crate::session::anchor::task_sig),
			effective_verification_policy,
		) {
			chat_session.add_system_managed_user_message(&note)?;
			crate::log_debug!("Supervisor recitation injected");
		}
	}

	// Supervisor: falsifiable plan commitments. A plan task can declare a
	// machine-checkable `valid_if` condition (the assumption its approach rests
	// on). Re-checked cheaply each turn; a broken condition means the plan is
	// drifting on a dead assumption — steer the agent to revise it. Dedup by a
	// marker keyed on the broken set so the same break is flagged once, but a
	// NEW break still lands.
	if config.supervisor.enabled {
		let broken = crate::mcp::core::plan::broken_plan_conditions();
		if !broken.is_empty() {
			let key = broken
				.iter()
				.map(|(n, _, c)| format!("{n}:{c}"))
				.collect::<Vec<_>>()
				.join("|");
			let marker = format!("<!-- octomind:plan_condition_broken:{} -->", {
				use std::hash::{Hash, Hasher};
				let mut h = std::collections::hash_map::DefaultHasher::new();
				key.hash(&mut h);
				h.finish()
			});
			let already_flagged = chat_session
				.session
				.messages
				.iter()
				.any(|m| m.content.contains(&marker));
			if !already_flagged {
				let mut note = format!("<pay-attention>\n{marker}\nA runtime-checked plan assumption is no longer true. The external plan manager will reassess the unfinished route before work continues. Broken condition(s):\n");
				for (n, title, cond) in &broken {
					let title = crate::supervisor::escape_xml_text(title);
					let cond = crate::supervisor::escape_xml_text(cond);
					note.push_str(&format!("- task {n} \"{title}\": valid if {cond}\n"));
				}
				note.push_str("</pay-attention>");
				chat_session.add_system_managed_user_message(&note)?;
				chat_session.pending_plan_signal =
					Some(crate::supervisor::plan::PlanSignal::Reassess);
				crate::supervisor::notify(&format!(
					"{} broken plan condition(s) — plan revision steered",
					broken.len()
				));
				crate::log_debug!("Plan condition steer: {} broken condition(s)", broken.len());
			}
		}
	}

	// Signals produced without a tool-result boundary (notably a broken-plan
	// assumption detected above) are reconciled here, still before the normal
	// specialist request. No signal means no planner call.
	if config.supervisor.enabled && chat_session.pending_plan_signal.is_some() {
		animation_manager.set_phase("Reconciling plan …").await;
		if let Err(error) = crate::supervisor::plan::reconcile_after_actions(
			chat_session,
			config,
			operation_rx.clone(),
		)
		.await
		{
			crate::log_debug!("External plan reconciliation failed: {}", error);
		}
		animation_manager.clear_phase();
	}

	// The pack is optional context. If it is the only reason the fully assembled
	// request crosses the model's usable ceiling, drop it rather than blocking the
	// user's task; then re-check so unrelated injected state still fails closed.
	if chat_session.active_memory_pack.is_some()
		&& crate::session::chat::conversation_compression::ensure_context_within_ceiling(
			chat_session,
			config,
		)
		.await
		.is_err()
	{
		crate::log_debug!("Active memory pack dropped: insufficient context headroom");
		chat_session.clear_active_memory_pack();
	}
	crate::session::chat::conversation_compression::ensure_context_within_ceiling(
		chat_session,
		config,
	)
	.await?;
	if memory_pack_rebuilt {
		if let Some(pack) = chat_session.active_memory_pack.as_deref() {
			let items = chat_session.recalled_refs.len() as u64;
			crate::supervisor::stats::recall();
			crate::supervisor::stats::memory_pack(
				items,
				crate::session::estimate_tokens(pack) as u64,
			);
			crate::supervisor::notify(&format!("active memory pack: {items} item(s)"));
		}
	}

	// Advance Anthropic-style content cache markers after persistent pre-call
	// injections (inbox hints, steers, etc.) and before the runtime-only memory pack.
	// This preserves the previous marker while moving the oldest marker to the latest
	// user/tool boundary for this new request.
	let cache_manager = crate::session::cache::CacheManager::new();
	let supports_caching = crate::session::model_supports_caching(&model);
	if let Err(e) = cache_manager.check_and_apply_auto_cache_threshold(
		&mut chat_session.session,
		config,
		supports_caching,
		role,
	) {
		crate::log_debug!("pre-request cache marker advance failed: {}", e);
	}
	// Materialize the active pack only for the provider request. It must never
	// enter persistence, compression, extraction, or later conversation history.
	chat_session.ensure_active_memory_pack_message();

	// Make API call. `session.messages` is borrowed directly — no clone — and
	// the validation params hold that shared borrow only until they're consumed
	// by `chat_completion_with_validation` below.
	let max_retries = chat_session.max_retries;
	let schema = chat_session.schema.clone();
	let reasoning_effort = chat_session.reasoning_effort;
	let validation_params = ChatCompletionWithValidationParams::new(
		&chat_session.session.messages,
		&model,
		temperature,
		chat_session.top_p,
		chat_session.top_k,
		chat_session.max_tokens,
		&config_clone,
	)
	.with_max_retries(max_retries)
	.with_full_context_tokens(true)
	.with_cancellation_token(operation_rx.clone());
	let validation_params = if let Some(schema) = schema {
		validation_params.with_schema(schema)
	} else {
		validation_params
	};
	let validation_params = if let Some(effort) = reasoning_effort {
		validation_params.with_reasoning_effort(effort)
	} else {
		validation_params
	};
	let api_result = chat_completion_with_validation(validation_params).await;
	chat_session.remove_active_memory_pack_message();

	// DON'T stop animation here - process_response stops it before tool output.
	// After the tool header is printed, response.rs restarts the animation so it
	// runs during tool execution, giving the user progress feedback.

	// CRITICAL FIX: Check for cancellation after API call completion
	// This prevents the race condition where Ctrl+C is pressed after API completes
	// but before response processing begins
	if *operation_rx_for_response.borrow() {
		crate::log_debug!("Operation cancelled by user.");
		return Ok(()); // Return gracefully to main loop instead of force exit
	}

	// Process response
	match api_result {
		Ok(response) => {
			// CRITICAL FIX: Track exchange cost immediately after successful API call
			// This ensures all API calls (with or without tool calls) have their costs tracked
			if let Err(e) =
				CostTracker::track_exchange_cost(chat_session, &response.exchange, config)
			{
				if mode.is_terminal_mode() {
					println!(
						"{}: Failed to track exchange cost: {}",
						"Warning".bright_yellow(),
						e
					);
				}
			}

			// Update animation cost BEFORE process_response stops it.
			// track_exchange_cost() just updated total_cost; push it now so the
			// animation (and next turn's start) shows the correct post-call value.
			anim_state.update_cost(chat_session.session.info.total_cost);

			// Display rate limit information if available
			display_rate_limit_info(&response.exchange);

			// Process the response with tool calls
			// CRITICAL FIX: Use operation_cancelled instead of creating a new token
			// This ensures Ctrl+C cancellation works properly during tool execution
			let process_result = process_response(ResponseProcessingParams {
				content: response.content,
				exchange: response.exchange,
				tool_calls: response.tool_calls,
				thinking: response.thinking,
				finish_reason: response.finish_reason,
				response_id: response.response_id,
				chat_session: &mut *chat_session,
				config,
				role,
				operation_cancelled: operation_rx_for_response.clone(),
				sink: sink.clone(),
				mode,
			})
			.await;

			// Propagate response-processing errors (e.g. follow-up API call failures
			// after tool execution) so the main loop can offer a Ctrl+G retry.
			// Previously this was printed-and-swallowed, hiding the failure from
			// the retry mechanism.
			process_result?;
		}
		Err(e) => {
			// Stop animation on error before returning
			animation_manager.stop_current().await;
			return Err(e);
		}
	}

	// External plan manager: reconcile a signal emitted on the turn's FINAL
	// (tool-less) response before completion verification reads plan state. Mid-turn signals
	// reconcile at the tool-result boundary; a final-message `phase_complete`
	// crosses no such boundary, so without this the verifier would see plan state
	// the manager never reconciled. Free no-op without a pending signal.
	if config.supervisor.enabled
		&& config.supervisor.plan.enabled
		&& chat_session.pending_plan_signal.is_some()
	{
		animation_manager.set_phase("Reconciling plan …").await;
		if let Err(error) = crate::supervisor::plan::reconcile_after_actions(
			chat_session,
			config,
			operation_rx.clone(),
		)
		.await
		{
			crate::log_debug!("External plan reconciliation failed: {}", error);
		}
		animation_manager.clear_phase();
	}

	// Unfinished hand-back pre-gate (free, deterministic): in non-interactive
	// runs there is no user to pick the turn back up, so a final message with no
	// tool calls while the agent's OWN status still says exploring/progressing is
	// a promise, not a result ("Let me implement the fix." → session end). Advisory
	// continuation driven purely by the self-report — done stays gated,
	// blocked/need_input stay legitimate hand-backs. Bounded by the free-check
	// budget, so a model that keeps yielding cannot loop it.
	if config.supervisor.gate.enabled
		&& chat_session.completion_gate_eligible
		&& !mode.is_interactive()
		&& matches!(
			chat_session.last_self_report,
			Some(crate::supervisor::detect::SelfReport::Exploring)
				| Some(crate::supervisor::detect::SelfReport::Progressing)
		) && chat_session.nudge_iterations < crate::supervisor::gate::MAX_ITERATIONS
	{
		chat_session.add_system_managed_user_message(CONTINUE_NOTE)?;
		chat_session.last_self_report = None;
		chat_session.nudge_iterations += 1;
		crate::supervisor::stats::pregate_block();
		crate::supervisor::notify("turn ended while still in progress — continuing");
		crate::log_debug!(
			"Pre-gate: unfinished hand-back; re-running turn (iter {})",
			chat_session.nudge_iterations
		);
		return Box::pin(execute_api_call_and_process_response(
			chat_session,
			config,
			role,
			operation_rx,
			mode,
			sink,
		))
		.await;
	}

	// Supervisor verify-gate: on self-reported completion, verify before accepting.
	// On gaps, inject an advisory and re-run the turn (bounded by max_iterations).
	if config.supervisor.gate.enabled {
		crate::log_debug!(
			"gate: self_report={:?} iter={}/{} nudges={} needs_verification={}",
			chat_session.last_self_report,
			chat_session.gate_iterations,
			crate::supervisor::gate::MAX_ITERATIONS,
			chat_session.nudge_iterations,
			chat_session
				.detectors
				.needs_verification(crate::supervisor::workdir::fingerprint())
		);
	}
	// An explicit `done` self-report claims completion — and so does ending the
	// turn with no status at all after having changed state: a token the model
	// forgot must not become an unverified exit (observed: sessions ending with
	// self_report=None skipped the whole gate). Pure answers (no mutations)
	// stay ungated, in every mode alike.
	if config.supervisor.gate.enabled
		&& claims_user_task_completion(
			chat_session.completion_gate_eligible,
			chat_session.last_self_report,
			!chat_session.evidence.mutated_paths().is_empty(),
		) && chat_session.gate_iterations < crate::supervisor::gate::MAX_ITERATIONS
	{
		// One genuine user message defines the verification turn. Supervisor,
		// recall, skill, and continuation injections after it remain part of the
		// runtime conversation but cannot move this boundary.
		let turn_start = latest_real_user_turn_start(&chat_session.session.messages);
		// Task content via the continuation-aware helper: after a compaction drains
		// the raw user turns, the live request survives only inside the
		// `<continuation>` wrapper's `<task>`. Reading the message directly returns
		// empty there, and an empty task makes `verify()` fail open — silently
		// disabling the verify-gate for the rest of the session.
		let task = crate::session::latest_real_user_task_content(&chat_session.session.messages)
			.unwrap_or_default()
			.to_string();
		let live_plan = crate::mcp::core::plan::render_plan_details().unwrap_or_default();
		// Read the resolution captured before work began, before completion checks
		// consult session-persistent plan state.
		let resolved_task = completion_task(chat_session, &task, &live_plan);
		let plan_changed_this_turn = live_plan != resolved_task.plan_at_turn_start;
		let plan_applies = crate::supervisor::resolve::plan_applies(&resolved_task, &live_plan);
		crate::log_debug!(
			"gate task scope={} sources={} plan_relevant={} answer_only={} plan_changed={}",
			resolved_task.scope.as_str(),
			resolved_task.context_sources.join(","),
			resolved_task.plan_relevant,
			resolved_task.answer_only,
			plan_changed_this_turn
		);
		// Free pre-gate (no model call): the most common false-done is claiming
		// completion right after a code change without re-running any check. Catch
		// it deterministically before paying for the LLM verify-gate. Bounded by the
		// free-check budget (nudge_iterations), so it can't loop unbounded.
		// Check every message since the current turn's real user task, not just
		// the newest user-role message: recite/steer/recall inject their own
		// user-role notes after the pre-gate note, which would hide it and cause
		// a duplicate nudge that burns the gate budget. Scoping to the current
		// turn also avoids matching a pre-gate note left in earlier history.
		let already_nudged = {
			let msgs = &chat_session.session.messages;
			msgs[turn_start..]
				.iter()
				.any(|m| m.content.contains(PREGATE_MARKER))
		};
		// A project with `[[validator]]` guardrails has declared its own
		// verification regime: end-of-turn scripts that run on their configured
		// conditions and fail loudly into the inbox. Nudging the model to "run
		// a check" on top of that second-guesses the project's regime — and
		// misfires on jobs whose deliverable is a report, where running checks
		// is not the task. Job-agnostic by design: keyed on configuration
		// presence, never on message or job content.
		let validators_configured =
			crate::session::guardrails::get_rules(&chat_session.session.info.name)
				.map(|r| !r.validators.is_empty())
				.unwrap_or(false);
		// The current-turn verdict covers role instructions and immediate user
		// wording. The persisted user policy covers prior genuine turns, including
		// answer-only turns that never reached a completion claim. An explicit
		// later permission changes that policy during turn admission; silence does
		// not. Detector streak state is intentionally not an instruction store.
		let check_run_forbidden = resolved_task.forbids_verification
			|| chat_session.session.info.verification_policy.forbids();
		if check_run_forbidden {
			crate::log_debug!("Pre-gate: check-run forbidden by user/instructions; standing down");
		}
		// Observe-only turns (a report, briefing, review, explanation) deliver
		// text, not state — "run a check" is a category error there, and the
		// tree fingerprint may have moved for reasons outside the agent (a
		// concurrent editor, a generated artifact). The classifier's
		// answer_only verdict stands this pre-gate down just as it suppresses
		// automatic plan formation; the LLM verify-gate still judges the report
		// itself under its observe-only rules.
		if !resolved_task.answer_only
			&& !validators_configured
			&& !check_run_forbidden
			&& chat_session
				.detectors
				.needs_verification(crate::supervisor::workdir::fingerprint())
		{
			let next_iteration = chat_session.nudge_iterations.saturating_add(1);
			if next_iteration >= crate::supervisor::gate::MAX_ITERATIONS {
				chat_session.nudge_iterations = next_iteration;
				chat_session.gate_failed = true;
				chat_session.learning_outcome =
					crate::supervisor::learning::TrajectoryOutcome::Failed;
				chat_session.pending_plan_signal = None;
				crate::supervisor::stats::pregate_block();
				crate::supervisor::stats::gate_fail();
				crate::supervisor::notify(
					"unverified state changes remain — repair budget exhausted",
				);
				return Ok(());
			}
			if !already_nudged {
				chat_session.add_system_managed_user_message(PREGATE_NOTE)?;
			}
			chat_session.last_self_report = None; // force the re-run to re-evaluate
			chat_session.nudge_iterations = next_iteration;
			crate::supervisor::stats::pregate_block();
			crate::supervisor::notify("done claimed with unverified state changes — re-running");
			crate::log_debug!(
				"Pre-gate: unverified mutation; re-running turn (iter {})",
				chat_session.nudge_iterations
			);
			return Box::pin(execute_api_call_and_process_response(
				chat_session,
				config,
				role,
				operation_rx,
				mode,
				sink,
			))
			.await;
		}

		// The whole turn's answer, not just its last message: a supervisor re-run
		// answers the correction it was given, so the deliverable usually sits in an
		// earlier part of the same turn.
		let result = current_turn_answer(
			&chat_session.turn_answers,
			config.supervisor.gate.max_tokens as usize,
		);
		let result = if result.is_empty() {
			chat_session.last_response.clone()
		} else {
			result
		};
		let claim = chat_session.last_self_report_reason.clone();
		let actions = chat_session.evidence.render();
		// Only a relevant pre-existing plan or a plan changed by this turn reaches
		// the verifier. Unrelated old plan state remains alive but cannot add scope.
		let plan = if plan_applies {
			live_plan
		} else {
			String::new()
		};
		// Runtime-gathered ground truth: the diff of what actually changed and
		// the last command's recorded output — the verifier judges state, not story.
		let mut ground_truth = crate::supervisor::gate::render_ground_truth(
			chat_session.evidence.mutated_paths(),
			&chat_session.evidence.recent_commands(),
		);
		// Verification-evidence provenance: the runtime KNOWS whether any
		// command-shaped check succeeded since the last state change; the
		// verifier must not have to infer that absence from a raw action log
		// (small verifiers demonstrably don't). Stated as observed fact — the
		// verdict stays the verifier's.
		if !chat_session.evidence.mutated_paths().is_empty() {
			let provenance = if chat_session
				.detectors
				.needs_verification(crate::supervisor::workdir::fingerprint())
			{
				Some(
					"Runtime observation: NO check of any kind has succeeded on the changed state since the agent's last state change.",
				)
			} else if chat_session.detectors.cleared_by_readback_only() {
				Some(
					"Runtime observation: since its last state change the agent only re-read its own edited artifacts; no command-shaped check (build, test, run, validator) succeeded on the changed state. Inspection verifies artifact content, never behavior.",
				)
			} else {
				None
			};
			if let Some(p) = provenance {
				if !ground_truth.is_empty() {
					ground_truth.push_str("\n\n");
				}
				ground_truth.push_str(p);
			}
		}
		let prior_gaps = chat_session.last_gate_gaps.clone();
		crate::supervisor::stats::gate_run();
		animation_manager.set_phase("Verifying completion …").await;
		let verdict = crate::supervisor::gate::verify(
			config,
			crate::supervisor::gate::GateInput {
				original_task: &resolved_task.original_request,
				task: &resolved_task.resolved_request,
				task_scope: resolved_task.scope,
				context_sources: &resolved_task.context_sources,
				resolution_evidence: &resolved_task.resolution_evidence,
				result: &result,
				claim: claim.as_deref(),
				actions: &actions,
				grounds: chat_session.evidence.grounds(),
				plan: &plan,
				ground_truth: &ground_truth,
				prior_gaps: &prior_gaps,
				role_context: &crate::supervisor::role_context(&chat_session.session.messages),
				evidence_conditions: &resolved_task.evidence_conditions,
			},
			operation_rx.clone(),
		)
		.await;
		animation_manager.clear_phase();
		// Did the trajectory gain anything since the pass that produced
		// `prior_gaps`? A finding that survives new evidence is diagnostic (see
		// the Gaps arm); one that survives a re-run with nothing new is not.
		let new_evidence = chat_session.evidence.actions_since_gate() > 0;
		chat_session.evidence.mark_gate_checkpoint();
		match verdict {
			crate::supervisor::gate::GateVerdict::Pass => {
				if plan_applies {
					let summary = claim.as_deref().unwrap_or("Completion verified");
					if let Err(error) = crate::supervisor::plan::finalize_after_completion(summary)
					{
						chat_session.gate_failed = true;
						chat_session.learning_outcome =
							crate::supervisor::learning::TrajectoryOutcome::Failed;
						crate::supervisor::stats::gate_fail();
						crate::supervisor::notify(&format!(
							"completion evidence passed, but plan finalization failed: {error}"
						));
						reinforce_recalled(chat_session, config, -0.05).await;
						return Ok(());
					}
				}
				chat_session.gate_iterations = 0;
				chat_session.nudge_iterations = 0;
				chat_session.gate_failed = false;
				chat_session.learning_outcome =
					crate::supervisor::learning::TrajectoryOutcome::Verified;
				chat_session.last_gate_gaps.clear();
				crate::supervisor::stats::gate_pass();
				crate::log_debug!("Verify-gate: PASS");
				crate::supervisor::notify("completion verified");
				reinforce_recalled(chat_session, config, 0.05).await;
			}
			crate::supervisor::gate::GateVerdict::Gaps(gaps) => {
				chat_session.pending_plan_signal = None;
				// The same finding twice, across a re-run that DID gather new
				// evidence, is a check the loop cannot converge on — the agent
				// answered it and the verdict did not move. Repeating the advisory
				// only spends the budget to arrive here again, so hand the finding
				// to the user instead of blocking on it. The trajectory stays
				// unverified; learning may retain only a failure-labelled experience.
				if new_evidence && crate::supervisor::gate::gaps_unchanged(&prior_gaps, &gaps) {
					chat_session.gate_iterations = 0;
					chat_session.last_gate_gaps.clear();
					chat_session.gate_failed = true;
					chat_session.learning_outcome =
						crate::supervisor::learning::TrajectoryOutcome::Failed;
					crate::supervisor::stats::gate_stall();
					let mut msg = String::from(
						"verification did not converge — unchanged after new evidence",
					);
					for g in &gaps {
						msg.push_str("\n- ");
						msg.push_str(g);
					}
					crate::supervisor::notify(&msg);
					crate::log_debug!(
						"Verify-gate: {} gap(s) unchanged after new evidence; not re-running",
						gaps.len()
					);
					reinforce_recalled(chat_session, config, -0.15).await;
					return Ok(());
				}
				let note = crate::supervisor::gate::format_advisory(&gaps);
				chat_session.add_system_managed_user_message(&note)?;
				chat_session.last_self_report = None; // force the re-run to re-evaluate
				chat_session.gate_iterations += 1;
				chat_session.last_gate_gaps = gaps.clone();
				crate::log_debug!(
					"Verify-gate: {} gap(s); re-running turn (iter {})",
					gaps.len(),
					chat_session.gate_iterations
				);
				if chat_session.gate_iterations < crate::supervisor::gate::MAX_ITERATIONS {
					let mut msg = format!("verification found {} gap(s) — re-running", gaps.len());
					for g in &gaps {
						msg.push_str("\n- ");
						msg.push_str(g);
					}
					crate::supervisor::notify(&msg);
					return Box::pin(execute_api_call_and_process_response(
						chat_session,
						config,
						role,
						operation_rx,
						mode,
						sink,
					))
					.await;
				}
				chat_session.gate_failed = true;
				chat_session.learning_outcome =
					crate::supervisor::learning::TrajectoryOutcome::Failed;
				crate::supervisor::stats::gate_fail();
				crate::log_debug!("Verify-gate: iterations exhausted; gaps remain");
				// Name the gaps here too: this is the user's last word on the turn,
				// and "gaps remain" without them is unactionable.
				let mut msg = String::from("verification gaps remain — iterations exhausted");
				for g in &gaps {
					msg.push_str("\n- ");
					msg.push_str(g);
				}
				crate::supervisor::notify(&msg);
				reinforce_recalled(chat_session, config, -0.15).await;
			}
			crate::supervisor::gate::GateVerdict::Indeterminate(reason) => {
				chat_session.pending_plan_signal = None;
				// A verdict the verifier could not produce is not verified work: the
				// trajectory stays labelled unverified whatever the re-entry below
				// does, and only a later PASS clears the label.
				chat_session.gate_failed = true;
				chat_session.learning_outcome =
					crate::supervisor::learning::TrajectoryOutcome::Failed;
				chat_session.gate_iterations += 1;
				crate::log_debug!(
					"Verify-gate: indeterminate: {} (iter {})",
					reason,
					chat_session.gate_iterations
				);
				// The same bounded budget a substantive gap spends: an unreadable
				// verdict that fell through here was completion accepted without
				// verification.
				if let Some(note) = crate::supervisor::gate::unverified_reentry(
					chat_session.gate_iterations,
					crate::supervisor::gate::MAX_ITERATIONS,
				) {
					chat_session.add_system_managed_user_message(&note)?;
					chat_session.last_self_report = None; // force the re-run to re-evaluate
					crate::supervisor::notify(&format!(
						"completion could not be verified ({reason}) — re-running"
					));
					return Box::pin(execute_api_call_and_process_response(
						chat_session,
						config,
						role,
						operation_rx,
						mode,
						sink,
					))
					.await;
				}
				crate::supervisor::stats::gate_fail();
				crate::log_debug!("Verify-gate: iterations exhausted; completion unverified");
				crate::supervisor::notify(&format!("completion could not be verified: {reason}"));
				reinforce_recalled(chat_session, config, -0.05).await;
			}
		}
	}

	// With the completion gate disabled, final self-report is the configured
	// completion authority. It may retire only a plan owned by this turn;
	// unrelated persistent plan state remains untouched.
	if config.supervisor.enabled
		&& config.supervisor.plan.enabled
		&& !config.supervisor.gate.enabled
		&& chat_session.completion_gate_eligible
		&& chat_session.last_self_report == Some(crate::supervisor::detect::SelfReport::Done)
		&& crate::mcp::core::plan::has_active_plan()
	{
		let task = crate::session::latest_real_user_task_content(&chat_session.session.messages)
			.unwrap_or_default()
			.to_string();
		let live_plan = crate::mcp::core::plan::render_plan_details().unwrap_or_default();
		let resolved_task = completion_task(chat_session, &task, &live_plan);
		if crate::supervisor::resolve::plan_applies(&resolved_task, &live_plan) {
			let summary = chat_session
				.last_self_report_reason
				.as_deref()
				.unwrap_or("Specialist reported completion");
			if let Err(error) = crate::supervisor::plan::finalize_after_completion(summary) {
				crate::log_debug!("External plan finalization failed: {}", error);
			}
		} else {
			crate::log_debug!("External plan retained: completed turn does not own it");
		}
	}

	// A background fold still in flight at turn end: collect and apply it so
	// the session persists compacted — replace only, never auto-continue (no
	// agent call is made here; the summary was already paid for).
	match crate::session::chat::conversation_compression::settle_pending_fold(
		chat_session,
		&config_clone,
	)
	.await
	{
		Ok(true) => {
			if let Err(error) = chat_session.save() {
				crate::log_debug!("Session save after settled fold failed: {}", error);
			}
		}
		Ok(false) => {}
		Err(error) => crate::log_debug!("Settling background fold failed: {}", error),
	}

	// A terminal turn without a verify-gate verdict still records materially
	// reported use for retention, but applies no correctness credit. Gate paths
	// already consumed the references above, so this is a no-op for them.
	reinforce_recalled(chat_session, config, 0.0).await;

	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test]
	async fn outcome_credit_updates_only_materially_used_memory() {
		let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
		let data = tempfile::tempdir().unwrap();
		let previous = std::env::var_os("OCTOMIND_DATA_DIR");
		std::env::set_var("OCTOMIND_DATA_DIR", data.path());
		let config = crate::session::chat::test_support::fake_provider_config();
		let role = "__credit_role";
		let project = "__credit_project";
		let backend =
			crate::supervisor::learning::backend::create_backend(&config.supervisor.learning);
		for content in [
			"exposed but unused",
			"materially used",
			"used without verdict",
		] {
			backend
				.store(
					&crate::supervisor::learning::Lesson {
						content: content.to_string(),
						role: role.to_string(),
						project: project.to_string(),
						created: chrono::Utc::now().to_rfc3339(),
						..Default::default()
					},
					&config,
				)
				.await
				.unwrap();
		}
		let mut session = ChatSession::for_tests(Vec::new());
		session.recalled_refs = vec![
			(
				"M1".to_string(),
				"exposed but unused".to_string(),
				role.to_string(),
				project.to_string(),
			),
			(
				"M2".to_string(),
				"materially used".to_string(),
				role.to_string(),
				project.to_string(),
			),
		];
		session.used_memory_ids.insert("M2".to_string());
		reinforce_recalled(&mut session, &config, 0.05).await;
		session.recalled_refs = vec![(
			"M3".to_string(),
			"used without verdict".to_string(),
			role.to_string(),
			project.to_string(),
		)];
		session.used_memory_ids.insert("M3".to_string());
		reinforce_recalled(&mut session, &config, 0.0).await;
		let memories = backend.retrieve_all(role, project, &config).await.unwrap();
		let unused = memories
			.iter()
			.find(|memory| memory.content == "exposed but unused")
			.unwrap();
		let used = memories
			.iter()
			.find(|memory| memory.content == "materially used")
			.unwrap();
		let unused_importance = unused.importance;
		let used_importance = used.importance;
		let used_count = used.use_count;
		let used_last_used = used.last_used.clone();
		let neutral = memories
			.iter()
			.find(|memory| memory.content == "used without verdict")
			.unwrap();
		let neutral_importance = neutral.importance;
		let neutral_count = neutral.use_count;

		match previous {
			Some(value) => std::env::set_var("OCTOMIND_DATA_DIR", value),
			None => std::env::remove_var("OCTOMIND_DATA_DIR"),
		}
		assert_eq!(unused_importance, 0.5);
		assert!((used_importance - 0.55).abs() < f64::EPSILON);
		assert_eq!(used_count, 1);
		assert!(!used_last_used.is_empty());
		assert_eq!(neutral_importance, 0.5);
		assert_eq!(neutral_count, 1);
	}

	#[test]
	fn turn_answer_joins_every_pass_oldest_first() {
		// Turn-boundary and tool-call filtering happen at the append/clear
		// sites (the ledger is state); assembly joins the passes in order.
		let answers = vec![
			"THE BRIEF".to_string(),
			"The link is grounded; the brief stands.".to_string(),
		];
		assert_eq!(
			current_turn_answer(&answers, 8192),
			format!("THE BRIEF{ANSWER_PART_SEPARATOR}The link is grounded; the brief stands.")
		);
	}

	#[test]
	fn turn_answer_keeps_the_newest_pass_when_over_budget() {
		// The older pass alone exceeds the token budget; the newest is always kept.
		let old = "many different words fill the older pass ".repeat(50);
		let answers = vec![old, "the amendment".to_string()];
		assert_eq!(current_turn_answer(&answers, 16), "the amendment");
	}

	#[test]
	fn pregate_feedback_is_domain_agnostic() {
		assert!(PREGATE_NOTE.contains("state changes"));
		assert!(PREGATE_NOTE.contains("domain-specific validator"));
		assert!(!PREGATE_NOTE.contains("code changes"));
		assert!(!PREGATE_NOTE.contains("build / test / lint"));
	}

	#[test]
	fn system_managed_response_cannot_complete_the_latest_user_task() {
		use crate::supervisor::detect::SelfReport;

		assert!(claims_user_task_completion(
			true,
			Some(SelfReport::Done),
			false
		));
		assert!(claims_user_task_completion(true, None, true));
		assert!(!claims_user_task_completion(
			false,
			Some(SelfReport::Done),
			false
		));
		assert!(!claims_user_task_completion(false, None, true));
	}
}

#[cfg(test)]
#[path = "api_executor_tests.rs"]
mod e2e_tests;
