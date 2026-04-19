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

// Main session loop - orchestrates all session operations

use super::super::commands::{MODEL_COMMAND, ROLE_COMMAND, SESSION_COMMAND};
use super::super::input::{calculate_current_context_tokens, read_user_input, InputResult};
use super::super::CostTracker;
use super::api_executor::execute_api_call_and_process_response;
use super::api_prep::prepare_for_api_call;
use super::commands::CommandResult;
use super::core::{ChatSession, SessionInitParams};
use super::error_utils::handle_api_error;
use super::layer_processor::process_layers_if_enabled;
use super::prompt_setup::setup_system_prompt_and_cache;
use super::setup::setup_and_initialize_session;
use crate::config::Config;
use crate::session::cancellation::SessionCancellation;
use crate::session::output::{JsonlSink, OutputMode, SilentSink};
use crate::{log_debug, log_info};
use anyhow::Result;
use colored::*;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
// Helper function to print command output in CLI context
async fn print_command_output(
	output: &mut super::commands::CommandOutput,
	session: &mut ChatSession,
	config: &Config,
) {
	output.display_cli(session, config).await;
}

/// Extract webhook hook names from the Debug representation of session args.
fn extract_hooks_from_args<T: std::fmt::Debug>(args: &T) -> Vec<String> {
	let args_str = format!("{:?}", args);
	// Match: hooks: ["name1", "name2"]
	if let Some(start) = args_str.find("hooks: [") {
		let inner_start = start + 8;
		if let Some(end) = args_str[inner_start..].find(']') {
			let hooks_str = &args_str[inner_start..inner_start + end];
			return hooks_str
				.split(',')
				.filter_map(|s| {
					let trimmed = s.trim().trim_matches('"');
					if trimmed.is_empty() {
						None
					} else {
						Some(trimmed.to_string())
					}
				})
				.collect();
		}
	}
	Vec::new()
}

/// Start webhook listeners for all activated hooks.
/// Returns RAII guards that stop the listeners on drop.
async fn start_webhook_guards<T: std::fmt::Debug>(
	args: &T,
	config: &Config,
	session_name: &str,
) -> Result<Vec<crate::session::webhook_listener::WebhookListenerGuard>> {
	let hook_names = extract_hooks_from_args(args);
	let mut guards = Vec::with_capacity(hook_names.len());
	for hook_name in &hook_names {
		let hook_config = config.get_hook_by_name(hook_name).ok_or_else(|| {
			anyhow::anyhow!(
				"Hook '{}' not found in config. Define it in [[hooks]].",
				hook_name
			)
		})?;
		let (addr, script_path) = crate::session::webhook_listener::validate_hook(hook_config)?;
		guards.push(
			crate::session::webhook_listener::start_webhook_listener(
				session_name,
				hook_config,
				addr,
				script_path,
			)
			.await?,
		);
	}
	Ok(guards)
}

