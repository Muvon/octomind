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

// Run an interactive session
pub async fn run_interactive_session<T: std::fmt::Debug>(args: &T, config: &Config) -> Result<()> {
	// Setup and initialize session using helper function
	let (mut chat_session, config_for_role, role, mut first_message_processed) =
		setup_and_initialize_session(args, config).await?;

	// Get current directory for file operations
	let current_dir = std::env::current_dir()?;

	// Setup system prompt and cache using helper function (BEFORE showing interactive prompts)
	setup_system_prompt_and_cache(&mut chat_session, &config_for_role, &role, true).await?;

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

	// Set the thread-local config for logging macros
	crate::config::set_thread_config(&current_config);

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
				if current_config.runtime_output_mode.as_deref() != Some("jsonl") {
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

				// Attempt one retry
				log_debug!("Retrying session save after failure...");
				if let Err(retry_err) = chat_session.save() {
					eprintln!(
						"{}",
						"⚠️  Retry failed. Session may be corrupted.".bright_red()
					);
					log_debug!("Retry save failed: {}", retry_err);
				} else {
					eprintln!("{}", "✓ Retry succeeded - session saved.".bright_green());
				}
			}

			// Clear operation context
			*current_operation.lock().unwrap() = None;

			// CRITICAL FIX: Reset continuation state when cancelled
			// This prevents infinite loop where continuation_pending remains true after Ctrl+C
			if chat_session.continuation_pending {
				chat_session.continuation_pending = false;
				log_debug!(
					"Continuation state reset due to cancellation - breaking continuation loop"
				);
			}

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

		// Get a new operation token for this iteration
		let operation_rx = cancellation.new_operation();

		// CRITICAL FIX: Check if continuation is pending from previous iteration
		// If so, skip reading user input and process the injected summary request immediately
		// BUT FIRST: Check if operation was cancelled to prevent infinite loops
		// Calculate current context tokens for display
		let current_context_tokens = calculate_current_context_tokens(
			&chat_session.session.messages,
			&current_config,
			&role,
		)
		.await;

		let input_result = if chat_session.continuation_pending {
			// Safety check: If cancellation occurred, reset continuation state and read user input normally
			if cancellation.is_cancelled() {
				log_debug!("Cancellation detected during continuation - resetting continuation state and reading user input");
				chat_session.continuation_pending = false;
				read_user_input(
					chat_session.estimated_cost,
					&current_config,
					&role,
					current_context_tokens,
					current_config.max_session_tokens_threshold,
					&chat_session.session.info.name,
					false, // Don't show status
				)?
			} else {
				log_debug!(
					"Continuation pending - processing injected summary request automatically"
				);
				// Get the last message which should be the injected summary request
				chat_session
					.session
					.messages
					.last()
					.filter(|msg| msg.role == "user")
					.map(|msg| InputResult::Text(msg.content.clone()))
					.unwrap_or_else(|| {
						log_debug!("Warning: Expected summary request message not found, falling back to user input");
						read_user_input(
							chat_session.estimated_cost,
							&current_config,
							&role,
							current_context_tokens,
							current_config.max_session_tokens_threshold,
							&chat_session.session.info.name,
							false,
						)
						.unwrap_or(InputResult::Text(String::new()))
					})
			}
		} else if let Some(prompt_text) = chat_session.pending_prompt.take() {
			// CRITICAL FIX: Process pending prompt from /prompt command
			// This allows the prompt to be processed as normal user input
			log_debug!("Processing pending prompt template as user input");
			InputResult::Text(prompt_text)
		} else {
			// Read user input with command completion and cost estimation
			read_user_input(
				chat_session.estimated_cost,
				&current_config,
				&role,
				current_context_tokens,
				current_config.max_session_tokens_threshold,
				&chat_session.session.info.name,
				false, // Don't show status line after first interaction
			)?
		};

		// Handle the input result with proper error recovery
		let input = match input_result {
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

				// Ensure session is saved
				if let Err(e) = chat_session.save() {
					log_debug!("Warning: Failed to save session after cancellation: {}", e);
				}
				continue;
			}
			InputResult::Exit => {
				// Ctrl+D pressed - graceful exit handled in input.rs
				// Ensure session is saved
				if let Err(e) = chat_session.save() {
					log_debug!("Warning: Failed to save session: {}", e);
				}
				break;
			}
		};

		// Check if the input is an exit command
		if input == "/exit" || input == "/quit" {
			// Show resume command with session ID
			let resume_cmd = format!(
				"octomind session --resume {}",
				chat_session.session.info.name
			)
			.bright_cyan();
			println!("\nTo continue this session, run: {}", resume_cmd);
			break;
		}

		// Skip if input is empty
		if input.trim().is_empty() {
			continue;
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
					&role,
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
					} else if input.starts_with(MODEL_COMMAND) || input.starts_with(ROLE_COMMAND) {
						// This is a command that requires config reload
						// Reload the configuration
						match crate::config::Config::load() {
							Ok(updated_config) => {
								// Update our current config with the new role-specific config
								current_config = updated_config.get_merged_config_for_role(&role);
								// Update thread config for logging macros
								crate::config::set_thread_config(&current_config);
								log_info!("Configuration reloaded successfully");
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

		// CONVERSATION COMPRESSION: Check if AI should compress older exchanges
		// This happens BEFORE user message is added to ensure user's new request is not broken by summarization
		// AI decides if compression is beneficial based on conversation history
		let compression_occurred =
			match crate::session::chat::conversation_compression::check_and_compress_conversation(
				&mut chat_session,
				&current_config,
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

		// CRITICAL FIX: After compression, check if continuation should trigger
		// Compression freed up space, so we should check if we can continue with pending work
		if compression_occurred && crate::mcp::dev::plan::core::has_active_plan() {
			log_debug!("Compression completed with active plan - checking if continuation needed");
			use crate::session::chat::session_continuation;
			if session_continuation::check_and_handle_continuation(
				&mut chat_session,
				&current_config,
			)
			.await?
			{
				log_debug!("Continuation triggered after compression - skipping to next iteration");
				// The summary request message has already been injected by check_and_handle_continuation
				// Just continue the loop to process it immediately without waiting for user input
				continue;
			}
		}

		// NEW FLOW: Check for continuation BEFORE processing new user request

		// CRITICAL: Skip continuation check immediately after compression
		// Compression already evaluated token pressure with CURRENT state and modified the session.
		// Checking continuation here would use STALE token counts (pre-compression) causing false triggers.
		// The next user request will check continuation with accurate post-compression state.
		//
		// This is one of the two correct moments to trigger continuation:
		// 1) On new user request (HERE) - but NOT immediately after compression
		// 2) After all tool results gathered, before sending to AI (in tool_result_processor)
		if !chat_session.continuation_pending && !compression_occurred {
			use crate::session::chat::session_continuation;
			if session_continuation::check_and_handle_continuation(
				&mut chat_session,
				&current_config,
			)
			.await?
			{
				log_debug!("Token limit reached on new user request - continuation triggered, skipping to next iteration");
				// The summary request message has already been injected by check_and_handle_continuation
				// Just continue the loop to process it immediately without waiting for user input
				continue;
			}
		}

		// CRITICAL FIX: Set processing state BEFORE adding user message
		// This ensures cancellation cleanup works correctly if Ctrl+C is pressed
		// between adding the message and starting the API call
		*processing_state.lock().unwrap() = ProcessingState::CallingAPI;

		// CRITICAL: Capture user message insertion index AFTER compression/continuation mutations.
		// This keeps error rollback truncation aligned with current session layout.
		let user_message_index = chat_session.session.messages.len();

		// Add user message for standard processing flow
		// CRITICAL FIX: Add user message unless continuation is pending or layers modified session
		// Logic:
		// - continuation_pending = true: Continuation message already added → Skip (avoid duplicates)
		// - workflow_modified_session = true: Layers added messages to session → Skip (avoid duplicates)
		// - workflow_modified_session = false: Layers didn't add messages → Add user message (needed for conversation)
		if !chat_session.continuation_pending && !workflow_modified_session {
			// CRITICAL: Set first_prompt_idx if not already set (INCLUSIVE boundary for compression)
			// This protects bootstrap/instructions forever - compression NEVER goes below this index
			if chat_session.first_prompt_idx.is_none() {
				chat_session.first_prompt_idx = Some(user_message_index);
			}

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
		prepare_for_api_call(&mut chat_session, &current_config, operation_rx.clone()).await?;

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
			crate::mcp::process::set_notification_sender(notif_tx);

			// Drain notifications to stderr in a background task
			let notif_drain = tokio::spawn(async move {
				while let Some(msg) = notif_rx.recv().await {
					if let crate::websocket::ServerMessage::McpNotification(n) = msg {
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
					crate::mcp::process::clear_notification_sender();
					notif_drain.abort();
					continue;
				}
			}; // end tokio::select!

			crate::mcp::process::clear_notification_sender();
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

				// CRITICAL FIX: Check if continuation was triggered during tool processing
				// If continuation_pending is true, it means a summary request was injected
				// and we need to skip waiting for user input and process it immediately
				if chat_session.continuation_pending {
					log_debug!("Continuation triggered during tool processing - skipping to next iteration to process summary request automatically");
					// The summary request message has already been injected by check_and_handle_continuation
					// Just continue the loop to process it immediately without waiting for user input
					continue;
				}

				// SAFETY CHECK: Ensure continuation state is properly cleared after successful processing
				// This provides additional protection against continuation state getting stuck
				if chat_session.continuation_pending {
					log_debug!("Warning: Continuation state still pending after successful processing - clearing it");
					chat_session.continuation_pending = false;
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
}

// Run a single non-interactive session with provided input
pub async fn run_interactive_session_with_input<T: std::fmt::Debug>(
	args: &T,
	config: &Config,
	initial_input: &str,
) -> Result<()> {
	// Setup and initialize session using helper function
	let (mut chat_session, config_for_role, role, first_message_processed) =
		setup_and_initialize_session(args, config).await?;

	// Setup system prompt and cache using helper function (non-interactive mode)
	setup_system_prompt_and_cache(&mut chat_session, &config_for_role, &role, false).await?;

	// Set up cancellation handling for non-interactive mode (simplified)
	let mut cancellation = SessionCancellation::new();

	// Simplified tokio-based Ctrl+C handler for non-interactive mode
	let _signal_handler = cancellation.start_signal_handler();

	// Set the thread-local config for logging macros
	let mut current_config = config_for_role.clone();
	crate::config::set_thread_config(&current_config);

	// Use initial_input as the input for this session (convert to owned String for mutability)
	let mut input = initial_input.to_string();
	let current_dir = std::env::current_dir()?;

	// Create operation receiver for cancellation
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

	// Check if this is a command (same logic as interactive session)
	if input.starts_with('/') {
		// Handle special /done command separately
		if input.trim() == "/done" {
			// Disable continuation triggers during /done processing
			chat_session.disable_continuation();

			// Clear plan data
			if let Err(e) = crate::mcp::dev::plan::clear_plan_data().await {
				log_debug!("Failed to clear plan data: {}", e);
			}

			// Re-enable continuation triggers after /done processing
			chat_session.enable_continuation();

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
					println!("{}", "Note: Session switching is not supported in run mode. Use 'octomind session' for interactive session management.".yellow());
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
		crate::mcp::process::set_notification_sender(notif_tx);

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
		crate::mcp::process::clear_notification_sender();
		let _ = drain_handle.await;

		result
	} else {
		// Non-interactive run mode: set up notification sender so warnings print to stderr
		let (notif_tx, mut notif_rx) =
			tokio::sync::mpsc::unbounded_channel::<crate::websocket::ServerMessage>();
		crate::mcp::process::set_notification_sender(notif_tx);

		let notif_drain = tokio::spawn(async move {
			while let Some(msg) = notif_rx.recv().await {
				if let crate::websocket::ServerMessage::McpNotification(n) = msg {
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

		crate::mcp::process::clear_notification_sender();
		let _ = notif_drain.await;

		result
	};
	match api_result {
		Ok(_) => {
			// JSONL output is now streamed via callback - no need for batch output
		}
		Err(e) => {
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

	// Save session before exit
	let _ = chat_session.save();
	Ok(())
}
