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

//! Runtime MCP provider — session runtime and tool-surface control.
//!
//! These tools reconfigure the running session's tool surface:
//! - `mcp`        — register/enable/disable MCP servers at runtime.
//! - `agent`      — register/enable/disable in-process dynamic agents.
//! - `skill`      — load and activate skills from taps.
//! - `capability` — discover/enable domain tool bundles at runtime.
//!
//! They live under the `runtime` builtin server. The `core` server hosts
//! `plan`; the `orchestration` server hosts `tap` and `schedule`.

use crate::config::Config;
use crate::mcp::{McpFunction, McpToolCall, McpToolResult};
use anyhow::Result;

pub mod capability;
pub mod dynamic;
pub mod dynamic_agents;
pub mod plugin;
pub mod skill;
pub mod skill_auto;

#[cfg(test)]
mod skill_tests;

pub use capability::execute_capability_command;
pub use dynamic::execute_mcp_command;
pub use dynamic_agents::execute_agent_tool_command;
pub use skill::execute_skill_tool;

pub fn get_all_functions() -> Vec<McpFunction> {
	vec![
		crate::mcp::runtime::dynamic::get_mcp_tool_function(),
		crate::mcp::runtime::dynamic_agents::get_agent_tool_function(),
		crate::mcp::runtime::skill::get_skill_function(),
		crate::mcp::runtime::capability::get_capability_function(),
	]
}

pub async fn execute_runtime_tool(call: &McpToolCall, config: &Config) -> Result<McpToolResult> {
	match call.tool_name.as_str() {
		"mcp" => crate::mcp::runtime::execute_mcp_command(call, config).await,
		"agent" => crate::mcp::runtime::execute_agent_tool_command(call).await,
		// `execute_skill_tool` returns `Result<_, String>` for historical
		// reasons — convert to anyhow at the boundary so all runtime tools
		// share a uniform error type.
		"skill" => crate::mcp::runtime::execute_skill_tool(call)
			.await
			.map_err(|e| anyhow::anyhow!("{}", e)),
		"capability" => crate::mcp::runtime::execute_capability_command(call, config).await,
		other => Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("Tool '{other}' not implemented in runtime server"),
		)),
	}
}
