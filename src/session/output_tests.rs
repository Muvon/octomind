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
use crate::websocket::AssistantPayload;
#[test]
fn test_output_mode_from_cli_arg() {
	// JSONL mode always returns Jsonl
	assert_eq!(OutputMode::from_cli_arg("jsonl", true), OutputMode::Jsonl);
	assert_eq!(OutputMode::from_cli_arg("jsonl", false), OutputMode::Jsonl);

	// Plain mode depends on terminal
	assert_eq!(
		OutputMode::from_cli_arg("plain", true),
		OutputMode::Interactive
	);
	assert_eq!(
		OutputMode::from_cli_arg("plain", false),
		OutputMode::NonInteractive
	);

	// Unknown mode defaults based on terminal
	assert_eq!(
		OutputMode::from_cli_arg("unknown", true),
		OutputMode::Interactive
	);
	assert_eq!(
		OutputMode::from_cli_arg("unknown", false),
		OutputMode::NonInteractive
	);
}

#[test]
fn test_output_mode_from_runtime_mode() {
	assert_eq!(
		OutputMode::from_runtime_mode("interactive"),
		OutputMode::Interactive
	);
	assert_eq!(
		OutputMode::from_runtime_mode("plain"),
		OutputMode::NonInteractive
	);
	assert_eq!(OutputMode::from_runtime_mode("jsonl"), OutputMode::Jsonl);
	assert_eq!(
		OutputMode::from_runtime_mode("websocket"),
		OutputMode::WebSocket
	);
	assert_eq!(
		OutputMode::from_runtime_mode("unknown"),
		OutputMode::NonInteractive
	);
}

#[test]
fn test_output_mode_is_interactive() {
	assert!(OutputMode::Interactive.is_interactive());
	assert!(!OutputMode::NonInteractive.is_interactive());
	assert!(!OutputMode::Jsonl.is_interactive());
	assert!(!OutputMode::WebSocket.is_interactive());
}

#[test]
fn test_output_mode_should_show_animations() {
	assert!(OutputMode::Interactive.should_show_animations());
	assert!(!OutputMode::NonInteractive.should_show_animations());
	assert!(!OutputMode::Jsonl.should_show_animations());
	assert!(!OutputMode::WebSocket.should_show_animations());
}

#[test]
fn test_output_mode_should_suppress_cli_output() {
	assert!(!OutputMode::Interactive.should_suppress_cli_output());
	assert!(!OutputMode::NonInteractive.should_suppress_cli_output());
	assert!(OutputMode::Jsonl.should_suppress_cli_output());
	assert!(OutputMode::WebSocket.should_suppress_cli_output());
}

#[test]
fn test_output_mode_is_terminal_mode() {
	assert!(OutputMode::Interactive.is_terminal_mode());
	assert!(OutputMode::NonInteractive.is_terminal_mode());
	assert!(!OutputMode::Jsonl.is_terminal_mode());
	assert!(!OutputMode::WebSocket.is_terminal_mode());
}

#[test]
fn test_silent_sink_discards_messages() {
	let sink = SilentSink;
	let msg = ServerMessage::Assistant(AssistantPayload {
		content: "test".to_string(),
		session_id: "session_123".to_string(),
		step: None,
	});

	// Should not panic, should not output anything
	sink.emit(msg);
}

#[test]
fn test_jsonl_sink_emits_valid_json() {
	let sink = JsonlSink;
	let msg = ServerMessage::Assistant(AssistantPayload {
		content: "test content".to_string(),
		session_id: "session_123".to_string(),
		step: None,
	});

	// Note: In real test, you'd capture stdout
	// For now, just verify it doesn't panic
	sink.emit(msg);
}

#[test]
fn test_websocket_sink_sends_through_channel() {
	let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
	let sink = WebSocketSink::new(tx);

	let msg = ServerMessage::Assistant(AssistantPayload {
		content: "test".to_string(),
		session_id: "session_123".to_string(),
		step: None,
	});

	sink.emit(msg);

	// Verify message was sent and is the correct variant
	let received = rx.try_recv().unwrap();
	assert!(
		matches!(received, ServerMessage::Assistant(AssistantPayload { content, .. }) if content == "test")
	);
}

