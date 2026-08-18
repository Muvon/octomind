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

//! Runtime-owned plan storage, rendering, and sidecar transitions.
//!
//! Lifecycle changes enter through the `sidecar_*` functions under
//! `supervisor::plan`. The specialist can only observe injected plan state and
//! emit sparse hidden signals; there is no MCP command surface.
use super::memory_storage::MemoryPlanStorage;
use super::storage::{PlanStorage, TaskData, TaskStatus};
use anyhow::Result;

use std::sync::{Arc, Mutex};

lazy_static::lazy_static! {
	// CLI-only global storage (fallback when not in session context)
	static ref CLI_PLAN_STORAGE: Arc<Mutex<MemoryPlanStorage>> = Arc::new(Mutex::new(MemoryPlanStorage::new()));
	// CLI-only global task start index (fallback when not in session context)
	static ref CLI_TASK_START_INDEX: Arc<Mutex<Option<usize>>> = Arc::new(Mutex::new(None));
}

/// Get plan storage for the current context.
/// Returns session-scoped storage if in a session, otherwise CLI global.
fn get_storage() -> Arc<Mutex<MemoryPlanStorage>> {
	if let Some(session_id) = crate::session::context::current_session_id() {
		crate::session::context::get_plan_storage(&session_id)
	} else {
		CLI_PLAN_STORAGE.clone()
	}
}

/// Set the start index for the current runtime-owned phase.
pub fn set_current_task_start_index(index: usize) {
	if let Some(session_id) = crate::session::context::current_session_id() {
		crate::session::context::set_task_start_index(&session_id, index);
	} else {
		let mut start_index = CLI_TASK_START_INDEX.lock().unwrap();
		*start_index = Some(index);
	}
	crate::log_debug!("Plan task start index set to: {}", index);
}

/// Get the current task start index without clearing (called when setting message range)
pub fn get_current_task_start_index() -> Option<usize> {
	if let Some(session_id) = crate::session::context::current_session_id() {
		crate::session::context::get_task_start_index(&session_id)
	} else {
		let start_index = CLI_TASK_START_INDEX.lock().unwrap();
		*start_index
	}
}

/// Clear the current task start index (called after successful compression)
/// This allows the next task to set a new start_index
pub fn clear_task_start_index() {
	if let Some(session_id) = crate::session::context::current_session_id() {
		crate::session::context::clear_task_start_index(&session_id);
	} else {
		let mut start_index = CLI_TASK_START_INDEX.lock().unwrap();
		*start_index = None;
	}
	crate::log_debug!("Cleared task start_index after successful compression");
}

/// Check if there's an active plan (for compression hints)
pub fn has_active_plan() -> bool {
	let storage = get_storage();
	let storage = storage.lock().unwrap();
	storage.has_active_plan().unwrap_or(false)
}

/// Appended to plan recitations and the compaction fold prompt when the active
/// plan was last touched before the latest real user message: the plan
/// predates the current request and needs explicit confirmation, not silent
/// obedience.
pub const PLAN_STALENESS_MARKER: &str = "⚠ plan untouched since before the latest user message — confirm it still applies: revise or finish it, or continue it explicitly";

/// Some(marker) when an active plan is stale against the timestamp of the
/// latest real-user task message.
pub fn plan_staleness_marker(latest_task_timestamp: u64) -> Option<&'static str> {
	let storage = get_storage();
	let storage = storage.lock().unwrap();
	if !storage.has_active_plan().unwrap_or(false) {
		return None;
	}
	(storage.touched_at() < latest_task_timestamp).then_some(PLAN_STALENESS_MARKER)
}

/// Plan checklist for recitation with the staleness marker applied — the one
/// place the marker joins the checklist, so the recite and its tests cannot
/// diverge.
pub fn render_plan_checklist_with_staleness(
	latest_task_timestamp: Option<u64>,
) -> Option<String> {
	let mut checklist = render_plan_checklist()?;
	if let Some(marker) = latest_task_timestamp.and_then(plan_staleness_marker) {
		checklist.push_str(marker);
		checklist.push('\n');
	}
	Some(checklist)
}

