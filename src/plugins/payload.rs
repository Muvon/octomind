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

//! Builders that turn octomind's own types into the wire types of the plugin
//! protocol.

use super::host::EventContext;
use super::protocol::{PlanState, PlanTask, Status, ToolCall, ToolInfo, ToolOutcome};
use crate::config::Config;
use crate::mcp::core::plan::{broken_plan_conditions, get_current_task_start_index, has_active_plan, PlanStorage};
use crate::mcp::tool_shape;
use crate::mcp::{McpFunction, McpToolCall, McpToolResult};
use crate::session::chat::session::ChatSession;
use crate::session::self_report;
use crate::session::Message;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Length of a result hash. Sixteen hex characters (64 bits) tell two result
/// texts apart within one session; the hash is never used as a secret.
const RESULT_HASH_HEX_CHARS: usize = 16;

/// Plan tasks store their outcome as `Done when: <outcome>`; the wire form
/// carries the bare outcome.
const DONE_WHEN_PREFIX: &str = "Done when: ";

/// The fields common to every event, read from the session. It owns its
/// values, so a call site can build it, change the session, then borrow.
pub struct ContextParts {
	pub session_id: String,
	pub parent_session_id: Option<String>,
	pub role: String,
	pub workdir: PathBuf,
	pub project: String,
	pub status: Option<Status>,
	pub plan: Option<PlanState>,
	pub active_skills: Vec<String>,
}

impl ContextParts {
	pub fn of(chat_session: &ChatSession) -> Self {
		let session_id = chat_session.session.info.name.clone();
		let workdir = crate::mcp::get_thread_working_directory();
		let status = chat_session
			.session
			.messages
			.iter()
			.rev()
			.find(|message| message.role == "assistant")
			.and_then(|message| status(&message.content));
		Self {
			project: super::project::project_name(&workdir),
			active_skills: crate::session::context::get_active_skills(&session_id),
			parent_session_id: super::lifecycle::parent_session_id(&session_id),
			session_id,
			role: chat_session.role.clone(),
			workdir,
			status,
			plan: plan_state(),
		}
	}

	pub fn context<'a>(&'a self, messages: &'a [Message], tools: Option<Vec<ToolInfo>>) -> EventContext<'a> {
		EventContext {
			messages,
			detached: false,
			..self.detached_context(tools)
		}
	}

	/// The context of an event sent from a place that has no message list. The
	/// event carries no messages and the plugins keep their place in the history.
	pub fn detached_context(&self, tools: Option<Vec<ToolInfo>>) -> EventContext<'_> {
		EventContext {
			session_id: &self.session_id,
			parent_session_id: self.parent_session_id.as_deref(),
			role: &self.role,
			workdir: &self.workdir,
			project: &self.project,
			messages: &[],
			detached: true,
			status: self.status.clone(),
			plan: self.plan.clone(),
			tools,
			active_skills: self.active_skills.clone(),
		}
	}
}

/// The status line of an assistant message in wire form; `None` when the
/// message carries none.
pub fn status(text: &str) -> Option<Status> {
	self_report::parse_self_report_handoff(text).map(wire_status)
}

fn wire_status(report: self_report::ParsedSelfReport) -> Status {
	let non_empty = |text: String| (!text.is_empty()).then_some(text);
	Status {
		state: report.state.as_str().to_string(),
		focus: non_empty(report.handoff.focus),
		next: non_empty(report.handoff.next),
		carry: report.handoff.carry,
		fields: report.fields,
	}
}

/// The current session's plan in wire form; `None` without an open plan.
pub fn plan_state() -> Option<PlanState> {
	if !has_active_plan() {
		return None;
	}
	// Plans outside a session live in a store only the plan module can open.
	let session_id = crate::session::context::current_session_id()?;
	let broken = broken_plan_conditions();
	let storage = crate::session::context::get_plan_storage(&session_id);
	let storage = storage.lock().unwrap();
	let plan = storage.get_plan().ok()?;
	let tasks = plan
		.tasks
		.iter()
		.enumerate()
		.map(|(index, task)| PlanTask {
			title: task.title.clone(),
			done_when: task
				.description
				.strip_prefix(DONE_WHEN_PREFIX)
				.unwrap_or(&task.description)
				.to_string(),
			done: index < plan.current_task_index,
			valid_if: task.valid_if.clone(),
			// `broken_plan_conditions` counts tasks from one.
			valid: task
				.valid_if
				.as_ref()
				.map(|_| !broken.iter().any(|(number, _, _)| *number == index + 1)),
		})
		.collect();
	Some(PlanState {
		title: plan.title.clone(),
		tasks,
		current: plan.current_task_index,
		touched_at: storage.touched_at(),
		task_start_index: get_current_task_start_index(),
	})
}

/// The session's tool list in wire form.
pub async fn tool_infos(config: &Config) -> Vec<ToolInfo> {
	crate::mcp::get_available_functions(config)
		.await
		.iter()
		.map(tool_info)
		.collect()
}

fn tool_info(function: &McpFunction) -> ToolInfo {
	ToolInfo {
		name: function.name.clone(),
		server: crate::mcp::tool_map::get_tool_server_name(&function.name).unwrap_or_default(),
		description: function.description.clone(),
		parameters: function.parameters.clone(),
		read_only: tool_shape::tool_read_only_hint(&function.name),
	}
}

/// A call before it ran. `denied` is the reason a built-in rule already
/// denied it; `rules` are the ids of the plugin's own rules that would.
pub fn tool_call(
	call: &McpToolCall,
	capability: Option<String>,
	denied: Option<String>,
	rules: Vec<String>,
) -> ToolCall {
	ToolCall {
		id: call.tool_id.clone(),
		tool: call.tool_name.clone(),
		server: crate::mcp::tool_map::get_tool_server_name(&call.tool_name).unwrap_or_default(),
		capability,
		arguments: call.parameters.clone(),
		denied,
		rules,
	}
}

/// A call after it ran. `result` is the text after octomind's own size cap;
/// the host drops small texts per plugin when it sends the event.
pub fn tool_outcome(
	call: &McpToolCall,
	result: &McpToolResult,
	workdir: &Path,
	denied: bool,
	replaceable: bool,
) -> ToolOutcome {
	let text = result.extract_content();
	ToolOutcome {
		id: call.tool_id.clone(),
		tool: call.tool_name.clone(),
		server: crate::mcp::tool_map::get_tool_server_name(&call.tool_name).unwrap_or_default(),
		arguments: call.parameters.clone(),
		error: result.is_error(),
		denied,
		result_hash: result_hash(&text),
		size_tokens: crate::session::estimate_tokens(&text),
		size_bytes: text.len(),
		result: Some(text),
		mutation: tool_shape::is_mutation_call(&call.tool_name, &call.parameters),
		command: tool_shape::executed_command(&call.tool_name, &call.parameters).map(str::to_string),
		free_form: tool_shape::executes_free_form_command(&call.tool_name),
		paths: tool_shape::param_paths(&call.parameters)
			.iter()
			.map(|path| workdir.join(path).to_string_lossy().into_owned())
			.collect(),
		replaceable,
		returned: None,
	}
}

/// Lowercase hex of the start of the text's SHA-256. Plugins compare hashes
/// across runs and builds, so it must not depend on the toolchain.
pub fn result_hash(text: &str) -> String {
	let mut hex = hex::encode(Sha256::digest(text.as_bytes()));
	hex.truncate(RESULT_HASH_HEX_CHARS);
	hex
}

#[cfg(test)]
#[path = "payload_tests.rs"]
mod tests;
