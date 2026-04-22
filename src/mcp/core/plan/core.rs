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

//! Core plan tool implementation
/// Core implementation of the plan MCP tool.
///
/// Handles all supported commands (start, step, next, list, done, reset).
///
/// - Validates all parameters with clear MCP-compliant error messages.
/// - Ensures all errors use Ok(McpToolResult::error(...))—never Err().
/// - Handles cancellation, session cleanup, and output format.
///
/// # Commands and Parameters
/// - `command` (string): required; one of start, step, next, list, done, reset
///     - start: requires `title` (string) and `tasks` (array)
///     - step/next/done: require `content` (string)
///
/// Output: Always {content: [{type: "text", text: ...}], isError: ...} and includes tool_id.
use super::memory_storage::MemoryPlanStorage;
use super::storage::{PlanStorage, TaskData, TaskStatus};
use crate::mcp::{McpToolCall, McpToolResult};
use anyhow::Result;
use serde_json::Value;

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

/// Set the start index for the current task (called by session before plan tool execution)
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

/// Get and clear the current task start index (called when setting message range)
pub fn get_and_clear_start_index() -> Option<usize> {
	if let Some(session_id) = crate::session::context::current_session_id() {
		crate::session::context::take_task_start_index(&session_id)
	} else {
		let mut start_index = CLI_TASK_START_INDEX.lock().unwrap();
		start_index.take()
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
	let reader = BufReader::new(file);
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

/// Set message range for the last completed task (called from session after plan(next))
pub fn set_last_task_message_range(start_index: usize, end_index: usize) -> Result<()> {
	let storage = get_storage();
	let mut storage = storage.lock().unwrap();
	storage.set_current_task_message_range(start_index, end_index)
}

/// Get the last completed task for compression (called from session)
pub fn get_last_completed_task_for_compression() -> Option<super::storage::PlanTask> {
	let storage = get_storage();
	let storage = storage.lock().unwrap();
	storage.get_last_completed_task().ok().flatten()
}

/// Get current plan context for compression (plan title, progress, current task)
pub fn get_plan_context() -> Option<(String, usize, usize, String)> {
	let storage = get_storage();
	let storage = storage.lock().unwrap();
	if !storage.has_active_plan().unwrap_or(false) {
		return None;
	}

	let plan_title = storage.get_plan_title().ok()?;
	let completed_count = storage.get_completed_task_count().ok()?;
	let (_current_idx, total, current_title, _) = storage.get_current_task_info().ok()?;

	Some((plan_title, completed_count, total, current_title))
}

/// Execute plan tool command
pub async fn execute_plan(call: &McpToolCall) -> Result<McpToolResult> {
	// Extract command parameter
	let command = match call.parameters.get("command") {
		Some(Value::String(cmd)) => {
			if cmd.trim().is_empty() {
				return Ok(McpToolResult::error(
					call.tool_name.clone(),
					call.tool_id.clone(),
					"Command parameter cannot be empty".to_string(),
				));
			}
			cmd.clone()
		}
		Some(_) => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Command parameter must be a string".to_string(),
			));
		}
		None => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Missing required parameter 'command'".to_string(),
			));
		}
	};

	// Route to appropriate command handler
	match command.as_str() {
		"start" => handle_start_command(call).await,
		"step" => handle_step_command(call).await,
		"next" => handle_next_command(call).await,
		"list" => handle_list_command(call).await,
		"done" => handle_done_command(call).await,
		"reset" => handle_reset_command(call).await,
		_ => Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!(
				"Unknown command '{command}'. Available commands: start, step, next, list, done, reset"
			),
		)),
	}
}

