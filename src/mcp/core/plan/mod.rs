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

//! Plan tool - structured task execution with step-by-step progression
/// MCP Tool: plan
///
/// Provides structured, step-by-step task execution and progress tracking for Octomind sessions.
/// Commands:
///   - start: Begin a new plan. Requires `title` (string) and `tasks` (array of strings).
///   - step: Add progress to the current step. Requires `content` (string).
///   - next: Mark current step as complete. Requires `content` (string).
///   - list: Show full plan progress with completion status.
///   - done: Complete the plan, optionally with `content` summary.
///   - reset: Abort and clear the plan.
///
/// Parameters are strictly validated. All errors use MCP-compliant error responses.
/// See core.rs for full logic and error handling.
pub mod compression;
pub mod core;
pub mod memory_storage;
pub mod storage;

pub use compression::{
	has_pending_compression, has_pending_project_compression, process_pending_compression,
	process_pending_phase_compression, process_pending_project_compression,
	set_pending_compression_range, CompressionMetrics, PhaseCompression, ProjectCompression,
};
pub use core::{
	broken_plan_conditions, clear_plan_data, clear_task_start_index, execute_plan,
	get_and_clear_start_index, get_completed_task_count, get_current_plan_display,
	get_current_task_start_index, get_last_completed_task_for_compression, has_active_plan,
	open_plan_tasks, render_plan_checklist, render_plan_details, set_current_task_start_index,
	set_last_task_message_range, sidecar_advance, sidecar_finish, sidecar_revise, sidecar_start,
};
pub use memory_storage::MemoryPlanStorage;
pub use storage::{ExecutionPlan, MessageRange, PlanStatus, PlanStorage, PlanTask, TaskStatus};

use crate::mcp::McpFunction;
use serde_json::json;

/// Get plan function definition for MCP
pub fn get_plan_function() -> McpFunction {
	McpFunction {
        name: "plan".to_string(),
        description: "Manual compatibility interface for structured multi-step work.

When supervisor self-reporting is active, routine plan lifecycle is carried out-of-band in the hidden status line. Do NOT call this tool merely to start, advance, list, or close that sidecar plan: those transitions ride with normal work responses and cost no standalone API round trip.

Use this tool only when explicit manual plan control is needed, or when the sidecar is unavailable. Plans are domain-neutral: tasks describe observable outcomes, whether the work concerns code, research, operations, writing, or another domain. Skip planning for answers and focused work completable without meaningful context-loss risk.

Commands:
- start: create plan with tasks array (ERROR if a plan is already active — plans survive context compression; continue the active plan with step/next instead of re-creating it)
- step: add progress note to current task (does NOT advance it)
- next: mark current task DONE and advance to next
- list: show all tasks with status
- done: complete the plan with final summary
- reset: clear all plan data

Each task requires a short title and a concise, observable completion condition. Do not pad tasks with implementation detail already available in context.".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The operation to perform",
                    "enum": ["start", "step", "next", "list", "done", "reset"]
                },
                "tasks": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "required": ["title", "description"],
                        "properties": {
                            "title": {
                                "type": "string",
                                "description": "Short, clear task title"
                            },
                            "description": {
                                "type": "string",
                                "description": "Concise observable outcome that marks this task complete. Include domain-specific detail only when it is needed to resume or verify the work."
                            },
                            "valid_if": {
                                "type": "string",
                                "description": "OPTIONAL falsifiable condition this task's approach depends on — the assumption whose breakage invalidates the task. Machine-checkable forms: 'file_exists: <path>' or 'file_absent: <path>' (re-checked automatically each turn; a broken condition triggers a plan-revision steer). Free-form prose is allowed but only judged by the verifier, not auto-checked."
                            }
                        }
                    },
                    "description": "Ordered task outcomes (REQUIRED for 'start'). Use only for meaningful dependent phases."
                },
                "content": {
                    "type": "string",
                    "description": "REQUIRED for 'start' (plan goal/title), 'step' (progress details), 'next' (task completion summary), and 'done' (final summary). NOT required for 'list' or 'reset'."
                }
            },
            "required": ["command"],
            "additionalProperties": false
        }),
    }
}
