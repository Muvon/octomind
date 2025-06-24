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
use regex::Regex;
use std::path::Path;

// All constants kept internal - no configuration needed
const SUMMARY_REQUEST_PROMPT: &str = r#"
CRITICAL: Session approaching token limits. Provide STRUCTURED summary for seamless continuation:

## OBJECTIVE
Current main task/goal being worked on.

## PROGRESS
What has been accomplished:
- Specific code changes made
- Files modified with key changes
- Tools used and results
- Problems solved

## CURRENT STATE
Exact current situation:
- Active work in progress
- Pending operations
- Current focus area

## REQUIRED FILE CONTEXTS
List ALL files needed as context to continue work. Use EXACT format:
```
filename:startline:endline
filename:startline:endline
```
- Use absolute paths from project root
- Include only essential line ranges
- Focus on relevant functions/classes/sections
- Maximum 10 file ranges total

## NEXT ACTIONS
Immediate next steps:
- Specific actions to take
- Expected approach
- Anticipated outcomes

## CRITICAL NOTES
Key details/decisions/constraints that must be preserved.

Follow this structure EXACTLY for optimal continuation.
"#;

const CONTINUATION_USER_MESSAGE_TEMPLATE: &str = r#"Thank you for the summary. Here's the required file context:

{}

Let's continue our work from where we left off.
Please proceed with the next steps as outlined in your summary.
CRITICAL: use tool calling in paralell when its possible to reach results faster and more efficiently."#;

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

	// Parse file context requirements from the AI's summary
	let file_contexts = parse_file_contexts(response_content);

	// Generate file context content
	let context_content = generate_file_context_content(&file_contexts);

	// Create continuation message with file context
	let continuation_content = CONTINUATION_USER_MESSAGE_TEMPLATE.replace("{}", &context_content);

	let continue_message = crate::session::Message {
		role: "user".to_string(),
		content: continuation_content,
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

	// Log context information
	if !file_contexts.is_empty() {
		println!(
			"{}",
			format!("📁 Loaded context from {} file(s)", file_contexts.len()).bright_cyan()
		);
		for (filepath, start, end) in &file_contexts {
			println!("   {} (lines {}-{})", filepath, start, end);
		}
	}

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

/// Parse file context requirements from AI summary response
/// Expected format: filename:startline:endline
fn parse_file_contexts(summary_content: &str) -> Vec<(String, usize, usize)> {
	let mut contexts = Vec::new();

	// Look for code blocks or lines with filename:startline:endline pattern
	let file_pattern = Regex::new(r"([^\s:]+):(\d+):(\d+)").unwrap();

	for captures in file_pattern.captures_iter(summary_content) {
		if let (Some(filename), Some(start_str), Some(end_str)) =
			(captures.get(1), captures.get(2), captures.get(3))
		{
			if let (Ok(start_line), Ok(end_line)) = (
				start_str.as_str().parse::<usize>(),
				end_str.as_str().parse::<usize>(),
			) {
				let filename = filename.as_str().to_string();

				// Validate line range
				if start_line > 0 && end_line >= start_line && end_line <= 10000 {
					contexts.push((filename, start_line, end_line));
				}
			}
		}
	}

	// Limit to maximum 10 file contexts for performance
	contexts.truncate(10);
	contexts
}

/// Read file content for specified line ranges with 1-indexed line numbers
fn read_file_context(filepath: &str, start_line: usize, end_line: usize) -> Result<String> {
	use std::fs;
	use std::io::{BufRead, BufReader};

	// Validate file exists and is readable
	if !Path::new(filepath).exists() {
		return Ok(format!("// File not found: {}", filepath));
	}

	let file = fs::File::open(filepath)?;
	let reader = BufReader::new(file);
	let mut result = String::new();

	result.push_str(&format!(
		"=== {} (lines {}-{}) ===\n",
		filepath, start_line, end_line
	));

	for (line_num, line_result) in reader.lines().enumerate() {
		let line_number = line_num + 1; // Convert to 1-indexed

		if line_number < start_line {
			continue;
		}

		if line_number > end_line {
			break;
		}

		match line_result {
			Ok(line_content) => {
				result.push_str(&format!("{}: {}\n", line_number, line_content));
			}
			Err(_) => {
				result.push_str(&format!("{}: // Error reading line\n", line_number));
			}
		}
	}

	result.push('\n');
	Ok(result)
}

/// Generate file context content from parsed file requirements
fn generate_file_context_content(file_contexts: &[(String, usize, usize)]) -> String {
	if file_contexts.is_empty() {
		return "No specific file context requested.".to_string();
	}

	let mut context_content = String::new();
	context_content.push_str("FILE CONTEXT:\n\n");

	for (filepath, start_line, end_line) in file_contexts {
		match read_file_context(filepath, *start_line, *end_line) {
			Ok(file_content) => {
				context_content.push_str(&file_content);
			}
			Err(e) => {
				context_content.push_str(&format!(
					"=== {} (lines {}-{}) ===\n// Error reading file: {}\n\n",
					filepath, start_line, end_line, e
				));
			}
		}
	}

	context_content
}
