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

	// Resolve each genuine user turn before compression or agent work. The same
	// cached resolution later serves planning and completion, so this is one
	// semantic pass, not a second policy classifier. Doing it at turn admission
	// is essential: an answer-only turn may never claim completion, but its
	// explicit "I will test it; do not run checks" policy must still govern the
	// next mutation turn. System-managed turns are ineligible and cannot update
	// user-owned policy.
	if config.supervisor.enabled
		&& chat_session.completion_gate_eligible
		&& (config.supervisor.gate.enabled || config.supervisor.plan.enabled)
		&& chat_session.gate_task.is_none()
	{
		let session_context = chat_session.session.info.anchor.to_xml();
		let active_plan = crate::mcp::core::plan::render_plan_details();
		if let Some(context) = crate::supervisor::resolve::TaskContext::capture(
			&chat_session.session.messages,
			&session_context,
			active_plan.as_deref(),
			chat_session.session.info.verification_policy,
		) {
			let animation_manager = crate::session::chat::get_animation_manager();
			animation_manager
				.set_phase("Resolving current task …")
				.await;
			let resolved =
				crate::supervisor::resolve::resolve(config, &context, operation_rx.clone()).await;
			animation_manager.clear_phase();
			let policy_changed = chat_session
				.session
				.info
				.verification_policy
				.apply(resolved.verification_policy_update);
			let outcome_contract = crate::supervisor::resolve::outcome_contract_note(&resolved);
			chat_session.gate_task = Some(resolved);
			if let Some(note) = outcome_contract {
				chat_session.add_system_managed_user_message(&note)?;
				crate::log_debug!("Supervisor outcome contract injected");
			}
			if policy_changed {
				// Persist at the ownership boundary. Most modes also save after the
				// response, but cancellation and ACP monitor paths can return earlier.
				if let Err(error) = chat_session.save() {
					crate::log_debug!("Failed to persist verification policy update: {}", error);
				}
			}
			if *operation_rx.borrow() {
				return Err(anyhow::Error::new(crate::session::cancellation::Cancelled));
			}
		}
	}

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
		crate::log_debug!("Compression failed before API call: {}.", e);
	}
	crate::session::chat::conversation_compression::ensure_context_within_ceiling(
		chat_session,
		config,
	)
	.await?;

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
				log_info!(
					"System message has been automatically marked for caching to save tokens."
				);
				// Save the session to ensure the cached status is persisted
				if let Err(e) = chat_session.save() {
					crate::log_debug!("session save failed: {}", e);
				}
			}
		}
	}

	Ok(())
}
