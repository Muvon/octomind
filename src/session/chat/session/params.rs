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

// Session parameter struct shared across all callers (CLI, ACP, WebSocket).
// Replaces the previous Debug-format-string parser — every consumer now reads
// fields directly off this struct.

/// Generic session arguments that can be used by any caller (CLI, WebSocket, etc.)
#[derive(Debug, Clone, Default)]
pub struct GenericSessionArgs {
	pub name: Option<String>,
	pub resume: Option<String>,
	pub resume_recent: bool,
	pub model: Option<String>,
	pub max_tokens: Option<u32>,
	pub temperature: Option<f32>,
	pub role: String,
	pub max_retries: Option<u32>,
	/// Output mode: "plain", "jsonl", or "websocket". Anything else is
	/// normalised to "plain" by `setup_and_initialize_session`.
	pub mode: String,
	/// When true, the non-interactive session loop never exits on its own —
	/// it waits indefinitely for messages (via `octomind send`).
	pub daemon: bool,
	/// Webhook hook names to activate for this session.
	pub hooks: Vec<String>,
}

impl GenericSessionArgs {
	/// Create new session args with defaults
	pub fn new(role: String) -> Self {
		Self {
			role,
			mode: "plain".to_string(),
			..Default::default()
		}
	}

	/// Create args for resuming a session
	pub fn resume(session_id: String, role: String) -> Self {
		Self {
			resume: Some(session_id),
			role,
			mode: "plain".to_string(),
			..Default::default()
		}
	}
}
