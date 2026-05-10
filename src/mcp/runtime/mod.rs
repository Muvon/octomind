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
//! These tools mutate or inspect the running session:
//! - `mcp`        — register/enable/disable MCP servers at runtime.
//! - `agent`      — register/enable/disable in-process dynamic agents.
//! - `skill`      — load and activate skills from taps.
//! - `schedule`   — inject future user messages into the current session.
//! - `capability` — discover/enable domain tool bundles at runtime.
//!
//! They live under the `runtime` builtin server. Core hosts only `plan` and
//! `tap`.

use crate::config::Config;
use crate::mcp::{McpFunction, McpToolCall, McpToolResult};
use anyhow::Result;

pub fn get_all_functions() -> Vec<McpFunction> {
	vec![
		crate::mcp::core::dynamic::get_mcp_tool_function(),
		crate::mcp::core::dynamic_agents::get_agent_tool_function(),
		crate::mcp::core::skill::get_skill_function(),
		crate::mcp::core::schedule::get_schedule_function(),
		crate::mcp::core::capability::get_capability_function(),
	]
}

pub async fn execute_runtime_tool(call: &McpToolCall, config: &Config) -> Result<McpToolResult> {
	match call.tool_name.as_str() {
		"mcp" => crate::mcp::core::execute_mcp_command(call, config).await,
		"agent" => crate::mcp::core::execute_agent_tool_command(call).await,
		// `execute_skill_tool` returns `Result<_, String>` for historical
		// reasons — convert to anyhow at the boundary so all runtime tools
		// share a uniform error type.
		"skill" => crate::mcp::core::execute_skill_tool(call)
			.await
			.map_err(|e| anyhow::anyhow!("{}", e)),
		"schedule" => crate::mcp::core::execute_schedule_tool(call).await,
		"capability" => crate::mcp::core::execute_capability_command(call, config).await,
		other => Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("Tool '{other}' not implemented in runtime server"),
		)),
	}
}
