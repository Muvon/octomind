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

// ── Client → Server ──────────────────────────────────────────────────────────

/// Create or resume a session. No AI call is made.
/// Server responds with a `status` message containing the `session_id`.
///
/// - `session_id` absent  → create new auto-named session
/// - `session_id` present → resume if exists on disk, else create with that name
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMessage {
	/// Session name / ID. Absent = auto-named, present = create-or-resume.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub session_id: Option<String>,
}

/// Send user input to an existing session and receive an AI response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserMessage {
	/// Session name / ID — must refer to an established session.
	pub session_id: String,

	/// User input sent to the AI. Must be non-empty, max 10 MB.
	pub content: String,
}

/// Execute a session command (equivalent to `/command [args…]` in the CLI).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandMessage {
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
		match self {
			ClientMessage::Session(_) => Ok(()),

			ClientMessage::Message(m) => {
				if m.session_id.trim().is_empty() {
					return Err("session_id cannot be empty".to_string());
				}
				if m.content.trim().is_empty() {
					return Err("content cannot be empty".to_string());
				}
				if m.content.len() > 10 * 1024 * 1024 {
					return Err("content exceeds maximum size (10MB)".to_string());
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
}

// ── Server → Client ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantPayload {
	pub content: String,
	pub session_id: String,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpNotificationPayload {
	/// MCP server name that sent the notification
	pub server: String,
	/// JSON-RPC notification method (e.g. "notifications/message", "notifications/progress")
	pub method: String,
	/// Notification params as-is from the server
	pub params: Value,
}

/// Outgoing message from server to client.
/// Tagged by `"type"` — each variant carries only its own typed fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
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
}

impl ServerMessage {
	pub fn error(message: String) -> Self {
		ServerMessage::Error(ErrorPayload { message })
	}

	pub fn status(message: String, session_id: Option<String>) -> Self {
		ServerMessage::Status(StatusPayload {
			message,
			session_id,
			data: None,
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
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
	use super::*;

	// SessionMessage

	#[test]
	fn test_session_no_id_valid() {
		let json = r#"{"type":"session"}"#;
		let msg: ClientMessage = serde_json::from_str(json).unwrap();
		assert!(matches!(
			msg,
			ClientMessage::Session(SessionMessage { session_id: None })
		));
		assert!(msg.validate().is_ok());
	}

	#[test]
	fn test_session_with_id_valid() {
		let json = r#"{"type":"session","session_id":"my-feature-x"}"#;
		let msg: ClientMessage = serde_json::from_str(json).unwrap();
		assert!(matches!(
			msg,
			ClientMessage::Session(SessionMessage {
				session_id: Some(_)
			})
		));
		assert!(msg.validate().is_ok());
	}

	#[test]
	fn test_session_roundtrip() {
		let msg = ClientMessage::Session(SessionMessage {
			session_id: Some("my-session".to_string()),
		});
		let json = serde_json::to_string(&msg).unwrap();
		assert!(json.contains("\"type\":\"session\""));
		assert!(json.contains("my-session"));
	}

	// UserMessage

	#[test]
	fn test_message_valid() {
		let json = r#"{"type":"message","session_id":"sess_123","content":"Fix the bug"}"#;
		let msg: ClientMessage = serde_json::from_str(json).unwrap();
		assert!(msg.validate().is_ok());
	}

	#[test]
	fn test_message_missing_session_id_fails_deserialize() {
		// session_id is a required (non-Option) field — serde rejects it
		let json = r#"{"type":"message","content":"Fix the bug"}"#;
		assert!(serde_json::from_str::<ClientMessage>(json).is_err());
	}

	#[test]
	fn test_message_missing_content_fails_deserialize() {
		let json = r#"{"type":"message","session_id":"sess_123"}"#;
		assert!(serde_json::from_str::<ClientMessage>(json).is_err());
	}

	#[test]
	fn test_message_empty_session_id_fails_validate() {
		let msg = ClientMessage::Message(UserMessage {
			session_id: "  ".to_string(),
			content: "Fix the bug".to_string(),
		});
		assert!(msg.validate().is_err());
	}

	#[test]
	fn test_message_empty_content_fails_validate() {
		let msg = ClientMessage::Message(UserMessage {
			session_id: "sess_123".to_string(),
			content: "  ".to_string(),
		});
		assert!(msg.validate().is_err());
	}

	#[test]
	fn test_message_content_too_large_fails_validate() {
		let msg = ClientMessage::Message(UserMessage {
			session_id: "sess_123".to_string(),
			content: "x".repeat(11 * 1024 * 1024),
		});
		assert!(msg.validate().is_err());
	}

	#[test]
	fn test_message_roundtrip() {
		let msg = ClientMessage::Message(UserMessage {
			session_id: "sess_123".to_string(),
			content: "Hello".to_string(),
		});
		let json = serde_json::to_string(&msg).unwrap();
		assert!(json.contains("\"type\":\"message\""));
		assert!(json.contains("sess_123"));
		assert!(json.contains("Hello"));
	}

	// CommandMessage

	#[test]
	fn test_command_valid_no_args() {
		let json = r#"{"type":"command","session_id":"sess_123","command":"info"}"#;
		let msg: ClientMessage = serde_json::from_str(json).unwrap();
		assert!(msg.validate().is_ok());
		if let ClientMessage::Command(c) = msg {
			assert!(c.args.is_empty());
		}
	}

	#[test]
	fn test_command_valid_with_args() {
		let json = r#"{"type":"command","session_id":"sess_123","command":"mcp","args":["list"]}"#;
		let msg: ClientMessage = serde_json::from_str(json).unwrap();
		assert!(msg.validate().is_ok());
		if let ClientMessage::Command(c) = msg {
			assert_eq!(c.args, vec!["list"]);
		}
	}

	#[test]
	fn test_command_missing_session_id_fails_deserialize() {
		let json = r#"{"type":"command","command":"info"}"#;
		assert!(serde_json::from_str::<ClientMessage>(json).is_err());
	}

	#[test]
	fn test_command_missing_command_fails_deserialize() {
		let json = r#"{"type":"command","session_id":"sess_123"}"#;
		assert!(serde_json::from_str::<ClientMessage>(json).is_err());
	}

	#[test]
	fn test_command_empty_session_id_fails_validate() {
		let msg = ClientMessage::Command(CommandMessage {
			session_id: "  ".to_string(),
			command: "info".to_string(),
			args: vec![],
		});
		assert!(msg.validate().is_err());
	}

	#[test]
	fn test_command_empty_command_fails_validate() {
		let msg = ClientMessage::Command(CommandMessage {
			session_id: "sess_123".to_string(),
			command: "  ".to_string(),
			args: vec![],
		});
		assert!(msg.validate().is_err());
	}

	#[test]
	fn test_command_roundtrip() {
		let msg = ClientMessage::Command(CommandMessage {
			session_id: "sess_123".to_string(),
			command: "model".to_string(),
			args: vec!["openrouter:anthropic/claude-sonnet-4".to_string()],
		});
		let json = serde_json::to_string(&msg).unwrap();
		assert!(json.contains("\"type\":\"command\""));
		assert!(json.contains("\"command\":\"model\""));
		assert!(json.contains("sess_123"));
		// args omitted when empty, present when not
		assert!(json.contains("args"));
	}

	#[test]
	fn test_command_args_omitted_when_empty() {
		let msg = ClientMessage::Command(CommandMessage {
			session_id: "sess_123".to_string(),
			command: "info".to_string(),
			args: vec![],
		});
		let json = serde_json::to_string(&msg).unwrap();
		assert!(!json.contains("\"args\""));
	}

	// ServerMessage

	#[test]
	fn test_server_message_assistant_serialization() {
		let msg = ServerMessage::Assistant(AssistantPayload {
			content: "Response".to_string(),
			session_id: "sess_123".to_string(),
		});
		let json = serde_json::to_string(&msg).unwrap();
		assert!(json.contains("\"type\":\"assistant\""));
		assert!(json.contains("Response"));
		assert!(json.contains("sess_123"));
	}

	#[test]
	fn test_server_message_error_serialization() {
		let msg = ServerMessage::error("something went wrong".to_string());
		let json = serde_json::to_string(&msg).unwrap();
		assert!(json.contains("\"type\":\"error\""));
		assert!(json.contains("something went wrong"));
	}

	#[test]
	fn test_server_message_status_serialization() {
		let msg =
			ServerMessage::status("Session created: foo".to_string(), Some("foo".to_string()));
		let json = serde_json::to_string(&msg).unwrap();
		assert!(json.contains("\"type\":\"status\""));
		assert!(json.contains("Session created: foo"));
		assert!(json.contains("\"session_id\":\"foo\""));
	}

	#[test]
	fn test_server_message_status_no_session_id() {
		let msg = ServerMessage::status("Connected".to_string(), None);
		let json = serde_json::to_string(&msg).unwrap();
		assert!(json.contains("\"type\":\"status\""));
		assert!(!json.contains("session_id"));
	}

	#[test]
	fn test_server_message_tool_use_serialization() {
		let msg = ServerMessage::ToolUse(ToolUsePayload {
			tool: "list_files".to_string(),
			tool_id: "call_abc".to_string(),
			server: "filesystem".to_string(),
			params: serde_json::json!({"directory": "src"}),
			session_id: "sess_123".to_string(),
		});
		let json = serde_json::to_string(&msg).unwrap();
		assert!(json.contains("\"type\":\"tool_use\""));
		assert!(json.contains("\"tool\":\"list_files\""));
		assert!(json.contains("\"server\":\"filesystem\""));
	}

	#[test]
	fn test_server_message_tool_result_serialization() {
		let msg = ServerMessage::ToolResult(ToolResultPayload {
			tool: "list_files".to_string(),
			tool_id: "call_abc".to_string(),
			server: "filesystem".to_string(),
			content: "src/main.rs\nsrc/lib.rs".to_string(),
			success: true,
			session_id: "sess_123".to_string(),
		});
		let json = serde_json::to_string(&msg).unwrap();
		assert!(json.contains("\"type\":\"tool_result\""));
		assert!(json.contains("\"success\":true"));
	}

	#[test]
	fn test_server_message_cost_serialization() {
		let msg = ServerMessage::Cost(CostPayload {
			session_tokens: 1234,
			session_cost: 0.0025,
			input_tokens: 1000,
			output_tokens: 200,
			cache_read_tokens: 30,
			cache_write_tokens: 4,
			reasoning_tokens: 0,
			session_id: "sess_123".to_string(),
		});
		let json = serde_json::to_string(&msg).unwrap();
		assert!(json.contains("\"type\":\"cost\""));
		assert!(json.contains("\"session_tokens\":1234"));
	}

	#[test]
	fn test_server_message_skill_serialization_with_trigger() {
		let msg = ServerMessage::skill(
			"activate",
			"programming-rust",
			Some("file(Cargo.toml)".to_string()),
			"sess_123",
		);
		let json = serde_json::to_string(&msg).unwrap();
		assert!(json.contains("\"type\":\"skill\""));
		assert!(json.contains("\"action\":\"activate\""));
		assert!(json.contains("\"name\":\"programming-rust\""));
		assert!(json.contains("\"trigger\":\"file(Cargo.toml)\""));
	}

	#[test]
	fn test_server_message_skill_serialization_without_trigger() {
		let msg = ServerMessage::skill("forget", "programming-rust", None, "sess_123");
		let json = serde_json::to_string(&msg).unwrap();
		assert!(json.contains("\"type\":\"skill\""));
		assert!(json.contains("\"action\":\"forget\""));
		assert!(!json.contains("trigger"));
	}

	#[test]
	fn test_unknown_type_fails_deserialize() {
		let json = r#"{"type":"unknown","session_id":"sess_123"}"#;
		assert!(serde_json::from_str::<ClientMessage>(json).is_err());
	}
}
