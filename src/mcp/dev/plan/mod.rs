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
	compress_completed_task, get_compression_id, has_pending_compression,
	process_pending_compression, request_compression, set_pending_compression_range,
	CompressionMetrics,
};
pub use core::{
	clear_plan_data, clear_plan_tool_executing, execute_plan, get_and_clear_start_index,
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
        description: "Execute structured plans with detailed task breakdown and step-by-step progression for COMPLEX, MULTI-STEP tasks that require careful coordination and context tracking.

⚠️  **WHEN TO USE PLANS:**
- Complex implementations requiring multiple coordinated steps
- Tasks that span multiple files, systems, or components
- Long-running work that may be interrupted and resumed
- Multi-phase projects (research → design → implement → test)
- Tasks requiring specific sequencing and dependencies
- Work that benefits from progress tracking and checkpoints

🚫 **DO NOT USE PLANS FOR:**
- Simple, single-step tasks (just do them directly!)
- Quick fixes or small changes
- Straightforward implementations that can be done in one go
- Tasks that are obvious and don't need decomposition
- Anything that takes less than 10-15 minutes
- Simple questions or information requests

**RULE OF THUMB:** If you can complete the task in a single focused session without losing context, DON'T use a plan. Plans are for complex work that genuinely benefits from structured breakdown and progress tracking.

**MANDATORY: All tasks must include detailed descriptions!**

Commands:
- start: Create new plan with detailed tasks (ERROR if plan exists - use 'done' or 'reset' first)
- step: Add progress details to current task (does NOT complete it)
- next: Mark current task as COMPLETED and advance to next task
- list: Show all tasks with full descriptions and progress status
- done: Complete entire plan with final summary
- reset: Clear all plan data

**Task Format (REQUIRED):**
Each task must be an object with:
- title: Short, clear task title
- description: DETAILED explanation of exactly what needs to be done

Example: tasks=[{\"title\": \"Setup database\", \"description\": \"Install PostgreSQL 14+, create 'myapp' database, set up users table with id, email, password_hash fields, configure connection pooling with max 20 connections, create indexes on email field, and test connectivity with sample queries\"}]

<description_requirements>
- Write descriptions as if someone else needs to complete the task from scratch
- Include specific file paths, exact commands, configuration details
- Specify expected outcomes and validation criteria
- Add error handling steps and troubleshooting notes
- Mention dependencies and prerequisites
- Use technical specifics, not vague statements
- Minimum 2-3 detailed sentences per task
</description_requirements>

<examples>
❌ BAD: \"Fix authentication bug\"
✅ GOOD: \"Debug authentication failure in src/auth/login.rs by adding logging to validate_token() function, check JWT expiration logic around line 45, test with expired tokens, and ensure proper error messages are returned to client with 401 status code\"

❌ BAD: \"Update config\"
✅ GOOD: \"Modify config-templates/default.toml to add new [database] section with connection_pool_size=20, timeout_seconds=30, and retry_attempts=3, then update src/config/mod.rs DatabaseConfig struct to include these fields with proper validation\"
</examples>

**Best Practices:**
- Use detailed descriptions that explain EXACTLY what needs to be done
- Include specific steps, requirements, and expected outcomes
- Make descriptions comprehensive enough for context recovery after breaks
- Think sequentially - each task should build on previous ones
- Include technical details, file paths, commands, and configurations
- Reserve for genuinely complex work that benefits from structured approach".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The operation to perform",
                    "enum": ["start", "step", "next", "list", "done", "reset"]
                },
                "title": {
                    "type": "string",
                    "description": "Plan title (required for 'start' command)"
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
                    "description": "Progress/completion details. REQUIRED for 'step' (adds progress), 'next' (marks task complete), and 'done' (final summary). NOT required for 'start', 'list', or 'reset'."
                }
            },
            "required": ["command", "content"],
            "additionalProperties": false
        }),
    }
}
