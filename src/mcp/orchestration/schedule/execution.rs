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

//! Session-owned executions of scheduled tool calls. Timers never run a second copy of a busy schedule.

use super::storage::{ScheduleAction, ScheduleEntry};
use crate::config::Config;
use crate::mcp::{McpToolCall, McpToolResult};
use crate::session::chat::response::tool_execution::{execute_tools_in_context, ToolExecutionContext};
use crate::session::context::SessionId;
use crate::session::inbox::{push_inbox_message, InboxMessage, InboxSource};
use crate::session::output::OutputMode;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use tokio::sync::watch;
use tokio::task::JoinHandle;

struct Execution {
	cancel: watch::Sender<bool>,
	task: JoinHandle<()>,
	running: bool,
	tap_run: Option<String>,
}

type Executions = HashMap<SessionId, HashMap<String, Execution>>;
static EXECUTIONS: OnceLock<Mutex<Executions>> = OnceLock::new();

fn executions() -> &'static Mutex<Executions> {
	EXECUTIONS.get_or_init(Default::default)
}

pub(crate) fn init_for_session() {
	let id = crate::session::context::expect_session_id();
	executions().lock().unwrap().entry(id).or_default();
}

pub(crate) fn clear_for_session(id: &SessionId) {
	if let Some(jobs) = executions().lock().unwrap().remove(id) {
		for job in jobs.into_values() {
			// Let the normal executor cancel its tools and reap their results before it exits.
			job.cancel.send_replace(true);
			drop(job.task);
		}
	}
}

pub(crate) fn has_running() -> bool {
	let Some(id) = crate::session::context::current_session_id() else {
		return false;
	};
	executions()
		.lock()
		.unwrap()
		.get(&id)
		.is_some_and(|jobs| jobs.values().any(|job| job.running))
}

pub(crate) fn dispatch(entry: ScheduleEntry, config: &Config) {
	let ScheduleAction::ToolCall { tool_call, workdir } = entry.action else {
		unreachable!("only tool actions are dispatched to the executor");
	};
	let session_id = crate::session::context::expect_session_id();
	let id = entry.id;
	let mut registry = executions().lock().unwrap();
	let Some(jobs) = registry.get_mut(&session_id) else {
		push_inbox_message(InboxMessage {
			source: InboxSource::Schedule { id },
			content: "Scheduled tool failed: session services are not initialized.".to_string(),
		});
		return;
	};
	// Completed one-shots must not accumulate in long-lived sessions; retain only work still preventing overlap.
	jobs.retain(|_, job| {
		job.running
			|| job.tap_run.as_deref().is_some_and(|id| {
				crate::session::tap_runs::find_job(id)
					.is_some_and(|job| job.status == crate::session::tap_runs::TapJobStatus::Running)
			})
	});
	if jobs.contains_key(&id) {
		crate::log_debug!("Skipping busy scheduled tool {}", id);
		return;
	}
	let call = McpToolCall {
		tool_name: tool_call.name,
		parameters: tool_call.arguments,
		tool_id: format!("schedule-{}-{}", id, uuid::Uuid::new_v4().simple()),
	};
	let config = config.clone();
	let (cancel, rx) = watch::channel(false);
	let task_id = id.clone();
	let task_session = session_id.clone();
	let task = tokio::spawn(crate::supervisor::accounting::inherit(async move {
		crate::session::context::with_session_id(task_session.clone(), async move {
			crate::mcp::workdir::with_working_directory(workdir, async move {
				let (result, tap_run) = execute(&call, &config, rx).await;
				let status = if result.is_error() { "failed" } else { "completed" };
				// Publish before clearing Running: a one-shot must not let the session exit before delivery.
				push_inbox_message(InboxMessage {
					source: InboxSource::Schedule { id: task_id.clone() },
					content: format!(
						"[Scheduled tool '{}' {} ({})]\n\n{}",
						call.tool_name,
						status,
						task_id,
						result.extract_content()
					),
				});
				if let Some(job) = executions()
					.lock()
					.unwrap()
					.get_mut(&task_session)
					.and_then(|jobs| jobs.get_mut(&task_id))
				{
					job.tap_run = tap_run;
					job.running = false;
				}
				crate::session::context::notify_schedule_change(&task_session);
			})
			.await;
		})
		.await;
	}));
	jobs.insert(
		id,
		Execution {
			cancel,
			task,
			running: true,
			tap_run: None,
		},
	);
}

async fn execute(
	call: &McpToolCall,
	config: &Config,
	cancellation: watch::Receiver<bool>,
) -> (McpToolResult, Option<String>) {
	let error = |message| McpToolResult::error(call.tool_name.clone(), call.tool_id.clone(), message);
	if !crate::mcp::get_available_functions(config)
		.await
		.iter()
		.any(|function| function.name == call.tool_name)
		|| crate::mcp::tool_map::get_server_for_tool(&call.tool_name).is_none()
	{
		return (
			error(format!(
				"tool '{}' is no longer available in this session",
				call.tool_name
			)),
			None,
		);
	}
	let mut context = ToolExecutionContext::Scheduled {
		session_name: crate::session::context::expect_session_id(),
		tap_run: None,
	};
	// Reuse admission, authorizer, guardrails, hooks, telemetry and output limits. Results are shown by the inbox.
	let result = match execute_tools_in_context(
		vec![call.clone()],
		&mut context,
		config,
		Some(cancellation),
		OutputMode::WebSocket,
	)
	.await
	{
		Ok((mut results, _)) => results
			.pop()
			.unwrap_or_else(|| error("Scheduled tool execution cancelled".to_string())),
		Err(cause) => error(format!("Scheduled tool execution failed: {cause:#}")),
	};
	let ToolExecutionContext::Scheduled { tap_run, .. } = context else {
		unreachable!("scheduled execution retains its context");
	};
	(result, tap_run)
}

#[cfg(test)]
#[path = "execution_tests.rs"]
mod tests;
