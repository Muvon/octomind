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

// WebSocket protocol message definitions

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub const MESSAGE_ATTACHMENTS_CAPABILITY: &str = "message_attachments_v1";
/// The bind's session accepts `cancel`/`close` frames and a bind `role`, and
/// echoes a request's `request_id` on the cost frame that closes its turn.
pub const SESSION_CONTROL_CAPABILITY: &str = "session_control_v1";
pub const RUN_EVENTS_CAPABILITY: &str = "run_events_v1";
/// Session output frames carry `stream_seq`, the bind ack reports the stream
/// position, and a bind may `resume` a stream it was reading.
pub const STREAM_RESUME_CAPABILITY: &str = "stream_resume_v1";
/// Sent by both sides: the server forwards MCP elicitations as `elicitation`
/// frames, and a client that lists it in its bind `capabilities` answers them
/// with `elicitation_response`. Without it, the session declines them.
pub const ELICITATION_CAPABILITY: &str = "elicitation_v1";
/// A client that lists it in its bind `capabilities` gets the slash commands a
/// session can run in the bind ack's `commands`. Without it the ack has no such
/// field, so older clients see the ack they always did.
pub const COMMANDS_CAPABILITY: &str = "commands_v1";

// ── Client → Server ──────────────────────────────────────────────────────────

/// Create or resume a session. No AI call is made.
/// Server responds with a `status` message containing the `session_id`.
///
/// - `session_id` absent  → create new auto-named session
/// - `session_id` present → resume if exists on disk, else create with that name
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMessage {
	/// Optional client-supplied correlation ID. Echoed by the server in the ack.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub request_id: Option<String>,

	/// Session name / ID. Absent = auto-named, present = create-or-resume.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub session_id: Option<String>,

	/// Working directory the session runs in: an existing absolute directory.
	/// Absent = the server's own directory, as before. Everything octomind
	/// resolves from the working directory follows it — `AGENTS.md`,
	/// `.agents/` (guardrails, skills, tools, plugins) and the project id MCP
	/// servers scope their state by. Re-sent on each bind; it re-anchors the
	/// session, so a mid-turn `workdir` change does not outlive the turn.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub cwd: Option<String>,

	/// Agent tag (e.g. `developer:rust`) or role name a session CREATED by this
	/// bind runs under. Absent = the server's role. A resumed session keeps the
	/// role it already runs under — switch it with the `role` command — so a
	/// client may send the same bind every time without ever re-resolving.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub role: Option<String>,

	/// Continue reading a stream this client already read up to `after`: the
	/// frames it missed follow the ack when the server still holds them all
	/// (the ack says `resumed`). Either way the bind takes the session's output
	/// over — a bind without `resume` only takes output nobody live owns.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub resume: Option<StreamResume>,

	/// Capabilities this client implements, e.g. [`ELICITATION_CAPABILITY`].
	/// Re-sent on each bind; they belong to whichever client owns the output.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub capabilities: Vec<String>,
}

/// Where a client stopped reading a session's output.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StreamResume {
	/// The `stream` id of the bind ack the client read under.
	pub stream: String,
	/// The last `stream_seq` the client received.
	pub after: u64,
}

/// Send user input to an existing session and receive an AI response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserMessage {
	/// Optional client-supplied correlation ID. Echoed by the server in the ack.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub request_id: Option<String>,

	/// Session name / ID — must refer to an established session.
	pub session_id: String,

	/// User input sent to the AI. May be empty when attachments are present, max 10 MB.
	pub content: String,

	/// Media uploaded out-of-band and referenced by opaque IDs.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub attachments: Vec<Attachment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
	/// Opaque media identifier. Never interpreted as a caller-supplied path.
	pub id: String,
	pub kind: AttachmentKind,
	pub media_type: String,
	pub name: String,
	pub size: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentKind {
	Image,
	Video,
	Audio,
}