/// Handle start command - create new plan
async fn handle_start_command(call: &McpToolCall) -> Result<McpToolResult> {
	// Use content as the plan title/goal
	let title = match call.parameters.get("content") {
		Some(Value::String(t)) if !t.trim().is_empty() => t.clone(),
		_ => "Active Plan".to_string(),
	};

	// Extract tasks parameter - ONLY detailed objects supported
	let tasks = match call.parameters.get("tasks") {
		Some(Value::Array(task_array)) => {
			let mut tasks = Vec::new();
			for (i, task_value) in task_array.iter().enumerate() {
				match task_value {
					// Handle detailed task objects (ONLY supported format)
					Value::Object(task_obj) => {
						let title = match task_obj.get("title") {
							Some(Value::String(t)) => {
								if t.trim().is_empty() {
									return Ok(McpToolResult::error(
										call.tool_name.clone(),
										call.tool_id.clone(),
										format!("Task {} title cannot be empty", i + 1),
									));
								}
								t.clone()
							}
							Some(_) => {
								return Ok(McpToolResult::error(
									call.tool_name.clone(),
									call.tool_id.clone(),
									format!("Task {} title must be a string", i + 1),
								));
							}
							None => {
								return Ok(McpToolResult::error(
									call.tool_name.clone(),
									call.tool_id.clone(),
									format!("Task {} missing required 'title' field", i + 1),
								));
							}
						};

						let description = match task_obj.get("description") {
							Some(Value::String(d)) => {
								if d.trim().is_empty() {
									return Ok(McpToolResult::error(
										call.tool_name.clone(),
										call.tool_id.clone(),
										format!("Task {} description cannot be empty", i + 1),
									));
								}
								d.clone()
							}
							Some(_) => {
								return Ok(McpToolResult::error(
									call.tool_name.clone(),
									call.tool_id.clone(),
									format!("Task {} description must be a string", i + 1),
								));
							}
							None => {
								return Ok(McpToolResult::error(
									call.tool_name.clone(),
									call.tool_id.clone(),
									format!("Task {} missing required 'description' field", i + 1),
								));
							}
						};

						// Optional phase field
						let phase = match task_obj.get("phase") {
							Some(Value::String(p)) => {
								if p.trim().is_empty() {
									None
								} else {
									Some(p.clone())
								}
							}
							Some(Value::Null) => None,
							Some(_) => {
								return Ok(McpToolResult::error(
									call.tool_name.clone(),
									call.tool_id.clone(),
									format!("Task {} phase must be a string", i + 1),
								));
							}
							None => None,
						};

						tasks.push(TaskData::new(title, description, phase));
					}
					_ => {
						return Ok(McpToolResult::error(
							call.tool_name.clone(),
							call.tool_id.clone(),
							format!("Task {} must be an object with 'title' and 'description' fields. Simple strings are no longer supported - use detailed task objects for better context recovery.", i + 1),
						));
					}
				}
			}

			if tasks.is_empty() {
				return Ok(McpToolResult::error(
					call.tool_name.clone(),
					call.tool_id.clone(),
					"Tasks array cannot be empty".to_string(),
				));
			}

			tasks
		}
		Some(_) => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Tasks parameter must be an array of detailed task objects with 'title' and 'description' fields".to_string(),
			));
		}
		None => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Missing required parameter 'tasks'".to_string(),
			));
		}
	};

	// Create plan - but first check if one already exists
	let storage = get_storage();
	let mut storage = storage.lock().unwrap();

	// Safety check: prevent accidental overwrite of existing plan
	if storage.has_active_plan().unwrap_or(false) {
		return Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			"Active plan already exists. Use 'done' to complete current plan, 'reset' to clear it, or 'list' to view current progress before starting a new plan.".to_string(),
		));
	}

	if let Err(e) = storage.create_plan(title.clone(), tasks.clone()) {
		return Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("Failed to create plan: {e}"),
		));
	}

	// Release lock before persisting to the session log.
	drop(storage);
	persist_plan_snapshot();

	// Build response
	let mut response = format!("PLAN CREATED: {title}\n\nTASKS:\n");
	for (i, task) in tasks.iter().enumerate() {
		response.push_str(&format!("{}. {}\n", i + 1, task.title));
		response.push_str(&format!("   📝 {}\n", task.description));
	}
	response.push_str(&format!(
		"\nCURRENT: Task 1/{} - {}",
		tasks.len(),
		tasks[0].title
	));

	// CRITICAL FIX: Set start_index for first task when plan is created
	// This will be used by compression to know where the first task's work begins
	// Note: We can't get message_count here (no session access), so we signal
	// that start_index should be set in response.rs when plan tool returns
	// The flag will be checked and start_index will be set AFTER plan(start) completes

	Ok(McpToolResult::success(
		call.tool_name.clone(),
		call.tool_id.clone(),
		response,
	))
}

