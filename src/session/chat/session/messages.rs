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

// Session message operations

use super::core::ChatSession;
use crate::config::Config;
use crate::session::ProviderExchange;
use crate::{log_debug, log_info};
use anyhow::Result;
use colored::Colorize;
use std::io::IsTerminal;

impl ChatSession {
	const ACTIVE_MEMORY_MESSAGE_NAME: &'static str = "__active_memory_pack";

	fn is_active_memory_message(message: &crate::session::Message) -> bool {
		message.name.as_deref() == Some(Self::ACTIVE_MEMORY_MESSAGE_NAME)
	}

	/// Replace the current turn's active memory pack without writing it to the
	/// session log. The message is runtime context, not conversation history.
	pub fn set_active_memory_pack(&mut self, pack: Option<String>) {
		self.remove_active_memory_pack_message();
		self.active_memory_pack = pack.filter(|value| !value.trim().is_empty());
	}

	/// Re-materialize the runtime pack if compression or error cleanup removed its
	/// transient message. At most one copy may exist in the live request context.
	pub fn ensure_active_memory_pack_message(&mut self) {
		if self
			.session
			.messages
			.iter()
			.any(Self::is_active_memory_message)
		{
			return;
		}
		let Some(pack) = self.active_memory_pack.as_deref() else {
			return;
		};
		let content = crate::session::ensure_system_managed(pack);
		let mut message = crate::session::Session::build_message("user", &content);
		message.name = Some(Self::ACTIVE_MEMORY_MESSAGE_NAME.to_string());
		self.session.messages.push(message);
	}

	/// Remove only the materialized request copy; keep `active_memory_pack` so a
	/// tool follow-up can inject the same pack into its next specialist request.
	pub fn remove_active_memory_pack_message(&mut self) {
		self.session
			.messages
			.retain(|message| !Self::is_active_memory_message(message));
	}

	pub fn clear_active_memory_pack(&mut self) {
		self.remove_active_memory_pack_message();
		self.active_memory_pack = None;
		self.recalled_refs.clear();
		self.used_memory_ids.clear();
	}

	// Sync runtime state from ChatSession fields to session.info (for persistence)
	fn sync_runtime_state(&mut self) {
		self.session.info.role = self.role.clone();
		self.session.info.cache_next_user_message = self.cache_next_user_message;
		self.session.info.spending_threshold_checkpoint = self.spending_threshold_checkpoint;
		// Snapshot the verify-gate's ground truth so a resume restores the
		// still-open turn's recorded actions instead of an empty ledger.
		self.session.info.evidence = self.evidence.clone();
	}

	// Save the session (syncs runtime state first)
	pub fn save(&mut self) -> Result<()> {
		self.sync_runtime_state();
		self.session.save()
	}

	// Check if spending threshold is exceeded and prompt user if needed
	pub fn check_spending_threshold(&mut self, config: &Config) -> Result<bool> {
		// If threshold is 0 or negative, feature is disabled
		if config.max_session_spending_threshold <= 0.0 {
			return Ok(true); // Continue without checking
		}

		let current_cost = self.session.info.total_cost;
		let threshold = config.max_session_spending_threshold;
		let cost_since_checkpoint = current_cost - self.spending_threshold_checkpoint;

		// Check if we've exceeded the threshold since last checkpoint
		if cost_since_checkpoint >= threshold {
			// In ACP/WebSocket mode stdout/stderr are reserved for protocol — auto-decline silently
			if crate::logging::tracing_setup::is_structured_output_mode() {
				return Ok(false);
			}

			use colored::*;
			use std::io::{self, Write};

			println!();
			println!(
				"{}",
				"⚠️  SPENDING THRESHOLD REACHED ⚠️".bright_yellow().bold()
			);
			println!(
				"{} ${:.5}",
				"Current session cost:".bright_cyan(),
				current_cost
			);
			println!("{} ${:.5}", "Threshold:".bright_cyan(), threshold);
			println!(
				"{} ${:.5}",
				"Cost since last checkpoint:".bright_cyan(),
				cost_since_checkpoint
			);
			println!();
			println!(
				"{}",
				"Continuing may result in additional charges.".bright_yellow()
			);

			// Auto-decline in non-interactive mode (run command, piped input, etc.)
			if !std::io::stdin().is_terminal() {
				println!(
					"{}",
					"Spending threshold reached but automatically declining in non-interactive mode. Stopping execution.".bright_red()
				);
				return Ok(false);
			}

			// Interactive mode - ask user for confirmation
			print!(
				"{}",
				"Do you want to continue? (y/N): ".bright_white().bold()
			);
			io::stdout().flush()?;

			let mut input = String::new();
			io::stdin().read_line(&mut input)?;
			let response = input.trim().to_lowercase();

			if response == "y" || response == "yes" {
				// User confirmed, reset checkpoint to current cost
				self.spending_threshold_checkpoint = current_cost;
				println!(
					"{}",
					"✓ Continuing session. Threshold checkpoint reset.".bright_green()
				);
				println!();
				Ok(true)
			} else {
				println!(
					"{}",
					"✗ Session cancelled by user due to spending threshold.".bright_red()
				);
				Ok(false)
			}
		} else {
			Ok(true) // Under threshold, continue
		}
	}

