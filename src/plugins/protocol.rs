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

//! Wire types of the plugin protocol.
//!
//! A plugin is a separate program. Octomind sends it one event at each fixed
//! point of the agent loop and applies the effects in its answer. Messages
//! are JSON-RPC 2.0: one JSON object per line over standard input and output
//! or a local socket, one object per request body over HTTP. The method of a
//! request is the event name; its `params` is [`Event`] with the payload of
//! that event; its `result` is [`Answer`]. The plugin never calls octomind;
//! the one message it sends on its own is [`METHOD_PROGRESS`].
//!
//! This file is the contract. It names no concept of any particular plugin:
//! everything a plugin knows arrives in an event, and everything it wants
//! done is one of the effects below.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Version both sides must speak. A plugin that answers `initialize` with
/// another number is switched off for the session.
pub const PROTOCOL_VERSION: u32 = 1;

pub const METHOD_INITIALIZE: &str = "initialize";
/// Notification carrying the session's full message list, sent once after an
/// answer asked for it with [`Answer::history`].
pub const METHOD_SESSION_HISTORY: &str = "session.history";
/// Notification: the host no longer waits for the answer to a request. The
/// plugin stops the work for it and keeps no state that answer would have
/// committed.
pub const METHOD_CANCEL: &str = "cancel";
/// Notification from the plugin to the host: the plugin is still working on a
/// request. The host starts its wait for that request's answer again, so work
/// that takes long is not cut off while a plugin that went silent still is.
pub const METHOD_PROGRESS: &str = "progress";

/// Every event name, in the order the loop reaches them.
pub mod event {
	pub const SESSION_START: &str = "session.start";
	pub const PROMPT_SUBMIT: &str = "prompt.submit";
	pub const REQUEST_BEFORE: &str = "request.before";
	pub const TOOL_BEFORE: &str = "tool.before";
	pub const TOOL_AFTER: &str = "tool.after";
	pub const TURN_END: &str = "turn.end";
	pub const COMPACT_BEFORE: &str = "compact.before";
	pub const COMPACT_AFTER: &str = "compact.after";
	pub const SUBAGENT_START: &str = "subagent.start";
	pub const SUBAGENT_END: &str = "subagent.end";
	pub const COMMAND: &str = "command";
	pub const STATUS: &str = "status";
	pub const JUDGE: &str = "judge";
	pub const SESSION_END: &str = "session.end";

	pub const ALL: [&str; 14] = [
		SESSION_START,
		PROMPT_SUBMIT,
		REQUEST_BEFORE,
		TOOL_BEFORE,
		TOOL_AFTER,
		TURN_END,
		COMPACT_BEFORE,
		COMPACT_AFTER,
		SUBAGENT_START,
		SUBAGENT_END,
		COMMAND,
		STATUS,
		JUDGE,
		SESSION_END,
	];

	/// Events sent as notifications: the plugin sends no answer.
	pub const NOTIFICATIONS: [&str; 2] = [COMPACT_AFTER, SUBAGENT_END];
}

/// Every effect name, as listed per event in [`InitializeParams::events`].
pub mod effect {
	pub const CONTEXT: &str = "context";
	pub const PIN: &str = "pin";
	pub const USAGE: &str = "usage";
	pub const NOTICE: &str = "notice";
	pub const REGISTER: &str = "register";
	pub const HISTORY: &str = "history";
	pub const PLAN: &str = "plan";
	pub const REPLACE_PROMPT: &str = "replace_prompt";
	pub const BLOCK: &str = "block";
	pub const DENY: &str = "deny";
	pub const LIFT: &str = "lift";
	pub const REPLACE: &str = "replace";
	pub const NOTE: &str = "note";
	pub const STOP: &str = "stop";
	pub const CONTINUE: &str = "continue";
	pub const INHERIT: &str = "inherit";
	pub const OUTPUT: &str = "output";
	pub const ROWS: &str = "rows";
	pub const VERIFIED: &str = "verified";
	pub const REPORT: &str = "report";
	pub const READ: &str = "read";
	pub const RESULT_NOTE: &str = "result_note";
	pub const JUDGEMENTS: &str = "judgements";
}

// ---------------------------------------------------------------------------
// JSON-RPC framing
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RpcRequest {
	pub jsonrpc: String,
	/// Absent on a notification.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub id: Option<u64>,
	pub method: String,
	#[serde(default)]
	pub params: Value,
}

