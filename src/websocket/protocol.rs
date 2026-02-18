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

/// Type of message sent from client to server
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClientMessageType {
	/// Create a new session or resume an existing one by session_id.
	/// No AI call is made. Server responds with session_id.
	Session,

	/// Send a user message to an existing session. Requires session_id and content.
	Message,

	/// Execute a session command (equivalent to /command in CLI).
	/// Requires session_id and command. Args are optional.
	Command,
}

/// Message from client to server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientMessage {
	/// Message type - required on every message
	#[serde(rename = "type")]
	pub message_type: ClientMessageType,

	/// Session name / ID.
	/// - For "session" type: absent = create auto-named, present = create-or-resume
	/// - For "message" and "command" types: required
	#[serde(skip_serializing_if = "Option::is_none")]
	pub session_id: Option<String>,

	/// Message content. Required for "message" type, ignored for others.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub content: Option<String>,

	/// Command name (without leading slash). Required for "command" type.
	/// Examples: "info", "model", "mcp", "help", "role"
	#[serde(skip_serializing_if = "Option::is_none")]
	pub command: Option<String>,

	/// Command arguments. Optional for "command" type.
	/// Examples: ["list"] for /mcp list, ["openrouter:claude-sonnet-4"] for /model
	#[serde(skip_serializing_if = "Option::is_none")]
	pub args: Option<Vec<String>>,
}

/// Message from server to client
/// Typed messages for different kinds of output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerMessage {
	/// Unique message ID (server-generated)
	pub id: String,

	/// Message type determines how to interpret content
	#[serde(rename = "type")]
	pub message_type: MessageType,

	/// The actual content (format depends on type)
	pub content: String,

	/// Optional metadata (structured data)
	#[serde(skip_serializing_if = "Option::is_none")]
	pub meta: Option<Value>,

	/// Session ID (always present after session is established)
	#[serde(skip_serializing_if = "Option::is_none")]
	pub session_id: Option<String>,
}

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

impl ClientMessage {
	/// Validate client message fields based on type
	pub fn validate(&self) -> Result<(), String> {
		match self.message_type {
			ClientMessageType::Session => {
				// session_id is optional (absent = auto-name, present = create-or-resume)
				Ok(())
			}
			ClientMessageType::Message => {
				// session_id is required
				match &self.session_id {
					None => return Err("session_id is required for message type".to_string()),
					Some(id) if id.trim().is_empty() => {
						return Err("session_id cannot be empty".to_string())
					}
					_ => {}
				}

				// content is required and must not be empty
				match &self.content {
					None => return Err("content is required for message type".to_string()),
					Some(c) if c.trim().is_empty() => {
						return Err("content cannot be empty".to_string())
					}
					Some(c) if c.len() > 10 * 1024 * 1024 => {
						return Err("content exceeds maximum size (10MB)".to_string())
					}
					_ => {}
				}

				Ok(())
			}
			ClientMessageType::Command => {
				// session_id is required
				match &self.session_id {
					None => return Err("session_id is required for command type".to_string()),
					Some(id) if id.trim().is_empty() => {
						return Err("session_id cannot be empty".to_string())
					}
					_ => {}
				}

				// command name is required and must not be empty
				match &self.command {
					None => return Err("command is required for command type".to_string()),
					Some(c) if c.trim().is_empty() => {
						return Err("command cannot be empty".to_string())
					}
					_ => {}
				}

				Ok(())
			}
		}
	}
}

impl ServerMessage {
	/// Create a new server message
	pub fn new(message_type: MessageType, content: String, session_id: Option<String>) -> Self {
		Self {
			id: uuid::Uuid::new_v4().to_string(),
			message_type,
			content,
			meta: None,
			session_id,
		}
	}

	/// Create a new server message with metadata
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

	/// Create an error message
	pub fn error(message: String) -> Self {
		Self::new(MessageType::Error, message, None)
	}

