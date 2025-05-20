// Chat session implementation

use crate::store::Store;
use crate::config::Config;
use crate::session::{Session, get_sessions_dir, load_session, create_system_prompt, openrouter, list_available_sessions};
use std::io::{self, Write};
use std::fs::File;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use anyhow::Result;
use chrono::{DateTime, Utc};
use ctrlc;
use colored::Colorize;
use super::commands::*;
use super::input::read_user_input;
use super::response::process_response;
use super::animation::show_loading_animation;

// Chat session manager for interactive coding sessions
pub struct ChatSession {
	pub session: Session,
	pub last_response: String,
	pub model: String,
	pub temperature: f32,
	pub estimated_cost: f64,
}

impl ChatSession {
	// Display detailed information about the session, including layer-specific stats
	pub fn display_session_info(&self) {
		use colored::*;

		// Display overall session metrics
		println!("{}", "───────────── Session Information ─────────────".bright_cyan());

		// Session basics
		println!("{} {}", "Session name:".yellow(), self.session.info.name.bright_white());
		println!("{} {}", "Main model:".yellow(), self.session.info.model.bright_white());

		// Total token usage
		let total_tokens = self.session.info.input_tokens + self.session.info.output_tokens + self.session.info.cached_tokens;
		println!("{} {}", "Total tokens:".yellow(), total_tokens.to_string().bright_white());
		println!("{} {} input, {} output, {} cached",
			"Breakdown:".yellow(),
			self.session.info.input_tokens.to_string().bright_blue(),
			self.session.info.output_tokens.to_string().bright_green(),
			self.session.info.cached_tokens.to_string().bright_magenta());

		// Cost information
		println!("{} ${:.5}", "Total cost:".yellow(), self.session.info.total_cost);

		// Messages count
		println!("{} {}", "Messages:".yellow(), self.session.messages.len());

		// Display layered stats if available
		if !self.session.info.layer_stats.is_empty() {
			println!();
			println!("{}", "───────────── Layer-by-Layer Statistics ─────────────".bright_cyan());

			// Group by layer type
			let mut layer_stats: std::collections::HashMap<String, Vec<&crate::session::LayerStats>> = std::collections::HashMap::new();

			// Group stats by layer type
			for stat in &self.session.info.layer_stats {
				layer_stats.entry(stat.layer_type.clone())
					.or_insert_with(Vec::new)
					.push(stat);
			}

			// Print stats for each layer type
			for (layer_type, stats) in layer_stats.iter() {
				// Add special highlighting for context optimization
				let layer_display = if layer_type == "context_optimization" {
					format!("Layer: {}", layer_type).bright_magenta()
				} else {
					format!("Layer: {}", layer_type).bright_yellow()
				};

				println!("{}", layer_display);

				// Count total tokens and cost for this layer type
				let mut total_input = 0;
				let mut total_output = 0;
				let mut total_cost = 0.0;

				// Count executions
				let executions = stats.len();

				for stat in stats {
					total_input += stat.input_tokens;
					total_output += stat.output_tokens;
					total_cost += stat.cost;
				}

				// Print the stats
				println!("  {}: {}", "Model".blue(), stats[0].model);
				println!("  {}: {}", "Executions".blue(), executions);
				println!("  {}: {} input, {} output",
					"Tokens".blue(),
					total_input.to_string().bright_white(),
					total_output.to_string().bright_white());
				println!("  {}: ${:.5}", "Cost".blue(), total_cost);

				// Add special note for context optimization
				if layer_type == "context_optimization" {
					println!("  {}", "Note: These are costs for optimizing context between interactions".bright_cyan());
				}

				println!();
			}
		} else {
			println!();
			println!("{}", "No layer-specific statistics available.".bright_yellow());
			println!("{}", "This may be because the session was created before layered architecture was enabled.".bright_yellow());
		}

		println!();
	}