impl RpcRequest {
	pub fn new(id: Option<u64>, method: &str, params: Value) -> Self {
		Self {
			jsonrpc: "2.0".to_string(),
			id,
			method: method.to_string(),
			params,
		}
	}
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RpcResponse {
	pub jsonrpc: String,
	pub id: u64,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub result: Option<Value>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub error: Option<RpcError>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RpcError {
	pub code: i64,
	pub message: String,
}

/// `params` of [`METHOD_CANCEL`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CancelParams {
	/// `id` of the request that is cancelled.
	pub id: u64,
}

/// `params` of [`METHOD_PROGRESS`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProgressParams {
	/// `id` of the request the plugin is still working on.
	pub id: u64,
}

// ---------------------------------------------------------------------------
// Handshake
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InitializeParams {
	pub protocol: u32,
	pub host: ProgramInfo,
	/// Events the host can send, each with the effects it applies for it.
	pub events: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InitializeResult {
	pub protocol: u32,
	pub plugin: ProgramInfo,
	/// Events the plugin wants. An event not listed here is never sent.
	pub subscribe: Vec<String>,
	#[serde(default)]
	pub options: PluginOptions,
	/// Fields the plugin adds to the agent's status line.
	#[serde(default)]
	pub status_fields: Vec<StatusField>,
	/// Slash commands the plugin provides.
	#[serde(default)]
	pub commands: Vec<CommandSpec>,
	/// How long the host waits for the answer to an event without hearing
	/// from the plugin, in seconds, by event name. A [`METHOD_PROGRESS`]
	/// notification for the request starts the wait again. The host's own
	/// limit for the plugin applies when it is shorter. An event not listed
	/// gets the host's default.
	#[serde(default)]
	pub timeouts: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProgramInfo {
	pub name: String,
	pub version: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PluginOptions {
	/// `tool.after`: carry a result's text only when it has at least this many
	/// tokens. Smaller results still arrive with their hash and size.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub min_output_tokens: Option<usize>,
	/// Cut tool results inside `messages` to this many characters.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub result_chars: Option<usize>,
	/// `tool.after`: send [`ToolOutcome::returned`], with its text cut to this
	/// many characters. Without it the plugin sees only what the agent saw.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub returned_chars: Option<usize>,
	/// `turn.end`: send each changed file's `diff` or `head`, cut at this many
	/// characters, and `tree_status`. Without it the host sends only `path`
	/// and `exists` per file, for a plugin that reads the work tree itself.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub diff_chars: Option<usize>,
	/// `read`: the most files the host sends for one answer, and the largest
	/// file it reads. Larger files arrive with `skipped: too_large`.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub read_max_files: Option<usize>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub read_max_file_bytes: Option<usize>,
}

/// One extra field of the status line. The host adds `instruction` to its
/// status-line prompt, parses the field and returns its value in
/// [`Status::fields`] under `name`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StatusField {
	pub name: String,
	pub instruction: String,
	/// Value shown for the field in the status-line template.
	#[serde(default)]
	pub default: Value,
	/// Value shown for the field in the example line. Without it the example
	/// shows `default`.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub example: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CommandSpec {
	/// Without the leading slash.
	pub name: String,
	pub hint: String,
	pub description: String,
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

/// `params` of every event: the fields common to all, then the payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Event<P> {
	/// Stable for the whole session, including after a resume.
	pub session_id: String,
	/// Set for a subagent's session.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub parent_session_id: Option<String>,
	pub role: String,
	pub workdir: String,
	/// Name of the project the session works in: the base name of the
	/// repository root, or of the workdir outside a repository.
	pub project: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub git_remote: Option<String>,
	/// The model the agent's requests go to, when the host knows it.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub model: Option<String>,
	/// Messages added to the session since the previous event sent to this
	/// plugin. With `messages_reset` it is the full list instead.
	#[serde(default)]
	pub messages: Vec<Message>,
	/// The host rewrote its history (a compaction, a resume, a rollback), so
	/// `messages` replaces everything the plugin holds for this session.
	#[serde(default)]
	pub messages_reset: bool,
	/// The status line parsed from the latest assistant message.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub status: Option<Status>,
	/// Rules the plugin registered that matched since the previous event.
	#[serde(default)]
	pub rule_matches: Vec<RuleMatch>,
	/// The host's plan, when one is open.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub plan: Option<PlanState>,
	/// The host could not apply the last `plan` effect, with its reason.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub plan_error: Option<String>,
	/// The session's tool list: always present on `session.start`, afterwards
	/// only when it changed since the previous event.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub tools: Option<Vec<ToolInfo>>,
	/// Names of the skills active in the session now.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub active_skills: Vec<String>,
	/// Names of this plugin's context blocks the host left out of the previous
	/// model request for lack of room.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub dropped_context: Vec<String>,
	/// Files the plugin asked for with `read`, and files it read before that
	/// changed on disk since.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub files: Vec<FileText>,
	/// How many judgements of each purpose changed what the host did since the
	/// previous event, by [`Judge::purpose`].
	#[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
	pub judged: BTreeMap<String, u64>,
	#[serde(flatten)]
	pub payload: P,
}

/// One file's text as the host read it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileText {
	/// Relative to the workdir.
	pub path: String,
	/// SHA-256 of the file's bytes, hex. Empty when the file was not read.
	pub hash: String,
	/// `None` when the file was not read; `skipped` says why.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub text: Option<String>,
	/// `missing`, `outside`, `ignored`, `not_text` or `too_large`.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub skipped: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Message {
	/// `system`, `user`, `assistant` or `tool`.
	pub role: String,
	pub content: String,
	/// Seconds since the Unix epoch.
	pub timestamp: u64,
	/// For role `user`: the host or a plugin put this text there, not the
	/// person. Such a message is never the user's request.
	#[serde(default)]
	pub managed: bool,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub id: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub name: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub tool_call_id: Option<String>,
	/// For an assistant message: the tool calls as the provider returned them.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub tool_calls: Option<Value>,
	/// For an assistant message: its reasoning text.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub thinking: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Status {
	/// `exploring`, `progressing`, `blocked`, `need_input` or `done`.
	pub state: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub focus: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub next: Option<String>,
	#[serde(default)]
	pub carry: Vec<String>,
	/// Values of the fields plugins registered, by field name.
	#[serde(default)]
	pub fields: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RuleMatch {
	pub id: String,
	pub outcome: RuleOutcome,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuleOutcome {
	/// The rule matched and took effect.
	Applied,
	/// The rule matched in `observe` mode, so nothing was done.
	Observed,
}

/// The host's plan as the agent sees it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlanState {
	pub title: String,
	pub tasks: Vec<PlanTask>,
	/// Index into `tasks` of the open task.
	pub current: usize,
	/// When a model last changed the plan, seconds since the Unix epoch.
	pub touched_at: u64,
	/// Index of the first message of the open task in the host's history.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub task_start_index: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlanTask {
	pub title: String,
	/// What must be observable for the task to count as done.
	pub done_when: String,
	pub done: bool,
	/// A file condition that keeps the task valid: `file_exists:<path>` or
	/// `file_absent:<path>`.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub valid_if: Option<String>,
	/// `Some(false)` when `valid_if` no longer holds.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub valid: Option<bool>,
}

/// `session.start`: a session starts, resumes, forks or is cleared.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionStart {
	/// `startup`, `resume`, `clear`, `compact` or `fork`.
	pub source: String,
	pub system_prompt: String,
	/// A person is at the terminal. `false` for one-shot and server runs.
	pub interactive: bool,
	/// The agent can read a file the host saved a full tool output to.
	pub can_read_saved_output: bool,
	/// The value a parent session handed down with `inherit`.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub inherited: Option<Value>,
	/// Capabilities available to the session's role.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub capabilities: Vec<String>,
	/// Absolute paths of the instruction files the host already put in the
	/// model's context, such as `AGENTS.md`.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub context_files: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolInfo {
	pub name: String,
	pub server: String,
	#[serde(default)]
	pub description: String,
	/// JSON schema of the arguments.
	#[serde(default)]
	pub parameters: Value,
	/// The tool declares that it changes nothing.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub read_only: Option<bool>,
}

/// `prompt.submit`: the user sent a prompt, before the model sees it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PromptSubmit {
	pub prompt: String,
	/// The text as typed, when a built-in rule rewrote it.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub original: Option<String>,
	/// The host's summary of earlier work, when a compaction left one.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub carried_summary: Option<String>,
	/// The goal that summary records, when it has one.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub goal: Option<String>,
}

