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
use crate::session::timing;
use anyhow::Result;
use chrono::Timelike;

pub fn handle_report(
	session: &ChatSession,
	config: &Config,
	params: &[&str],
) -> Result<CommandResult> {
	match params {
		[] => session_report(session, config),
		["day"] => day_report(config),
		_ => Ok(CommandResult::HandledWithOutput(Box::new(
			CommandOutput::Error {
				error: format!("Unknown /report argument: {}", params.join(" ")),
				context: Some(serde_json::json!({"hint": "Usage: /report [day]"})),
			},
		))),
	}
}

/// Human time and energy of every session with a turn since local midnight.
fn day_report(config: &Config) -> Result<CommandResult> {
	let now = chrono::Local::now();
	// ponytail: on a DST-change day midnight is off by the shift; a turn
	// in that hour lands on the neighbouring day.
	let day_start = (now.timestamp() - i64::from(now.num_seconds_from_midnight())) as u64;
	let (names, turns): (Vec<String>, Vec<Vec<timing::Turn>>) =
		crate::session::report::SessionReport::turns_since(
			&crate::directories::get_sessions_dir()?,
			day_start,
		)?
		.into_iter()
		.unzip();
	let timing = timing::compute(&turns, &config.timing);
	Ok(CommandResult::HandledWithOutput(Box::new(
		CommandOutput::ReportDay {
			sessions: names,
			timing: serde_json::to_value(&timing)?,
		},
	)))
}

fn session_report(session: &ChatSession, config: &Config) -> Result<CommandResult> {
	// Generate and display session usage report
	if let Some(ref session_file) = session.session.session_file {
		// Close the in-flight request's window so the last row shows its real
		// cost instead of 0 — the report reads the log, not live state.
		if let Err(error) =
			crate::session::logger::log_stats_checkpoint(session_file, &session.session.info)
		{
			crate::log_debug!("Stats checkpoint before report failed: {}", error);
		}
		let session_file_str = session_file.to_string_lossy();
		match crate::session::report::SessionReport::generate_from_log(&session_file_str) {
			Ok(report) => {
				let timing = timing::compute(std::slice::from_ref(&report.turns), &config.timing);
				let turns = &timing.sessions[0].turns;
				// Convert report entries to JSON
				let entries: Vec<serde_json::Value> = report
					.entries
					.iter()
					.map(|entry| {
						let turn = entry.turn.map(|i| &turns[i]);
						serde_json::json!({
							"user_request": entry.user_request,
							"cost": entry.cost,
							"tool_calls": entry.tool_calls,
							"tools_used": entry.tools_used,
							"task_time": entry.task_time,
							"ai_time": entry.ai_time,
							"processing_time": entry.processing_time,
							"human_time_min": turn.map(|t| t.active_min + t.wait_min),
							"energy_dhe": turn.map(|t| t.energy_dhe),
							"rubber_stamp": turn.is_some_and(|t| t.rubber_stamp)
						})
					})
					.collect();

				let totals = serde_json::json!({
					"total_cost": report.totals.total_cost,
					"total_tool_calls": report.totals.total_tool_calls,
					"total_task_time_ms": report.totals.total_task_time_ms,
					"total_ai_time_ms": report.totals.total_ai_time_ms,
					"total_processing_time_ms": report.totals.total_processing_time_ms,
					"timing": timing
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
