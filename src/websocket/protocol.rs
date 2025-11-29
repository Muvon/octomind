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

/// Message from client to server
/// Simple text-based input, just like terminal
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientMessage {
	/// The actual input text (user message, command, anything)
	/// Examples: "Fix the bug", "/help", "/run analyze"
	pub content: String,

	/// Optional session ID for resuming existing sessions
	/// If None, creates new session. If Some, resumes existing.
	/// Since communication is sequential within a session, session_id is sufficient for correlation.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub session_id: Option<String>,
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

	/// Session ID (always present after first message)
	/// Since communication is sequential within a session, session_id is sufficient for correlation.
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
}

impl ClientMessage {
	/// Validate client message
	pub fn validate(&self) -> Result<(), String> {
		// Content must not be empty
		if self.content.trim().is_empty() {
			return Err("Message content cannot be empty".to_string());
		}

		// Content size limit (10MB default)
		if self.content.len() > 10 * 1024 * 1024 {
			return Err("Message content exceeds maximum size (10MB)".to_string());
		}

		Ok(())
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

	/// Validate server message
	pub fn validate(&self) -> Result<(), String> {
		if self.id.trim().is_empty() {
			return Err("Message ID cannot be empty".to_string());
		}

		if self.content.len() > 10 * 1024 * 1024 {
			return Err("Message content exceeds maximum size (10MB)".to_string());
		}

		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;

	#[test]
	fn test_client_message_serialization() {
		let msg = ClientMessage {
			content: "Hello".to_string(),
			session_id: None,
		};

		let json = serde_json::to_string(&msg).unwrap();
		assert!(json.contains("Hello"));
		assert!(!json.contains("session_id")); // Should be omitted when None
	}

	#[test]
	fn test_client_message_deserialization() {
		let json = r#"{"content":"Hello","session_id":"sess_123"}"#;
		let msg: ClientMessage = serde_json::from_str(json).unwrap();

		assert_eq!(msg.content, "Hello");
		assert_eq!(msg.session_id, Some("sess_123".to_string()));
	}

	#[test]
	fn test_client_message_validation() {
		// Valid message
		let msg = ClientMessage {
			content: "Hello".to_string(),
			session_id: None,
		};
		assert!(msg.validate().is_ok());

		// Empty content
		let msg = ClientMessage {
			content: "".to_string(),
			session_id: None,
		};
		assert!(msg.validate().is_err());

		// Content too large
		let msg = ClientMessage {
			content: "x".repeat(11 * 1024 * 1024),
			session_id: None,
		};
		assert!(msg.validate().is_err());
	}

	#[test]
	fn test_server_message_serialization() {
		let msg = ServerMessage::new(
			MessageType::Assistant,
			"Response".to_string(),
			Some("sess_123".to_string()),
		);

		let json = serde_json::to_string(&msg).unwrap();
		assert!(json.contains("assistant"));
		assert!(json.contains("Response"));
		assert!(json.contains("sess_123"));
	}

	#[test]
	fn test_server_message_with_metadata() {
		let meta = json!({
			"tokens": 100,
			"cost": 0.002
		});

		let msg = ServerMessage::with_metadata(
			MessageType::Cost,
			"Cost info".to_string(),
			meta,
			Some("sess_123".to_string()),
		);

		assert_eq!(msg.message_type, MessageType::Cost);
		assert!(msg.meta.is_some());
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

	#[test]
	fn test_round_trip_serialization() {
		let original = ClientMessage {
			content: "Test message".to_string(),
			session_id: Some("sess_123".to_string()),
		};

		let json = serde_json::to_string(&original).unwrap();
		let deserialized: ClientMessage = serde_json::from_str(&json).unwrap();

		assert_eq!(original.content, deserialized.content);
		assert_eq!(original.session_id, deserialized.session_id);
	}
}
