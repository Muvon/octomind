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

//! Pure utility functions for MCP tool handling.

use serde::{Deserialize, Serialize};

use super::{McpToolCall, McpToolResult};

// Guess the category of a tool based on its name
pub fn guess_tool_category(tool_name: &str) -> &'static str {
	match tool_name {
		"core" => "system",
		"text_editor" | "batch_edit" | "extract_lines" => "filesystem",
		"shell" | "ast_grep" | "workdir" | "view" | "list_files" => "filesystem",
		"plan" => "core",
		name if name.contains("file") || name.contains("editor") => "core",
		name if name.contains("search") || name.contains("find") => "search",
		name if name.contains("image") || name.contains("photo") => "media",
		name if name.contains("web") || name.contains("http") => "web",
		name if name.contains("db") || name.contains("database") => "database",
		name if name.contains("browser") => "browser",
		name if name.contains("terminal") => "terminal",
		name if name.contains("video") => "video",
		name if name.contains("audio") => "audio",
		name if name.contains("location") || name.contains("map") => "location",
		name if name.contains("google") => "google",
		name if name.contains("weather") => "weather",
		name if name.contains("calculator") || name.contains("math") => "math",
		name if name.contains("news") => "news",
		name if name.contains("email") => "email",
		name if name.contains("calendar") => "calendar",
		name if name.contains("translate") => "translation",
		name if name.contains("github") => "github",
		name if name.contains("git") => "git",
		_ => "external",
	}
}

// Structure to represent tool responses for OpenAI/Claude format
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResponseMessage {
	pub role: String,
	pub tool_call_id: String,
	pub name: String,
	pub content: String,
}

// Convert tool results to proper messages with global truncation
pub fn tool_results_to_messages(
	results: &[McpToolResult],
	config: &crate::config::Config,
) -> Vec<ToolResponseMessage> {
	let mut messages = Vec::new();

	for result in results {
		let content_str = result.extract_content();

		// Apply global MCP response truncation
		let (final_content, was_truncated) = crate::utils::truncation::truncate_mcp_response_global(
			&content_str,
			config.mcp_response_tokens_threshold,
		);
		if was_truncated {
			let is_terminal = crate::config::with_thread_config(|c| c.output_mode())
				.unwrap_or(crate::session::output::OutputMode::NonInteractive)
				.is_terminal_mode();
			if is_terminal {
				use colored::Colorize;
				eprintln!(
					"{}",
					format!(
						"⚠️  Tool '{}' response truncated to {} tokens (mcp_response_tokens_threshold)",
						result.tool_name, config.mcp_response_tokens_threshold
					)
					.bright_yellow()
				);
			}
		}

		messages.push(ToolResponseMessage {
			role: "tool".to_string(),
			tool_call_id: result.tool_id.clone(),
			name: result.tool_name.clone(),
			content: final_content,
		});
	}

	messages
}

// Ensure tool calls have valid IDs
pub fn ensure_tool_call_ids(calls: &mut [McpToolCall]) {
	for call in calls.iter_mut() {
		if call.tool_id.is_empty() {
			call.tool_id = format!("tool_{}", uuid::Uuid::new_v4().simple());
		}
	}
}