	// Check if request spending threshold is exceeded and stop execution if needed
	pub fn check_request_spending_threshold(&mut self, config: &Config) -> Result<bool> {
		// If threshold is 0 or negative, feature is disabled
		if config.max_request_spending_threshold <= 0.0 {
			return Ok(true); // Continue without checking
		}

		let current_cost = self.session.info.total_cost;
		let threshold = config.max_request_spending_threshold;
		let cost_since_request_start = current_cost - self.request_spending_checkpoint;

		// Check if we've exceeded the threshold since request start
		if cost_since_request_start >= threshold {
			// In ACP/WebSocket mode stdout/stderr are reserved for protocol — suppress UI output
			if !crate::logging::tracing_setup::is_structured_output_mode() {
				use colored::*;

				println!();
				println!(
					"{}",
					"⚠️  REQUEST SPENDING THRESHOLD EXCEEDED ⚠️"
						.bright_red()
						.bold()
				);
				println!(
					"{} ${:.5}",
					"Current request cost:".bright_cyan(),
					cost_since_request_start
				);
				println!("{} ${:.5}", "Threshold:".bright_cyan(), threshold);
				println!(
					"{} ${:.5}",
					"Total session cost:".bright_cyan(),
					current_cost
				);
				println!();
				println!(
					"{}",
					"Request execution stopped to prevent overspending.".bright_red()
				);
				println!();
			}

			return Ok(false); // Stop execution
		}

		Ok(true) // Under threshold, continue
	}

	// Initialize request spending checkpoint at the start of a new request
	pub fn start_request_spending_tracking(&mut self) {
		self.request_spending_checkpoint = self.session.info.total_cost;
	}

	// Write the initial SUMMARY entry the first time we touch the session file.
	// Called before the first message write so the file always starts with metadata.
	fn ensure_file_initialized(&mut self) -> Result<()> {
		if let Some(session_file) = &self.session.session_file {
			if !session_file.exists() {
				let summary_entry =
					crate::session::persistence::summary_log_entry(&self.session.info);
				let session_file = session_file.clone();
				crate::session::append_to_session_file(
					&session_file,
					&serde_json::to_string(&summary_entry)?,
				)?;
			}
		}
		Ok(())
	}
	// Add a system message
	pub fn add_system_message(&mut self, content: &str) -> Result<()> {
		// Lazily create the session file with its initial SUMMARY on first write
		// Must happen BEFORE logger which also writes to the same file
		self.ensure_file_initialized()?;

		// ATOMIC ADD: persist FIRST, push only on success.
		let message = crate::session::Session::build_message("system", content);
		if let Some(session_file) = &self.session.session_file {
			let message_json = serde_json::to_string(&message)?;
			crate::session::append_to_session_file(session_file, &message_json)?;
		}
		self.session.messages.push(message);

		Ok(())
	}

