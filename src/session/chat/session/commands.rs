// Session command processing

use super::core::ChatSession;
use crate::config::Config;
use std::io::{self, Write};
use anyhow::Result;
use colored::Colorize;
use crate::session::list_available_sessions;
use chrono::{DateTime, Utc};
use super::super::commands::*;

impl ChatSession {
	// Process user commands
	pub fn process_command(&mut self, input: &str, config: &Config) -> Result<bool> {
		// Extract command and potential parameters
		let input_parts: Vec<&str> = input.trim().split_whitespace().collect();
		let command = input_parts[0];
		let params = if input_parts.len() > 1 {
			&input_parts[1..]
		} else {
			&[]
		};

		match command {
			EXIT_COMMAND | QUIT_COMMAND => {
				println!("{}", "Ending session. Your conversation has been saved.".bright_green());
				return Ok(true);
			},
			HELP_COMMAND => {
				println!("{}", "\nAvailable commands:\n".bright_cyan());
				println!("{} - {}", HELP_COMMAND.cyan(), "Show this help message");
				println!("{} - {}", COPY_COMMAND.cyan(), "Copy last response to clipboard");
				println!("{} - {}", CLEAR_COMMAND.cyan(), "Clear the screen");
				println!("{} - {}", SAVE_COMMAND.cyan(), "Save the session");
				println!("{} - {}", CACHE_COMMAND.cyan(), "Mark a cache checkpoint at the last user message");
				println!("{} - {}", LIST_COMMAND.cyan(), "List all available sessions");
				println!("{} [name] - {}", SESSION_COMMAND.cyan(), "Switch to another session or create a new one (without name creates fresh session)");
				println!("{} - {}", INFO_COMMAND.cyan(), "Display detailed token and cost breakdown for this session");
				println!("{} - {}", LAYERS_COMMAND.cyan(), "Toggle layered processing architecture on/off");
				println!("{} - {}", DONE_COMMAND.cyan(), "Optimize the session context, restart layered processing for next message, and apply EditorConfig formatting");
				println!("{} - {}", DEBUG_COMMAND.cyan(), "Toggle debug mode for detailed logs");
				println!("{} [threshold] - {}", TRUNCATE_COMMAND.cyan(), "Toggle automatic context truncation when token limit is reached");
				println!("{} or {} - {}\n", EXIT_COMMAND.cyan(), QUIT_COMMAND.cyan(), "Exit the session");

				// Additional info about caching
				println!("{}", "** About Cache Checkpoints **".bright_yellow());
				println!("{}", "The system message with function definitions is automatically cached for all sessions.");
				println!("{}", "Use '/cache' to mark your last user message for caching.");
				println!("{}", "This is useful for large text blocks like code snippets that don't change between requests.");
				println!("{}", "The model provider will charge less for cached content in subsequent requests.");
				println!("{}", "Cached tokens will be displayed in the usage statistics after your next message.");
				println!("{}", "Best practice: Use separate messages with the most data-heavy part marked for caching.");
				println!("{}", "Automatic caching: When non-cached tokens reach a configured threshold,");
				println!("{}", "    a cache checkpoint will be automatically placed (configurable via config.toml).\n");

				// Add information about layered architecture
				println!("{}", "** About Layered Processing **".bright_yellow());
				println!("{}", "The layered architecture processes your initial query through multiple AI layers:");
				println!("{}", "1. Query Processor: Improves your initial query");
				println!("{}", "2. Context Generator: Gathers relevant context information");
				println!("{}", "3. Developer: Executes the actual development work");
				println!("{}", "The Reducer functionality is available through the /done command.");
				println!("{}", "Only the first message in a session uses the full layered architecture.");
				println!("{}", "Subsequent messages use direct communication with the developer model.");
				println!("{}", "Use the /done command to optimize context, apply EditorConfig formatting to edited files, and restart the layered pipeline.");
				println!("{}", "Toggle layered processing with /layers command.\n");
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
			INFO_COMMAND => {
				self.display_session_info();
			},
			LAYERS_COMMAND => {
				// Toggle layered processing
				// First, load the config from disk to ensure we have the latest values
				let mut loaded_config = match crate::config::Config::load() {
					Ok(cfg) => cfg,
					Err(_) => {
						println!("{}", "Error loading configuration file. Using current settings instead.".bright_red());
						config.clone()
					}
				};

				// Toggle the setting
				loaded_config.openrouter.enable_layers = !loaded_config.openrouter.enable_layers;

				// Save the updated config
				if let Err(e) = loaded_config.save() {
					println!("{}: {}", "Failed to save configuration".bright_red(), e);
					return Ok(false);
				}

				// Show the new state
				if loaded_config.openrouter.enable_layers {
					println!("{}", "Layered processing architecture is now ENABLED.".bright_green());
					println!("{}", "Your queries will now be processed through multiple AI models.".bright_yellow());
				} else {
					println!("{}", "Layered processing architecture is now DISABLED.".bright_yellow());
					// println!("{}", "Using standard single-model processing with Claude.".bright_blue());
				}
				println!("{}", "Configuration has been saved to disk.");

				// Return a special code that indicates we should reload the config in the main loop
				// This will ensure all future commands use the updated config
				return Ok(true);
			},
			DEBUG_COMMAND => {
				// Toggle debug mode
				// First, load the config from disk to ensure we have the latest values
				let mut loaded_config = match crate::config::Config::load() {
					Ok(cfg) => cfg,
					Err(_) => {
						println!("{}", "Error loading configuration file. Using current settings instead.".bright_red());
						config.clone()
					}
				};

				// Toggle the setting
				loaded_config.openrouter.debug = !loaded_config.openrouter.debug;

				// Save the updated config
				if let Err(e) = loaded_config.save() {
					println!("{}: {}", "Failed to save configuration".bright_red(), e);
					return Ok(false);
				}

				// Show the new state
				if loaded_config.openrouter.debug {
					println!("{}", "Debug mode is now ENABLED.".bright_green());
					println!("{}", "Detailed logging will be shown for API calls and tool executions.".bright_yellow());
				} else {
					println!("{}", "Debug mode is now DISABLED.".bright_yellow());
					println!("{}", "Only essential information will be displayed.".bright_blue());
				}
				println!("{}", "Configuration has been saved to disk.");

				// Return a special code that indicates we should reload the config in the main loop
				return Ok(true);
			},
			TRUNCATE_COMMAND => {
				// Toggle auto-truncation mode
				// First, load the config from disk to ensure we have the latest values
				let mut loaded_config = match crate::config::Config::load() {
					Ok(cfg) => cfg,
					Err(_) => {
						println!("{}", "Error loading configuration file. Using current settings instead.".bright_red());
						config.clone()
					}
				};

				// Toggle the setting
				loaded_config.openrouter.enable_auto_truncation = !loaded_config.openrouter.enable_auto_truncation;

				// Update token thresholds if parameters were provided
				if params.len() >= 1 {
					if let Ok(threshold) = params[0].parse::<usize>() {
						loaded_config.openrouter.max_request_tokens_threshold = threshold;
						println!("{}", format!("Max request token threshold set to {} tokens", threshold).bright_green());
					}
				}

				// Save the updated config
				if let Err(e) = loaded_config.save() {
					println!("{}: {}", "Failed to save configuration".bright_red(), e);
					return Ok(false);
				}

				// Show the new state
				if loaded_config.openrouter.enable_auto_truncation {
					println!("{}", "Auto-truncation is now ENABLED.".bright_green());
					println!("{}", format!("Context will be automatically truncated when exceeding {} tokens.",
						loaded_config.openrouter.max_request_tokens_threshold).bright_yellow());
				} else {
					println!("{}", "Auto-truncation is now DISABLED.".bright_yellow());
					println!("{}", "You'll need to manually reduce context when it gets too large.".bright_blue());
				}
				println!("{}", "Configuration has been saved to disk.");

				// Return a special code that indicates we should reload the config in the main loop
				return Ok(true);
			},
			CACHE_COMMAND => {
				match self.session.add_cache_checkpoint(false) {
					Ok(true) => {
						println!("{}", "Cache checkpoint added at the last user message. This will be used for future requests.".bright_green());
						println!("{}", "Note: For large text blocks, it's best to split them into separate messages with the cached part containing most of the data.".bright_yellow());
						println!("{}", "You'll see the cached token count in the usage summary for your next message.".bright_blue());
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
			LIST_COMMAND => {
				match list_available_sessions() {
					Ok(sessions) => {
						if sessions.is_empty() {
							println!("{}", "No sessions found.".bright_yellow());
						} else {
							println!("{}", "\nAvailable sessions:\n".bright_cyan());
							println!("{:<20} {:<25} {:<15} {:<10} {:<10}",
								"Name".cyan(),
								"Created".cyan(),
								"Model".cyan(),
								"Tokens".cyan(),
								"Cost".cyan());

							println!("{}", "─".repeat(80).cyan());

							for (name, info) in sessions {
								// Format date from timestamp
								let created_time = DateTime::<Utc>::from_timestamp(info.created_at as i64, 0)
									.map(|dt| dt.naive_local().format("%Y-%m-%d %H:%M:%S").to_string())
									.unwrap_or_else(|| "Unknown".to_string());

								// Determine if this is the current session
								let is_current = match &self.session.session_file {
									Some(path) => path.file_stem().and_then(|s| s.to_str()).unwrap_or("") == name,
									None => false,
								};

								let name_display = if is_current {
									format!("{} (current)", name).bright_green()
								} else {
									name.white()
								};

								// Simplify model name - strip provider prefix if present
								let model_parts: Vec<&str> = info.model.split('/').collect();
								let model_name = if model_parts.len() > 1 { model_parts[1] } else { &info.model };

								// Calculate total tokens
								let total_tokens = info.input_tokens + info.output_tokens + info.cached_tokens;

								println!("{:<20} {:<25} {:<15} {:<10} ${:<.5}",
									name_display,
									created_time.blue(),
									model_name.yellow(),
									total_tokens.to_string().bright_blue(),
									info.total_cost.to_string().bright_magenta());
							}

							println!();
							println!("{}", "You can switch to another session with:".blue());
							println!("{}", "  /session <session_name>".bright_green());
							println!("{}", "  /session (creates a new session)".bright_green());
							println!();
						}
					},
					Err(e) => {
						println!("{}: {}", "Failed to list sessions".bright_red(), e);
					}
				}
			},
			SESSION_COMMAND => {
				// Handle session switching
				if params.is_empty() {
					// If no session name provided, create a new session with a random name
					// Use the same timestamp-based naming convention as in the main function
					let timestamp = std::time::SystemTime::now()
						.duration_since(std::time::UNIX_EPOCH)
						.unwrap_or_default()
						.as_secs();
					let new_session_name = format!("session_{}", timestamp);

					println!("{}", format!("Creating new session: {}", new_session_name).bright_green());

					// Save current session before switching - no need to save here
					// The main loop will handle saving before switching

					// Set the session name to return
					self.session.info.name = new_session_name;
					return Ok(true);
				} else {
					// Get the session name from the parameters
					let new_session_name = params.join(" ");
					let current_session_path = self.session.session_file.clone();

					// Check if we're already in this session
					if let Some(current_path) = &current_session_path {
						if current_path.file_stem().and_then(|s| s.to_str()).unwrap_or("") == new_session_name {
							println!("{}", "You are already in this session.".blue());
							return Ok(false);
						}
					}

					// Return a signal to the main loop with the session name to switch to
					// We'll use a specific return code that tells the main loop to switch sessions
					self.session.info.name = new_session_name;
					return Ok(true);
				}
			},
			_ => return Ok(false), // Not a command
		}

		Ok(false) // Continue session
	}
}