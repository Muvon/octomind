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

	/// Create OutputMode from a runtime_output_mode string (no terminal detection needed)
	pub fn from_runtime_mode(mode: &str) -> Self {
		match mode {
			"interactive" => Self::Interactive,
			"jsonl" => Self::Jsonl,
			"websocket" => Self::WebSocket,
			_ => Self::NonInteractive,
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
#[path = "output_tests.rs"]
mod tests;