/// Handle step command - add details or get current details
async fn handle_step_command(call: &McpToolCall) -> Result<McpToolResult> {
	let storage = get_storage();
	let storage = storage.lock().unwrap();

	// Check if plan exists
	if !storage.has_active_plan().unwrap_or(false) {
		return Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			"No active plan. Use 'start' command to create a plan first.".to_string(),
		));
	}

	// Check if content parameter exists
	match call.parameters.get("content") {
		Some(Value::String(content)) => {
			if content.trim().is_empty() {
				return Ok(McpToolResult::error(
					call.tool_name.clone(),
					call.tool_id.clone(),
					"Content parameter cannot be empty".to_string(),
				));
			}

			// Add step details
			drop(storage);
			let storage = get_storage();
			let mut storage = storage.lock().unwrap();
			if let Err(e) = storage.add_step_details(content.clone()) {
				return Ok(McpToolResult::error(
					call.tool_name.clone(),
					call.tool_id.clone(),
					format!("Failed to add step details: {e}"),
				));
			}

			let (current, total, task_title, _task_description) = storage
				.get_current_task_info()
				.unwrap_or((0, 0, "Unknown".to_string(), "No description".to_string()));

			drop(storage);
			persist_plan_snapshot();

			Ok(McpToolResult::success(
				call.tool_name.clone(),
				call.tool_id.clone(),
				format!("Step details added to Task {current}/{total} - {task_title}"),
			))
		}
		Some(_) => Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			"Content parameter must be a string".to_string(),
		)),
		None => {
			// Get current step details
			let details = storage
				.get_current_step_details()
				.unwrap_or_else(|_| "No details recorded yet".to_string());
			let (current, total, task_title, _task_description) = storage
				.get_current_task_info()
				.unwrap_or((0, 0, "Unknown".to_string(), "No description".to_string()));

			let response = if details.is_empty() {
				format!(
					"CURRENT TASK ({current}/{total}): {task_title}\n\nNo details recorded yet."
				)
			} else {
				format!("CURRENT TASK ({current}/{total}): {task_title}\n\nDETAILS:\n{details}")
			};

			Ok(McpToolResult::success(
				call.tool_name.clone(),
				call.tool_id.clone(),
				response,
			))
		}
	}
}

/// Handle next command - complete current task and move forward
async fn handle_next_command(call: &McpToolCall) -> Result<McpToolResult> {
	// Extract content parameter
	let content = match call.parameters.get("content") {
		Some(Value::String(c)) => {
			if c.trim().is_empty() {
				return Ok(McpToolResult::error(
					call.tool_name.clone(),
					call.tool_id.clone(),
					"Content parameter cannot be empty".to_string(),
				));
			}
			c.clone()
		}
		Some(_) => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Content parameter must be a string".to_string(),
			));
		}
		None => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Missing required parameter 'content'".to_string(),
			));
		}
	};

	let storage = get_storage();
	let mut storage = storage.lock().unwrap();

	// Check if plan exists
	if !storage.has_active_plan().unwrap_or(false) {
		return Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			"No active plan. Use 'start' command to create a plan first.".to_string(),
		));
	}

	// Complete current task
	if let Err(e) = storage.complete_current_task(content.clone()) {
		return Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("Failed to complete task: {e}"),
		));
	}

	// Get the completed task for compression
	let completed_task = storage.get_last_completed_task().ok().flatten();
	let completed_task_phase = completed_task.as_ref().and_then(|t| t.phase.clone());

	// Check if more tasks remain
	let has_more = storage.has_more_tasks().unwrap_or(false);
	let plan_title = storage
		.get_plan_title()
		.unwrap_or_else(|_| "Unknown Plan".to_string());

	// Check if this completes a phase (last task of a phase)
	let phase_completed = if let Some(ref phase_name) = completed_task_phase {
		// Check if next task has different phase or no more tasks
		if has_more {
			let (_, _, _, _) = storage.get_current_task_info().unwrap_or((
				0,
				0,
				"Unknown".to_string(),
				"No description".to_string(),
			));
			// Get next task phase
			let next_task_phase = storage.get_plan().ok().and_then(|plan| {
				plan.tasks
					.get(plan.current_task_index)
					.and_then(|t| t.phase.clone())
			});
			next_task_phase.as_ref() != Some(phase_name)
		} else {
			true // Last task always completes its phase
		}
	} else {
		false
	};

	let response = if has_more {
		let (current, total, task_title, _task_description) = storage
			.get_current_task_info()
			.unwrap_or((0, 0, "Unknown".to_string(), "No description".to_string()));
		format!("Task completed: {content}\n\nNEXT TASK ({current}/{total}): {task_title}")
	} else {
		format!("Final task completed: {content}\n\nAll tasks in plan '{plan_title}' are now complete. Use 'done' command to finalize.")
	};

	drop(storage);
	persist_plan_snapshot();

	// Request task compression if we have a completed task
	if let Some(task) = completed_task {
		super::compression::request_compression(task);
	}

	// NOTE: start_index is NOT cleared here anymore!
	// It will only be cleared in compression.rs AFTER successful compression
	// This allows multiple tasks to accumulate if compression keeps getting skipped
	// (e.g., when tasks have minimal work and compression would add tokens instead of saving)

	// Automatic phase compression: trigger when phase completes
	if phase_completed {
		if let Some(phase_name) = completed_task_phase {
			super::compression::request_phase_compression(
				phase_name.clone(),
				(0, 0), // Will be calculated in compression logic
				format!("Phase '{}' completed", phase_name),
			);
		}
	}

	Ok(McpToolResult::success(
		call.tool_name.clone(),
		call.tool_id.clone(),
		response,
	))
}