	// Add a user message
	pub fn add_user_message(&mut self, content: &str) -> Result<()> {
		let images = self.take_pending_image().into_iter().collect();
		let videos = self.take_pending_video().into_iter().collect();
		self.add_user_message_with_attachments(content, images, videos)
	}

	/// Add a user message with attachments already loaded by a transport.
	/// This keeps multi-attachment WebSocket turns on the same atomic persistence
	/// path as interactive pending attachments.
	pub fn add_user_message_with_attachments(
		&mut self,
		content: &str,
		images: Vec<crate::session::image::ImageAttachment>,
		videos: Vec<crate::session::video::VideoAttachment>,
	) -> Result<()> {
		// Build the message in full WITHOUT pushing, attach image/video media,
		// persist it, and only THEN push to the in-memory Vec. This keeps memory
		// and disk strictly in sync — a persist failure leaves no orphan message.
		let mut message = crate::session::Session::build_message("user", content);

		if !images.is_empty() {
			message.images = Some(images);
			if !crate::logging::tracing_setup::is_structured_output_mode() {
				println!("{}", "📎 Image attached to message".bright_green());
			}
		}

		if !videos.is_empty() {
			message.videos = Some(videos);
			if !crate::logging::tracing_setup::is_structured_output_mode() {
				println!("{}", "🎬 Video attached to message".bright_green());
			}
		}

		// ATOMIC ADD: persist FIRST, push only on success.
		// Cache marker is applied AFTER push (it mutates the in-memory message and
		// may demote older markers / reset token counters — those mutations are
		// purely in-memory and do not need to be reflected in the persisted JSON
		// line, since cache state is derived per-request from the session struct).
		if let Some(session_file) = &self.session.session_file {
			let message_json = serde_json::to_string(&message)?;
			crate::session::append_to_session_file(session_file, &message_json)?;
		}
		self.session.messages.push(message);
		self.begin_turn_timing();
		// A genuine turn gets a freshly retrieved pack. Clear the prior runtime
		// pack only after persistence succeeds, preserving the atomic-add contract.
		self.clear_active_memory_pack();
		// A genuine user turn starts a new adaptive-compression phase. Preserve
		// the exact post-compression watermark, but reset autonomous runway
		// expansion so this request gets the normal short safety horizon. Keeping
		// this at the shared insertion boundary makes CLI, ACP, and WebSocket
		// behavior identical; system-managed user-role injections use a different
		// method and intentionally do not reset it.
		self.session.info.consecutive_compressions = 0;
		self.session.info.note_turn_start();
		self.learning_outcome = crate::supervisor::learning::TrajectoryOutcome::Unknown;

		// This response is owned by a genuine user turn, so a `done` report may
		// be verified against the task that was just added.
		self.completion_gate_eligible = true;
		self.gate_deferred = false;
		self.steer_attempt = 0;
		self.steer_last_signal = crate::supervisor::detect::DetectorSignal::None;
		self.last_steered_calls = None;
		// New genuine task: the verify-gate's evidence ledger starts fresh (gate and
		// steer re-runs arrive as system-managed messages and keep accumulating), and
		// the gate's per-turn re-entry budget resets — a previous turn's exhaustion
		// must not latch the gate off for the rest of the session. `gate_failed` is
		// deliberately NOT reset: it labels the trajectory for distill and is cleared
		// only by a later PASS.
		self.evidence.reset();
		self.gate_task = None;
		self.gate_iterations = 0;
		self.nudge_iterations = 0;
		self.last_gate_gaps.clear();
		self.pending_plan_signal = None;
		self.plan_evaluated = false;
		self.planner_failed = false;
		self.plan_evidence_checkpoint = 0;
		// New genuine task: the turn-answer ledger starts fresh — the previous
		// turn's deliverable must not pad this turn's verification.
		self.turn_answers.clear();
		// Reset per-task detector state (loop / no-progress / truncation / dedup /
		// drift streaks and the prior task's unverified-mutation latch).
		self.detectors.reset_streak();
		// Keep bounded analysis findings across real user turns. A new turn may be
		// a continuation ("continue", a correction, or an answer), so clearing
		// here loses load-bearing context before task resolution exists. The next
		// compaction ranks the bounded store against the live task and naturally
		// evicts stale focus.
		// Drop any subagent verdict that outlived its turn (a background run that
		// reported after the parent's last tool round). It vouches for the PREVIOUS
		// task's tree, so letting it reach this turn's first round fold could clear
		// a change it never saw.
		let _ = crate::supervisor::delegate::take_handback();

		// Check if we should cache this user message (after push, so the message exists
		// at a known index and the cache manager can enforce the 2-marker limit).
		if self.cache_next_user_message {
			let supports_caching = crate::session::model_supports_caching(&self.session.info.model);
			if supports_caching {
				let cache_manager = crate::session::cache::CacheManager::new();
				if let Ok(true) = cache_manager
					.apply_cache_to_current_user_message(&mut self.session, supports_caching)
				{
					if !crate::logging::tracing_setup::is_structured_output_mode() {
						use colored::*;
						println!(
							"{}",
							"✓ Current user message marked for caching".bright_green()
						);
					}
				}
			}
			// Reset the flag after applying (or attempting to apply) cache
			self.cache_next_user_message = false;
		}

		Ok(())
	}

