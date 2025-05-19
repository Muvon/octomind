// Chat session implementation

use crate::config::Config;
use crate::store::Store;
use super::{Session, get_sessions_dir, load_session, create_system_prompt, openrouter, mcp};
use crossterm::{cursor, execute};
use std::io::{self, Write, stdout};
use std::fs::File;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use anyhow::Result;
use ctrlc;
use rustyline::error::ReadlineError;
use rustyline::{Editor, Config as RustylineConfig, CompletionType, EditMode};
use colored::Colorize;

// Model choices (moved to config, kept here for backward compatibility)
pub const CLAUDE_MODEL: &str = "anthropic/claude-3.7-sonnet";
pub const DEFAULT_MODEL: &str = CLAUDE_MODEL;

// Chat commands
const HELP_COMMAND: &str = "/help";
const EXIT_COMMAND: &str = "/exit";
const QUIT_COMMAND: &str = "/quit";
const COPY_COMMAND: &str = "/copy";
const CLEAR_COMMAND: &str = "/clear";
const SAVE_COMMAND: &str = "/save";
const CACHE_COMMAND: &str = "/cache";

// List of all available commands for autocomplete
pub const COMMANDS: [&str; 7] = [
	HELP_COMMAND,
	EXIT_COMMAND,
	QUIT_COMMAND,
	COPY_COMMAND,
	CLEAR_COMMAND,
	SAVE_COMMAND,
	CACHE_COMMAND,
];

// Chat session manager for interactive coding sessions
pub struct ChatSession {
	pub session: Session,
	pub last_response: String,
	pub model: String,
	pub temperature: f32,
	pub estimated_cost: f64,
}

impl ChatSession {
	// Create a new chat session
	pub fn new(name: String, model: Option<String>, config: &Config) -> Self {
		let model_name = model.unwrap_or_else(|| config.openrouter.model.clone());

		Self {
			session: Session::new(name, model_name.clone(), "openrouter".to_string()),
			last_response: String::new(),
			model: model_name,
			temperature: 0.7, // Default temperature
			estimated_cost: 0.0, // Initialize estimated cost as zero
		}
	}

	// Initialize a new chat session or load existing one
	pub fn initialize(name: Option<String>, resume: Option<String>, model: Option<String>, config: &Config) -> Result<Self> {
		let sessions_dir = get_sessions_dir()?;

		// Determine session name
		let session_name = if let Some(name_arg) = &name {
			name_arg.clone()
		} else if let Some(resume_name) = &resume {
			resume_name.clone()
		} else {
			// Generate a name based on timestamp
			let timestamp = std::time::SystemTime::now()
				.duration_since(std::time::UNIX_EPOCH)
				.unwrap_or_default()
				.as_secs();
			format!("session_{}", timestamp)
		};

		let session_file = sessions_dir.join(format!("{}.jsonl", session_name));

		// Load or create session
		if (resume.is_some() || (name.is_some() && session_file.exists())) && session_file.exists() {
			println!("Resuming session: {}", session_name);
			let session = load_session(&session_file)?;

			// Create chat session from loaded session
			let mut chat_session = ChatSession {
				session,
				last_response: String::new(),
				model: model.unwrap_or_else(|| config.openrouter.model.clone()),
				temperature: 0.7,
				estimated_cost: 0.0,
			};

			// Get last assistant response if any
			for msg in chat_session.session.messages.iter().rev() {
				if msg.role == "assistant" {
					chat_session.last_response = msg.content.clone();
					break;
				}
			}

			Ok(chat_session)
		} else {
			// Create new session
			println!("Starting new session: {}", session_name);

			// Create session file if it doesn't exist
			if !session_file.exists() {
				let file = File::create(&session_file)?;
				drop(file);
			}

			let mut chat_session = ChatSession::new(session_name, model, config);
			chat_session.session.session_file = Some(session_file);

			Ok(chat_session)
		}
	}

	// Save the session
	pub fn save(&self) -> Result<()> {
		self.session.save()
	}

	// Add a system message
	pub fn add_system_message(&mut self, content: &str) -> Result<()> {
		// Add message to session
		self.session.add_message("system", content);

		// Save to session file
		if let Some(session_file) = &self.session.session_file {
			let message_json = serde_json::to_string(&self.session.messages.last().unwrap())?;
			super::append_to_session_file(session_file, &format!("SYSTEM: {}", message_json))?;
		}

		Ok(())
	}