/// Persist the current plan to the session log (best-effort, no-op outside a session).
fn persist_plan_snapshot() {
	let Some(session_id) = crate::session::context::current_session_id() else {
		return;
	};
	// Clone the plan out while holding the lock briefly, then release before I/O.
	let plan_clone = {
		let storage = get_storage();
		let storage = storage.lock().unwrap();
		match storage.get_plan() {
			Ok(plan) => plan.clone(),
			Err(_) => return,
		}
	};
	if let Err(e) = crate::session::logger::log_plan_snapshot(&session_id, &plan_clone) {
		crate::log_debug!("Failed to log plan snapshot: {}", e);
	}
}

/// Mark the plan as cleared in the session log (best-effort, no-op outside a session).
fn persist_plan_cleared() {
	let Some(session_id) = crate::session::context::current_session_id() else {
		return;
	};
	if let Err(e) = crate::session::logger::log_plan_cleared(&session_id) {
		crate::log_debug!("Failed to log plan cleared: {}", e);
	}
}

fn sidecar_tasks(
	tasks: &[crate::supervisor::plan::PlanTaskDirective],
	minimum: usize,
) -> Result<Vec<TaskData>> {
	if tasks.len() < minimum {
		anyhow::bail!("A sidecar plan needs at least {minimum} meaningful task(s)");
	}
	if tasks.len() > 6 {
		anyhow::bail!("A sidecar plan supports at most 6 meaningful tasks");
	}
	tasks
		.iter()
		.map(|task| {
			let title = task.title.trim();
			let done_when = task.done_when.trim();
			if title.is_empty() || done_when.is_empty() {
				anyhow::bail!("Plan task title and done_when must be non-empty");
			}
			Ok(TaskData::new(
				title.to_string(),
				format!("Done when: {done_when}"),
				None,
				None,
			))
		})
		.collect()
}

/// Start a runtime-owned plan without a tool result or compression side effect.
pub fn sidecar_start(
	title: &str,
	tasks: &[crate::supervisor::plan::PlanTaskDirective],
) -> Result<()> {
	let title = title.trim();
	if title.is_empty() {
		anyhow::bail!("Plan title must be non-empty");
	}
	let tasks = sidecar_tasks(tasks, 2)?;
	let storage = get_storage();
	let mut storage = storage.lock().unwrap();
	if storage.has_active_plan().unwrap_or(false) {
		anyhow::bail!("A plan is already active");
	}
	storage.create_plan(title.to_string(), tasks)?;
	drop(storage);
	persist_plan_snapshot();
	Ok(())
}

/// Complete the current sidecar task. This is deliberately independent from
/// conversation compression: planning tracks work; PACT owns context policy.
pub fn sidecar_advance(summary: &str) -> Result<()> {
	let summary = summary.trim();
	if summary.is_empty() {
		anyhow::bail!("Advance summary must be non-empty");
	}
	let storage = get_storage();
	let mut storage = storage.lock().unwrap();
	if !storage.has_active_plan().unwrap_or(false) {
		anyhow::bail!("No active plan");
	}
	if !storage.has_more_tasks()? {
		anyhow::bail!("All plan tasks are already complete");
	}
	storage.complete_current_task(summary.to_string())?;
	drop(storage);
	persist_plan_snapshot();
	Ok(())
}

/// Revise only the open tail after new evidence invalidates the route.
pub fn sidecar_revise(
	reason: &str,
	tasks: &[crate::supervisor::plan::PlanTaskDirective],
) -> Result<()> {
	if reason.trim().is_empty() {
		anyhow::bail!("Plan revision reason must be non-empty");
	}
	let tasks = sidecar_tasks(tasks, 1)?;
	let storage = get_storage();
	let mut storage = storage.lock().unwrap();
	storage.replace_remaining(tasks)?;
	drop(storage);
	persist_plan_snapshot();
	Ok(())
}

