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

// Interactive session runner

use super::super::animation::show_loading_animation;
use super::super::commands::*;
use super::super::context_truncation::check_and_truncate_context;
use super::super::input::read_user_input;
use super::super::response::process_response;
use super::core::ChatSession;
use crate::config::Config;
use crate::session::create_system_prompt;
use crate::{log_debug, log_info};
use anyhow::Result;
use std::io::Write; // Added for stdout flushing
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

// Run an interactive session
pub async fn run_interactive_session<T: clap::Args + std::fmt::Debug>(
	args: &T,
	config: &Config,
) -> Result<()> {
	use clap::Args;
	use std::fmt::Debug;

	// Extract args from clap::Args
	#[derive(Args, Debug)]
	struct SessionArgs {
		/// Name of the session to start or resume
		#[arg(long, short)]
		name: Option<String>,

		/// Resume an existing session
		#[arg(long, short)]
		resume: Option<String>,

		/// Model to use instead of the one configured in config
		#[arg(long)]
		model: Option<String>,

		/// Temperature for the AI response
		#[arg(long, default_value = "0.7")]
		temperature: f32,

		/// Session role: developer (default with layers and tools) or assistant (simple chat without tools)
		#[arg(long, default_value = "developer")]
		role: String,
	}

	// Read args as SessionArgs
	let args_str = format!("{:?}", args);
	let session_args: SessionArgs = {
		// Get model
		let model = if args_str.contains("model: Some(\"") {
			let start = args_str.find("model: Some(\"").unwrap() + 13;
			let end = args_str[start..].find('\"').unwrap() + start;
			Some(args_str[start..end].to_string())
		} else {
			None
		};

		// Get name
		let name = if args_str.contains("name: Some(\"") {
			let start = args_str.find("name: Some(\"").unwrap() + 12;
			let end = args_str[start..].find('\"').unwrap() + start;
			Some(args_str[start..end].to_string())
		} else {
			None
		};

		// Get resume
		let resume = if args_str.contains("resume: Some(\"") {
			let start = args_str.find("resume: Some(\"").unwrap() + 14;
			let end = args_str[start..].find('\"').unwrap() + start;
			Some(args_str[start..end].to_string())
		} else {
			None
		};

		// Get role
		let role = if args_str.contains("role: \"") {
			let start = args_str.find("role: \"").unwrap() + 7;
			let end = args_str[start..].find('\"').unwrap() + start;
			args_str[start..end].to_string()
		} else {
			"developer".to_string() // Default role
		};

		// Get temperature
		let temperature = if args_str.contains("temperature: ") {
			let start = args_str.find("temperature: ").unwrap() + 13;
			let end = args_str[start..].find(',').unwrap_or(
				args_str[start..]
					.find('}')
					.unwrap_or(args_str.len() - start),
			) + start;
			args_str[start..end].trim().parse::<f32>().unwrap_or(0.7)
		} else {
			0.7 // Default temperature
		};

		SessionArgs {
			name,
			resume,
			model,
			temperature,
			role,
		}
	};

	// For developer role, show MCP server status
	let current_dir = std::env::current_dir()?;
	if session_args.role == "developer" {
		// Check if external MCP server is configured
		let mode_config = config.get_mode_config(&session_args.role);
		let mcp_config = &mode_config.1;

		if mcp_config.server_refs.is_empty() {
			use colored::*;
			println!(
				"{}",
				"💡 Tip: For code development, consider starting an external MCP server:"
					.bright_yellow()
			);
			println!("{}", "   octocode mcp --path=.".bright_cyan());
			println!(
				"{}",
				"   Then configure it in your system config:".bright_cyan()
			);
			if let Ok(config_path) = crate::directories::get_config_file_path() {
				println!("{}", format!("   {}", config_path.display()).bright_cyan());
			}
			println!();
		} else {
			// Check if octocode is enabled in the server_refs
			let octocode_enabled = mcp_config.server_refs.contains(&"octocode".to_string());

			if octocode_enabled {
				use colored::*;
				println!(
					"{}",
					"🔗 octocode MCP server is enabled for enhanced codebase analysis"
						.bright_green()
				);
				println!();
			} else {
				use colored::*;
				println!(
					"{}",
					"💡 Tip: Install octocode for enhanced codebase analysis:".bright_yellow()
				);
				println!(
					"{}",
					"   cargo install octocode  # or download from releases".bright_cyan()
				);
				println!(
					"{}",
					"   It will be auto-enabled when available in PATH".bright_cyan()
				);
				println!();
			}
		}
	}

	// Get the merged configuration for the specified role
	let mode_config = config.get_merged_config_for_mode(&session_args.role);

	// Create or load session
	let mut chat_session = ChatSession::initialize(
		session_args.name,
		session_args.resume,
		session_args.model.clone(),
		Some(session_args.temperature),
		&mode_config,
	)?;

	// If runtime model override is provided, update the session's model (runtime only)
	if let Some(ref runtime_model) = session_args.model {
		chat_session.model = runtime_model.clone();
		log_info!("Using runtime model override: {}", runtime_model);
	}

	// Always set the temperature from the command line (runtime only)
	chat_session.temperature = session_args.temperature;

	// Track if the first message has been processed through layers
	let mut first_message_processed = !chat_session.session.messages.is_empty();
	println!("Interactive coding session started. Type your questions/requests.");
	println!("Type /help for available commands.");

	// Show history usage info for new sessions
	if chat_session.session.messages.is_empty() {
		use colored::*;
		println!(
			"{}",
			"💡 Tip: Use ↑/↓ arrows or Ctrl+R for command history search".bright_yellow()
		);
	}

	// Initialize with system prompt if new session
	if chat_session.session.messages.is_empty() {
		// Create system prompt based on role
		let system_prompt = create_system_prompt(&current_dir, config, &session_args.role).await;
		chat_session.add_system_message(&system_prompt)?;

		// CRITICAL FIX: Apply automatic cache markers for system messages AND tool definitions
		// This ensures consistent caching behavior across all supported models
		let supports_caching = crate::session::model_supports_caching(&chat_session.model);
		let has_tools = !config.mcp.servers.is_empty();
		
		if supports_caching {
			let cache_manager = crate::session::cache::CacheManager::new();
			cache_manager.add_automatic_cache_markers(
				&mut chat_session.session.messages,
				has_tools,
				supports_caching,
			);
			
			log_info!("System prompt has been automatically marked for caching to save tokens in future interactions.");
			// Save the session to ensure the cached status is persisted
			let _ = chat_session.save();
		} else {
			// Don't show warning for models that don't support caching
			log_info!("Note: This model doesn't support caching, but system prompt is still optimized.");
		}

		// Add assistant welcome message
		let welcome_message = format!(
			"Hello! Octomind ready to serve you. Working dir: {} (Role: {})",
			current_dir
				.file_name()
				.unwrap_or_default()
				.to_string_lossy(),
			session_args.role
		);
		chat_session.add_assistant_message(
			&welcome_message,
			None,
			&mode_config,
			&session_args.role,
		)?;

		// Print welcome message with colors if terminal supports them
		use colored::*;
		println!("{}", welcome_message.bright_green());
	} else {
		// Print the last few messages for context with colors if terminal supports them
		let last_messages = chat_session
			.session
			.messages
			.iter()
			.rev()
			.take(3)
			.collect::<Vec<_>>();
		use colored::*;

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
	let ctrl_c_pressed = Arc::new(AtomicBool::new(false));
	let ctrl_c_pressed_clone = ctrl_c_pressed.clone();

	// Track the processing state to determine what to do on cancellation
	#[derive(Debug, Clone, PartialEq)]
	enum ProcessingState {
		Idle,                 // No operation in progress
		ReadingInput,         // Reading user input
		ProcessingLayers,     // Processing through layers
		CallingAPI,           // Making API call
		ExecutingTools,       // Executing tools
		ProcessingResponse,   // Processing response
		CompletedWithResults, // Completed successfully with results to keep
	}

	let processing_state = Arc::new(std::sync::Mutex::new(ProcessingState::Idle));
	let processing_state_clone = processing_state.clone();

	// Track the last user message index to know what to remove on cancellation
	let last_user_message_index = Arc::new(std::sync::Mutex::new(None::<usize>));

	// Set up sophisticated Ctrl+C handler with immediate feedback
	ctrlc::set_handler(move || {
		// Double Ctrl+C forces immediate exit
		if ctrl_c_pressed_clone.load(Ordering::SeqCst) {
			println!("\n🛑 Forcing exit due to repeated Ctrl+C...");
			std::process::exit(130); // 130 is standard exit code for SIGINT
		}

		// Set the flag immediately
		ctrl_c_pressed_clone.store(true, Ordering::SeqCst);

		// Get current processing state to provide appropriate feedback
		let state = processing_state_clone.lock().unwrap().clone();

		// Provide immediate feedback based on current state
		match state {
			ProcessingState::Idle | ProcessingState::ReadingInput => {
				println!("\n🛑 Interrupting... Ready for new input");
			}
			ProcessingState::ProcessingLayers => {
				println!("\n🛑 Interrupting layer processing... Ready for new input");
			}
			ProcessingState::CallingAPI => {
				println!("\n🛑 Interrupting API request... Cleaning up... Ready for new input");
			}
			ProcessingState::ExecutingTools => {
				println!("\n🛑 Interrupting tool execution... Killing processes... Ready for new input");
			}
			ProcessingState::ProcessingResponse => {
				println!("\n🛑 Interrupting response processing... Preserving work... Ready for new input");
			}
			ProcessingState::CompletedWithResults => {
				println!("\n🛑 Operation completed... All work preserved... Ready for new input");
			}
		}

		println!("💡 Press Ctrl+C again to force exit");
		std::io::stdout().flush().unwrap();
	})
	.expect("Error setting Ctrl+C handler");

	// We need to handle configuration reloading, so keep our own copy that we can update
	let mut current_config = mode_config.clone();

	// Set the thread-local config for logging macros
	crate::config::set_thread_config(&current_config);

	// Main interaction loop
	loop {
		// Set processing state to idle
		*processing_state.lock().unwrap() = ProcessingState::Idle;

		// Handle cancellation at the start of each loop iteration
		if ctrl_c_pressed.load(Ordering::SeqCst) {
			log_debug!("Ctrl+C detected - checking for incomplete tool calls to clean up");
			
			// CRITICAL FIX: Always check for and clean up incomplete tool calls when Ctrl+C is pressed
			// This ensures MCP protocol compliance regardless of processing state
			
			// Check if we have any incomplete tool calls in the conversation
			let mut cleanup_needed = false;
			let mut cleanup_from_index = None;
			
			// Look for assistant messages with tool_calls that don't have corresponding tool_result messages
			for i in 0..chat_session.session.messages.len() {
				if let Some(msg) = chat_session.session.messages.get(i) {
					if msg.role == "assistant" && msg.tool_calls.is_some() {
						// This assistant message has tool_calls - check if all have matching tool results
						if let Some(tool_calls_value) = &msg.tool_calls {
							if let Ok(tool_calls_array) =
								serde_json::from_value::<Vec<serde_json::Value>>(
									tool_calls_value.clone(),
								) {
								for tool_call in tool_calls_array {
									if let Some(tool_id) =
										tool_call.get("id").and_then(|id| id.as_str())
									{
										// Check if there's a matching tool result message after this assistant message
										let has_matching_result = chat_session
											.session
											.messages[i + 1..]
											.iter()
											.any(|result_msg| {
												result_msg.role == "tool"
													&& result_msg.tool_call_id.as_ref()
														== Some(&tool_id.to_string())
											});

										if !has_matching_result {
											// Found incomplete tool call - need to clean up from here
											cleanup_needed = true;
											cleanup_from_index = Some(i);
											break;
										}
									}
								}
							}
						}
					}
				}
				if cleanup_needed {
					break;
				}
			}

			// If we found incomplete tool calls, clean them up
			if cleanup_needed {
				if let Some(cleanup_index) = cleanup_from_index {
					// Find the user message that preceded this assistant message with incomplete tool calls
					let mut user_message_index = cleanup_index;
					for i in (0..cleanup_index).rev() {
						if let Some(msg) = chat_session.session.messages.get(i) {
							if msg.role == "user" {
								user_message_index = i;
								break;
							}
						}
					}
					
					// Remove everything from the user message that led to incomplete tool calls
					chat_session.session.messages.truncate(user_message_index);
					log_debug!(
						"Removed incomplete tool calls and related messages due to cancellation (from index {})",
						user_message_index
					);
				}
			} else {
				// No incomplete tool calls, but still clean up based on processing state
				let should_remove_last_message = {
					let state = processing_state.lock().unwrap().clone();
					matches!(
						state,
						ProcessingState::CallingAPI | ProcessingState::ExecutingTools | ProcessingState::ProcessingResponse
					)
				};

				if should_remove_last_message {
					if let Some(last_index) = *last_user_message_index.lock().unwrap() {
						chat_session.session.messages.truncate(last_index);
						log_debug!("Removed incomplete request due to cancellation");
					}
				}
			}

			// Save the session after cleanup to persist changes
			if let Err(e) = chat_session.save() {
				log_debug!("Warning: Failed to save session after cancellation cleanup: {}", e);
			}

			// Reset for next iteration
			ctrl_c_pressed.store(false, Ordering::SeqCst);
			*last_user_message_index.lock().unwrap() = None;
			continue;
		}

		// Set state to reading input
		*processing_state.lock().unwrap() = ProcessingState::ReadingInput;

		// Create a fresh cancellation flag for this iteration
		let operation_cancelled = Arc::new(AtomicBool::new(false));

		// Read user input with command completion and cost estimation
		let mut input = read_user_input(chat_session.estimated_cost)?;

		// Check if the input is an exit command from Ctrl+D
		if input == "/exit" || input == "/quit" {
			println!("Ending session. Your conversation has been saved.");
			break;
		}

		// Skip if input is empty (could be from Ctrl+C)
		if input.trim().is_empty() {
			continue;
		}

		// Check if this is a command
		if input.starts_with('/') {
			// Handle special /done command separately
			if input.trim() == "/done" {
				// Reset first_message_processed to false so that the next message goes through layers again
				first_message_processed = false;

				// Apply reducer functionality to optimize context
				let result = super::super::context_reduction::perform_context_reduction(
					&mut chat_session,
					&current_config,
					&session_args.role,
					operation_cancelled.clone(),
				)
				.await;

				if let Err(e) = result {
					use colored::*;
					println!(
						"{}: {}",
						"Error performing context reduction".bright_red(),
						e
					);
				} else {
					use colored::*;
					println!(
						"{}",
						"\nNext message will be processed through the full layered architecture."
							.bright_green()
					);

					// EditorConfig formatting has been removed to simplify dependencies
					// Users can apply EditorConfig formatting manually or through their IDE
				}
				continue;
			}

			let exit = chat_session
				.process_command(&input, &current_config, &session_args.role)
				.await?;
			if exit {
				// First check if it's a session switch command
				if input.starts_with(SESSION_COMMAND) {
					// We need to switch to another session
					let new_session_name = chat_session.session.info.name.clone();

					// Save current session before switching
					chat_session.save()?;

					// Initialize the new session
					let new_chat_session = ChatSession::initialize(
						Some(new_session_name), // Use the name from the command
						None,
						None, // Keep using the default model
						None, // Use default temperature
						&current_config,
					)?;

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
						use colored::*;

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
				} else if input.starts_with(LAYERS_COMMAND)
					|| input.starts_with(DEBUG_COMMAND)
					|| input.starts_with(LOGLEVEL_COMMAND)
					|| input.starts_with(TRUNCATE_COMMAND)
				{
					// This is a command that requires config reload
					// Reload the configuration
					match crate::config::Config::load() {
						Ok(updated_config) => {
							// Update our current config with the new role-specific config
							current_config =
								updated_config.get_merged_config_for_mode(&session_args.role);
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
			continue;
		}

		// Check for cancellation before starting layered processing
		if ctrl_c_pressed.load(Ordering::SeqCst) {
			continue;
		}

		// SIMPLIFIED FLOW:
		// 1. Process through layers if needed (first message with layers enabled)
		// 2. Use the processed input for the main model chat

		// If layers are enabled and this is the first message, process it through layers first
		if current_config.get_enable_layers(&session_args.role)
			&& !first_message_processed
			&& session_args.role == "developer"
		{
			// Set processing state to layers
			*processing_state.lock().unwrap() = ProcessingState::ProcessingLayers;

			// This is the first message with layered architecture enabled
			// We will process it through layers to get improved input for the main model

			// Check for Ctrl+C before starting layered processing
			if ctrl_c_pressed.load(Ordering::SeqCst) {
				continue;
			}

			// Process using layered architecture to get improved input
			// Each layer processes function calls with its own model internally,
			// so the final output already incorporates all function call results
			let layered_result = super::super::layered_response::process_layered_response(
				&input,
				&mut chat_session,
				&current_config,
				&session_args.role,
				operation_cancelled.clone(),
			)
			.await;

			match layered_result {
				Ok(processed_input) => {
					// Check for cancellation after layer processing
					if ctrl_c_pressed.load(Ordering::SeqCst) {
						continue;
					}

					// Use the processed input from layers instead of the original input
					// This processed input already includes any function call responses
					input = processed_input;

					// Mark that we've processed the first message through layers
					first_message_processed = true;

					log_info!(
						"{}",
						"Layers processing complete. Using enhanced input for main model."
					);
				}
				Err(e) => {
					// Check for cancellation in error case
					if ctrl_c_pressed.load(Ordering::SeqCst) {
						continue;
					}

					// Print colorful error message and continue with original input
					use colored::*;
					println!(
						"\n{}: {}",
						"Error processing through layers".bright_red(),
						e
					);
					println!("{}", "Continuing with original input.".yellow());
					// Still mark as processed to avoid infinite retry loops
					first_message_processed = true;
				}
			}
		}

		// Store the user message index before adding it
		*last_user_message_index.lock().unwrap() = Some(chat_session.session.messages.len());

		// UNIFIED STANDARD PROCESSING FLOW
		// The same code path is used whether the input is from layers or direct user input

		// Add user message for standard processing flow
		chat_session.add_user_message(&input)?;

		// Check if we need to truncate the context to stay within token limits
		let truncate_cancelled = Arc::new(AtomicBool::new(false));
		check_and_truncate_context(
			&mut chat_session,
			&current_config,
			&session_args.role,
			truncate_cancelled.clone(),
		)
		.await?;

		// Ensure system message is cached before making API calls
		let mut system_message_cached = false;

		// Check if system message is already cached
		for msg in &chat_session.session.messages {
			if msg.role == "system" && msg.cached {
				system_message_cached = true;
				break;
			}
		}

		// If system message not already cached, add a cache checkpoint
		if !system_message_cached {
			if let Ok(cached) = chat_session.session.add_cache_checkpoint(true) {
				if cached && crate::session::model_supports_caching(&chat_session.model) {
					log_info!(
						"{}",
						"System message has been automatically marked for caching to save tokens."
					);
					// Save the session to ensure the cached status is persisted
					let _ = chat_session.save();
				}
			}
		}

		// Set processing state to calling API
		*processing_state.lock().unwrap() = ProcessingState::CallingAPI;

		// Call OpenRouter in a separate task
		let model = chat_session.model.clone();
		let temperature = chat_session.temperature;
		let config_clone = current_config.clone();

		// Create a task to show loading animation with current cost
		let animation_cancel = operation_cancelled.clone();
		let current_cost = chat_session.session.info.total_cost;
		let animation_task = tokio::spawn(async move {
			let _ = show_loading_animation(animation_cancel, current_cost).await;
		});

		// Start a separate task to monitor for Ctrl+C and propagate to operation_cancelled flag
		let op_cancelled = operation_cancelled.clone();
		let ctrlc_flag = ctrl_c_pressed.clone();
		let _cancel_monitor = tokio::spawn(async move {
			while !op_cancelled.load(Ordering::SeqCst) {
				// Check if global Ctrl+C flag is set
				if ctrlc_flag.load(Ordering::SeqCst) {
					// Set the operation cancellation flag immediately
					op_cancelled.store(true, Ordering::SeqCst);
					break; // Exit the loop once cancelled
				}
				// Use very fast polling for immediate response
				tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;
			}
		});

		// Check for Ctrl+C before making API call
		if ctrl_c_pressed.load(Ordering::SeqCst) {
			// Immediately stop and return to main loop
			operation_cancelled.store(true, Ordering::SeqCst);
			let _ = animation_task.await;
			continue;
		}

		// Check spending threshold before making API call
		match chat_session.check_spending_threshold(&current_config) {
			Ok(should_continue) => {
				if !should_continue {
					// User chose not to continue due to spending threshold
					operation_cancelled.store(true, Ordering::SeqCst);
					let _ = animation_task.await;
					continue;
				}
			}
			Err(e) => {
				// Error checking threshold, log and continue
				use colored::*;
				println!("{}: {}", "Warning: Error checking spending threshold".bright_yellow(), e);
			}
		}

		// Now directly perform the API call - ensure usage parameter is included
		// for consistent cost tracking across all API requests
		let api_result = crate::session::chat_completion_with_provider(
			&chat_session.session.messages,
			&model,
			temperature,
			&config_clone,
		)
		.await;

		// Stop the animation - but use TRUE to stop it, not false!
		operation_cancelled.store(true, Ordering::SeqCst);
		let _ = animation_task.await;

		// Check for Ctrl+C again before processing response
		if ctrl_c_pressed.load(Ordering::SeqCst) {
			// Skip processing response if Ctrl+C was pressed during API call
			continue;
		}

		// Process the response
		match api_result {
			Ok(response) => {
				// Set processing state based on whether we have tool calls
				if response
					.tool_calls
					.as_ref()
					.is_some_and(|calls| !calls.is_empty())
				{
					*processing_state.lock().unwrap() = ProcessingState::ExecutingTools;
				} else {
					*processing_state.lock().unwrap() = ProcessingState::ProcessingResponse;
				}

				// Process the response, handling tool calls recursively
				// Create a fresh cancellation flag to avoid any "Operation cancelled" messages when not requested
				let tool_process_cancelled = Arc::new(AtomicBool::new(false));

				// Connect global cancellation to tool processing cancellation
				let tool_cancelled_clone = tool_process_cancelled.clone();
				let ctrl_c_clone = ctrl_c_pressed.clone();
				let _tool_cancel_monitor = tokio::spawn(async move {
					while !tool_cancelled_clone.load(Ordering::SeqCst) {
						if ctrl_c_clone.load(Ordering::SeqCst) {
							tool_cancelled_clone.store(true, Ordering::SeqCst);
							break;
						}
						// Very fast polling for immediate tool cancellation
						tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;
					}
				});

				// Convert to legacy format for compatibility
				let legacy_exchange = response.exchange;

				let process_result = process_response(
					response.content,
					legacy_exchange,
					response.tool_calls,
					response.finish_reason,
					&mut chat_session,
					&current_config,
					&session_args.role,
					tool_process_cancelled.clone(),
				)
				.await;

				// Update processing state to completed when done
				*processing_state.lock().unwrap() = ProcessingState::CompletedWithResults;

				if let Err(e) = process_result {
					// Print colorful error message
					use colored::*;
					println!("\n{}: {}", "Error processing response".bright_red(), e);
				}
			}
			Err(e) => {
				// Print colorful error message with provider-aware context
				use colored::*;
				
				// Extract provider name from the model string
				let provider_name = if let Ok((provider, _)) = crate::session::providers::ProviderFactory::parse_model(&model) {
					provider
				} else {
					"unknown provider".to_string()
				};
				
				println!("\n{}: {}", format!("Error calling {}", provider_name).bright_red(), e);
				
				// Provider-specific help message
				match provider_name.to_lowercase().as_str() {
					"openrouter" => {
						println!("{}", "Make sure OpenRouter API key is set in the config or as OPENROUTER_API_KEY environment variable.".yellow());
					}
					"anthropic" => {
						println!("{}", "Make sure Anthropic API key is set in the config or as ANTHROPIC_API_KEY environment variable.".yellow());
					}
					"openai" => {
						println!("{}", "Make sure OpenAI API key is set in the config or as OPENAI_API_KEY environment variable.".yellow());
					}
					"google" => {
						println!("{}", "Make sure Google credentials are set in the config or as GOOGLE_APPLICATION_CREDENTIALS environment variable.".yellow());
					}
					"amazon" => {
						println!("{}", "Make sure AWS credentials are configured properly for Amazon Bedrock access.".yellow());
					}
					"cloudflare" => {
						println!("{}", "Make sure Cloudflare API key is set in the config or as CLOUDFLARE_API_KEY environment variable.".yellow());
					}
					_ => {
						println!("{}", "Make sure the API key for this provider is properly configured.".yellow());
					}
				}
			}
		}
	}

	Ok(())
}
