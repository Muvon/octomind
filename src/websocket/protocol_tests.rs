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

use super::*;

// SessionMessage

#[test]
fn test_session_no_id_valid() {
	let json = r#"{"type":"session"}"#;
	let msg: ClientMessage = serde_json::from_str(json).unwrap();
	assert!(matches!(
		msg,
		ClientMessage::Session(SessionMessage {
			session_id: None,
			..
		})
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
			session_id: Some(_),
			..
		})
	));
	assert!(msg.validate().is_ok());
}

#[test]
fn test_session_roundtrip() {
	let msg = ClientMessage::Session(SessionMessage {
		request_id: None,
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
	let ClientMessage::Message(message) = msg else {
		panic!("expected message frame");
	};
	assert!(message.attachments.is_empty());
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
		request_id: None,
		session_id: "  ".to_string(),
		content: "Fix the bug".to_string(),
		attachments: Vec::new(),
	});
	assert!(msg.validate().is_err());
}

#[test]
fn test_message_empty_content_fails_validate() {
	let msg = ClientMessage::Message(UserMessage {
		request_id: None,
		session_id: "sess_123".to_string(),
		content: "  ".to_string(),
		attachments: Vec::new(),
	});
	assert!(msg.validate().is_err());
}

#[test]
fn test_message_content_too_large_fails_validate() {
	let msg = ClientMessage::Message(UserMessage {
		request_id: None,
		session_id: "sess_123".to_string(),
		content: "x".repeat(11 * 1024 * 1024),
		attachments: Vec::new(),
	});
	assert!(msg.validate().is_err());
}

#[test]
fn test_message_roundtrip() {
	let msg = ClientMessage::Message(UserMessage {
		request_id: None,
		session_id: "sess_123".to_string(),
		content: "Hello".to_string(),
		attachments: Vec::new(),
	});
	let json = serde_json::to_string(&msg).unwrap();
	assert_eq!(
		json,
		r#"{"type":"message","session_id":"sess_123","content":"Hello"}"#
	);
}

#[test]
fn test_message_with_attachment_and_empty_content_is_valid() {
	let json = r#"{"type":"message","session_id":"sess_123","content":"","attachments":[{"id":"AbCdEf0123456789GhIjKlMn","kind":"image","media_type":"image/png","name":"screenshot.png","size":1234}]}"#;
	let msg: ClientMessage = serde_json::from_str(json).unwrap();
	assert!(msg.validate().is_ok());
}

#[test]
fn test_unsafe_attachment_ids_are_rejected() {
	for id in [
		"../etc/passwd",
		"/absolute/path/to/file",
		"short",
		"AbCdEf0123456789GhIjKlM/",
	] {
		let msg = ClientMessage::Message(UserMessage {
			request_id: None,
			session_id: "sess_123".to_string(),
			content: String::new(),
			attachments: vec![Attachment {
				id: id.to_string(),
				kind: AttachmentKind::Image,
				media_type: "image/png".to_string(),
				name: "screenshot.png".to_string(),
				size: 1234,
			}],
		});
		assert!(msg.validate().is_err(), "unsafe id was accepted: {id}");
	}
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
		request_id: None,
		session_id: "  ".to_string(),
		command: "info".to_string(),
		args: vec![],
	});
	assert!(msg.validate().is_err());
}

#[test]
fn test_command_empty_command_fails_validate() {
	let msg = ClientMessage::Command(CommandMessage {
		request_id: None,
		session_id: "sess_123".to_string(),
		command: "  ".to_string(),
		args: vec![],
	});
	assert!(msg.validate().is_err());
}

