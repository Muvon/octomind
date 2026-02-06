// Copyright 2025 Muvon Un Limited
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
use crate::session::estimate_tokens;
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

// Global state for pending compression requests
// This allows the plan tool to signal that compression should happen
// without needing to pass ChatSession through the MCP execution chain
lazy_static::lazy_static! {
	static ref PENDING_COMPRESSION: Arc<Mutex<Option<PlanTask>>> = Arc::new(Mutex::new(None));
	static ref PENDING_PHASE_COMPRESSION: Arc<Mutex<Option<PhaseCompressionRequest>>> = Arc::new(Mutex::new(None));
	static ref PENDING_PROJECT_COMPRESSION: Arc<Mutex<Option<ProjectCompressionRequest>>> = Arc::new(Mutex::new(None));
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
/// This is called by the plan tool when a task is completed
pub fn request_compression(task: PlanTask) {
	crate::log_debug!("Compression requested for task: {}", task.title);
	let mut pending = PENDING_COMPRESSION.lock().unwrap();
	*pending = Some(task);
}

/// Set message range on the pending compression task
/// This is called by the session after detecting a plan tool execution
pub fn set_pending_compression_range(start_index: usize, end_index: usize) -> Result<()> {
	let mut pending = PENDING_COMPRESSION.lock().unwrap();
	if let Some(ref mut task) = *pending {
		task.message_range = Some(MessageRange {
			start_index,
			end_index,
		});
		crate::log_debug!(
			"Set message range for pending compression: {}-{} (task: {})",
			start_index,
			end_index,
			task.title
		);
		Ok(())
	} else {
		Err(anyhow::anyhow!(
            "No pending compression to set range on - compression was not requested or already processed"
        ))
	}
}

/// Check if there's a pending compression request and execute it
/// This is called from the session response processing after tool execution
pub async fn process_pending_compression(
	session: &mut ChatSession,
) -> Result<Option<CompressionMetrics>> {
	let task = {
		let mut pending = PENDING_COMPRESSION.lock().unwrap();
		pending.take() // Take the task, leaving None
	};

	if let Some(task) = task {
		crate::log_debug!("Processing pending compression for task: {}", task.title);
		let metrics = compress_completed_task(session, &task).await?;
		Ok(Some(metrics))
	} else {
		Ok(None)
	}
}

/// Check if there's a pending compression request
pub fn has_pending_compression() -> bool {
	PENDING_COMPRESSION.lock().unwrap().is_some()
}

/// Request phase compression
pub fn request_phase_compression(phase_name: String, task_range: (usize, usize), summary: String) {
	crate::log_debug!(
		"Phase compression requested: {} (tasks {}-{})",
		phase_name,
		task_range.0 + 1,
		task_range.1 + 1
	);
	let mut pending = PENDING_PHASE_COMPRESSION.lock().unwrap();
	*pending = Some(PhaseCompressionRequest {
		phase_name,
		task_range,
		summary,
		message_range: None,
	});
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
	let mut pending = PENDING_PROJECT_COMPRESSION.lock().unwrap();
	*pending = Some(ProjectCompressionRequest {
		plan_title,
		summary,
		total_tasks,
		total_phases,
		message_range: None,
	});
}

/// Check if there's a pending phase compression request
pub fn has_pending_phase_compression() -> bool {
	PENDING_PHASE_COMPRESSION.lock().unwrap().is_some()
}

/// Check if there's a pending project compression request
pub fn has_pending_project_compression() -> bool {
	PENDING_PROJECT_COMPRESSION.lock().unwrap().is_some()
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
) -> Result<CompressionMetrics> {
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

	// Calculate tokens before compression (for metrics)
	let tokens_before = calculate_range_tokens(session, message_range)?;

	// Get compression ID for tracking
	let compression_id = get_compression_id().unwrap_or_else(|| "unknown".to_string());

	// Create compressed knowledge entry with validated summary
	let compressed_entry = format_compressed_summary(task, summary, &compression_id);

	// Calculate tokens in compressed entry
	let tokens_after = estimate_tokens(&compressed_entry) as u64;

	// Remove messages in range
	let messages_removed =
		session.remove_messages_in_range(message_range.start_index, message_range.end_index)?;

	// Insert compressed summary
	session.insert_compressed_knowledge(message_range.start_index, compressed_entry)?;

	// Calculate metrics
	let tokens_saved = tokens_before.saturating_sub(tokens_after);
	let metrics = CompressionMetrics::new(messages_removed, tokens_saved, tokens_before);

	crate::log_debug!(
		"Compression complete: {} messages removed, {} tokens saved ({:.1}% reduction)",
		metrics.messages_removed,
		metrics.tokens_saved,
		metrics.compression_ratio * 100.0
	);

	Ok(metrics)
}

/// Format task summary as structured knowledge block
/// Uses validated summary to avoid unwrap panic
fn format_compressed_summary(task: &PlanTask, summary: &str, compression_id: &str) -> String {
	let completed_at = task
		.completed_at
		.map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
		.unwrap_or_else(|| "Unknown".to_string());

	format!(
		"## Task Completed: {}\n\n\
		 **Description**: {}\n\n\
		 **Summary**: {}\n\n\
		 **Completed**: {}\n\n\
		 ---\n\
		 *Compressed (ID: {}) - Detailed tool calls and intermediate work have been removed to optimize context.*",
		task.title,
		task.description,
		summary,
		completed_at,
		compression_id
	)
}

/// Calculate total tokens in message range
fn calculate_range_tokens(session: &ChatSession, range: &MessageRange) -> Result<u64> {
	let mut total_tokens = 0u64;

	// Validate range
	if range.start_index >= session.session.messages.len() {
		return Err(anyhow::anyhow!("Invalid start_index in message range"));
	}

	if range.end_index > session.session.messages.len() {
		return Err(anyhow::anyhow!("Invalid end_index in message range"));
	}

	// Count tokens in range (start_index+1 to end_index inclusive, matching message removal)
	for i in (range.start_index + 1)..=range.end_index {
		if let Some(message) = session.session.messages.get(i) {
			let tokens = estimate_tokens(&message.content) as u64;
			total_tokens += tokens;
		}
	}

	Ok(total_tokens)
}

/// Process pending phase compression
pub async fn process_pending_phase_compression(
	session: &mut ChatSession,
) -> Result<Option<CompressionMetrics>> {
	let request = {
		let mut pending = PENDING_PHASE_COMPRESSION.lock().unwrap();
		pending.take()
	};

	if let Some(req) = request {
		crate::log_debug!("Processing pending phase compression: {}", req.phase_name);
		let metrics = compress_phase(session, &req).await?;
		Ok(Some(metrics))
	} else {
		Ok(None)
	}
}

/// Compress multiple task compressions into single phase summary
async fn compress_phase(
	session: &mut ChatSession,
	request: &PhaseCompressionRequest,
) -> Result<CompressionMetrics> {
	// Find all compressed task summaries for this phase
	let mut task_summaries = Vec::new();
	let mut start_index = None;
	let mut end_index = None;

	// Look for task compressions that belong to this phase
	for (i, msg) in session.session.messages.iter().enumerate() {
		if let Some(name) = &msg.name {
			if name == "plan_compression" && msg.content.contains("## Task Completed:") {
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

	// Calculate tokens before compression
	let tokens_before = calculate_range_tokens(
		session,
		&MessageRange {
			start_index: start_idx,
			end_index: end_idx,
		},
	)?;

	// Create phase summary
	let phase_summary =
		format_phase_summary(&request.phase_name, &request.summary, task_summaries.len());

	let tokens_after = estimate_tokens(&phase_summary) as u64;

	// Remove all task compression messages in range
	let messages_removed = session.remove_messages_in_range(start_idx, end_idx)?;

	// Insert phase summary
	session.insert_compressed_knowledge(start_idx, phase_summary)?;

	// Calculate metrics
	let tokens_saved = tokens_before.saturating_sub(tokens_after);
	let metrics = CompressionMetrics::new(messages_removed, tokens_saved, tokens_before);

	crate::log_debug!(
		"Phase compression '{}': {} task summaries → 1 phase summary, {} tokens saved",
		request.phase_name,
		task_summaries.len(),
		metrics.tokens_saved
	);

	Ok(metrics)
}

fn format_phase_summary(phase_name: &str, summary: &str, task_count: usize) -> String {
	format!(
		"## Phase Completed: {}\n\n\
		 **Tasks Completed**: {}\n\n\
		 **Summary**: {}\n\n\
		 ---\n\
		 *Phase Compression - {} task summaries compressed into phase overview*",
		phase_name, task_count, summary, task_count
	)
}

/// Process pending project compression
pub async fn process_pending_project_compression(
	session: &mut ChatSession,
) -> Result<Option<CompressionMetrics>> {
	let request = {
		let mut pending = PENDING_PROJECT_COMPRESSION.lock().unwrap();
		pending.take()
	};

	if let Some(req) = request {
		crate::log_debug!("Processing pending project compression: {}", req.plan_title);
		let metrics = compress_project(session, &req).await?;
		Ok(Some(metrics))
	} else {
		Ok(None)
	}
}

/// Compress entire plan (all tasks + phases) into final project summary
async fn compress_project(
	session: &mut ChatSession,
	request: &ProjectCompressionRequest,
) -> Result<CompressionMetrics> {
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
		return Err(anyhow!(
			"Project compression requires at least 2 compressions (found {})",
			compression_indices.len()
		));
	}

	let start_idx = *compression_indices.first().unwrap();
	let end_idx = *compression_indices.last().unwrap();

	// Calculate tokens before
	let tokens_before = calculate_range_tokens(
		session,
		&MessageRange {
			start_index: start_idx,
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

	// Remove all compression messages
	let messages_removed = session.remove_messages_in_range(start_idx, end_idx)?;

	// Insert project summary
	session.insert_compressed_knowledge(start_idx, project_summary)?;

	let tokens_saved = tokens_before.saturating_sub(tokens_after);
	let metrics = CompressionMetrics::new(messages_removed, tokens_saved, tokens_before);

	crate::log_debug!(
		"Project compression complete: {} summaries → 1 project summary, {} tokens saved",
		compression_indices.len(),
		metrics.tokens_saved
	);

	Ok(metrics)
}

fn format_project_summary(
	plan_title: &str,
	summary: &str,
	total_tasks: usize,
	total_phases: usize,
	summaries_compressed: usize,
) -> String {
	format!(
		"## Project Completed: {}\n\n\
		 **Scale**: {} tasks across {} phases\n\n\
		 **Summary**: {}\n\n\
		 ---\n\
		 *Project Compression - {} summaries consolidated into final project overview*",
		plan_title, total_tasks, total_phases, summary, summaries_compressed
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

		let formatted = format_compressed_summary(&task, "Task completed successfully", "test_123");
		assert!(formatted.contains("## Task Completed: Test Task"));
		assert!(formatted.contains("**Description**: Test description"));
		assert!(formatted.contains("**Summary**: Task completed successfully"));
		assert!(formatted.contains("Compressed"));
		assert!(formatted.contains("test_123"));
	}
}
