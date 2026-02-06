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
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

// Global state for pending compression requests
// This allows the plan tool to signal that compression should happen
// without needing to pass ChatSession through the MCP execution chain
lazy_static::lazy_static! {
	static ref PENDING_COMPRESSION: Arc<Mutex<Option<PlanTask>>> = Arc::new(Mutex::new(None));
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
			"Set message range for pending compression: {}-{}",
			start_index,
			end_index
		);
		Ok(())
	} else {
		Err(anyhow::anyhow!("No pending compression to set range on"))
	}
}

/// Check if there's a pending compression request and execute it
/// This is called from the session response processing after tool execution
pub async fn process_pending_compression(session: &mut ChatSession) -> Result<Option<CompressionMetrics>> {
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
	// Validate task has required data
	let _summary = task
		.summary
		.as_ref()
		.ok_or_else(|| anyhow::anyhow!("Task has no summary - cannot compress"))?;

	let message_range = task
		.message_range
		.as_ref()
		.ok_or_else(|| anyhow::anyhow!("Task has no message range - cannot compress"))?;

	crate::log_debug!(
		"Compressing task '{}' (messages {}-{})",
		task.title,
		message_range.start_index,
		message_range.end_index
	);

	// Calculate tokens before compression (for metrics)
	let tokens_before = calculate_range_tokens(session, message_range)?;

	// Create compressed knowledge entry
	let compressed_entry = format_compressed_summary(task);

	// Calculate tokens in compressed entry
	let tokens_after = estimate_tokens(&compressed_entry) as u64;

	// Remove messages in range
	let messages_removed = session.remove_messages_in_range(
		message_range.start_index,
		message_range.end_index,
	)?;

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
fn format_compressed_summary(task: &PlanTask) -> String {
	let summary = task.summary.as_ref().unwrap(); // Safe: validated in caller
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
		 *This is a compressed summary. Detailed tool calls and intermediate work have been removed to optimize context.*",
		task.title,
		task.description,
		summary,
		completed_at
	)
}

/// Calculate total tokens in message range
fn calculate_range_tokens(
	session: &ChatSession,
	range: &MessageRange,
) -> Result<u64> {
	let mut total_tokens = 0u64;

	// Validate range
	if range.start_index >= session.session.messages.len() {
		return Err(anyhow::anyhow!("Invalid start_index in message range"));
	}

	if range.end_index > session.session.messages.len() {
		return Err(anyhow::anyhow!("Invalid end_index in message range"));
	}

	// Count tokens in range (start_index+1 to end_index-1, the messages that will be removed)
	for i in (range.start_index + 1)..range.end_index {
		if let Some(message) = session.session.messages.get(i) {
			let tokens = estimate_tokens(&message.content) as u64;
			total_tokens += tokens;
		}
	}

	Ok(total_tokens)
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
		};

		let formatted = format_compressed_summary(&task);
		assert!(formatted.contains("## Task Completed: Test Task"));
		assert!(formatted.contains("**Description**: Test description"));
		assert!(formatted.contains("**Summary**: Task completed successfully"));
		assert!(formatted.contains("compressed summary"));
	}
}