/// Complete every still-open bookkeeping item after completion is accepted by
/// the configured authority (independent verifier, or final `done` when that
/// gate is disabled). The plan is a decomposition aid, not a second source of
/// requirements; its cursor may legitimately lag when one deliverable
/// evidences several phases at once.
fn finish_accepted_plan(storage: &mut dyn PlanStorage, summary: &str) -> Result<()> {
	while storage.has_more_tasks()? {
		storage.complete_current_task(summary.to_string())?;
	}
	storage.complete_plan(summary.to_string())
}

/// Commit an atomic finalization after completion is accepted.
pub fn sidecar_finish(summary: &str) -> Result<()> {
	let summary = summary.trim();
	if summary.is_empty() {
		anyhow::bail!("Plan finish summary must be non-empty");
	}
	let storage = get_storage();
	let mut storage = storage.lock().unwrap();
	if !storage.has_active_plan().unwrap_or(false) {
		return Ok(());
	}
	finish_accepted_plan(&mut *storage, summary)?;
	storage.clear_plan()?;
	drop(storage);
	clear_task_start_index();
	persist_plan_cleared();
	Ok(())
}

/// Restore the active plan (if any) from the session log into session-scoped storage.
/// Called at session startup (all entry points) right after init_session_services.
/// Safe no-op when the log file doesn't exist or contains no snapshot.
pub fn restore_plan_for_session(session_name: &str) {
	let log_file = match crate::session::logger::get_session_log_file(session_name) {
		Ok(p) => p,
		Err(e) => {
			crate::log_debug!("restore_plan_for_session: cannot resolve log file: {}", e);
			return;
		}
	};
	if !log_file.exists() {
		return;
	}

	let file = match std::fs::File::open(&log_file) {
		Ok(f) => f,
		Err(e) => {
			crate::log_debug!("restore_plan_for_session: open failed: {}", e);
			return;
		}
	};

	use std::io::{BufRead, BufReader};
	let decoder = match zstd::stream::read::Decoder::new(file) {
		Ok(d) => d,
		Err(e) => {
			crate::log_debug!("restore_plan_for_session: zstd decoder failed: {}", e);
			return;
		}
	};
	let reader = BufReader::new(decoder);
	let mut latest_plan: Option<super::storage::ExecutionPlan> = None;

	for line in reader.lines().map_while(Result::ok) {
		let Ok(val) = serde_json::from_str::<serde_json::Value>(&line) else {
			continue;
		};
		let Some(t) = val.get("type").and_then(|t| t.as_str()) else {
			continue;
		};
		match t {
			"PLAN_SNAPSHOT" => {
				if let Some(plan_val) = val.get("plan") {
					match serde_json::from_value::<super::storage::ExecutionPlan>(plan_val.clone())
					{
						Ok(plan) => latest_plan = Some(plan),
						Err(e) => {
							crate::log_debug!("restore_plan_for_session: deserialize failed: {}", e)
						}
					}
				}
			}
			"PLAN_CLEARED" => {
				latest_plan = None;
			}
			_ => {}
		}
	}

	if let Some(plan) = latest_plan {
		let session_id = session_name.to_string();
		let storage = crate::session::context::get_plan_storage(&session_id);
		let mut storage = storage.lock().unwrap();
		if let Err(e) = storage.load_plan(plan) {
			crate::log_debug!("restore_plan_for_session: load_plan failed: {}", e);
		} else {
			crate::log_debug!("Restored active plan for session '{}'", session_name);
		}
	}
}

