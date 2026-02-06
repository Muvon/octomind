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

//! Storage abstraction for plan tool

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionPlan {
	pub title: String,
	pub tasks: Vec<PlanTask>,
	pub current_task_index: usize,
	pub created_at: DateTime<Utc>,
	pub status: PlanStatus,
	#[serde(default)]
	pub phase_compressions: Vec<super::compression::PhaseCompression>,
	#[serde(default)]
	pub project_compression: Option<super::compression::ProjectCompression>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageRange {
	pub start_index: usize, // First message index when task started
	pub end_index: usize,   // Last message index before compression
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhaseCompression {
	pub phase_name: String,
	pub task_range: (usize, usize),
	pub summary: String,
	pub compressed_at: DateTime<Utc>,
	pub message_range: MessageRange,
	pub metrics: super::compression::CompressionMetrics,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectCompression {
	pub summary: String,
	pub compressed_at: DateTime<Utc>,
	pub message_range: MessageRange,
	pub metrics: super::compression::CompressionMetrics,
	pub total_tasks: usize,
	pub total_phases: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanTask {
	pub title: String,
	pub description: String,     // Detailed explanation of what needs to be done
	pub details: String,         // Progress details from `step` commands
	pub summary: Option<String>, // Final summary from `next` command
	pub status: TaskStatus,
	pub completed_at: Option<DateTime<Utc>>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub message_range: Option<MessageRange>, // Message range for compression
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub phase: Option<String>, // Optional phase grouping
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PlanStatus {
	Active,
	Completed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TaskStatus {
	InProgress,
	Completed,
}

/// Task creation data structure
#[derive(Debug, Clone)]
pub struct TaskData {
	pub title: String,
	pub description: String,
	pub phase: Option<String>,
}

impl TaskData {
	pub fn new(title: String, description: String, phase: Option<String>) -> Self {
		Self {
			title,
			description,
			phase,
		}
	}
}

/// Storage abstraction for plan execution
pub trait PlanStorage {
	/// Create new execution plan with detailed tasks
	fn create_plan(&mut self, title: String, tasks: Vec<TaskData>) -> Result<()>;

	/// Add details to current task (can be called multiple times)
	fn add_step_details(&mut self, content: String) -> Result<()>;

	/// Get current task details
	fn get_current_step_details(&self) -> Result<String>;

	/// Complete current task with summary and move to next
	fn complete_current_task(&mut self, summary: String) -> Result<()>;

	/// Check if there are more tasks to complete
	fn has_more_tasks(&self) -> Result<bool>;

	/// Get task list with status and descriptions
	fn get_task_list(&self) -> Result<Vec<(String, String, TaskStatus)>>; // (title, description, status)

	/// Get current task info with description
	fn get_current_task_info(&self) -> Result<(usize, usize, String, String)>; // (current, total, title, description)

	/// Mark entire plan as completed
	fn complete_plan(&mut self, summary: String) -> Result<()>;

	/// Clear all plan data
	fn clear_plan(&mut self) -> Result<()>;

	/// Check if plan exists
	fn has_active_plan(&self) -> Result<bool>;

	/// Get plan title
	fn get_plan_title(&self) -> Result<String>;

	/// Set message range for current task (for compression tracking)
	fn set_current_task_message_range(
		&mut self,
		start_index: usize,
		end_index: usize,
	) -> Result<()>;

	/// Get the completed task with its message range (for compression)
	fn get_last_completed_task(&self) -> Result<Option<PlanTask>>;

	/// Get count of completed tasks
	fn get_completed_task_count(&self) -> Result<usize>;

	/// Get current task index
	fn get_current_task_index(&self) -> Result<usize>;

	/// Get total task count
	fn get_total_task_count(&self) -> Result<usize>;

	/// Get phase compression count
	fn get_phase_count(&self) -> Result<usize>;

	/// Get the execution plan (for phase detection)
	fn get_plan(&self) -> Result<&ExecutionPlan>;
}
