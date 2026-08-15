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

//! /monitor session command — thin wrapper over the MCP `monitor` tool.
//!
//! Lists the monitors the agent started via the `monitor` tool for this
//! session, so the user can see what is being watched without asking:
//!
//! `/monitor` → list running monitors (id, command, workdir, uptime)

use super::{CommandOutput, CommandResult};
use crate::mcp::McpToolCall;
use anyhow::Result;
use serde_json::json;

pub async fn handle_monitor() -> Result<CommandResult> {
	let call = McpToolCall {
		tool_name: "monitor".to_string(),
		tool_id: format!("cmd_monitor_{}", uuid::Uuid::new_v4().simple()),
		parameters: json!({ "action": "list" }),
	};

	match crate::mcp::orchestration::monitor::execute_monitor_tool(&call).await {
		Ok(result) => {
			let text = result.extract_content();
			let is_error = result.is_error();
			Ok(CommandResult::HandledWithOutput(Box::new(
				CommandOutput::Monitor {
					data: json!({
						"subcommand": "list",
						"is_error": is_error,
						"message": text,
					}),
				},
			)))
		}
		Err(e) => Ok(CommandResult::HandledWithOutput(Box::new(
			CommandOutput::Monitor {
				data: json!({
					"subcommand": "error",
					"message": format!("monitor tool failed: {e}"),
				}),
			},
		))),
	}
}
