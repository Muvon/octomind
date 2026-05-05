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
	clear_plan_data, clear_task_start_index, execute_plan, get_and_clear_start_index,
	get_completed_task_count, get_current_plan_display, get_current_task_start_index,
	get_last_completed_task_for_compression, has_active_plan, set_current_task_start_index,
	set_last_task_message_range,
};
pub use memory_storage::MemoryPlanStorage;
pub use storage::{ExecutionPlan, MessageRange, PlanStatus, PlanStorage, PlanTask, TaskStatus};

use crate::mcp::McpFunction;
use serde_json::json;

/// Get plan function definition for MCP
pub fn get_plan_function() -> McpFunction {
	McpFunction {
        name: "plan".to_string(),
        description: "Structured step-by-step task tracker for COMPLEX, MULTI-STEP work spanning multiple files or components.

Use for: multi-file implementations, long-running work that may be interrupted, tasks needing sequencing and checkpoints.
Skip for: single-step changes, quick fixes, anything completable in one focused pass without losing context.

Commands:
- start: create plan with tasks array (ERROR if plan already exists, use done or reset first)
- step: add progress note to current task (does NOT advance it)
- next: mark current task DONE and advance to next
- list: show all tasks with status
- done: complete the plan with final summary
- reset: clear all plan data

Each task requires title (short) and description (detailed: file paths, commands, expected outcomes, validation steps).".to_string(),
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
                                "description": "Comprehensive explanation of exactly what needs to be done. Include: specific file paths, exact commands to run, configuration details, expected outcomes, error handling steps, validation criteria, and any dependencies. Write as if someone else needs to complete this task from scratch with zero context. Minimum 2-3 sentences with technical specifics."
                            }
                        }
                    },
                    "description": "Array of detailed task objects with titles and comprehensive descriptions (REQUIRED for 'start' command). Each task description must include specific technical details, file paths, commands, expected outcomes, and validation steps - write as if someone else needs to complete the task from scratch."
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
