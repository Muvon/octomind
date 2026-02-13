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

//! Output abstraction for streaming structured messages
//!
//! This module provides zero-cost abstractions for different output modes:
//! - CLI (interactive and non-interactive)
//! - JSONL (structured JSON output)
//! - WebSocket (streaming to clients)
//!
//! # Design
//!
//! Uses trait-based abstraction with static dispatch for zero-cost performance.
//! All sinks are zero-sized types (ZST) that compile down to direct function calls.

use crate::websocket::ServerMessage;
use std::io::IsTerminal;

/// Output mode determines behavior and output format
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
	/// Interactive CLI with colors, animations, and prompts
	Interactive,
	/// Non-interactive CLI without animations (piped input/output)
	NonInteractive,
	/// Structured JSON Lines output (one JSON object per line)
	Jsonl,
	/// WebSocket streaming mode
	WebSocket,
}

impl OutputMode {
	/// Create OutputMode from CLI argument and terminal detection
	pub fn from_cli_arg(mode: &str, is_terminal: bool) -> Self {
		match mode {
			"jsonl" => Self::Jsonl,
			"plain" if is_terminal => Self::Interactive,
			"plain" => Self::NonInteractive,
			_ => {
				if is_terminal {
					Self::Interactive
				} else {
					Self::NonInteractive
				}
			}
		}
	}

	/// Check if mode is interactive (shows animations, prompts, colors)
	pub fn is_interactive(self) -> bool {
		matches!(self, Self::Interactive)
	}

	/// Check if animations should be shown
	pub fn should_show_animations(self) -> bool {
		matches!(self, Self::Interactive)
	}

	/// Check if CLI output should be suppressed (using structured output instead)
	pub fn should_suppress_cli_output(self) -> bool {
		matches!(self, Self::Jsonl | Self::WebSocket)
	}

	/// Check if this is a terminal-based mode (not structured output)
	pub fn is_terminal_mode(self) -> bool {
		matches!(self, Self::Interactive | Self::NonInteractive)
	}
}

/// Trait for streaming output messages
///
/// Implementations are zero-sized types (ZST) for zero-cost abstraction.
/// The compiler inlines all calls, resulting in direct function calls with no overhead.
pub trait OutputSink: Clone {
	/// Emit a message to the output destination
	fn emit(&self, msg: ServerMessage);
}

/// Silent sink - discards all messages (used for CLI modes)
///
/// Zero-sized type - compiles to nothing
#[derive(Clone, Copy)]
pub struct SilentSink;

impl OutputSink for SilentSink {
	#[inline]
	fn emit(&self, _msg: ServerMessage) {
		// Intentionally empty - CLI handles output separately
	}
}

/// JSONL sink - prints messages as JSON Lines to stdout
///
/// Zero-sized type - compiles to direct println! calls
#[derive(Clone, Copy)]
pub struct JsonlSink;

impl OutputSink for JsonlSink {
	#[inline]
	fn emit(&self, msg: ServerMessage) {
		// Print as single-line JSON (JSONL format)
		if let Ok(json) = serde_json::to_string(&msg) {
			println!("{}", json);
		}
	}
}

/// WebSocket sink - sends messages through a channel
///
/// Contains channel sender for async message delivery
#[derive(Clone)]
pub struct WebSocketSink {
	tx: tokio::sync::mpsc::UnboundedSender<ServerMessage>,
}

impl WebSocketSink {
	/// Create new WebSocket sink with channel sender
	pub fn new(tx: tokio::sync::mpsc::UnboundedSender<ServerMessage>) -> Self {
		Self { tx }
	}
}

impl OutputSink for WebSocketSink {
	#[inline]
	fn emit(&self, msg: ServerMessage) {
		// Send through channel, ignore errors (client may have disconnected)
		let _ = self.tx.send(msg);
	}
}

/// Detect output mode from environment and CLI arguments
pub fn detect_output_mode(cli_mode: &str) -> OutputMode {
	OutputMode::from_cli_arg(cli_mode, std::io::stdin().is_terminal())
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::websocket::MessageType;

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
		let msg = ServerMessage::new(
			MessageType::Assistant,
			"test".to_string(),
			Some("session_123".to_string()),
		);

		// Should not panic, should not output anything
		sink.emit(msg);
	}

	#[test]
	fn test_jsonl_sink_emits_valid_json() {
		let sink = JsonlSink;
		let msg = ServerMessage::new(
			MessageType::Assistant,
			"test content".to_string(),
			Some("session_123".to_string()),
		);

		// Note: In real test, you'd capture stdout
		// For now, just verify it doesn't panic
		sink.emit(msg);
	}

	#[test]
	fn test_websocket_sink_sends_through_channel() {
		let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
		let sink = WebSocketSink::new(tx);

		let msg = ServerMessage::new(
			MessageType::Assistant,
			"test".to_string(),
			Some("session_123".to_string()),
		);

		sink.emit(msg.clone());

		// Verify message was sent
		let received = rx.try_recv().unwrap();
		assert_eq!(received.content, "test");
		assert_eq!(received.message_type, MessageType::Assistant);
	}

	#[test]
	fn test_websocket_sink_handles_closed_channel() {
		let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
		let sink = WebSocketSink::new(tx);

		// Close receiver
		drop(rx);

		let msg = ServerMessage::new(
			MessageType::Assistant,
			"test".to_string(),
			Some("session_123".to_string()),
		);

		// Should not panic when channel is closed
		sink.emit(msg);
	}
}
