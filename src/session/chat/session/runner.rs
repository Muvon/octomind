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
use super::super::context_truncation::{
	check_and_truncate_context_with_cancellation, TruncationOptions,
};
use super::super::input::{read_user_input, InputResult};
use super::super::response::{process_response, ResponseProcessingParams};
use super::super::CostTracker;
use super::core::{ChatSession, SessionInitParams};
use crate::config::Config;
use crate::session::cancellation::SessionCancellation;
use crate::session::{create_system_prompt, ChatCompletionWithValidationParams};
use crate::{log_debug, log_info};
use anyhow::Result;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::sync::watch;

// Type alias for extracted session parameters
type SessionParams = (
	Option<String>, // name
	Option<String>, // resume
	bool,           // resume_recent
	Option<String>, // model
	Option<u32>,    // max_tokens
	Option<f32>,    // temperature (None = use role config)
	String,         // role
	Option<u32>,    // max_retries (None = use role config)
);

// Extract session parameters from Debug format with proper fallbacks
fn extract_session_params<T: std::fmt::Debug>(args: &T, _config: &Config) -> SessionParams {
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

	// Get resume_recent
	let resume_recent = args_str.contains("resume_recent: true");

	// Get role
	let role = if args_str.contains("role: \"") {
		let start = args_str.find("role: \"").unwrap() + 7;
		let end = args_str[start..].find('"').unwrap() + start;
		args_str[start..end].to_string()
	} else {
		"developer".to_string() // Default role
	};

	// Get temperature - check if explicitly provided via CLI (now Optional)
	let temperature = if args_str.contains("temperature: Some(") {
		let start = args_str.find("temperature: Some(").unwrap() + 18;
		let end = args_str[start..].find(')').unwrap() + start;
		args_str[start..end].trim().parse::<f32>().ok()
	} else {
		None // No temperature specified, use role config
	};

	// Get max_tokens
	let max_tokens = if args_str.contains("max_tokens: Some(") {
		let start = args_str.find("max_tokens: Some(").unwrap() + 17;
		let end = args_str[start..].find(')').unwrap() + start;
		args_str[start..end].trim().parse::<u32>().ok()
	} else {
		None // No max_tokens specified
	};

	// Get max_retries - check if explicitly provided via CLI (now Optional)
	let max_retries = if args_str.contains("max_retries: Some(") {
		let start = args_str.find("max_retries: Some(").unwrap() + 18;
		let end = args_str[start..].find(')').unwrap() + start;
		args_str[start..end].trim().parse::<u32>().ok()
	} else {
		None // No max_retries specified, use role config
	};

	(
		name,
		resume,
		resume_recent,
		model,
		max_tokens,
		temperature,
		role,
		max_retries,
	)
}

// Helper function to print command output in CLI context
// Uses the strongly-typed CommandOutput display method
fn print_command_output(
	output: &super::commands::CommandOutput,
	session: &ChatSession,
	config: &Config,
) {
	output.display_cli(session, config);
}