	// Add a system-managed user-role message. Provider APIs still see role=user,
	// but task/compression/learning logic must not treat it as a user request —
	// enforced here: unmarked content is wrapped so it always classifies as
	// system-managed (see `ensure_system_managed`).
	pub fn add_system_managed_user_message(&mut self, content: &str) -> Result<()> {
		let content = crate::session::ensure_system_managed(content);
		let message = crate::session::Session::build_message("user", &content);
		if let Some(session_file) = &self.session.session_file {
			let message_json = serde_json::to_string(&message)?;
			crate::session::append_to_session_file(session_file, &message_json)?;
		}
		self.session.messages.push(message);

		Ok(())
	}

	/// Add a system-managed message that starts a new externally-triggered AI
	/// response. Unlike supervisor/recall notes injected *inside* a user turn,
	/// this message does not own the latest human task and cannot complete it.
	pub fn add_system_managed_turn_message(&mut self, content: &str) -> Result<()> {
		self.add_system_managed_user_message(content)?;
		self.abandon_turn_timing();
		// A turn that ended with session-owned work in flight hands its open task
		// to the delivery that resumes it; every other control-plane turn owns none.
		self.completion_gate_eligible = std::mem::take(&mut self.gate_deferred);
		Ok(())
	}

	// Add a tool message
	pub fn add_tool_message(
		&mut self,
		content: &str,
		tool_call_id: &str,
		tool_name: &str,
		_config: &Config,
	) -> Result<()> {
		// Tool result content is persisted as a Message JSON below; no separate log entry.
		// Create the tool message
		let tool_message = crate::session::Message {
			role: "tool".to_string(),
			content: content.to_string(),
			timestamp: std::time::SystemTime::now()
				.duration_since(std::time::UNIX_EPOCH)
				.unwrap_or_default()
				.as_secs(),
			cached: false,
			tool_call_id: Some(tool_call_id.to_string()),
			name: Some(tool_name.to_string()),
			..Default::default()
		};

		// ATOMIC ADD: persist BEFORE pushing to in-memory Vec and updating token counters.
		// A partial failure (one tool_result persisted, next ENOSPC) must not leave a
		// pushed-but-unpersisted tool_message in memory — that would create orphaned
		// tool_use blocks for Anthropic on the next request.
		if let Some(session_file) = &self.session.session_file {
			let message_json = serde_json::to_string(&tool_message)?;
			crate::session::append_to_session_file(session_file, &message_json)?;
		}
		self.session.messages.push(tool_message);

		// Update token tracking for auto-cache threshold logic
		// Tool messages count as "input" for the next API call, so we track them as non-cached input tokens
		let tool_content_tokens = crate::session::estimate_tokens(content) as u64;
		let tool_overhead_tokens = 8; // Rough estimate for role + tool_call_id + name overhead

		// Update the session's current token tracking
		// This ensures tool message tokens are counted toward auto-cache thresholds
		// Tool messages are input tokens (they go to the API as input), not output tokens
		let tool_input_tokens = tool_content_tokens + tool_overhead_tokens;
		self.session.info.current_total_tokens += tool_input_tokens;
		self.session.info.current_non_cached_tokens += tool_input_tokens;

		Ok(())
	}