impl Attachment {
	fn validate_id(&self) -> Result<(), String> {
		if self.id.len() != 24 || !self.id.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
			return Err(format!(
				"attachment id '{}' must be exactly 24 ASCII alphanumeric characters",
				self.id
			));
		}
		Ok(())
	}

	/// Resolve only after validating the opaque ID. Keeping this operation here
	/// prevents transport metadata from ever being treated as a filesystem path.
	///
	/// The writer stores the file as `<id>.<ext>` (the extension is required on
	/// its side, both for format detection and so the file is browsable in a
	/// Files UI), so we locate it by prefix rather than reconstructing the
	/// extension from `media_type` — that would need a mime→extension table
	/// kept in sync across two repos forever, and any drift becomes a silent
	/// "file not found".
	pub(crate) fn resolve_path(&self, media_root: &Path) -> Result<PathBuf, String> {
		self.validate_id()?;
		let prefix = format!("{}.", self.id);
		let entries = std::fs::read_dir(media_root)
			.map_err(|error| format!("attachment '{}' could not be located: {}", self.id, error))?;

		let mut matches = Vec::new();
		for entry in entries {
			let entry = entry.map_err(|error| format!("attachment '{}' could not be located: {}", self.id, error))?;
			if entry.file_name().to_string_lossy().starts_with(&prefix) {
				matches.push(entry.path());
			}
		}

		match matches.len() {
			0 => Err(format!("attachment '{}' not found", self.id)),
			1 => Ok(matches.remove(0)),
			_ => Err(format!(
				"attachment '{}' has multiple matching files in the media store",
				self.id
			)),
		}
	}
}

/// Execute a session command (equivalent to `/command [args…]` in the CLI).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandMessage {
	/// Optional client-supplied correlation ID. Echoed by the server in the ack.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub request_id: Option<String>,

	/// Session name / ID — must refer to an established session.
	pub session_id: String,

	/// Command name without the leading `/`.
	/// Examples: `"info"`, `"model"`, `"mcp"`, `"help"`, `"role"`
	pub command: String,

	/// Optional arguments.
	/// Examples: `["list"]` for `/mcp list`, `["openrouter:claude-sonnet-4"]` for `/model`
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub args: Vec<String>,
}

/// Stop the session's running turn (equivalent to Ctrl+C in the CLI).
///
/// Read while a turn runs, from any connection: the turn ends as it would on
/// Ctrl+C and its `cost` frame carries `cancelled: true`. A session with no
/// running turn is a no-op — the ack is the only reply either way.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CancelMessage {
	/// Optional client-supplied correlation ID. Echoed by the server in the ack.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub request_id: Option<String>,

	/// Session whose turn to stop.
	pub session_id: String,
}

/// Unload an idle session from server memory. Its log stays on disk, so a
/// later `session` bind resumes it. A long-running server holds every bound
/// session otherwise; clients that create many short-lived sessions close
/// them once done. Refused while the session runs a turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloseMessage {
	/// Optional client-supplied correlation ID. Echoed by the server in the ack.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub request_id: Option<String>,

	/// Session to unload.
	pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunLogMessage {
	pub session_id: String,
	pub run_id: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub request_id: Option<String>,
	/// Reply with status and only the run's latest `usage` event instead of the whole log.
	/// Older servers ignore the field and send the full log.
	#[serde(default, skip_serializing_if = "std::ops::Not::not")]
	pub summary: bool,
}