/// `request.before`: before every model request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RequestBefore {
	pub phase: RequestPhase,
	pub turn: TurnKind,
	/// Estimated tokens left in the context window.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub free_context_tokens: Option<usize>,
	/// Background work of this session is still running.
	#[serde(default)]
	pub pending_async: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RequestPhase {
	/// First request of a turn.
	TurnStart,
	/// A request that follows a round of tool calls.
	FollowUp,
	/// A request that follows a `continue` effect.
	Continuation,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TurnKind {
	/// The turn belongs to the user's latest task: the user sent a prompt, or
	/// the host resumed a turn that was waiting for background work.
	User,
	/// The host started the turn: a background result, a schedule, a hook.
	System,
}

/// `tool.before`: before a round of tool calls runs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolBefore {
	pub calls: Vec<ToolCall>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
	pub id: String,
	pub tool: String,
	pub server: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub capability: Option<String>,
	pub arguments: Value,
	/// A built-in rule already denied the call. The plugin is not asked.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub denied: Option<String>,
	/// Ids of rules this plugin registered that would deny the call.
	#[serde(default)]
	pub rules: Vec<String>,
	/// Set for a call a `code` script made: the id of that `code` call.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub script_call_id: Option<String>,
}

/// `tool.after`: after a round of tool calls, before the model sees the
/// results. With no `calls` and with `background` set, it reports output of
/// detached jobs that arrived between rounds.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolAfter {
	/// What the assistant said, or thought, when it made the calls.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub intent: Option<String>,
	pub calls: Vec<ToolOutcome>,
	/// Fingerprints of the working tree before and after the round.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub tree: Option<TreeChange>,
	/// Subagent runs that finished in this round.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub subagents: Option<SubagentRuns>,
	/// Output of detached jobs that arrived since the previous event.
	#[serde(default)]
	pub background: Vec<BackgroundOutput>,
	/// The host ends the turn after this round, with this reason.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub stopping: Option<String>,
	/// Absolute paths of the instruction files the host put in the model's
	/// context during this round.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub context_files: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolOutcome {
	pub id: String,
	pub tool: String,
	pub server: String,
	pub arguments: Value,
	pub error: bool,
	/// A rule denied the call, so `result` is the reason, not tool output.
	#[serde(default)]
	pub denied: bool,
	/// Text of the result, after the host's own size cap. Absent when the
	/// result is below `min_output_tokens`. A result that is not plain text
	/// arrives as its text form with `replaceable` unset.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub result: Option<String>,
	/// Hash of the full result text, stable for equal text.
	pub result_hash: String,
	/// Estimated tokens of the result.
	pub size_tokens: usize,
	/// Length of the result text in bytes.
	#[serde(default)]
	pub size_bytes: usize,
	/// The call changes state, by the host's tool classification.
	pub mutation: bool,
	/// The command the call ran, when it ran one.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub command: Option<String>,
	/// The tool runs a free-form command string, such as a shell.
	#[serde(default)]
	pub free_form: bool,
	/// Paths named in the arguments, made absolute by the host.
	#[serde(default)]
	pub paths: Vec<String>,
	/// The host can replace this result and keep the original readable.
	#[serde(default)]
	pub replaceable: bool,
	/// What the tool returned, when the host showed the agent something else:
	/// a note in place of a repeated result, or a text that no longer starts
	/// the way the tool's did. Sent to a plugin that set
	/// [`PluginOptions::returned_chars`].
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub returned: Option<Returned>,
	/// For a `code` call: every tool call its script made, in call order. The
	/// agent saw only what the script printed (`result`), so only the `code`
	/// call can be replaced.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub script_calls: Vec<ToolOutcome>,
}

