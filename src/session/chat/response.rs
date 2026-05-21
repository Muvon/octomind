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

// Response processing module - main orchestrator

pub mod tool_execution;
pub mod tool_result_processor;

use super::{CostTracker, MessageHandler, ToolProcessor};
use crate::config::Config;
use crate::providers::ThinkingBlock;
use crate::session::chat::assistant_output::print_assistant_response;
use crate::session::chat::display_thinking;
use crate::session::chat::session::ChatSession;
use crate::session::ProviderExchange;
use crate::{log_debug, log_info};
use anyhow::Result;
use colored::Colorize;

use crate::session::output::{OutputMode, OutputSink};
use crate::websocket::{
	AssistantPayload, CostPayload, ServerMessage, ThinkingPayload, ToolResultPayload,
	ToolUsePayload,
};

// Response processing parameters struct
pub struct ResponseProcessingParams<'a, S: OutputSink> {
	pub content: String,
	pub exchange: ProviderExchange,
	pub tool_calls: Option<Vec<crate::mcp::McpToolCall>>,
	pub thinking: Option<ThinkingBlock>,
	pub finish_reason: Option<String>,
	pub response_id: Option<String>,
	pub chat_session: &'a mut ChatSession,
	pub config: &'a Config,
	pub role: &'a str,
	pub operation_cancelled: tokio::sync::watch::Receiver<bool>,
	pub sink: S,
	pub mode: OutputMode,
}

impl<'a, S: OutputSink> ResponseProcessingParams<'a, S> {
	/// Set thinking block
	pub fn with_thinking(mut self, thinking: Option<ThinkingBlock>) -> Self {
		self.thinking = thinking;
		self
	}

	/// Set output mode (preferred over with_interactive)
	pub fn with_mode(mut self, mode: OutputMode) -> Self {
		self.mode = mode;
		self
	}

	/// Emit a message through the output sink
	/// This is used for streaming JSON output (WebSocket/JSONL)
	pub fn emit(&self, msg: ServerMessage) {
		self.sink.emit(msg);
	}
}

fn emit_thinking_event<S: OutputSink>(
	params: &ResponseProcessingParams<'_, S>,
	thinking: &ThinkingBlock,
	session_id: &str,
) {
	params.emit(ServerMessage::Thinking(ThinkingPayload {
		content: thinking.content.clone(),
		session_id: session_id.to_string(),
	}));
}

// Helper function to log debug information about the response
fn log_response_debug(
	_config: &Config,
	finish_reason: &Option<String>,
	tool_calls: &Option<Vec<crate::mcp::McpToolCall>>,
) {
	if let Some(ref reason) = finish_reason {
		log_debug!("Processing response with finish_reason: {}", reason);
	}
	if let Some(ref calls) = tool_calls {
		log_debug!("Processing {} tool calls", calls.len());
	}
}