	// Create a new chat session
	pub fn new(name: String, model: Option<String>, config: &Config) -> Self {
		let model_name = model.unwrap_or_else(|| config.openrouter.model.clone());

		// Create a new session with initial info
		let session_info = crate::session::SessionInfo {
			name: name.clone(),
			created_at: SystemTime::now()
				.duration_since(UNIX_EPOCH)
				.unwrap_or_default()
				.as_secs(),
			model: model_name.clone(),
			provider: "openrouter".to_string(),
			input_tokens: 0,
			output_tokens: 0,
			cached_tokens: 0,
			total_cost: 0.0,
			duration_seconds: 0,
			layer_stats: Vec::new(), // Initialize empty layer stats
		};

		Self {
			session: Session {
				info: session_info,
				messages: Vec::new(),
				session_file: None,
			},
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

		// Check if we should load or create a session
		let should_resume = (resume.is_some() || (name.is_some() && session_file.exists())) && session_file.exists();

		if should_resume {
			use colored::*;

			// Try to load session
			match load_session(&session_file) {
				Ok(session) => {
					// When session is loaded successfully, show its info
					println!("{}", format!("✓ Resuming session: {}", session_name).bright_green());

					// Show a brief summary of the session
					let created_time = DateTime::<Utc>::from_timestamp(session.info.created_at as i64, 0)
						.map(|dt| dt.naive_local().format("%Y-%m-%d %H:%M:%S").to_string())
						.unwrap_or_else(|| "Unknown".to_string());

					// Simplify model name
					let model_parts: Vec<&str> = session.info.model.split('/').collect();
					let model_name = if model_parts.len() > 1 { model_parts[1] } else { &session.info.model };

					// Calculate total tokens
					let total_tokens = session.info.input_tokens + session.info.output_tokens + session.info.cached_tokens;

					println!("{} {}", "Created:".blue(), created_time.white());
					println!("{} {}", "Model:".blue(), model_name.yellow());
					println!("{} {}", "Messages:".blue(), session.messages.len().to_string().white());
					println!("{} {}", "Tokens:".blue(), total_tokens.to_string().bright_blue());
					println!("{} ${:.5}", "Cost:".blue(), session.info.total_cost.to_string().bright_magenta());

					// Create chat session from loaded session
					let mut chat_session = ChatSession {
						session,
						last_response: String::new(),
						model: model.unwrap_or_else(|| config.openrouter.model.clone()),
						temperature: 0.7,
						estimated_cost: 0.0,
					};

					// Update the estimated cost from the loaded session
					chat_session.estimated_cost = chat_session.session.info.total_cost;

					// Get last assistant response if any
					for msg in chat_session.session.messages.iter().rev() {
						if msg.role == "assistant" {
							chat_session.last_response = msg.content.clone();
							break;
						}
					}

					Ok(chat_session)
				},
				Err(e) => {
					// If loading fails, inform the user and create a new session
					println!("{}: {}", format!("Failed to load session {}", session_name).bright_red(), e);
					println!("{}", "Creating a new session instead...".yellow());

					// Generate a new unique session name
					let timestamp = std::time::SystemTime::now()
						.duration_since(std::time::UNIX_EPOCH)
						.unwrap_or_default()
						.as_secs();
					let new_session_name = format!("session_{}", timestamp);
					let new_session_file = sessions_dir.join(format!("{}.jsonl", new_session_name));

					println!("{}", format!("Starting new session: {}", new_session_name).bright_green());

					// Create file if it doesn't exist
					if !new_session_file.exists() {
						let file = File::create(&new_session_file)?;
						drop(file);
					}

					let mut chat_session = ChatSession::new(new_session_name, model, config);
					chat_session.session.session_file = Some(new_session_file);

					// Immediately save the session info to ensure SUMMARY is written
					let info_json = serde_json::to_string(&chat_session.session.info)?;
					crate::session::append_to_session_file(
						chat_session.session.session_file.as_ref().unwrap(),
						&format!("SUMMARY: {}", info_json)
					)?;

					Ok(chat_session)
				}
			}
		} else {
			// Create new session
			use colored::*;
			println!("{}", format!("Starting new session: {}", session_name).bright_green());

			// Create session file if it doesn't exist
			if !session_file.exists() {
				let file = File::create(&session_file)?;
				drop(file);
			}

			let mut chat_session = ChatSession::new(session_name, model, config);
			chat_session.session.session_file = Some(session_file);

			// Immediately save the session info to ensure SUMMARY is written
			let info_json = serde_json::to_string(&chat_session.session.info)?;
			crate::session::append_to_session_file(
				chat_session.session.session_file.as_ref().unwrap(),
				&format!("SUMMARY: {}", info_json)
			)?;

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
			crate::session::append_to_session_file(session_file, &message_json)?;
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
			crate::session::append_to_session_file(session_file, &message_json)?;
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
				// Calculate regular and cached tokens
				let mut regular_prompt_tokens = usage.prompt_tokens;
				let mut cached_tokens = 0;

				// Check prompt_tokens_details for cached_tokens first
				if let Some(details) = &usage.prompt_tokens_details {
					if let Some(cached) = details.get("cached_tokens") {
						if let serde_json::Value::Number(num) = cached {
							if let Some(num_u64) = num.as_u64() {
								cached_tokens = num_u64;
								// Adjust regular tokens to account for cached tokens
								regular_prompt_tokens = usage.prompt_tokens.saturating_sub(cached_tokens);
							}
						}
					}
				}

				// Fall back to breakdown field if prompt_tokens_details didn't have cached tokens
				if cached_tokens == 0 && usage.prompt_tokens > 0 {
					if let Some(breakdown) = &usage.breakdown {
						if let Some(cached) = breakdown.get("cached") {
							if let serde_json::Value::Number(num) = cached {
								if let Some(num_u64) = num.as_u64() {
									cached_tokens = num_u64;
									// Adjust regular tokens to account for cached tokens
									regular_prompt_tokens = usage.prompt_tokens.saturating_sub(cached_tokens);
								}
							}
						}
					}
				}

				// Check for cached tokens in the base API response for models that report differently
				if cached_tokens == 0 && usage.prompt_tokens > 0 {
					if let Some(response) = &ex.response.get("usage") {
						if let Some(cached) = response.get("cached_tokens") {
							if let Some(num) = cached.as_u64() {
								cached_tokens = num;
								regular_prompt_tokens = usage.prompt_tokens.saturating_sub(cached_tokens);
							}
						}
					}
				}

				// Update session token counts
				self.session.info.input_tokens += regular_prompt_tokens;
				self.session.info.output_tokens += usage.completion_tokens;
				self.session.info.cached_tokens += cached_tokens;

				// If OpenRouter provided cost data, use it directly
				if let Some(cost) = usage.cost {
					// OpenRouter credits = dollars, use the value directly
					self.session.info.total_cost += cost;
					self.estimated_cost = self.session.info.total_cost;
					
					// Log the actual cost received from the API for debugging
					if config.openrouter.debug {
						println!("Debug: Adding ${:.5} from OpenRouter API (total now: ${:.5})", 
							cost, self.session.info.total_cost);
						
						// Enhanced debug: dump full usage object
						println!("Debug: Full usage object:");
						if let Ok(usage_str) = serde_json::to_string_pretty(usage) {
							println!("{}", usage_str);
						}
						
						// Look for any cache-related fields
						if let Some(breakdown) = &usage.breakdown {
							println!("Debug: Usage breakdown:");
							for (key, value) in breakdown {
								println!("  {} = {}", key, value);
							}
						}
						
						// Check if there's a raw usage object with additional fields
						if let Some(raw_usage) = ex.response.get("usage") {
							println!("Debug: Raw usage from response:");
							if let Ok(raw_str) = serde_json::to_string_pretty(raw_usage) {
								println!("{}", raw_str);
							}
						}
					}
				} else {
					// No explicit cost data, look at the raw response to check if it contains cost data
					let cost_from_raw = ex.response.get("usage")
						.and_then(|u| u.get("cost"))
						.and_then(|c| c.as_f64());
						
					if let Some(cost) = cost_from_raw {
						// Use the cost value directly
						self.session.info.total_cost += cost;
						self.estimated_cost = self.session.info.total_cost;
						
						// Log that we had to fetch cost from raw response
						if config.openrouter.debug {
							println!("Debug: Using cost from raw response: ${:.5} (total now: ${:.5})", 
								cost, self.session.info.total_cost);
						}
					} else {
						// ERROR - OpenRouter did not provide cost data
						println!("{}", "ERROR: OpenRouter did not provide cost data. Make sure usage.include=true is set!".bright_red());
						
						// Dump the raw response JSON to debug
						if config.openrouter.debug {
							println!("{}", "Raw OpenRouter response:".bright_red());
							if let Ok(resp_str) = serde_json::to_string_pretty(&ex.response) {
								println!("{}", resp_str);
							}
							
							// Check if usage tracking was explicitly requested
							let has_usage_flag = ex.request.get("usage")
								.and_then(|u| u.get("include"))
								.and_then(|i| i.as_bool())
								.unwrap_or(false);
								
							println!("{} {}", "Request had usage.include flag:".bright_yellow(), has_usage_flag);
						}
					}
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
			crate::session::append_to_session_file(session_file, &message_json)?;

			// If we have a raw exchange, save it as well
			if let Some(ex) = exchange {
				let exchange_json = serde_json::to_string(&ex)?;
				crate::session::append_to_session_file(session_file, &exchange_json)?;
			}
		}

		Ok(())
	}

	// Process user commands
	pub fn process_command(&mut self, input: &str, config: &Config) -> Result<bool> {
		use colored::*;

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
				println!("{} or {} - {}", EXIT_COMMAND.cyan(), QUIT_COMMAND.cyan(), "Exit the session");
				println!("{} - {}", COPY_COMMAND.cyan(), "Copy last response to clipboard");
				println!("{} - {}", CLEAR_COMMAND.cyan(), "Clear the screen");
				println!("{} - {}", SAVE_COMMAND.cyan(), "Save the session");
				println!("{} - {}", CACHE_COMMAND.cyan(), "Mark a cache checkpoint at the last user message");
				println!("{} - {}", LIST_COMMAND.cyan(), "List all available sessions");
				println!("{} [name] - {}", SESSION_COMMAND.cyan(), "Switch to another session or create a new one (without name creates fresh session)");
				println!("{} - {}", INFO_COMMAND.cyan(), "Display detailed token and cost breakdown for this session");
				println!("{} - {}", LAYERS_COMMAND.cyan(), "Toggle layered processing architecture on/off");
				println!("{} - {}", DONE_COMMAND.cyan(), "Optimize the session context and restart layered processing for next message");
				println!("{} - {}", DEBUG_COMMAND.cyan(), "Toggle debug mode for detailed logs");
				println!("{} - {}\n", HELP_COMMAND.cyan(), "Show this help message");

				// Additional info about caching
				println!("{}", "** About Cache Checkpoints **".bright_yellow());
				println!("{}", "The system message with function definitions is automatically cached for all sessions.");
				println!("{}", "Use '/cache' to mark your last user message for caching.");
				println!("{}", "This is useful for large text blocks like code snippets that don't change between requests.");
				println!("{}", "The model provider will charge less for cached content in subsequent requests.");
				println!("{}", "Cached tokens will be displayed in the usage statistics after your next message.");
				println!("{}", "Best practice: Use separate messages with the most data-heavy part marked for caching.\n");

				// Add information about layered architecture
				println!("{}", "** About Layered Processing **".bright_yellow());
				println!("{}", "The layered architecture processes your initial query through multiple AI layers:");
				println!("{}", "1. Query Processor: Improves your initial query");
				println!("{}", "2. Context Generator: Gathers relevant context information");
				println!("{}", "3. Developer: Executes the actual development work");
				println!("{}", "The Reducer functionality is available through the /done command.");
				println!("{}", "Only the first message in a session uses the full layered architecture.");
				println!("{}", "Subsequent messages use direct communication with the developer model.");
				println!("{}", "Use the /done command to optimize context and restart the layered pipeline.");
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
			CACHE_COMMAND => {
				// Default behavior - cache the last user message
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

	// Track if the first message has been processed through layers
	let mut first_message_processed = !chat_session.session.messages.is_empty();
	println!("Interactive coding session started. Type your questions/requests.");
	println!("Type /help for available commands.");

	// Initialize with system prompt if new session
	if chat_session.session.messages.is_empty() {
		// Create system prompt
		let system_prompt = create_system_prompt(&current_dir, config).await;
		chat_session.add_system_message(&system_prompt)?;

		// Mark system message with function declarations as cached by default
		// This ensures all heavy initial context is cached to save on tokens
		if let Ok(cached) = chat_session.session.add_cache_checkpoint(true) {
			if cached {
				println!("{}", "System prompt has been marked for caching to save tokens in future interactions.".yellow());
				// Save the session to ensure the cached status is persisted
				let _ = chat_session.save();
			} else {
				println!("{}", "Warning: Failed to mark system prompt for caching.".red());
			}
		} else {
			println!("{}", "Error: Could not set cache checkpoint for system message.".bright_red());
		}

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
	let ctrl_c_pressed = Arc::new(AtomicBool::new(false));
	let ctrl_c_pressed_clone = ctrl_c_pressed.clone();

	// Set up Ctrl+C handler
	ctrlc::set_handler(move || {
		// If already set, do a hard exit to break out of any operation
		if ctrl_c_pressed_clone.load(Ordering::SeqCst) {
			println!("\nForcing exit due to repeated Ctrl+C...");
			std::process::exit(130); // 130 is standard exit code for SIGINT
		}

		ctrl_c_pressed_clone.store(true, Ordering::SeqCst);
		println!("\nCtrl+C pressed, will cancel after current operation completes.");
		println!("Press Ctrl+C again to force immediate exit.");
	}).expect("Error setting Ctrl+C handler");

	// We need to handle configuration reloading, so keep our own copy that we can update
	let mut current_config = config.clone();

	// Main interaction loop
	loop {
		// Check if Ctrl+C was pressed
		if ctrl_c_pressed.load(Ordering::SeqCst) {
			// Reset for next time
			ctrl_c_pressed.store(false, Ordering::SeqCst);
			println!("\nOperation cancelled.");
			continue;
		}

		// Create a fresh cancellation flag for this iteration
		// Each request gets its own cancellation flag derived from the global one
		let operation_cancelled = Arc::new(AtomicBool::new(false));

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
			// Handle special /done command separately
			if input.trim() == "/done" {
				// Reset first_message_processed to false so that the next message goes through layers again
				first_message_processed = false;

				// Apply reducer functionality to optimize context
				let result = super::perform_context_reduction(
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
				}
				continue;
			}

			let exit = chat_session.process_command(&input, &current_config)?;
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
				} else if input.starts_with(LAYERS_COMMAND) || input.starts_with(DEBUG_COMMAND) {
					// This is a command that requires config reload
					// Reload the configuration
					match crate::config::Config::load() {
						Ok(updated_config) => {
							// Update our current config
							current_config = updated_config;
							use colored::Colorize;
							println!("{}", "Configuration reloaded successfully".bright_green());
						},
						Err(e) => {
							use colored::Colorize;
							println!("{}: {}", "Error reloading configuration".bright_red(), e);
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
		// This is crucial - we need a fresh flag for each response processing cycle
		let process_cancelled = Arc::new(AtomicBool::new(false));

		// Check if Ctrl+C was pressed (and the operation was cancelled)
		if ctrl_c_pressed.load(Ordering::SeqCst) {
			// Already handled at the start of the loop
			ctrl_c_pressed.store(false, Ordering::SeqCst);
			continue;
		}

		// Add user message
		// (This moved to process_response or process_layered_response)

		// Check if layered architecture is enabled AND this is the first message
		if current_config.openrouter.enable_layers && !first_message_processed {
			// Process using layered architecture for the first message only
			let process_result = super::process_layered_response(
				&input,
				&mut chat_session,
				&current_config,
				process_cancelled.clone()
			).await;

			if let Err(e) = process_result {
				// Print colorful error message
				use colored::*;
				println!("\n{}: {}", "Error processing response".bright_red(), e);
			}

			// Mark that we've processed the first message through layers
			first_message_processed = true;
		} else {
			// Add user message for standard processing flow
			chat_session.add_user_message(&input)?;

			// Ensure system message is cached before making API calls
			// This is important for token savings since system message typically contains
			// all the function definitions and is unlikely to change
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
					if cached {
						println!("{}", "System message has been automatically marked for caching to save tokens.".yellow());
						// Save the session to ensure the cached status is persisted
						let _ = chat_session.save();
					}
				}
			}

			// Convert messages to OpenRouter format
			let or_messages = openrouter::convert_messages(&chat_session.session.messages);

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

			// Start a separate task to monitor for Ctrl+C
			let op_cancelled = operation_cancelled.clone();
			let ctrlc_flag = ctrl_c_pressed.clone();
			let _cancel_monitor = tokio::spawn(async move {
				while !op_cancelled.load(Ordering::SeqCst) {
					// Check if global Ctrl+C flag is set
					if ctrlc_flag.load(Ordering::SeqCst) {
						// Set the operation cancellation flag
						op_cancelled.store(true, Ordering::SeqCst);
						break; // Exit the loop once cancelled
					}
					tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
				}
			});

			// Now directly perform the API call - ensure usage parameter is included
			// for consistent cost tracking across all API requests
			let api_result = openrouter::chat_completion(
				or_messages,
				&model,
				temperature,
				&config_clone
			).await;

			// Stop the animation - but use TRUE to stop it, not false!
			operation_cancelled.store(true, Ordering::SeqCst);
			let _ = animation_task.await;

			// Process the response
			match api_result {
				Ok((content, exchange)) => {
					// Process the response, handling tool calls recursively
					// Create a fresh cancellation flag to avoid any "Operation cancelled" messages when not requested
					let tool_process_cancelled = Arc::new(AtomicBool::new(false));
					let process_result = process_response(
						content,
						exchange,
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
	}

	Ok(())
}