/// The user's answer to an `elicitation` frame, in MCP's own result shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElicitationResponseMessage {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub request_id: Option<String>,
	pub session_id: String,
	/// The `id` of the `elicitation` frame being answered.
	pub id: String,
	pub result: rmcp::model::ElicitResult,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RunEvent {
	Prompt {
		content: String,
	},
	Assistant {
		content: String,
	},
	Thinking {
		content: String,
	},
	ToolUse {
		tool: String,
		tool_id: String,
		params: Value,
	},
	ToolResult {
		tool_id: String,
		content: String,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		success: Option<bool>,
	},
	Usage {
		cost: f64,
		input_tokens: u64,
		output_tokens: u64,
	},
	Status {
		status: crate::session::tap_runs::TapJobStatus,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		error: Option<String>,
	},
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunEventPayload {
	pub session_id: String,
	pub run_id: String,
	pub role: String,
	pub seq: u64,
	pub event: RunEvent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunLogPayload {
	pub session_id: String,
	pub run_id: String,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub role: Option<String>,
	pub status: String,
	pub source: String,
	pub seq: u64,
	pub truncated: bool,
	pub events: Vec<RunEvent>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub error: Option<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub request_id: Option<String>,
}

impl RunLogPayload {
	pub(crate) fn error(request: RunLogMessage, error: String) -> Self {
		Self {
			session_id: request.session_id,
			run_id: request.run_id,
			role: None,
			status: "unknown".to_string(),
			source: "disk".to_string(),
			seq: 0,
			truncated: false,
			events: Vec::new(),
			error: Some(error),
			request_id: request.request_id,
		}
	}
}

/// Incoming message from client to server.
/// Internally tagged by `"type"` so each variant carries only its own fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
	Session(SessionMessage),
	Message(UserMessage),
	Command(CommandMessage),
	Cancel(CancelMessage),
	Close(CloseMessage),
	RunLog(RunLogMessage),
	ElicitationResponse(ElicitationResponseMessage),
}

impl ClientMessage {
	/// Semantic validation beyond what serde already enforces structurally.
	pub fn validate(&self) -> Result<(), String> {
		fn validate_request_id(request_id: &Option<String>) -> Result<(), String> {
			if let Some(id) = request_id {
				if id.trim().is_empty() {
					return Err("request_id cannot be empty when provided".to_string());
				}
				if id.len() > 256 {
					return Err("request_id exceeds maximum size (256 bytes)".to_string());
				}
			}
			Ok(())
		}

		match self {
			ClientMessage::Session(m) => validate_request_id(&m.request_id)?,
			ClientMessage::Message(m) => validate_request_id(&m.request_id)?,
			ClientMessage::Command(c) => validate_request_id(&c.request_id)?,
			ClientMessage::Cancel(c) => validate_request_id(&c.request_id)?,
			ClientMessage::Close(c) => validate_request_id(&c.request_id)?,
			ClientMessage::RunLog(c) => validate_request_id(&c.request_id)?,
			ClientMessage::ElicitationResponse(c) => validate_request_id(&c.request_id)?,
		}

		match self {
			ClientMessage::RunLog(m) => {
				if m.session_id.trim().is_empty() {
					return Err("session_id cannot be empty".to_string());
				}
				crate::session::tap_runs::validate_run_id(&m.run_id)
			}
			ClientMessage::Session(m) => {
				if let Some(cwd) = &m.cwd {
					if !Path::new(cwd).is_absolute() {
						return Err("cwd must be an absolute path".to_string());
					}
				}
				if m.role.as_deref().is_some_and(|role| role.trim().is_empty()) {
					return Err("role cannot be empty when provided".to_string());
				}
				if m.resume.as_ref().is_some_and(|resume| resume.stream.trim().is_empty()) {
					return Err("resume.stream cannot be empty".to_string());
				}
				Ok(())
			}

			ClientMessage::Message(m) => {
				if m.session_id.trim().is_empty() {
					return Err("session_id cannot be empty".to_string());
				}
				if m.content.trim().is_empty() && m.attachments.is_empty() {
					return Err("content and attachments cannot both be empty".to_string());
				}
				if m.content.len() > 10 * 1024 * 1024 {
					return Err("content exceeds maximum size (10MB)".to_string());
				}
				for attachment in &m.attachments {
					attachment.validate_id()?;
				}
				Ok(())
			}

			ClientMessage::Command(c) => {
				if c.session_id.trim().is_empty() {
					return Err("session_id cannot be empty".to_string());
				}
				if c.command.trim().is_empty() {
					return Err("command cannot be empty".to_string());
				}
				Ok(())
			}

			ClientMessage::Cancel(CancelMessage { session_id, .. })
			| ClientMessage::Close(CloseMessage { session_id, .. }) => {
				if session_id.trim().is_empty() {
					return Err("session_id cannot be empty".to_string());
				}
				Ok(())
			}

			ClientMessage::ElicitationResponse(m) => {
				if m.session_id.trim().is_empty() {
					return Err("session_id cannot be empty".to_string());
				}
				if m.id.trim().is_empty() {
					return Err("id cannot be empty".to_string());
				}
				Ok(())
			}
		}
	}

	pub fn request_id(&self) -> Option<&str> {
		match self {
			ClientMessage::Session(m) => m.request_id.as_deref(),
			ClientMessage::Message(m) => m.request_id.as_deref(),
			ClientMessage::Command(c) => c.request_id.as_deref(),
			ClientMessage::Cancel(c) => c.request_id.as_deref(),
			ClientMessage::Close(c) => c.request_id.as_deref(),
			ClientMessage::RunLog(c) => c.request_id.as_deref(),
			ClientMessage::ElicitationResponse(c) => c.request_id.as_deref(),
		}
	}

	pub fn message_type(&self) -> &'static str {
		match self {
			ClientMessage::Session(_) => "session",
			ClientMessage::Message(_) => "message",
			ClientMessage::Command(_) => "command",
			ClientMessage::Cancel(_) => "cancel",
			ClientMessage::Close(_) => "close",
			ClientMessage::RunLog(_) => "run_log",
			ClientMessage::ElicitationResponse(_) => "elicitation_response",
		}
	}

	pub fn session_id(&self) -> Option<&str> {
		match self {
			ClientMessage::Session(m) => m.session_id.as_deref(),
			ClientMessage::Message(m) => Some(m.session_id.as_str()),
			ClientMessage::Command(c) => Some(c.session_id.as_str()),
			ClientMessage::Cancel(c) => Some(c.session_id.as_str()),
			ClientMessage::Close(c) => Some(c.session_id.as_str()),
			ClientMessage::RunLog(c) => Some(c.session_id.as_str()),
			ClientMessage::ElicitationResponse(c) => Some(c.session_id.as_str()),
		}
	}
}

