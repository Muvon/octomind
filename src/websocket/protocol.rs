// Copyright 2025 Muvon Un Limited
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

/// Types of messages the server can send
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MessageType {
	/// AI assistant response text
	Assistant,

	/// Tool execution notification (AI intends to use tool)
	ToolUse,

	/// Tool execution result (after execution)
	ToolResult,

	/// Cost and token usage information
	Cost,

	/// Error message
	Error,

	/// Status/info message (non-critical)
	Status,

	/// AI thinking/reasoning content (separate from assistant response)
	Thinking,
}

/// Outgoing message from server to client
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerMessage {
	/// Unique message ID (server-generated UUID)
	pub id: String,

	/// Message type determines how to interpret `content`
	#[serde(rename = "type")]
	pub message_type: MessageType,

	/// The actual content (format depends on type)
	pub content: String,

	/// Optional structured metadata (varies by type)
	#[serde(skip_serializing_if = "Option::is_none")]
	pub meta: Option<Value>,

	/// Session ID — always present after a session is established
	#[serde(skip_serializing_if = "Option::is_none")]
	pub session_id: Option<String>,
}

impl ServerMessage {
	pub fn new(message_type: MessageType, content: String, session_id: Option<String>) -> Self {
		Self {
			id: uuid::Uuid::new_v4().to_string(),
			message_type,
			content,
			meta: None,
			session_id,
		}
	}

	pub fn with_metadata(
		message_type: MessageType,
		content: String,
		meta: Value,
		session_id: Option<String>,
	) -> Self {
		Self {
			id: uuid::Uuid::new_v4().to_string(),
			message_type,
			content,
			meta: Some(meta),
			session_id,
		}
	}

	pub fn error(message: String) -> Self {
		Self::new(MessageType::Error, message, None)
	}

	pub fn status(message: String, session_id: Option<String>) -> Self {
		Self::new(MessageType::Status, message, session_id)
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
	fn test_server_message_serialization() {
		let msg = ServerMessage::new(
			MessageType::Assistant,
			"Response".to_string(),
			Some("sess_123".to_string()),
		);
		let json = serde_json::to_string(&msg).unwrap();
		assert!(json.contains("\"type\":\"assistant\""));
		assert!(json.contains("Response"));
		assert!(json.contains("sess_123"));
	}

	#[test]
	fn test_server_message_type_serialization() {
		assert_eq!(
			serde_json::to_string(&MessageType::Assistant).unwrap(),
			"\"assistant\""
		);
		assert_eq!(
			serde_json::to_string(&MessageType::ToolUse).unwrap(),
			"\"tool_use\""
		);
		assert_eq!(
			serde_json::to_string(&MessageType::Error).unwrap(),
			"\"error\""
		);
	}

	#[test]
	fn test_unknown_type_fails_deserialize() {
		let json = r#"{"type":"unknown","session_id":"sess_123"}"#;
		assert!(serde_json::from_str::<ClientMessage>(json).is_err());
	}
}
