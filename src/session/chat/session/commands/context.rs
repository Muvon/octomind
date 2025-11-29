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

// Context command handler

use super::super::core::ChatSession;
use super::{CommandOutput, CommandResult};
use anyhow::Result;

pub fn handle_context(session: &ChatSession, params: &[&str]) -> Result<CommandResult> {
	// Parse filter parameter if provided
	let filter = if params.is_empty() {
		"all".to_string()
	} else {
		params[0].to_lowercase()
	};

	// Build JSON output with actual messages
	let filtered_messages: Vec<serde_json::Value> = session
		.session
		.messages
		.iter()
		.filter(|msg| match filter.as_str() {
			"all" => true,
			"assistant" => msg.role == "assistant",
			"user" => msg.role == "user",
			"tool" => msg.role == "tool",
			"system" => msg.role == "system",
			"large" => msg.content.len() > 1000,
			_ => true,
		})
		.map(|msg| {
			serde_json::json!({
				"role": msg.role,
				"content": msg.content,
				"name": msg.name,
				"tool_calls": msg.tool_calls,
				"tool_call_id": msg.tool_call_id,
			})
		})
		.collect();

	Ok(CommandResult::HandledWithOutput(CommandOutput::Context {
		filter,
		total_messages: session.session.messages.len(),
		filtered_messages,
	}))
}
