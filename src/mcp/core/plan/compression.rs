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

//! Plan-driven context compression
//!
//! This module implements autonomous context compression triggered by plan task completion.
//! When a task is completed via plan(next), the session history is compressed by:
//! 1. Removing detailed tool calls and intermediate work
//! 2. Injecting a structured summary of what was accomplished
//! 3. Tracking compression metrics for reporting

use super::storage::{MessageRange, PlanTask};
use crate::session::chat::session::ChatSession;
use crate::session::context::SessionId;
use crate::session::estimate_tokens;
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

// ---------------------------------------------------------------------------
// Session-scoped compression registries
// ---------------------------------------------------------------------------
// Each session gets its own pending compression state. CLI mode uses a
// dedicated "cli" key. The pattern mirrors context.rs session-keyed registries.

/// Session-keyed pending task compressions.
static PENDING_COMPRESSIONS: RwLock<Option<HashMap<SessionId, PendingTaskCompression>>> =
	RwLock::new(None);

/// Session-keyed pending phase compressions.
static PENDING_PHASE_COMPRESSIONS: RwLock<Option<HashMap<SessionId, PhaseCompressionRequest>>> =
	RwLock::new(None);

/// Session-keyed pending project compressions.
static PENDING_PROJECT_COMPRESSIONS: RwLock<Option<HashMap<SessionId, ProjectCompressionRequest>>> =
	RwLock::new(None);

// CLI fallback globals — used only when not in a session context.
lazy_static::lazy_static! {
	static ref CLI_PENDING_COMPRESSION: Arc<Mutex<Option<PendingTaskCompression>>> = Arc::new(Mutex::new(None));
	static ref CLI_PENDING_PHASE_COMPRESSION: Arc<Mutex<Option<PhaseCompressionRequest>>> = Arc::new(Mutex::new(None));
	static ref CLI_PENDING_PROJECT_COMPRESSION: Arc<Mutex<Option<ProjectCompressionRequest>>> = Arc::new(Mutex::new(None));
}

/// Get the effective session key, or None for CLI fallback.
fn effective_session_id() -> Option<SessionId> {
	crate::session::context::current_session_id()
}

/// Clean up all pending compression state for a session.
/// Called from context.rs cleanup_session().
pub fn cleanup_compression_state(session_id: &SessionId) {
	if let Ok(mut guard) = PENDING_COMPRESSIONS.write() {
		if let Some(registry) = guard.as_mut() {
			registry.remove(session_id);
		}
	}
	if let Ok(mut guard) = PENDING_PHASE_COMPRESSIONS.write() {
		if let Some(registry) = guard.as_mut() {
			registry.remove(session_id);
		}
	}
	if let Ok(mut guard) = PENDING_PROJECT_COMPRESSIONS.write() {
		if let Some(registry) = guard.as_mut() {
			registry.remove(session_id);
		}
	}
}

