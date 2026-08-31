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

//! `/status` — unified process-local activity for the current session.
//!
//! The default view is concise and active-only across agents, command monitors,
//! and MCP resource-backed jobs. A category filter renders its full view:
//! `/status agents [id]`, `/status monitors`, or `/status jobs`.
//!
//! Resource URI schemes stay opaque. The MCP server that returned the
//! `ResourceLink` remains the owner and is queried once through `resources/read`
//! only for the full jobs view.

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

pub async fn handle_status(params: &[&str]) -> Result<CommandResult> {
	let Some(session_id) = crate::session::context::current_session_id() else {
		return Ok(CommandResult::HandledWithOutput(Box::new(
			CommandOutput::Status {
				data: json!({
					"subcommand": "error",
					"message": "status requires an active session context",
				}),
			},
		)));
	};

	let filter = params.first().map(|value| value.trim()).unwrap_or("");
	let data = match filter {
		"" => build_overview(&session_id),
		"agents" => super::agents::build_agents_status(&params[1..])?,
		"monitors" => build_monitors_status(&session_id),
		"jobs" => build_jobs_status(&session_id).await,
		other => json!({
			"view": "error",
			"message": format!(
				"Unknown status filter '{other}'. Use /status, /status agents [id], /status monitors, or /status jobs."
			),
		}),
	};

	Ok(CommandResult::HandledWithOutput(Box::new(
		CommandOutput::Status { data },
	)))
}

fn build_overview(session_id: &crate::session::context::SessionId) -> serde_json::Value {
	let agents = super::agents::active_agents_status();
	let monitors = monitor_items(session_id);
	let jobs = concise_job_items(session_id);
	json!({
		"view": "overview",
		"active": agents.len() + monitors.len() + jobs.len(),
		"agents": agents,
		"monitors": monitors,
		"jobs": jobs,
	})
}

fn build_monitors_status(session_id: &crate::session::context::SessionId) -> serde_json::Value {
	let monitors = monitor_items(session_id);
	json!({
		"view": "monitors",
		"active": monitors.len(),
		"monitors": monitors,
	})
}

async fn build_jobs_status(session_id: &crate::session::context::SessionId) -> serde_json::Value {
	let pending = crate::session::shell_jobs::pending_resources_for_session(session_id);
	let jobs = join_all(pending.into_iter().map(read_mcp_job)).await;
	let items: Vec<serde_json::Value> = jobs
		.iter()
		.map(|job| {
			json!({
				"server": job.server_name,
				"uri": job.uri,
				"label": job.label,
				"state": job.state,
				"elapsed_secs": job.elapsed_secs,
				"resource_status": job.resource_status,
			})
		})
		.collect();
	json!({
		"view": "jobs",
		"active": items.len(),
		"jobs": items,
	})
}

fn concise_job_items(session_id: &crate::session::context::SessionId) -> Vec<serde_json::Value> {
	crate::session::shell_jobs::pending_resources_for_session(session_id)
		.into_iter()
		.map(|job| {
			json!({
				"server": job.server_name,
				"uri": job.uri,
				"label": job.label,
				"state": if job.delivering { "delivering completion" } else { "running" },
				"elapsed_secs": job.started_at.elapsed().unwrap_or_default().as_secs(),
			})
		})
		.collect()
}

fn monitor_items(session_id: &crate::session::context::SessionId) -> Vec<serde_json::Value> {
	crate::mcp::orchestration::monitor::status_for_session(session_id)
		.into_iter()
		.map(|monitor| {
			json!({
				"id": monitor.id,
				"description": monitor.description,
				"command": monitor.command,
				"workdir": monitor.workdir,
				"elapsed_secs": monitor.started_at.elapsed().unwrap_or_default().as_secs(),
				"flush_interval_secs": monitor.flush_interval_secs,
				"max_batch_bytes": monitor.max_batch_bytes,
				"timeout_ms": monitor.timeout_ms,
			})
		})
		.collect()
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
#[path = "status_tests.rs"]
mod tests;
