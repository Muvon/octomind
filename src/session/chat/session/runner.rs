// Interactive session runner

use super::core::ChatSession;
use crate::{log_debug, log_info};
use crate::store::Store;
use crate::config::Config;
use crate::session::{create_system_prompt, openrouter};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::io::Write; // Added for stdout flushing
use anyhow::Result;
use super::super::input::read_user_input;
use super::super::response::process_response;
use super::super::animation::show_loading_animation;
use super::super::context_truncation::check_and_truncate_context;
use super::super::commands::*;
use crate::session::indexer;

// Run an interactive session
pub async fn run_interactive_session<T: clap::Args + std::fmt::Debug>(
	args: &T,
	store: &Store,
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

		SessionArgs {
			name,
			resume,
			model,
			role,
		}
	};

	// Check if there's an index, and if not, run indexer (only for developer role)
	let current_dir = std::env::current_dir()?;
	if session_args.role == "developer" {
		let octodev_dir = current_dir.join(".octodev");
		let index_path = octodev_dir.join("storage");
		if !index_path.exists() {
			// Run the indexer directly using our indexer integration
			indexer::index_current_directory(store, config).await?
		} else {
			log_info!("Using existing index from {}", index_path.display());
		}

		// Start a watcher in the background to keep the index updated
		log_info!("Starting watcher to keep index updated during session...");
		indexer::start_watcher_in_background(store, config).await?;
	}

	// Get the merged configuration for the specified role
	let mode_config = config.get_merged_config_for_mode(&session_args.role);

	// Create or load session
	let mut chat_session = ChatSession::initialize(
		session_args.name,
		session_args.resume,
		session_args.model,
		&mode_config
	)?;

	// Track if the first message has been processed through layers
	let mut first_message_processed = !chat_session.session.messages.is_empty();
	println!("Interactive coding session started. Type your questions/requests.");
	println!("Type /help for available commands.");
	
	// Show history usage info for new sessions
	if chat_session.session.messages.is_empty() {
		use colored::*;
		println!("{}", "💡 Tip: Use ↑/↓ arrows or Ctrl+R for command history search".bright_yellow());
	}

	// Initialize with system prompt if new session
	if chat_session.session.messages.is_empty() {
		// Create system prompt based on role
		let system_prompt = create_system_prompt(&current_dir, config, &session_args.role).await;
		chat_session.add_system_message(&system_prompt)?;

		// Mark system message with function declarations as cached by default
		// This ensures all heavy initial context is cached to save on tokens
		if let Ok(cached) = chat_session.session.add_cache_checkpoint(true) {
			if cached && crate::session::model_supports_caching(&chat_session.model) {
				log_info!("System prompt has been marked for caching to save tokens in future interactions.");
				// Save the session to ensure the cached status is persisted
				let _ = chat_session.save();
			} else if !crate::session::model_supports_caching(&chat_session.model) {
				// Don't show warning for models that don't support caching
				log_info!("Note: This model doesn't support caching, but system prompt is still optimized.");
			} else {
				log_info!("Warning: Failed to mark system prompt for caching.");
			}
		} else {
			log_info!("Error: Could not set cache checkpoint for system message.");
		}

		// Add assistant welcome message
		let welcome_message = format!(
			"Hello! Octodev ready to serve you. Working dir: {} (Role: {})",
			current_dir.file_name().unwrap_or_default().to_string_lossy(),
			session_args.role
		);
		chat_session.add_assistant_message(&welcome_message, None, &mode_config)?;

		// Print welcome message with colors if terminal supports them
		use colored::*;
		println!("{}", welcome_message.bright_green());
	} else {
		// Print the last few messages for context with colors if terminal supports them
		let last_messages = chat_session.session.messages.iter().rev().take(3).collect::<Vec<_>>();
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

	// Set up a shared cancellation flag that can be set by Ctrl+C
	let ctrl_c_pressed = Arc::new(AtomicBool::new(false));
	let ctrl_c_pressed_clone = ctrl_c_pressed.clone();

	// Set up Ctrl+C handler for immediate cancellation
	ctrlc::set_handler(move || {
		// If already set, do a hard exit to break out of any operation
		if ctrl_c_pressed_clone.load(Ordering::SeqCst) {
			println!("\n🛑 Forcing exit due to repeated Ctrl+C...");
			std::process::exit(130); // 130 is standard exit code for SIGINT
		}

		ctrl_c_pressed_clone.store(true, Ordering::SeqCst);
		// Immediately display user feedback with visual indicators
		print!("\n🛑 Operation cancelled");
		std::io::stdout().flush().unwrap(); // Force immediate display
		print!(" | 📝 Work preserved");
		std::io::stdout().flush().unwrap();
		print!(" | ✨ Continue with new command");
		std::io::stdout().flush().unwrap();
		println!(" | 🔄 Press Ctrl+C again to force exit");
		std::io::stdout().flush().unwrap(); // Ensure all output is shown immediately
	}).expect("Error setting Ctrl+C handler");

	// We need to handle configuration reloading, so keep our own copy that we can update
	let mut current_config = mode_config.clone();

	// Set the thread-local config for logging macros
	crate::config::set_thread_config(&current_config);

	// Main interaction loop
	loop {
		// Check if Ctrl+C was pressed
		if ctrl_c_pressed.load(Ordering::SeqCst) {
			// Reset for next time
			ctrl_c_pressed.store(false, Ordering::SeqCst);
			println!("Ready for new input.");
			continue;
		}

		// Create a fresh cancellation flag for this iteration
		// Each request gets its own cancellation flag derived from the global one
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
					operation_cancelled.clone()
				).await;

				if let Err(e) = result {
					use colored::*;
					println!("{}: {}", "Error performing context reduction".bright_red(), e);
				} else {
					use colored::*;
					println!("{}", "\nNext message will be processed through the full layered architecture.".bright_green());

					// Apply EditorConfig formatting to all modified files
					let formatter_result = super::super::editorconfig_formatter::apply_editorconfig_formatting(None).await;
					if let Err(e) = formatter_result {
						println!("{}: {}", "Error applying EditorConfig formatting".bright_red(), e);
					}
				}
				continue;
			}

			let exit = chat_session.process_command(&input, &current_config, &session_args.role).await?;
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
						&current_config
					)?;

					// Replace the current chat session
					chat_session = new_chat_session;

					// Reset first message flag for new session
					first_message_processed = !chat_session.session.messages.is_empty();

					// Print the last few messages for context with colors
					if !chat_session.session.messages.is_empty() {
						let last_messages = chat_session.session.messages.iter().rev().take(3).collect::<Vec<_>>();
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
				} else if input.starts_with(LAYERS_COMMAND) || input.starts_with(DEBUG_COMMAND) || input.starts_with(LOGLEVEL_COMMAND) || input.starts_with(TRUNCATE_COMMAND) {
					// This is a command that requires config reload
					// Reload the configuration
					match crate::config::Config::load() {
						Ok(updated_config) => {
							// Update our current config with the new role-specific config
							current_config = updated_config.get_merged_config_for_mode(&session_args.role);
							// Update thread config for logging macros
							crate::config::set_thread_config(&current_config);
							log_info!("Configuration reloaded successfully");
						},
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

		// Create a new cancellation flag for processing the response
		let process_cancelled = Arc::new(AtomicBool::new(false));

		// Check if Ctrl+C was pressed
		if ctrl_c_pressed.load(Ordering::SeqCst) {
			ctrl_c_pressed.store(false, Ordering::SeqCst);
			continue;
		}

		// SIMPLIFIED FLOW:
		// 1. Process through layers if needed (first message with layers enabled)
		// 2. Use the processed input for the main model chat

		// If layers are enabled and this is the first message, process it through layers first
		if current_config.openrouter.enable_layers && !first_message_processed && session_args.role == "developer" {
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
				process_cancelled.clone()
			).await;

			match layered_result {
				Ok(processed_input) => {
					// Use the processed input from layers instead of the original input
					// This processed input already includes any function call responses
					input = processed_input;

					// Mark that we've processed the first message through layers
					first_message_processed = true;

					log_info!("{}", "Layers processing complete. Using enhanced input for main model.");
				},
				Err(e) => {
					// Print colorful error message and continue with original input
					use colored::*;
					println!("\n{}: {}", "Error processing through layers".bright_red(), e);
					println!("{}", "Continuing with original input.".yellow());
					// Still mark as processed to avoid infinite retry loops
					first_message_processed = true;
				}
			}
		}

		// UNIFIED STANDARD PROCESSING FLOW
		// The same code path is used whether the input is from layers or direct user input

		// Add user message for standard processing flow
		chat_session.add_user_message(&input)?;

		// Check if we need to truncate the context to stay within token limits
		let truncate_cancelled = Arc::new(AtomicBool::new(false));
		check_and_truncate_context(&mut chat_session, &current_config, truncate_cancelled.clone()).await?;

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
					log_info!("{}", "System message has been automatically marked for caching to save tokens.");
					// Save the session to ensure the cached status is persisted
					let _ = chat_session.save();
				}
			}
		}

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
				// Use a shorter sleep time to respond to cancellation faster
				tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
			}
		});

		// Check for Ctrl+C before making API call
		if ctrl_c_pressed.load(Ordering::SeqCst) {
			// Immediately stop and return to main loop
			operation_cancelled.store(true, Ordering::SeqCst);
			let _ = animation_task.await;
			continue;
		}

		// Now directly perform the API call - ensure usage parameter is included
		// for consistent cost tracking across all API requests
		let api_result = openrouter::chat_completion(
			chat_session.session.messages.clone(),
			&model,
			temperature,
			&config_clone
		).await;

		// Stop the animation - but use TRUE to stop it, not false!
		operation_cancelled.store(true, Ordering::SeqCst);
		let _ = animation_task.await;

		// Check for Ctrl+C again before processing response
		if ctrl_c_pressed.load(Ordering::SeqCst) {
			// Skip processing response if Ctrl+C was pressed
			continue;
		}

		// Process the response
		match api_result {
			Ok((content, exchange, tool_calls, finish_reason)) => {
				// Process the response, handling tool calls recursively
				// Create a fresh cancellation flag to avoid any "Operation cancelled" messages when not requested
				let tool_process_cancelled = Arc::new(AtomicBool::new(false));
				let process_result = process_response(
					content,
					exchange,
					tool_calls,
					finish_reason,
					&mut chat_session,
					&current_config,
					tool_process_cancelled.clone()
				).await;

				if let Err(e) = process_result {
					// Print colorful error message
					use colored::*;
					println!("\n{}: {}", "Error processing response".bright_red(), e);
				}
			},
			Err(e) => {
				// Print colorful error message
				use colored::*;
				println!("\n{}: {}", "Error calling OpenRouter".bright_red(), e);
				println!("{}", "Make sure OpenRouter API key is set in the config or as OPENROUTER_API_KEY environment variable.".yellow());
			}
		}
	}

	// Clean up the watcher when the session ends (only for developer role)
	if session_args.role == "developer" {
		let _ = crate::session::indexer::cleanup_watcher().await;
	}

	Ok(())
}
