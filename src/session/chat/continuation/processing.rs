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

// Session continuation processing - handles summary response and session reset

use crate::config::Config;
use crate::log_info;
use crate::session::chat::assistant_output::print_assistant_response;
use crate::session::chat::continuation::constants::CONTINUATION_USER_MESSAGE_TEMPLATE;
use crate::session::chat::continuation::detection::{
	should_trigger_continuation, ContinuationParams,
};
use crate::session::chat::continuation::file_context::{
	collect_user_request_history, generate_file_context_content, parse_file_contexts,
};
use crate::session::chat::continuation::injection::inject_summary_request;
use crate::session::chat::session::ChatSession;
use anyhow::Result;
use colored::Colorize;
use std::sync::atomic::Ordering;

/// Process continuation after AI responds with summary
/// Returns true if continuation was processed, false if this wasn't a continuation response
pub async fn process_continuation_response(
	chat_session: &mut ChatSession,
	response_content: &str,
	_has_tool_calls: bool, // Prefixed with underscore to indicate intentional non-use
	config: &Config,
	role: &str,
) -> Result<bool> {
	// Only process if we're expecting a continuation response
	if !chat_session.continuation_pending {
		return Ok(false);
	}

	// CRITICAL FIX: Process continuation regardless of tool calls
	// Tool calls will be handled in the normal flow, but we need to process continuation
	// when the AI provides a summary response, even if it includes tool calls

	// CRITICAL FIX: Display the assistant summary to the user BEFORE processing
	println!(
		"{}",
		"📋 Session Summary (Token limit reached)"
			.bright_blue()
			.bold()
	);
	println!("{}", "─".repeat(50).dimmed());
	print_assistant_response(response_content, config, role);
	println!();

	// Log continuation processing (less visible to user)
	log_info!("Work summary received - resetting session for continuation...");
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

	// Add initial messages (welcome + instructions) using centralized function
	let current_dir = std::env::current_dir().unwrap_or_default();
	if let Ok(initial_messages) =
		crate::session::chat::session::get_initial_messages(config, role, &current_dir).await
	{
		chat_session.session.messages.extend(initial_messages);
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

	// Collect user request history before clearing session
	let user_history = collect_user_request_history(&chat_session.session.messages);

	// Parse file context requirements from the AI's summary
	let file_contexts = parse_file_contexts(response_content);

	// Generate file context content
	let context_content = generate_file_context_content(&file_contexts);

	// Create continuation message with user history and file context
	let continuation_content = CONTINUATION_USER_MESSAGE_TEMPLATE
		.replace("{}", &user_history)
		.replace("{}", &context_content);

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

	// CRITICAL FIX: Display smart summary of continuation message to user
	println!("{}", "🔄 Continuing Session".bright_green().bold());
	println!("{}", "─".repeat(50).dimmed());

	// Show task summary
	let task_count = user_history
		.lines()
		.filter(|line| !line.trim().is_empty())
		.count();
	if task_count > 0 {
		println!(
			"{} {}",
			"📋 Continuing with".dimmed(),
			format!("{} task(s)", task_count).bright_white()
		);
	}

	// Show file context summary
	if !file_contexts.is_empty() {
		println!(
			"{} {}",
			"📁 Loaded context from".dimmed(),
			format!("{} file(s)", file_contexts.len()).bright_white()
		);
		for (filepath, start, end) in &file_contexts {
			println!(
				"   {} {}",
				"•".dimmed(),
				format!("{} (lines {}-{})", filepath, start, end).bright_cyan()
			);
		}
	}

	println!("{}", "🚀 Ready to continue...".bright_green());
	println!();

	// Reset continuation state
	chat_session.continuation_pending = false;

	// Log context information
	if !file_contexts.is_empty() {
		log_info!("Loaded context from {} file(s)", file_contexts.len());
		for (filepath, start, end) in &file_contexts {
			log_info!("   {} (lines {}-{})", filepath, start, end);
		}
	} else {
		log_info!("No file contexts found in AI summary - check format");
		crate::log_debug!("Summary content for context parsing: {}", response_content);
	}

	// Log user history preservation
	let history_lines = user_history.lines().count();
	if history_lines > 1 {
		log_info!(
			"Preserved {} user request(s) for continuation context",
			history_lines
		);
	}

	log_info!("Session context preserved - continuing automatically...");

	Ok(true)
}

/// Check and handle continuation at any point during processing
/// This is the main entry point that should be called during response processing
pub async fn check_and_handle_continuation(
	chat_session: &mut ChatSession,
	config: &Config,
) -> Result<bool> {
	check_and_handle_continuation_with_cancellation(chat_session, config, None).await
}

/// Check and handle continuation with cancellation support
/// This allows CTRL-C to interrupt long-running continuation operations
pub async fn check_and_handle_continuation_with_cancellation(
	chat_session: &mut ChatSession,
	config: &Config,
	operation_cancelled: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> Result<bool> {
	// Check for cancellation at the start
	if let Some(ref cancelled) = operation_cancelled {
		if cancelled.load(Ordering::SeqCst) {
			return Err(anyhow::anyhow!("Operation cancelled"));
		}
	}

	let current_tokens = {
		// Get system prompt for the role from ChatSession
		let (_, _, _, _, system_prompt) = config.get_role_config(&chat_session.role);

		// Get tool definitions
		let tools = crate::mcp::get_available_functions(config).await;

		// Use enhanced token counting that includes system prompt + tools
		crate::session::estimate_full_context_tokens(
			&chat_session.session.messages,
			Some(system_prompt),
			if tools.is_empty() { None } else { Some(&tools) },
		)
	};

	let mut params = ContinuationParams::new(chat_session, config, current_tokens);

	if should_trigger_continuation(&params) {
		// Check for cancellation before injecting summary request
		if let Some(ref cancelled) = operation_cancelled {
			if cancelled.load(Ordering::SeqCst) {
				return Err(anyhow::anyhow!("Operation cancelled"));
			}
		}

		inject_summary_request(&mut params)?;
		return Ok(true); // Continuation triggered
	}

	Ok(false) // No continuation needed
}