// Helper function to handle final response when no tool calls are present
fn handle_final_response(
	content: &str,
	thinking: &Option<ThinkingBlock>,
	response_id: Option<String>,
	chat_session: &mut ChatSession,
	config: &Config,
	role: &str,
	mode: OutputMode,
) -> Result<()> {
	// Display thinking first if present (only in interactive mode to avoid clutter)
	if mode.is_interactive() {
		if let Some(ref thinking_block) = thinking {
			display_thinking(thinking_block);
		}
	}

	// Add the assistant message with response_id to maintain conversation continuity.
	// The response_id is essential for OpenAI Responses API to track conversation state.
	let assistant_message = crate::session::Message {
		role: "assistant".to_string(),
		content: content.to_string(),
		timestamp: std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.unwrap_or_default()
			.as_secs(),
		cached: false,
		cache_ttl: None,
		tool_call_id: None,
		name: None,
		tool_calls: None,
		images: None,
		videos: None,
		thinking: None,
		id: response_id, // CRITICAL: Set the response_id for conversation continuity
	};

	chat_session
		.session
		.messages
		.push(assistant_message.clone());
	chat_session.last_response = content.to_string();

	// Persist to session file so the message survives session close/resume
	if let Some(session_file) = &chat_session.session.session_file {
		let message_json = serde_json::to_string(&assistant_message)?;
		crate::session::append_to_session_file(session_file, &message_json)?;
	}

	// CRITICAL FIX: DO NOT track cost/tokens here - already tracked by CostTracker::track_exchange_cost()
	// in api_executor.rs:163. Tracking here causes DUPLICATE cost/token counting.
	// Only log the exchange for debugging purposes.

	// CRITICAL FIX: ALWAYS print assistant response (both interactive and non-interactive modes)
	// The mode controls animations/prompts, NOT whether to show the AI's response
	// Skip if using structured output (JSONL/WebSocket - handled by sink)
	if mode.is_terminal_mode() {
		print_assistant_response(content, config, role, thinking);
	}

	// Display cost line only for non-interactive mode or specific scenarios
	// Skip for interactive mode to avoid duplication before user input prompt
	// Skip if using structured output (JSONL/WebSocket - handled by sink)
	use std::io::IsTerminal;
	if !std::io::stdin().is_terminal() && mode.is_terminal_mode() {
		// Non-interactive mode - always show cost line
		CostTracker::display_cost_line(chat_session);
	}
	// Interactive mode: Skip cost line here to avoid duplication before user input

	Ok(())
}

// Get the actual server name for a tool (async version that matches execution)
pub async fn get_tool_server_name_async(tool_name: &str, _config: &Config) -> String {
	// First check static tool map
	if let Some(name) = crate::mcp::tool_map::get_tool_server_name(tool_name) {
		return name;
	}

	// Then check dynamic MCP servers - returns actual server name
	if let Some(name) = crate::mcp::core::dynamic::get_dynamic_server_name_by_tool(tool_name) {
		return name;
	}

	// Then check dynamic agents - they use "agent" namespace
	if crate::mcp::core::dynamic_agents::is_dynamic_by_tool(tool_name) {
		return "agent".to_string();
	}

	"unknown".to_string()
}

// Print a single param as `{prefix} key  value`, with `value` bounded to a
// reasonable display length so long paths/contents never blow up the layout.
fn print_preview_param(prefix: &str, key: &str, key_width: usize, value: &serde_json::Value) {
	let formatted = preview_value(value);
	println!(
		"{}{} {}",
		prefix,
		format!("{:width$}", key, width = key_width).bright_black(),
		formatted,
	);
}

// Compact (single-line, length-bounded) rendering of a param value for the
// upfront preview. Strings get `"…"` truncation, arrays show first element
// + count, objects collapse to `{…}`. Newlines are flattened to spaces.
fn preview_value(v: &serde_json::Value) -> String {
	const MAX_STR: usize = 60;
	match v {
		serde_json::Value::String(s) => {
			let cleaned = s.replace('\n', " ");
			if cleaned.chars().count() > MAX_STR {
				format!(
					"\"{}…\"",
					cleaned.chars().take(MAX_STR - 1).collect::<String>()
				)
			} else {
				format!("\"{}\"", cleaned)
			}
		}
		serde_json::Value::Array(a) => {
			if a.is_empty() {
				"[]".to_string()
			} else if a.len() == 1 {
				format!("[{}]", preview_value(&a[0]))
			} else if a.len() == 2
				&& a.iter().all(|e| {
					matches!(
						e,
						serde_json::Value::Number(_) | serde_json::Value::String(_)
					)
				}) {
				// Compact range-like pairs (e.g. lines [1, 150]) — show both values
				format!("[{}, {}]", preview_value(&a[0]), preview_value(&a[1]))
			} else {
				format!("[{}, +{}]", preview_value(&a[0]), a.len() - 1)
			}
		}
		serde_json::Value::Object(_) => "{…}".to_string(),
		serde_json::Value::Null => "null".to_string(),
		_ => v.to_string(),
	}
}

