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

// Report command handler

use super::super::core::ChatSession;
use super::{CommandOutput, CommandResult};
use crate::config::Config;
use anyhow::Result;

pub fn handle_report(session: &ChatSession, _config: &Config) -> Result<CommandResult> {
	// Generate and display session usage report
	if let Some(ref session_file) = session.session.session_file {
		let session_file_str = session_file.to_string_lossy();
		match crate::session::report::SessionReport::generate_from_log(&session_file_str) {
			Ok(report) => {
				// Convert report entries to JSON
				let entries: Vec<serde_json::Value> = report
					.entries
					.iter()
					.map(|entry| {
						serde_json::json!({
							"user_request": entry.user_request,
							"cost": entry.cost,
							"tool_calls": entry.tool_calls,
							"tools_used": entry.tools_used,
							"task_time": entry.task_time,
							"ai_time": entry.ai_time,
							"processing_time": entry.processing_time
						})
					})
					.collect();

				let totals = serde_json::json!({
					"total_cost": report.totals.total_cost,
					"total_tool_calls": report.totals.total_tool_calls,
					"total_task_time_ms": report.totals.total_task_time_ms,
					"total_ai_time_ms": report.totals.total_ai_time_ms,
					"total_processing_time_ms": report.totals.total_processing_time_ms
				});

				Ok(CommandResult::HandledWithOutput(Box::new(
					CommandOutput::Report { entries, totals },
				)))
			}
			Err(e) => Ok(CommandResult::HandledWithOutput(Box::new(
				CommandOutput::Error {
					error: format!("Failed to generate report: {}", e),
					context: Some(serde_json::json!({
						"hint": "Make sure the session log file exists and is readable."
					})),
				},
			))),
		}
	} else {
		Ok(CommandResult::HandledWithOutput(Box::new(
			CommandOutput::Error {
				error: "No session file available for report generation.".to_string(),
				context: Some(serde_json::json!({
					"hint": "No session file found. Sessions are auto-saved after each interaction."
				})),
			},
		)))
	}
}
