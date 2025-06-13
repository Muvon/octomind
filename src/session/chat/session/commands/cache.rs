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

// Cache command handler

use super::super::core::ChatSession;
use crate::config::Config;
use anyhow::Result;
use colored::Colorize;

pub async fn handle_cache(
	session: &mut ChatSession,
	config: &Config,
	params: &[&str],
) -> Result<bool> {
	// Parse cache command arguments for advanced functionality
	if params.is_empty() {
		// Default behavior - set flag to cache the NEXT user message
		let supports_caching = crate::session::model_supports_caching(&session.session.info.model);
		if !supports_caching {
			println!("{}", "This model does not support caching.".bright_yellow());
		} else {
			// Set the flag to cache the next user message
			session.cache_next_user_message = true;

			// Log the command execution
			if let Some(session_file) = &session.session.session_file {
				if let Some(session_name) = session_file.file_stem().and_then(|s| s.to_str()) {
					let command_line = "/cache".to_string();
					let _ =
						crate::session::logger::log_session_command(session_name, &command_line);
				}
			}

			println!(
				"{}",
				"The next user message will be marked for caching.".bright_green()
			);

			// Show cache statistics
			let cache_manager = crate::session::cache::CacheManager::new();
			let stats =
				cache_manager.get_cache_statistics_with_config(&session.session, Some(config));
			println!("{}", stats.format_for_display());

			// Save the session with updated runtime state
			if let Err(e) = session.save() {
				println!("{} {}", "Warning: Could not save session:".bright_red(), e);
			}
		}
	} else {
		match params[0] {
			"stats" => {
				// Show detailed cache statistics
				let cache_manager = crate::session::cache::CacheManager::new();
				let stats =
					cache_manager.get_cache_statistics_with_config(&session.session, Some(config));
				println!("{}", stats.format_for_display());
			}
			"clear" => {
				// Clear content cache markers (but keep system markers)
				let cache_manager = crate::session::cache::CacheManager::new();
				let cleared = cache_manager.clear_content_cache_markers(&mut session.session);

				if cleared > 0 {
					println!(
						"{}",
						format!("Cleared {} content cache markers", cleared).bright_green()
					);
					let _ = session.save();
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
					println!(
						"{}",
						format!(
							"Auto-cache will trigger when non-cached tokens reach {} tokens",
							config.cache_tokens_threshold
						)
						.bright_blue()
					);
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
					println!(
						"{}",
						format!(
							"Auto-cache will trigger if {} minutes pass since last checkpoint",
							timeout_minutes
						)
						.bright_blue()
					);
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
	Ok(false)
}
