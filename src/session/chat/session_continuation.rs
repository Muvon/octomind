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

// Session continuation module - handles automatic session reset when token limits are reached

use crate::config::Config;
use crate::session::chat::session::ChatSession;
use anyhow::Result;

// All constants kept internal - no configuration needed
const SUMMARY_REQUEST_PROMPT: &str = r#"
CRITICAL: This session is approaching token limits. Please provide a comprehensive summary:

**CURRENT OBJECTIVE**: What is the main task/goal we're working on?

**WORK COMPLETED**: What has been accomplished so far? Include:
- Code changes made
- Files modified
- Tools used
- Problems solved

**CURRENT STATE**: Where are we right now? Include:
- What's currently in progress
- Any pending operations
- Current context/focus

**NEXT STEPS**: What needs to be done next? Include:
- Immediate next actions
- Planned approach
- Expected outcomes

**CRITICAL CONTEXT**: Any important details, decisions, or constraints that must be preserved.

Provide this summary in a clear, actionable format so we can continue seamlessly.
"#;

const CONTINUATION_USER_MESSAGE: &str = r#"Thank you for the summary. Let's continue our work from where we left off. Please proceed with the next steps as outlined in your summary."#;

/// Parameters for continuation processing
pub struct ContinuationParams<'a> {
	pub chat_session: &'a mut ChatSession,
	pub config: &'a Config,
	pub current_tokens: usize,
}

impl<'a> ContinuationParams<'a> {
	pub fn new(
		chat_session: &'a mut ChatSession,
		config: &'a Config,
		current_tokens: usize,
	) -> Self {
		Self {
			chat_session,
			config,
			current_tokens,
		}
	}
}

/// Check if we should trigger session continuation
pub fn should_trigger_continuation(params: &ContinuationParams) -> bool {
	params.config.max_session_tokens_threshold > 0
		&& params.current_tokens >= params.config.max_session_tokens_threshold
		&& !params.chat_session.continuation_pending
}

/// Check if we're currently in continuation process
pub fn is_continuation_in_progress(chat_session: &ChatSession) -> bool {
	chat_session.continuation_pending
}

/// Inject summary request into current session
/// This can happen during ANY processing step, not just user input
pub fn inject_summary_request(params: &mut ContinuationParams) -> Result<()> {
	use colored::*;

	println!(
		"\n{}",
		"🔄 Token limit reached during processing - requesting work summary...".bright_blue()
	);

	// Add summary request as user message
	let summary_message = crate::session::Message {
		role: "user".to_string(),
		content: SUMMARY_REQUEST_PROMPT.to_string(),
		timestamp: std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.unwrap_or_default()
			.as_secs(),
		cached: false,
		tool_calls: None,
		tool_call_id: None,
		name: None,
		images: None,
	};

	params.chat_session.session.messages.push(summary_message);
	params.chat_session.continuation_pending = true;

	println!(
		"{}",
		"   Summary request injected into conversation flow".cyan()
	);

	Ok(())
}

/// Process continuation after AI responds with summary
/// Returns true if continuation was processed, false if this wasn't a continuation response
pub fn process_continuation_response(
	chat_session: &mut ChatSession,
	response_content: &str,
	has_tool_calls: bool,
) -> Result<bool> {
	// Only process if we're expecting a continuation response
	if !chat_session.continuation_pending {
		return Ok(false);
	}

	// If the response has tool calls, let them execute first
	if has_tool_calls {
		return Ok(false);
	}

	use colored::*;

	println!(
		"\n{}",
		"✅ Work summary received - resetting session for continuation...".bright_green()
	);

	// Find system message to preserve
	let system_message = chat_session
		.session
		.messages
		.iter()
		.find(|msg| msg.role == "system")
		.cloned();

	// Clear all messages
	chat_session.session.messages.clear();

	// Rebuild with minimal context
	if let Some(system_msg) = system_message {
		chat_session.session.messages.push(system_msg);
	}

	// Add the AI's summary as assistant message
	let summary_message = crate::session::Message {
		role: "assistant".to_string(),
		content: response_content.to_string(),
		timestamp: std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.unwrap_or_default()
			.as_secs(),
		cached: false,
		tool_calls: None,
		tool_call_id: None,
		name: None,
		images: None,
	};
	chat_session.session.messages.push(summary_message);

	// Add continuation user message
	let continue_message = crate::session::Message {
		role: "user".to_string(),
		content: CONTINUATION_USER_MESSAGE.to_string(),
		timestamp: std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.unwrap_or_default()
			.as_secs(),
		cached: false,
		tool_calls: None,
		tool_call_id: None,
		name: None,
		images: None,
	};
	chat_session.session.messages.push(continue_message);

	// Reset continuation state
	chat_session.continuation_pending = false;

	println!(
		"{}",
		"🚀 Session continued with preserved context - resuming work...".bright_cyan()
	);

	Ok(true)
}

/// Check and handle continuation at any point during processing
/// This is the main entry point that should be called during response processing
pub fn check_and_handle_continuation(
	chat_session: &mut ChatSession,
	config: &Config,
) -> Result<bool> {
	let current_tokens = crate::session::estimate_message_tokens(&chat_session.session.messages);

	let mut params = ContinuationParams::new(chat_session, config, current_tokens);

	if should_trigger_continuation(&params) {
		inject_summary_request(&mut params)?;
		return Ok(true); // Continuation triggered
	}

	Ok(false) // No continuation needed
}