/// A tool result as the tool returned it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Returned {
	pub error: bool,
	/// The start of the text.
	pub text: String,
}

/// Both fields are empty when the host cannot measure the tree.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TreeChange {
	pub before: String,
	pub after: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct SubagentRuns {
	pub finished: u32,
	/// How many of them reported their own work as verified.
	pub verified: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BackgroundOutput {
	/// The id a `replace` names this output by: the tool call that started the
	/// job when the host knows it, else an id of the output's own. Empty when
	/// the output cannot be replaced.
	pub call_id: String,
	/// The job's name or command line, as the host shows it.
	#[serde(default)]
	pub label: String,
	pub output: String,
}

/// `turn.end`: the agent stopped with no more tool calls.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TurnEnd {
	pub turn: TurnKind,
	/// The final texts of this turn, oldest first. There is more than one
	/// after a `continue` effect.
	pub answers: Vec<String>,
	/// Background work will resume this turn later, so it is not over.
	#[serde(default)]
	pub pending_async: bool,
	/// Files the turn's tool calls changed, with what is on disk now.
	#[serde(default)]
	pub changes: Vec<FileChange>,
	/// Lines of `git status --porcelain`, capped by the host.
	#[serde(default)]
	pub tree_status: Vec<String>,
	/// Fingerprint of the working tree now.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub tree: Option<String>,
	/// Model requests made since the turn began.
	pub requests: u64,
	/// Why the host ended the turn itself, for example `loop`.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub stopped: Option<String>,
	#[serde(default)]
	pub background: Vec<BackgroundOutput>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileChange {
	pub path: String,
	pub exists: bool,
	/// Diff against the last commit. Absent outside a Git work tree and for
	/// a file that did not change.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub diff: Option<String>,
	/// First lines of the file, when it has no diff.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub head: Option<String>,
	/// The host cut `diff` or `head` at its size limit.
	#[serde(default)]
	pub truncated: bool,
}

/// `compact.before`: before a compaction folds messages away.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompactBefore {
	/// The user asked for it, for example with `/done`.
	#[serde(default)]
	pub forced: bool,
}