	// Add a user message
	pub fn add_user_message(&mut self, content: &str) -> Result<()> {
		// Add message to session
		self.session.add_message("user", content);

		// Save to session file
		if let Some(session_file) = &self.session.session_file {
			let message_json = serde_json::to_string(&self.session.messages.last().unwrap())?;
			super::append_to_session_file(session_file, &format!("USER: {}", message_json))?;
		}

		Ok(())
	}

	// Add an assistant message
	pub fn add_assistant_message(&mut self, content: &str, exchange: Option<openrouter::OpenRouterExchange>, config: &Config) -> Result<()> {
		// Add message to session
		let message = self.session.add_message("assistant", content);
		self.last_response = content.to_string();

		// Update token counts and estimated costs if we have usage data
		if let Some(ex) = &exchange {
			if let Some(usage) = &ex.usage {
				// Update session token counts
				self.session.info.input_tokens += usage.prompt_tokens;
				self.session.info.output_tokens += usage.completion_tokens;

				// If OpenRouter provided cost data, use it (preferred)
				if let Some(cost_credits) = usage.cost {
					// Convert from credits to dollars (100,000 credits = $1)
					let cost_dollars = cost_credits as f64 / 100000.0;

					// Update total cost
					self.session.info.total_cost += cost_dollars;
					self.estimated_cost = self.session.info.total_cost;
				} else {
					// Fallback to configured pricing if OpenRouter didn't provide cost
					let input_cost = usage.prompt_tokens as f64 * config.openrouter.pricing.input_price;
					let output_cost = usage.completion_tokens as f64 * config.openrouter.pricing.output_price;
					let current_cost = input_cost + output_cost;

					// Update total cost
					self.session.info.total_cost += current_cost;
					self.estimated_cost = self.session.info.total_cost;
				}

				// Update session duration
				let current_time = std::time::SystemTime::now()
					.duration_since(std::time::UNIX_EPOCH)
					.unwrap_or_default()
					.as_secs();
				let start_time = self.session.info.created_at;
				self.session.info.duration_seconds = current_time - start_time;
			}
		}

		// Save to session file
		if let Some(session_file) = &self.session.session_file {
			let message_json = serde_json::to_string(&message)?;
			super::append_to_session_file(session_file, &format!("ASSISTANT: {}", message_json))?;

			// If we have a raw exchange, save it as well
			if let Some(ex) = exchange {
				let exchange_json = serde_json::to_string(&ex)?;
				super::append_to_session_file(session_file, &format!("EXCHANGE: {}", exchange_json))?;
			}
		}

		Ok(())
	}

	// Process user commands
	pub fn process_command(&mut self, input: &str) -> Result<bool> {
		use colored::*;

		match input.trim() {
			EXIT_COMMAND | QUIT_COMMAND => {
				println!("{}", "Ending session. Your conversation has been saved.".bright_green());
				return Ok(true);
			},
			HELP_COMMAND => {
				println!("{}", "\nAvailable commands:\n".bright_cyan());
				println!("{} or {} - {}", EXIT_COMMAND.cyan(), QUIT_COMMAND.cyan(), "Exit the session");
				println!("{} - {}", COPY_COMMAND.cyan(), "Copy last response to clipboard");
				println!("{} - {}", CLEAR_COMMAND.cyan(), "Clear the screen");
				println!("{} - {}", SAVE_COMMAND.cyan(), "Save the session");
				println!("{} - {}", CACHE_COMMAND.cyan(), "Mark a cache checkpoint at the last user message to save on tokens with supported models");
				println!("{} - {}\n", HELP_COMMAND.cyan(), "Show this help message");

				// Additional info about caching
				println!("{}", "** About Cache Checkpoints **".bright_yellow());
				println!("{}", "When using /cache, your last user message will be marked for caching.");
				println!("{}", "This is useful for large text blocks like code snippets that don't change between requests.");
				println!("{}", "The model provider will charge less for cached content in subsequent requests.");
				println!("{}", "Best practice: Use separate messages with the most data-heavy part marked for caching.\n");
			},
			COPY_COMMAND => {
				println!("Clipboard functionality is disabled in this version.");
			},
			CLEAR_COMMAND => {
				// ANSI escape code to clear screen and move cursor to top-left
				print!("\x1B[2J\x1B[1;1H");
				io::stdout().flush()?;
			},
			SAVE_COMMAND => {
				if let Err(e) = self.save() {
					println!("{}: {}", "Failed to save session".bright_red(), e);
				} else {
					println!("{}", "Session saved successfully.".bright_green());
				}
			},
			CACHE_COMMAND => {
				match self.session.add_cache_checkpoint() {
					Ok(true) => {
						println!("{}", "Cache checkpoint added at the last user message. This will be used for future requests.".bright_green());
						println!("{}", "Note: For large text blocks, it's best to split them into separate messages with the cached part containing most of the data.".bright_yellow());
						// Save the session with the cached message
						let _ = self.save();
					},
					Ok(false) => {
						println!("{}", "No user messages found to mark as a cache checkpoint.".bright_yellow());
					},
					Err(e) => {
						println!("{}: {}", "Failed to add cache checkpoint".bright_red(), e);
					}
				}
			},
			_ => return Ok(false), // Not a command
		}

		Ok(false) // Continue session
	}
}

