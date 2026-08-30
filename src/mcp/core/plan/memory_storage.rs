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

//! In-memory storage implementation for runtime-owned plans.

use super::storage::{ExecutionPlan, PlanStatus, PlanStorage, PlanTask, TaskData, TaskStatus};
use anyhow::{anyhow, Result};
use chrono::Utc;

/// In-memory storage for plan execution
pub struct MemoryPlanStorage {
	plan: Option<ExecutionPlan>,
	/// Unix seconds of the last model-driven plan mutation (create, advance,
	/// revise, finish, restore). Compared against the latest real-user message
	/// timestamp to flag a plan that predates the current request. Runtime
	/// bookkeeping (message ranges) does not count as engagement.
	touched_at: u64,
}

impl MemoryPlanStorage {
	pub fn new() -> Self {
		Self {
			plan: None,
			touched_at: 0,
		}
	}

	pub fn touched_at(&self) -> u64 {
		self.touched_at
	}

	fn touch(&mut self) {
		self.touched_at = Utc::now().timestamp().max(0) as u64;
	}

	/// Backdate the engagement clock so tests can prove a mutation refreshes
	/// it even within the same wall-clock second.
	#[cfg(test)]
	pub(crate) fn set_touched_at(&mut self, timestamp: u64) {
		self.touched_at = timestamp;
	}

	/// Replace only the unfinished tail while preserving completed history.
	pub fn replace_remaining(&mut self, tasks: Vec<TaskData>) -> Result<()> {
		if tasks.is_empty() {
			return Err(anyhow!("A revised plan needs at least one remaining task"));
		}
		let plan = self
			.plan
			.as_mut()
			.ok_or_else(|| anyhow!("No active plan"))?;
		plan.tasks.truncate(plan.current_task_index);
		plan.tasks.extend(tasks.into_iter().map(|task| PlanTask {
			title: task.title,
			description: task.description,
			details: String::new(),
			summary: None,
			status: TaskStatus::InProgress,
			completed_at: None,
			message_range: None,
			phase: task.phase,
			valid_if: task.valid_if,
		}));
		self.touch();
		Ok(())
	}
}

impl Default for MemoryPlanStorage {
	fn default() -> Self {
		Self::new()
	}
}

impl PlanStorage for MemoryPlanStorage {
	fn create_plan(&mut self, title: String, tasks: Vec<TaskData>) -> Result<()> {
		if tasks.is_empty() {
			return Err(anyhow!("Cannot create plan with empty task list"));
		}

		let plan_tasks: Vec<PlanTask> = tasks
			.into_iter()
			.map(|task_data| PlanTask {
				title: task_data.title,
				description: task_data.description,
				details: String::new(),
				summary: None,
				status: TaskStatus::InProgress, // All tasks start as InProgress, managed by current_task_index
				completed_at: None,
				message_range: None,    // Initialize as None, will be set during compression
				phase: task_data.phase, // Optional phase grouping
				valid_if: task_data.valid_if, // Optional falsifiable validity condition
			})
			.collect();

		self.plan = Some(ExecutionPlan {
			title,
			tasks: plan_tasks,
			current_task_index: 0,
			created_at: Utc::now(),
			status: PlanStatus::Active,
		});
		self.touch();

		Ok(())
	}

	fn add_step_details(&mut self, content: String) -> Result<()> {
		let plan = self
			.plan
			.as_mut()
			.ok_or_else(|| anyhow!("No active plan"))?;

		if plan.current_task_index >= plan.tasks.len() {
			return Err(anyhow!("No current task to update"));
		}

		let current_task = &mut plan.tasks[plan.current_task_index];
		if !current_task.details.is_empty() {
			current_task.details.push_str("\n\n");
		}
		current_task.details.push_str(&content);
		self.touch();

		Ok(())
	}

	fn get_current_step_details(&self) -> Result<String> {
		let plan = self
			.plan
			.as_ref()
			.ok_or_else(|| anyhow!("No active plan"))?;

		if plan.current_task_index >= plan.tasks.len() {
			return Err(anyhow!("No current task"));
		}

		Ok(plan.tasks[plan.current_task_index].details.clone())
	}

	fn complete_current_task(&mut self, summary: String) -> Result<()> {
		let plan = self
			.plan
			.as_mut()
			.ok_or_else(|| anyhow!("No active plan"))?;

		if plan.current_task_index >= plan.tasks.len() {
			return Err(anyhow!("No current task to complete"));
		}

		// Complete current task
		let current_task = &mut plan.tasks[plan.current_task_index];
		current_task.summary = Some(summary);
		current_task.status = TaskStatus::Completed;
		current_task.completed_at = Some(Utc::now());

		// Move to next task
		plan.current_task_index += 1;
		self.touch();

		Ok(())
	}

