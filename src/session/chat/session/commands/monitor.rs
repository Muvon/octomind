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

//! /monitor session command — unified background activity view.
//!
//! Lists both monitors started through the orchestration `monitor` tool and
//! pending MCP resource-backed jobs (for example OctoFS shell commands that
//! automatically moved to `octofs://jobs/...`). MCP resources are read once,
//! with a short timeout, so the view includes their current server-reported
//! status and bounded output tail without polling.
//!
//! `/monitor` → list all current background activity

use super::{CommandOutput, CommandResult};
use anyhow::Result;
use futures::future::join_all;
use serde_json::json;
use std::time::Duration;

const RESOURCE_READ_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_RESOURCE_STATUS_CHARS: usize = 4000;

#[derive(Debug)]
struct RenderedMcpJob {
	server_name: String,
	uri: String,
	label: String,
	state: &'static str,
	elapsed_secs: u64,
	resource_status: String,
}

pub async fn handle_monitor() -> Result<CommandResult> {
	let Some(session_id) = crate::session::context::current_session_id() else {
		return Ok(CommandResult::HandledWithOutput(Box::new(
			CommandOutput::Monitor {
				data: json!({
					"subcommand": "error",
					"message": "monitor requires an active session context",
				}),
			},
		)));
	};

	let pending = crate::session::shell_jobs::pending_resources_for_session(&session_id);
	let jobs = join_all(pending.into_iter().map(read_mcp_job)).await;
	let job_count = jobs.len();
	let monitor_count = crate::mcp::orchestration::monitor::running_monitor_count(&session_id);

	let mut sections = Vec::new();
	if !jobs.is_empty() {
		sections.push(render_mcp_jobs(&jobs));
	}
	if let Some(monitors) = crate::mcp::orchestration::monitor::render_running_monitors(&session_id)
	{
		sections.push(monitors);
	}
	let message = if sections.is_empty() {
		"No background activity.".to_string()
	} else {
		sections.join("\n\n")
	};
	let jobs_json: Vec<serde_json::Value> = jobs
		.iter()
		.map(|job| {
			json!({
				"server": job.server_name,
				"uri": job.uri,
				"label": job.label,
				"state": job.state,
				"elapsed_seconds": job.elapsed_secs,
				"resource_status": job.resource_status,
			})
		})
		.collect();

	Ok(CommandResult::HandledWithOutput(Box::new(
		CommandOutput::Monitor {
			data: json!({
				"subcommand": "list",
				"is_error": false,
				"message": message,
				"job_count": job_count,
				"monitor_count": monitor_count,
				"jobs": jobs_json,
			}),
		},
	)))
}

async fn read_mcp_job(job: crate::session::shell_jobs::PendingResource) -> RenderedMcpJob {
	let elapsed_secs = job.started_at.elapsed().unwrap_or_default().as_secs();
	let resource_status = match tokio::time::timeout(
		RESOURCE_READ_TIMEOUT,
		crate::mcp::client::read_resource_text(&job.server_name, &job.uri),
	)
	.await
	{
		Ok(Ok(text)) if text.trim().is_empty() => "resource returned no text status".to_string(),
		Ok(Ok(text)) => bound_resource_status(&text),
		Ok(Err(error)) => format!("status unavailable: {error}"),
		Err(_) => "status read timed out".to_string(),
	};

	RenderedMcpJob {
		server_name: job.server_name,
		uri: job.uri,
		label: job.label,
		state: if job.delivering {
			"delivering completion"
		} else {
			"awaiting completion"
		},
		elapsed_secs,
		resource_status,
	}
}

fn render_mcp_jobs(jobs: &[RenderedMcpJob]) -> String {
	let entries = jobs
		.iter()
		.map(|job| {
			let status = job
				.resource_status
				.lines()
				.map(|line| format!("    {line}"))
				.collect::<Vec<_>>()
				.join("\n");
			format!(
				"[{}] {} — tracked {}s\n  MCP server: {}\n  State: {}\n  Current resource:\n{}",
				job.uri, job.label, job.elapsed_secs, job.server_name, job.state, status
			)
		})
		.collect::<Vec<_>>()
		.join("\n");
	format!("MCP background jobs:\n{entries}")
}

fn bound_resource_status(text: &str) -> String {
	let count = text.chars().count();
	if count <= MAX_RESOURCE_STATUS_CHARS {
		return text.to_string();
	}
	let head_chars = MAX_RESOURCE_STATUS_CHARS / 3;
	let tail_chars = MAX_RESOURCE_STATUS_CHARS - head_chars;
	let head: String = text.chars().take(head_chars).collect();
	let tail: String = text
		.chars()
		.rev()
		.take(tail_chars)
		.collect::<Vec<_>>()
		.into_iter()
		.rev()
		.collect();
	format!(
		"{head}\n[{} status characters omitted]\n{tail}",
		count - MAX_RESOURCE_STATUS_CHARS
	)
}

#[cfg(test)]
#[path = "monitor_tests.rs"]
mod tests;