// Upfront preview before execution. Fires for any number of tools — single
// or parallel — so the display is always consistent.
//
// Layout:
//
//   ╭ tools
//   │ ▸ tool1 · server
//   │     key  value
//   │     key  value
//   │ ▸ tool2 · server
//   │     key  value
//   ╰ N queued
//
// Indented params let parallel `view` calls be distinguished without an
// inline `(k=v, k=v)` that would truncate badly for long paths.
async fn display_tool_preview(config: &Config, tool_calls: &[crate::mcp::McpToolCall]) {
	if tool_calls.is_empty() {
		return;
	}

	let sep = "·".bright_black();
	let rail = "│".bright_black();
	// One indent step under `│ ▸` — keeps params visually nested without
	// burning horizontal space.
	let param_prefix = format!("{}  ", rail);

	println!("{} {}", "╭".bright_cyan(), "tools".bright_cyan());
	for call in tool_calls {
		let server_name = get_tool_server_name_async(&call.tool_name, config).await;
		println!(
			"{} {} {} {} {}",
			rail,
			"▸".bright_cyan(),
			call.tool_name.bright_cyan(),
			sep,
			server_name.bright_blue(),
		);
		if let Ok(params) = serde_json::from_value::<serde_json::Map<String, serde_json::Value>>(
			call.parameters.clone(),
		) {
			let key_width = params.keys().map(|k| k.len()).max().unwrap_or(0).min(20);
			for (k, v) in params.iter() {
				print_preview_param(&param_prefix, k, key_width, v);
			}
		}
	}
	println!(
		"{} {} {}",
		"╰".bright_cyan(),
		tool_calls.len(),
		"queued".bright_black(),
	);
	println!();
}

// Helper function to resolve current tool calls
fn resolve_tool_calls(
	current_tool_calls_param: &mut Option<Vec<crate::mcp::McpToolCall>>,
	_current_content: &str,
) -> Vec<crate::mcp::McpToolCall> {
	// Earlier fallback parsed tool calls out of raw response text. All providers
	// now return structured `tool_calls`, so the text-parse fallback only ever
	// returned an empty Vec. Behave the same way without the dead call.
	current_tool_calls_param.take().unwrap_or_default()
}

// Helper function to check for cancellation
fn check_cancellation(operation_cancelled: &tokio::sync::watch::Receiver<bool>) -> Result<()> {
	if *operation_cancelled.borrow() {
		crate::log_debug!("Operation cancelled by user.");
		return Err(anyhow::Error::new(crate::session::cancellation::Cancelled));
	}
	Ok(())
}

// Helper function to add assistant message with tool calls preserved
fn add_assistant_message_with_tool_calls(
	chat_session: &mut ChatSession,
	current_content: &str,
	current_exchange: &ProviderExchange,
	response_id: Option<String>,
	thinking: &Option<ThinkingBlock>,
	_config: &Config,
	_role: &str,
) -> Result<()> {
	// CRITICAL FIX: We need to add the assistant message with tool_calls PRESERVED
	// The standard add_assistant_message only stores text content, but we need
	// to preserve the tool_calls from the original API response for proper conversation flow

	// Extract the original tool_calls from the exchange response based on provider
	let original_tool_calls = MessageHandler::extract_original_tool_calls(current_exchange);

	// Create the assistant message directly with tool_calls preserved from the exchange
	let thinking_value = thinking
		.as_ref()
		.and_then(|block| serde_json::to_value(block).ok());

	let assistant_message = crate::session::Message {
		role: "assistant".to_string(),
		content: current_content.to_string(),
		timestamp: std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.unwrap_or_default()
			.as_secs(),
		cached: false,
		cache_ttl: None,
		tool_call_id: None,
		name: None,
		tool_calls: original_tool_calls.clone(),
		images: None,
		videos: None,
		thinking: thinking_value,
		id: response_id.clone(),
	};

	// ATOMIC ADD: persist BEFORE pushing to in-memory Vec.
	// If persist fails (e.g. ENOSPC), `?` propagates with clean memory state — no
	// orphaned assistant(tool_calls=...) without matching tool_results, which would
	// otherwise corrupt the conversation for Anthropic ("tool_use ids found without
	// tool_result"). Tool execution + process_tool_results only runs after this returns Ok.
	if let Some(session_file) = &chat_session.session.session_file {
		let message_json = serde_json::to_string(&assistant_message)?;
		crate::session::append_to_session_file(session_file, &message_json)?;
	}
	chat_session.session.messages.push(assistant_message);

	// Update last response - no cost tracking here as it will be handled by follow-up processing
	chat_session.last_response = current_content.to_string();

	// CRITICAL FIX: DO NOT track cost/tokens here - already tracked by CostTracker::track_exchange_cost()
	// in api_executor.rs:163. Tracking here causes DUPLICATE cost/token counting.

	Ok(())
}