// Helper function to setup session parameters and initialize chat session
pub async fn setup_and_initialize_session<T: std::fmt::Debug>(
	args: &T,
	config: &Config,
) -> Result<(ChatSession, Config, String, bool)> {
	use indicatif::{ProgressBar, ProgressStyle};
	use std::io::IsTerminal;
	use std::time::Duration;

	// Show loading spinner in interactive mode
	let spinner = if std::io::stdin().is_terminal() {
		let sp = ProgressBar::new_spinner();
		sp.set_style(
			ProgressStyle::default_spinner()
				.template(" {spinner:.cyan} {msg:.cyan}")
				.unwrap()
				.tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧"),
		);
		sp.set_message("Starting session...");
		sp.enable_steady_tick(Duration::from_millis(80));
		Some(sp)
	} else {
		None
	};

	// Extract session parameters
	let (name, resume, resume_recent, model, max_tokens, temperature, role, max_retries) =
		extract_session_params(args, config);

	// Get role config for defaults
	let (role_config, _, _, _, _) = config.get_role_config(&role);

	// Get current directory
	let current_dir = std::env::current_dir()?;

	// Get the merged configuration for the specified role
	let config_for_role = config.get_merged_config_for_role(&role);

	// Validate session token threshold if enabled (before initializing session)
	if config_for_role.max_session_tokens_threshold > 0 {
		if let Err(e) =
			crate::session::validate_session_token_threshold(&config_for_role, &role, &current_dir)
				.await
		{
			return Err(anyhow::anyhow!(
				"Session initialization failed: {}
To fix this issue
1. Increase max_session_tokens_threshold in your config
2. Or disable session continuation by setting max_session_tokens_threshold = 0
3. Or reduce the number of MCP servers to lower tool overhead",
				e
			));
		}
	}

	// Create or load session
	let mut session_params = SessionInitParams::new(&config_for_role, &role);

	if let Some(name) = name {
		session_params = session_params.with_name(name);
	}
	if let Some(resume) = resume {
		session_params = session_params.with_resume(resume);
	}
	if resume_recent {
		session_params = session_params.with_resume_recent(true);
	}
	if let Some(model) = model.clone() {
		session_params = session_params.with_model(model);
	}

	// Use CLI temperature if provided, otherwise use role config temperature
	let effective_temperature = temperature.unwrap_or(role_config.temperature);
	session_params = session_params.with_temperature(effective_temperature);

	// Use CLI max_tokens if provided, otherwise use config default
	let effective_max_tokens =
		max_tokens.unwrap_or_else(|| config_for_role.get_effective_max_tokens());
	session_params = session_params.with_max_tokens(effective_max_tokens);

	// Use CLI max_retries if provided, otherwise use root config max_retries
	let effective_max_retries = max_retries.unwrap_or(config_for_role.max_retries);
	session_params = session_params.with_max_retries(effective_max_retries);

	// Clean up spinner BEFORE initializing session (which prints messages)
	if let Some(sp) = spinner {
		sp.finish_and_clear();
		// Clear entire line and move cursor to beginning
		print!("\x1B[2K\r");
		std::io::Write::flush(&mut std::io::stdout()).ok();
	}

	let mut chat_session = ChatSession::initialize(session_params).await?;

	// Apply runtime overrides (these override the session initialization values)
	if let Some(runtime_model) = &model {
		chat_session.model = runtime_model.clone();
		log_info!("Using runtime model override: {}", runtime_model);
	}

	// Apply runtime temperature override if provided via CLI
	if let Some(runtime_temperature) = temperature {
		chat_session.temperature = runtime_temperature;
		log_info!(
			"Using runtime temperature override: {}",
			runtime_temperature
		);
	}

	// Apply runtime max_tokens override if provided via CLI
	if let Some(runtime_max_tokens) = max_tokens {
		chat_session.max_tokens = runtime_max_tokens;
		log_info!("Using runtime max_tokens override: {}", runtime_max_tokens);
	}

	// Apply runtime max_retries override if provided via CLI
	if let Some(runtime_max_retries) = max_retries {
		chat_session.max_retries = runtime_max_retries;
		log_info!(
			"Using runtime max_retries override: {}",
			runtime_max_retries
		);
	}

	// Track if the first message has been processed through layers
	let first_message_processed = !chat_session.session.messages.is_empty();

	Ok((chat_session, config_for_role, role, first_message_processed))
}

