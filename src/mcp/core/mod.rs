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

// Core MCP provider - modular structure
// Handles core tools: plan, mcp, agent, schedule, skill, capability

pub mod capability;
pub mod dynamic;
pub mod dynamic_agents;
pub mod functions;
pub mod plan;
pub mod schedule;
pub mod skill;
pub mod skill_auto;

#[cfg(test)]
mod plan_tests;

#[cfg(test)]
mod skill_tests;

// Re-export main functionality
pub use capability::execute_capability_command;
pub use dynamic::execute_mcp_command;
pub use dynamic_agents::execute_agent_tool_command;
pub use functions::get_all_functions;
pub use plan::{clear_plan_data, execute_plan};
pub use schedule::{
	execute_schedule_tool, flush_due_to_inbox, has_pending_schedules, next_schedule_sleep,
};
pub use skill::execute_skill_tool;