// Animation frames for loading indicator
const LOADING_FRAMES: [&str; 8] = [
	"⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧",
];

// Read user input with support for multiline input and command completion
pub fn read_user_input(estimated_cost: f64) -> Result<String> {
	// Configure rustyline
	let config = RustylineConfig::builder()
		.completion_type(CompletionType::List)
		.edit_mode(EditMode::Emacs)
		.auto_add_history(true) // Automatically add lines to history
		.bell_style(rustyline::config::BellStyle::None) // No bell
		.build();

	// Create editor with our custom helper
	let mut editor = Editor::with_config(config)?;

	// Add command completion
	use crate::session::chat_helper::CommandHelper;
	editor.set_helper(Some(CommandHelper::new()));

	// Set prompt with colors if terminal supports them and include cost estimation
	let prompt = if estimated_cost > 0.0 {
		format!("[~${:.2}] > ", estimated_cost).bright_blue().to_string()
	} else {
		"> ".bright_blue().to_string()
	};

	// Read line with command completion
	match editor.readline(&prompt) {
		Ok(line) => {
			// Add to history
			let _ = editor.add_history_entry(line.clone());
			Ok(line)
		},
		Err(ReadlineError::Interrupted) => {
			// Ctrl+C
			println!("\nCancelled");
			Ok(String::new())
		},
		Err(ReadlineError::Eof) => {
			// Ctrl+D
			println!("\nExiting session.");
			Ok("/exit".to_string())
		},
		Err(err) => {
			println!("Error: {:?}", err);
			Ok(String::new())
		}
	}
}