#[test]
fn test_command_roundtrip() {
	let msg = ClientMessage::Command(CommandMessage {
		request_id: None,
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
		request_id: None,
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
		step: None,
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
fn test_request_id_validation_and_ack_serialization() {
	let msg: ClientMessage = serde_json::from_str(
		r#"{"type":"message","request_id":"req-1","session_id":"sess_123","content":"Hello"}"#,
	)
	.unwrap();
	assert!(msg.validate().is_ok());
	assert_eq!(msg.request_id(), Some("req-1"));
	assert_eq!(msg.message_type(), "message");
	assert_eq!(msg.session_id(), Some("sess_123"));

	let ack = ServerMessage::ack(&msg);
	let json = serde_json::to_string(&ack).unwrap();
	assert!(json.contains("\"type\":\"ack\""));
	assert!(json.contains("\"request_id\":\"req-1\""));
	assert!(json.contains("\"message_type\":\"message\""));
	assert!(json.contains("\"session_id\":\"sess_123\""));
	assert!(json.contains("\"status\":\"received\""));
	assert!(!json.contains("capabilities"));
}

#[test]
fn test_session_ack_advertises_message_attachments() {
	let msg: ClientMessage =
		serde_json::from_str(r#"{"type":"session","request_id":"bind-1","session_id":"sess_123"}"#)
			.unwrap();
	let ack = ServerMessage::ack(&msg);
	let json = serde_json::to_string(&ack).unwrap();
	assert!(json.contains("\"capabilities\":[\"message_attachments_v1\"]"));
}

#[test]
fn test_empty_request_id_fails_validate() {
	let msg = ClientMessage::Command(CommandMessage {
		request_id: Some(" ".to_string()),
		session_id: "sess_123".to_string(),
		command: "info".to_string(),
		args: vec![],
	});
	assert!(msg.validate().is_err());
}

#[test]
fn test_error_for_request_serialization() {
	let msg = ServerMessage::error_for_request(
		"content cannot be empty".to_string(),
		Some("req-2".to_string()),
	);
	let json = serde_json::to_string(&msg).unwrap();
	assert!(json.contains("\"type\":\"error\""));
	assert!(json.contains("\"request_id\":\"req-2\""));
}

#[test]
fn test_server_message_status_serialization() {
	let msg = ServerMessage::status("Session created: foo".to_string(), Some("foo".to_string()));
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
fn test_server_message_evolution_is_not_a_command_status() {
	let msg = ServerMessage::evolution(
		"promoted",
		"evo-1",
		"evolved-rust",
		"skill",
		"active",
		serde_json::json!({"project":"octomind","domain":"developer"}),
		"sess_123",
	);
	let value = serde_json::to_value(msg).unwrap();
	assert_eq!(value["type"], "evolution");
	assert_eq!(value["action"], "promoted");
	assert!(value.get("data").is_none());
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

// ---- request_id size boundary ----

#[test]
fn request_id_longer_than_256_bytes_is_rejected() {
	let long_id = "x".repeat(257);
	let json = format!(
		r#"{{"type":"command","session_id":"s","command":"info","request_id":"{long_id}"}}"#
	);
	let msg: ClientMessage = serde_json::from_str(&json).expect("parse");
	let error = msg
		.validate()
		.expect_err("257 bytes exceeds the documented maximum");
	assert!(error.contains("256"), "got: {error}");

	let exact = "x".repeat(256);
	let json =
		format!(r#"{{"type":"command","session_id":"s","command":"info","request_id":"{exact}"}}"#);
	let msg: ClientMessage = serde_json::from_str(&json).expect("parse");
	assert!(
		msg.validate().is_ok(),
		"exactly 256 bytes is the documented maximum"
	);
}

// ---- command_status ----

#[test]
fn command_status_carries_data_so_clients_finalize_the_turn() {
	let message = ServerMessage::command_status(
		"Nothing to compress".to_string(),
		Some("s1".to_string()),
		serde_json::json!({"command_type": "done", "message": "Nothing to compress"}),
	);
	let value = serde_json::to_value(&message).expect("serialize");
	assert_eq!(value["type"], "status");
	assert_eq!(value["message"], "Nothing to compress");
	assert_eq!(value["session_id"], "s1");
	assert_eq!(
		value["data"]["command_type"], "done",
		"a data-carrying status is what distinguishes a finished command from the handshake ack"
	);
}