/// Compact live checklist for goal recitation: status icon + title, with only
/// the current phase's observable completion condition expanded. Sync — safe at the pre-request
/// injection point. Returns None when no plan is active.
pub fn render_plan_checklist() -> Option<String> {
	let storage = get_storage();
	let storage = storage.lock().unwrap();
	if !storage.has_active_plan().unwrap_or(false) {
		return None;
	}
	let task_list = storage.get_task_list().ok()?;
	if task_list.is_empty() {
		return None;
	}
	let current = storage
		.get_current_task_info()
		.map(|(c, _, _, _)| c)
		.unwrap_or(0);
	let completed = task_list
		.iter()
		.filter(|(_, _, status)| matches!(status, TaskStatus::Completed))
		.count();

	let mut s = format!("Live plan ({completed}/{} done):\n", task_list.len());
	for (i, (title, description, status)) in task_list.iter().enumerate() {
		let num = i + 1;
		let icon = match status {
			TaskStatus::Completed => "✅",
			TaskStatus::InProgress if num == current => "🔄",
			TaskStatus::InProgress => "⏳",
		};
		let marker = if num == current { " ← current" } else { "" };
		s.push_str(&format!("{icon} {title}{marker}\n"));
		if num == current && !description.trim().is_empty() {
			s.push_str(&format!("   {description}\n"));
		}
	}
	// Surface falsifiable validity conditions on open tasks so the agent sees
	// what each remaining task's approach depends on.
	for (i, task) in storage
		.get_plan()
		.map(|p| p.tasks.clone())
		.unwrap_or_default()
		.iter()
		.enumerate()
	{
		if matches!(task.status, TaskStatus::Completed) {
			continue;
		}
		if let Some(cond) = &task.valid_if {
			s.push_str(&format!("   ⤷ task {} valid if: {cond}\n", i + 1));
		}
	}
	Some(s)
}

/// Full manager/verifier view of the runtime-owned plan. Unlike recitation,
/// this expands every phase's outcome so an external decision never relies on
/// titles alone.
pub fn render_plan_details() -> Option<String> {
	let storage = get_storage();
	let storage = storage.lock().unwrap();
	if !storage.has_active_plan().unwrap_or(false) {
		return None;
	}
	let plan = storage.get_plan().ok()?;
	let mut output = format!("Plan: {}\n", plan.title);
	for (index, task) in plan.tasks.iter().enumerate() {
		let state = if index < plan.current_task_index {
			"completed"
		} else if index == plan.current_task_index {
			"current"
		} else {
			"pending"
		};
		output.push_str(&format!(
			"{}. [{}] {} — {}\n",
			index + 1,
			state,
			task.title,
			task.description
		));
	}
	Some(output)
}

/// Evaluate one validity condition deterministically. Returns Some(true) when
/// it holds, Some(false) when it is broken, None when the condition is not
/// machine-checkable (free-form prose — left for the agent/verifier to judge).
/// Supported grammar: `file_exists: <path>` / `file_absent: <path>`.
fn check_condition(cond: &str) -> Option<bool> {
	let (op, path) = cond.split_once(':')?;
	let path = path.trim();
	if path.is_empty() {
		return None;
	}
	match op.trim().to_ascii_lowercase().as_str() {
		"file_exists" => Some(std::path::Path::new(path).exists()),
		"file_absent" => Some(!std::path::Path::new(path).exists()),
		_ => None,
	}
}

/// Open tasks whose declared `valid_if` condition is deterministically broken
/// right now. Returns (task_number, title, condition) per broken item. Cheap:
/// filesystem stat per checkable condition, no model call.
pub fn broken_plan_conditions() -> Vec<(usize, String, String)> {
	let storage = get_storage();
	let storage = storage.lock().unwrap();
	if !storage.has_active_plan().unwrap_or(false) {
		return Vec::new();
	}
	let Ok(plan) = storage.get_plan() else {
		return Vec::new();
	};
	let mut broken = Vec::new();
	for (i, task) in plan.tasks.iter().enumerate() {
		if i < plan.current_task_index || matches!(task.status, TaskStatus::Completed) {
			continue;
		}
		let Some(cond) = &task.valid_if else {
			continue;
		};
		if check_condition(cond) == Some(false) {
			broken.push((i + 1, task.title.clone(), cond.clone()));
		}
	}
	broken
}