/// `compact.after`: notification after a compaction. The next event carries
/// the new history with `messages_reset`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompactAfter {
	/// The host's summary of the folded work.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub carried_summary: Option<String>,
	/// The goal that summary records, when it has one.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub goal: Option<String>,
}

/// `subagent.start`: a subagent is about to start.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SubagentStart {
	/// Absent when the child's id is not known before it starts.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub child_session_id: Option<String>,
	pub prompt: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub system_prompt: Option<String>,
}

/// `subagent.end`: notification that a subagent finished.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SubagentEnd {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub child_session_id: Option<String>,
}

/// `command`: the user ran a command the plugin provides.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Command {
	pub name: String,
	pub arguments: String,
}

/// `status`: `/info` or a connected client asks for the plugin's state.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct StatusRequest {}

/// `judge`: the host asks for a judgement it otherwise makes by a fixed rule,
/// for example which of several skills fits a request. A plugin that has no
/// answer returns none, and the host keeps the outcome of its own rule.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Judge {
	/// What the host decides with the answer, for example `skills`.
	pub purpose: String,
	/// What the questions are about.
	pub subject: Value,
	/// Questions about the subject, by id.
	#[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
	pub questions: BTreeMap<String, Question>,
	/// Parts of the subject that each get the same yes-or-no question.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub items: Option<JudgeItems>,
}

/// One question of a `judge` event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Question {
	/// Yes or no.
	YesNo { text: String },
	/// One of `options`: each option's name with what it stands for.
	Choice {
		text: String,
		options: BTreeMap<String, String>,
	},
}

/// Parts of a `judge` subject, each judged on its own. The plugin decides how
/// many of them one model call reads: it shows the model `subject` with the
/// parts of that call as a list under `key`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JudgeItems {
	pub key: String,
	/// The yes-or-no question for one part. `{slot}` becomes the part's place
	/// in the list the model reads and `{id}` its id.
	pub question: String,
	pub items: Vec<JudgeItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JudgeItem {
	/// Unique among the items; the answer names the part by it.
	pub id: String,
	pub data: Value,
}

/// `session.end`: the session ends. The answer may carry `notice`, `usage`
/// and `report`; the host waits for it only briefly.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionEnd {
	/// `exit`, `clear`, `switch` or `error`.
	pub reason: String,
}

// ---------------------------------------------------------------------------
// Answer and effects
// ---------------------------------------------------------------------------

