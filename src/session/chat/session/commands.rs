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

// Session command processing

use super::super::command_executor;
use super::super::commands::*;
use super::core::ChatSession;
use super::utils::format_number;
use crate::session::chat::assistant_output::print_assistant_response;
use crate::session::list_available_sessions;
use crate::{
	config::{Config, LogLevel},
	log_info,
};
use anyhow::Result;
use arboard::Clipboard;
use chrono::{DateTime, Utc};
use colored::Colorize;
use std::io::{self, Write};

// Helper function to get the actual server name for a tool using the same logic as execution
async fn get_tool_server_name_async(tool_name: &str, config: &Config) -> String {
	let tool_server_map = crate::mcp::build_tool_server_map(config).await;

	if let Some(target_server) = tool_server_map.get(tool_name) {
		target_server.name.clone()
	} else {
		// Fallback to category guess if no server found, but ensure we still show the tool
		crate::mcp::guess_tool_category(tool_name).to_string()
	}
}

impl ChatSession {
	// Process user commands
	pub async fn process_command(
		&mut self,
		input: &str,
		config: &mut Config,
		role: &str,
	) -> Result<bool> {
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
				println!(
					"{}",
					"Ending session. Your conversation has been saved.".bright_green()
				);
				return Ok(true);
			}
			HELP_COMMAND => {
				println!("{}", "\nAvailable commands:\n".bright_cyan());
				println!("{} - Show this help message", HELP_COMMAND.cyan());
				println!("{} - Copy last response to clipboard", COPY_COMMAND.cyan());
				println!("{} - Clear the screen", CLEAR_COMMAND.cyan());
				println!("{} - Save the session", SAVE_COMMAND.cyan());
				println!(
					"{} - Manage cache checkpoints: /cache [stats|clear|threshold]",
					CACHE_COMMAND.cyan()
				);
				println!(
					"{} [page] - List all available sessions with pagination (default: page 1)",
					LIST_COMMAND.cyan()
				);
				println!("{} [name] - Switch to another session or create a new one (without name creates fresh session)", SESSION_COMMAND.cyan());
				println!(
					"{} - Display detailed token and cost breakdown for this session",
					INFO_COMMAND.cyan()
				);
				println!(
					"{} - Toggle layered processing architecture on/off",
					LAYERS_COMMAND.cyan()
				);
				println!("{} - Optimize the session context, restart layered processing for next message, and apply EditorConfig formatting", DONE_COMMAND.cyan());
				println!(
					"{} [level] - Set logging level: none, info, or debug",
					LOGLEVEL_COMMAND.cyan()
				);
				println!(
					"{} - Perform smart context truncation to reduce token usage",
					TRUNCATE_COMMAND.cyan()
				);
				println!(
					"{} - Create intelligent summary of entire conversation using local processing",
					SUMMARIZE_COMMAND.cyan()
				);
				println!(
					"{} [model] - Show current model or change to a different model (runtime only)",
					MODEL_COMMAND.cyan()
				);
				println!(
					"{} <command_name> - Execute a command layer (e.g., /run estimate)",
					RUN_COMMAND.cyan()
				);
				println!(
					"{} [list|info|full] - Show MCP server status and tools (info is default)",
					MCP_COMMAND.cyan()
				);
				println!(
					"{} - Generate detailed usage report with cost breakdown per request",
					REPORT_COMMAND.cyan()
				);
				println!(
					"{} <path_or_url> - Attach image to your next message (supports PNG, JPEG, GIF, WebP, BMP)",
					IMAGE_COMMAND.cyan()
				);
				println!(
					"{} or {} - Exit the session\n",
					EXIT_COMMAND.cyan(),
					QUIT_COMMAND.cyan()
				);

				// Add keyboard shortcuts section
				println!("{}", "Keyboard shortcuts:\n".bright_cyan());
				println!(
					"{} - Insert newline for multi-line input",
					"Ctrl+J".bright_green()
				);
				println!("{} - Accept hint/completion", "Ctrl+E".bright_green());
				println!("{} - Cancel input", "Ctrl+C".bright_green());
				println!("{} - Exit session", "Ctrl+D".bright_green());
				println!();

				// Additional info about caching
				println!("{}", "** About Cache Management **".bright_yellow());
				println!("The system message and tool definitions are automatically cached for supported providers.");
				println!("Use '/cache' to mark your last user message for caching.");
				println!("Use '/cache stats' to view detailed cache statistics and efficiency.");
				println!("Use '/cache clear' to remove content cache markers (keeps system/tool caches).");
				println!("Use '/cache threshold' to view auto-cache settings.");
				println!("Supports 2-marker system: when you add a 3rd marker, the first one moves to the new position.");
				println!("Automatic caching triggers based on token threshold (configurable).");
				println!(
					"Cached tokens reduce costs on subsequent requests with the same content.\n"
				);

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

				// Add information about command layers
				println!("{}", "** About Command Layers **".bright_yellow());
				println!("Command layers are specialized AI helpers that can be invoked without affecting the session history.");
				println!(
					"Commands are defined in the [[commands]] section of your configuration file."
				);
				println!("Example usage: /run estimate - runs the 'estimate' command layer");
				println!("Command layers use the same infrastructure as normal layers but don't store context.");
				println!("This allows you to get specialized help without cluttering your conversation.\n");

				// Show available commands for current role
				let available_commands = command_executor::list_available_commands(config, role);
				if available_commands.is_empty() {
					println!("{}", "No command layers configured.".bright_blue());
					println!("Use '/run' to see configuration examples.\n");
				} else {
					println!("{}", "Available command layers:".bright_blue());
					for cmd in &available_commands {
						println!("  {} {}", "/run".cyan(), cmd.bright_yellow());
					}
					println!();
				}
			}
			COPY_COMMAND => {
				if self.last_response.is_empty() {
					println!(
						"{}",
						"No response to copy. Send a message first.".bright_yellow()
					);
				} else {
					match Clipboard::new() {
						Ok(mut clipboard) => match clipboard.set_text(&self.last_response) {
							Ok(_) => {
								println!("{}", "Last response copied to clipboard.".bright_green());
							}
							Err(e) => {
								println!("{}: {}", "Failed to copy to clipboard".bright_red(), e);
							}
						},
						Err(e) => {
							println!("{}: {}", "Failed to access clipboard".bright_red(), e);
						}
					}
				}
			}
			CLEAR_COMMAND => {
				// ANSI escape code to clear screen and move cursor to top-left
				print!("\x1B[2J\x1B[1;1H");
				io::stdout().flush()?;
			}
			SAVE_COMMAND => {
				if let Err(e) = self.save() {
					println!("{}: {}", "Failed to save session".bright_red(), e);
				} else {
					println!("{}", "Session saved successfully.".bright_green());
				}
			}
			INFO_COMMAND => {
				self.display_session_info();
			}
			REPORT_COMMAND => {
				// Generate and display session usage report
				if let Some(ref session_file) = self.session.session_file {
					let session_file_str = session_file.to_string_lossy();
					match crate::session::report::SessionReport::generate_from_log(
						&session_file_str,
					) {
						Ok(report) => {
							report.display(config);
						}
						Err(e) => {
							println!("{}: Failed to generate report: {}", "Error".bright_red(), e);
							println!(
								"{}: Make sure the session log file exists and is readable.",
								"Hint".bright_yellow()
							);
						}
					}
				} else {
					println!(
						"{}: No session file available for report generation.",
						"Error".bright_red()
					);
					println!(
						"{}: Save the session first with /save command.",
						"Hint".bright_yellow()
					);
				}
			}
			LAYERS_COMMAND => {
				// Toggle layered processing (RUNTIME ONLY - no config file changes)
				let current_role = role; // Use the passed role parameter

				// Toggle the setting for the appropriate role in the runtime config
				if let Some(role) = config.role_map.get_mut(current_role) {
					role.config.enable_layers = !role.config.enable_layers;
				}

				// Get the current state from the updated config
				let is_enabled = config
					.role_map
					.get(current_role)
					.map(|r| r.config.enable_layers)
					.unwrap_or(false);

				// Log the command execution
				if let Some(session_file) = &self.session.session_file {
					if let Some(session_name) = session_file.file_stem().and_then(|s| s.to_str()) {
						let command_line = "/layers".to_string();
						let _ = crate::session::logger::log_session_command(
							session_name,
							&command_line,
						);
					}
				}

				// Show the new state
				if is_enabled {
					println!(
						"{}",
						"Layered processing architecture is now ENABLED (runtime only)."
							.bright_green()
					);
					println!(
						"{}",
						"Your queries will now be processed through multiple AI models."
							.bright_yellow()
					);
				} else {
					println!(
						"{}",
						"Layered processing architecture is now DISABLED (runtime only)."
							.bright_yellow()
					);
				}
				println!(
					"{}",
					"Note: This change only affects the current session and won't be saved to config."
						.bright_blue()
				);

				// Save the session with updated runtime state
				if let Err(e) = self.save() {
					println!("{} {}", "Warning: Could not save session:".bright_red(), e);
				}

				// Return false since we don't need to reload config (runtime-only change)
				return Ok(false);
			}
			LOGLEVEL_COMMAND => {
				// Handle log level command
				if params.is_empty() {
					// Show current log level - use system-wide getter
					let current_level = config.get_log_level();

					let level_str = match current_level {
						LogLevel::None => "none",
						LogLevel::Info => "info",
						LogLevel::Debug => "debug",
					};
					println!(
						"{}",
						format!("Current log level: {}", level_str).bright_cyan()
					);
					println!("{}", "Available levels: none, info, debug".bright_yellow());
					return Ok(false);
				}

				// Parse the requested log level
				let new_level = match params[0].to_lowercase().as_str() {
					"none" => LogLevel::None,
					"info" => LogLevel::Info,
					"debug" => LogLevel::Debug,
					_ => {
						println!(
							"{}",
							"Invalid log level. Use: none, info, or debug".bright_red()
						);
						return Ok(false);
					}
				};

				// Create a mutable config reference for the update
				let mut temp_config = config.clone();

				// Update the specific field using selective update mechanism
				if let Err(e) = temp_config.update_specific_field(|cfg| {
					// Update the root configuration (takes precedence)
					cfg.log_level = new_level.clone();
				}) {
					println!("{}: {}", "Failed to save configuration".bright_red(), e);
					return Ok(false);
				}

				// Show the new state
				match new_level {
					LogLevel::None => {
						println!("{}", "Log level set to NONE.".bright_yellow());
						println!(
							"{}",
							"Only essential information will be displayed.".bright_blue()
						);
					}
					LogLevel::Info => {
						println!("{}", "Log level set to INFO.".bright_green());
						println!("{}", "Moderate logging will be shown.".bright_yellow());
					}
					LogLevel::Debug => {
						println!("{}", "Log level set to DEBUG.".bright_green());
						println!(
							"{}",
							"Detailed logging will be shown for API calls and tool executions."
								.bright_yellow()
						);
					}
				}
				log_info!("Configuration has been saved to disk.");

				// Return a special code that indicates we should reload the config in the main loop
				return Ok(true);
			}
			DEBUG_COMMAND => {
				// Backward compatibility - toggle between none and debug
				// Create a mutable config reference for the update
				let mut temp_config = config.clone();

				// Update the specific field using selective update mechanism
				if let Err(e) = temp_config.update_specific_field(|cfg| {
					// Toggle between none and debug for backward compatibility
					let current_level = cfg.get_log_level();
					cfg.log_level = match current_level {
						LogLevel::Debug => LogLevel::None,
						_ => LogLevel::Debug,
					};
				}) {
					println!("{}: {}", "Failed to save configuration".bright_red(), e);
					return Ok(false);
				}

				// Show the new state
				if temp_config.log_level.is_debug_enabled() {
					println!("{}", "Debug mode is now ENABLED.".bright_green());
					println!(
						"{}",
						"Detailed logging will be shown for API calls and tool executions."
							.bright_yellow()
					);
				} else {
					println!("{}", "Debug mode is now DISABLED.".bright_yellow());
					println!(
						"{}",
						"Only essential information will be displayed.".bright_blue()
					);
				}
				log_info!("Configuration has been saved to disk.");

				// Return a special code that indicates we should reload the config in the main loop
				return Ok(true);
			}
			TRUNCATE_COMMAND => {
				// Perform smart truncation processing once
				println!("{}", "Performing smart context truncation...".bright_cyan());

				// Estimate current token usage
				let current_tokens =
					crate::session::estimate_message_tokens(&self.session.messages);
				println!(
					"{}",
					format!(
						"Current context size: {} tokens",
						format_number(current_tokens as u64)
					)
					.bright_blue()
				);

				// Use the smart truncation logic directly (bypassing auto-truncation checks)
				match crate::session::chat::perform_smart_truncation(self, config, current_tokens)
					.await
				{
					Ok(()) => {
						// Calculate new token count after truncation
						let new_tokens =
							crate::session::estimate_message_tokens(&self.session.messages);
						let tokens_saved = current_tokens.saturating_sub(new_tokens);

						if tokens_saved > 0 {
							println!(
								"{}",
								format!(
									"Smart truncation completed: {} tokens removed, new context size: {} tokens",
									format_number(tokens_saved as u64),
									format_number(new_tokens as u64)
								)
								.bright_green()
							);
						} else {
							println!(
								"{}",
								"No truncation needed - context size is within optimal range"
									.bright_yellow()
							);
						}
					}
					Err(e) => {
						println!("{}: {}", "Smart truncation failed".bright_red(), e);
					}
				}

				return Ok(false);
			}
			SUMMARIZE_COMMAND => {
				// Perform smart full summarization using local processing
				println!(
					"{}",
					"Performing smart conversation summarization...".bright_cyan()
				);

				// Estimate current token usage
				let current_tokens =
					crate::session::estimate_message_tokens(&self.session.messages);
				println!(
					"{}",
					format!(
						"Current context size: {} tokens",
						format_number(current_tokens as u64)
					)
					.bright_blue()
				);

				// Use the smart full summarization logic
				match crate::session::chat::perform_smart_full_summarization(self, config).await {
					Ok(()) => {
						// Calculate new token count after summarization
						let new_tokens =
							crate::session::estimate_message_tokens(&self.session.messages);
						let tokens_saved = current_tokens.saturating_sub(new_tokens);

						println!(
							"{}",
							format!(
								"Smart summarization completed: {} tokens saved, new context size: {} tokens",
								format_number(tokens_saved as u64),
								format_number(new_tokens as u64)
							)
							.bright_green()
						);
					}
					Err(e) => {
						println!("{}: {}", "Smart summarization failed".bright_red(), e);
					}
				}

				return Ok(false);
			}
			CACHE_COMMAND => {
				// Parse cache command arguments for advanced functionality
				if params.is_empty() {
					// Default behavior - set flag to cache the NEXT user message
					let supports_caching =
						crate::session::model_supports_caching(&self.session.info.model);
					if !supports_caching {
						println!("{}", "This model does not support caching.".bright_yellow());
					} else {
						// Set the flag to cache the next user message
						self.cache_next_user_message = true;

						// Log the command execution
						if let Some(session_file) = &self.session.session_file {
							if let Some(session_name) =
								session_file.file_stem().and_then(|s| s.to_str())
							{
								let command_line = "/cache".to_string();
								let _ = crate::session::logger::log_session_command(
									session_name,
									&command_line,
								);
							}
						}

						println!(
							"{}",
							"The next user message will be marked for caching.".bright_green()
						);

						// Show cache statistics
						let cache_manager = crate::session::cache::CacheManager::new();
						let stats = cache_manager
							.get_cache_statistics_with_config(&self.session, Some(config));
						println!("{}", stats.format_for_display());

						// Save the session with updated runtime state
						if let Err(e) = self.save() {
							println!("{} {}", "Warning: Could not save session:".bright_red(), e);
						}
					}
				} else {
					match params[0] {
						"stats" => {
							// Show detailed cache statistics
							let cache_manager = crate::session::cache::CacheManager::new();
							let stats = cache_manager
								.get_cache_statistics_with_config(&self.session, Some(config));
							println!("{}", stats.format_for_display());
						}
						"clear" => {
							// Clear content cache markers (but keep system markers)
							let cache_manager = crate::session::cache::CacheManager::new();
							let cleared =
								cache_manager.clear_content_cache_markers(&mut self.session);

							if cleared > 0 {
								println!(
									"{}",
									format!("Cleared {} content cache markers", cleared)
										.bright_green()
								);
								let _ = self.save();
							} else {
								println!("{}", "No content cache markers to clear".bright_yellow());
							}
						}
						"threshold" => {
							// Show current threshold settings using the system-wide configuration getters
							if config.cache_tokens_threshold > 0 {
								println!(
									"{}",
									format!(
										"Current auto-cache threshold: {} tokens",
										config.cache_tokens_threshold
									)
									.bright_cyan()
								);
								println!("{}", format!("Auto-cache will trigger when non-cached tokens reach {} tokens",
									config.cache_tokens_threshold).bright_blue());
							} else {
								println!(
									"{}",
									"Auto-cache is disabled (threshold set to 0)".bright_yellow()
								);
							}

							// Show time-based threshold
							let timeout_seconds = config.cache_timeout_seconds;
							if timeout_seconds > 0 {
								let timeout_minutes = timeout_seconds / 60;
								println!(
									"{}",
									format!(
										"Time-based auto-cache: {} seconds ({} minutes)",
										timeout_seconds, timeout_minutes
									)
									.bright_green()
								);
								println!("{}", format!("Auto-cache will trigger if {} minutes pass since last checkpoint",
									timeout_minutes).bright_blue());
							} else {
								println!("{}", "Time-based auto-cache is disabled".bright_yellow());
							}
						}
						_ => {
							println!("{}", "Invalid cache command. Usage:".bright_red());
							println!(
								"{}",
								"  /cache - Add cache checkpoint at last user message".cyan()
							);
							println!(
								"{}",
								"  /cache stats - Show detailed cache statistics".cyan()
							);
							println!("{}", "  /cache clear - Clear content cache markers".cyan());
							println!(
								"{}",
								"  /cache threshold - Show auto-cache threshold settings".cyan()
							);
						}
					}
				}
			}
			LIST_COMMAND => {
				// Parse optional page parameter
				let page = if !params.is_empty() {
					match params[0].parse::<usize>() {
						Ok(p) if p > 0 => p,
						_ => {
							println!(
								"{}: Page number must be a positive integer",
								"Error".bright_red()
							);
							return Ok(false);
						}
					}
				} else {
					1 // Default to page 1
				};

				match list_available_sessions() {
					Ok(sessions) => {
						if sessions.is_empty() {
							println!("{}", "No sessions found.".bright_yellow());
						} else {
							// Pagination settings
							const SESSIONS_PER_PAGE: usize = 15;
							let total_sessions = sessions.len();
							let total_pages = total_sessions.div_ceil(SESSIONS_PER_PAGE);

							if page > total_pages {
								println!(
									"{}: Page {} not found. Total pages: {}",
									"Error".bright_red(),
									page,
									total_pages
								);
								return Ok(false);
							}

							// Calculate pagination bounds
							let start_idx = (page - 1) * SESSIONS_PER_PAGE;
							let end_idx =
								std::cmp::min(start_idx + SESSIONS_PER_PAGE, total_sessions);
							let page_sessions = &sessions[start_idx..end_idx];

							// Create markdown table
							let mut markdown_content = String::new();

							// Add header with pagination info
							markdown_content.push_str(&format!(
								"# Available Sessions (Page {} of {})\n\n",
								page, total_pages
							));
							markdown_content.push_str(&format!(
								"Showing {} of {} sessions\n\n",
								page_sessions.len(),
								total_sessions
							));

							// Create table header
							markdown_content
								.push_str("| Name | Created | Model | Tokens | Cost |\n");
							markdown_content
								.push_str("|------|---------|-------|--------|------|\n");

							// Add table rows
							for (name, info) in page_sessions {
								// Format date from timestamp
								let created_time =
									DateTime::<Utc>::from_timestamp(info.created_at as i64, 0)
										.map(|dt| {
											dt.naive_local().format("%Y-%m-%d %H:%M").to_string()
										})
										.unwrap_or_else(|| "Unknown".to_string());

								// Determine if this is the current session
								let is_current = match &self.session.session_file {
									Some(path) => {
										path.file_stem().and_then(|s| s.to_str()).unwrap_or("")
											== name
									}
									None => false,
								};

								let name_display = if is_current {
									format!("**{}** *(current)*", name)
								} else {
									name.clone()
								};

								// Simplify model name - strip provider prefix if present
								let model_parts: Vec<&str> = info.model.split('/').collect();
								let model_name = if model_parts.len() > 1 {
									model_parts[1]
								} else {
									&info.model
								};

								// Calculate total tokens
								let total_tokens =
									info.input_tokens + info.output_tokens + info.cached_tokens;

								markdown_content.push_str(&format!(
									"| {} | {} | {} | {} | ${:.5} |\n",
									name_display,
									created_time,
									model_name,
									format_number(total_tokens),
									info.total_cost
								));
							}

							// Add navigation info
							markdown_content.push_str("\n## Navigation\n\n");
							if total_pages > 1 {
								if page > 1 {
									markdown_content
										.push_str(&format!("- Previous: `/list {}`\n", page - 1));
								}
								if page < total_pages {
									markdown_content
										.push_str(&format!("- Next: `/list {}`\n", page + 1));
								}
								markdown_content.push_str(&format!(
									"- Go to page: `/list <page>` (1-{})\n\n",
									total_pages
								));
							}

							markdown_content.push_str("## Session Management\n\n");
							markdown_content
								.push_str("- Switch to session: `/session <session_name>`\n");
							markdown_content.push_str("- Create new session: `/session`\n");

							// Render using markdown renderer if available
							if config.enable_markdown_rendering {
								use crate::session::chat::markdown::MarkdownRenderer;
								let theme = config.markdown_theme.parse().unwrap_or_default();
								let renderer = MarkdownRenderer::with_theme(theme);
								match renderer.render_and_print(&markdown_content) {
									Ok(_) => {
										// Successfully rendered as markdown
									}
									Err(e) => {
										// Fallback to plain text if markdown rendering fails
										crate::log_debug!(
											"{}: {}",
											"Warning: Markdown rendering failed".yellow(),
											e
										);
										println!("{}", markdown_content);
									}
								}
							} else {
								// Fallback to plain text
								println!("{}", markdown_content);
							}
						}
					}
					Err(e) => {
						println!("{}: {}", "Failed to list sessions".bright_red(), e);
					}
				}
			}
			MODEL_COMMAND => {
				// Handle model command
				if params.is_empty() {
					// Show current model and system default
					println!(
						"{}",
						format!("Current session model: {}", self.model).bright_cyan()
					);

					// Show the system default model
					let system_model = config.get_effective_model();
					println!(
						"{}",
						format!("System default model: {}", system_model).bright_blue()
					);

					println!();
					println!("{}", "Note: Use '/model <model-name>' to change the model for this session only.".bright_yellow());
					println!(
						"{}",
						"Model changes are runtime-only and won't be saved to config."
							.bright_yellow()
					);
					return Ok(false);
				}

				// Change to a new model (runtime only)
				let new_model = params.join(" ");
				let old_model = self.model.clone();

				// Log the command execution
				if let Some(session_file) = &self.session.session_file {
					if let Some(session_name) = session_file.file_stem().and_then(|s| s.to_str()) {
						let command_line = format!("/model {}", new_model);
						let _ = crate::session::logger::log_session_command(
							session_name,
							&command_line,
						);
					}
				}

				// Update session model (runtime only - don't update config)
				self.model = new_model.clone();
				self.session.info.model = new_model.clone();

				println!(
					"{}",
					format!(
						"Model changed from {} to {} (runtime only)",
						old_model, new_model
					)
					.bright_green()
				);
				println!("{}", "Note: This change only affects the current session and won't be saved to config.".bright_yellow());

				// Save the session with the updated model info (but not config)
				if let Err(e) = self.save() {
					println!("{} {}", "Warning: Could not save session:".bright_red(), e);
				}

				return Ok(false);
			}
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

