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

// Global storage instance - using memory storage for now
lazy_static::lazy_static! {
	static ref PLAN_STORAGE: Arc<Mutex<MemoryPlanStorage>> = Arc::new(Mutex::new(MemoryPlanStorage::new()));
	// Track when the current task started (message index)
	// This is set by the session when plan(start) or plan(next) is called
	static ref CURRENT_TASK_START_INDEX: Arc<Mutex<Option<usize>>> = Arc::new(Mutex::new(None));
	// Guard against parallel plan tool execution
	static ref PLAN_TOOL_EXECUTING: Arc<Mutex<bool>> = Arc::new(Mutex::new(false));
}

/// Set the start index for the current task (called by session before plan tool execution)
pub fn set_current_task_start_index(index: usize) -> Result<()> {
	let mut executing = PLAN_TOOL_EXECUTING.lock().unwrap();
	if *executing {
		return Err(anyhow::anyhow!(
			"Cannot set start index: another plan tool is currently executing. \
			Plan tools must execute sequentially to ensure correct compression."
		));
	}
	*executing = true;

	let mut start_index = CURRENT_TASK_START_INDEX.lock().unwrap();
	*start_index = Some(index);
	crate::log_debug!("Plan task start index set to: {}", index);
	Ok(())
}

/// Mark plan tool execution as complete (called after tool execution)
pub fn clear_plan_tool_executing() {
	let mut executing = PLAN_TOOL_EXECUTING.lock().unwrap();
	*executing = false;
	crate::log_debug!("Plan tool execution guard cleared");
}

/// Get and clear the current task start index (called when setting message range)
pub fn get_and_clear_start_index() -> Option<usize> {
	let mut start_index = CURRENT_TASK_START_INDEX.lock().unwrap();
	start_index.take()
}

/// Check if there's an active plan (for compression hints)
pub fn has_active_plan() -> bool {
	let storage = PLAN_STORAGE.lock().unwrap();
	storage.has_active_plan().unwrap_or(false)
}

/// Set message range for the last completed task (called from session after plan(next))
pub fn set_last_task_message_range(start_index: usize, end_index: usize) -> Result<()> {
	let mut storage = PLAN_STORAGE.lock().unwrap();
	storage.set_current_task_message_range(start_index, end_index)
}

/// Get the last completed task for compression (called from session)
pub fn get_last_completed_task_for_compression() -> Option<super::storage::PlanTask> {
	let storage = PLAN_STORAGE.lock().unwrap();
	storage.get_last_completed_task().ok().flatten()
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
	// Extract title parameter
	let title = match call.parameters.get("title") {
		Some(Value::String(t)) => {
			if t.trim().is_empty() {
				return Ok(McpToolResult::error(
					call.tool_name.clone(),
					call.tool_id.clone(),
					"Title parameter cannot be empty".to_string(),
				));
			}
			t.clone()
		}
		Some(_) => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Title parameter must be a string".to_string(),
			));
		}
		None => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Missing required parameter 'title'".to_string(),
			));
		}
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
	let mut storage = PLAN_STORAGE.lock().unwrap();

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

	Ok(McpToolResult::success(
		call.tool_name.clone(),
		call.tool_id.clone(),
		response,
	))
}

/// Handle step command - add details or get current details
async fn handle_step_command(call: &McpToolCall) -> Result<McpToolResult> {
	let storage = PLAN_STORAGE.lock().unwrap();

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
			let mut storage = PLAN_STORAGE.lock().unwrap();
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

	let mut storage = PLAN_STORAGE.lock().unwrap();

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

	// Request task compression if we have a completed task
	if let Some(task) = completed_task {
		super::compression::request_compression(task);
	}

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
	let storage = PLAN_STORAGE.lock().unwrap();

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

	let mut storage = PLAN_STORAGE.lock().unwrap();

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

	drop(storage);

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
	let mut storage = PLAN_STORAGE.lock().unwrap();

	if let Err(e) = storage.clear_plan() {
		return Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("Failed to reset plan: {e}"),
		));
	}

	Ok(McpToolResult::success(
		call.tool_name.clone(),
		call.tool_id.clone(),
		"Plan data cleared successfully".to_string(),
	))
}

/// Clear plan data (called from session done command)
pub async fn clear_plan_data() -> Result<()> {
	let mut storage = PLAN_STORAGE.lock().unwrap();
	storage.clear_plan()
}

/// Get completed task count for compression hints
pub fn get_completed_task_count() -> Result<usize> {
	let storage = PLAN_STORAGE.lock().unwrap();
	storage.get_completed_task_count()
}

/// Get current plan display for session commands
pub async fn get_current_plan_display() -> Result<String> {
	let storage = PLAN_STORAGE.lock().unwrap();

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
	let storage = PLAN_STORAGE.lock().unwrap();

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
