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

use super::super::animation::{show_loading_animation, show_no_animation};
use super::super::commands::*;
use super::super::context_truncation::check_and_truncate_context;
use super::super::input::read_user_input;
use super::super::response::{process_response, ResponseProcessingParams};
use super::core::{ChatSession, SessionInitParams};
use crate::config::Config;
use crate::session::{create_system_prompt, ChatCompletionWithValidationParams};
use crate::{log_debug, log_info};
use anyhow::Result;
use std::io::Write; // Added for stdout flushing
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

// Type alias for extracted session parameters
type SessionParams = (
	Option<String>, // name
	Option<String>, // resume
	Option<String>, // model
	Option<u32>,    // max_tokens
	f32,            // temperature
	String,         // role
	u32,            // max_retries
);

// Helper function to extract session parameters from Debug format
// This allows both SessionArgs and RunArgs (after conversion) to work
fn extract_session_params<T: std::fmt::Debug>(args: &T) -> SessionParams {
	let args_str = format!("{:?}", args);

	// Get model
	let model = if args_str.contains("model: Some(\"") {
		let start = args_str.find("model: Some(\"").unwrap() + 13;
		let end = args_str[start..].find('"').unwrap() + start;
		Some(args_str[start..end].to_string())
	} else {
		None
	};

	// Get name
	let name = if args_str.contains("name: Some(\"") {
		let start = args_str.find("name: Some(\"").unwrap() + 12;
		let end = args_str[start..].find('"').unwrap() + start;
		Some(args_str[start..end].to_string())
	} else {
		None
	};

	// Get resume
	let resume = if args_str.contains("resume: Some(\"") {
		let start = args_str.find("resume: Some(\"").unwrap() + 14;
		let end = args_str[start..].find('"').unwrap() + start;
		Some(args_str[start..end].to_string())
	} else {
		None
	};

	// Get role
	let role = if args_str.contains("role: \"") {
		let start = args_str.find("role: \"").unwrap() + 7;
		let end = args_str[start..].find('"').unwrap() + start;
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

	// Get max_tokens
	let max_tokens = if args_str.contains("max_tokens: ") {
		let start = args_str.find("max_tokens: ").unwrap() + 12;
		let end = args_str[start..].find(',').unwrap_or(
			args_str[start..]
				.find('}')
				.unwrap_or(args_str.len() - start),
		) + start;
		args_str[start..end].trim().parse::<u32>().ok()
	} else {
		None // No max_tokens specified
	};

	// Get max_retries
	let max_retries = if args_str.contains("max_retries: ") {
		let start = args_str.find("max_retries: ").unwrap() + 13;
		let end = args_str[start..].find(',').unwrap_or(
			args_str[start..]
				.find('}')
				.unwrap_or(args_str.len() - start),
		) + start;
		args_str[start..end].trim().parse::<u32>().unwrap_or(0)
	} else {
		0 // Default max_retries
	};

	(
		name,
		resume,
		model,
		max_tokens,
		temperature,
		role,
		max_retries,
	)
}

// Run an interactive session
pub async fn run_interactive_session<T: std::fmt::Debug>(args: &T, config: &Config) -> Result<()> {
	// Extract session parameters
	let (name, resume, model, max_tokens, temperature, role, max_retries) =
		extract_session_params(args);
	// For developer role, show MCP server status
	let current_dir = std::env::current_dir()?;

	// Get the merged configuration for the specified role
	let config_for_role = config.get_merged_config_for_role(&role);

	// Create or load session
	let mut session_params = SessionInitParams::new(&config_for_role, &role);

	if let Some(name) = name {
		session_params = session_params.with_name(name);
	}
	if let Some(resume) = resume {
		session_params = session_params.with_resume(resume);
	}
	if let Some(model) = model.clone() {
		session_params = session_params.with_model(model);
	}
	session_params = session_params.with_temperature(temperature);
	if let Some(max_tokens) =
		max_tokens.or_else(|| Some(config_for_role.get_effective_max_tokens()))
	{
		session_params = session_params.with_max_tokens(max_tokens);
	}
	session_params = session_params.with_max_retries(max_retries);

	let mut chat_session = ChatSession::initialize(session_params)?;

	// If runtime model override is provided, update the session's model (runtime only)
	if let Some(runtime_model) = &model {
		chat_session.model = runtime_model.clone();
		log_info!("Using runtime model override: {}", runtime_model);
	}

	// Always set the temperature from the command line (runtime only)
	chat_session.temperature = temperature;

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
		// Create system prompt based on role - use merged config for role
		let system_prompt = create_system_prompt(&current_dir, &config_for_role, &role).await;
		chat_session.add_system_message(&system_prompt)?;

		// Process layer system prompts during session initialization
		// This ensures layer system prompts are processed once and cached for the entire session
		let (role_config, _, _, _, _) = config_for_role.get_role_config(&role);
		if role_config.enable_layers {
			use crate::session::layers::LayeredOrchestrator;
			// Create orchestrator with processed system prompts - use original config for layers
			let _orchestrator = LayeredOrchestrator::from_config_with_processed_prompts(
				config,
				&role,
				&current_dir,
			)
			.await;
			log_info!("Layer system prompts processed and cached for session");
		}

		// CRITICAL FIX: Apply automatic cache markers for system messages AND tool definitions
		// This ensures consistent caching behavior across all supported models
		let supports_caching = crate::session::model_supports_caching(&chat_session.model);
		let has_tools = !config_for_role.mcp.servers.is_empty();

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
			log_info!(
				"Note: This model doesn't support caching, but system prompt is still optimized."
			);
		}

		// Add assistant welcome message with variable processing
		let role_config = config.get_role_config_struct(&role);
		let welcome_message =
			crate::session::helper_functions::process_placeholders_async_with_role(
				&role_config.welcome,
				&current_dir,
				Some(&role),
			)
			.await;

		chat_session.add_assistant_message(&welcome_message, None, &config_for_role, &role)?;

		// Apply cache marker to welcome message as well for consistency
		if supports_caching {
			let cache_manager = crate::session::cache::CacheManager::new();
			cache_manager.add_automatic_cache_markers(
				&mut chat_session.session.messages,
				has_tools,
				supports_caching,
			);
		}

		// Check for custom instructions file in current working directory
		let instructions_filename = &config.custom_instructions_file_name;
		if !instructions_filename.is_empty() {
			let instructions_path = current_dir.join(instructions_filename);
			if instructions_path.exists() {
				match std::fs::read_to_string(&instructions_path) {
					Ok(instructions_content) => {
						// Process placeholders in instructions content using the same pattern as welcome message
						let processed_instructions =
							crate::session::helper_functions::process_placeholders_async_with_role(
								&instructions_content,
								&current_dir,
								Some(&role),
							)
							.await;

						// Add as user message after welcome message
						chat_session.add_user_message(&processed_instructions)?;

						// Apply cache marker to the assistant message (welcome message) if caching is supported
						// This ensures the assistant message before the instructions is cached
						if supports_caching {
							let cache_manager = crate::session::cache::CacheManager::new();
							cache_manager.add_automatic_cache_markers(
								&mut chat_session.session.messages,
								has_tools,
								supports_caching,
							);
						}

						log_info!(
							"Added {} content as user message with variable processing",
							instructions_filename
						);
					}
					Err(e) => {
						log_debug!("Failed to read {}: {}", instructions_filename, e);
					}
				}
			}
		}
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

	// Enhanced processing state tracking for smart cancellation
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

	// Smart operation tracking for surgical cleanup
	#[derive(Debug, Clone)]
	struct OperationContext {
		user_message_index: Option<usize>,
		assistant_message_index: Option<usize>,
		operation_id: String,
		has_tool_calls: bool,
		completed_tool_ids: Vec<String>,
	}

	let current_operation = Arc::new(std::sync::Mutex::new(None::<OperationContext>));

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
				println!(
					"\n🛑 Interrupting tool execution... Killing processes... Ready for new input"
				);
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
	let mut current_config = config_for_role.clone();

	// Set the thread-local config for logging macros
	crate::config::set_thread_config(&current_config);

	// Main interaction loop
	loop {
		// Set processing state to idle
		*processing_state.lock().unwrap() = ProcessingState::Idle;

		// SMART CANCELLATION: Handle cancellation with surgical cleanup
		if ctrl_c_pressed.load(Ordering::SeqCst) {
			log_debug!("Ctrl+C detected - performing smart cleanup based on operation state");

			// CRITICAL FIX: Display cost information before cleanup
			// This ensures users see the cost spent before cancellation
			use crate::session::chat::cost_tracker::CostTracker;
			CostTracker::display_cost_line(&chat_session);

			let current_state = processing_state.lock().unwrap().clone();
			let operation = current_operation.lock().unwrap().clone();

			match current_state {
				ProcessingState::Idle | ProcessingState::ReadingInput => {
					// Nothing to clean up - just reset and continue
					log_debug!("Cancelled during idle state - no cleanup needed");
				}
				ProcessingState::ProcessingLayers => {
					// Layers processing was interrupted - remove only the current user message if it was added
					if let Some(op) = operation {
						if let Some(user_idx) = op.user_message_index {
							if user_idx < chat_session.session.messages.len() {
								chat_session.session.messages.truncate(user_idx);
								log_debug!("Removed incomplete user message due to layer processing cancellation");
							}
						}
					}
				}
				ProcessingState::CallingAPI => {
					// API call was interrupted - remove only incomplete assistant response if any
					if let Some(op) = operation {
						if let Some(assistant_idx) = op.assistant_message_index {
							// Remove incomplete assistant message
							if assistant_idx < chat_session.session.messages.len() {
								chat_session.session.messages.truncate(assistant_idx);
								log_debug!("Removed incomplete assistant response due to API call cancellation");
							}
						}
						// Keep user message - it's complete and valid
					}
				}
				ProcessingState::ExecutingTools => {
					// Tool execution was interrupted - cleanup is now handled immediately in response.rs
					// This ensures conversation state integrity without waiting for next loop iteration
					log_debug!("Tool execution cancelled - cleanup handled immediately during response processing");
				}
				ProcessingState::ProcessingResponse => {
					// Response processing was interrupted - minimal cleanup
					// Most work is already done, just ensure consistency
					log_debug!("Cancelled during response processing - preserving completed work");
				}
				ProcessingState::CompletedWithResults => {
					// Operation completed successfully - nothing to clean up
					log_debug!("Cancelled after completion - all work preserved");
				}
			}

			// Save the session after cleanup to persist changes
			if let Err(e) = chat_session.save() {
				log_debug!("Warning: Failed to save session after smart cleanup: {}", e);
			}

			// Reset for next iteration
			ctrl_c_pressed.store(false, Ordering::SeqCst);
			*current_operation.lock().unwrap() = None;
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
					&role,
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
				.process_command(&input, &mut current_config, &role)
				.await?;
			if exit {
				// First check if it's a session switch command
				if input.starts_with(SESSION_COMMAND) {
					// We need to switch to another session
					let new_session_name = chat_session.session.info.name.clone();

					// Save current session before switching
					chat_session.save()?;

					// Initialize the new session
					let session_params = SessionInitParams::new(&current_config, &role)
						.with_name(new_session_name)
						.with_max_retries(max_retries);
					let new_chat_session = ChatSession::initialize(session_params)?;

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
				} else if input.starts_with(LAYERS_COMMAND) {
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
		if current_config.get_enable_layers(&role) && !first_message_processed {
			// Set processing state to layers
			*processing_state.lock().unwrap() = ProcessingState::ProcessingLayers;

			// This is the first message with layered architecture enabled
			// We will process it through layers to get improved input for the main model

			// Check for Ctrl+C before starting layered processing
			if ctrl_c_pressed.load(Ordering::SeqCst) {
				continue;
			}

			// Track session message count before layer processing

			let messages_before_layers = chat_session.session.messages.len();

			// Process using layered architecture to get improved input
			// Each layer processes function calls with its own model internally,
			// so the final output already incorporates all function call results
			let layered_result = super::super::layered_response::process_layered_response(
				&input,
				&mut chat_session,
				&current_config,
				&role,
				operation_cancelled.clone(),
			)
			.await;

			match layered_result {
				Ok(processed_input) => {
					// Check for cancellation after layer processing
					if ctrl_c_pressed.load(Ordering::SeqCst) {
						continue;
					}

					// Check if layers modified the session (added messages via output_mode)
					let messages_after_layers = chat_session.session.messages.len();
					let layers_modified_session = messages_after_layers > messages_before_layers;

					if layers_modified_session {
						// Layers used output_mode append/replace and added messages to session
						// Skip adding user message to avoid duplicates
						log_info!(
							"Layers modified session ({} messages added).",
							messages_after_layers - messages_before_layers
						);

						// Mark that we've processed the first message through layers
						// We will continue here with the user message
						// to guarantee that the otput from layer next
						// processed with the main loop
						first_message_processed = true;
					} else {
						// Layers didn't modify session (all had output_mode = none)
						// Use the processed input from layers instead of the original input
						input = processed_input;

						// Mark that we've processed the first message through layers
						first_message_processed = true;

						log_info!(
							"{}",
							"Layers processing complete. Using enhanced input for main model."
						);
					}
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

		// Initialize operation context for smart tracking
		let operation_id = format!(
			"op_{}",
			std::time::SystemTime::now()
				.duration_since(std::time::UNIX_EPOCH)
				.unwrap_or_default()
				.as_millis()
		);

		let user_message_index = chat_session.session.messages.len();

		// UNIFIED STANDARD PROCESSING FLOW
		// The same code path is used whether the input is from layers or direct user input

		// Add user message for standard processing flow
		chat_session.add_user_message(&input)?;

		// Create operation context for tracking
		*current_operation.lock().unwrap() = Some(OperationContext {
			user_message_index: Some(user_message_index),
			assistant_message_index: None,
			operation_id: operation_id.clone(),
			has_tool_calls: false,
			completed_tool_ids: Vec::new(),
		});

		log_debug!(
			"Started operation {} with user message at index {}",
			operation_id,
			user_message_index
		);

		// Check if we need to truncate the context to stay within token limits
		let truncate_cancelled = Arc::new(AtomicBool::new(false));
		check_and_truncate_context(
			&mut chat_session,
			&current_config,
			&role,
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
		// Use a separate flag for animation to avoid conflicts with user cancellation detection
		let animation_cancel = Arc::new(AtomicBool::new(false));
		let animation_cancel_clone = animation_cancel.clone();
		let current_cost = chat_session.session.info.total_cost;
		let animation_task = tokio::spawn(async move {
			let _ = show_loading_animation(animation_cancel_clone, current_cost).await;
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
				println!(
					"{}: {}",
					"Warning: Error checking spending threshold".bright_yellow(),
					e
				);
			}
		}

		// Now perform the API call with input validation and context management
		// This will check input size and prompt user for action if limits are exceeded
		// Clone messages to avoid borrowing conflicts
		let messages = chat_session.session.messages.clone();
		let max_retries = chat_session.max_retries; // Extract before mutable borrow
		let validation_params = ChatCompletionWithValidationParams::new(
			&messages,
			&model,
			temperature,
			chat_session.max_tokens,
			&config_clone,
		)
		.with_max_retries(max_retries)
		.with_chat_session(&mut chat_session)
		.with_cancellation_token(operation_cancelled.clone());
		let api_result = crate::session::chat_completion_with_validation(validation_params).await;

		// Stop the animation using the separate animation flag (not the operation_cancelled flag)
		animation_cancel.store(true, Ordering::SeqCst);
		let _ = animation_task.await;

		// Check for Ctrl+C again before processing response
		if ctrl_c_pressed.load(Ordering::SeqCst) {
			// Skip processing response if Ctrl+C was pressed during API call
			continue;
		}

		// Process the response
		match api_result {
			Ok(response) => {
				// Update operation context with assistant message info
				if let Some(ref mut op) = *current_operation.lock().unwrap() {
					op.assistant_message_index = Some(chat_session.session.messages.len());
					op.has_tool_calls = response
						.tool_calls
						.as_ref()
						.is_some_and(|calls| !calls.is_empty());
					log_debug!(
						"Updated operation {} with assistant message at index {}",
						op.operation_id,
						chat_session.session.messages.len()
					);
				}

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

				let process_result = process_response(ResponseProcessingParams::new(
					response.content,
					legacy_exchange,
					response.tool_calls,
					response.finish_reason,
					&mut chat_session,
					&current_config,
					&role,
					tool_process_cancelled.clone(),
				))
				.await;

				// After processing, update operation context with completed tool IDs
				if let Some(ref mut op) = *current_operation.lock().unwrap() {
					// Find all tool result messages that were added during this operation
					let mut completed_tools = Vec::new();
					for msg in &chat_session.session.messages {
						if msg.role == "tool" {
							if let Some(tool_id) = &msg.tool_call_id {
								if !op.completed_tool_ids.contains(tool_id) {
									completed_tools.push(tool_id.clone());
								}
							}
						}
					}
					op.completed_tool_ids.extend(completed_tools.clone());
					if !completed_tools.is_empty() {
						log_debug!(
							"Operation {} completed tools: {:?}",
							op.operation_id,
							completed_tools
						);
					}
				}

				// Update processing state to completed when done
				*processing_state.lock().unwrap() = ProcessingState::CompletedWithResults;

				if let Err(e) = process_result {
					// Print colorful error message
					use colored::*;
					println!("\n{}: {}", "Error processing response".bright_red(), e);
				}
			}
			Err(e) => {
				// CRITICAL FIX: Remove the user message that was added before the failed API call
				// This prevents the failed message from polluting the conversation context
				if let Some(ref op) = *current_operation.lock().unwrap() {
					if let Some(user_idx) = op.user_message_index {
						if user_idx < chat_session.session.messages.len() {
							chat_session.session.messages.truncate(user_idx);
							log_debug!("Removed user message due to API call failure");
						}
					}
				}

				// Print colorful error message with provider-aware context
				use colored::*;

				// Extract provider name from the model string
				let provider_name = if let Ok((provider, _)) =
					crate::providers::ProviderFactory::parse_model(&model)
				{
					provider
				} else {
					"unknown provider".to_string()
				};

				println!(
					"\n{}: {}",
					format!("Error calling {}", provider_name).bright_red(),
					e
				);

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
						println!(
							"{}",
							"Make sure the API key for this provider is properly configured."
								.yellow()
						);
					}
				}
			}
		}

		// Clear operation context at the end of each successful iteration
		*current_operation.lock().unwrap() = None;
	}

	Ok(())
}

// Run a single non-interactive session with provided input
// THIS IS just helper and USED as simplified version of interactive session
// That used for run command THAT is not interactive and get request and process it
// in the same way session procsss interactive request from the user but without inetractive
pub async fn run_interactive_session_with_input<T: std::fmt::Debug>(
	args: &T,
	config: &Config,
	initial_input: &str,
) -> Result<()> {
	// Extract session parameters
	let (name, resume, model, max_tokens, temperature, role, max_retries) =
		extract_session_params(args);

	// Suppress MCP server status messages for non-interactive mode
	let current_dir = std::env::current_dir()?;

	// Get the merged configuration for the specified role
	let config_for_role = config.get_merged_config_for_role(&role);

	// Create or load session - same as interactive
	let mut session_params = SessionInitParams::new(&config_for_role, &role);

	if let Some(name) = name {
		session_params = session_params.with_name(name);
	}
	if let Some(resume) = resume {
		session_params = session_params.with_resume(resume);
	}
	if let Some(model) = model.clone() {
		session_params = session_params.with_model(model);
	}
	session_params = session_params.with_temperature(temperature);
	if let Some(max_tokens) =
		max_tokens.or_else(|| Some(config_for_role.get_effective_max_tokens()))
	{
		session_params = session_params.with_max_tokens(max_tokens);
	}
	session_params = session_params.with_max_retries(max_retries);

	let mut chat_session = ChatSession::initialize(session_params)?;

	// Apply runtime overrides - same as interactive
	if let Some(runtime_model) = &model {
		chat_session.model = runtime_model.clone();
		log_info!("Using runtime model override: {}", runtime_model);
	}
	chat_session.temperature = temperature;

	// Track if the first message has been processed through layers
	let first_message_processed = !chat_session.session.messages.is_empty();

	// Initialize with system prompt if new session - same as interactive
	if chat_session.session.messages.is_empty() {
		let system_prompt = create_system_prompt(&current_dir, &config_for_role, &role).await;
		chat_session.add_system_message(&system_prompt)?;

		// Process layer system prompts - same as interactive
		let (role_config, _, _, _, _) = config.get_role_config(&role);
		if role_config.enable_layers {
			use crate::session::layers::LayeredOrchestrator;
			let _orchestrator = LayeredOrchestrator::from_config_with_processed_prompts(
				config,
				&role,
				&current_dir,
			)
			.await;
			log_info!("Layer system prompts processed and cached for session");
		}

		// Apply automatic cache markers - same as interactive
		let supports_caching = crate::session::model_supports_caching(&chat_session.model);
		let has_tools = !config_for_role.mcp.servers.is_empty();

		if supports_caching {
			let cache_manager = crate::session::cache::CacheManager::new();
			cache_manager.add_automatic_cache_markers(
				&mut chat_session.session.messages,
				has_tools,
				supports_caching,
			);
			log_info!("System prompt has been automatically marked for caching to save tokens in future interactions.");
			let _ = chat_session.save();
		} else {
			log_info!(
				"Note: This model doesn't support caching, but system prompt is still optimized."
			);
		}

		// Add assistant welcome message - same as interactive
		let role_config = config.get_role_config_struct(&role);
		let welcome_message =
			crate::session::helper_functions::process_placeholders_async_with_role(
				&role_config.welcome,
				&current_dir,
				Some(&role),
			)
			.await;

		chat_session.add_assistant_message(&welcome_message, None, &config_for_role, &role)?;

		// Apply cache marker to welcome message - same as interactive
		if supports_caching {
			let cache_manager = crate::session::cache::CacheManager::new();
			cache_manager.add_automatic_cache_markers(
				&mut chat_session.session.messages,
				has_tools,
				supports_caching,
			);
		}

		// Check for custom instructions file - same as interactive
		let instructions_filename = &config.custom_instructions_file_name;
		if !instructions_filename.is_empty() {
			let instructions_path = current_dir.join(instructions_filename);
			if instructions_path.exists() {
				match std::fs::read_to_string(&instructions_path) {
					Ok(instructions_content) => {
						let processed_instructions =
							crate::session::helper_functions::process_placeholders_async_with_role(
								&instructions_content,
								&current_dir,
								Some(&role),
							)
							.await;

						chat_session.add_user_message(&processed_instructions)?;

						if supports_caching {
							let cache_manager = crate::session::cache::CacheManager::new();
							cache_manager.add_automatic_cache_markers(
								&mut chat_session.session.messages,
								has_tools,
								supports_caching,
							);
						}

						log_info!(
							"Added {} content as user message with variable processing",
							instructions_filename
						);
					}
					Err(e) => {
						log_debug!("Failed to read {}: {}", instructions_filename, e);
					}
				}
			}
		}
	}

	// Set up cancellation handling for non-interactive mode (simplified)
	let ctrl_c_pressed = Arc::new(AtomicBool::new(false));
	let ctrl_c_pressed_clone = ctrl_c_pressed.clone();

	// Simplified Ctrl+C handler for non-interactive mode
	ctrlc::set_handler(move || {
		ctrl_c_pressed_clone.store(true, Ordering::SeqCst);
		println!("\n🛑 Operation cancelled by user");
		std::process::exit(130); // Exit immediately in non-interactive mode
	})
	.expect("Error setting Ctrl+C handler");

	// Set the thread-local config for logging macros
	let mut current_config = config_for_role.clone();
	crate::config::set_thread_config(&current_config);

	// Process the single input (same logic as interactive session)
	let mut input = initial_input.to_string();
	let operation_cancelled = Arc::new(AtomicBool::new(false));

	// Check if this is a command (same logic as interactive session)
	if input.starts_with('/') {
		use colored::*;

		// Handle special /done command separately
		if input.trim() == "/done" {
			println!(
				"{}",
				"✓ Session optimized and ready for next message".bright_green()
			);
			let _ = chat_session.save();
			return Ok(());
		}

		// Process the command
		let exit = chat_session
			.process_command(&input, &mut current_config, &role)
			.await?;

		if exit {
			// Check if it's a session switch command
			if input.starts_with(crate::session::chat::commands::SESSION_COMMAND) {
				println!("{}", "Note: Session switching is not supported in run mode. Use 'octomind session' for interactive session management.".yellow());
			}
		}

		// Save session after command execution
		let _ = chat_session.save();
		return Ok(());
	}

	// Layer processing if enabled and first message - same as interactive
	if current_config.get_enable_layers(&role) && !first_message_processed {
		// Track session message count before layer processing
		let messages_before_layers = chat_session.session.messages.len();

		// Process using layered architecture - same as interactive
		let layered_result = super::super::layered_response::process_layered_response(
			&input,
			&mut chat_session,
			&current_config,
			&role,
			operation_cancelled.clone(),
		)
		.await;

		match layered_result {
			Ok(processed_input) => {
				// Check if layers modified the session
				let messages_after_layers = chat_session.session.messages.len();
				let layers_modified_session = messages_after_layers > messages_before_layers;

				if layers_modified_session {
					// Layers used output_mode append/replace - session already has the messages
					log_info!(
						"Layers modified session ({} messages added). Skipping user message addition.",
						messages_after_layers - messages_before_layers
					);
					// Save session and exit - processing is complete
					let _ = chat_session.save();
					return Ok(());
				} else {
					// Use processed input from layers
					input = processed_input;
					log_info!("Layers processing complete. Using enhanced input for main model.");
				}
			}
			Err(e) => {
				use colored::*;
				println!(
					"\n{}: {}",
					"Error processing through layers".bright_red(),
					e
				);
				println!("{}", "Continuing with original input.".yellow());
			}
		}
	}

	// Add user message - same as interactive
	let user_message_index = chat_session.session.messages.len();
	chat_session.add_user_message(&input)?;

	// Check and truncate context - same as interactive
	let truncate_cancelled = Arc::new(AtomicBool::new(false));
	check_and_truncate_context(
		&mut chat_session,
		&current_config,
		&role,
		truncate_cancelled.clone(),
	)
	.await?;

	// Ensure system message is cached - same as interactive
	let mut system_message_cached = false;
	for msg in &chat_session.session.messages {
		if msg.role == "system" && msg.cached {
			system_message_cached = true;
			break;
		}
	}

	if !system_message_cached {
		if let Ok(cached) = chat_session.session.add_cache_checkpoint(true) {
			if cached && crate::session::model_supports_caching(&chat_session.model) {
				log_info!(
					"System message has been automatically marked for caching to save tokens."
				);
				let _ = chat_session.save();
			}
		}
	}

	// Show no animation for non-interactive mode
	let animation_cancel = Arc::new(AtomicBool::new(false));
	let animation_cancel_clone = animation_cancel.clone();
	let current_cost = chat_session.session.info.total_cost;
	let animation_task = tokio::spawn(async move {
		let _ = show_no_animation(animation_cancel_clone, current_cost).await;
	});

	// Auto-accept spending threshold for non-interactive mode
	// Skip the spending threshold check - auto-proceed in non-interactive mode

	// Make API call - same as interactive
	let model = chat_session.model.clone();
	let temperature = chat_session.temperature;
	let config_clone = current_config.clone();

	let messages = chat_session.session.messages.clone();
	let max_retries = chat_session.max_retries; // Extract before mutable borrow
	let validation_params = ChatCompletionWithValidationParams::new(
		&messages,
		&model,
		temperature,
		chat_session.max_tokens,
		&config_clone,
	)
	.with_max_retries(max_retries)
	.with_chat_session(&mut chat_session)
	.with_cancellation_token(operation_cancelled.clone());
	let api_result = crate::session::chat_completion_with_validation(validation_params).await;

	// Stop animation
	animation_cancel.store(true, Ordering::SeqCst);
	let _ = animation_task.await;

	// Process response - same as interactive
	match api_result {
		Ok(response) => {
			// Process the response with tool calls - same as interactive
			let tool_process_cancelled = Arc::new(AtomicBool::new(false));
			let legacy_exchange = response.exchange;

			let process_result = process_response(ResponseProcessingParams::new(
				response.content,
				legacy_exchange,
				response.tool_calls,
				response.finish_reason,
				&mut chat_session,
				&current_config,
				&role,
				tool_process_cancelled.clone(),
			))
			.await;

			if let Err(e) = process_result {
				use colored::*;
				println!("\n{}: {}", "Error processing response".bright_red(), e);
			}
		}
		Err(e) => {
			// Remove user message on API failure - same as interactive
			if user_message_index < chat_session.session.messages.len() {
				chat_session.session.messages.truncate(user_message_index);
				log_debug!("Removed user message due to API call failure");
			}

			// Print error with provider context - same as interactive
			use colored::*;
			let provider_name =
				if let Ok((provider, _)) = crate::providers::ProviderFactory::parse_model(&model) {
					provider
				} else {
					"unknown provider".to_string()
				};

			println!(
				"\n{}: {}",
				format!("Error calling {}", provider_name).bright_red(),
				e
			);

			// Provider-specific help - same as interactive
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
					println!(
						"{}",
						"Make sure the API key for this provider is properly configured.".yellow()
					);
				}
			}
		}
	}

	// Save session before exit
	let _ = chat_session.save();

	Ok(())
}