					println!(
						"{}",
						format!("Creating new session: {}", new_session_name).bright_green()
					);

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
						if current_path
							.file_stem()
							.and_then(|s| s.to_str())
							.unwrap_or("") == new_session_name
						{
							println!("{}", "You are already in this session.".blue());
							return Ok(false);
						}
					}

					// Return a signal to the main loop with the session name to switch to
					// We'll use a specific return code that tells the main loop to switch sessions
					self.session.info.name = new_session_name;
					return Ok(true);
				}
			}
			MCP_COMMAND => {
				// Handle /mcp command for showing MCP server status and tools
				// Support subcommands: list, info, full
				let subcommand = if params.is_empty() { "info" } else { params[0] };

				match subcommand {
					"list" => {
						// Very short list - just tool names
						println!();
						println!("{}", "Available Tools".bright_cyan().bold());
						println!("{}", "─".repeat(30).dimmed());

						let mode_config = config.get_merged_config_for_mode(role);
						let available_functions =
							crate::mcp::get_available_functions(&mode_config).await;

						if available_functions.is_empty() {
							println!("{}", "No tools available.".yellow());
						} else {
							// Group tools by server name
							let mut servers: std::collections::HashMap<
								String,
								Vec<&crate::mcp::McpFunction>,
							> = std::collections::HashMap::new();

							for func in &available_functions {
								let server_name =
									get_tool_server_name_async(&func.name, &mode_config).await;
								servers.entry(server_name).or_default().push(func);
							}

							for (server_name, tools) in servers {
								println!();
								println!("  {}", server_name.bright_blue().bold());
								for tool in tools {
									println!("    {}", tool.name.bright_white());
								}
							}
						}

						println!();
						println!("{}", "Use '/mcp info' for descriptions or '/mcp full' for detailed parameters.".dimmed());
					}
					"info" => {
						// Default view - server status + tools with short descriptions
						println!();
						println!("{}", "MCP Server Status".bright_cyan().bold());
						println!("{}", "─".repeat(50).dimmed());

						// Get the merged config for this role
						let mode_config = config.get_merged_config_for_mode(role);

						if mode_config.mcp.servers.is_empty() {
							println!("{}", "No MCP servers configured for this role.".yellow());
							return Ok(false);
						}

						// Show server status
						let server_report = crate::mcp::server::get_server_status_report();

						for server in &mode_config.mcp.servers {
							let (health, restart_info) = match server.server_type {
								crate::config::McpServerType::Developer
								| crate::config::McpServerType::Filesystem
								| crate::config::McpServerType::Agent => {
									// Internal servers are always running
									(
										crate::mcp::process::ServerHealth::Running,
										Default::default(),
									)
								}
								crate::config::McpServerType::External => {
									// External servers - get from status report
									server_report
										.get(&server.name)
										.map(|(h, r)| (*h, r.clone()))
										.unwrap_or((
											crate::mcp::process::ServerHealth::Dead,
											Default::default(),
										))
								}
							};

							let health_display = match health {
								crate::mcp::process::ServerHealth::Running => "✅ Running".green(),
								crate::mcp::process::ServerHealth::Dead => "❌ Dead".red(),
								crate::mcp::process::ServerHealth::Restarting => {
									"🔄 Restarting".yellow()
								}
								crate::mcp::process::ServerHealth::Failed => {
									"💥 Failed".bright_red()
								}
							};

							println!();
							println!("{}: {}", server.name.bright_white().bold(), health_display);
							println!("  Type: {:?}", server.server_type);
							println!("  Mode: {:?}", server.mode);

							if !server.tools.is_empty() {
								println!(
									"  Configured tools: {}",
									server.tools.join(", ").dimmed()
								);
							}

							if restart_info.restart_count > 0 {
								println!("  Restart count: {}", restart_info.restart_count);
								if restart_info.consecutive_failures > 0 {
									println!(
										"  Consecutive failures: {}",
										restart_info.consecutive_failures
									);
								}
							}
						}

						// Show available tools with short descriptions
						println!();
						println!("{}", "Available Tools".bright_cyan().bold());
						println!("{}", "─".repeat(50).dimmed());

						let available_functions =
							crate::mcp::get_available_functions(&mode_config).await;
						if available_functions.is_empty() {
							println!("{}", "No tools available.".yellow());
						} else {
							// Group tools by server name
							let mut servers: std::collections::HashMap<
								String,
								Vec<&crate::mcp::McpFunction>,
							> = std::collections::HashMap::new();

							for func in &available_functions {
								let server_name =
									get_tool_server_name_async(&func.name, &mode_config).await;
								servers.entry(server_name).or_default().push(func);
							}

							for (server_name, tools) in servers {
								println!();
								println!("  {}", server_name.bright_blue().bold());

								for tool in tools {
									// Show name and short description
									let short_desc = if tool.description.chars().count() > 60 {
										let truncated: String =
											tool.description.chars().take(57).collect();
										format!("{}...", truncated)
									} else {
										tool.description.clone()
									};

									if short_desc.is_empty() {
										println!("    {}", tool.name.bright_white());
									} else {
										println!(
											"    {} - {}",
											tool.name.bright_white(),
											short_desc.dimmed()
										);
									}
								}
							}
						}

						println!();
						println!("{}", "Use '/mcp list' for names only or '/mcp full' for detailed parameters.".dimmed());
					}
					"full" => {
						// Full detailed view with parameters
						println!();
						println!(
							"{}",
							"MCP Server Status & Tools (Full Details)"
								.bright_cyan()
								.bold()
						);
						println!("{}", "─".repeat(60).dimmed());

						// Get the merged config for this role
						let mode_config = config.get_merged_config_for_mode(role);

						if mode_config.mcp.servers.is_empty() {
							println!("{}", "No MCP servers configured for this role.".yellow());
							return Ok(false);
						}

						// Show server status
						let server_report = crate::mcp::server::get_server_status_report();

						for server in &mode_config.mcp.servers {
							let (health, restart_info) = match server.server_type {
								crate::config::McpServerType::Developer
								| crate::config::McpServerType::Filesystem
								| crate::config::McpServerType::Agent => {
									// Internal servers are always running
									(
										crate::mcp::process::ServerHealth::Running,
										Default::default(),
									)
								}
								crate::config::McpServerType::External => {
									// External servers - get from status report
									server_report
										.get(&server.name)
										.map(|(h, r)| (*h, r.clone()))
										.unwrap_or((
											crate::mcp::process::ServerHealth::Dead,
											Default::default(),
										))
								}
							};

							let health_display = match health {
								crate::mcp::process::ServerHealth::Running => "✅ Running".green(),
								crate::mcp::process::ServerHealth::Dead => "❌ Dead".red(),
								crate::mcp::process::ServerHealth::Restarting => {
									"🔄 Restarting".yellow()
								}
								crate::mcp::process::ServerHealth::Failed => {
									"💥 Failed".bright_red()
								}
							};

							println!();
							println!("{}: {}", server.name.bright_white().bold(), health_display);
							println!("  Type: {:?}", server.server_type);
							println!("  Mode: {:?}", server.mode);

							if !server.tools.is_empty() {
								println!(
									"  Configured tools: {}",
									server.tools.join(", ").dimmed()
								);
							}

							if restart_info.restart_count > 0 {
								println!("  Restart count: {}", restart_info.restart_count);
								if restart_info.consecutive_failures > 0 {
									println!(
										"  Consecutive failures: {}",
										restart_info.consecutive_failures
									);
								}
							}
						}

						// Show available tools with full details
						println!();
						println!("{}", "Available Tools (Full Details)".bright_cyan().bold());
						println!("{}", "─".repeat(60).dimmed());

						let available_functions =
							crate::mcp::get_available_functions(&mode_config).await;
						if available_functions.is_empty() {
							println!("{}", "No tools available.".yellow());
						} else {
							// Group tools by server name
							let mut servers: std::collections::HashMap<
								String,
								Vec<&crate::mcp::McpFunction>,
							> = std::collections::HashMap::new();

							for func in &available_functions {
								let server_name =
									get_tool_server_name_async(&func.name, &mode_config).await;
								servers.entry(server_name).or_default().push(func);
							}

							for (server_name, tools) in servers {
								println!();
								println!("  {}", server_name.bright_blue().bold());

								for tool in tools {
									// Full detailed view with parameters
									println!("    {}", tool.name.bright_white().bold());

									// Show full description
									if !tool.description.is_empty() {
										println!("      {}", tool.description.dimmed());
									}

									// Show parameters if available
									if let Some(properties) = tool.parameters.get("properties") {
										if let Some(props_obj) = properties.as_object() {
											if !props_obj.is_empty() {
												println!("      {}", "Parameters:".bright_green());

												// Get required parameters
												let required_params: std::collections::HashSet<
													String,
												> = tool
													.parameters
													.get("required")
													.and_then(|r| r.as_array())
													.map(|arr| {
														arr.iter()
															.filter_map(|v| v.as_str())
															.map(|s| s.to_string())
															.collect()
													})
													.unwrap_or_default();

												for (param_name, param_info) in props_obj {
													let is_required =
														required_params.contains(param_name);
													let required_marker = if is_required {
														"*".bright_red()
													} else {
														" ".normal()
													};

													let param_type = param_info
														.get("type")
														.and_then(|t| t.as_str())
														.unwrap_or("any");

													let param_desc = param_info
														.get("description")
														.and_then(|d| d.as_str())
														.unwrap_or("");

													println!(
														"        {}{}: {} {}",
														required_marker,
														param_name.bright_cyan(),
														param_type.yellow(),
														if !param_desc.is_empty() {
															format!("- {}", param_desc).dimmed()
														} else {
															"".normal()
														}
													);

													// Show enum values if available
													if let Some(enum_values) =
														param_info.get("enum")
													{
														if let Some(enum_array) =
															enum_values.as_array()
														{
															let values: Vec<String> = enum_array
																.iter()
																.filter_map(|v| v.as_str())
																.map(|s| s.to_string())
																.collect();
															if !values.is_empty() {
																println!(
																	"          {}: {}",
																	"options".bright_black(),
																	values
																		.join(", ")
																		.bright_black()
																);
															}
														}
													}

													// Show default value if available
													if let Some(default_val) =
														param_info.get("default")
													{
														println!(
															"          {}: {}",
															"default".bright_black(),
															default_val.to_string().bright_black()
														);
													}
												}
											}
										}
									} else if tool.parameters != serde_json::json!({}) {
										// Show raw parameters if not in standard format
										println!(
											"      {}: {}",
											"Schema".bright_green(),
											tool.parameters.to_string().dimmed()
										);
									}

									println!(); // Add spacing between tools
								}
							}
						}

						println!();
						println!("{}", "Legend: ".bright_yellow());
						println!("  {} Required parameter", "*".bright_red());
						println!(
							"  {}",
							"Use '/mcp list' for names only or '/mcp info' for overview.".dimmed()
						);
					}
					"health" => {
						// Health check and restart subcommand
						println!();
						println!("{}", "MCP Server Health Check".bright_cyan().bold());
						println!("{}", "─".repeat(50).dimmed());

						let mode_config = config.get_merged_config_for_mode(role);

						if mode_config.mcp.servers.is_empty() {
							println!("{}", "No MCP servers configured for this role.".yellow());
							return Ok(false);
						}

						// Show current health monitor status
						if crate::mcp::health_monitor::is_health_monitor_running() {
							println!("{}", "🔍 Health monitor: RUNNING".bright_green());
						} else {
							println!("{}", "🔍 Health monitor: STOPPED".bright_red());
						}
						println!();

						// Force a health check on all servers
						println!(
							"{}",
							"Performing health check on all external servers...".bright_blue()
						);

						if let Err(e) =
							crate::mcp::health_monitor::force_health_check(&mode_config).await
						{
							println!("{}: {}", "Health check failed".bright_red(), e);
							return Ok(false);
						}

						// Show updated server status
						let server_report = crate::mcp::server::get_server_status_report();

						for server in &mode_config.mcp.servers {
							if let crate::config::McpServerType::External = server.server_type {
								let (health, restart_info) = server_report
									.get(&server.name)
									.map(|(h, r)| (*h, r.clone()))
									.unwrap_or((
										crate::mcp::process::ServerHealth::Dead,
										Default::default(),
									));

								let health_display = match health {
									crate::mcp::process::ServerHealth::Running => {
										"✅ Running".green()
									}
									crate::mcp::process::ServerHealth::Dead => "❌ Dead".red(),
									crate::mcp::process::ServerHealth::Restarting => {
										"🔄 Restarting".yellow()
									}
									crate::mcp::process::ServerHealth::Failed => {
										"💥 Failed".bright_red()
									}
								};

								println!(
									"{}: {}",
									server.name.bright_white().bold(),
									health_display
								);

								if restart_info.restart_count > 0 {
									println!("  Restart count: {}", restart_info.restart_count);
									if restart_info.consecutive_failures > 0 {
										println!(
											"  Consecutive failures: {}",
											restart_info.consecutive_failures
										);
									}
								}

								// Show last health check time
								if let Some(last_check) = restart_info.last_health_check {
									if let Ok(duration) =
										std::time::SystemTime::now().duration_since(last_check)
									{
										println!("  Last checked: {}s ago", duration.as_secs());
									}
								}
							}
						}

						println!();
						println!("{}", "Health check completed. Dead servers will be automatically restarted by the health monitor.".bright_blue());
					}
					"dump" => {
						// Dump raw tool definitions in JSON format for debugging
						println!();
						println!("{}", "Raw MCP Tool Definitions (JSON)".bright_cyan().bold());
						println!("{}", "─".repeat(50).dimmed());

						let mode_config = config.get_merged_config_for_mode(role);
						let available_functions =
							crate::mcp::get_available_functions(&mode_config).await;

						if available_functions.is_empty() {
							println!("{}", "No tools available.".yellow());
						} else {
							for (i, func) in available_functions.iter().enumerate() {
								println!();
								println!("{}. {}", i + 1, func.name.bright_white().bold());
								println!(
									"{}",
									serde_json::to_string_pretty(&serde_json::json!({
										"name": func.name,
										"description": func.description,
										"parameters": func.parameters
									}))
									.unwrap_or_default()
								);
							}
						}

						println!();
						println!(
							"{}",
							"Use this output to debug tool schema validation issues.".dimmed()
						);
					}
					"validate" => {
						// Validate tool schema definitions
						println!();
						println!("{}", "MCP Tool Schema Validation".bright_cyan().bold());
						println!("{}", "─".repeat(50).dimmed());

						let mode_config = config.get_merged_config_for_mode(role);
						let available_functions =
							crate::mcp::get_available_functions(&mode_config).await;

						if available_functions.is_empty() {
							println!("{}", "No tools available to validate.".yellow());
						} else {
							let mut all_valid = true;

							for (i, func) in available_functions.iter().enumerate() {
								println!();
								println!(
									"{}. Validating {}",
									i + 1,
									func.name.bright_white().bold()
								);

								let mut issues = Vec::new();

								// Check if parameters has "type" field OR "oneOf" field (MCP schema requirement)
								let has_type = func.parameters.get("type").is_some();
								let has_one_of = func.parameters.get("oneOf").is_some();

								if !has_type && !has_one_of {
									issues.push(
										"Missing 'type' or 'oneOf' field in root schema"
											.to_string(),
									);
								}

								// Check if properties exist and have proper type definitions
								if let Some(properties) = func.parameters.get("properties") {
									if let Some(props_obj) = properties.as_object() {
										for (prop_name, prop_def) in props_obj {
											let prop_has_type = prop_def.get("type").is_some();
											let prop_has_one_of = prop_def.get("oneOf").is_some();

											if !prop_has_type && !prop_has_one_of {
												issues.push(format!(
													"Property '{}' missing 'type' or 'oneOf' field",
													prop_name
												));
											}
										}
									}
								} else if has_type {
									// Only require properties if we have a type field (not for oneOf schemas)
									let schema_type =
										func.parameters.get("type").and_then(|t| t.as_str());
									if schema_type == Some("object") {
										issues.push(
											"Object type schema missing 'properties' field"
												.to_string(),
										);
									}
								}

								if issues.is_empty() {
									println!("  {}", "✅ Valid schema".bright_green());
								} else {
									all_valid = false;
									println!("  {}", "❌ Schema issues found:".bright_red());
									for issue in issues {
										println!("    - {}", issue.yellow());
									}
								}
							}

							println!();
							if all_valid {
								println!("{}", "✅ All tool schemas are valid!".bright_green());
							} else {
								println!(
									"{}",
									"❌ Some tool schemas have validation issues.".bright_red()
								);
								println!("{}", "These issues may cause API errors with providers like Anthropic.".yellow());
							}
						}
					}
					_ => {
						// Invalid subcommand
						println!();
						println!("{}", "Invalid MCP subcommand.".bright_red());
						println!();
						println!("{}", "Available subcommands:".bright_cyan());
						println!("  {} - Show tool names only", "/mcp list".cyan());
						println!(
							"  {} - Show server status and tools with descriptions (default)",
							"/mcp info".cyan()
						);
						println!(
							"  {} - Show full details including parameters",
							"/mcp full".cyan()
						);
						println!(
							"  {} - Check server health and attempt restart if needed",
							"/mcp health".cyan()
						);
						println!(
							"  {} - Dump raw tool definitions in JSON format",
							"/mcp dump".cyan()
						);
						println!();
						println!(
							"  {} - Validate tool schema definitions",
							"/mcp validate".cyan()
						);
						println!();
						println!(
							"{}",
							"Usage: /mcp [list|info|full|health|dump|validate]".bright_blue()
						);
					}
				}

				return Ok(false);
			}
			RUN_COMMAND => {
				// Handle /run command for executing command layers
				if params.is_empty() {
					// Show available commands for this role
					let available_commands =
						command_executor::list_available_commands(config, role);
					if available_commands.is_empty() {
						println!("{}", "No command layers configured.".bright_yellow());
						println!("{}", "Command layers can be defined in the global [[commands]] section of your configuration.".bright_blue());
						println!("{}", "Example configuration:".bright_cyan());
						println!(
							"{}",
							r#"[[commands]]
name = "estimate"
model = "openrouter:openai/gpt-4.1-mini"
system_prompt = "You are a project estimation expert. Analyze the work done and provide estimates."
temperature = 0.2
input_mode = "Last"

[commands.mcp]
server_refs = ["developer", "filesystem"]
allowed_tools = []"#
								.bright_white()
						);
					} else {
						println!("{}", "Available command layers:".bright_cyan());
						for cmd in &available_commands {
							println!("  {} {}", "/run".cyan(), cmd.bright_yellow());
						}
						println!();
						println!("{}", "Usage: /run <command_name>".bright_blue());
						println!("{}", "Example: /run estimate".bright_green());
					}
					return Ok(false);
				}

				let command_name = params[0];

				// Check if command exists
				if !command_executor::command_exists(config, role, command_name) {
					let available_commands =
						command_executor::list_available_commands(config, role);
					println!(
						"{} {}",
						"Command not found:".bright_red(),
						command_name.bright_yellow()
					);
					if !available_commands.is_empty() {
						println!("{}", "Available commands:".bright_cyan());
						for cmd in &available_commands {
							println!("  {}", cmd.bright_yellow());
						}
					}
					return Ok(false);
				}

				// Get the input for the command layer
				// For now, we'll use the last user message or the whole session depending on the input_mode
				// We could also allow passing input as additional parameters
				let command_input = if params.len() > 1 {
					// Use the provided input after the command name
					params[1..].join(" ")
				} else {
					// Use the last user message or a default input
					self.session
						.messages
						.iter()
						.filter(|m| m.role == "user")
						.next_back()
						.map(|m| m.content.clone())
						.unwrap_or_else(|| "No recent user input found".to_string())
				};

				// Execute the command layer
				println!();
				let operation_cancelled =
					std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
				match command_executor::execute_command_layer(
					command_name,
					&command_input,
					self,
					config,
					role,
					operation_cancelled,
				)
				.await
				{
					Ok(result) => {
						println!();
						println!("{}", "Command result:".bright_green());
						// Use markdown-aware printing for command results
						print_assistant_response(&result, config, role);
						println!();
					}
					Err(e) => {
						println!("{} {}", "Command execution failed:".bright_red(), e);
					}
				}

				return Ok(false);
			}
			IMAGE_COMMAND => {
				// Handle /image command for attaching images
				if params.is_empty() {
					println!("{}", "Usage: /image <path_to_image_or_url>".bright_yellow());
					println!("{}", "Examples:".bright_blue());
					println!("{}", "  /image screenshot.png".bright_white());
					println!("{}", "  /image /path/to/image.jpg".bright_white());
					println!(
						"{}",
						"  /image https://example.com/image.png".bright_white()
					);
					println!(
						"{}",
						"Supported formats: PNG, JPEG, GIF, WebP, BMP".bright_blue()
					);

					// Check if current model supports vision
					let (provider, model_name) =
						match crate::session::providers::ProviderFactory::get_provider_for_model(
							&self.model,
						) {
							Ok((provider, model)) => (provider, model),
							Err(_) => {
								println!(
									"{}",
									"Unable to check vision support for current model".bright_red()
								);
								return Ok(false);
							}
						};

					if provider.supports_vision(&model_name) {
						println!("{}", "✅ Current model supports vision".bright_green());
					} else {
						println!(
							"{}",
							"⚠️  Current model does not support vision".bright_yellow()
						);
					}

					// Check clipboard for images
					if let Ok(true) = self.try_attach_from_clipboard().await {
						// Image was found and attached from clipboard
						return Ok(false);
					} else {
						println!(
							"{}",
							"💡 Tip: Copy an image to clipboard and run /image to auto-attach it"
								.bright_blue()
						);
					}

					return Ok(false);
				}

				let image_path = params.join(" ");
				match self.attach_image_from_path(&image_path).await {
					Ok(_) => {
						println!("{}", "✅ Image attached successfully!".bright_green());
						println!(
							"{}",
							"Your next message will include this image.".bright_cyan()
						);
					}
					Err(e) => {
						println!("{}: {}", "❌ Failed to attach image".bright_red(), e);
					}
				}
				return Ok(false);
			}
			_ => return Ok(false), // Not a command
		}

		Ok(false) // Continue session
	}
}