/// Get current plan display for session commands
pub async fn get_current_plan_display() -> Result<String> {
	let storage = get_storage();
	let storage = storage.lock().unwrap();

	// Check if plan exists
	if !storage.has_active_plan().unwrap_or(false) {
		return Err(anyhow::anyhow!("No active plan. Complex plans are created automatically by the external planner; focused work remains plan-free."));
	}

	let plan_title = storage
		.get_plan_title()
		.unwrap_or_else(|_| "Unknown Plan".to_string());
	let task_list = storage.get_task_list().unwrap_or_else(|_| Vec::new());
	let (current, total, current_task_title, current_task_description) = storage
		.get_current_task_info()
		.unwrap_or((0, 0, "Unknown".to_string(), "No description".to_string()));

	let mut response = format!("PLAN: {plan_title}\n\nTASKS:\n");

	for (i, (task_title, task_description, status)) in task_list.iter().enumerate() {
		let task_num = i + 1;
		let status_icon = match status {
			TaskStatus::Completed => "✅",
			TaskStatus::InProgress => {
				if task_num == current {
					"🔄"
				} else {
					"⏳"
				}
			}
		};

		let status_text = if task_num == current {
			" (IN PROGRESS)"
		} else {
			"" // Both completed and pending tasks show no additional text
		};

		response.push_str(&format!(
			"{status_icon} {task_num}. {task_title}{status_text}\n"
		));
		let description_lines: Vec<&str> = task_description.lines().collect();
		for line in description_lines {
			response.push_str(&format!("   📝 {}\n", line));
		}
		response.push('\n'); // Extra line between tasks
	}

	if current <= total {
		response.push_str(&format!(
			"CURRENT: Task {current}/{total} - {current_task_title}\n📝 {current_task_description}"
		));
	}

	Ok(response)
}

