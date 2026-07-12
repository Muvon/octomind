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

use super::super::response::{
	process_response, run_deferred_plan_compression, ResponseProcessingParams,
};
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
const PREGATE_NOTE: &str = "<pay-attention>\n<!-- octomind:pre_gate_unverified_mutation -->\nYou may only report done after a verification has actually passed. You reported done with code changes still unverified, so that claim isn't trustworthy yet. Run this project's check (build / test / lint — whatever it uses), watch the result, and report the actual outcome: pass, fail, or — if this project has no such check — which command you tried and why none applies. Base the report on the observed result, not on what you expect.\n</pay-attention>";

/// Apply the verify-gate's verdict back to the entries recalled this trajectory:
/// positive `delta` reinforces (the recall helped); negative decays (it may have
/// misled). Clears the recalled set either way.
async fn reinforce_recalled(chat_session: &mut ChatSession, config: &Config, delta: f64) {
	let refs = std::mem::take(&mut chat_session.recalled_refs);
	if refs.is_empty() {
		return;
	}
	let backend = crate::supervisor::learning::backend::create_backend(&config.supervisor.learning);
	for (content, role, project) in &refs {
		let _ = backend
			.reinforce(content, role, project, delta, config)
			.await;
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

	// Inject learned lessons. Two triggers:
	//   - first call of the session → global tier + full hybrid scoped recall;
	//   - a new user message (pending_recall) → embedding-only scoped recall.
	// Already-injected lessons are skipped (no duplication), and tool follow-up
	// rounds — which set neither flag — don't re-run recall.
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
		let (block, new_contents) = crate::supervisor::learning::inject::retrieve_and_format(
			config,
			&user_input,
			role,
			&project,
			first_call,
			&chat_session.injected_lessons,
			operation_rx.clone(),
		)
		.await;
		animation_manager.clear_phase();
		if !block.is_empty() {
			chat_session.add_system_managed_user_message(&block)?;
			crate::supervisor::stats::recall();
			crate::supervisor::notify(&format!(
				"recalled {} lesson(s) into context",
				new_contents.len()
			));
			for c in new_contents {
				chat_session
					.recalled_refs
					.push((c.clone(), role.to_string(), project.clone()));
				chat_session.injected_lessons.insert(c);
			}
		}
	}

	// Supervisor: inject any queued steer note (advisory re-anchor) at the safe
	// pre-request point — same message-ordering guarantees as recall above.
	if let Some(note) = chat_session.steer_pending.take() {
		chat_session.add_system_managed_user_message(&note)?;
		crate::log_debug!("Supervisor steer injected");
	}

	// Supervisor: goal recitation. Once the session has compacted at least once
	// the durable goal lives only in the mid-transcript compressed summary, where
	// attention is weak. Re-emit a tiny goal block here — at the tail, in the
	// recency window — and crucially BEFORE the cache-marker advance below, so the
	// cached prefix stays intact (the recited block lands after it each turn).
	if config.supervisor.enabled && config.supervisor.recite.enabled {
		// Prefer the live plan checklist (refreshed every turn from plan storage)
		// over the anchor's stale next_steps snapshot for the recency-slot block.
		let plan_checklist = crate::mcp::core::plan::render_plan_checklist();
		if let Some(note) = crate::supervisor::recite::recite_note(
			&chat_session.session.info.anchor,
			plan_checklist.as_deref(),
		) {
			chat_session.add_system_managed_user_message(&note)?;
			crate::log_debug!("Supervisor goal recitation injected");
		}
	}

	// Advance Anthropic-style content cache markers after all pre-call message injections
	// (learning context, inbox hints, etc.) and immediately before building the request.
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

	// Supervisor verify-gate: on self-reported completion, verify before accepting.
	// On gaps, inject an advisory and re-run the turn (bounded by max_iterations).
	if config.supervisor.gate.enabled
		&& chat_session.last_self_report == Some(crate::supervisor::detect::SelfReport::Done)
		&& chat_session.gate_iterations < config.supervisor.gate.max_iterations
	{
		// Free pre-gate (no model call): the most common false-done is claiming
		// completion right after a code change without re-running any check. Catch
		// it deterministically before paying for the LLM verify-gate. Shares the
		// gate_iterations budget, so it can't loop unbounded.
		// Check every message since the current turn's real user task, not just
		// the newest user-role message: recite/steer/recall inject their own
		// user-role notes after the pre-gate note, which would hide it and cause
		// a duplicate nudge that burns the gate budget. Scoping to the current
		// turn also avoids matching a pre-gate note left in earlier history.
		let already_nudged = {
			let msgs = &chat_session.session.messages;
			let turn_start = msgs
				.iter()
				.rposition(crate::session::is_real_user_task_message)
				.unwrap_or(0);
			msgs[turn_start..]
				.iter()
				.any(|m| m.content.contains(PREGATE_MARKER))
		};
		if config.supervisor.gate.require_check_after_mutation
			&& chat_session.detectors.needs_verification()
			&& !already_nudged
		{
			chat_session.add_system_managed_user_message(PREGATE_NOTE)?;
			chat_session.last_self_report = None; // force the re-run to re-evaluate
			chat_session.gate_iterations += 1;
			crate::supervisor::stats::pregate_block();
			crate::supervisor::notify("done claimed without a check after changes — re-running");
			if chat_session.gate_iterations < config.supervisor.gate.max_iterations {
				crate::log_debug!(
					"Pre-gate: unverified mutation; re-running turn (iter {})",
					chat_session.gate_iterations
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
			// Budget exhausted — fall through to the LLM gate / acceptance.
		}

		// Free plan pre-gate (no model call): a self-reported `done` while the live
		// plan still has open items is drift-by-omission — parts of the decomposed
		// task silently dropped. The agent must finish them or close them out via
		// the plan tool. Same marker/budget pattern as the mutation pre-gate above.
		if config.supervisor.gate.require_plan_complete {
			let open = crate::mcp::core::plan::open_plan_tasks();
			let already_nudged_plan = {
				let msgs = &chat_session.session.messages;
				let turn_start = msgs
					.iter()
					.rposition(crate::session::is_real_user_task_message)
					.unwrap_or(0);
				msgs[turn_start..].iter().any(|m| {
					m.content
						.contains(crate::supervisor::gate::PLAN_GATE_MARKER)
				})
			};
			if !open.is_empty() && !already_nudged_plan {
				let note = crate::supervisor::gate::format_plan_advisory(&open);
				chat_session.add_system_managed_user_message(&note)?;
				chat_session.last_self_report = None; // force the re-run to re-evaluate
				chat_session.gate_iterations += 1;
				crate::supervisor::stats::plan_block();
				crate::supervisor::notify(&format!(
					"done claimed with {} open plan item(s) — re-running",
					open.len()
				));
				if chat_session.gate_iterations < config.supervisor.gate.max_iterations {
					crate::log_debug!(
						"Plan pre-gate: {} open item(s); re-running turn (iter {})",
						open.len(),
						chat_session.gate_iterations
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
				// Budget exhausted — fall through to the LLM gate / acceptance.
			}
		}

		// Free evidence check (no model call): a `done` answer that cites « » quotes
		// which appear in NO tool result, or `file:line` references that do not
		// hold on disk, is fabricating its support. Catch both deterministically
		// and re-ground via the same bounded re-run.
		if config.supervisor.claim_check {
			let tool_outputs: Vec<String> = chat_session
				.session
				.messages
				.iter()
				.filter(|m| m.role == "tool")
				.map(|m| m.content.clone())
				.collect();
			let unverified = crate::supervisor::detect::unverified_citations(
				&chat_session.last_response,
				&tool_outputs,
			);
			let bad_refs =
				crate::supervisor::detect::unverified_file_refs(&chat_session.last_response);
			if !unverified.is_empty() || !bad_refs.is_empty() {
				let mut note = String::from("<pay-attention>\n");
				if !unverified.is_empty() {
					note.push_str(
						"Each quote below was presented as «verbatim» from a tool result, but none string-matches any output you received — so it is unsupported. For each, go back to the actual tool output (not your earlier answer): copy the exact lines that support the claim, then restate the claim from them. If no tool output contains them, say so and drop that claim — \"not found in tool output\" is the correct answer here; never invent a source. Unsupported quotes:\n",
					);
					for q in &unverified {
						note.push_str("- «");
						note.push_str(q);
						note.push_str("»\n");
					}
				}
				if !bad_refs.is_empty() {
					note.push_str(
						"Each file:line reference below does not hold on disk — the file is missing or the line is beyond its end. Re-check the real location and cite the correct file and line; if the reference was illustrative or the file was intentionally deleted, say so instead of citing it as a location. Invalid references:\n",
					);
					for r in &bad_refs {
						note.push_str("- ");
						note.push_str(r);
						note.push('\n');
					}
				}
				note.push_str("</pay-attention>");
				chat_session.add_system_managed_user_message(&note)?;
				chat_session.last_self_report = None; // force the re-run to re-evaluate
				chat_session.gate_iterations += 1;
				crate::supervisor::stats::claim_block();
				crate::supervisor::notify(&format!(
					"{} unverifiable citation(s), {} invalid file reference(s) — re-running",
					unverified.len(),
					bad_refs.len()
				));
				if chat_session.gate_iterations < config.supervisor.gate.max_iterations {
					crate::log_debug!(
						"Evidence check: {} unverified citation(s), {} bad file ref(s); re-running (iter {})",
						unverified.len(),
						bad_refs.len(),
						chat_session.gate_iterations
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
				// Budget exhausted — fall through to the LLM gate / acceptance.
			}
		}

		// The genuine task is the most recent user turn that is NOT a supervisor
		// injection — so re-runs verify against the real request, not our advisory.
		let task = chat_session
			.session
			.messages
			.iter()
			.rev()
			.find(|m| crate::session::is_real_user_task_message(m))
			.map(|m| m.content.clone())
			.unwrap_or_default();
		let result = chat_session.last_response.clone();
		let claim = chat_session.last_self_report_reason.clone();
		let actions = chat_session.evidence.render();
		// Durable goal + live plan for the verifier: a terse follow-up turn
		// ("continue") is only verifiable against what it refers to.
		let plan_checklist = crate::mcp::core::plan::render_plan_checklist();
		let context = crate::supervisor::gate::render_session_context(
			&chat_session.session.info.anchor.intent,
			plan_checklist.as_deref(),
		);
		// Runtime-gathered ground truth: the diff of what actually changed and
		// the last command's recorded output — the verifier judges state, not story.
		let ground_truth = crate::supervisor::gate::render_ground_truth(
			chat_session.evidence.mutated_paths(),
			chat_session.evidence.last_command(),
		);
		let prior_gaps = chat_session.last_gate_gaps.clone();
		crate::supervisor::stats::gate_run();
		animation_manager.set_phase("Verifying completion …").await;
		let verdict = crate::supervisor::gate::verify(
			config,
			crate::supervisor::gate::GateInput {
				task: &task,
				result: &result,
				claim: claim.as_deref(),
				actions: &actions,
				context: &context,
				ground_truth: &ground_truth,
				prior_gaps: &prior_gaps,
			},
			operation_rx.clone(),
		)
		.await;
		animation_manager.clear_phase();
		match verdict {
			crate::supervisor::gate::GateVerdict::Pass => {
				chat_session.gate_iterations = 0;
				chat_session.gate_failed = false;
				chat_session.last_gate_gaps.clear();
				crate::supervisor::stats::gate_pass();
				crate::log_debug!("Verify-gate: PASS");
				crate::supervisor::notify("completion verified");
				reinforce_recalled(chat_session, config, 0.05).await;
			}
			crate::supervisor::gate::GateVerdict::Gaps(gaps) => {
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
				if chat_session.gate_iterations < config.supervisor.gate.max_iterations {
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
				crate::supervisor::stats::gate_fail();
				crate::log_debug!("Verify-gate: iterations exhausted; gaps remain");
				crate::supervisor::notify("verification gaps remain — iterations exhausted");
				reinforce_recalled(chat_session, config, -0.15).await;
			}
		}
	}

	// Deferred plan(done) compression: held until the turn's completion is
	// accepted, so it runs ONCE here — a non-re-run exit reached only after the
	// verify-gate passed (or when no gate applied / iterations were exhausted).
	// The gate's gap path returns earlier to re-run the turn, so a rejected
	// completion never compresses and the re-run keeps full context. No-op unless
	// plan(done) queued a project compression.
	run_deferred_plan_compression(chat_session).await;

	Ok(())
}