// Show loading animation while waiting for response
async fn show_loading_animation(cancel_flag: Arc<AtomicBool>) -> Result<()> {
	use colored::*;

	let mut stdout = stdout();
	let mut frame_idx = 0;

	// Save cursor position
	execute!(stdout, cursor::SavePosition)?;

	while !cancel_flag.load(Ordering::SeqCst) {
		// Display frame with color if supported
		execute!(stdout, cursor::RestorePosition)?;

		print!(" {} {}",
			LOADING_FRAMES[frame_idx].cyan(),
			"Generating response...".bright_blue());

		stdout.flush()?;

		// Update frame index
		frame_idx = (frame_idx + 1) % LOADING_FRAMES.len();

		// Delay
		tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
	}

	// Clear loading message
	execute!(stdout, cursor::RestorePosition)?;
	print!("                             "); // Clear loading message
	execute!(stdout, cursor::RestorePosition)?;
	stdout.flush()?;

	Ok(())
}

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

		SessionArgs {
			name,
			resume,
			model,
		}
	};

	// Ensure there's an index
	let current_dir = std::env::current_dir()?;
	let octodev_dir = current_dir.join(".octodev");
	let index_path = octodev_dir.join("storage");
	if !index_path.exists() {
		println!("No index found. Indexing current directory first...");
		crate::indexer::index_files(store, crate::state::create_shared_state(), config).await?;
	}

	// Create or load session
	let mut chat_session = ChatSession::initialize(
		session_args.name,
		session_args.resume,
		session_args.model,
		config
	)?;

	// Start the interactive session
	println!("Interactive coding session started. Type your questions/requests.");
	println!("Type /help for available commands.");

	// Initialize with system prompt if new session
	if chat_session.session.messages.is_empty() {
		// Create system prompt
		let system_prompt = create_system_prompt(&current_dir, config).await;
		chat_session.add_system_message(&system_prompt)?;

		// Add assistant welcome message
		let welcome_message = format!(
			"Hello! I'm ready to help you with your code in `{}`. What would you like to do?",
			current_dir.file_name().unwrap_or_default().to_string_lossy()
		);
		chat_session.add_assistant_message(&welcome_message, None, config)?;

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
			} else if msg.role == "user" {
				println!("> {}", msg.content.bright_blue());
			}
		}
	}

	// Set up a shared cancellation flag that can be set by Ctrl+C
	let cancel_flag = Arc::new(AtomicBool::new(false));
	let cancel_flag_clone = cancel_flag.clone();

	// Set up Ctrl+C handler
	ctrlc::set_handler(move || {
		cancel_flag_clone.store(true, Ordering::SeqCst);
	}).expect("Error setting Ctrl+C handler");

	// Main interaction loop
	loop {
		// Reset the cancel flag before each interaction
		cancel_flag.store(false, Ordering::SeqCst);

		// Read user input with command completion and cost estimation
		let input = read_user_input(chat_session.estimated_cost)?;

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
			let exit = chat_session.process_command(&input)?;
			if exit {
				break;
			}
			continue;
		}

		// Add user message
		chat_session.add_user_message(&input)?;

		// Convert messages to OpenRouter format
		let or_messages = openrouter::convert_messages(&chat_session.session.messages);

		// Call OpenRouter in a separate task
		let model = chat_session.model.clone();
		let temperature = chat_session.temperature;
		let config_clone = config.clone();
		let api_task = tokio::spawn(async move {
			openrouter::chat_completion(or_messages, &model, temperature, &config_clone).await
		});

		// Create a task to show loading animation
		let animation_cancel_flag = cancel_flag.clone();
		let animation_task = tokio::spawn(async move {
			let _ = show_loading_animation(animation_cancel_flag).await;
		});

		// Poll for completion or cancellation
		let mut response = None;
		let mut was_cancelled = false;

		tokio::select! {
			result = api_task => {
				response = Some(result);
			},
				_ = async {
					while !cancel_flag.load(Ordering::SeqCst) {
						tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
					}
				} => {
				was_cancelled = true;
			}
		}

		// Stop the animation
		cancel_flag.store(true, Ordering::SeqCst);
		let _ = animation_task.await;

		// Handle cancellation or response
		if was_cancelled {
			println!("\nRequest cancelled by user.");
			continue;
		}

		// Process the response
		match response.unwrap() {
			Ok(Ok((content, exchange))) => {
				// Check for tool calls if MCP is enabled
				if config.mcp.enabled {
					let tool_calls = mcp::parse_tool_calls(&content);

					if !tool_calls.is_empty() {
						// Execute all tool calls in parallel
						let mut tool_tasks = Vec::new();

						for tool_call in tool_calls.clone() {
							// Print colorful tool execution message
							use colored::*;
							println!("  - Executing: {}", tool_call.tool_name.yellow());

							// Execute in a tokio task
							let config_clone = config.clone();
							let task = tokio::spawn(async move {
								mcp::execute_tool_call(&tool_call, &config_clone).await
							});

							tool_tasks.push(task);
						}

						// Collect all results
						let mut tool_results = Vec::new();
						for task in tool_tasks {
							match task.await {
								Ok(result) => match result {
									Ok(res) => tool_results.push(res),
									Err(e) => {
										// Print colorful error message
										use colored::*;
										println!("  - {}: {}", "Error executing tool".bright_red(), e);
									},
								},
								Err(e) => {
									// Print colorful task error message
									use colored::*;
									println!("  - {}: {}", "Task error".bright_red(), e);
								},
							}
						}

						// Add assistant message with the initial response
						chat_session.add_assistant_message(&content, Some(exchange.clone()), config)?;

						// Display results
						if !tool_results.is_empty() {
							let formatted = mcp::format_tool_results(&tool_results);
							println!("{}", formatted);

							// Create user message with tool results
							let tool_results_message = serde_json::to_string(&tool_results)
								.unwrap_or_else(|_| "[]".to_string());

							let tool_message = format!("<function_results>\n{}\n</function_results>",
								tool_results_message);

							chat_session.add_user_message(&tool_message)?;

							// Call the AI again with the tool results
							let or_messages = openrouter::convert_messages(&chat_session.session.messages);

							// Set cancel flag to false for the new request
							cancel_flag.store(false, Ordering::SeqCst);

							// Call OpenRouter in a separate task
							let model = chat_session.model.clone();
							let temperature = chat_session.temperature;
							let config_clone = config.clone();
							let api_task = tokio::spawn(async move {
								openrouter::chat_completion(or_messages, &model, temperature, &config_clone).await
							});

							// Create a task to show loading animation
							let animation_cancel_flag = cancel_flag.clone();
							let animation_task = tokio::spawn(async move {
								let _ = show_loading_animation(animation_cancel_flag).await;
							});

							// Poll for completion or cancellation
							let mut final_response = None;
							let mut was_cancelled = false;

							tokio::select! {
								result = api_task => {
									final_response = Some(result);
								},
									_ = async {
										while !cancel_flag.load(Ordering::SeqCst) {
											tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
										}
									} => {
									was_cancelled = true;
								}
							}

							// Stop the animation
							cancel_flag.store(true, Ordering::SeqCst);
							let _ = animation_task.await;

							if was_cancelled {
								println!("\nRequest cancelled by user.");
								continue;
							}

							// Process the final response
							match final_response.unwrap() {
								Ok(Ok((final_content, final_exchange))) => {
									// Add assistant message with the final response
									chat_session.add_assistant_message(&final_content, Some(final_exchange), config)?;

									// Print assistant response with color if terminal supports it
									use colored::*;
									println!("\n{}", final_content.bright_green());

									// Display cumulative token usage
									println!();
									println!("── {} ────────────────────────────────────────", "session usage".bright_cyan());
									println!("{} {} prompt, {} completion, {} total, ${:.5}",
										"tokens:".bright_blue(),
										chat_session.session.info.input_tokens,
										chat_session.session.info.output_tokens,
										chat_session.session.info.input_tokens + chat_session.session.info.output_tokens,
										chat_session.session.info.total_cost);
									println!();
								},
								Ok(Err(e)) => {
									// Print colorful error message
									use colored::*;
									println!("\n{}: {}", "Error calling OpenRouter".bright_red(), e);
								},
								Err(e) => {
									// Print colorful task error message
									use colored::*;
									println!("\n{}: {}", "Task error".bright_red(), e);
								}
							}
						}
					} else {
						// No tool calls, just regular content
						chat_session.add_assistant_message(&content, Some(exchange), config)?;

						// Print assistant response with color if terminal supports it
						use colored::*;
						println!("\n{}", content.bright_green());

						// Display cumulative token usage
						println!();
						println!("── {} ────────────────────────────────────────", "session usage".bright_cyan());
						println!("{} {} prompt, {} completion, {} total, ${:.5}",
							"tokens:".bright_blue(),
							chat_session.session.info.input_tokens,
							chat_session.session.info.output_tokens,
							chat_session.session.info.input_tokens + chat_session.session.info.output_tokens,
							chat_session.session.info.total_cost);
						println!();
					}
				} else {
					// MCP not enabled, just show content
					chat_session.add_assistant_message(&content, Some(exchange), config)?;

					// Print assistant response with color if terminal supports it
					use colored::*;
					println!("\n{}", content.bright_green());

					// Display cumulative token usage
					println!();
					println!("── {} ────────────────────────────────────────", "session usage".bright_cyan());
					println!("{} {} prompt, {} completion, {} total, ${:.5}",
						"tokens:".bright_blue(),
						chat_session.session.info.input_tokens,
						chat_session.session.info.output_tokens,
						chat_session.session.info.input_tokens + chat_session.session.info.output_tokens,
						chat_session.session.info.total_cost);
					println!();
				}
			},
			Ok(Err(e)) => {
				// Print colorful error message
				use colored::*;
				println!("\n{}: {}", "Error calling OpenRouter".bright_red(), e);
				println!("{}", "Make sure OpenRouter API key is set in the config or as OPENROUTER_API_KEY environment variable.".yellow());
			},
			Err(e) => {
				// Print colorful task error message
				use colored::*;
				println!("\n{}: {}", "Task error".bright_red(), e);
			}
		}
	}

	Ok(())
}