// ── Server → Client ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantPayload {
	pub content: String,
	pub session_id: String,
	/// Workflow step name this message originated from. Omitted for
	/// single-session `run` output; set per step by `octomind workflow`.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub step: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkingPayload {
	pub content: String,
	pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolUsePayload {
	pub tool: String,
	pub tool_id: String,
	pub server: String,
	pub params: Value,
	pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultPayload {
	pub tool: String,
	pub tool_id: String,
	pub server: String,
	pub content: String,
	pub success: bool,
	pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostPayload {
	pub session_tokens: u64,
	pub session_cost: f64,
	pub input_tokens: u64,
	pub output_tokens: u64,
	pub cache_read_tokens: u64,
	pub cache_write_tokens: u64,
	pub reasoning_tokens: u64,
	/// Prompt tokens of the session's own auxiliary model calls — compression
	/// folds and the supervisor (gate, condense, resolve, plan, learning) —
	/// which the main-model counters above do not include. `session_cost` does;
	/// these let a consumer that prices tokens itself bill the whole session.
	/// Running totals, like every other counter here.
	#[serde(default)]
	pub aux_input_tokens: u64,
	/// Completion tokens (reasoning included) of those auxiliary calls.
	#[serde(default)]
	pub aux_output_tokens: u64,
	/// Provider-reported USD of those auxiliary calls. They run on their own
	/// models, so a consumer pricing the main tokens itself must not reprice
	/// these at the main model's rate.
	#[serde(default)]
	pub aux_cost: f64,
	#[serde(default)]
	pub supervisor_accounting: crate::supervisor::report::SupervisorReport,
	pub session_id: String,
	/// The turn is over, but it left work that streams a follow-up turn to this
	/// connection on its own — a delegated `tap` run, an async agent job, or a
	/// result already queued (`session::has_pending_handback`). A client that
	/// wants that result stays connected until a cost frame without it. Omitted
	/// when false, so an idle turn's frame reads exactly as older agents' did.
	#[serde(default, skip_serializing_if = "std::ops::Not::not")]
	pub pending_work: bool,
	/// The turn this frame closes was stopped by a `cancel` frame. Omitted when
	/// false, so a finished turn's frame reads exactly as older agents' did.
	#[serde(default, skip_serializing_if = "std::ops::Not::not")]
	pub cancelled: bool,
	/// Echo of the `request_id` of the client frame whose turn this cost
	/// closes. Absent on costs of turns the session ran by itself — injected
	/// work drained ahead of a message, or a background inbox turn — so a
	/// client can tell its own request's end from theirs without counting.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub request_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusPayload {
	pub message: String,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub session_id: Option<String>,
	/// Optional structured data for command results
	#[serde(skip_serializing_if = "Option::is_none")]
	pub data: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorPayload {
	pub message: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub request_id: Option<String>,
	/// Machine-readable reason a client can act on; relays may rewrite `message`.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub code: Option<String>,
}

/// `code` of the error answering an `elicitation_response` whose question is
/// no longer pending (answered already, or its tool call ended).
pub const ELICITATION_GONE_ERROR: &str = "elicitation_gone";

/// Acknowledges a valid client frame. Sent at once for every frame but a
/// `session` bind, whose ack is sent once the session is bound — so it can say
/// where the session's output stands — or once the bind failed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AckPayload {
	/// Echo of the client-supplied request_id, when present.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub request_id: Option<String>,
	/// Client frame type being acknowledged: session, message, command, cancel or close.
	pub message_type: String,
	/// Session ID from the input frame when it is known.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub session_id: Option<String>,
	pub status: String,
	/// Protocol features supported for this bind.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub capabilities: Vec<String>,
	/// The bound session's output stream (successful binds only). Changes when
	/// the session is loaded anew — a restart, or a `close` and a later bind.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub stream: Option<String>,
	/// Highest `stream_seq` the stream has assigned, 0 if none (successful
	/// binds only).
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub stream_seq: Option<u64>,
	/// Whether the bind's `resume` is honoured: true means every frame after
	/// `resume.after` follows this ack. Present only when resume was requested.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub resumed: Option<bool>,
	/// Slash commands the session can run, in `/help` order. Present only when
	/// the bind listed [`COMMANDS_CAPABILITY`].
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub commands: Option<Vec<CommandInfo>>,
}

/// One entry of the ack's `commands`: a command a `command` frame can run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandInfo {
	/// Without the leading `/`, as a `command` frame carries it.
	pub name: String,
	pub description: String,
	/// Argument shape, e.g. `[provider:model]`; absent only when the command takes none.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub hint: Option<String>,
}

/// Skill lifecycle event (activate via auto-activation, explicit use, or forget).
/// Emitted for structured output modes (JSONL, WebSocket) so clients can track
/// which skills are currently shaping the session context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillPayload {
	/// Lifecycle action: "activate" (auto-activation), "use" (explicit), "forget".
	pub action: String,
	/// Skill name (e.g. "programming-rust").
	pub name: String,
	/// For `action = "activate"`: the matched rule that fired (e.g. "file(Cargo.toml)").
	/// Absent for explicit use/forget.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub trigger: Option<String>,
	pub session_id: String,
}