/// Handle list command - show task list with progress
async fn handle_list_command(call: &McpToolCall) -> Result<McpToolResult> {
	let storage = get_storage();
	let storage = storage.lock().unwrap();

	// Check if plan exists
	if !storage.has_active_plan().unwrap_or(false) {
		return Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			"No active plan. Use 'start' command to create a plan first.".to_string(),
		));
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

		// Add description with proper indentation
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

	Ok(McpToolResult::success(
		call.tool_name.clone(),
		call.tool_id.clone(),
		response,
	))
}

/// Handle done command - complete entire plan
async fn handle_done_command(call: &McpToolCall) -> Result<McpToolResult> {
	// Extract content parameter
	let content = match call.parameters.get("content") {
		Some(Value::String(c)) => {
			if c.trim().is_empty() {
				return Ok(McpToolResult::error(
					call.tool_name.clone(),
					call.tool_id.clone(),
					"Content parameter cannot be empty".to_string(),
				));
			}
			c.clone()
		}
		Some(_) => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Content parameter must be a string".to_string(),
			));
		}
		None => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Missing required parameter 'content'".to_string(),
			));
		}
	};

	let storage = get_storage();
	let mut storage = storage.lock().unwrap();

	// Check if plan exists
	if !storage.has_active_plan().unwrap_or(false) {
		return Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			"No active plan. Use 'start' command to create a plan first.".to_string(),
		));
	}

	let plan_title = storage
		.get_plan_title()
		.unwrap_or_else(|_| "Unknown Plan".to_string());
	let total_tasks = storage.get_total_task_count().unwrap_or(0);
	let total_phases = storage.get_phase_count().unwrap_or(0);

	// Complete plan
	if let Err(e) = storage.complete_plan(content.clone()) {
		return Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("Failed to complete plan: {e}"),
		));
	}

	// Get last completed task before dropping storage — needed for forced compression
	let last_task = storage.get_last_completed_task().ok().flatten();

	drop(storage);
	persist_plan_cleared();

	// If start_index is still set, the last task's compression was skipped (e.g. 20% threshold).
	// Force-compress it now so project compression has material to consolidate.
	if get_current_task_start_index().is_some() {
		if let Some(task) = last_task {
			super::compression::request_forced_compression(task);
		}
	}

	// Automatically request project compression
	super::compression::request_project_compression(
		plan_title.clone(),
		content.clone(),
		total_tasks,
		total_phases,
	);

	let response = format!(
		"PLAN COMPLETED: {}\n\n\
		 Total Tasks: {}\n\
		 Total Phases: {}\n\n\
		 FINAL SUMMARY: {}",
		plan_title, total_tasks, total_phases, content
	);

	Ok(McpToolResult::success(
		call.tool_name.clone(),
		call.tool_id.clone(),
		response,
	))
}

/// Handle reset command - clear all plan data
async fn handle_reset_command(call: &McpToolCall) -> Result<McpToolResult> {
	let storage = get_storage();
	let mut storage = storage.lock().unwrap();

	if let Err(e) = storage.clear_plan() {
		return Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("Failed to reset plan: {e}"),
		));
	}

	drop(storage);
	persist_plan_cleared();

	Ok(McpToolResult::success(
		call.tool_name.clone(),
		call.tool_id.clone(),
		"Plan data cleared successfully".to_string(),
	))
}

/// Clear plan data (called from session done command)
pub async fn clear_plan_data() -> Result<()> {
	let storage = get_storage();
	let mut storage = storage.lock().unwrap();
	storage.clear_plan()?;
	drop(storage);
	persist_plan_cleared();
	Ok(())
}

/// Get completed task count for compression hints
pub fn get_completed_task_count() -> Result<usize> {
	let storage = get_storage();
	let storage = storage.lock().unwrap();
	storage.get_completed_task_count()
}

/// Get current plan display for session commands
pub async fn get_current_plan_display() -> Result<String> {
	let storage = get_storage();
	let storage = storage.lock().unwrap();

	// Check if plan exists
	if !storage.has_active_plan().unwrap_or(false) {
		return Err(anyhow::anyhow!("Use plan tool only for COMPLEX, multi-step tasks that require structured breakdown. For simple tasks, just execute them directly without a plan."));
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

		// Add description with proper indentation
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
