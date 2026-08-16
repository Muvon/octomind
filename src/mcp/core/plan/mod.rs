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

pub use compression::CompressionMetrics;
pub use core::{
	broken_plan_conditions, clear_task_start_index, get_current_plan_display,
	get_current_task_start_index, has_active_plan, render_plan_checklist, render_plan_details,
	set_current_task_start_index, sidecar_advance, sidecar_finish, sidecar_revise, sidecar_start,
};
pub use memory_storage::MemoryPlanStorage;
pub use storage::{ExecutionPlan, MessageRange, PlanStatus, PlanStorage, PlanTask, TaskStatus};