/// Grounded behavior-evolution lifecycle event. Separate from `Status`: status
/// messages carrying data are command completions in existing clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvolutionPayload {
	pub action: String,
	pub id: String,
	pub name: String,
	pub kind: String,
	pub state: String,
	pub scope: Value,
	pub session_id: String,
}

/// A message injected into the session by something other than the user
/// (a scheduled timer fired, a background agent completed, a skill activated, …).
/// Emitted just before the AI is invoked, so clients can render what the AI
/// is actually about to respond to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InjectedPayload {
	/// Machine-readable source kind:
	/// `schedule`, `background_agent`, `tap_run`, `skill`, `skill_validator`,
	/// `inject`, `webhook`.
	pub source_kind: String,
	/// Human-readable source label, e.g. `"schedule abc12345"`, `"agent reviewer"`.
	pub source_label: String,
	/// The content that's about to be added to the conversation as a user turn.
	pub content: String,
	pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpNotificationPayload {
	/// MCP server name that sent the notification
	pub server: String,
	/// JSON-RPC notification method (e.g. "notifications/message", "notifications/progress")
	pub method: String,
	/// Notification params as-is from the server
	pub params: Value,
	/// Tool call the notification belongs to, when the MCP progress token
	/// resolves to one. Lets clients attach progress to the right tool card.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub tool_id: Option<String>,
}