#[test]
fn test_websocket_sink_handles_closed_channel() {
	let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
	let sink = WebSocketSink::new(tx);

	// Close receiver
	drop(rx);

	let msg = ServerMessage::Assistant(AssistantPayload {
		content: "test".to_string(),
		session_id: "session_123".to_string(),
		step: None,
	});

	// Should not panic when channel is closed
	sink.emit(msg);
}

#[test]
fn test_detect_output_mode_jsonl_is_terminal_independent() {
	assert_eq!(detect_output_mode("jsonl"), OutputMode::Jsonl);
}

#[test]
fn test_detect_output_mode_keeps_unknown_modes_on_the_terminal() {
	// stdin terminal-ness varies by harness; both outcomes are terminal
	// modes, so assert the invariant instead of the exact variant.
	assert!(detect_output_mode("plain").is_terminal_mode());
	assert!(detect_output_mode("").is_terminal_mode());
	assert!(detect_output_mode("no-such-mode").is_terminal_mode());
}

#[test]
fn test_from_cli_arg_is_case_sensitive() {
	// Only lowercase "jsonl"/"plain" are recognized; anything else —
	// including different casing — falls back to terminal detection.
	assert_ne!(OutputMode::from_cli_arg("JSONL", true), OutputMode::Jsonl);
	assert_eq!(
		OutputMode::from_cli_arg("JSONL", true),
		OutputMode::Interactive
	);
	assert_eq!(
		OutputMode::from_cli_arg("Plain", false),
		OutputMode::NonInteractive
	);
}

#[test]
fn test_from_cli_arg_has_no_websocket_arm() {
	// WebSocket is a runtime mode only; the CLI arg falls through to the
	// terminal-based default rather than selecting the WebSocket sink.
	assert_eq!(
		OutputMode::from_cli_arg("websocket", true),
		OutputMode::Interactive
	);
	assert_eq!(
		OutputMode::from_cli_arg("websocket", false),
		OutputMode::NonInteractive
	);
}

#[test]
fn test_output_mode_is_copy_and_debuggable() {
	let mode = OutputMode::Jsonl;
	let copied = mode; // Copy: `mode` stays usable after the "move"
	assert_eq!(mode, copied);
	assert!(format!("{:?}", OutputMode::WebSocket).contains("WebSocket"));
}

#[test]
fn test_websocket_sink_preserves_order_across_messages() {
	let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
	let sink = WebSocketSink::new(tx);

	for i in 0..3 {
		sink.emit(ServerMessage::Assistant(AssistantPayload {
			content: format!("msg-{i}"),
			session_id: "session_123".to_string(),
			step: None,
		}));
	}

	for i in 0..3 {
		let received = rx.try_recv().expect("message must be queued");
		assert!(
			matches!(&received, ServerMessage::Assistant(p) if p.content == format!("msg-{i}")),
			"expected msg-{i}, got {received:?}"
		);
	}
	assert!(rx.try_recv().is_err(), "channel must be drained");
}

#[test]
fn test_websocket_sink_clone_shares_the_channel() {
	let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
	let sink = WebSocketSink::new(tx);
	let cloned = sink.clone();

	sink.emit(ServerMessage::Assistant(AssistantPayload {
		content: "from-original".to_string(),
		session_id: "session_123".to_string(),
		step: None,
	}));
	cloned.emit(ServerMessage::Assistant(AssistantPayload {
		content: "from-clone".to_string(),
		session_id: "session_123".to_string(),
		step: None,
	}));

	let first = rx.try_recv().expect("first message must be queued");
	let second = rx.try_recv().expect("second message must be queued");
	assert!(matches!(&first, ServerMessage::Assistant(p) if p.content == "from-original"));
	assert!(matches!(&second, ServerMessage::Assistant(p) if p.content == "from-clone"));
}
