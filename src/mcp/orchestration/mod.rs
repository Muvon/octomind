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

//! Orchestration MCP provider — cross-domain delegation and session-flow control.
//!
//! Orchestrator-tier session primitives:
//! - `tap`      — discover and run registry specialists (cross-domain delegation).
//! - `schedule` — inject deferred/recurring user messages (session-flow control).
//!
//! They live under the `orchestration` builtin server, granted only via the
//! `orchestration` capability. Narrow domain specialists never see them.

use crate::mcp::McpFunction;

pub mod schedule;
pub mod tap;

pub use schedule::{
	execute_schedule_tool, flush_due_to_inbox, flush_idle_to_inbox, has_pending_idle_schedules,
	has_pending_schedules, is_session_idle, next_schedule_sleep,
};
pub use tap::execute_tap_command;

pub fn get_all_functions() -> Vec<McpFunction> {
	vec![tap::get_tap_function(), schedule::get_schedule_function()]
}
