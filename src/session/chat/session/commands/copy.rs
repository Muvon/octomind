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

// Copy command handler

use super::super::core::ChatSession;
use super::{CommandOutput, CommandResult};
use anyhow::Result;
use arboard::Clipboard;

/// Which slice of the session `/copy` puts on the clipboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyScope {
	Last,
	Assistant,
	User,
	All,
}

impl CopyScope {
	/// Every accepted scope, in the order `/copy` documents them.
	pub const ALL: [CopyScope; 4] = [
		CopyScope::Last,
		CopyScope::Assistant,
		CopyScope::User,
		CopyScope::All,
	];

	pub fn parse(raw: &str) -> Option<Self> {
		match raw.to_ascii_lowercase().as_str() {
			"last" => Some(Self::Last),
			"assistant" => Some(Self::Assistant),
			"user" => Some(Self::User),
			"all" => Some(Self::All),
			_ => None,
		}
	}

	pub fn name(self) -> &'static str {
		match self {
			Self::Last => "last",
			Self::Assistant => "assistant",
			Self::User => "user",
			Self::All => "all",
		}
	}
}

/// Render the clipboard payload for `scope`. `None` when the scope selects
/// nothing, so the caller reports "nothing to copy" instead of silently
/// clearing the clipboard.
///
/// `Last` is the raw last response — byte-identical to the pre-scope `/copy`.
/// The other scopes emit markdown-labelled blocks in session order, keeping
/// only user turns and assistant prose: tool calls, tool results and
/// system-managed injections are left out. An assistant message that also
/// carries tool calls still contributes its prose.
pub fn build_payload(session: &ChatSession, scope: CopyScope) -> Option<String> {
	if scope == CopyScope::Last {
		return (!session.last_response.is_empty()).then(|| session.last_response.clone());
	}

	let mut blocks = Vec::new();
	for message in &session.session.messages {
		let label = match message.role.as_str() {
			"assistant" if scope != CopyScope::User => "Assistant",
			"user"
				if scope != CopyScope::Assistant
					&& crate::session::is_real_user_task_message(message) =>
			{
				"User"
			}
			_ => continue,
		};
		let content = message.content.trim();
		if content.is_empty() {
			continue;
		}
		blocks.push(format!("**{label}:**\n\n{content}"));
	}

	(!blocks.is_empty()).then(|| blocks.join("\n\n"))
}

pub fn handle_copy(session: &ChatSession, params: &[&str]) -> Result<CommandResult> {
	let scope = match params.first() {
		None => CopyScope::Last,
		Some(raw) => match CopyScope::parse(raw) {
			Some(scope) => scope,
			None => {
				return Ok(CommandResult::HandledWithOutput(Box::new(
					CommandOutput::Error {
						error: format!("Unknown /copy scope '{}'.", raw),
						context: Some(serde_json::json!({
							"valid_scopes": CopyScope::ALL.map(CopyScope::name),
							"hint": "Use /copy [scope]; the default is 'last'.",
						})),
					},
				)));
			}
		},
	};

	let Some(payload) = build_payload(session, scope) else {
		return Ok(CommandResult::HandledWithOutput(Box::new(
			CommandOutput::Copy {
				copied: false,
				length: None,
				scope: scope.name().to_string(),
			},
		)));
	};

	let copied = Clipboard::new()
		.and_then(|mut clipboard| clipboard.set_text(&payload))
		.is_ok();

	Ok(CommandResult::HandledWithOutput(Box::new(
		CommandOutput::Copy {
			copied,
			length: copied.then(|| payload.len()),
			scope: scope.name().to_string(),
		},
	)))
}

#[cfg(test)]
#[path = "copy_tests.rs"]
mod tests;