/// `result` of every event that is not a notification. Every field is
/// optional. The host ignores an effect the event does not allow.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Answer {
	/// Set or clear named context blocks: text added to each model request
	/// and never written into the history. The host joins a plugin's blocks
	/// in the order they were first set and adds nothing between them.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub context: Vec<Block>,
	/// Set or clear named pinned blocks: text the host keeps through every
	/// compaction and recites near the end of the context.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub pin: Vec<Block>,
	/// What the plugin spent on models since its last answer.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub usage: Vec<Usage>,
	/// The plugin's accounting for the session as data, for the host's
	/// machine-readable cost output. Sending it again replaces what the
	/// plugin sent before.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub report: Option<Report>,
	/// Lines shown to the user as a dim notice. Each entry is one notice; a
	/// notice may hold several lines.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub notice: Vec<String>,
	/// Rules, skill directories and commands the plugin provides. Sending it
	/// again replaces what the plugin registered before.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub register: Option<Register>,
	/// Ask for the session's full message list as a `session.history`
	/// notification. For a plugin that lost its state.
	#[serde(default, skip_serializing_if = "is_false")]
	pub history: bool,
	/// Paths or globs, relative to the workdir, whose text the plugin wants.
	/// The host sends them with the next event in `files`. It reads only
	/// regular text files inside the workdir's repository that Git does not
	/// ignore.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub read: Vec<String>,
	/// Change the host's plan.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub plan: Option<PlanUpdate>,
	/// `prompt.submit`: replace the text of the prompt.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub replace_prompt: Option<String>,
	/// `prompt.submit`: stop the prompt and show the user this message.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub block: Option<String>,
	/// `tool.before`: do not run these calls. The model gets the reason as
	/// the call's result.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub deny: Vec<Deny>,
	/// `tool.before`: let these calls pass rules this plugin registered.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub lift: Vec<Lift>,
	/// `tool.after`: replace the text of these results.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub replace: Vec<Replace>,
	/// Text queued for the model as feedback managed by the system. It is
	/// written into the history just before the next model request.
	/// `{plan_checklist}` in a note sent together with `plan` becomes the
	/// host's checklist after the change.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub note: Vec<String>,
	/// `tool.after`: text written into the history right after the round's
	/// results, before the host folds history or recites. `{plan_checklist}`
	/// works as in `note`.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub result_note: Vec<String>,
	/// `tool.after`: end the turn with this notice, with no further request.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub stop: Option<String>,
	/// `turn.end`: keep the turn going with this text as the next input.
	#[serde(default, skip_serializing_if = "Option::is_none", rename = "continue")]
	pub continue_with: Option<String>,
	/// `subagent.start`: a value handed to the subagent's `session.start`.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub inherit: Option<Value>,
	/// `command`: the result shown to the user.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub output: Option<CommandOutput>,
	/// `status`: label and value pairs for `/info`.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub rows: Vec<Row>,
	/// `turn.end`: the plugin checked the turn's work and found it complete.
	/// The host passes it to a parent session as the subagent's verdict.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub verified: Option<bool>,
	/// `judge`: the answers by question id, and by item id for `items`. A
	/// question the plugin could not answer is left out.
	#[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
	pub judgements: BTreeMap<String, Judgement>,
}