/// An MCP server asks the user for input mid-tool-call. `params` is MCP's own
/// `elicitation/create` request, unchanged (form `requestedSchema` or URL).
/// Sent only to a client that bound with [`ELICITATION_CAPABILITY`]; re-sent
/// after each bind while unanswered, so clients drop duplicates by `id`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElicitationPayload {
	pub session_id: String,
	/// Answer with an `elicitation_response` carrying this id.
	pub id: String,
	/// MCP server asking.
	pub server: String,
	/// Tool call the question belongs to, when known.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub tool_id: Option<String>,
	pub params: rmcp::model::ElicitRequestParams,
}

/// A model provider call failed with a retryable error (429, 5xx, a dropped
/// connection) and the agent is waiting before trying it again. Sent once per
/// retry, before the wait starts. Informational and mid-turn: it NEVER ends a
/// turn — the turn's own `cost` (or `error`) frame still does, whether a retry
/// succeeds or the last one fails. Clients show it until the next real frame.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RetryPayload {
	/// The retry about to run, 1-based (`1..=max_retries`).
	pub attempt: u32,
	/// Retries allowed for this call.
	pub max_retries: u32,
	/// Milliseconds the call waits before that retry.
	pub delay_ms: u64,
	/// Short cause of the failed attempt: `HTTP 503`, `connection error` or
	/// `request failed`. Never a response body or credential.
	pub reason: String,
	/// The same facts as one line, for clients that just print it.
	pub message: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub session_id: Option<String>,
}

/// Outgoing message from server to client.
/// Tagged by `"type"` — each variant carries only its own typed fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
	/// Immediate acknowledgement that a valid client input frame was received.
	Ack(AckPayload),
	RunEvent(RunEventPayload),
	RunLog(RunLogPayload),
	/// AI assistant response text
	Assistant(AssistantPayload),
	/// AI thinking/reasoning content (separate from assistant response)
	Thinking(ThinkingPayload),
	/// Tool execution notification (AI intends to use tool)
	ToolUse(ToolUsePayload),
	/// Tool execution result (after execution)
	ToolResult(ToolResultPayload),
	/// Cost and token usage information
	Cost(CostPayload),
	/// Status/info message (non-critical)
	Status(StatusPayload),
	/// Error message
	Error(ErrorPayload),
	/// Notification received from an MCP server (e.g. progress, log messages)
	McpNotification(McpNotificationPayload),
	/// Skill lifecycle event (activate / use / forget) — emitted for structured output
	Skill(SkillPayload),
	/// Generated behavior lifecycle event (candidate / trial / promote / rollback).
	Evolution(EvolutionPayload),
	/// A message injected into the session loop by a non-user source
	/// (scheduled timer, background agent, skill, …). Emitted just before
	/// the AI processes it so clients can render the trigger.
	Injected(InjectedPayload),
	/// A provider call is being retried after a transient failure. Mid-turn
	/// progress, never a turn boundary (see [`RetryPayload`]).
	Retry(RetryPayload),
	/// An MCP server asks the user for input (see [`ElicitationPayload`]).
	Elicitation(ElicitationPayload),
}