// Run an interactive session
pub async fn run_interactive_session<T: std::fmt::Debug>(args: &T, config: &Config) -> Result<()> {
	// Setup and initialize session using helper function

	let (chat_session, config_for_role, role, first_message_processed, spinner) =
		setup_and_initialize_session(args, config).await?;

	let session_id = chat_session.session.info.name.clone();
	crate::session::context::with_session_id(session_id, async move {
		// MCP init (spinner still running from setup)
		// MCP init with progress on spinner
		{
			use std::sync::{Arc, Mutex};
			let pending: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
			let total: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));
			let sp_ref = spinner.clone();

			let cb = move |progress: crate::mcp::McpInitProgress| {
				let Some(sp) = sp_ref.as_ref() else { return };
				match &progress {
					crate::mcp::McpInitProgress::Starting { servers } => {
						*total.lock().unwrap() = servers.len();
						*pending.lock().unwrap() = servers.clone();
						if !servers.is_empty() {
							sp.set_message(format!(
								"Starting MCP: {} [0/{}]",
								servers.join(", "),
								servers.len()
							));
						}
					}
					crate::mcp::McpInitProgress::Completed { server, .. } => {
						let mut pg = pending.lock().unwrap();
						pg.retain(|s| s != server);
						let done = *total.lock().unwrap() - pg.len();
						let tc = *total.lock().unwrap();
						if pg.is_empty() {
							sp.set_message(format!("MCP ready [{}/{}]", done, tc));
						} else {
							sp.set_message(format!(
								"Starting MCP: {} [{}/{}]",
								pg.join(", "),
								done,
								tc
							));
						}
					}
				}
			};
			crate::mcp::initialize_mcp_for_role_with_callback(&role, config, Some(&cb)).await?;
		}

		crate::session::context::init_session_services(&role);

		let _inject_listener =
			crate::session::inject_listener::start_inject_listener(&chat_session.session.info.name);
		let _webhook_guards =
			start_webhook_guards(args, config, &chat_session.session.info.name).await?;
		let current_dir = crate::mcp::get_thread_working_directory();
		let mut chat_session = chat_session;
		let mut first_message_processed = first_message_processed;

		setup_system_prompt_and_cache(&mut chat_session, &config_for_role, &role, true).await?;

		crate::mcp::core::skill_auto::load_env_skills(&mut chat_session).await;

		// Done initializing — clear spinner, print skills
		if let Some(sp) = spinner {
			// Print skills through spinner before clearing
			if let Some(sid) = crate::session::context::current_session_id() {
				let active = crate::session::context::get_active_skills(&sid);
				for name in &active {
					sp.println(format!(
						"{} {}",
						"Using skill:".dimmed(),
						name.bright_cyan()
					));
				}
			}
			sp.disable_steady_tick();
			sp.finish_and_clear();
			print!("\x1B[2K\r");
			std::io::Write::flush(&mut std::io::stdout()).ok();
			drop(sp);
		}

		// Print the last few messages for context with colors if terminal supports them (for resumed sessions)
		// Only show context for truly resumed sessions, not new sessions
		if chat_session.was_resumed {
			let last_messages = chat_session
				.session
				.messages
				.iter()
				.rev()
				.take(3)
				.collect::<Vec<_>>();

			for msg in last_messages.iter().rev() {
				if msg.role == "assistant" {
					println!("{}", msg.content.bright_green());
				} else if msg.role == "tool" {
					log_debug!(msg.content);
				} else if msg.role == "user" {
					println!("> {}", msg.content.bright_blue());
				}
			}
		}

		// Set up advanced cancellation system for proper CTRL+C handling
		// Enhanced processing state tracking for smart cancellation
		#[derive(Debug, Clone, PartialEq)]
		enum ProcessingState {
			Idle,                 // No operation in progress
			ReadingInput,         // Reading user input
			CallingAPI,           // Making API call (includes layers, tools, response processing)
			CompletedWithResults, // Completed successfully with results to keep
		}

		let processing_state = Arc::new(std::sync::Mutex::new(ProcessingState::Idle));
		let _processing_state_clone = processing_state.clone();

		// Smart operation tracking for surgical cleanup
		#[derive(Debug, Clone)]
		struct OperationContext {
			user_message_index: Option<usize>,
			assistant_message_index: Option<usize>, // Track when assistant message is added
		}

		let current_operation = Arc::new(std::sync::Mutex::new(None::<OperationContext>));

		// Create the cancellation manager for this session
		let mut cancellation = SessionCancellation::new();
		let _ctrl_c_count = Arc::new(AtomicBool::new(false)); // Keep for now

		// Start signal handler
		let _signal_handler = cancellation.start_signal_handler();

		// We need to handle configuration reloading, so keep our own copy that we can update
		let mut current_config = config_for_role.clone();

		// Apply runtime state from session log if this is a resumed session
		if chat_session.was_resumed {
			if let Some(session_file) = &chat_session.session.session_file {
				if let Ok(runtime_state) =
					crate::session::extract_runtime_state_from_log(session_file)
				{
					// Workflow state is now stored in role config, not runtime state
					// This section is kept for backward compatibility but does nothing
					if let Some(_workflow_enabled) = runtime_state.layers_enabled {
						log_info!(
							"Legacy layers_enabled state ignored - using workflow configuration"
						);
					}
				}
			}
		}

		// Set the thread-local config for logging macros
		crate::config::set_thread_config(&current_config);
		crate::config::set_thread_role(&role);
		// Main interaction loop
		loop {
			// SMART CANCELLATION: Handle cancellation with surgical cleanup
			// CRITICAL: Check cancellation BEFORE resetting processing state
			// This ensures cleanup logic sees the correct state (e.g., CallingAPI)
			if cancellation.is_cancelled() {
				log_debug!("Ctrl+C detected - performing smart cleanup based on operation state");

				// CRITICAL FIX: Stop animation IMMEDIATELY before any cleanup
				// This ensures the spinner stops instantly and user sees clean prompt
				use crate::session::chat::get_animation_manager;
				let animation_manager = get_animation_manager();
				animation_manager.stop_current().await;

				// CRITICAL FIX: Use suspend to prevent ghost lines
				// This ensures spinner is properly hidden before cost display
				animation_manager.with_suspended_spinner(|| {
					// Display cost information before cleanup
					// This ensures users see the cost spent before cancellation
					// Skip in JSONL mode
					if !current_config.output_mode().should_suppress_cli_output() {
						CostTracker::display_cost_line(&chat_session);
					}
				});

				let current_state = processing_state.lock().unwrap().clone();
				let operation = current_operation.lock().unwrap().clone();

				match current_state {
					ProcessingState::Idle | ProcessingState::ReadingInput => {
						// Nothing to clean up - just reset and continue
						log_debug!("Cancelled during idle state - no cleanup needed");
					}
					ProcessingState::CallingAPI => {
						// API call was interrupted - determine if we're in multi-turn conversation
						// Multi-turn = tools were already executed and we're processing follow-up response
						// Check: Are there ANY tool messages in the current operation's context?
						if let Some(op) = operation {
							// Check if there are tool messages AFTER the user message for this operation
							// This indicates tools were executed and we're in a follow-up API call
							let user_idx = op.user_message_index.unwrap_or(0);
							let has_tool_messages = chat_session
								.session
								.messages
								.iter()
								.skip(user_idx)
								.any(|msg| msg.role == "tool");

							if has_tool_messages {
								// MULTI-TURN: Tools were executed, conversation state is valid
								// Keep EVERYTHING - user message, assistant message, tool results
								log_debug!("Ctrl+C during multi-turn (tools executed) - preserving all conversation state");
							} else {
								// FIRST CALL: No tools executed yet
								// Remove user message (and assistant if added) for clean retry
								if let Some(user_idx) = op.user_message_index {
									if user_idx < chat_session.session.messages.len() {
										chat_session.session.messages.truncate(user_idx);
										log_debug!("Ctrl+C during first API call - removed user message for clean retry");
									}
								}
							}
						}
					}
					ProcessingState::CompletedWithResults => {
						// Operation completed successfully - nothing to clean up
						log_debug!("Cancelled after completion - all work preserved");
					}
				}

				// Save the session after cleanup to persist changes
				// PHASE 4: Robust save with retry and error reporting
				// CRITICAL FIX: Write TRUNCATION_POINT marker to session file
				// This tells the loader to discard messages after this point on resume
				if let Some(session_file) = &chat_session.session.session_file {
					let truncation_entry = serde_json::json!({
						"type": "TRUNCATION_POINT",
						"timestamp": std::time::SystemTime::now()
							.duration_since(std::time::UNIX_EPOCH)
							.unwrap_or_default()
							.as_secs(),
						"message_count": chat_session.session.messages.len(),
						"reason": "ctrl_c_cleanup"
					});
					if let Err(e) = crate::session::append_to_session_file(
						session_file,
						&serde_json::to_string(&truncation_entry).unwrap_or_default(),
					) {
						log_debug!("Failed to write TRUNCATION_POINT: {}", e);
					}
				}

				if let Err(e) = chat_session.save() {
					use colored::*;
					if !crate::logging::tracing_setup::is_structured_output_mode() {
						eprintln!();
						eprintln!(
							"{}",
							"⚠️  CRITICAL: Failed to save session after cleanup"
								.bright_red()
								.bold()
						);
						eprintln!("{} {}", "Error:".bright_yellow(), e);
						eprintln!(
							"{}",
							"Session state may be inconsistent on resume.".bright_yellow()
						);
						eprintln!();
					} else {
						crate::log_error!("CRITICAL: Failed to save session after cleanup: {}", e);
					}

					// Attempt one retry
					log_debug!("Retrying session save after failure...");
					if let Err(retry_err) = chat_session.save() {
						if !crate::logging::tracing_setup::is_structured_output_mode() {
							eprintln!(
								"{}",
								"⚠️  Retry failed. Session may be corrupted.".bright_red()
							);
						} else {
							crate::log_error!("Retry save failed: {}", retry_err);
						}
						log_debug!("Retry save failed: {}", retry_err);
					} else if !crate::logging::tracing_setup::is_structured_output_mode() {
						eprintln!("{}", "✓ Retry succeeded - session saved.".bright_green());
					}
				}

				// Clear operation context
				*current_operation.lock().unwrap() = None;

				// CRITICAL FIX: Reset cancellation state BEFORE continuing
				// This prevents infinite loop where cancellation is always true
				cancellation.reset();
				log_debug!("Cancellation state reset - ready for new operation");

				continue;
			}

			// CRITICAL: Reset processing state to Idle AFTER cancellation cleanup
			// This ensures cleanup logic sees the correct state during cancellation
			*processing_state.lock().unwrap() = ProcessingState::Idle;

			// Set state to reading input
			*processing_state.lock().unwrap() = ProcessingState::ReadingInput;

			// Flush any due schedule entries into the inbox so all injection sources
			// are unified — the loop only needs to drain the inbox from here on.
			crate::mcp::core::flush_due_to_inbox();

			// Pop the first pending inbox message (background agent, schedule, skill, /prompt).
			// If one is ready, skip user input entirely and process it immediately.
			let pending_msg = crate::session::inbox::try_pop_inbox_message();

			// Get a new operation token for this iteration
			let operation_rx = cancellation.new_operation();

			// Calculate current context tokens for display
			let current_context_tokens = calculate_current_context_tokens(
				&chat_session.session.messages,
				&current_config,
				&role,
			)
			.await;

			let input_result = if let Some(inbox_msg) = pending_msg {
				// An inbox message is ready — inject it without waiting for user input.
				log_debug!("Processing inbox message from {:?}", inbox_msg.source);
				InputResult::Text(inbox_msg.content)
			} else {
				// Reedline blocks for user input. A shared slot lets the
				// inbox-notification thread (inside read_user_input) show a
				// preview above the prompt when an inbox entry arrives.  The
				// user then presses Enter to accept it.
				let inbox_pending: Arc<std::sync::Mutex<Option<String>>> =
					Arc::new(std::sync::Mutex::new(None));

				// Spawn a task that watches the inbox notify and sets the preview.
				let inbox_notify = crate::session::inbox::get_inbox_notify();
				let slot_for_notify = inbox_pending.clone();
				let session_name_for_peek = chat_session.session.info.name.clone();
				let notify_task = tokio::spawn(async move {
					if let Some(notify) = inbox_notify {
						loop {
							notify.notified().await;
							// Flush schedule entries so the inbox has the message ready.
							crate::mcp::core::flush_due_to_inbox();
							// Verify the inbox actually has a message — the permit
							// may be stale from a previous iteration where
							// try_pop_inbox_message() already consumed the entry.
							if let Some(preview) =
								crate::session::inbox::peek_inbox_preview(&session_name_for_peek)
							{
								if let Ok(mut guard) = slot_for_notify.lock() {
									*guard = Some(preview);
								}
								break;
							}
							// Stale permit — go back to waiting for a real message.
						}
					}
				});

				let estimated_cost = chat_session.estimated_cost;
				let config_clone = current_config.clone();
				let role_clone = role.clone();
				let session_name = chat_session.session.info.name.clone();
				let max_threshold = current_config.max_session_tokens_threshold;
				let inbox_pending_clone = inbox_pending.clone();

				let result = tokio::task::spawn_blocking(move || {
					read_user_input(
						estimated_cost,
						&config_clone,
						&role_clone,
						current_context_tokens,
						max_threshold,
						&session_name,
						inbox_pending_clone,
					)
				})
				.await
				.unwrap_or_else(|e| {
					log_debug!("Input thread panicked: {}", e);
					Ok(InputResult::Text(String::new()))
				})?;

				// Clean up the notify watcher — no longer needed this iteration.
				notify_task.abort();

				result
			};

			// Handle the input result with proper error recovery
			let mut input = match input_result {
				InputResult::Text(text) => text,
				InputResult::AddWithoutSending(text) => {
					// Ctrl+G pressed - add message to context without sending

					// Skip if input is empty
					if text.trim().is_empty() {
						continue;
					}

					// Add the message to session context
					chat_session.add_user_message(&text)?;

					// Save the session to persist the added message
					if let Err(e) = chat_session.save() {
						log_debug!(
							"Warning: Failed to save session after adding message: {}",
							e
						);
					}

					// Provide feedback to user
					println!("{}", "✓ Message added to context".bright_cyan());

					// Continue to next input without sending to API
					continue;
				}
				InputResult::Cancelled => {
					// Ctrl+C pressed during input
					log_debug!("Input cancelled by user - cleaning up");

					// Kill any running async jobs
					if let Some(manager) = crate::mcp::agent::functions::get_job_manager() {
						manager.kill_all();
					}

					// Ensure session is saved
					if let Err(e) = chat_session.save() {
						log_debug!("Warning: Failed to save session after cancellation: {}", e);
					}
					continue;
				}
				InputResult::Exit => {
					// Ctrl+D pressed - graceful exit handled in input.rs
					// Fire-and-forget learning extraction on exit (skip if /done already extracted)
					if current_config.learning.enabled && !chat_session.learning_extracted {
						let project = current_dir
							.file_name()
							.and_then(|n| n.to_str())
							.unwrap_or("unknown")
							.to_string();
						crate::learning::extract::extract_lessons_detached(
							chat_session.session.messages.clone(),
							current_config.clone(),
							role.clone(),
							project,
							chat_session.session.info.name.clone(),
						);
					}
					// Kill any running async jobs
					if let Some(manager) = crate::mcp::agent::functions::get_job_manager() {
						manager.kill_all();
					}
					// Ensure session is saved
					if let Err(e) = chat_session.save() {
						log_debug!("Warning: Failed to save session: {}", e);
					}
					break;
				}
			};

			// Check if the input is an exit command
			if input == "/exit" || input == "/quit" {
				// Fire-and-forget learning extraction on exit (skip if /done already extracted)
				if current_config.learning.enabled && !chat_session.learning_extracted {
					let project = current_dir
						.file_name()
						.and_then(|n| n.to_str())
						.unwrap_or("unknown")
						.to_string();
					crate::learning::extract::extract_lessons_detached(
						chat_session.session.messages.clone(),
						current_config.clone(),
						role.clone(),
						project,
						chat_session.session.info.name.clone(),
					);
				}
				// Kill any running async jobs before exiting
				if let Some(manager) = crate::mcp::agent::functions::get_job_manager() {
					manager.kill_all();
				}
				// Show resume command with session ID
				let resume_cmd =
					format!("octomind run --resume {}", chat_session.session.info.name)
						.bright_cyan();
				println!("\nTo continue this session, run: {}", resume_cmd);
				break;
			}

			// Skip if input is empty — unless an inbox message is waiting.
			if input.trim().is_empty() {
				// User pressed Enter on empty prompt. Check if an inbox
				// message arrived while they were at the prompt.
				crate::mcp::core::flush_due_to_inbox();
				if crate::session::inbox::has_inbox_messages() {
					if let Some(msg) = crate::session::inbox::try_pop_inbox_message() {
						log_debug!("Processing inbox message from {:?}", msg.source);
						input = msg.content;
					}
				}
				if input.trim().is_empty() {
					continue;
				}
			}

			// Initialize request spending tracking at the start of each request
			chat_session.start_request_spending_tracking();

			// Immediate feedback - show that we received the input
			// This reduces perceived latency by giving instant visual feedback
			if !input.starts_with('/') {
				// Flush stdout to ensure prompt is cleared immediately
				print!("\r\x1B[K");
				std::io::Write::flush(&mut std::io::stdout()).unwrap_or(());
			}

			// Check if this is a command
			if input.starts_with('/') {
				// Handle special /done command separately
				if input.trim() == "/done" {
					// Handle /done command using dedicated handler
					match super::commands::handle_done(
						&mut chat_session,
						&current_config,
						operation_rx.clone(),
					)
					.await
					{
						Ok((exit_flag, reset_first_message)) => {
							if reset_first_message {
								// Reset first_message_processed to false so that the next message goes through layers again
								first_message_processed = false;
							}
							if exit_flag {
								break;
							}
						}
						Err(e) => {
							println!("{}: {}", "❌ /done command failed".bright_red(), e);
						}
					}
					continue;
				}

				// Try to process as command
				let command_result = chat_session
					.process_command(&input, &mut current_config, &role, operation_rx.clone())
					.await?;

				match command_result {
					CommandResult::TreatAsUserInput => {
						// This input should be treated as user input, fall through to normal processing
					}
					CommandResult::Exit => {
						// First check if it's a session switch command
						if input.starts_with(SESSION_COMMAND) {
							// We need to switch to another session
							let new_session_name = chat_session.session.info.name.clone();

							// Save current session before switching
							chat_session.save()?;

							// Initialize the new session
							let session_params = SessionInitParams::new(&current_config, &role)
								.with_name(new_session_name)
								.with_max_retries(current_config.max_retries);
							let new_chat_session = ChatSession::initialize(session_params).await?;

							// Replace the current chat session
							chat_session = new_chat_session;

							// Reset first message flag for new session
							first_message_processed = !chat_session.session.messages.is_empty();

							// Print the last few messages for context with colors
							if !chat_session.session.messages.is_empty() {
								let last_messages = chat_session
									.session
									.messages
									.iter()
									.rev()
									.take(3)
									.collect::<Vec<_>>();

								for msg in last_messages.iter().rev() {
									if msg.role == "assistant" {
										println!("{}", msg.content.bright_green());
									} else if msg.role == "user" {
										println!("> {}", msg.content.bright_blue());
									}
								}
							}

							// Continue with the session
							continue;
						} else if input.starts_with(MODEL_COMMAND)
							|| input.starts_with(ROLE_COMMAND)
						{
							// This is a command that requires config reload
							// Reload the configuration
							match crate::config::Config::load() {
								Ok(updated_config) => {
									// Update our current config with the new role-specific config
									current_config =
										updated_config.get_merged_config_for_role(&role);
									// Update thread config for logging macros
									crate::config::set_thread_config(&current_config);
									crate::config::set_thread_role(&role);
								}
								Err(e) => {
									log_info!("Error reloading configuration: {}", e);
								}
							}
							// Continue with the session
							continue;
						} else {
							// It's a regular exit command
							break;
						}
					}
					CommandResult::Handled => {
						// Command was handled successfully, continue with session
						continue;
					}
					CommandResult::HandledWithOutput(mut json_output) => {
						// Command was handled with output
						// Print it for CLI using existing display functions
						print_command_output(&mut json_output, &mut chat_session, &current_config)
							.await;
						continue;
					}
				}
			}

			// Run skill activation + validators on user input
			{
				crate::mcp::core::skill_auto::run_activation(
					crate::mcp::core::skill_auto::Event::User,
					&input,
					&current_dir,
					&mut chat_session,
				)
				.await;
				crate::mcp::core::skill_auto::run_validators(
					crate::mcp::core::skill_auto::Event::User,
					&input,
					&current_dir,
				)
				.await;
			}

			// Check for cancellation before starting layered processing
			if cancellation.is_cancelled() {
				continue;
			}

			// Process layers if enabled using helper function
			let (processed_input, workflow_modified_session, _layer_cancelled) =
				process_layers_if_enabled(
					&input,
					&mut chat_session,
					&current_config,
					&role,
					first_message_processed,
					operation_rx.clone(),
				)
				.await?;

			// Check for cancellation after layer processing
			if cancellation.is_cancelled() {
				continue;
			}

			let final_input = if workflow_modified_session {
				// Layers used output_mode append/replace and added messages to session
				// Skip adding user message to avoid duplicates and continue with the user message
				// to guarantee that the output from layer next processed with the main loop
				first_message_processed = true;
				input // Use original input
			} else {
				// Use the processed input from layers (or original if layers not enabled)
				// Mark that we've processed the first message through layers
				first_message_processed = true;
				processed_input
			};

			// Initialize operation context for smart tracking
			let operation_id = format!(
				"op_{}",
				std::time::SystemTime::now()
					.duration_since(std::time::UNIX_EPOCH)
					.unwrap_or_default()
					.as_millis()
			);
			// CRITICAL: Set first_prompt_idx BEFORE compression runs so the anchor is always
			// correct. Compression uses first_prompt_idx as the lower boundary — if it's None
			// during the first compression, the system message index is used as fallback, which
			// can allow compressing the very first user message.
			// We use the current message count because that is exactly where the user message
			// will be inserted after compression finishes (compression only removes/replaces
			// older messages, never appends).
			if !workflow_modified_session && chat_session.first_prompt_idx.is_none() {
				chat_session.first_prompt_idx = Some(chat_session.session.messages.len());
			}

			// CONVERSATION COMPRESSION: Check if AI should compress older exchanges
			// This happens BEFORE user message is added to ensure user's new request is not broken by summarization
			// AI decides if compression is beneficial based on conversation history
			// EXPONENTIAL COOLDOWN: Reset consecutive compressions so this turn starts with minimal cooldown.
			chat_session.session.info.consecutive_compressions = 0;
			let _compression_occurred =
			match crate::session::chat::conversation_compression::check_and_compress_conversation(
				&mut chat_session,
				&current_config,
				operation_rx.clone(),
				false,
			)
			.await
			{
				Ok(compressed) => compressed,
				Err(e) => {
					// Best-effort: log error but continue session
					log_debug!(
						"Conversation compression failed: {}. Continuing session.",
						e
					);
					false
				}
			};

			// CRITICAL FIX: Set processing state BEFORE adding user message
			// This ensures cancellation cleanup works correctly if Ctrl+C is pressed
			// between adding the message and starting the API call
			*processing_state.lock().unwrap() = ProcessingState::CallingAPI;

			// CRITICAL: Capture user message insertion index AFTER compression mutations.
			// This keeps error rollback truncation aligned with current session layout.

			let user_message_index = chat_session.session.messages.len();

			// Add user message for standard processing flow
			// Skip if layers already modified the session (to avoid duplicates)
			if !workflow_modified_session {
				// Append constraints if configured
				let final_input_with_constraints = super::utils::append_constraints_if_exists(
					&final_input,
					&current_config.custom_constraints_file_name,
					&current_dir,
				);
				chat_session.add_user_message(&final_input_with_constraints)?;
			}

			// Create operation context for tracking
			*current_operation.lock().unwrap() = Some(OperationContext {
				user_message_index: Some(user_message_index),
				assistant_message_index: None, // Will be set when assistant message is added
			});

			log_debug!(
				"Started operation {} with user message at index {}",
				operation_id,
				user_message_index
			);

			// Prepare for API call using helper function
			// Treat cancellation as a soft signal — continue the loop instead of exiting the session
			match prepare_for_api_call(&mut chat_session, &current_config, operation_rx.clone())
				.await
			{
				Ok(()) => {}
				Err(e) if e.to_string().contains("Operation cancelled") => continue,
				Err(e) => return Err(e),
			}

			// Capture message count BEFORE API call to detect if assistant message gets added
			let messages_before_api = chat_session.session.messages.len();

			// Check for Ctrl+C before making API call
			if cancellation.is_cancelled() {
				// Immediately stop and return to main loop
				continue;
			}

			// Execute API call and process response using helper function
			// CRITICAL FIX: Use tokio::select! to race API call against cancellation
			// This allows INSTANT Ctrl+C response instead of waiting for API to complete
			let user_message_index_for_error = user_message_index;
			let model_for_error = chat_session.model.clone();

			let api_result = {
				// Set up notification forwarding for interactive terminal mode.
				// Notifications (e.g. MCP server warnings) are printed to stderr so they
				// don't interfere with the readline prompt on stdout.
				let (notif_tx, mut notif_rx) =
					tokio::sync::mpsc::unbounded_channel::<crate::websocket::ServerMessage>();
				crate::mcp::process::set_notification_sender(None, notif_tx);

				// Drain notifications to stderr in a background task
				let notif_drain = tokio::spawn(async move {
					while let Some(msg) = notif_rx.recv().await {
						if let crate::websocket::ServerMessage::McpNotification(n) = msg {
							if !crate::logging::tracing_setup::is_structured_output_mode() {
								use colored::Colorize;
								eprintln!(
									"{}",
									format!(
										"⚠ [{}] {}",
										n.server,
										n.params
											.get("message")
											.and_then(|m| m.as_str())
											.unwrap_or(&n.method)
									)
									.yellow()
								);
							}
						}
					}
				});

				let result = tokio::select! {
					// API call branch
					result = execute_api_call_and_process_response(
						&mut chat_session,
						&current_config,
						&role,
						operation_rx.clone(),
						OutputMode::Interactive,
						SilentSink,
					) => result,
					// Cancellation branch - INSTANT response
					_ = async {
						let mut cancel_rx = cancellation.operation_receiver();
						while !*cancel_rx.borrow() {
							if cancel_rx.changed().await.is_err() {
								break;
							}
						}
					} => {
						// Ctrl+C pressed - stop animation and return to prompt immediately
						use crate::session::chat::get_animation_manager;
						get_animation_manager().stop_current().await;
						log_debug!("API call cancelled by user - returning to prompt");
						crate::mcp::process::clear_notification_sender(None);
						notif_drain.abort();
						continue;
					}
				}; // end tokio::select!

				crate::mcp::process::clear_notification_sender(None);
				let _ = notif_drain.await;

				result
			}; // end notification wrapper block

			match api_result {
				Ok(_) => {
					// CRITICAL FIX: Check for cancellation BEFORE marking as completed
					// If cancelled during HTTP request, we need to remove the user message
					if cancellation.is_cancelled() {
						log_debug!(
						"Operation cancelled during or after API call - cleaning up user message"
					);

						// Check if assistant message was added (response was processed)
						let messages_after_api = chat_session.session.messages.len();
						let assistant_message_added = messages_after_api > messages_before_api;

						if !assistant_message_added {
							// No assistant response was processed - remove the user message
							if user_message_index < chat_session.session.messages.len() {
								chat_session.session.messages.truncate(user_message_index);
								log_debug!("Removed user message due to cancellation before assistant response");
							}
						}
						// If assistant message was added, keep everything (conversation state is valid)

						continue; // Return to main loop for next user input
					}

					// Update processing state to completed when done (only if not cancelled)
					*processing_state.lock().unwrap() = ProcessingState::CompletedWithResults;

					// Update operation context with assistant message index if one was added
					if let Some(ref mut op) = *current_operation.lock().unwrap() {
						let messages_after_api = chat_session.session.messages.len();
						if messages_after_api > messages_before_api {
							// Assistant message was added - record its index
							op.assistant_message_index = Some(messages_before_api);
							log_debug!("Assistant message added at index {}", messages_before_api);
						}
					}
				}
				Err(e) => {
					// Handle API error using helper function
					handle_api_error(
						&mut chat_session,
						user_message_index_for_error,
						&model_for_error,
						&e,
						OutputMode::Interactive,
					);
				}
			}

			// Clear operation context at the end of each successful iteration
			*current_operation.lock().unwrap() = None;
		}

		Ok(())
	})
	.await
}

