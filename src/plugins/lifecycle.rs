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

//! Connects plugins for a session and sends the events that open and close it,
//! also for subagents.

use super::payload::{self, ContextParts};
use super::protocol::{event, Judge, SessionEnd, SessionStart, SubagentEnd, SubagentStart};
use crate::config::Config;
use crate::session::chat::session::utils::AGENTS_FILE;
use crate::session::chat::session::ChatSession;
use anyhow::Result;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

/// After a resume the pins and status fields come back from the session file
/// and the plugins get the full message list again.
const SOURCE_RESUME: &str = "resume";
/// The session ends because the user leaves, or its subagent run is over.
pub const REASON_EXIT: &str = "exit";
/// The session ends because the user starts another one.
pub const REASON_SWITCH: &str = "switch";

/// Tools that can open a file the host saved a full tool output to.
const SPILL_READER_TOOLS: [&str; 3] = ["view", "batch_edit", "shell"];

/// `_meta` key of ACP `session/new` that carries the parent's session id to a
/// child process.
pub const PARENT_SESSION_META_KEY: &str = "octomind.parent_session";
/// What a plugin handed down to the child travels in `_meta` of `session/new`
/// under this prefix followed by the plugin name.
pub const PLUGIN_META_PREFIX: &str = "octomind.plugin.";

/// Who a subagent session belongs to, and what its parent's plugins handed down.
struct Parentage {
	parent_session_id: String,
	/// Taken by the child's `session.start`.
	inherited: Vec<(String, Value)>,
}

static PARENTAGE: OnceLock<parking_lot::Mutex<HashMap<String, Parentage>>> = OnceLock::new();

fn parentage() -> &'static parking_lot::Mutex<HashMap<String, Parentage>> {
	PARENTAGE.get_or_init(Default::default)
}

/// Record that `session_id` is a subagent of `parent_session_id`. Its events
/// then name the parent, and its `session.start` carries `inherited`.
pub fn set_parentage(session_id: &str, parent_session_id: &str, inherited: Vec<(String, Value)>) {
	parentage().lock().insert(
		session_id.to_string(),
		Parentage {
			parent_session_id: parent_session_id.to_string(),
			inherited,
		},
	);
}

/// The session a subagent session belongs to.
pub fn parent_session_id(session_id: &str) -> Option<String> {
	parentage()
		.lock()
		.get(session_id)
		.map(|entry| entry.parent_session_id.clone())
}

/// What a parent process wrote into `_meta` of `session/new`: the parent's
/// session id and the value each plugin handed down. `None` when the request
/// does not come from a plugin-aware parent.
pub fn parentage_from_meta(meta: &serde_json::Map<String, Value>) -> Option<(String, Vec<(String, Value)>)> {
	let parent = meta.get(PARENT_SESSION_META_KEY)?.as_str()?.to_string();
	let inherited = meta
		.iter()
		.filter_map(|(key, value)| Some((key.strip_prefix(PLUGIN_META_PREFIX)?.to_string(), value.clone())))
		.collect();
	Some((parent, inherited))
}

/// Add the parent's session id and each plugin's value to the `_meta` of the
/// `session/new` params for a child process.
pub fn write_child_meta(params: &mut Value, parent_session_id: &str, inherited: &[(String, Value)]) {
	let meta = &mut params["_meta"];
	if !meta.is_object() {
		*meta = json!({});
	}
	meta[PARENT_SESSION_META_KEY] = json!(parent_session_id);
	for (plugin, value) in inherited {
		meta[format!("{PLUGIN_META_PREFIX}{plugin}")] = value.clone();
	}
}

/// Connect the plugins the config lists for this role. Call it before the
/// system prompt is built: the status-line instruction depends on what the
/// plugins registered at their handshake.
pub async fn connect(session_id: &str, role: &str, workdir: &Path, config: &Config) {
	super::start_session(session_id, role, workdir, &config.plugins).await;
}

/// Send `session.start` once the system prompt is final. `source` is
/// `startup`, `resume` or `compact`.
pub async fn session_started(chat_session: &mut ChatSession, config: &Config, source: &str) -> Result<()> {
	if !config.has_plugins() {
		return Ok(());
	}
	let session_id = chat_session.session.info.name.clone();
	// The tool list and the plan are read from the session's own scope.
	crate::session::context::with_session_id(session_id.clone(), async {
		if source == SOURCE_RESUME {
			chat_session.restore_plugin_state();
			super::mark_history_reset(&session_id);
		}
		let inherited = take_inherited(&session_id);
		let tools = if super::subscribed(&session_id, event::SESSION_START) {
			Some(payload::tool_infos(config).await)
		} else {
			None
		};
		let mut parts = ContextParts::of(chat_session);
		parts.parent_session_id = parent_session_id(&session_id);
		let start = SessionStart {
			source: source.to_string(),
			system_prompt: system_prompt_of(chat_session),
			interactive: config.output_mode().is_interactive(),
			can_read_saved_output: spill_reader_available(),
			inherited: None,
			capabilities: capability_names(&chat_session.role, config),
			context_files: context_files(&parts.workdir),
		};
		let effects =
			super::session_start(&parts.context(&chat_session.session.messages, tools), start, &inherited).await;
		chat_session.session.info.plugin_status_fields = super::status_fields(&session_id);
		super::apply::apply(chat_session, effects).await
	})
	.await
}