	// Add an assistant message
	pub fn add_assistant_message(
		&mut self,
		content: &str,
		exchange: Option<ProviderExchange>,
		config: &Config,
		role: &str,
	) -> Result<()> {
		// ATOMIC ADD: build, persist, then push. If persist fails, `?` propagates with
		// clean memory state — no orphaned assistant message and no token/cost
		// bookkeeping side-effects.
		let message = crate::session::Session::build_message("assistant", content);
		if let Some(session_file) = &self.session.session_file {
			let message_json = serde_json::to_string(&message)?;
			crate::session::append_to_session_file(session_file, &message_json)?;
		}
		self.session.messages.push(message);
		self.last_response = content.to_string();
		// Turn-answer ledger: a final (no tool calls) joins the turn's deliverable.
		if !content.trim().is_empty() {
			self.turn_answers.push(content.to_string());
		}

		// Update token counts and estimated costs if we have usage data
		if let Some(ex) = &exchange {
			if let Some(usage) = &ex.usage {
				// Track API time if available
				if let Some(api_time_ms) = usage.request_time_ms {
					self.session.info.total_api_time_ms += api_time_ms;
				}

				// CACHE-AWARE COMPRESSION: Track API calls for amortized cost analysis
				// Each API call = potential cache write/read, critical for compression economics
				self.session.info.total_api_calls += 1;

				// Update session token counts using octolib data directly
				let cache_manager = crate::session::cache::CacheManager::new();
				cache_manager.update_token_tracking(
					&mut self.session,
					usage.input_tokens, // Non-cached input tokens from API
					usage.output_tokens,
					usage.cache_read_tokens,
					usage.cache_write_tokens,
					usage.reasoning_tokens,
				);

				// Check if we should automatically move the cache marker
				let cache_manager = crate::session::cache::CacheManager::new();
				let supports_caching =
					crate::session::model_supports_caching(&self.session.info.model);
				if let Ok(true) = cache_manager.check_and_apply_auto_cache_threshold(
					&mut self.session,
					config,
					supports_caching,
					role,
				) {
					log_info!(
						"{}",
						"Auto-cache threshold reached - cache checkpoint applied."
					);
				}

				let raw_cost = ex
					.response
					.get("usage")
					.and_then(|value| value.get("cost"))
					.and_then(|value| value.as_f64());
				let (cost, cost_source) = match (usage.cost, raw_cost) {
					(Some(cost), _) => (Some(cost), "normalized"),
					(None, Some(cost)) => (Some(cost), "raw"),
					(None, None) => (None, "unreported"),
				};
				if let Some(cost) = cost {
					self.session.info.total_cost += cost;
					self.estimated_cost = self.session.info.total_cost;
				}
				let cost_summary = cost
					.map(|value| format!("${value:.5} ({cost_source})"))
					.unwrap_or_else(|| cost_source.to_string());
				log_debug!(
					"Provider usage [message]: provider={}, input={}, output={}, cache_read={}, cache_write={}, reasoning={}, cost={}, session_total=${:.5}",
					ex.provider,
					usage.input_tokens,
					usage.output_tokens,
					usage.cache_read_tokens,
					usage.cache_write_tokens,
					usage.reasoning_tokens,
					cost_summary,
					self.session.info.total_cost
				);

				// Update session duration
				let current_time = std::time::SystemTime::now()
					.duration_since(std::time::UNIX_EPOCH)
					.unwrap_or_default()
					.as_secs();
				let start_time = self.session.info.created_at;
				self.session.info.duration_seconds = current_time - start_time;
			}
		}

		// (Persistence happened at the top of this function — atomic add.)

		Ok(())
	}
}

#[cfg(test)]
#[path = "messages_tests.rs"]
mod tests;