	fn has_more_tasks(&self) -> Result<bool> {
		let plan = self
			.plan
			.as_ref()
			.ok_or_else(|| anyhow!("No active plan"))?;
		Ok(plan.current_task_index < plan.tasks.len())
	}

	fn get_task_list(&self) -> Result<Vec<(String, String, TaskStatus)>> {
		let plan = self
			.plan
			.as_ref()
			.ok_or_else(|| anyhow!("No active plan"))?;

		let mut tasks = Vec::new();
		for (i, task) in plan.tasks.iter().enumerate() {
			let status = if i < plan.current_task_index {
				TaskStatus::Completed
			} else {
				TaskStatus::InProgress // Current and pending tasks both show as InProgress
			};
			tasks.push((task.title.clone(), task.description.clone(), status));
		}

		Ok(tasks)
	}

	fn get_current_task_info(&self) -> Result<(usize, usize, String, String)> {
		let plan = self
			.plan
			.as_ref()
			.ok_or_else(|| anyhow!("No active plan"))?;

		if plan.current_task_index >= plan.tasks.len() {
			return Err(anyhow!("All tasks completed"));
		}

		let current_task = &plan.tasks[plan.current_task_index];
		Ok((
			plan.current_task_index + 1, // 1-indexed for display
			plan.tasks.len(),
			current_task.title.clone(),
			current_task.description.clone(),
		))
	}

	fn complete_plan(&mut self, _summary: String) -> Result<()> {
		let plan = self
			.plan
			.as_mut()
			.ok_or_else(|| anyhow!("No active plan"))?;

		plan.status = PlanStatus::Completed;
		self.touch();
		Ok(())
	}

	fn clear_plan(&mut self) -> Result<()> {
		self.plan = None;
		Ok(())
	}

	fn has_active_plan(&self) -> Result<bool> {
		Ok(self.plan.is_some() && matches!(self.plan.as_ref().unwrap().status, PlanStatus::Active))
	}

	fn get_plan_title(&self) -> Result<String> {
		let plan = self
			.plan
			.as_ref()
			.ok_or_else(|| anyhow!("No active plan"))?;
		Ok(plan.title.clone())
	}

	fn set_current_task_message_range(
		&mut self,
		start_index: usize,
		end_index: usize,
	) -> Result<()> {
		let plan = self
			.plan
			.as_mut()
			.ok_or_else(|| anyhow!("No active plan"))?;

		// Set message range for the task that was just completed (current_task_index - 1)
		if plan.current_task_index == 0 {
			return Err(anyhow!("No completed task to set message range for"));
		}

		let completed_task_index = plan.current_task_index - 1;
		if completed_task_index >= plan.tasks.len() {
			return Err(anyhow!("Invalid task index"));
		}

		plan.tasks[completed_task_index].message_range = Some(super::storage::MessageRange {
			start_index,
			end_index,
		});

		Ok(())
	}

	fn get_last_completed_task(&self) -> Result<Option<PlanTask>> {
		let plan = self
			.plan
			.as_ref()
			.ok_or_else(|| anyhow!("No active plan"))?;

		// Get the last completed task (current_task_index - 1)
		if plan.current_task_index == 0 {
			return Ok(None); // No completed tasks yet
		}

		let completed_task_index = plan.current_task_index - 1;
		if completed_task_index >= plan.tasks.len() {
			return Ok(None);
		}

		Ok(Some(plan.tasks[completed_task_index].clone()))
	}

	fn get_completed_task_count(&self) -> Result<usize> {
		let plan = self
			.plan
			.as_ref()
			.ok_or_else(|| anyhow!("No active plan"))?;
		Ok(plan.current_task_index)
	}

	fn get_current_task_index(&self) -> Result<usize> {
		let plan = self
			.plan
			.as_ref()
			.ok_or_else(|| anyhow!("No active plan"))?;
		Ok(plan.current_task_index)
	}

	fn get_total_task_count(&self) -> Result<usize> {
		let plan = self
			.plan
			.as_ref()
			.ok_or_else(|| anyhow!("No active plan"))?;
		Ok(plan.tasks.len())
	}

	fn get_plan(&self) -> Result<&ExecutionPlan> {
		self.plan.as_ref().ok_or_else(|| anyhow!("No active plan"))
	}

	fn load_plan(&mut self, plan: ExecutionPlan) -> Result<()> {
		self.plan = Some(plan);
		self.touch();
		Ok(())
	}
}

#[cfg(test)]
#[path = "memory_storage_tests.rs"]
mod tests;