/// Send `session.end`, book what the plugins answered and drop the session's
/// plugin state. `reason` is `exit`, `switch` or `error`. The session file is
/// saved after the booking, so the callers that leave octomind next keep it.
pub async fn session_ended(chat_session: &mut ChatSession, reason: &str) {
	let session_id = chat_session.session.info.name.clone();
	if super::subscribed(&session_id, event::SESSION_END) {
		crate::session::context::with_session_id(session_id.clone(), async {
			let mut parts = ContextParts::of(chat_session);
			parts.parent_session_id = parent_session_id(&session_id);
			let payload = SessionEnd {
				reason: reason.to_string(),
			};
			super::session_end(&parts.context(&chat_session.session.messages, None), payload).await;
		})
		.await;
		// The answer's notices are shown and its usage is booked before the state goes.
		super::apply::settle(&session_id, Some(&mut chat_session.session));
		if let Err(error) = chat_session.save() {
			crate::log_debug!("Session not saved after session.end: {:#}", error);
		}
	}
	parentage().lock().remove(&session_id);
	super::end_session(&session_id);
}

/// A subagent is about to start: ask the parent's plugins what to hand down.
/// `child_session_id` is `None` when the child's id is not known yet (a child
/// process). The parent has no chat session at hand here, so the common fields
/// are built from the session registry. Returns the value per plugin name.
pub async fn subagent_starting(
	parent_session_id: &str,
	child_session_id: Option<&str>,
	task: &str,
	system_prompt: Option<&str>,
) -> Vec<(String, Value)> {
	let parts = detached_parts(parent_session_id, None, &parent_role(parent_session_id));
	let payload = SubagentStart {
		child_session_id: child_session_id.map(str::to_string),
		prompt: task.to_string(),
		system_prompt: system_prompt.map(str::to_string),
	};
	let inherited = super::subagent_start(&parts.detached_context(None), payload).await;
	super::apply::settle(parent_session_id, None);
	inherited
}

/// A subagent finished.
pub async fn subagent_ended(parent_session_id: &str, child_session_id: Option<&str>) {
	let parts = detached_parts(parent_session_id, None, &parent_role(parent_session_id));
	let payload = SubagentEnd {
		child_session_id: child_session_id.map(str::to_string),
	};
	super::subagent_end(&parts.detached_context(None), payload).await;
}

/// Ask the plugins of `session_id` for a judgement. The places that need one
/// (skill and capability choice, agent routing, the fold of the history)
/// have no message list at hand, and a judgement needs none. `None` when no
/// plugin gave one: the caller keeps the outcome of its own rule.
pub async fn judge_detached(session_id: &str, payload: Judge) -> Option<super::Judged> {
	if !super::subscribed(session_id, event::JUDGE) {
		return None;
	}
	let parent = parent_session_id(session_id);
	let parts = detached_parts(session_id, parent.as_deref(), &parent_role(session_id));
	super::judge(&parts.detached_context(None), payload).await
}

fn take_inherited(session_id: &str) -> Vec<(String, Value)> {
	parentage()
		.lock()
		.get_mut(session_id)
		.map(|entry| std::mem::take(&mut entry.inherited))
		.unwrap_or_default()
}

fn parent_role(parent_session_id: &str) -> String {
	crate::session::context::get_session_role(&parent_session_id.to_string()).unwrap_or_default()
}

/// The common event fields for a session that has no chat session at hand.
/// Status and plan stay empty: they are read from a chat session.
fn detached_parts(session_id: &str, parent_session_id: Option<&str>, role: &str) -> ContextParts {
	let workdir = crate::mcp::get_thread_working_directory();
	ContextParts {
		project: super::project::project_name(&workdir),
		active_skills: crate::session::context::get_active_skills(&session_id.to_string()),
		session_id: session_id.to_string(),
		parent_session_id: parent_session_id.map(str::to_string),
		role: role.to_string(),
		// The model of a session without a chat session is not readable here.
		model: None,
		status: None,
		plan: None,
		workdir,
	}
}

fn system_prompt_of(chat_session: &ChatSession) -> String {
	chat_session
		.session
		.messages
		.iter()
		.find(|message| message.role == "system")
		.map(|message| message.content.clone())
		.unwrap_or_default()
}

/// The same check the condense step makes for its saved originals: some tool
/// that reads a file is available.
fn spill_reader_available() -> bool {
	SPILL_READER_TOOLS
		.iter()
		.any(|tool| crate::mcp::tool_map::get_server_for_tool(tool).is_some())
}

/// Capabilities the role's domain may use, as the `capability` tool lists them.
fn capability_names(role: &str, config: &Config) -> Vec<String> {
	match crate::agent::registry::list_all_capabilities(&config.capabilities) {
		Ok(capabilities) => capabilities
			.into_iter()
			.filter(|capability| crate::agent::registry::cap_available_in_domain(&capability.domains, role))
			.map(|capability| capability.name)
			.collect(),
		Err(error) => {
			crate::log_debug!("Capabilities not listed for session.start: {:#}", error);
			Vec::new()
		}
	}
}

/// Instruction files octomind puts into the model's context. An empty file is
/// skipped there, so it is skipped here.
fn context_files(workdir: &Path) -> Vec<String> {
	let path = workdir.join(AGENTS_FILE);
	match std::fs::read_to_string(&path) {
		Ok(text) if !text.trim().is_empty() => vec![path.to_string_lossy().into_owned()],
		_ => Vec::new(),
	}
}

#[cfg(test)]
#[path = "lifecycle_tests.rs"]
mod tests;