/// Get current plan as JSON for session commands
pub async fn get_current_plan_json() -> Result<serde_json::Value> {
	let storage = get_storage();
	let storage = storage.lock().unwrap();

	// Check if plan exists
	if !storage.has_active_plan().unwrap_or(false) {
		return Err(anyhow::anyhow!("No active plan"));
	}

	let plan_title = storage
		.get_plan_title()
		.unwrap_or_else(|_| "Unknown Plan".to_string());
	let task_list = storage.get_task_list().unwrap_or_else(|_| Vec::new());
	let (current, total, current_task_title, current_task_description) = storage
		.get_current_task_info()
		.unwrap_or((0, 0, "Unknown".to_string(), "No description".to_string()));

	Ok(serde_json::json!({
		"plan_title": plan_title,
		"current_task": current,
		"total_tasks": total,
		"current_task_title": current_task_title,
		"current_task_description": current_task_description,
		"tasks": task_list.iter().map(|(title, desc, status)| {
			serde_json::json!({
				"title": title,
				"description": desc,
				"status": match status {
					TaskStatus::Completed => "completed",
					TaskStatus::InProgress => "in_progress"
				}
			})
		}).collect::<Vec<_>>()
	}))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test]
	async fn staleness_marker_fires_only_for_an_active_plan_older_than_the_request() {
		// A unique session id scopes plan storage to this test — no global
		// contamination across parallel tests.
		crate::session::context::with_session_id("plan-staleness-test".to_string(), async {
			// No plan → no marker regardless of timestamp.
			assert_eq!(plan_staleness_marker(u64::MAX), None);

			let storage = get_storage();
			storage
				.lock()
				.unwrap()
				.create_plan(
					"demo".to_string(),
					vec![TaskData::new("a".into(), "do a".into(), None, None)],
				)
				.unwrap();

			// Fresh plan against an older request → no marker.
			assert_eq!(plan_staleness_marker(1), None);

			// Plan untouched since before the latest request → marker; the
			// same second is NOT stale (the mutation follows the message
			// within one turn).
			storage.lock().unwrap().set_touched_at(10);
			assert_eq!(plan_staleness_marker(11), Some(PLAN_STALENESS_MARKER));
			assert_eq!(plan_staleness_marker(10), None);

			// The recitation renderer appends the marker as the last line —
			// and only when actually stale.
			let recited = render_plan_checklist_with_staleness(Some(11)).unwrap();
			assert!(recited.starts_with("Live plan"));
			assert!(recited.trim_end().ends_with(PLAN_STALENESS_MARKER));
			let fresh = render_plan_checklist_with_staleness(Some(9)).unwrap();
			assert!(!fresh.contains(PLAN_STALENESS_MARKER));
			// No user task timestamp available → no marker.
			let unknown = render_plan_checklist_with_staleness(None).unwrap();
			assert!(!unknown.contains(PLAN_STALENESS_MARKER));

			// Engaging the plan clears the marker (self-clearing steering).
			storage
				.lock()
				.unwrap()
				.complete_current_task("done a".into())
				.unwrap();
			assert_eq!(plan_staleness_marker(11), None);

			// A finished plan is not recited and cannot be stale.
			storage
				.lock()
				.unwrap()
				.complete_plan("done".into())
				.unwrap();
			assert_eq!(plan_staleness_marker(u64::MAX), None);
			assert_eq!(render_plan_checklist_with_staleness(Some(u64::MAX)), None);
		})
		.await;
	}

	#[test]
	fn condition_check_only_answers_the_machine_checkable() {
		assert_eq!(check_condition("file_exists: Cargo.toml"), Some(true));
		assert_eq!(
			check_condition("file_exists: definitely/not/a/real/path.xyz"),
			Some(false)
		);
		assert_eq!(
			check_condition("file_absent: definitely/not/a/real/path.xyz"),
			Some(true)
		);
		assert_eq!(check_condition("file_absent: Cargo.toml"), Some(false));
		// Free-form prose is not machine-checkable — left to the agent/verifier.
		assert_eq!(check_condition("the API still returns 200"), None);
		assert_eq!(check_condition("file_exists:"), None);
	}

	#[test]
	fn sidecar_task_count_is_bounded() {
		let task = crate::supervisor::plan::PlanTaskDirective {
			title: "phase".to_string(),
			done_when: "outcome exists".to_string(),
		};
		assert!(sidecar_tasks(std::slice::from_ref(&task), 2).is_err());
		let six = vec![task.clone(); 6];
		assert!(sidecar_tasks(&six, 2).is_ok());
		let seven = vec![task; 7];
		assert!(sidecar_tasks(&seven, 2).is_err());
	}

	#[test]
	fn verified_finish_closes_every_remaining_plan_item_atomically() {
		let mut storage = MemoryPlanStorage::new();
		storage
			.create_plan(
				"review".to_string(),
				["inspect", "synthesize", "deliver"]
					.into_iter()
					.map(|title| {
						TaskData::new(
							title.to_string(),
							format!("Done when: {title} is evidenced"),
							None,
							None,
						)
					})
					.collect(),
			)
			.unwrap();
		storage
			.complete_current_task("inspection already evidenced".to_string())
			.unwrap();

		finish_accepted_plan(&mut storage, "full deliverable verified").unwrap();

		assert!(!storage.has_active_plan().unwrap());
		assert_eq!(storage.get_completed_task_count().unwrap(), 3);
		let tasks = &storage.get_plan().unwrap().tasks;
		assert_eq!(
			tasks[0].summary.as_deref(),
			Some("inspection already evidenced")
		);
		assert!(tasks
			.iter()
			.all(|task| matches!(task.status, TaskStatus::Completed)));
		assert!(tasks[1..]
			.iter()
			.all(|task| task.summary.as_deref() == Some("full deliverable verified")));
	}
}
