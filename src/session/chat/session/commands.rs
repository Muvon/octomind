// Session command processing

use super::core::ChatSession;
use crate::{config::{Config, LogLevel}, log_info};
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
		let input_parts: Vec<&str> = input.split_whitespace().collect();
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
				println!("{} - Show this help message", HELP_COMMAND.cyan());
				println!("{} - Copy last response to clipboard", COPY_COMMAND.cyan());
				println!("{} - Clear the screen", CLEAR_COMMAND.cyan());
				println!("{} - Save the session", SAVE_COMMAND.cyan());
				println!("{} - Manage cache checkpoints: /cache [stats|clear|threshold]", CACHE_COMMAND.cyan());
				println!("{} - List all available sessions", LIST_COMMAND.cyan());
				println!("{} [name] - Switch to another session or create a new one (without name creates fresh session)", SESSION_COMMAND.cyan());
				println!("{} - Display detailed token and cost breakdown for this session", INFO_COMMAND.cyan());
				println!("{} - Toggle layered processing architecture on/off", LAYERS_COMMAND.cyan());
				println!("{} - Optimize the session context, restart layered processing for next message, and apply EditorConfig formatting", DONE_COMMAND.cyan());
				println!("{} [level] - Set logging level: none, info, or debug", LOGLEVEL_COMMAND.cyan());
				println!("{} [threshold] - Toggle automatic context truncation when token limit is reached", TRUNCATE_COMMAND.cyan());
				println!("{} [model] - Show current model or change to a different model", MODEL_COMMAND.cyan());
				println!("{} or {} - Exit the session\n", EXIT_COMMAND.cyan(), QUIT_COMMAND.cyan());

				// Additional info about caching
				println!("{}", "** About Cache Management **".bright_yellow());
				println!("The system message and tool definitions are automatically cached for supported providers.");
				println!("Use '/cache' to mark your last user message for caching.");
				println!("Use '/cache stats' to view detailed cache statistics and efficiency.");
				println!("Use '/cache clear' to remove content cache markers (keeps system/tool caches).");
				println!("Use '/cache threshold' to view auto-cache settings.");
				println!("Supports 2-marker system: when you add a 3rd marker, the first one moves to the new position.");
				println!("Automatic caching triggers based on token threshold percentage (configurable).");
				println!("Cached tokens reduce costs on subsequent requests with the same content.\n");

				// Add information about layered architecture
				println!("{}", "** About Layered Processing **".bright_yellow());
				println!("The layered architecture processes your initial query through multiple AI layers:");
				println!("1. Query Processor: Improves your initial query");
				println!("2. Context Generator: Gathers relevant context information");
				println!("3. Developer: Executes the actual development work");
				println!("The Reducer functionality is available through the /done command.");
				println!("Only the first message in a session uses the full layered architecture.");
				println!("Subsequent messages use direct communication with the developer model.");
				println!("Use the /done command to optimize context, apply EditorConfig formatting to edited files, and restart the layered pipeline.");
				println!("Toggle layered processing with /layers command.\n");
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

				// For now, we'll default to developer role since that's where layers are typically used
				// In the future, this could be passed as a parameter from the session context
				let current_role = "developer"; // TODO: Get this from session context
				
				// Toggle the setting for the appropriate role
				match current_role {
					"developer" => {
						loaded_config.developer.config.enable_layers = !loaded_config.developer.config.enable_layers;
					},
					"assistant" => {
						loaded_config.assistant.config.enable_layers = !loaded_config.assistant.config.enable_layers;
					},
					_ => {
						// Fall back to global config for unknown roles
						loaded_config.openrouter.enable_layers = !loaded_config.openrouter.enable_layers;
					}
				}

				// Save the updated config
				if let Err(e) = loaded_config.save() {
					println!("{}: {}", "Failed to save configuration".bright_red(), e);
					return Ok(false);
				}

				// Get the current state from the appropriate config section
				let is_enabled = match current_role {
					"developer" => loaded_config.developer.config.enable_layers,
					"assistant" => loaded_config.assistant.config.enable_layers,
					_ => loaded_config.openrouter.enable_layers,
				};

				// Show the new state
				if is_enabled {
					println!("{}", "Layered processing architecture is now ENABLED.".bright_green());
					println!("{}", "Your queries will now be processed through multiple AI models.".bright_yellow());
				} else {
					println!("{}", "Layered processing architecture is now DISABLED.".bright_yellow());
					// println!("{}", "Using standard single-model processing with Claude.".bright_blue());
				}
				log_info!("Configuration has been saved to disk.");

				// Return a special code that indicates we should reload the config in the main loop
				// This will ensure all future commands use the updated config
				return Ok(true);
			},
			LOGLEVEL_COMMAND => {
				// Handle log level command
				let mut loaded_config = match crate::config::Config::load() {
					Ok(cfg) => cfg,
					Err(_) => {
						println!("{}", "Error loading configuration file. Using current settings instead.".bright_red());
						config.clone()
					}
				};

				if params.is_empty() {
					// Show current log level
					let current_level = match loaded_config.openrouter.log_level {
						LogLevel::None => "none",
						LogLevel::Info => "info",
						LogLevel::Debug => "debug",
					};
					println!("{}", format!("Current log level: {}", current_level).bright_cyan());
					println!("{}", "Available levels: none, info, debug".bright_yellow());
					return Ok(false);
				}

				// Parse the requested log level
				let new_level = match params[0].to_lowercase().as_str() {
					"none" => LogLevel::None,
					"info" => LogLevel::Info,
					"debug" => LogLevel::Debug,
					_ => {
						println!("{}", "Invalid log level. Use: none, info, or debug".bright_red());
						return Ok(false);
					}
				};

				// Update the configuration
				loaded_config.openrouter.log_level = new_level.clone();

				// Save the updated config
				if let Err(e) = loaded_config.save() {
					println!("{}: {}", "Failed to save configuration".bright_red(), e);
					return Ok(false);
				}

				// Show the new state
				match new_level {
					LogLevel::None => {
						println!("{}", "Log level set to NONE.".bright_yellow());
						println!("{}", "Only essential information will be displayed.".bright_blue());
					},
					LogLevel::Info => {
						println!("{}", "Log level set to INFO.".bright_green());
						println!("{}", "Moderate logging will be shown.".bright_yellow());
					},
					LogLevel::Debug => {
						println!("{}", "Log level set to DEBUG.".bright_green());
						println!("{}", "Detailed logging will be shown for API calls and tool executions.".bright_yellow());
					}
				}
				log_info!("Configuration has been saved to disk.");

				// Return a special code that indicates we should reload the config in the main loop
				return Ok(true);
			},
			DEBUG_COMMAND => {
				// Backward compatibility - toggle between none and debug
				let mut loaded_config = match crate::config::Config::load() {
					Ok(cfg) => cfg,
					Err(_) => {
						println!("{}", "Error loading configuration file. Using current settings instead.".bright_red());
						config.clone()
					}
				};

				// Toggle between none and debug for backward compatibility
				loaded_config.openrouter.log_level = match loaded_config.openrouter.log_level {
					LogLevel::Debug => LogLevel::None,
					_ => LogLevel::Debug,
				};

				// Save the updated config
				if let Err(e) = loaded_config.save() {
					println!("{}: {}", "Failed to save configuration".bright_red(), e);
					return Ok(false);
				}

				// Show the new state
				if loaded_config.openrouter.log_level.is_debug_enabled() {
					println!("{}", "Debug mode is now ENABLED.".bright_green());
					println!("{}", "Detailed logging will be shown for API calls and tool executions.".bright_yellow());
				} else {
					println!("{}", "Debug mode is now DISABLED.".bright_yellow());
					println!("{}", "Only essential information will be displayed.".bright_blue());
				}
				log_info!("Configuration has been saved to disk.");

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
				if !params.is_empty() {
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
				log_info!("Configuration has been saved to disk.");

				// Return a special code that indicates we should reload the config in the main loop
				return Ok(true);
			},
			CACHE_COMMAND => {
				// Parse cache command arguments for advanced functionality
				if params.is_empty() {
					// Default behavior - set flag to cache the NEXT user message
					let supports_caching = crate::session::model_supports_caching(&self.session.info.model);
					if !supports_caching {
						println!("{}", "This model does not support caching.".bright_yellow());
					} else {
						// Set the flag to cache the next user message
						self.cache_next_user_message = true;
						println!("{}", "The next user message will be marked for caching.".bright_green());
						
						// Show cache statistics
						let cache_manager = crate::session::cache::CacheManager::new();
						let stats = cache_manager.get_cache_statistics(&self.session);
						println!("{}", stats.format_for_display());
					}
				} else {
					match params[0] {
						"stats" => {
							// Show detailed cache statistics
							let cache_manager = crate::session::cache::CacheManager::new();
							let stats = cache_manager.get_cache_statistics(&self.session);
							println!("{}", stats.format_for_display());
							
							// Additional threshold information
							println!("{}", format!(
								"Auto-cache threshold: {}%", 
								if let Ok(config) = crate::config::Config::load() {
									config.openrouter.cache_tokens_pct_threshold
								} else {
									40
								}
							).bright_blue());
						},
						"clear" => {
							// Clear content cache markers (but keep system markers)
							let cache_manager = crate::session::cache::CacheManager::new();
							let cleared = cache_manager.clear_content_cache_markers(&mut self.session);
							
							if cleared > 0 {
								println!("{}", format!("Cleared {} content cache markers", cleared).bright_green());
								let _ = self.save();
							} else {
								println!("{}", "No content cache markers to clear".bright_yellow());
							}
						},
						"threshold" => {
							// Show current threshold settings
							if let Ok(config) = crate::config::Config::load() {
								if config.openrouter.cache_tokens_absolute_threshold > 0 {
									println!("{}", format!("Current auto-cache threshold: {} tokens (absolute)", 
										config.openrouter.cache_tokens_absolute_threshold).bright_cyan());
									println!("{}", format!("Auto-cache will trigger when non-cached tokens reach {} tokens", 
										config.openrouter.cache_tokens_absolute_threshold).bright_blue());
								} else {
									let threshold = config.openrouter.cache_tokens_pct_threshold;
									println!("{}", format!("Current auto-cache threshold: {}% (percentage)", threshold).bright_cyan());
									
									if threshold == 0 || threshold == 100 {
										println!("{}", "Auto-cache is disabled".bright_yellow());
									} else {
										println!("{}", format!("Auto-cache will trigger when non-cached tokens reach {}% of total", threshold).bright_blue());
									}
								}
								
								// Show time-based threshold
								let timeout_seconds = config.openrouter.cache_timeout_seconds;
								if timeout_seconds > 0 {
									let timeout_minutes = timeout_seconds / 60;
									println!("{}", format!("Time-based auto-cache: {} seconds ({} minutes)", 
										timeout_seconds, timeout_minutes).bright_green());
									println!("{}", format!("Auto-cache will trigger if {} minutes pass since last checkpoint", 
										timeout_minutes).bright_blue());
								} else {
									println!("{}", "Time-based auto-cache is disabled".bright_yellow());
								}
							} else {
								println!("{}", "Could not load configuration".bright_red());
							}
						},
						_ => {
							println!("{}", "Invalid cache command. Usage:".bright_red());
							println!("{}", "  /cache - Add cache checkpoint at last user message".cyan());
							println!("{}", "  /cache stats - Show detailed cache statistics".cyan());
							println!("{}", "  /cache clear - Clear content cache markers".cyan());
							println!("{}", "  /cache threshold - Show auto-cache threshold settings".cyan());
						}
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
			MODEL_COMMAND => {
				// Handle model command
				if params.is_empty() {
					// Show current model
					println!("{}", format!("Current model: {}", self.model).bright_cyan());
					return Ok(false);
				}

				// Change to a new model
				let new_model = params.join(" ");
				let old_model = self.model.clone();
				self.model = new_model.clone();
				self.session.info.model = new_model.clone();

				println!("{}", format!("Model changed from {} to {}", old_model, new_model).bright_green());
				println!("{}", "The new model will be used for future messages in this session.".bright_yellow());

				// Save the session with the updated model
				if let Err(e) = self.save() {
					println!("{}: {}", "Failed to save session with new model".bright_red(), e);
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
