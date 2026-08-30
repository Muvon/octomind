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
			let entry = entry.map_err(|error| {
				format!("attachment '{}' could not be located: {}", self.id, error)
			})?;
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

/// Incoming message from client to server.
/// Internally tagged by `"type"` so each variant carries only its own fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
	Session(SessionMessage),
	Message(UserMessage),
	Command(CommandMessage),
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
		}

		match self {
			ClientMessage::Session(_) => Ok(()),

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
		}
	}

	pub fn request_id(&self) -> Option<&str> {
		match self {
			ClientMessage::Session(m) => m.request_id.as_deref(),
			ClientMessage::Message(m) => m.request_id.as_deref(),
			ClientMessage::Command(c) => c.request_id.as_deref(),
		}
	}

	pub fn message_type(&self) -> &'static str {
		match self {
			ClientMessage::Session(_) => "session",
			ClientMessage::Message(_) => "message",
			ClientMessage::Command(_) => "command",
		}
	}

	pub fn session_id(&self) -> Option<&str> {
		match self {
			ClientMessage::Session(m) => m.session_id.as_deref(),
			ClientMessage::Message(m) => Some(m.session_id.as_str()),
			ClientMessage::Command(c) => Some(c.session_id.as_str()),
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
	pub session_id: String,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AckPayload {
	/// Echo of the client-supplied request_id, when present.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub request_id: Option<String>,
	/// Client frame type being acknowledged: session, message, or command.
	pub message_type: String,
	/// Session ID from the input frame when it is known.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub session_id: Option<String>,
	pub status: String,
	/// Protocol features supported for this bind.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub capabilities: Vec<String>,
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

/// Outgoing message from server to client.
/// Tagged by `"type"` — each variant carries only its own typed fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
	/// Immediate acknowledgement that a valid client input frame was received.
	Ack(AckPayload),
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
}

impl ServerMessage {
	pub fn error(message: String) -> Self {
		ServerMessage::Error(ErrorPayload {
			message,
			request_id: None,
		})
	}

	pub fn error_for_request(message: String, request_id: Option<String>) -> Self {
		ServerMessage::Error(ErrorPayload {
			message,
			request_id,
		})
	}

	pub fn ack(client_msg: &ClientMessage) -> Self {
		ServerMessage::Ack(AckPayload {
			request_id: client_msg.request_id().map(ToOwned::to_owned),
			message_type: client_msg.message_type().to_string(),
			session_id: client_msg.session_id().map(ToOwned::to_owned),
			status: "received".to_string(),
			capabilities: match client_msg {
				ClientMessage::Session(_) => vec![MESSAGE_ATTACHMENTS_CAPABILITY.to_string()],
				_ => Vec::new(),
			},
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
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[path = "protocol_tests.rs"]
mod tests;