/// Wrapper for pending task compression with force flag
#[derive(Debug, Clone)]
struct PendingTaskCompression {
	task: PlanTask,
	/// When true, bypass the 20% minimum context fraction threshold.
	/// Used by plan(done) to ensure the final task gets compressed.
	force: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhaseCompression {
	pub phase_name: String,
	pub task_range: (usize, usize),
	pub summary: String,
	pub compressed_at: chrono::DateTime<chrono::Utc>,
	pub message_range: MessageRange,
	pub metrics: CompressionMetrics,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectCompression {
	pub summary: String,
	pub compressed_at: chrono::DateTime<chrono::Utc>,
	pub message_range: MessageRange,
	pub metrics: CompressionMetrics,
	pub total_tasks: usize,
	pub total_phases: usize,
}

#[derive(Debug, Clone)]
pub struct PhaseCompressionRequest {
	pub phase_name: String,
	pub task_range: (usize, usize),
	pub summary: String,
	pub message_range: Option<MessageRange>,
}

#[derive(Debug, Clone)]
pub struct ProjectCompressionRequest {
	pub plan_title: String,
	pub summary: String,
	pub total_tasks: usize,
	pub total_phases: usize,
	pub message_range: Option<MessageRange>,
}

/// Request compression for a completed task
/// This is called by the plan tool when a task is completed via plan(next)
pub fn request_compression(task: PlanTask) {
	crate::log_debug!("Compression requested for task: {}", task.title);
	let ptc = PendingTaskCompression { task, force: false };
	if let Some(session_id) = effective_session_id() {
		let mut guard = PENDING_COMPRESSIONS.write().unwrap();
		let registry = guard.get_or_insert_with(HashMap::new);
		registry.insert(session_id, ptc);
	} else {
		let mut pending = CLI_PENDING_COMPRESSION.lock().unwrap();
		*pending = Some(ptc);
	}
}

/// Request forced compression for a completed task (bypasses 20% threshold)
/// Called by plan(done) to ensure the final task is compressed before project compression
pub fn request_forced_compression(task: PlanTask) {
	crate::log_debug!("Forced compression requested for task: {}", task.title);
	let ptc = PendingTaskCompression { task, force: true };
	if let Some(session_id) = effective_session_id() {
		let mut guard = PENDING_COMPRESSIONS.write().unwrap();
		let registry = guard.get_or_insert_with(HashMap::new);
		registry.insert(session_id, ptc);
	} else {
		let mut pending = CLI_PENDING_COMPRESSION.lock().unwrap();
		*pending = Some(ptc);
	}
}

/// Set message range on the pending compression task
/// This is called by the session after detecting a plan tool execution
///
/// # Parameters
///
/// - `start_index`: Index of the message to preserve (exclusive - this message stays)
/// - `end_index`: Last message index to remove (inclusive - this message is removed)
///
/// # Index Constraints (CRITICAL)
///
/// - `end_index` must be **< messages.len()** (use `get_message_count() - 1` for last message)
/// - `end_index >= messages.len()` will cause compression to fail with "Invalid end_index"
/// - Uses inclusive range removal, so valid indices are `0..messages.len()-1`
///
/// # Example
///
/// For 93 messages (indices 0-92):
/// - CORRECT: `set_pending_compression_range(10, 92);` // Compress to last message
pub fn set_pending_compression_range(start_index: usize, end_index: usize) -> Result<()> {
	// When start >= end, there are 0 messages to compress (the drain range start+1..=end
	// is empty). This is a valid scenario — e.g. a task with no tool calls — not an error.
	// Clear the pending compression so process_pending_compression doesn't fail later.
	if start_index >= end_index {
		crate::log_debug!(
			"Compression range is empty (start={}, end={}) — nothing to compress, skipping",
			start_index,
			end_index
		);

		// Remove the pending compression so it doesn't fail with "no message range" later
		if let Some(session_id) = effective_session_id() {
			let mut guard = PENDING_COMPRESSIONS.write().unwrap();
			if let Some(registry) = guard.as_mut() {
				registry.remove(&session_id);
			}
		} else {
			let mut pending = CLI_PENDING_COMPRESSION.lock().unwrap();
			*pending = None;
		}

		return Ok(());
	}

	let range = MessageRange {
		start_index,
		end_index,
	};

	if let Some(session_id) = effective_session_id() {
		let mut guard = PENDING_COMPRESSIONS.write().unwrap();
		if let Some(registry) = guard.as_mut() {
			if let Some(ptc) = registry.get_mut(&session_id) {
				ptc.task.message_range = Some(range);
				crate::log_debug!(
					"Compression range set: {} to {} for task '{}'",
					start_index,
					end_index,
					ptc.task.title
				);
				return Ok(());
			}
		}
		Err(anyhow!("No pending compression to set range for"))
	} else {
		let mut pending = CLI_PENDING_COMPRESSION.lock().unwrap();
		if let Some(ref mut ptc) = *pending {
			ptc.task.message_range = Some(range);
			crate::log_debug!(
				"Compression range set: {} to {} for task '{}'",
				start_index,
				end_index,
				ptc.task.title
			);
			Ok(())
		} else {
			Err(anyhow!("No pending compression to set range for"))
		}
	}
}

/// Check if there's a pending compression request and execute it
/// This is called from the session response processing after tool execution
pub async fn process_pending_compression(
	session: &mut ChatSession,
) -> Result<Option<CompressionMetrics>> {
	let ptc = if let Some(session_id) = effective_session_id() {
		let mut guard = PENDING_COMPRESSIONS.write().unwrap();
		guard
			.as_mut()
			.and_then(|registry| registry.remove(&session_id))
	} else {
		let mut pending = CLI_PENDING_COMPRESSION.lock().unwrap();
		pending.take()
	};

	if let Some(ptc) = ptc {
		crate::log_debug!(
			"Processing pending compression for task: {} (force: {})",
			ptc.task.title,
			ptc.force
		);
		let phase = format!("Compressing task ({})…", ptc.task.title);
		crate::session::chat::animation_manager::get_animation_manager()
			.set_phase(&phase)
			.await;
		let result = compress_completed_task(session, &ptc.task, ptc.force).await;
		crate::session::chat::animation_manager::get_animation_manager().clear_phase();
		result
	} else {
		Ok(None)
	}
}

/// Check if there's a pending compression request
pub fn has_pending_compression() -> bool {
	if let Some(session_id) = effective_session_id() {
		let guard = PENDING_COMPRESSIONS.read().unwrap();
		guard
			.as_ref()
			.map(|r| r.contains_key(&session_id))
			.unwrap_or(false)
	} else {
		CLI_PENDING_COMPRESSION.lock().unwrap().is_some()
	}
}

/// Request phase compression
pub fn request_phase_compression(phase_name: String, task_range: (usize, usize), summary: String) {
	crate::log_debug!(
		"Phase compression requested: {} (tasks {}-{})",
		phase_name,
		task_range.0 + 1,
		task_range.1 + 1
	);
	let req = PhaseCompressionRequest {
		phase_name,
		task_range,
		summary,
		message_range: None,
	};
	if let Some(session_id) = effective_session_id() {
		let mut guard = PENDING_PHASE_COMPRESSIONS.write().unwrap();
		let registry = guard.get_or_insert_with(HashMap::new);
		registry.insert(session_id, req);
	} else {
		let mut pending = CLI_PENDING_PHASE_COMPRESSION.lock().unwrap();
		*pending = Some(req);
	}
}

/// Request project compression
pub fn request_project_compression(
	plan_title: String,
	summary: String,
	total_tasks: usize,
	total_phases: usize,
) {
	crate::log_debug!(
		"Project compression requested: {} ({} tasks, {} phases)",
		plan_title,
		total_tasks,
		total_phases
	);
	let req = ProjectCompressionRequest {
		plan_title,
		summary,
		total_tasks,
		total_phases,
		message_range: None,
	};
	if let Some(session_id) = effective_session_id() {
		let mut guard = PENDING_PROJECT_COMPRESSIONS.write().unwrap();
		let registry = guard.get_or_insert_with(HashMap::new);
		registry.insert(session_id, req);
	} else {
		let mut pending = CLI_PENDING_PROJECT_COMPRESSION.lock().unwrap();
		*pending = Some(req);
	}
}

/// Check if there's a pending phase compression request
pub fn has_pending_phase_compression() -> bool {
	if let Some(session_id) = effective_session_id() {
		let guard = PENDING_PHASE_COMPRESSIONS.read().unwrap();
		guard
			.as_ref()
			.map(|r| r.contains_key(&session_id))
			.unwrap_or(false)
	} else {
		CLI_PENDING_PHASE_COMPRESSION.lock().unwrap().is_some()
	}
}

/// Check if there's a pending project compression request
pub fn has_pending_project_compression() -> bool {
	if let Some(session_id) = effective_session_id() {
		let guard = PENDING_PROJECT_COMPRESSIONS.read().unwrap();
		guard
			.as_ref()
			.map(|r| r.contains_key(&session_id))
			.unwrap_or(false)
	} else {
		CLI_PENDING_PROJECT_COMPRESSION.lock().unwrap().is_some()
	}
}

/// Get the current compression ID for logging/tracking
/// Uses full nanosecond timestamp for uniqueness
pub fn get_compression_id() -> Option<String> {
	let now = std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.unwrap_or_default();

	Some(format!(
		"comp_{}_{}",
		now.as_millis(),
		now.as_nanos() // Full nanoseconds for uniqueness
	))
}

/// Metrics tracking compression effectiveness
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressionMetrics {
	pub messages_removed: usize,
	pub tokens_saved: u64,
	pub compression_ratio: f64, // Ratio of tokens saved to original tokens
}

impl CompressionMetrics {
	pub fn new(messages_removed: usize, tokens_saved: u64, original_tokens: u64) -> Self {
		let compression_ratio = if original_tokens > 0 {
			tokens_saved as f64 / original_tokens as f64
		} else {
			0.0
		};

		Self {
			messages_removed,
			tokens_saved,
			compression_ratio,
		}
	}
}

fn adjusted_task_compression_start_index(
	messages: &[crate::session::Message],
	start_index: usize,
	end_index: usize,
) -> Option<usize> {
	if start_index >= messages.len() || end_index >= messages.len() || start_index >= end_index {
		return None;
	}

	let anchor = messages.get(start_index)?;

	// Case 1: start_index is an assistant with tool_calls. Whatever we return
	// as adjusted_start becomes the LAST kept message in the prefix, and the
	// inserted compression summary takes the next slot. An asst_with_tool_calls
	// cannot legally be followed by another assistant message (Anthropic
	// requires the immediate next message to be a tool_result), so we MUST
	// advance past this asst together with all its tool_results. If there are
	// no consecutive tool_results to skip past (the asst was already orphan
	// before compression — e.g. its results were drained by an earlier cycle)
	// or the tool run extends past end_index, skip compression rather than
	// inserting the summary right after the orphan and corrupting the stream.
	if anchor.role == "assistant" && anchor.tool_calls.is_some() {
		let mut next = start_index + 1;
		while next <= end_index && messages[next].role == "tool" {
			next += 1;
		}

		return if next > start_index + 1 && next <= end_index {
			Some(next)
		} else {
			None
		};
	}

	// Case 2: start_index is a tool_result whose parent assistant (somewhere
	// before start_index, past any earlier tool_results) has tool_calls. The
	// drain removes start_index+1..=end, which may include sibling tool_results
	// for the same assistant. Walk back past consecutive tool messages to find
	// the parent, then advance forward past ALL consecutive tool messages from
	// start_index so no sibling results are split by the drain boundary.
	if anchor.role == "tool" {
		let mut parent_idx = start_index;
		while parent_idx > 0 && messages[parent_idx - 1].role == "tool" {
			parent_idx -= 1;
		}
		if parent_idx > 0 {
			let parent = &messages[parent_idx - 1];
			if parent.role == "assistant" && parent.tool_calls.is_some() {
				let mut next = start_index + 1;
				while next <= end_index && messages[next].role == "tool" {
					next += 1;
				}

				return if next <= end_index { Some(next) } else { None };
			}
		}
	}

	Some(start_index)
}

/// Compress session history for a completed task
///
/// This function:
/// 1. Extracts the task summary
/// 2. Formats it as structured knowledge
/// 3. Removes messages in the task's range
/// 4. Injects the compressed summary
/// 5. Returns compression metrics
///
/// # Arguments
/// * `session` - Mutable reference to ChatSession
/// * `task` - The completed task with summary and message range
///
/// # Returns
/// CompressionMetrics with details about compression effectiveness
pub async fn compress_completed_task(
	session: &mut ChatSession,
	task: &PlanTask,
	force: bool,
) -> Result<Option<CompressionMetrics>> {
	// Validate task has required data (fail fast with clear error)
	let summary = task
		.summary
		.as_ref()
		.ok_or_else(|| anyhow!("Task has no summary - cannot compress"))?;

	let message_range = task
		.message_range
		.as_ref()
		.ok_or_else(|| anyhow!("Task has no message range - cannot compress"))?;

	crate::log_debug!(
		"Compressing task '{}' (messages {}-{})",
		task.title,
		message_range.start_index,
		message_range.end_index
	);

	let Some(adjusted_start) = adjusted_task_compression_start_index(
		&session.session.messages,
		message_range.start_index,
		message_range.end_index,
	) else {
		crate::log_info!(
			"Task compression skipped: no safe messages to remove after preserving anchor tool results."
		);
		return Ok(None);
	};

	if adjusted_start != message_range.start_index {
		crate::log_debug!(
			"Task compression: advancing start_index past anchor tool results ({} -> {})",
			message_range.start_index,
			adjusted_start
		);
	}

	let adjusted_range = MessageRange {
		start_index: adjusted_start,
		end_index: message_range.end_index,
	};

	// Calculate tokens before compression (for metrics)
	let tokens_before = calculate_range_tokens(session, &adjusted_range)?;

	// Get compression ID for tracking
	let compression_id = get_compression_id().unwrap_or_else(|| "unknown".to_string());

	// Get active plan context to preserve state after compression
	let plan_context = super::core::get_plan_context();

	// Extract file references from tool calls in the message range
	// These allow the model to re-read critical files after compression
	let file_refs = extract_file_refs_from_messages(
		&session.session.messages[adjusted_range.start_index..=adjusted_range.end_index],
	);

	// Build the AnchorUpdate for this compaction. Heuristic for now: the
	// task summary becomes one `changes_made` entry, file_refs come from
	// tool-call extraction, and `intent` is set on the first compaction
	// from the task description (first-write-wins). Future work can
	// replace this with an LLM-generated update for richer
	// decisions/errors_seen content — the call shape stays the same.
	let now_unix = std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.unwrap_or_default()
		.as_secs();
	let anchor_intent = if session.session.info.anchor.intent.is_empty() {
		Some(task.description.clone())
	} else {
		None
	};
	let anchor_update = crate::session::anchor::AnchorUpdate {
		intent: anchor_intent,
		changes_made: vec![format!("{}: {}", task.title, summary)],
		file_refs: file_refs.clone(),
		..Default::default()
	};

	// Project the anchor forward without committing — we only mutate
	// session state if compaction actually succeeds (skip checks below).
	let mut projected_anchor = session.session.info.anchor.clone();
	projected_anchor.extend(anchor_update, now_unix);

	// Create compressed knowledge entry with validated summary and the
	// projected anchor snapshot embedded for cross-compaction continuity.
	let compressed_entry = format_compressed_summary(
		task,
		summary,
		&compression_id,
		plan_context.as_ref(),
		&file_refs,
		&projected_anchor,
	);

	// Calculate tokens in compressed entry
	let tokens_after = estimate_tokens(&compressed_entry) as u64;

	// Skip compression if it doesn't reduce tokens
	if tokens_after >= tokens_before {
		crate::log_info!(
			"Task compression skipped: {} tokens before, {} tokens after (no savings).",
			tokens_before,
			tokens_after
		);
		return Ok(None);
	}

	// Skip compression if the range is too small relative to total context.
	// Compressing <20% of context produces negligible savings and risks losing
	// important detail for a trivial gain (e.g. 2% reduction is not worth it).
	// Bypassed when force=true (plan done — plan is finished, aggressive compression is appropriate).
	if !force {
		const MIN_CONTEXT_FRACTION: f64 = 0.20;
		let total_session_tokens =
			crate::session::estimate_session_tokens(&session.session.messages) as f64;
		if total_session_tokens > 0.0 {
			let range_fraction = tokens_before as f64 / total_session_tokens;
			if range_fraction < MIN_CONTEXT_FRACTION {
				crate::log_info!(
					"Task compression skipped: range is {:.1}% of context ({} / {} tokens) — below 20% threshold.",
					range_fraction * 100.0,
					tokens_before,
					total_session_tokens as u64
				);
				return Ok(None);
			}
		}
	}

	let (messages_removed, _) =
		session.remove_messages_in_range(adjusted_range.start_index, adjusted_range.end_index)?;

	// Insert compressed summary (compressed block is always cached=true — new stable boundary)
	session.insert_compressed_knowledge(adjusted_range.start_index, compressed_entry)?;

	// Calculate metrics
	let tokens_saved = tokens_before.saturating_sub(tokens_after);
	let metrics = CompressionMetrics::new(messages_removed, tokens_saved, tokens_before);

	crate::log_debug!(
		"Compression complete: {} messages removed, {} tokens saved ({:.1}% reduction)",
		metrics.messages_removed,
		metrics.tokens_saved,
		metrics.compression_ratio * 100.0
	);

	// CRITICAL: Log compression point to session file
	// This marker tells session loader to clear messages before this point on resume
	let _ = crate::session::logger::log_compression_point(
		&session.session.info.name,
		"task",
		messages_removed,
		tokens_saved,
		&session.session.messages,
	);

	// Commit the projected anchor — task compaction succeeded, so the
	// update is now part of session state and will be embedded in
	// subsequent compressed-knowledge messages.
	session.session.info.anchor = projected_anchor;
	// (dedup state is cleared inside `remove_messages_in_range` above —
	// see core.rs for rationale.)

	// CRITICAL FIX: Reset token tracking for fresh start after compression
	// This prevents token drift and ensures accurate cache/pricing calculations
	session.session.info.current_non_cached_tokens = 0;
	session.session.info.current_total_tokens = 0;

	// Reset cache checkpoint time
	session.session.info.last_cache_checkpoint_time = std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.unwrap_or_default()
		.as_secs();

	// CRITICAL: Clear start_index ONLY after successful compression
	// This allows multiple tasks to accumulate if compression keeps getting skipped
	// When compression is skipped (would add tokens), start_index stays the same,
	// so the next task will include all accumulated work in its compression range
	super::core::clear_task_start_index();
	crate::log_debug!(
		"Cleared start_index after successful compression - next task will set new start_index"
	);

	Ok(Some(metrics))
}

/// Format compressed knowledge with the session anchor as the primary
/// content (extend-anchor pattern, replacing the previous regenerate-from-
/// scratch summary). The anchor already contains the per-task data folded
/// in by `compress_completed_task` before this function runs, so we don't
/// duplicate task title/description/summary as separate blocks.
///
/// Active plan state and the latest task identifier are kept as small
/// scaffolding so the model can recognize what was just compacted, but the
/// real continuity comes from the anchor itself: intent, decisions,
/// changes_made, file_refs, errors_seen, next_steps — accumulated across
/// every compaction in this session.
fn format_compressed_summary(
	task: &PlanTask,
	summary: &str,
	compression_id: &str,
	plan_context: Option<&(String, usize, usize, String)>,
	_file_refs: &[String],
	anchor: &crate::session::anchor::Anchor,
) -> String {
	let completed_at = task
		.completed_at
		.map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
		.unwrap_or_else(|| "Unknown".to_string());

	let mut output = format!(
		"<task_compressed id=\"{}\" task=\"{}\" completed_at=\"{}\">\n",
		compression_id,
		escape_attr(&task.title),
		escape_attr(&completed_at),
	);

	// Anchor is the primary content. It contains intent, decisions,
	// changes_made (which already includes this task's summary, folded in
	// before format runs), file_refs accumulated across compactions, and
	// any errors / next_steps recorded so far.
	if !anchor.is_empty() {
		output.push_str("<anchor>\n");
		output.push_str(&anchor.to_xml());
		output.push_str("</anchor>\n");
	} else {
		// Defensive fallback — should not happen because the caller
		// extends the anchor before calling format. Kept so the function
		// is correct even if invoked out of band (e.g. tests).
		output.push_str(&format!(
			"<task>{}</task>\n<summary>{}</summary>\n",
			task.title, summary
		));
	}

	// CRITICAL: Preserve active plan state so the model knows the plan is
	// still in progress and which task is current.
	if let Some((plan_title, completed_count, total_tasks, current_task_title)) = plan_context {
		output.push_str(&format!(
			"<active_plan title=\"{}\" progress=\"{}/{}\" status=\"in_progress\">\n\
			<current_task>{}</current_task>\n\
			</active_plan>\n",
			escape_attr(plan_title),
			completed_count,
			total_tasks,
			current_task_title,
		));
	}

	output.push_str("</task_compressed>");
	output
}

/// Escape characters that would break a double-quoted XML attribute value.
/// Used only for attribute embedding (titles, ids, timestamps); body text
/// stays unescaped since it's free-form prose the LLM tolerates as-is.
fn escape_attr(s: &str) -> String {
	s.replace('&', "&amp;")
		.replace('"', "&quot;")
		.replace('<', "&lt;")
		.replace('>', "&gt;")
}

/// Calculate total tokens in message range using accurate token counting
/// This now counts ALL message fields: content, tool_calls, thinking, images, etc.
fn calculate_range_tokens(session: &ChatSession, range: &MessageRange) -> Result<u64> {
	let mut total_tokens = 0u64;

	// Validate range
	if range.start_index >= session.session.messages.len() {
		return Err(anyhow::anyhow!("Invalid start_index in message range"));
	}

	if range.end_index >= session.session.messages.len() {
		return Err(anyhow::anyhow!("Invalid end_index in message range"));
	}

	// Count tokens in range (start_index+1 to end_index inclusive, matching message removal)
	// Use accurate token counting that includes tool_calls, thinking, images, etc.
	for i in (range.start_index + 1)..=range.end_index {
		if let Some(message) = session.session.messages.get(i) {
			let tokens = crate::session::estimate_message_tokens(message) as u64;
			total_tokens += tokens;
		}
	}

	Ok(total_tokens)
}

/// Process pending phase compression
pub async fn process_pending_phase_compression(
	session: &mut ChatSession,
) -> Result<Option<CompressionMetrics>> {
	let request = if let Some(session_id) = effective_session_id() {
		let mut guard = PENDING_PHASE_COMPRESSIONS.write().unwrap();
		guard
			.as_mut()
			.and_then(|registry| registry.remove(&session_id))
	} else {
		let mut pending = CLI_PENDING_PHASE_COMPRESSION.lock().unwrap();
		pending.take()
	};

	if let Some(req) = request {
		crate::log_debug!("Processing pending phase compression: {}", req.phase_name);
		let phase = format!("Compressing phase ({})…", req.phase_name);
		crate::session::chat::animation_manager::get_animation_manager()
			.set_phase(&phase)
			.await;
		let result = compress_phase(session, &req).await;
		crate::session::chat::animation_manager::get_animation_manager().clear_phase();
		result
	} else {
		Ok(None)
	}
}

/// Compress multiple task compressions into single phase summary
async fn compress_phase(
	session: &mut ChatSession,
	request: &PhaseCompressionRequest,
) -> Result<Option<CompressionMetrics>> {
	// Find all compressed task summaries for this phase
	let mut task_summaries = Vec::new();
	let mut start_index = None;
	let mut end_index = None;

	// Look for task compressions that belong to this phase
	for (i, msg) in session.session.messages.iter().enumerate() {
		if let Some(name) = &msg.name {
			if name == "plan_compression" && msg.content.contains("<task_compressed") {
				// Check if this task belongs to the phase (by checking content or just take all recent ones)
				task_summaries.push((i, msg.content.clone()));
				if start_index.is_none() {
					start_index = Some(i);
				}
				end_index = Some(i);
			}
		}
	}

	// If no phase specified (single-phase plan), compress all task summaries
	// If phase specified, we already filtered above
	if task_summaries.is_empty() {
		return Err(anyhow!("No task compressions found for phase compression"));
	}

	let start_idx = start_index.unwrap();
	let end_idx = end_index.unwrap();

	// CRITICAL: Use start_idx - 1 to include the FIRST task compression message in the range
	// calculate_range_tokens and remove_messages_in_range both use (start_index + 1)..=end_index
	// So to include message at start_idx, we need to pass start_idx - 1
	let range_start = if start_idx > 0 { start_idx - 1 } else { 0 };

	// Calculate tokens before compression
	let tokens_before = calculate_range_tokens(
		session,
		&MessageRange {
			start_index: range_start,
			end_index: end_idx,
		},
	)?;

	// Create phase summary
	let phase_summary =
		format_phase_summary(&request.phase_name, &request.summary, task_summaries.len());

	let tokens_after = estimate_tokens(&phase_summary) as u64;

	// Skip compression if it doesn't reduce tokens
	if tokens_after >= tokens_before {
		crate::log_info!(
			"Phase compression skipped: {} tokens before, {} tokens after (no savings).",
			tokens_before,
			tokens_after
		);
		return Ok(None);
	}

	// Remove all task compression messages in range (using range_start to include first task)
	let (messages_removed, _) = session.remove_messages_in_range(range_start, end_idx)?;

	// Insert phase summary (compressed block is always cached=true — new stable boundary)
	session.insert_compressed_knowledge(range_start, phase_summary)?;

	// Calculate metrics
	let tokens_saved = tokens_before.saturating_sub(tokens_after);
	let metrics = CompressionMetrics::new(messages_removed, tokens_saved, tokens_before);

	crate::log_debug!(
		"Phase compression '{}': {} task summaries → 1 phase summary, {} tokens saved",
		request.phase_name,
		task_summaries.len(),
		metrics.tokens_saved
	);

	// CRITICAL: Log compression point to session file
	let _ = crate::session::logger::log_compression_point(
		&session.session.info.name,
		"phase",
		messages_removed,
		tokens_saved,
		&session.session.messages,
	);

	// (dedup state is cleared inside `remove_messages_in_range` — see core.rs.)

	// CRITICAL FIX: Reset token tracking for fresh start after compression
	// This prevents token drift and ensures accurate cache/pricing calculations
	session.session.info.current_non_cached_tokens = 0;
	session.session.info.current_total_tokens = 0;

	// Reset cache checkpoint time
	session.session.info.last_cache_checkpoint_time = std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.unwrap_or_default()
		.as_secs();

	// CRITICAL: Clear start_index after successful phase compression
	// Phase compression consolidates multiple task compressions, so we reset the range
	super::core::clear_task_start_index();
	crate::log_debug!(
		"Cleared start_index after successful phase compression - next task will set new start_index"
	);

	Ok(Some(metrics))
}

fn format_phase_summary(phase_name: &str, summary: &str, task_count: usize) -> String {
	let compression_id = get_compression_id().unwrap_or_else(|| "unknown".to_string());
	format!(
		"<phase_compressed id=\"{}\" phase=\"{}\" task_count=\"{}\">\n\
		<summary>{}</summary>\n\
		</phase_compressed>",
		compression_id,
		escape_attr(phase_name),
		task_count,
		summary,
	)
}

/// Process pending project compression
pub async fn process_pending_project_compression(
	session: &mut ChatSession,
) -> Result<Option<CompressionMetrics>> {
	let request = if let Some(session_id) = effective_session_id() {
		let mut guard = PENDING_PROJECT_COMPRESSIONS.write().unwrap();
		guard
			.as_mut()
			.and_then(|registry| registry.remove(&session_id))
	} else {
		let mut pending = CLI_PENDING_PROJECT_COMPRESSION.lock().unwrap();
		pending.take()
	};

	if let Some(req) = request {
		crate::log_debug!("Processing pending project compression: {}", req.plan_title);
		let phase = format!("Compressing project ({})…", req.plan_title);
		crate::session::chat::animation_manager::get_animation_manager()
			.set_phase(&phase)
			.await;
		let result = compress_project(session, &req).await;
		crate::session::chat::animation_manager::get_animation_manager().clear_phase();
		result
	} else {
		Ok(None)
	}
}

/// Compress entire plan (all tasks + phases) into final project summary
async fn compress_project(
	session: &mut ChatSession,
	request: &ProjectCompressionRequest,
) -> Result<Option<CompressionMetrics>> {
	// Find all plan-related compression messages (both task and phase compressions)
	let mut compression_indices = Vec::new();

	for (i, msg) in session.session.messages.iter().enumerate() {
		if let Some(name) = &msg.name {
			if name == "plan_compression" {
				compression_indices.push(i);
			}
		}
	}

	if compression_indices.len() < 2 {
		crate::log_info!(
			"Project compression skipped: need at least 2 task/phase compressions to consolidate (found {})",
			compression_indices.len()
		);
		return Ok(None);
	}

	let start_idx = *compression_indices.first().unwrap();
	let end_idx = *compression_indices.last().unwrap();

	// CRITICAL: Use start_idx - 1 to include the FIRST compression message in the range
	// calculate_range_tokens and remove_messages_in_range both use (start_index + 1)..=end_index
	// So to include message at start_idx, we need to pass start_idx - 1
	let range_start = if start_idx > 0 { start_idx - 1 } else { 0 };

	// Calculate tokens before
	let tokens_before = calculate_range_tokens(
		session,
		&MessageRange {
			start_index: range_start,
			end_index: end_idx,
		},
	)?;

	// Create project summary
	let project_summary = format_project_summary(
		&request.plan_title,
		&request.summary,
		request.total_tasks,
		request.total_phases,
		compression_indices.len(),
	);

	let tokens_after = estimate_tokens(&project_summary) as u64;

	// Skip compression if it doesn't reduce tokens
	if tokens_after >= tokens_before {
		crate::log_info!(
			"Project compression skipped: {} tokens before, {} tokens after (no savings).",
			tokens_before,
			tokens_after
		);
		return Ok(None);
	}

	// Remove all compression messages (using range_start to include first compression)
	let (messages_removed, _) = session.remove_messages_in_range(range_start, end_idx)?;

	// Insert project summary (compressed block is always cached=true — new stable boundary)
	session.insert_compressed_knowledge(range_start, project_summary)?;

	let tokens_saved = tokens_before.saturating_sub(tokens_after);
	let metrics = CompressionMetrics::new(messages_removed, tokens_saved, tokens_before);

	crate::log_debug!(
		"Project compression complete: {} summaries → 1 project summary, {} tokens saved",
		compression_indices.len(),
		metrics.tokens_saved
	);

	// CRITICAL: Log compression point to session file
	let _ = crate::session::logger::log_compression_point(
		&session.session.info.name,
		"project",
		messages_removed,
		tokens_saved,
		&session.session.messages,
	);

	// (dedup state is cleared inside `remove_messages_in_range` — see core.rs.)

	// CRITICAL FIX: Reset token tracking for fresh start after compression
	// This prevents token drift and ensures accurate cache/pricing calculations
	session.session.info.current_non_cached_tokens = 0;
	session.session.info.current_total_tokens = 0;

	// Reset cache checkpoint time
	session.session.info.last_cache_checkpoint_time = std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.unwrap_or_default()
		.as_secs();

	// CRITICAL: Clear start_index after successful project compression
	// Project compression is the final compression, so we reset the range
	super::core::clear_task_start_index();
	crate::log_debug!("Cleared start_index after successful project compression - plan complete");

	Ok(Some(metrics))
}

/// Extract file references from tool calls in messages
/// Returns merged and deduplicated file refs (path or path:start:end)
fn extract_file_refs_from_messages(messages: &[crate::session::Message]) -> Vec<String> {
	let mut refs: Vec<String> = Vec::new();

	for msg in messages {
		if msg.role != "assistant" {
			continue;
		}

		if let Some(calls) = msg.tool_calls.as_ref().and_then(|v| v.as_array()) {
			for call in calls {
				let name = call
					.get("function")
					.and_then(|f| f.get("name"))
					.and_then(|n| n.as_str())
					.unwrap_or("unknown");

				if let Some(args) = call.get("function").and_then(|f| f.get("arguments")) {
					crate::session::chat::file_context::extract_file_refs_from_args(
						name, args, &mut refs,
					);
				}
			}
		}
	}

	// Merge overlapping ranges
	crate::session::chat::file_context::merge_file_refs(&refs)
}

fn format_project_summary(
	plan_title: &str,
	summary: &str,
	total_tasks: usize,
	total_phases: usize,
	summaries_compressed: usize,
) -> String {
	let compression_id = get_compression_id().unwrap_or_else(|| "unknown".to_string());
	format!(
		"<project_compressed id=\"{}\" plan=\"{}\" tasks=\"{}\" phases=\"{}\" summaries_folded=\"{}\">\n\
		<summary>{}</summary>\n\
		</project_compressed>",
		compression_id,
		escape_attr(plan_title),
		total_tasks,
		total_phases,
		summaries_compressed,
		summary,
	)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_compression_metrics_calculation() {
		let metrics = CompressionMetrics::new(10, 5000, 10000);
		assert_eq!(metrics.messages_removed, 10);
		assert_eq!(metrics.tokens_saved, 5000);
		assert_eq!(metrics.compression_ratio, 0.5);
	}

	#[test]
	fn test_compression_metrics_zero_original() {
		let metrics = CompressionMetrics::new(5, 0, 0);
		assert_eq!(metrics.compression_ratio, 0.0);
	}

	#[test]
	fn test_format_compressed_summary() {
		use chrono::Utc;

		let task = PlanTask {
			title: "Test Task".to_string(),
			description: "Test description".to_string(),
			details: "Some details".to_string(),
			summary: Some("Task completed successfully".to_string()),
			status: super::super::storage::TaskStatus::Completed,
			completed_at: Some(Utc::now()),
			message_range: None,
			phase: None,
		};

		// With an empty anchor the function falls back to a minimal task
		// block (defensive — production never hits this because the
		// caller extends the anchor first).
		let empty_anchor = crate::session::anchor::Anchor::default();
		let formatted = format_compressed_summary(
			&task,
			"Task completed successfully",
			"test_123",
			None,
			&[],
			&empty_anchor,
		);
		assert!(formatted.contains("<task_compressed"));
		assert!(formatted.contains("id=\"test_123\""));
		assert!(formatted.contains("task=\"Test Task\""));
		assert!(formatted.contains("Task completed successfully"));
		assert!(formatted.contains("</task_compressed>"));
	}

	#[test]
	fn format_compressed_summary_uses_anchor_as_primary_content() {
		use chrono::Utc;
		let task = PlanTask {
			title: "Test Task".to_string(),
			description: "Test description".to_string(),
			details: "Some details".to_string(),
			summary: Some("ok".to_string()),
			status: super::super::storage::TaskStatus::Completed,
			completed_at: Some(Utc::now()),
			message_range: None,
			phase: None,
		};
		let mut anchor = crate::session::anchor::Anchor::default();
		anchor.extend(
			crate::session::anchor::AnchorUpdate {
				intent: Some("Refactor auth layer".to_string()),
				decisions: vec!["use JWT not sessions".to_string()],
				..Default::default()
			},
			0,
		);
		let formatted = format_compressed_summary(&task, "ok", "id_xyz", None, &[], &anchor);
		// Anchor content surfaces directly inside <anchor>…</anchor>.
		assert!(formatted.contains("<anchor>"));
		assert!(formatted.contains("Refactor auth layer"));
		assert!(formatted.contains("use JWT not sessions"));
		assert!(formatted.contains("</anchor>"));
		// The defensive per-task <summary> fallback must NOT appear when the
		// anchor carries content (it's only used when the anchor is empty).
		assert!(!formatted.contains("<summary>ok</summary>"));
	}

	#[test]
	fn task_compression_advances_past_tool_results_to_prevent_orphans() {
		use crate::session::Message;
		use serde_json::json;

		fn msg(role: &str) -> Message {
			Message {
				role: role.to_string(),
				content: format!("{} message", role),
				timestamp: 1000,
				cached: false,
				cache_ttl: None,
				tool_call_id: None,
				name: None,
				tool_calls: None,
				images: None,
				videos: None,
				thinking: None,
				id: None,
			}
		}

		let mut messages = Vec::new();
		messages.push(msg("system")); // 0

		let mut assistant = msg("assistant"); // 1
		assistant.tool_calls = Some(json!([
			{"id": "call_A", "type": "function", "function": {"name": "view_signatures", "arguments": "{}"}},
			{"id": "call_B", "type": "function", "function": {"name": "view", "arguments": "{}"}}
		]));
		messages.push(assistant);

		let mut tool_a = msg("tool"); // 2
		tool_a.tool_call_id = Some("call_A".to_string());
		messages.push(tool_a);

		let mut tool_b = msg("tool"); // 3
		tool_b.tool_call_id = Some("call_B".to_string());
		messages.push(tool_b);

		messages.push(msg("assistant")); // 4
		messages.push(msg("user")); // 5
		messages.push(msg("assistant")); // 6

		let start_index = 1usize;
		let end_index = 6usize;

		let drain_range = start_index + 1..=end_index;
		let tool_results_in_drain: Vec<&str> = messages[drain_range]
			.iter()
			.filter(|m| m.role == "tool")
			.filter_map(|m| m.tool_call_id.as_deref())
			.collect();
		assert!(tool_results_in_drain.contains(&"call_A"));
		assert!(tool_results_in_drain.contains(&"call_B"));

		let tool_ids_in_anchor: Vec<&str> = messages[start_index]
			.tool_calls
			.as_ref()
			.unwrap()
			.as_array()
			.unwrap()
			.iter()
			.map(|tc| tc["id"].as_str().unwrap())
			.collect();

		let adjusted_start =
			adjusted_task_compression_start_index(&messages, start_index, end_index).unwrap();
		assert_eq!(adjusted_start, 4);

		let safe_drain_range = adjusted_start + 1..=end_index;
		for msg in messages[safe_drain_range.clone()].iter() {
			if msg.role == "tool" {
				if let Some(ref tc_id) = msg.tool_call_id {
					assert!(
						!tool_ids_in_anchor.contains(&tc_id.as_str()),
						"drain range must not include anchor tool result {tc_id}"
					);
				}
			}
		}

		messages.drain(safe_drain_range);
		let assistant_msg = &messages[1];
		let tool_call_ids: Vec<String> = assistant_msg
			.tool_calls
			.as_ref()
			.unwrap()
			.as_array()
			.unwrap()
			.iter()
			.map(|tc| tc["id"].as_str().unwrap().to_string())
			.collect();

		for tc_id in &tool_call_ids {
			let has_result = messages
				.iter()
				.any(|m| m.role == "tool" && m.tool_call_id.as_deref() == Some(tc_id.as_str()));
			assert!(
				has_result,
				"tool_call {tc_id} must keep a matching tool_result"
			);
		}
	}

	#[test]
	fn task_compression_no_advancement_when_no_tool_calls() {
		use crate::session::Message;

		fn msg(role: &str) -> Message {
			Message {
				role: role.to_string(),
				content: format!("{} message", role),
				timestamp: 1000,
				cached: false,
				cache_ttl: None,
				tool_call_id: None,
				name: None,
				tool_calls: None,
				images: None,
				videos: None,
				thinking: None,
				id: None,
			}
		}

		let messages = [
			msg("system"),
			msg("user"),
			msg("assistant"),
			msg("user"),
			msg("assistant"),
		];

		let adjusted_start = adjusted_task_compression_start_index(&messages, 1, 4).unwrap();
		assert_eq!(adjusted_start, 1);
	}

	#[test]
	fn task_compression_skips_when_range_only_contains_anchor_tool_results() {
		use crate::session::Message;
		use serde_json::json;

		fn msg(role: &str) -> Message {
			Message {
				role: role.to_string(),
				content: format!("{} message", role),
				timestamp: 1000,
				cached: false,
				cache_ttl: None,
				tool_call_id: None,
				name: None,
				tool_calls: None,
				images: None,
				videos: None,
				thinking: None,
				id: None,
			}
		}

		let mut messages = Vec::new();
		messages.push(msg("system"));

		let mut assistant = msg("assistant");
		assistant.tool_calls = Some(json!([
			{"id": "call_A", "type": "function", "function": {"name": "view", "arguments": "{}"}}
		]));
		messages.push(assistant);

		let mut tool = msg("tool");
		tool.tool_call_id = Some("call_A".to_string());
		messages.push(tool);

		assert_eq!(adjusted_task_compression_start_index(&messages, 1, 2), None);
	}

	#[test]
	fn task_compression_start_in_middle_of_tool_results_orphans_tool_use() {
		use crate::session::Message;
		use serde_json::json;

		fn msg(role: &str) -> Message {
			Message {
				role: role.to_string(),
				content: format!("{} message", role),
				timestamp: 1000,
				cached: false,
				cache_ttl: None,
				tool_call_id: None,
				name: None,
				tool_calls: None,
				images: None,
				videos: None,
				thinking: None,
				id: None,
			}
		}

		// Scenario: start_index lands on a tool_result that is NOT the last
		// result for its parent assistant. The assistant at start_index-1 has
		// multiple tool_calls; some results fall before start_index, some after.
		// After drain, the assistant retains tool_use blocks whose results were
		// removed — Anthropic rejects this ("tool_use ids were found without
		// tool_result blocks immediately after").
		let mut messages = Vec::new();
		messages.push(msg("system")); // 0

		let mut assistant = msg("assistant"); // 1
		assistant.tool_calls = Some(json!([
			{"id": "toolu_AAA", "type": "function", "function": {"name": "view", "arguments": "{}"}},
			{"id": "toolu_BBB", "type": "function", "function": {"name": "edit", "arguments": "{}"}},
			{"id": "toolu_CCC", "type": "function", "function": {"name": "shell", "arguments": "{}"}}
		]));
		messages.push(assistant);

		let mut tool_a = msg("tool"); // 2
		tool_a.tool_call_id = Some("toolu_AAA".to_string());
		messages.push(tool_a);

		let mut tool_b = msg("tool"); // 3  ← start_index lands here
		tool_b.tool_call_id = Some("toolu_BBB".to_string());
		messages.push(tool_b);

		let mut tool_c = msg("tool"); // 4  ← this result will be drained
		tool_c.tool_call_id = Some("toolu_CCC".to_string());
		messages.push(tool_c);

		messages.push(msg("assistant")); // 5
		messages.push(msg("user")); // 6
		messages.push(msg("assistant")); // 7

		let start_index = 3usize; // tool_result for toolu_BBB
		let end_index = 7usize;

		// Current behaviour: adjusted_start does NOT advance past tool_c
		// because start_index is a tool message, not an assistant.
		let adjusted_start =
			adjusted_task_compression_start_index(&messages, start_index, end_index).unwrap();

		// Simulate the drain that compress_completed_task would do
		let drain_range = adjusted_start + 1..=end_index;
		let drained: Vec<Message> = messages.drain(drain_range).collect();
		let _ = drained;

		// Collect surviving tool_call ids from the assistant at index 1
		let tool_call_ids: Vec<&str> = messages[1]
			.tool_calls
			.as_ref()
			.unwrap()
			.as_array()
			.unwrap()
			.iter()
			.map(|tc| tc["id"].as_str().unwrap())
			.collect();

		// For every surviving tool_call, there MUST be a matching tool_result
		for tc_id in &tool_call_ids {
			let has_result = messages
				.iter()
				.any(|m| m.role == "tool" && m.tool_call_id.as_deref() == Some(tc_id));
			assert!(
				has_result,
				"BUG: tool_call {tc_id} has no matching tool_result after compression — \
				 Anthropic will reject with 'tool_use ids were found without tool_result blocks'"
			);
		}
	}

	#[test]
	fn task_compression_start_on_asst_with_tool_calls_whose_tool_result_was_already_drained() {
		// Real-world bug observed in session 260525-octomind-1631-3fda.jsonl
		// (lines 158-160): after multiple cascading compressions, an
		// `assistant` message with `tool_calls` survives in the log while its
		// matching `tool_result` was drained by an earlier compression
		// cycle. The asst is now immediately followed by a non-tool message
		// (here: a prior `<task_compressed>` summary, but it could be any
		// non-tool: user, plain asst, etc.).
		//
		// When the next task compression picks this asst as `start_index`,
		// Case 1 of `adjusted_task_compression_start_index` walks `next`
		// past consecutive tool messages from start_index+1 — but there
		// are none, so `next == start_index + 1` and the function returns
		// `Some(start_index)` unchanged.
		//
		// `compress_completed_task` then drains `[start_index+1..=end_index]`
		// and inserts a fresh `<task_compressed>` summary at start_index+1.
		// The kept asst_with_tool_calls now has another assistant message
		// (the new summary) as its immediate successor — no tool_result
		// anywhere. Anthropic rejects:
		//   "messages.N: tool_use ids were found without tool_result blocks
		//    immediately after: <toolu_id>"
		//
		// The fix must guarantee that if `start_index` is an
		// assistant_with_tool_calls whose tool_result is NOT immediately
		// after, this asst is not left as a "tail" anchor — either advance
		// past the asst (drain it too) or skip compression entirely.

		use crate::session::Message;
		use serde_json::json;

		fn msg(role: &str) -> Message {
			Message {
				role: role.to_string(),
				content: format!("{} message", role),
				timestamp: 1000,
				cached: false,
				cache_ttl: None,
				tool_call_id: None,
				name: None,
				tool_calls: None,
				images: None,
				videos: None,
				thinking: None,
				id: None,
			}
		}

		let mut messages = Vec::new();
		messages.push(msg("system")); // 0
		messages.push(msg("user")); // 1

		// Orphan asst_with_tool_calls — its tool_result was drained in a
		// previous compression cycle, leaving this assistant adjacent to a
		// non-tool message (the prior task's summary).
		let mut orphan_asst = msg("assistant"); // 2 ← start_index
		orphan_asst.tool_calls = Some(json!([
			{"id": "toolu_01JUz8Fe23qdwA1gSH6zFm17", "type": "function",
			 "function": {"name": "text_editor", "arguments": "{}"}}
		]));
		messages.push(orphan_asst);

		// Prior compression's summary message — assistant role, no tool_calls.
		// This is what makes messages[start_index+1] NOT a tool message.
		let mut prior_summary = msg("assistant"); // 3
		prior_summary.content =
			"<task_compressed id=\"prior\">earlier task summary</task_compressed>".into();
		messages.push(prior_summary);

		messages.push(msg("user")); // 4
		messages.push(msg("assistant")); // 5
		messages.push(msg("user")); // 6
		messages.push(msg("assistant")); // 7

		let start_index = 2usize;
		let end_index = 7usize;

		// Fixed behavior: when start_index lands on an asst_with_tool_calls
		// whose tool_result is NOT immediately following (already drained by
		// a previous compression), the function must return None so the
		// caller skips compression rather than inserting a summary right
		// after the orphan asst and creating an Anthropic-rejected stream.
		let adjusted = adjusted_task_compression_start_index(&messages, start_index, end_index);
		assert!(
			adjusted.is_none(),
			"asst_with_tool_calls at start_index without an immediate tool_result \
			 must skip compression (return None); got Some({:?}). Using this asst \
			 as the kept anchor would orphan its tool_use blocks against the \
			 inserted summary — Anthropic rejects with 'tool_use ids were found \
			 without tool_result blocks immediately after'.",
			adjusted
		);

		// Additionally verify the invariant the API requires: had we proceeded
		// with the OLD buggy behavior (Some(start_index)), the resulting
		// stream would have an asst_with_tool_calls followed by the summary.
		// We assert here on the UNCHANGED message stream (no drain performed
		// because compression was skipped) — every asst_with_tool_calls must
		// still be followed by its tool_result, OR the test scenario itself
		// must already be in the orphan state we're protecting against.
		let mut found_orphan_in_input = false;
		for (i, m) in messages.iter().enumerate() {
			let Some(tcs) = m.tool_calls.as_ref().and_then(|v| v.as_array()) else {
				continue;
			};
			if tcs.is_empty() {
				continue;
			}
			let tc_ids: Vec<&str> = tcs.iter().filter_map(|tc| tc["id"].as_str()).collect();
			let next = messages.get(i + 1);
			let next_is_matching_tool_result = matches!(
				next,
				Some(n) if n.role == "tool"
					&& n.tool_call_id.as_deref().is_some_and(|id| tc_ids.contains(&id))
			);
			if !next_is_matching_tool_result {
				found_orphan_in_input = true;
				// The orphan is in the INPUT — that's the precondition this
				// test sets up. The fix doesn't repair it; it just prevents
				// task compression from making it worse. A separate scrubber
				// (out of scope here) is responsible for repairing existing
				// orphan messages before they hit the API.
				assert_eq!(
					i, start_index,
					"the only orphan in this fixture should be the one we deliberately constructed at start_index"
				);
			}
		}
		assert!(
			found_orphan_in_input,
			"test fixture should contain the orphan asst_with_tool_calls at start_index"
		);
	}
}