fn is_false(value: &bool) -> bool {
	!*value
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Block {
	pub name: String,
	/// `None` clears the block.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub text: Option<String>,
	/// `context`: keep this block when the host drops blocks for room.
	#[serde(default, skip_serializing_if = "is_false")]
	pub keep: bool,
	/// `pin`: lines the host keeps as separate items when it folds history.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub items: Vec<PinItem>,
}

/// One line of a pinned block that survives a fold of the history on its own.
/// In JSON it is the bare text, or an object when `label` or `from_request`
/// is set.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(from = "PinItemWire", into = "PinItemWire")]
pub struct PinItem {
	pub text: String,
	/// The word the host prints before the text, in place of `constraint`.
	pub label: Option<String>,
	/// The text is a line of the current request, so the host may name that
	/// message as its source.
	pub from_request: bool,
}

impl PinItem {
	pub fn text(text: impl Into<String>) -> Self {
		Self {
			text: text.into(),
			..Self::default()
		}
	}
}

impl From<&str> for PinItem {
	fn from(text: &str) -> Self {
		Self::text(text)
	}
}

impl From<String> for PinItem {
	fn from(text: String) -> Self {
		Self::text(text)
	}
}

/// Equal to a text when the item is that text and nothing more.
impl PartialEq<str> for PinItem {
	fn eq(&self, text: &str) -> bool {
		self.label.is_none() && !self.from_request && self.text == text
	}
}

impl PartialEq<&str> for PinItem {
	fn eq(&self, text: &&str) -> bool {
		self == *text
	}
}

impl PartialEq<String> for PinItem {
	fn eq(&self, text: &String) -> bool {
		self == text.as_str()
	}
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum PinItemWire {
	Text(String),
	Full {
		text: String,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		label: Option<String>,
		#[serde(default, skip_serializing_if = "is_false")]
		from_request: bool,
	},
}

impl From<PinItemWire> for PinItem {
	fn from(wire: PinItemWire) -> Self {
		match wire {
			PinItemWire::Text(text) => Self::text(text),
			PinItemWire::Full {
				text,
				label,
				from_request,
			} => Self {
				text,
				label,
				from_request,
			},
		}
	}
}

impl From<PinItem> for PinItemWire {
	fn from(item: PinItem) -> Self {
		if item.label.is_none() && !item.from_request {
			return Self::Text(item.text);
		}
		Self::Full {
			text: item.text,
			label: item.label,
			from_request: item.from_request,
		}
	}
}

/// The answer to one question of a `judge` event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Judgement {
	YesNo {
		/// Probability of yes, 0 to 1.
		yes: f64,
	},
	Choice {
		/// The most probable option.
		choice: String,
		/// Every option with its probability; they sum to 1.
		probabilities: BTreeMap<String, f64>,
	},
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Usage {
	/// What the calls were for, as shown in `/info`.
	pub label: String,
	pub calls: u64,
	pub input_tokens: u64,
	pub output_tokens: u64,
	#[serde(default)]
	pub reasoning_tokens: u64,
	/// In US dollars.
	pub cost: f64,
	/// Wall time of the calls in milliseconds.
	#[serde(default)]
	pub wall_ms: u64,
	/// The agent waited for these calls.
	#[serde(default)]
	pub blocking: bool,
}

/// What a plugin did and spent in a session so far. `usage` is the running
/// bill the host adds up; this is the plugin's own breakdown of it.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Report {
	/// One entry per mechanic of the plugin, by its name. The host passes
	/// each entry on as it is.
	pub mechanics: BTreeMap<String, Value>,
	pub totals: ReportTotals,
}

/// Sums over all mechanics of a [`Report`]. Times are in milliseconds.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ReportTotals {
	/// Calls to a model that writes text.
	pub chat_calls: u64,
	/// Calls to a model that answers a fixed question.
	pub decision_calls: u64,
	/// Time the agent waited for the plugin.
	pub wall_blocking_ms: u64,
	/// Time the plugin worked while the agent went on. `None` when it
	/// cannot be told.
	pub wall_background_ms: Option<u64>,
	/// Time spent inside model calls.
	pub api_ms: u64,
	pub input_tokens: u64,
	pub output_tokens: u64,
	pub reasoning_tokens: u64,
	/// In US dollars.
	pub cost: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Register {
	/// Rules in the format of `.agents/guardrails.toml`. Each entry carries
	/// an `id` and a `mode`: `apply`, or `observe`, which records a match
	/// without applying the rule.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub rules: Option<String>,
	/// Directories with skills the plugin provides.
	#[serde(default)]
	pub skill_dirs: Vec<SkillDir>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SkillDir {
	pub id: String,
	pub path: String,
	/// `apply` or `observe`.
	pub mode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum PlanUpdate {
	Start {
		title: String,
		tasks: Vec<PlanTaskSpec>,
	},
	/// Close the open task and open the next one.
	Advance {
		summary: String,
	},
	/// Replace the tasks that are not done.
	Revise {
		reason: String,
		tasks: Vec<PlanTaskSpec>,
	},
	/// Close every task and drop the plan.
	Finish {
		summary: String,
	},
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlanTaskSpec {
	pub title: String,
	pub done_when: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Deny {
	pub call_id: String,
	pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Lift {
	pub call_id: String,
	pub rules: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Replace {
	/// A call of this round, or the `call_id` of a [`BackgroundOutput`].
	pub call_id: String,
	/// The new text. `{original_path}` in it becomes the path of the file
	/// the host saved the original output to.
	pub text: String,
	/// Save the original where the agent can read it. When the host cannot,
	/// it leaves the result as it is.
	#[serde(default)]
	pub keep_original: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct CommandOutput {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub text: Option<String>,
	/// Structured result for clients that render it themselves.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub data: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Row {
	pub label: String,
	pub value: String,
}

#[cfg(test)]
#[path = "protocol_tests.rs"]
mod tests;