// Run a single non-interactive session with provided input
pub async fn run_interactive_session_with_input<T: std::fmt::Debug>(
	args: &T,
	config: &Config,
	initial_input: &str,
) -> Result<()> {
	// Setup and initialize session using helper function

	let (chat_session, config_for_role, role, first_message_processed, spinner) =
		setup_and_initialize_session(args, config).await?;

	// Set task-local session ID so all session-scoped state (skills, plans, schedules, etc.)
	// uses the real session name — must happen after setup determines the actual name.
	let session_id = chat_session.session.info.name.clone();
	crate::session::context::with_session_id(session_id, async move {
	// MCP init — progress display handled internally when stderr is a terminal
	crate::mcp::initialize_mcp_for_role(&role, config).await?;
	crate::session::context::init_session_services(&role);
	// Start inject listener so `octomind send` can push messages into this session
	let _inject_listener = crate::session::inject_listener::start_inject_listener(&chat_session.session.info.name);
	// Start webhook listeners for any --hook flags
	let _webhook_guards = start_webhook_guards(args, config, &chat_session.session.info.name).await?;
	// Parse daemon flag from args debug string
	let daemon = format!("{:?}", args).contains("daemon: true");
	let mut chat_session = chat_session;

	// Setup system prompt and cache using helper function (non-interactive mode)
	setup_system_prompt_and_cache(&mut chat_session, &config_for_role, &role, false).await?;

	crate::mcp::core::skill_auto::load_env_skills(&mut chat_session).await;

	// Clear spinner from setup
	if let Some(sp) = spinner {
		sp.finish_and_clear();
		print!("\x1B[2K\r");
		std::io::Write::flush(&mut std::io::stdout()).ok();
	}

	// Set up cancellation handling for non-interactive mode (simplified)
	let mut cancellation = SessionCancellation::new();

	// Simplified tokio-based Ctrl+C handler for non-interactive mode
	let _signal_handler = cancellation.start_signal_handler();

	// Set the thread-local config for logging macros
	let mut current_config = config_for_role.clone();
	crate::config::set_thread_config(&current_config);
	crate::config::set_thread_role(&role);
	// Use initial_input as the input for this session (convert to owned String for mutability)
	// Use initial_input as the input for this session (convert to owned String for mutability)
	let mut input = initial_input.to_string();
	// Get current directory - use thread-local if set (ACP/WebSocket), otherwise process cwd
	let current_dir = crate::mcp::get_thread_working_directory();
	let mut operation_rx = cancellation.new_operation();

	// Apply runtime state from session log if this is a resumed session
	if chat_session.was_resumed {
		if let Some(session_file) = &chat_session.session.session_file {
			if let Ok(runtime_state) = crate::session::extract_runtime_state_from_log(session_file)
			{
				// Workflow state is now stored in role config, not runtime state
				// This section is kept for backward compatibility but does nothing
				if let Some(_workflow_enabled) = runtime_state.layers_enabled {
					log_info!("Legacy layers_enabled state ignored - using workflow configuration");
				}
			}
		}
	}

	// Skip initial message processing when daemon starts with no input.
	if !input.is_empty() {

	// Check if this is a command (same logic as interactive session)
	if input.starts_with('/') {
		// Handle special /done command separately
		if input.trim() == "/done" {
			// Clear plan data
			if let Err(e) = crate::mcp::core::plan::clear_plan_data().await {
				log_debug!("Failed to clear plan data: {}", e);
			}

			println!(
				"{}",
				"✓ Session optimized and ready for next message".bright_green()
			);
			let _ = chat_session.save();
			return Ok(());
		}

		// Try to process as command
		let command_result = chat_session
			.process_command(&input, &mut current_config, &role, operation_rx.clone())
			.await?;

		match command_result {
			crate::session::chat::session::commands::CommandResult::TreatAsUserInput => {
				// This input should be treated as user input, fall through to normal processing
			}
			crate::session::chat::session::commands::CommandResult::Exit => {
				// Check if it's a session switch command
				if input.starts_with(crate::session::chat::commands::SESSION_COMMAND) {
					println!("{}", "Note: Session switching is not supported in run mode. Use 'octomind run' for interactive session management.".yellow())
				}
				// Save session after command execution
				let _ = chat_session.save();
				return Ok(());
			}
			crate::session::chat::session::commands::CommandResult::Handled => {
				// Command was handled successfully
				// Save session after command execution
				let _ = chat_session.save();
				return Ok(());
			}
			crate::session::chat::session::commands::CommandResult::HandledWithOutput(
				mut json_output,
			) => {
				// Command was handled with output
				// Print it for CLI run command using existing display functions
				print_command_output(&mut json_output, &mut chat_session, &current_config).await;
				// Save session after command execution
				let _ = chat_session.save();
				return Ok(());
			}
		}
	}

	// Run skill activation + validators on user input
	crate::mcp::core::skill_auto::run_activation(
		crate::mcp::core::skill_auto::Event::User,
		&input,
		&current_dir,
		&mut chat_session,
	)
	.await;

	// Layer processing if enabled and first message using helper function
	let (processed_input, workflow_modified_session, layer_cancelled) = process_layers_if_enabled(
		&input,
		&mut chat_session,
		&current_config,
		&role,
		first_message_processed,
		operation_rx.clone(),
	)
	.await?;

	// CRITICAL FIX: Reset cancellation state after layer cancellation
	// This prevents subsequent operations from failing due to stale cancellation signal
	if layer_cancelled {
		cancellation.reset();
		log_info!(
			"Cancellation state reset after layer cancellation - ready for main model processing"
		);

		// Save session after layer cancellation cleanup to persist the cleaned state
		let _ = chat_session.save();
		log_info!("Session saved after layer cancellation cleanup");

		// Create new operation receiver with reset cancellation state
		operation_rx = cancellation.new_operation();
	}

	if workflow_modified_session {
		// Layers used output_mode append/replace and added messages to session
		// Continue processing to ensure main model gets called (same as interactive mode)
		log_info!("Layers modified session. Continuing with main model processing.");
		// Use processed input from layers for main model
		input = processed_input;
	} else {
		// Use processed input from layers (or original if layers not enabled)
		input = processed_input;
	}

	// Add user message - same as interactive
	let user_message_index = chat_session.session.messages.len();
	let has_workflow = current_config
		.role_map
		.get(&role)
		.and_then(|r| r.workflow.as_ref())
		.is_some();
	if !has_workflow {
		// Append constraints if configured
		let input_with_constraints = super::utils::append_constraints_if_exists(
			&input,
			&current_config.custom_constraints_file_name,
			&current_dir,
		);
		chat_session.add_user_message(&input_with_constraints)?;
	}

	// Prepare for API call using helper function
	prepare_for_api_call(&mut chat_session, &current_config, operation_rx.clone()).await?;

	// Execute API call and process response using helper function (non-interactive mode)
	let user_message_index_for_error = user_message_index;
	let operation_rx_clone = operation_rx.clone();
	let model_for_error = chat_session.model.clone();
	let api_result = if current_config.runtime_output_mode.as_deref() == Some("jsonl") {
		// For JSONL mode, set up a notification channel so MCP server notifications
		// are forwarded as structured JSON lines alongside the regular output.
		let (notif_tx, mut notif_rx) =
			tokio::sync::mpsc::unbounded_channel::<crate::websocket::ServerMessage>();
		crate::mcp::process::set_notification_sender(None, notif_tx);

		// Drain notifications to stdout in a background task
		let drain_handle = tokio::spawn(async move {
			while let Some(msg) = notif_rx.recv().await {
				if let Ok(json) = serde_json::to_string(&msg) {
					println!("{}", json);
				}
			}
		});

		let result = execute_api_call_and_process_response(
			&mut chat_session,
			&current_config,
			&role,
			operation_rx_clone,
			OutputMode::Jsonl,
			JsonlSink,
		)
		.await;

		// Stop forwarding notifications and wait for drain to finish
		crate::mcp::process::clear_notification_sender(None);
		let _ = drain_handle.await;

		result
	} else {
		// Non-interactive run mode: set up notification sender so warnings print to stderr
		let (notif_tx, mut notif_rx) =
			tokio::sync::mpsc::unbounded_channel::<crate::websocket::ServerMessage>();
		crate::mcp::process::set_notification_sender(None, notif_tx);

		let notif_drain = tokio::spawn(async move {
			while let Some(msg) = notif_rx.recv().await {
				if let crate::websocket::ServerMessage::McpNotification(n) = msg {
					if !crate::logging::tracing_setup::is_structured_output_mode() {
						use colored::Colorize;
						eprintln!(
							"{}",
							format!(
								"⚠ [{}] {}",
								n.server,
								n.params
									.get("message")
									.and_then(|m| m.as_str())
									.unwrap_or(&n.method)
							)
							.yellow()
						);
					}
				}
			}
		});

		let result = execute_api_call_and_process_response(
			&mut chat_session,
			&current_config,
			&role,
			operation_rx_clone,
			OutputMode::NonInteractive,
			SilentSink,
		)
		.await;

		crate::mcp::process::clear_notification_sender(None);
		let _ = notif_drain.await;

		result
	};
	match api_result {
		Ok(_) => {
			// JSONL output is now streamed via callback - no need for batch output
		}
		Err(e) => {
			// Kill any running async jobs on error/cancellation
			if let Some(manager) = crate::mcp::agent::functions::get_job_manager() {
				manager.kill_all();
			}

			// Handle API error using helper function
			let output_mode = if current_config.runtime_output_mode.as_deref() == Some("jsonl") {
				OutputMode::Jsonl
			} else {
				OutputMode::NonInteractive
			};
			handle_api_error(
				&mut chat_session,
				user_message_index_for_error,
				&model_for_error,
				&e,
				output_mode,
			);
		}
	}
	} // end if !input.is_empty()

	// Keep session alive while there are pending inbox messages, scheduled entries, or active jobs.
	// All injection sources (schedule, background agents) push to the inbox — drain it here.
	loop {
		// Flush any due schedule entries into the inbox first.
		crate::mcp::core::flush_due_to_inbox();

		// Process all messages currently in the inbox.
		while let Some(inbox_msg) = crate::session::inbox::try_pop_inbox_message() {
			log_debug!("Non-interactive: processing inbox message from {:?}", inbox_msg.source);
			chat_session.add_user_message(&inbox_msg.content)?;
			prepare_for_api_call(&mut chat_session, &current_config, operation_rx.clone()).await?;

			let is_jsonl = current_config.runtime_output_mode.as_deref() == Some("jsonl");
			let output_mode = if is_jsonl {
				OutputMode::Jsonl
			} else {
				OutputMode::NonInteractive
			};

			// Set up notification forwarding for this inbox message,
			// matching the pattern used for the initial message processing.
			let (notif_tx, mut notif_rx) =
				tokio::sync::mpsc::unbounded_channel::<crate::websocket::ServerMessage>();
			crate::mcp::process::set_notification_sender(None, notif_tx);

			let drain_handle = tokio::spawn(async move {
				while let Some(msg) = notif_rx.recv().await {
					if is_jsonl {
						if let Ok(json) = serde_json::to_string(&msg) {
							println!("{}", json);
						}
					} else if let crate::websocket::ServerMessage::McpNotification(n) = msg {
						if !crate::logging::tracing_setup::is_structured_output_mode() {
							use colored::Colorize;
							eprintln!(
								"{}",
								format!(
									"⚠ [{}] {}",
									n.server,
									n.params
										.get("message")
										.and_then(|m| m.as_str())
										.unwrap_or(&n.method)
								)
								.yellow()
							);
						}
					}
				}
			});

			let api_result = if is_jsonl {
				execute_api_call_and_process_response(
					&mut chat_session,
					&current_config,
					&role,
					operation_rx.clone(),
					output_mode,
					JsonlSink,
				)
				.await
			} else {
				execute_api_call_and_process_response(
					&mut chat_session,
					&current_config,
					&role,
					operation_rx.clone(),
					output_mode,
					SilentSink,
				)
				.await
			};

			crate::mcp::process::clear_notification_sender(None);
			let _ = drain_handle.await;

			if let Err(e) = api_result {
				log_debug!("Error processing inbox message: {}", e);
			}

			// Refresh operation receiver in case cancellation was triggered
			operation_rx = cancellation.new_operation();
		}

		// Check if there's anything left to wait for.
		let has_schedules = crate::mcp::core::has_pending_schedules();
		let active_jobs = crate::mcp::agent::functions::get_job_manager()
			.map(|m| m.active_count())
			.unwrap_or(0);

		if !daemon && !has_schedules && active_jobs == 0 {
			break;
		}

		// Wait for the next event: either a schedule timer fires or an inbox message arrives.
		let inbox_notify = crate::session::inbox::get_inbox_notify();
		tokio::select! {
			_ = crate::mcp::core::next_schedule_sleep() => {}
			_ = async {
				if let Some(notify) = inbox_notify {
					notify.notified().await;
				} else {
					std::future::pending::<()>().await;
				}
			} => {}
		}
	}

	// Save session before exit
	let _ = chat_session.save();
	Ok(())
	}).await
}
