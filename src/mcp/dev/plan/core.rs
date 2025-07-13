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
use super::storage::{PlanStorage, TaskStatus};
use crate::mcp::{McpToolCall, McpToolResult};
use anyhow::Result;
use serde_json::Value;

use std::sync::{Arc, Mutex};

// Global storage instance - using memory storage for now
lazy_static::lazy_static! {
	static ref PLAN_STORAGE: Arc<Mutex<MemoryPlanStorage>> = Arc::new(Mutex::new(MemoryPlanStorage::new()));
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
				"Unknown command '{}'. Available commands: start, step, next, list, done, reset",
				command
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

	// Extract tasks parameter
	let tasks = match call.parameters.get("tasks") {
		Some(Value::Array(task_array)) => {
			let mut tasks = Vec::new();
			for (i, task_value) in task_array.iter().enumerate() {
				match task_value {
					Value::String(task_str) => {
						if task_str.trim().is_empty() {
							return Ok(McpToolResult::error(
								call.tool_name.clone(),
								call.tool_id.clone(),
								format!("Task {} cannot be empty", i + 1),
							));
						}
						tasks.push(task_str.clone());
					}
					_ => {
						return Ok(McpToolResult::error(
							call.tool_name.clone(),
							call.tool_id.clone(),
							format!("Task {} must be a string", i + 1),
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
				"Tasks parameter must be an array of strings".to_string(),
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
			format!("Failed to create plan: {}", e),
		));
	}

	// Build response
	let mut response = format!("PLAN CREATED: {}\n\nTASKS:\n", title);
	for (i, task) in tasks.iter().enumerate() {
		response.push_str(&format!("{}. {}\n", i + 1, task));
	}
	response.push_str(&format!("\nCURRENT: Task 1/{} - {}", tasks.len(), tasks[0]));

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
					format!("Failed to add step details: {}", e),
				));
			}

			let (current, total, task_title) =
				storage
					.get_current_task_info()
					.unwrap_or((0, 0, "Unknown".to_string()));

			Ok(McpToolResult::success(
				call.tool_name.clone(),
				call.tool_id.clone(),
				format!(
					"Step details added to Task {}/{} - {}",
					current, total, task_title
				),
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
			let (current, total, task_title) =
				storage
					.get_current_task_info()
					.unwrap_or((0, 0, "Unknown".to_string()));

			let response = if details.is_empty() {
				format!(
					"CURRENT TASK ({}/{}): {}\n\nNo details recorded yet.",
					current, total, task_title
				)
			} else {
				format!(
					"CURRENT TASK ({}/{}): {}\n\nDETAILS:\n{}",
					current, total, task_title, details
				)
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
			format!("Failed to complete task: {}", e),
		));
	}

	// Check if more tasks remain
	let has_more = storage.has_more_tasks().unwrap_or(false);
	let plan_title = storage
		.get_plan_title()
		.unwrap_or_else(|_| "Unknown Plan".to_string());

	let response = if has_more {
		let (current, total, task_title) =
			storage
				.get_current_task_info()
				.unwrap_or((0, 0, "Unknown".to_string()));
		format!(
			"Task completed: {}\n\nNEXT TASK ({}/{}): {}",
			content, current, total, task_title
		)
	} else {
		format!("Final task completed: {}\n\nAll tasks in plan '{}' are now complete. Use 'done' command to finalize.", content, plan_title)
	};

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
	let (current, total, current_task_title) =
		storage
			.get_current_task_info()
			.unwrap_or((0, 0, "Unknown".to_string()));

	let mut response = format!("PLAN: {}\n\nTASKS:\n", plan_title);

	for (i, (task_title, status)) in task_list.iter().enumerate() {
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
			"{} {}. {}{}\n",
			status_icon, task_num, task_title, status_text
		));
	}

	if current <= total {
		response.push_str(&format!(
			"\nCURRENT: Task {}/{} - {}",
			current, total, current_task_title
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

	// Complete plan
	if let Err(e) = storage.complete_plan(content.clone()) {
		return Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("Failed to complete plan: {}", e),
		));
	}

	let response = format!(
		"PLAN COMPLETED: {}\n\nFINAL SUMMARY: {}",
		plan_title, content
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
			format!("Failed to reset plan: {}", e),
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
