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

//! Runtime-owned plan storage, rendering, compression, and sidecar transitions.
//!
//! Plan mutation is deliberately not an MCP tool surface. The external planner
//! in [`crate::supervisor::plan`] owns lifecycle decisions; specialists can only
//! observe injected state and emit sparse hidden signals.
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