// Helper function to setup system prompt and cache
pub async fn setup_system_prompt_and_cache(
	chat_session: &mut ChatSession,
	config_for_role: &Config,
	role: &str,
	is_interactive: bool,
) -> Result<()> {
	let current_dir = std::env::current_dir()?;

	// Initialize with system prompt if new session
	if chat_session.session.messages.is_empty() {
		// Create system prompt based on role - use merged config for role
		let system_prompt = create_system_prompt(&current_dir, config_for_role, role).await;
		chat_session.add_system_message(&system_prompt)?;

		// Process layer system prompts during session initialization
		// This ensures layer system prompts are processed once and cached for the entire session
		let (_role_config, _, _, _, _) = config_for_role.get_role_config(role);

		// Check if role uses workflow
		if let Some(role_data) = config_for_role.role_map.get(role) {
			if role_data.workflow.is_some() {
				// Workflow system handles layer processing
				log_info!("Role uses workflow system - layer prompts will be processed during workflow execution");
			}
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

		if is_interactive {
			// Add initial messages (welcome + instructions) using centralized function
			let initial_messages =
				super::utils::get_initial_messages(config_for_role, role, &current_dir).await?;
			for msg in initial_messages {
				match msg.role.as_str() {
					"assistant" => {
						chat_session.add_assistant_message(
							&msg.content,
							None,
							config_for_role,
							role,
						)?;
					}
					"user" => {
						chat_session.add_user_message(&msg.content)?;
					}
					_ => {} // Should not happen
				}
			}

			// Apply cache markers to initial messages if caching is supported
			if supports_caching {
				let cache_manager = crate::session::cache::CacheManager::new();
				cache_manager.add_automatic_cache_markers(
					&mut chat_session.session.messages,
					has_tools,
					supports_caching,
				);
			}
		} else {
			// Non-interactive mode: Add assistant welcome message
			let role_config = config_for_role.get_role_config_struct(role);
			let welcome_message =
				crate::session::helper_functions::process_placeholders_async_with_role(
					&role_config.welcome,
					&current_dir,
					Some(role),
				)
				.await;

			chat_session.add_assistant_message(&welcome_message, None, config_for_role, role)?;

			// Apply cache marker to welcome message
			if supports_caching {
				let cache_manager = crate::session::cache::CacheManager::new();
				cache_manager.add_automatic_cache_markers(
					&mut chat_session.session.messages,
					has_tools,
					supports_caching,
				);
			}

			// Check for custom instructions file
			let instructions_filename = &config_for_role.custom_instructions_file_name;
			if !instructions_filename.is_empty() {
				let instructions_path = current_dir.join(instructions_filename);
				if instructions_path.exists() {
					match std::fs::read_to_string(&instructions_path) {
						Ok(instructions_content) => {
							let processed_instructions =
								crate::session::helper_functions::process_placeholders_async_with_role(
									&instructions_content,
									&current_dir,
									Some(role),
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
	}

	Ok(())
}

// Helper function to process layers if enabled
pub async fn process_layers_if_enabled(
	input: &str,
	chat_session: &mut ChatSession,
	config: &Config,
	role: &str,
	first_message_processed: bool,
	operation_rx: watch::Receiver<bool>,
) -> Result<(String, bool, bool)> {
	// Check if role uses workflow
	let has_workflow = config
		.role_map
		.get(role)
		.and_then(|r| r.workflow.as_ref())
		.is_some();

	if has_workflow && !first_message_processed {
		// Track session message count before workflow processing
		let messages_before_workflow = chat_session.session.messages.len();

		// Process using workflow architecture to get improved input
		let workflow_result = super::super::layered_response::process_layered_response(
			input,
			chat_session,
			config,
			role,
			operation_rx,
		)
		.await;

		match workflow_result {
			Ok(processed_input) => {
				// Check if workflow modified the session
				let messages_after_workflow = chat_session.session.messages.len();
				let workflow_modified_session = messages_after_workflow > messages_before_workflow;

				if workflow_modified_session {
					// Workflow used output_mode append/replace and added messages to session
					log_info!(
						"Workflow modified session ({} messages added).",
						messages_after_workflow - messages_before_workflow
					);
					// Return indication that workflow modified session
					Ok((processed_input, true, false))
				} else {
					// Workflow didn't modify session (all had output_mode = none)
					// Use the processed input from workflow instead of the original input
					log_info!("Workflow processing complete. Using enhanced input for main model.");
					Ok((processed_input, false, false))
				}
			}
			Err(e) => {
				// Check if this is a cancellation error - if so, propagate it to main loop
				let error_msg = e.to_string();
				if error_msg.contains("Operation cancelled")
					|| error_msg.contains("Request cancelled")
				{
					// This is a cancellation error - handle gracefully and continue session
					use colored::*;
					println!("{}", "\nOperation cancelled by user.".bright_yellow());
					println!("{}", "Continuing with original input.".yellow());

					// CRITICAL FIX: Clean up any partial workflow modifications to session
					// When workflow is cancelled, it might have partially modified the session
					// We need to restore the session to its state before workflow processing
					let messages_after_cancellation = chat_session.session.messages.len();
					if messages_after_cancellation > messages_before_workflow {
						// Remove messages added by workflow before cancellation
						let messages_to_remove =
							messages_after_cancellation - messages_before_workflow;
						for _ in 0..messages_to_remove {
							chat_session.session.messages.pop();
						}
						println!(
							"{}",
							format!(
								"Cleaned up {} messages added by cancelled layers",
								messages_to_remove
							)
							.yellow()
						);
					}

					// Return original input and continue session normally
					return Ok((input.to_string(), false, true));
				}

				// Regular layer processing error - print message and continue with original input
				use colored::*;
				println!(
					"\n{}: {}",
					"Error processing through layers".bright_red(),
					e
				);
				println!("{}", "Continuing with original input.".yellow());
				// Return original input
				Ok((input.to_string(), false, false))
			}
		}
	} else {
		// Layers not enabled or already processed
		Ok((input.to_string(), false, false))
	}
}

// Helper function to prepare for API call (context truncation and caching)
pub async fn prepare_for_api_call(
	chat_session: &mut ChatSession,
	config: &Config,
	operation_rx: tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
	// Check if we need to truncate the context to stay within token limits
	check_and_truncate_context_with_cancellation(
		chat_session,
		config,
		TruncationOptions::default(), // Normal truncation, no defer
		Some(Arc::new(std::sync::atomic::AtomicBool::new(
			*operation_rx.borrow(),
		))),
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
					"System message has been automatically marked for caching to save tokens."
				);
				// Save the session to ensure the cached status is persisted
				let _ = chat_session.save();
			}
		}
	}

	Ok(())
}

// Helper function to execute API call and process response
pub async fn execute_api_call_and_process_response(
	chat_session: &mut ChatSession,
	config: &Config,
	role: &str,
	operation_rx: watch::Receiver<bool>,
	is_interactive: bool,
) -> Result<()> {
	let model = chat_session.model.clone();
	let temperature = chat_session.temperature;
	let config_clone = config.clone();

	// Create animation task
	let animation_cancel = Arc::new(AtomicBool::new(false));
	let animation_cancel_clone = animation_cancel.clone();
	let current_cost = chat_session.session.info.total_cost;

	// Set up monitor task to propagate global cancellation to animation
	let animation_cancel_monitor = animation_cancel.clone();
	let operation_cancelled_monitor = operation_rx.clone();
	let operation_rx_for_response = operation_rx.clone();
	let _cancel_monitor = tokio::spawn(async move {
		while !animation_cancel_monitor.load(Ordering::SeqCst) {
			if *operation_cancelled_monitor.borrow() {
				animation_cancel_monitor.store(true, Ordering::SeqCst);
				break;
			}
			tokio::time::sleep(tokio::time::Duration::from_millis(1)).await;
		}
	});

	let animation_task = tokio::spawn(async move {
		if is_interactive {
			let _ = show_loading_animation(animation_cancel_clone, current_cost).await;
		} else {
			let _ = show_no_animation(animation_cancel_clone, current_cost).await;
		}
	});

	// Check spending threshold for interactive mode
	if is_interactive {
		match chat_session.check_spending_threshold(config) {
			Ok(should_continue) => {
				if !should_continue {
					// User chose not to continue due to spending threshold
					let _ = animation_task.await;
					return Ok(());
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

		// Check request spending threshold
		match chat_session.check_request_spending_threshold(config) {
			Ok(should_continue) => {
				if !should_continue {
					// Request spending threshold exceeded - stop execution
					let _ = animation_task.await;
					return Ok(());
				}
			}
			Err(e) => {
				// Error checking request threshold, log and continue
				use colored::*;
				println!(
					"{}: {}",
					"Warning: Error checking request spending threshold".bright_yellow(),
					e
				);
			}
		}
	}

	// Make API call
	let messages = chat_session.session.messages.clone();
	let max_retries = chat_session.max_retries;
	let validation_params = ChatCompletionWithValidationParams::new(
		&messages,
		&model,
		temperature,
		chat_session.top_p,
		chat_session.top_k,
		chat_session.max_tokens,
		&config_clone,
	)
	.with_max_retries(max_retries)
	.with_chat_session(chat_session)
	.with_cancellation_token(operation_rx);
	let api_result = crate::session::chat_completion_with_validation(validation_params).await;

	// Stop animation
	animation_cancel.store(true, Ordering::SeqCst);
	let _ = animation_task.await;

	// CRITICAL FIX: Check for cancellation after API call completion
	// This prevents the race condition where Ctrl+C is pressed after API completes
	// but before response processing begins
	if *operation_rx_for_response.borrow() {
		use colored::*;
		println!("{}", "\nOperation cancelled by user.".bright_yellow());
		return Ok(()); // Return gracefully to main loop instead of force exit
	}

	// Process response
	match api_result {
		Ok(response) => {
			// CRITICAL FIX: Track exchange cost immediately after successful API call
			// This ensures all API calls (with or without tool calls) have their costs tracked
			if let Err(e) =
				CostTracker::track_exchange_cost(chat_session, &response.exchange, config)
			{
				use colored::*;
				println!(
					"{}: Failed to track exchange cost: {}",
					"Warning".bright_yellow(),
					e
				);
			}

			// Display rate limit information if available
			display_rate_limit_info(&response.exchange);

			// Process the response with tool calls
			// CRITICAL FIX: Use operation_cancelled instead of creating a new token
			// This ensures Ctrl+C cancellation works properly during tool execution
			let process_result = process_response(
				ResponseProcessingParams::new(
					response.content,
					response.exchange,
					response.tool_calls,
					response.finish_reason,
					response.response_id,
					chat_session,
					config,
					role,
					operation_rx_for_response.clone(),
				)
				.with_thinking(response.thinking)
				.with_interactive(is_interactive),
			) // Pass through interactive mode and thinking
			.await;

			if let Err(e) = process_result {
				use colored::*;
				println!("\n{}: {}", "Error processing response".bright_red(), e);
			}
		}
		Err(e) => {
			return Err(e);
		}
	}

	Ok(())
}

// Helper function to display rate limit information from provider response
fn display_rate_limit_info(exchange: &crate::session::ProviderExchange) {
	if let Some(ref rate_limit_headers) = exchange.rate_limit_headers {
		let mut rate_limit_info = Vec::new();

		match exchange.provider.as_str() {
			"anthropic" => {
				// Anthropic rate limit format
				if let (Some(tokens_remaining), Some(tokens_limit)) = (
					rate_limit_headers.get("tokens_remaining"),
					rate_limit_headers.get("tokens_limit"),
				) {
					rate_limit_info.push(format!("Tokens: {}/{}", tokens_remaining, tokens_limit));
				}

				if let (Some(input_remaining), Some(input_limit)) = (
					rate_limit_headers.get("input_tokens_remaining"),
					rate_limit_headers.get("input_tokens_limit"),
				) {
					rate_limit_info
						.push(format!("Input tokens: {}/{}", input_remaining, input_limit));
				}

				if let (Some(output_remaining), Some(output_limit)) = (
					rate_limit_headers.get("output_tokens_remaining"),
					rate_limit_headers.get("output_tokens_limit"),
				) {
					rate_limit_info.push(format!(
						"Output tokens: {}/{}",
						output_remaining, output_limit
					));
				}

				if !rate_limit_info.is_empty() {
					crate::log_info!("📊 Anthropic rate limits: {}", rate_limit_info.join(" | "));
				}
			}
			"openai" => {
				// OpenAI rate limit format
				if let (Some(requests_remaining), Some(requests_limit)) = (
					rate_limit_headers.get("requests_remaining"),
					rate_limit_headers.get("requests_limit"),
				) {
					rate_limit_info.push(format!(
						"Requests: {}/{}",
						requests_remaining, requests_limit
					));
				}

				if let (Some(tokens_remaining), Some(tokens_limit)) = (
					rate_limit_headers.get("tokens_remaining"),
					rate_limit_headers.get("tokens_limit"),
				) {
					rate_limit_info.push(format!("Tokens: {}/{}", tokens_remaining, tokens_limit));
				}

				if let Some(request_reset) = rate_limit_headers.get("request_reset") {
					rate_limit_info.push(format!("Request reset: {}", request_reset));
				}

				if !rate_limit_info.is_empty() {
					crate::log_info!("📊 OpenAI rate limits: {}", rate_limit_info.join(" | "));
				}
			}
			_ => {
				// Generic rate limit display for other providers
				if !rate_limit_headers.is_empty() {
					let info: Vec<String> = rate_limit_headers
						.iter()
						.map(|(k, v)| format!("{}: {}", k, v))
						.collect();
					crate::log_info!("📊 {} rate limits: {}", exchange.provider, info.join(" | "));
				}
			}
		}
	}
}

// Helper function to format provider errors with better context
pub fn format_provider_error(provider_name: &str, error: &anyhow::Error) -> String {
	let error_str = error.to_string();

	// Check if this is a status code error (like "520 <unknown status code>")
	if error_str.contains("API error") && error_str.contains("<unknown status code>") {
		// Extract status code and provide better context
		if let Some(status_start) = error_str.find("error ") {
			if let Some(status_end) = error_str[status_start + 6..].find(' ') {
				let status_code = &error_str[status_start + 6..status_start + 6 + status_end];

				// Provide context for common status codes
				let context = match status_code {
					"520" => "Server overloaded - this usually indicates the provider is experiencing high traffic. Try again in a few moments.",
					"429" => "Rate limit exceeded - you're making requests too quickly. Wait a moment before trying again.",
					"503" => "Service temporarily unavailable - the provider's servers are temporarily down.",
					"502" | "504" => "Gateway error - temporary connectivity issue with the provider.",
					"500" => "Internal server error - temporary issue on the provider's side.",
					_ => "Server error - temporary issue with the provider.",
				};

				return format!("HTTP {} - {}", status_code, context);
			}
		}
	}

	// Check for other common error patterns and provide better context
	if error_str.contains("rate limit") || error_str.contains("Rate limit") {
		return "Rate limit exceeded - you're making requests too quickly. Wait a moment before trying again.".to_string();
	}

	if error_str.contains("timeout") || error_str.contains("Timeout") {
		return "Request timed out - the provider took too long to respond. Try again.".to_string();
	}

	if error_str.contains("API key")
		|| error_str.contains("authentication")
		|| error_str.contains("unauthorized")
	{
		return format!(
			"Authentication failed - check your {} API key configuration.",
			provider_name
		);
	}

	if error_str.contains("overloaded") || error_str.contains("capacity") {
		return "Provider is currently overloaded - try again in a few moments.".to_string();
	}

	// For other errors, return the original message but cleaned up
	error_str
}

// Helper function to handle API errors with provider-specific messages
fn handle_api_error(
	chat_session: &mut ChatSession,
	user_message_index: usize,
	model: &str,
	error: &anyhow::Error,
) {
	// Remove user message on API failure
	if user_message_index < chat_session.session.messages.len() {
		chat_session.session.messages.truncate(user_message_index);
		log_debug!("Removed user message due to API call failure");
	}

	// CRITICAL FIX: Reset continuation state if API error occurs during continuation
	// This prevents infinite retry loops when rate limits are hit during continuation
	if chat_session.continuation_pending {
		chat_session.continuation_pending = false;
		log_debug!("Continuation state reset due to API error - breaking continuation loop");
	}

	// Print error with provider context
	use colored::*;

	// Extract provider name from the model string
	let provider_name =
		if let Ok((provider, _)) = crate::providers::ProviderFactory::parse_model(model) {
			provider
		} else {
			"unknown provider".to_string()
		};

	// Format error message with better context
	let error_message = format_provider_error(&provider_name, error);
	println!(
		"\n{}: {}",
		format!("Error calling {}", provider_name).bright_red(),
		error_message
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
			println!(
				"{}",
				"Make sure AWS credentials are configured properly for Amazon Bedrock access."
					.yellow()
			);
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

// Run an interactive session
pub async fn run_interactive_session<T: std::fmt::Debug>(args: &T, config: &Config) -> Result<()> {
	// Setup and initialize session using helper function
	let (mut chat_session, config_for_role, role, mut first_message_processed) =
		setup_and_initialize_session(args, config).await?;

	// Get current directory for file operations
	let current_dir = std::env::current_dir()?;

	// Setup system prompt and cache using helper function (BEFORE showing interactive prompts)
	setup_system_prompt_and_cache(&mut chat_session, &config_for_role, &role, true).await?;

	println!("Interactive coding session started. Type your questions/requests.");
	println!("Type /help for available commands.");

	// Show history usage info for new sessions
	if chat_session.session.messages.len() <= 2 {
		// System + welcome messages
		use colored::*;
		println!(
			"{}",
			"💡 Tip: Use ↑/↓ arrows or Ctrl+R for command history search".bright_yellow()
		);
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
	// Enhanced processing state tracking for smart cancellation
	#[derive(Debug, Clone, PartialEq)]
	#[allow(dead_code)] // Some variants may not be used after refactoring
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
	let _processing_state_clone = processing_state.clone();

	// Smart operation tracking for surgical cleanup
	#[derive(Debug, Clone)]
	#[allow(dead_code)] // Some fields may not be used after refactoring
	struct OperationContext {
		user_message_index: Option<usize>,
		assistant_message_index: Option<usize>,
		operation_id: String,
		has_tool_calls: bool,
		completed_tool_ids: Vec<String>,
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
		// Set processing state to idle
		*processing_state.lock().unwrap() = ProcessingState::Idle;

		// SMART CANCELLATION: Handle cancellation with surgical cleanup
		if cancellation.is_cancelled() {
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

		// Set state to reading input
		*processing_state.lock().unwrap() = ProcessingState::ReadingInput;

		// Get a new operation token for this iteration
		let operation_rx = cancellation.new_operation();

		// No more legacy compatibility bridge needed - use watch receiver directly

		// Create a child cancellation token for this operation (commented out for now)
		// let operation_cancellation_token = cancellation_token.child_token();

		// CRITICAL FIX: Check if continuation is pending from previous iteration
		// If so, skip reading user input and process the injected summary request immediately
		// BUT FIRST: Check if operation was cancelled to prevent infinite loops
		let input_result = if chat_session.continuation_pending {
			// Safety check: If cancellation occurred, reset continuation state and read user input normally
			if cancellation.is_cancelled() {
				log_debug!("Cancellation detected during continuation - resetting continuation state and reading user input");
				chat_session.continuation_pending = false;
				read_user_input(chat_session.estimated_cost, &current_config, &role)?
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
						read_user_input(chat_session.estimated_cost, &current_config, &role)
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
			read_user_input(chat_session.estimated_cost, &current_config, &role)?
		};

		// Handle the input result with proper error recovery
		let input = match input_result {
			InputResult::Text(text) => text,
			InputResult::AddWithoutSending(text) => {
				// Ctrl+G pressed - add message to context without sending
				use colored::*;

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
				// Ctrl+D pressed - graceful exit
				println!("Ending session. Your conversation has been saved.");

				// Ensure session is saved
				if let Err(e) = chat_session.save() {
					eprintln!("Warning: Failed to save session: {}", e);
				}
				break;
			}
		};

		// Check if the input is an exit command
		if input == "/exit" || input == "/quit" {
			println!("Ending session. Your conversation has been saved.");
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
						use colored::*;
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
				super::commands::CommandResult::TreatAsUserInput => {
					// This input should be treated as user input, fall through to normal processing
				}
				super::commands::CommandResult::Exit => {
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
				super::commands::CommandResult::Handled => {
					// Command was handled successfully, continue with session
					continue;
				}
				super::commands::CommandResult::HandledWithOutput(json_output) => {
					// Command was handled with output
					// Print it for CLI using existing display functions
					print_command_output(&json_output, &chat_session, &current_config);
					continue;
				}
			}
		}

		// Check for cancellation before starting layered processing
		if cancellation.is_cancelled() {
			continue;
		}

		// SIMPLIFIED FLOW:
		// 1. Process through layers if needed (first message with layers enabled)
		// 2. Use the processed input for the main model chat

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

		let user_message_index = chat_session.session.messages.len();

		// UNIFIED STANDARD PROCESSING FLOW
		// The same code path is used whether the input is from layers or direct user input

		// NEW FLOW: Check for continuation BEFORE processing new user request
		// This is one of the two correct moments to trigger continuation:
		// 1) On new user request (HERE)
		// 2) After all tool results gathered, before sending to AI (in tool_result_processor)
		if !chat_session.continuation_pending {
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

		// Add user message for standard processing flow
		// CRITICAL FIX: Add user message unless continuation is pending or layers modified session
		// Logic:
		// - continuation_pending = true: Continuation message already added → Skip (avoid duplicates)
		// - workflow_modified_session = true: Layers added messages to session → Skip (avoid duplicates)
		// - workflow_modified_session = false: Layers didn't add messages → Add user message (needed for conversation)
		if !chat_session.continuation_pending && !workflow_modified_session {
			// Append constraints if configured
			let final_input_with_constraints =
				crate::session::chat::session::utils::append_constraints_if_exists(
					&final_input,
					&current_config.custom_constraints_file_name,
					&current_dir,
				);
			chat_session.add_user_message(&final_input_with_constraints)?;
		}

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

		// Prepare for API call using helper function
		prepare_for_api_call(&mut chat_session, &current_config, operation_rx.clone()).await?;

		// Set processing state to calling API
		*processing_state.lock().unwrap() = ProcessingState::CallingAPI;

		// The cancellation is already being monitored by the watch channel
		// No need for additional monitoring here

		// Check for Ctrl+C before making API call
		if cancellation.is_cancelled() {
			// Immediately stop and return to main loop
			continue;
		}

		// Execute API call and process response using helper function
		let user_message_index_for_error = user_message_index;
		let model_for_error = chat_session.model.clone();
		match execute_api_call_and_process_response(
			&mut chat_session,
			&current_config,
			&role,
			operation_rx.clone(),
			true, // is_interactive
		)
		.await
		{
			Ok(_) => {
				// Update processing state to completed when done
				*processing_state.lock().unwrap() = ProcessingState::CompletedWithResults;

				// CRITICAL FIX: Check for cancellation after API call and response processing
				// This ensures we return to input prompt gracefully instead of continuing
				if cancellation.is_cancelled() {
					log_debug!(
						"Operation cancelled after API call completion - returning to input prompt"
					);
					continue; // Return to main loop for next user input
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

				// NOTE: Continuation check moved to AFTER potential summary response
				// If continuation was triggered during tool processing, the main loop will
				// make another API call to get the AI's summary, and THEN handle continuation
			}
			Err(e) => {
				// Handle API error using helper function
				handle_api_error(
					&mut chat_session,
					user_message_index_for_error,
					&model_for_error,
					&e,
				);
			}
		}

		// Clear operation context at the end of each successful iteration
		*current_operation.lock().unwrap() = None;

		// Clean up the cancellation sync task
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
		use colored::*;

		// Handle special /done command separately
		if input.trim() == "/done" {
			// Disable continuation triggers during /done processing
			chat_session.disable_continuation();

			// Clear plan data
			if let Err(e) = crate::mcp::dev::plan::clear_plan_data().await {
				crate::log_debug!("Failed to clear plan data: {}", e);
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
				json_output,
			) => {
				// Command was handled with output
				// Print it for CLI run command using existing display functions
				print_command_output(&json_output, &chat_session, &current_config);
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
		log_info!("Workflow processing complete. Using enhanced input for main model.");
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
		let input_with_constraints =
			crate::session::chat::session::utils::append_constraints_if_exists(
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
	match execute_api_call_and_process_response(
		&mut chat_session,
		&current_config,
		&role,
		operation_rx_clone,
		false, // is_interactive = false
	)
	.await
	{
		Ok(_) => {
			// Success - session will be saved below
		}
		Err(e) => {
			// Handle API error using helper function
			handle_api_error(
				&mut chat_session,
				user_message_index_for_error,
				&model_for_error,
				&e,
			);
		}
	}

	// Save session before exit
	let _ = chat_session.save();
	Ok(())
}
