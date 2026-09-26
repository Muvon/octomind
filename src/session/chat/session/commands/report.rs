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
use crate::session::report::{project_of, SessionReport};
use crate::session::timing::{self, SessionTurns, Timing};
use anyhow::Result;
use chrono::{Datelike, Local, NaiveDate, NaiveTime};

const USAGE: &str = "Usage: /report [day|week|month] [here]";

pub fn handle_report(
	session: &ChatSession,
	config: &Config,
	params: &[&str],
) -> Result<CommandResult> {
	let (period, here) = match params {
		[] => return session_report(session, config),
		[period] => (*period, false),
		[period, "here"] => (*period, true),
		_ => return Ok(usage_error(params)),
	};
	let today = Local::now().date_naive();
	let first_day = match period {
		"day" => today,
		"week" => today - chrono::Days::new(u64::from(today.weekday().num_days_from_monday())),
		"month" => today.with_day(1).expect("every month has a first day"),
		_ => return Ok(usage_error(params)),
	};
	period_report(config, period, first_day, today, here)
}

fn usage_error(params: &[&str]) -> CommandResult {
	CommandResult::HandledWithOutput(Box::new(CommandOutput::Error {
		error: format!("Unknown /report argument: {}", params.join(" ")),
		context: Some(serde_json::json!({ "hint": USAGE })),
	}))
}

#[derive(serde::Serialize)]
struct ProjectTime {
	project: String,
	time_min: f64,
	energy_dhe: f64,
}

/// Human time and energy per day and project from `first_day` through today,
/// over interactive sessions — only the current project's with `here`. Each
/// day is computed on its own: the energy budget and the flags are daily.
fn period_report(
	config: &Config,
	period: &str,
	first_day: NaiveDate,
	today: NaiveDate,
	here: bool,
) -> Result<CommandResult> {
	let mut sessions = SessionReport::turns_since(
		&crate::directories::get_sessions_dir()?,
		local_midnight(first_day),
	)?;
	// The same basename `generate_session_name` puts into new session names.
	let project = here.then(|| {
		crate::mcp::get_thread_working_directory()
			.file_name()
			.unwrap_or_default()
			.to_string_lossy()
			.to_string()
	});
	if let Some(project) = &project {
		sessions.retain(|session| project_of(&session.name) == project.as_str());
	}

	let mut days = Vec::new();
	let mut projects: Vec<ProjectTime> = Vec::new();
	for date in first_day.iter_days().take_while(|date| *date <= today) {
		let next = date.succ_opt().expect("today has a next day");
		let (start, end) = (local_midnight(date), local_midnight(next));
		// ponytail: a session running past midnight restarts at the boundary, so
		// the day's first turn misses the previous day's output in its estimate.
		let day: Vec<SessionTurns> = sessions
			.iter()
			.filter_map(|session| {
				let turns: Vec<_> = session
					.turns
					.iter()
					.filter(|turn| (start..end).contains(&turn.input_at))
					.cloned()
					.collect();
				(!turns.is_empty()).then(|| SessionTurns {
					name: session.name.clone(),
					opened_at: session.opened_at,
					turns,
				})
			})
			.collect();
		if day.is_empty() {
			continue;
		}
		let timing = timing::compute(&day, &config.timing);
		let day_projects = by_project(&day, &timing);
		for entry in &day_projects {
			add_project(
				&mut projects,
				&entry.project,
				entry.time_min,
				entry.energy_dhe,
			);
		}
		days.push(serde_json::json!({
			"date": date.to_string(),
			"sessions": day.iter().map(|session| &session.name).collect::<Vec<_>>(),
			"projects": day_projects,
			"timing": timing,
		}));
	}
	projects.sort_by(|a, b| b.time_min.total_cmp(&a.time_min));
	Ok(CommandResult::HandledWithOutput(Box::new(
		CommandOutput::ReportPeriod {
			period: period.to_string(),
			project,
			days,
			projects: serde_json::to_value(&projects)?,
		},
	)))
}

/// Time and energy per project, in first-seen order.
fn by_project(sessions: &[SessionTurns], timing: &Timing) -> Vec<ProjectTime> {
	let mut projects = Vec::new();
	for (session, session_timing) in sessions.iter().zip(&timing.sessions) {
		add_project(
			&mut projects,
			project_of(&session.name),
			session_timing.time_min,
			session_timing.energy_dhe,
		);
	}
	projects
}

fn add_project(projects: &mut Vec<ProjectTime>, project: &str, time_min: f64, energy_dhe: f64) {
	match projects.iter_mut().find(|entry| entry.project == project) {
		Some(entry) => {
			entry.time_min += time_min;
			entry.energy_dhe += energy_dhe;
		}
		None => projects.push(ProjectTime {
			project: project.to_string(),
			time_min,
			energy_dhe,
		}),
	}
}

/// Unix seconds of the local midnight opening `date`. Where a DST jump skips
/// midnight, the day opens at the first instant that exists.
fn local_midnight(date: NaiveDate) -> u64 {
	let midnight = date.and_time(NaiveTime::MIN);
	let opening = midnight
		.and_local_timezone(Local)
		.earliest()
		.or_else(|| {
			(midnight + chrono::Duration::hours(1))
				.and_local_timezone(Local)
				.earliest()
		})
		.expect("DST gaps last at most an hour");
	opening.timestamp() as u64
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
				let sessions = [SessionTurns {
					name: session.session.info.name.clone(),
					opened_at: report.opened_at,
					turns: report.turns,
				}];
				let timing = timing::compute(&sessions, &config.timing);
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