	/// Create a status message
	pub fn status(message: String, session_id: Option<String>) -> Self {
		Self::new(MessageType::Status, message, session_id)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_session_message_no_session_id() {
		let msg = ClientMessage {
			message_type: ClientMessageType::Session,
			session_id: None,
			content: None,
			command: None,
			args: None,
		};
		assert!(msg.validate().is_ok());
	}

	#[test]
	fn test_session_message_with_session_id() {
		let msg = ClientMessage {
			message_type: ClientMessageType::Session,
			session_id: Some("my-feature-x".to_string()),
			content: None,
			command: None,
			args: None,
		};
		assert!(msg.validate().is_ok());
	}

	#[test]
	fn test_session_message_content_ignored() {
		// content is allowed but ignored for session type
		let msg = ClientMessage {
			message_type: ClientMessageType::Session,
			session_id: None,
			content: Some("ignored".to_string()),
			command: None,
			args: None,
		};
		assert!(msg.validate().is_ok());
	}

	#[test]
	fn test_message_type_valid() {
		let msg = ClientMessage {
			message_type: ClientMessageType::Message,
			session_id: Some("sess_123".to_string()),
			content: Some("Fix the bug".to_string()),
			command: None,
			args: None,
		};
		assert!(msg.validate().is_ok());
	}

	#[test]
	fn test_message_type_missing_session_id() {
		let msg = ClientMessage {
			message_type: ClientMessageType::Message,
			session_id: None,
			content: Some("Fix the bug".to_string()),
			command: None,
			args: None,
		};
		assert!(msg.validate().is_err());
	}

	#[test]
	fn test_message_type_empty_session_id() {
		let msg = ClientMessage {
			message_type: ClientMessageType::Message,
			session_id: Some("  ".to_string()),
			content: Some("Fix the bug".to_string()),
			command: None,
			args: None,
		};
		assert!(msg.validate().is_err());
	}

	#[test]
	fn test_message_type_missing_content() {
		let msg = ClientMessage {
			message_type: ClientMessageType::Message,
			session_id: Some("sess_123".to_string()),
			content: None,
			command: None,
			args: None,
		};
		assert!(msg.validate().is_err());
	}

	#[test]
	fn test_message_type_empty_content() {
		let msg = ClientMessage {
			message_type: ClientMessageType::Message,
			session_id: Some("sess_123".to_string()),
			content: Some("  ".to_string()),
			command: None,
			args: None,
		};
		assert!(msg.validate().is_err());
	}

	#[test]
	fn test_message_type_content_too_large() {
		let msg = ClientMessage {
			message_type: ClientMessageType::Message,
			session_id: Some("sess_123".to_string()),
			content: Some("x".repeat(11 * 1024 * 1024)),
			command: None,
			args: None,
		};
		assert!(msg.validate().is_err());
	}

	#[test]
	fn test_command_type_valid() {
		let msg = ClientMessage {
			message_type: ClientMessageType::Command,
			session_id: Some("sess_123".to_string()),
			content: None,
			command: Some("info".to_string()),
			args: None,
		};
		assert!(msg.validate().is_ok());
	}

	#[test]
	fn test_command_type_with_args() {
		let msg = ClientMessage {
			message_type: ClientMessageType::Command,
			session_id: Some("sess_123".to_string()),
			content: None,
			command: Some("mcp".to_string()),
			args: Some(vec!["list".to_string()]),
		};
		assert!(msg.validate().is_ok());
	}

	#[test]
	fn test_command_type_missing_session_id() {
		let msg = ClientMessage {
			message_type: ClientMessageType::Command,
			session_id: None,
			content: None,
			command: Some("info".to_string()),
			args: None,
		};
		assert!(msg.validate().is_err());
	}

	#[test]
	fn test_command_type_missing_command() {
		let msg = ClientMessage {
			message_type: ClientMessageType::Command,
			session_id: Some("sess_123".to_string()),
			content: None,
			command: None,
			args: None,
		};
		assert!(msg.validate().is_err());
	}

	#[test]
	fn test_command_type_empty_command() {
		let msg = ClientMessage {
			message_type: ClientMessageType::Command,
			session_id: Some("sess_123".to_string()),
			content: None,
			command: Some("  ".to_string()),
			args: None,
		};
		assert!(msg.validate().is_err());
	}

	#[test]
	fn test_serialization_command_type() {
		let msg = ClientMessage {
			message_type: ClientMessageType::Command,
			session_id: Some("sess_123".to_string()),
			content: None,
			command: Some("model".to_string()),
			args: Some(vec!["openrouter:anthropic/claude-sonnet-4".to_string()]),
		};
		let json = serde_json::to_string(&msg).unwrap();
		assert!(json.contains("\"type\":\"command\""));
		assert!(json.contains("model"));
		assert!(json.contains("sess_123"));
		assert!(!json.contains("\"content\"")); // skipped when None
	}

	#[test]
	fn test_deserialization_command() {
		let json = r#"{"type":"command","session_id":"sess_123","command":"mcp","args":["list"]}"#;
		let msg: ClientMessage = serde_json::from_str(json).unwrap();
		assert_eq!(msg.message_type, ClientMessageType::Command);
		assert_eq!(msg.session_id, Some("sess_123".to_string()));
		assert_eq!(msg.command, Some("mcp".to_string()));
		assert_eq!(msg.args, Some(vec!["list".to_string()]));
	}

	#[test]
	fn test_serialization_session_type() {
		let msg = ClientMessage {
			message_type: ClientMessageType::Session,
			session_id: Some("my-session".to_string()),
			content: None,
			command: None,
			args: None,
		};
		let json = serde_json::to_string(&msg).unwrap();
		assert!(json.contains("\"type\":\"session\""));
		assert!(json.contains("my-session"));
		assert!(!json.contains("\"content\"")); // skipped when None
	}

	#[test]
	fn test_serialization_message_type() {
		let msg = ClientMessage {
			message_type: ClientMessageType::Message,
			session_id: Some("sess_123".to_string()),
			content: Some("Hello".to_string()),
			command: None,
			args: None,
		};
		let json = serde_json::to_string(&msg).unwrap();
		assert!(json.contains("\"type\":\"message\""));
		assert!(json.contains("sess_123"));
		assert!(json.contains("Hello"));
	}

	#[test]
	fn test_deserialization_session() {
		let json = r#"{"type":"session","session_id":"my-feature-x"}"#;
		let msg: ClientMessage = serde_json::from_str(json).unwrap();
		assert_eq!(msg.message_type, ClientMessageType::Session);
		assert_eq!(msg.session_id, Some("my-feature-x".to_string()));
		assert!(msg.content.is_none());
	}

	#[test]
	fn test_deserialization_message() {
		let json = r#"{"type":"message","session_id":"sess_123","content":"Fix the bug"}"#;
		let msg: ClientMessage = serde_json::from_str(json).unwrap();
		assert_eq!(msg.message_type, ClientMessageType::Message);
		assert_eq!(msg.session_id, Some("sess_123".to_string()));
		assert_eq!(msg.content, Some("Fix the bug".to_string()));
	}

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
	fn test_message_type_serialization() {
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
}