impl AckPayload {
	/// The ack of a `session` bind, before the server adds the stream position.
	pub fn bind(msg: &SessionMessage) -> Self {
		Self {
			capabilities: vec![
				MESSAGE_ATTACHMENTS_CAPABILITY.to_string(),
				SESSION_CONTROL_CAPABILITY.to_string(),
				RUN_EVENTS_CAPABILITY.to_string(),
				STREAM_RESUME_CAPABILITY.to_string(),
				ELICITATION_CAPABILITY.to_string(),
				COMMANDS_CAPABILITY.to_string(),
			],
			..Self::received(msg.request_id.clone(), "session", msg.session_id.clone())
		}
	}

	fn received(request_id: Option<String>, message_type: &str, session_id: Option<String>) -> Self {
		Self {
			request_id,
			message_type: message_type.to_string(),
			session_id,
			status: "received".to_string(),
			capabilities: Vec::new(),
			stream: None,
			stream_seq: None,
			resumed: None,
			commands: None,
		}
	}
}

impl ServerMessage {
	pub fn error(message: String) -> Self {
		ServerMessage::Error(ErrorPayload {
			message,
			request_id: None,
			code: None,
		})
	}

	pub fn error_for_request(message: String, request_id: Option<String>) -> Self {
		ServerMessage::Error(ErrorPayload {
			message,
			request_id,
			code: None,
		})
	}

	/// The answer to an `elicitation_response` for a question no longer pending.
	pub fn elicitation_gone(id: &str, request_id: Option<String>) -> Self {
		ServerMessage::Error(ErrorPayload {
			message: format!("no pending elicitation '{id}'"),
			request_id,
			code: Some(ELICITATION_GONE_ERROR.to_string()),
		})
	}

	pub fn ack(client_msg: &ClientMessage) -> Self {
		ServerMessage::Ack(match client_msg {
			ClientMessage::Session(bind) => AckPayload::bind(bind),
			other => AckPayload::received(
				other.request_id().map(ToOwned::to_owned),
				other.message_type(),
				other.session_id().map(ToOwned::to_owned),
			),
		})
	}

	pub fn status(message: String, session_id: Option<String>) -> Self {
		ServerMessage::Status(StatusPayload {
			message,
			session_id,
			data: None,
		})
	}

	/// A command-completion status. Unlike `status()` (which is also used for the
	/// connection handshake), this carries `data` — clients distinguish a finished
	/// command from a data-less handshake ack purely by the presence of `data`. Without
	/// it, an interactive `/done` (and any plain `Handled` command) is misread as the
	/// handshake and the client hangs on "working" because the turn never finalizes.
	pub fn command_status(message: String, session_id: Option<String>, data: Value) -> Self {
		ServerMessage::Status(StatusPayload {
			message,
			session_id,
			data: Some(data),
		})
	}

	pub fn skill(
		action: impl Into<String>,
		name: impl Into<String>,
		trigger: Option<String>,
		session_id: impl Into<String>,
	) -> Self {
		ServerMessage::Skill(SkillPayload {
			action: action.into(),
			name: name.into(),
			trigger,
			session_id: session_id.into(),
		})
	}

	pub fn evolution(
		action: impl Into<String>,
		id: impl Into<String>,
		name: impl Into<String>,
		kind: impl Into<String>,
		state: impl Into<String>,
		scope: Value,
		session_id: impl Into<String>,
	) -> Self {
		ServerMessage::Evolution(EvolutionPayload {
			action: action.into(),
			id: id.into(),
			name: name.into(),
			kind: kind.into(),
			state: state.into(),
			scope,
			session_id: session_id.into(),
		})
	}

	/// The notice for one provider retry (see [`RetryPayload`]).
	pub fn retry(event: &octolib::llm::RetryEvent, session_id: Option<String>) -> Self {
		let delay_secs = event.delay.as_secs_f64().round() as u64;
		ServerMessage::Retry(RetryPayload {
			attempt: event.attempt,
			max_retries: event.max_retries,
			delay_ms: event.delay.as_millis() as u64,
			reason: event.reason.clone(),
			message: format!(
				"Model provider unavailable ({}) — retrying in {}s, attempt {}/{}",
				event.reason, delay_secs, event.attempt, event.max_retries
			),
			session_id,
		})
	}
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[path = "protocol_tests.rs"]
mod tests;
