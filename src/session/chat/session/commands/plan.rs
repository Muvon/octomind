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

use super::{CommandOutput, CommandResult};
use anyhow::Result;

/// Handle /plan command - display current plan stored in MCP plan tool
pub async fn handle_plan() -> Result<CommandResult> {
	// Access the plan storage directly from the MCP plan tool
	match crate::mcp::dev::plan::core::get_current_plan_display().await {
		Ok(plan_display) => {
			// Get structured plan data
			let plan_json = crate::mcp::dev::plan::core::get_current_plan_json()
				.await
				.ok();

			Ok(CommandResult::HandledWithOutput(CommandOutput::Plan {
				has_plan: true,
				plan: plan_json,
				display: Some(plan_display),
			}))
		}
		Err(e) => Ok(CommandResult::HandledWithOutput(CommandOutput::Plan {
			has_plan: false,
			plan: None,
			display: Some(e.to_string()),
		})),
	}
}
