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

//! In-memory storage implementation for plan tool

use super::storage::{ExecutionPlan, PlanStatus, PlanStorage, PlanTask, TaskData, TaskStatus};
use anyhow::{anyhow, Result};
use chrono::Utc;

/// In-memory storage for plan execution
pub struct MemoryPlanStorage {
	plan: Option<ExecutionPlan>,
}

impl MemoryPlanStorage {
	pub fn new() -> Self {
		Self { plan: None }
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
			phase_compressions: Vec::new(),
			project_compression: None,
		});

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

	fn get_phase_count(&self) -> Result<usize> {
		let plan = self
			.plan
			.as_ref()
			.ok_or_else(|| anyhow!("No active plan"))?;
		Ok(plan.phase_compressions.len())
	}

	fn get_plan(&self) -> Result<&ExecutionPlan> {
		self.plan.as_ref().ok_or_else(|| anyhow!("No active plan"))
	}

	fn load_plan(&mut self, plan: ExecutionPlan) -> Result<()> {
		self.plan = Some(plan);
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn task(title: &str) -> TaskData {
		TaskData::new(title.to_string(), format!("do {title}"), None, None)
	}

	fn plan_with(titles: &[&str]) -> MemoryPlanStorage {
		let mut s = MemoryPlanStorage::new();
		s.create_plan("demo".to_string(), titles.iter().map(|t| task(t)).collect())
			.expect("plan created");
		s
	}

	#[test]
	fn empty_task_list_is_rejected() {
		let mut s = MemoryPlanStorage::new();
		assert!(s.create_plan("demo".to_string(), vec![]).is_err());
		assert!(!s.has_active_plan().unwrap());
	}

	#[test]
	fn every_accessor_errors_without_a_plan() {
		let mut s = MemoryPlanStorage::new();
		assert!(!s.has_active_plan().unwrap());
		assert!(s.get_plan_title().is_err());
		assert!(s.get_current_step_details().is_err());
		assert!(s.get_current_task_info().is_err());
		assert!(s.has_more_tasks().is_err());
		assert!(s.get_task_list().is_err());
		assert!(s.get_total_task_count().is_err());
		assert!(s.get_completed_task_count().is_err());
		assert!(s.get_last_completed_task().is_err());
		assert!(s.add_step_details("x".into()).is_err());
		assert!(s.complete_current_task("x".into()).is_err());
		assert!(s.set_current_task_message_range(0, 1).is_err());
	}

	#[test]
	fn a_fresh_plan_starts_on_the_first_task() {
		let s = plan_with(&["a", "b"]);
		assert!(s.has_active_plan().unwrap());
		assert_eq!(s.get_plan_title().unwrap(), "demo");
		assert_eq!(s.get_total_task_count().unwrap(), 2);
		assert_eq!(s.get_completed_task_count().unwrap(), 0);
		assert!(s.has_more_tasks().unwrap());
		assert!(s.get_last_completed_task().unwrap().is_none());

		let (index, total, title, description) = s.get_current_task_info().unwrap();
		assert_eq!((index, total), (1, 2)); // 1-indexed for display
		assert_eq!(title, "a");
		assert_eq!(description, "do a");
	}

	#[test]
	fn step_details_accumulate_with_a_blank_line_between_entries() {
		let mut s = plan_with(&["a"]);
		assert_eq!(s.get_current_step_details().unwrap(), "");
		s.add_step_details("first".into()).unwrap();
		s.add_step_details("second".into()).unwrap();
		assert_eq!(s.get_current_step_details().unwrap(), "first\n\nsecond");
	}

	#[test]
	fn details_are_per_task_not_shared() {
		let mut s = plan_with(&["a", "b"]);
		s.add_step_details("for a".into()).unwrap();
		s.complete_current_task("done a".into()).unwrap();
		// The new current task starts with empty details.
		assert_eq!(s.get_current_step_details().unwrap(), "");
		s.add_step_details("for b".into()).unwrap();
		assert_eq!(s.get_current_step_details().unwrap(), "for b");
		// …and the completed task kept its own.
		let last = s.get_last_completed_task().unwrap().unwrap();
		assert_eq!(last.details, "for a");
		assert_eq!(last.summary.as_deref(), Some("done a"));
		assert!(matches!(last.status, TaskStatus::Completed));
	}

	#[test]
	fn completing_every_task_exhausts_the_plan() {
		let mut s = plan_with(&["a", "b"]);
		s.complete_current_task("1".into()).unwrap();
		assert!(s.has_more_tasks().unwrap());
		assert_eq!(s.get_completed_task_count().unwrap(), 1);

		s.complete_current_task("2".into()).unwrap();
		assert!(!s.has_more_tasks().unwrap());
		assert_eq!(s.get_completed_task_count().unwrap(), 2);

		// Past the end: current-task accessors fail instead of panicking.
		assert!(s.get_current_task_info().is_err());
		assert!(s.get_current_step_details().is_err());
		assert!(s.add_step_details("late".into()).is_err());
		assert!(s.complete_current_task("extra".into()).is_err());
	}

	#[test]
	fn task_list_marks_only_passed_tasks_completed() {
		let mut s = plan_with(&["a", "b", "c"]);
		s.complete_current_task("1".into()).unwrap();

		let list = s.get_task_list().unwrap();
		assert_eq!(list.len(), 3);
		assert!(matches!(list[0].2, TaskStatus::Completed));
		// The current task and everything after it are still open.
		assert!(matches!(list[1].2, TaskStatus::InProgress));
		assert!(matches!(list[2].2, TaskStatus::InProgress));
	}

	#[test]
	fn message_range_attaches_to_the_task_that_was_just_completed() {
		let mut s = plan_with(&["a", "b"]);
		// Nothing completed yet — there is no task to attach a range to.
		assert!(s.set_current_task_message_range(0, 5).is_err());

		s.complete_current_task("done".into()).unwrap();
		s.set_current_task_message_range(3, 9).unwrap();

		let last = s.get_last_completed_task().unwrap().unwrap();
		let range = last.message_range.expect("range recorded");
		assert_eq!((range.start_index, range.end_index), (3, 9));
	}

	#[test]
	fn completing_the_plan_deactivates_it_without_dropping_data() {
		let mut s = plan_with(&["a"]);
		s.complete_plan("all done".into()).unwrap();
		assert!(!s.has_active_plan().unwrap());
		// The plan itself is still readable for reporting.
		assert_eq!(s.get_plan_title().unwrap(), "demo");
		assert!(matches!(
			s.get_plan().unwrap().status,
			PlanStatus::Completed
		));
	}

	#[test]
	fn clearing_removes_the_plan_entirely() {
		let mut s = plan_with(&["a"]);
		s.clear_plan().unwrap();
		assert!(!s.has_active_plan().unwrap());
		assert!(s.get_plan().is_err());
		// Clearing an already-empty storage is a no-op, not an error.
		assert!(s.clear_plan().is_ok());
	}

	#[test]
	fn load_plan_replaces_the_current_one() {
		let mut s = plan_with(&["a"]);
		let mut other = plan_with(&["x", "y"]);
		other.complete_current_task("1".into()).unwrap();
		let snapshot = other.get_plan().unwrap().clone();

		s.load_plan(snapshot).unwrap();
		assert_eq!(s.get_total_task_count().unwrap(), 2);
		assert_eq!(s.get_current_task_index().unwrap(), 1);
		assert_eq!(s.get_current_task_info().unwrap().2, "y");
	}
}
