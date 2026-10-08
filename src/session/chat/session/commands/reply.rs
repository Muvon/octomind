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

// Reply command handler — quotes an earlier assistant reply so the user can
// answer it point by point.

use super::super::core::ChatSession;
use super::{CommandOutput, CommandResult};
use crate::utils::time::format_ago;
use anyhow::Result;

/// Longest one-line preview kept for a menu row: wider than any terminal row,
/// which the menu then truncates to fit.
const PREVIEW_CHARS: usize = 240;

/// One assistant reply the user can quote.
#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
	pub text: String,
	/// Unix seconds the reply was recorded at.
	pub timestamp: u64,
}

impl Reply {
	/// The reply as one line of prose for a menu row: heading marks, code
	/// fences and blank lines drop out.
	pub fn preview(&self) -> String {
		self.text
			.lines()
			.map(|line| line.trim().trim_start_matches('#').trim_start())
			.filter(|line| !line.is_empty() && !line.starts_with("```"))
			.collect::<Vec<_>>()
			.join(" ")
			.chars()
			.take(PREVIEW_CHARS)
			.collect()
	}

	/// "1 line" / "14 lines".
	pub fn size(&self) -> String {
		match self.text.lines().count() {
			1 => "1 line".to_string(),
			lines => format!("{} lines", lines),
		}
	}
}

/// How long ago `timestamp` was, the way people say it: "just now", "4m ago".
pub fn age(timestamp: u64, now: u64) -> String {
	format_ago(now.saturating_sub(timestamp))
}

/// Assistant replies in the session, newest first. `/reply N` and the prompt's
/// pickers number them in this order, so 1 is always the latest.
pub fn assistant_replies(session: &ChatSession) -> Vec<Reply> {
	session
		.session
		.messages
		.iter()
		.rev()
		.filter(|message| message.role == "assistant")
		.map(|message| Reply {
			text: message.content.trim().to_string(),
			timestamp: message.timestamp,
		})
		.filter(|reply| !reply.text.is_empty())
		.collect()
}

/// Whether `text` contains every whitespace-separated word of `query`,
/// ignoring case. An empty query matches everything.
pub fn matches_words(text: &str, query: &str) -> bool {
	let text = text.to_lowercase();
	query
		.split_whitespace()
		.all(|word| text.contains(&word.to_lowercase()))
}

/// `text` as a markdown blockquote. Blank lines become a bare `>` so the
/// quote stays one block.
pub fn quote(text: &str) -> String {
	text.lines()
		.map(|line| {
			if line.trim().is_empty() {
				">".to_string()
			} else {
				format!("> {}", line)
			}
		})
		.collect::<Vec<_>>()
		.join("\n")
}

/// `/reply` quotes the latest reply, `/reply N` the Nth latest, and
/// `/reply WORDS` the latest reply containing every word.
pub fn handle_reply(session: &ChatSession, params: &[&str]) -> Result<CommandResult> {
	let replies = assistant_replies(session);
	let query = params.join(" ");
	let numbered = query.parse::<usize>().ok();

	let selected = match numbered {
		Some(number) => number
			.checked_sub(1)
			.and_then(|index| replies.get(index))
			.map(|reply| (number, reply)),
		None => replies
			.iter()
			.zip(1..)
			.find(|(reply, _)| matches_words(&reply.text, &query))
			.map(|(reply, number)| (number, reply)),
	};

	let Some((number, reply)) = selected else {
		let error = if replies.is_empty() {
			"No assistant reply to quote yet.".to_string()
		} else if numbered.is_some() {
			format!("No reply #{}: this session has {}.", query, replies.len())
		} else {
			format!("No reply contains '{}'.", query)
		};
		return Ok(CommandResult::HandledWithOutput(Box::new(
			CommandOutput::Error {
				error,
				context: Some(serde_json::json!({
					"replies": replies.len(),
					"hint": "Use /reply [N|words]; 1 is the latest reply. Press Tab after '/reply ' to pick one.",
				})),
			},
		)));
	};

	Ok(CommandResult::HandledWithOutput(Box::new(
		CommandOutput::Reply {
			number,
			timestamp: reply.timestamp,
			lines: reply.text.lines().count(),
			quote: quote(&reply.text),
		},
	)))
}

#[cfg(test)]
#[path = "reply_tests.rs"]
mod tests;
