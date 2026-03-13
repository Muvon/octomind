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

// System prompt and cache setup utilities

use super::core::ChatSession;
use crate::config::Config;
use crate::session::cache::CacheManager;
use crate::session::create_system_prompt;
use crate::session::helper_functions::process_placeholders_async_with_role;
use crate::session::model_supports_caching;
use crate::{log_debug, log_info};
use anyhow::Result;

// Helper function to setup system prompt and cache
pub async fn setup_system_prompt_and_cache(
	chat_session: &mut ChatSession,
	config_for_role: &Config,
	role: &str,
	is_interactive: bool,
) -> Result<()> {
	// Use thread-local working directory if set (ACP sessions), otherwise fall back to process cwd
	let current_dir = crate::mcp::get_thread_working_directory();

	// Initialize with system prompt if new session
	if chat_session.session.messages.is_empty() {
		// Create system prompt based on role - use merged config for role
		let system_prompt = create_system_prompt(&current_dir, config_for_role, role).await;
		chat_session.add_system_message(&system_prompt)?;

		// CRITICAL FIX: Apply automatic cache markers for system messages AND tool definitions
		// This ensures consistent caching behavior across all supported models
		let supports_caching = model_supports_caching(&chat_session.model);
		let has_tools = !config_for_role.mcp.servers.is_empty();

		if supports_caching {
			let cache_manager = CacheManager::new();
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
				let cache_manager = CacheManager::new();
				cache_manager.add_automatic_cache_markers(
					&mut chat_session.session.messages,
					has_tools,
					supports_caching,
				);
			}
		} else {
			// Non-interactive mode: Add assistant welcome message
			let role_config = config_for_role.get_role_config_struct(role);
			let welcome_message = process_placeholders_async_with_role(
				&role_config.welcome,
				&current_dir,
				Some(role),
			)
			.await;

			chat_session.add_assistant_message(&welcome_message, None, config_for_role, role)?;

			// Apply cache marker to welcome message
			if supports_caching {
				let cache_manager = CacheManager::new();
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
							if instructions_content.trim().is_empty() {
								log_debug!(
									"Skipping empty instructions file {}",
									instructions_filename
								);
							} else {
								let processed_instructions = process_placeholders_async_with_role(
									&instructions_content,
									&current_dir,
									Some(role),
								)
								.await;

								chat_session.add_user_message(&processed_instructions)?;

								if supports_caching {
									let cache_manager = CacheManager::new();
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
						}
						Err(e) => {
							log_debug!("Failed to read {}: {}", instructions_filename, e);
						}
					}
				}
			}
		}
	}

	// Add compression hints to system prompt for resumed sessions
	// This informs AI about compression state to improve reasoning
	if !chat_session.session.messages.is_empty() {
		if let Some(first_msg) = chat_session.session.messages.first_mut() {
			if first_msg.role == "system" {
				crate::session::add_compression_hints_to_prompt(
					&mut first_msg.content,
					&chat_session.session.info.compression_stats,
				);
			}
		}
	}

	Ok(())
}
