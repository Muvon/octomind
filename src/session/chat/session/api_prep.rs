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

// API preparation utilities

use super::core::ChatSession;
use crate::config::Config;
use crate::log_info;
use crate::session::model_supports_caching;
use anyhow::Result;

/// The turn's task resolution while its supervisor call runs in the background.
/// Nothing before the first model request needs it: the authorizer judges tool
/// calls from the raw user turns and learned rules, the recited checklist is
/// most useful once the request has left the recency window, and only the
/// completion gate cannot run without it. Owning the cancellation channel ties
/// the request's lifetime to the job, as for background folds: dropping the job
/// (a newer user turn) cancels it.
pub struct ResolveJob {
	handle: tokio::task::JoinHandle<crate::supervisor::resolve::ResolvedTask>,
	_cancel: tokio::sync::watch::Sender<bool>,
}

/// Apply the turn's resolution once its background call has finished, or with
/// `wait`, once it finishes.
pub async fn settle_resolution(chat_session: &mut ChatSession, wait: bool) {
	if !chat_session
		.resolve_job
		.as_ref()
		.is_some_and(|job| wait || job.handle.is_finished())
	{
		return;
	}
	let job = chat_session.resolve_job.take().expect("resolve job checked above");
	match job.handle.await {
		Ok(resolved) => apply_resolution(chat_session, resolved),
		Err(error) => crate::log_debug!("Task resolution did not complete: {}", error),
	}
}

fn apply_resolution(chat_session: &mut ChatSession, resolved: crate::supervisor::resolve::ResolvedTask) {
	let policy_changed = chat_session
		.session
		.info
		.verification_policy
		.apply(resolved.verification_policy_update);
	let rules_changed = resolved
		.session_rules_update
		.apply(&mut chat_session.session.info.session_rules);
	chat_session.evidence.retain_history(resolved.scope);
	chat_session.gate_task = Some(resolved);
	if policy_changed || rules_changed {
		// Persist at the ownership boundary. Most modes also save after the
		// response, but cancellation and ACP monitor paths can return earlier.
		if let Err(error) = chat_session.save() {
			crate::log_debug!("Failed to persist standing user policy update: {}", error);
		}
	}
}

// Helper function to prepare for API call (context truncation and caching)
pub async fn prepare_for_api_call(
	chat_session: &mut ChatSession,
	config: &Config,
	operation_rx: tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
	// Check for cancellation before compression
	if *operation_rx.borrow() {
		return Err(anyhow::Error::new(crate::session::cancellation::Cancelled));
	}

	crate::supervisor::authorizer::capture(chat_session, config);

	// Resolve each genuine user turn at admission. The same cached resolution
	// later serves planning, recitation and completion, so this is one semantic
	// pass, not a second policy classifier. It runs in the background (see
	// `ResolveJob`) and lands at the first request boundary after it finishes.
	// System-managed turns are ineligible and cannot update user-owned policy.
	settle_resolution(chat_session, false).await;
	if config.supervisor.enabled
		&& chat_session.completion_gate_eligible
		&& (config.supervisor.gate.enabled || config.supervisor.plan.enabled || config.supervisor.authorizer.enabled)
		&& chat_session.gate_task.is_none()
		&& chat_session.resolve_job.is_none()
	{
		let session_context = chat_session.session.info.anchor.to_xml();
		let active_plan = crate::mcp::core::plan::render_plan_details();
		if let Some(context) = crate::supervisor::resolve::TaskContext::capture(
			&chat_session.session.messages,
			&session_context,
			active_plan.as_deref(),
			chat_session.session.info.verification_policy,
			&chat_session.session.info.session_rules,
		) {
			let config = config.clone();
			let (cancel, cancel_rx) = tokio::sync::watch::channel(false);
			let resolve = crate::supervisor::accounting::background(async move {
				crate::supervisor::resolve::resolve(&config, &context, cancel_rx).await
			});
			let handle = match crate::session::context::current_session_id() {
				Some(session_id) => tokio::spawn(crate::session::context::with_session_id(session_id, resolve)),
				None => tokio::spawn(resolve),
			};
			chat_session.resolve_job = Some(ResolveJob {
				handle,
				_cancel: cancel,
			});
		}
	}
	// Turns that skip resolution, and standing context that changed mid-turn
	// (a skill activated), still get a current brief. A no-op while the
	// compacted context is unchanged.
	crate::supervisor::condense::refresh_specialist_brief(
		&mut chat_session.specialist_brief,
		&chat_session.session.messages,
		config,
		operation_rx.clone(),
	)
	.await;

	// Run compression if max_session_tokens_threshold is exceeded
	if let Err(e) = crate::session::chat::conversation_compression::check_and_compress_conversation(
		chat_session,
		config,
		operation_rx.clone(),
		crate::session::chat::conversation_compression::CompressionTrigger::Automatic,
	)
	.await
	{
		if crate::session::cancellation::is_cancelled(&e) {
			return Err(e);
		}
		// Inside the ceiling margin a failed compression is serious but not
		// fatal: ensure_context_within_ceiling below trims oversized tool
		// results without a model call, and errors itself if that still
		// leaves the context over the ceiling. Returning here instead cost
		// whole turns whenever the compression model returned one
		// unparseable response.
		if crate::session::chat::conversation_compression::within_ceiling_margin(chat_session, config).await {
			crate::log_info!(
				"Forced compression inside the context ceiling margin failed: {}. Falling back to deterministic trimming.",
				e
			);
		} else {
			crate::log_debug!("Compression failed before API call: {}.", e);
		}
	}
	crate::session::chat::conversation_compression::ensure_context_within_ceiling(chat_session, config).await?;

	// Deterministic auto-activation: when the latest message is a fresh
	// user input and a non-active capability strongly matches (margin-gated
	// cosine over hand-authored triggers), enable its MCP servers
	// directly — no LLM in the routing loop. Silent no-op if the model
	// isn't ready or nothing clears the gate.
	if config.auto_capabilities {
		crate::mcp::runtime::capability::auto_activate_capabilities(chat_session, config).await;
	}

	// Ensure system message is cached before making API calls
	let mut system_message_cached = false;

	// Check if system message is already cached
	for msg in &chat_session.session.messages {
		if msg.role == "system" && msg.cached {
			system_message_cached = true;
			break;
		}
	}

	// If system message not already cached, add a cache checkpoint
	if !system_message_cached {
		if let Ok(cached) = chat_session.session.add_cache_checkpoint(true) {
			if cached && model_supports_caching(&chat_session.model) {
				log_info!("System message has been automatically marked for caching to save tokens.");
				// Save the session to ensure the cached status is persisted
				if let Err(e) = chat_session.save() {
					crate::log_debug!("session save failed: {}", e);
				}
			}
		}
	}

	Ok(())
}

#[cfg(test)]
#[path = "api_prep_tests.rs"]
mod tests;