// Function to process response, handling tool calls recursively
pub async fn process_response<S: OutputSink>(
	params: ResponseProcessingParams<'_, S>,
) -> Result<()> {
	// Check if operation has been cancelled at the very start
	check_cancellation(&params.operation_cancelled)?;

	// Note: No explicit stop needed here. The spinner-aware print macros in
	// src/lib.rs use pb.suspend() around every println!/print!, which is
	// indicatif's documented safe way to interleave output with a live spinner.
	// The persistent bar stays up until a genuine turn boundary.

	// Debug logging for finish_reason and tool calls
	log_response_debug(params.config, &params.finish_reason, &params.tool_calls);

	// First, add the user message before processing response
	let last_message = params.chat_session.session.messages.last();
	if params.mode.is_terminal_mode() && last_message.is_none_or(|msg| msg.role != "user") {
		// This is an edge case - the content variable here is the AI response, not user input
		// We should have added the user message earlier in the main run_interactive_session
		println!(
			"{}",
			"Warning: User message not found in session. This is unexpected.".yellow()
		);
	}

	// Initialize tool processor
	let mut tool_processor = ToolProcessor::new();

	// Track if thinking has been displayed (to avoid displaying twice)
	let mut thinking_displayed = false;

	// Process original content first, then any follow-up tool calls
	let mut current_content = params.content.clone();
	let mut current_exchange = params.exchange.clone(); // Clone to avoid moving params
	let mut current_tool_calls_param = params.tool_calls.clone(); // Track of tool_calls parameter
	let mut current_response_id = params.response_id.clone(); // Track response_id through iterations
	let mut current_thinking = params.thinking.clone(); // Track thinking only for the current response
	let mut last_emitted_thinking: Option<String> = None;
	let operation_cancelled_ref = &params.operation_cancelled; // Create a reference to avoid moves

	loop {
		// Check for cancellation at the start of each loop iteration
		check_cancellation(operation_cancelled_ref)?;

		// Check for tool calls if MCP has any servers configured
		if !params.config.mcp.servers.is_empty() {
			// Resolve current tool calls for this iteration
			let current_tool_calls =
				resolve_tool_calls(&mut current_tool_calls_param, &current_content);

			if !current_tool_calls.is_empty() {
				let session_id = params.chat_session.session.info.name.clone();
				if params.mode.should_suppress_cli_output() {
					if let Some(ref thinking_block) = current_thinking {
						if last_emitted_thinking.as_deref() != Some(thinking_block.content.as_str())
						{
							emit_thinking_event(&params, thinking_block, &session_id);
							last_emitted_thinking = Some(thinking_block.content.clone());
						}
					}
				}

				// Display thinking first if present and not yet displayed - ONLY in interactive mode
				if params.mode.is_interactive() && !thinking_displayed {
					if let Some(ref thinking_block) = current_thinking {
						display_thinking(thinking_block);
						thinking_displayed = true;
					}
				}

				// Display the content to the user FIRST (before adding to session) - ONLY in interactive mode
				// Skip if using structured output (JSONL/WebSocket - handled by sink)
				if params.mode.is_interactive() {
					print_assistant_response(
						&current_content,
						params.config,
						params.role,
						&current_thinking,
					);
				}

				// Upfront preview: full `╭ tool · server` block + params per tool
				// (NEW format, no `╰` yet — block is "open"). When each result
				// arrives, the result section prints the output + `╰ ✓ tool …`
				// close line without re-rendering the header. For long-running
				// or hung tools, the user always sees what's currently running.
				if params.mode.is_interactive() {
					display_tool_preview(params.config, &current_tool_calls).await;
				}

				// Start animation during tool execution so the user sees progress feedback.
				// The animation is stopped inside execute_tools_with_context before any
				// tool output is printed, preventing ghost spinners.
				if params.mode.is_interactive() {
					use crate::session::chat::get_animation_manager;
					get_animation_manager().start_animation(&params.mode).await;
				}

				// Clone operation_cancelled to avoid borrow issues
				let operation_cancelled_clone = params.operation_cancelled.clone();

				// Early exit if cancellation was requested BEFORE adding message
				if *operation_cancelled_clone.borrow() {
					crate::log_debug!("Operation cancelled by user.");
					// Do NOT add any message to the session since tools weren't executed
					return Ok(());
				}

				// Execute all tool calls in parallel using the new module

				// Emit ToolUse notifications before execution so ACP/WebSocket clients
				// can register the tool call ID before the result arrives.
				if params.mode.should_suppress_cli_output() {
					for call in &current_tool_calls {
						let server =
							get_tool_server_name_async(&call.tool_name, params.config).await;
						params.emit(ServerMessage::ToolUse(ToolUsePayload {
							tool: call.tool_name.clone(),
							tool_id: call.tool_id.clone(),
							server,
							params: call.parameters.clone(),
							session_id: session_id.clone(),
						}));
					}
				}
				let (tool_results, total_tool_time_ms) =
					match tool_execution::execute_tools_parallel(
						current_tool_calls.clone(),
						params.chat_session,
						params.config,
						&mut tool_processor,
						operation_cancelled_clone.clone(),
						params.mode,
					)
					.await
					{
						Ok(results) => results,
						Err(e) => {
							// Check if this was a cancellation
							if crate::session::cancellation::is_cancelled(&e)
								|| *operation_cancelled_clone.borrow()
							{
								crate::log_debug!("Operation cancelled by user.");
								// Don't add assistant message since tools weren't executed
								return Ok(());
							}
							return Err(e);
						}
					};

				// Emit tool results through sink (WebSocket/JSONL)
				let session_id = params.chat_session.session.info.name.clone();
				for tool_result in &tool_results {
					let actual_content = tool_result.extract_content();
					let success = !tool_result.is_error();
					let tool_msg = ServerMessage::ToolResult(ToolResultPayload {
						tool: tool_result.tool_name.clone(),
						tool_id: tool_result.tool_id.clone(),
						server: crate::session::chat::response::get_tool_server_name_async(
							&tool_result.tool_name,
							params.config,
						)
						.await,
						content: actual_content,
						success,
						session_id: session_id.clone(),
					});
					params.emit(tool_msg);
				}

				// Check for cancellation BEFORE adding assistant message
				if *operation_cancelled_clone.borrow() {
					if params.mode.is_terminal_mode() {
						println!("{}", "\nTool execution cancelled.".bright_yellow());
					}
					// Don't add assistant message since tools were cancelled
					return Ok(());
				}

				// ONLY add assistant message if tools were NOT cancelled
				add_assistant_message_with_tool_calls(
					params.chat_session,
					&current_content,
					&current_exchange,
					current_response_id.clone(), // CRITICAL FIX: Use current_response_id from loop, not params.response_id
					&current_thinking,
					params.config,
					params.role,
				)?;

				// Process tool results if any exist
				if !tool_results.is_empty() {
					// Process tool results and handle follow-up API calls using the new module
					if let Some((
						new_content,
						new_exchange,
						new_tool_calls,
						new_response_id,
						new_thinking,
					)) = tool_result_processor::process_tool_results(
						tool_results,
						total_tool_time_ms,
						params.chat_session,
						params.config,
						params.role,
						operation_cancelled_clone.clone(),
					)
					.await?
					{
						// Update current content for next iteration
						current_content = new_content;
						current_exchange = new_exchange;
						current_tool_calls_param = new_tool_calls;
						current_response_id = new_response_id; // Update response_id from follow-up response
											 // CRITICAL FIX: Preserve thinking from follow-up response for Moonshot
											 // Moonshot requires reasoning_content for ALL assistant messages with tool calls
						current_thinking = new_thinking;

						// Continue when the follow-up surfaced more structured tool_calls;
						// otherwise the loop is done. (The earlier text-parse fallback never
						// returned anything, so no inline-content branch is needed.)
						if current_tool_calls_param.is_some()
							&& !current_tool_calls_param.as_ref().unwrap().is_empty()
						{
							continue;
						} else {
							break;
						}
					} else {
						// No follow-up response (cancelled or error), exit
						return Ok(());
					}
				} else {
					// No tool results and no follow-up tools to execute — done.
					break;
				}
			} else {
				// No tool calls in this content, break out of the loop
				break;
			}
		} else {
			// MCP not enabled, break out of the loop
			break;
		}
	}

	// Handle final response using the helper function (only when no tool calls are pending)
	// When tool calls are present, we already created an assistant message with add_assistant_message_with_tool_calls
	// Calling handle_final_response would create a duplicate assistant message without id
	// Pass thinking only if it hasn't been displayed yet (in tool call loop)
	let session_id = params.chat_session.session.info.name.clone();
	let thinking_for_final = if thinking_displayed {
		None
	} else {
		current_thinking.clone()
	};
	if params.mode.should_suppress_cli_output() {
		if let Some(ref thinking_block) = thinking_for_final {
			if last_emitted_thinking.as_deref() != Some(thinking_block.content.as_str()) {
				emit_thinking_event(&params, thinking_block, &session_id);
			}
		}
	}

	// Emit assistant message through sink (WebSocket/JSONL)
	params.emit(ServerMessage::Assistant(AssistantPayload {
		content: current_content.clone(),
		session_id: session_id.clone(),
	}));

	handle_final_response(
		&current_content,
		&thinking_for_final,
		current_response_id, // Use current_response_id (updated from follow-up responses)
		params.chat_session,
		params.config,
		params.role,
		params.mode,
	)?;

	// Run skill validators on assistant response.
	// Failures are pushed to the inbox so the session loop (interactive or
	// non-interactive) auto-continues with a new API turn — without this,
	// validator errors would sit in memory and the loop would block waiting
	// for user input.
	{
		let workdir = crate::mcp::get_thread_working_directory();
		let failures =
			crate::mcp::core::skill_auto::run_validators(&current_content, &workdir).await;
		for (skill_name, error) in &failures {
			// Wrap in <validation> tags so skill auto-activation (strip_xml_blocks)
			// ignores this injected message — it must not match user-intent skills.
			let error_msg = format!(
				"<validation skill=\"{}\">\nValidation failed: {}\nPlease fix the issue.\n</validation>",
				skill_name, error
			);
			crate::session::inbox::push_inbox_message(crate::session::inbox::InboxMessage {
				source: crate::session::inbox::InboxSource::SkillValidator {
					name: skill_name.clone(),
				},
				content: error_msg,
			});
			log_info!("Validator '{}' failed on assistant event", skill_name);
		}
	}

	// Run guardrail `[[validator]]` scripts. Filters short-circuit in this
	// order: role → `when` over call_log slice → `match` regex on assistant
	// text. Survivors spawn their script in parallel; non-zero exits inject
	// `<validation validator="…">…</validation>` into the inbox.
	crate::session::hooks::run_turn_validators(
		&session_id,
		params.role,
		&current_content,
	)
	.await;

	// 🗜️ DEFERRED PLAN COMPRESSION: Process plan(done) compression after assistant message
	// When plan(done) triggers compression, we defer it to here so the follow-up API call
	// (which generates the "plan completed" response) benefits from cache hits on the
	// unmodified conversation. Compression runs now — after the response is displayed —
	// so the session saves with compressed state for the next user turn.
	if crate::mcp::core::plan::has_pending_project_compression() {
		log_debug!("Processing deferred plan(done) compression after assistant response");

		// Task compression (forced for last task)
		match crate::mcp::core::plan::process_pending_compression(params.chat_session).await {
			Ok(Some(metrics)) => {
				params
					.chat_session
					.session
					.info
					.compression_stats
					.add_compression(
						crate::session::CompressionKind::Task,
						metrics.messages_removed,
						metrics.tokens_saved,
					);
				CostTracker::display_compression_result("Task", &metrics);
			}
			Ok(None) => {}
			Err(e) => {
				log_debug!("Deferred task compression failed: {}. Continuing.", e);
			}
		}

		// Phase compression
		match crate::mcp::core::plan::process_pending_phase_compression(params.chat_session).await {
			Ok(Some(metrics)) => {
				params
					.chat_session
					.session
					.info
					.compression_stats
					.add_compression(
						crate::session::CompressionKind::Phase,
						metrics.messages_removed,
						metrics.tokens_saved,
					);
				CostTracker::display_compression_result("Phase", &metrics);
			}
			Ok(None) => {}
			Err(e) => {
				log_debug!("Deferred phase compression failed: {}. Continuing.", e);
			}
		}

		// Project compression
		match crate::mcp::core::plan::process_pending_project_compression(params.chat_session).await
		{
			Ok(Some(metrics)) => {
				params
					.chat_session
					.session
					.info
					.compression_stats
					.add_compression(
						crate::session::CompressionKind::Project,
						metrics.messages_removed,
						metrics.tokens_saved,
					);
				CostTracker::display_compression_result("Project", &metrics);
			}
			Ok(None) => {}
			Err(e) => {
				log_debug!("Deferred project compression failed: {}. Continuing.", e);
			}
		}
	}

	// Emit cost message through sink (WebSocket/JSONL)
	let total_tokens = params.chat_session.session.info.input_tokens
		+ params.chat_session.session.info.output_tokens
		+ params.chat_session.session.info.cache_read_tokens
		+ params.chat_session.session.info.cache_write_tokens
		+ params.chat_session.session.info.reasoning_tokens;
	let cost_msg = ServerMessage::Cost(CostPayload {
		session_tokens: total_tokens,
		session_cost: params.chat_session.session.info.total_cost,
		input_tokens: params.chat_session.session.info.input_tokens,
		output_tokens: params.chat_session.session.info.output_tokens,
		cache_read_tokens: params.chat_session.session.info.cache_read_tokens,
		cache_write_tokens: params.chat_session.session.info.cache_write_tokens,
		reasoning_tokens: params.chat_session.session.info.reasoning_tokens,
		session_id,
	});

	params.emit(cost_msg);

	// Inject compression hint if applicable (non-intrusive, appended to response)
	// Skip if using structured output (JSONL/WebSocket - handled by sink)
	if params.mode.is_terminal_mode() {
		if let Some(hint) = params.chat_session.get_compression_hint(params.config) {
			println!("{}", hint);
		}
	}
	Ok(())
}
