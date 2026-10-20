// Copyright 2026 Muvon Un Limited
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

/// The system prompt for `role` as the current build and config define it.
/// Every path that writes the system message goes through here, so a new
/// session, a resumed one and a `/role` switch get the same text.
pub async fn build_system_prompt(current_dir: &std::path::Path, config_for_role: &Config, role: &str) -> String {
	let mut system_prompt = create_system_prompt(current_dir, config_for_role, role).await;
	// Supervisor: make the agent self-annotate each turn (parsed + stripped later).
	if config_for_role.supervisor.enabled {
		system_prompt.push_str("\n\n");
		system_prompt.push_str(crate::supervisor::detect::SELF_REPORT_INSTRUCTION);
	}
	push_mcp_server_instructions(&mut system_prompt, config_for_role);
	system_prompt
}

/// MCP `initialize` instructions are the server's own guidance on using its
/// tools; MCP clients surface them to the model. Config order keeps the
/// cached prefix stable. Servers not connected yet contribute nothing.
fn push_mcp_server_instructions(system_prompt: &mut String, config: &Config) {
	for server in &config.mcp.servers {
		let name = server.name();
		let Some(instructions) = crate::mcp::process::get_server_instructions(name) else {
			continue;
		};
		let instructions = instructions.trim();
		if instructions.is_empty() {
			continue;
		}
		system_prompt.push_str(&format!(
			"\n\n<mcp_server_instructions server=\"{name}\">\n{instructions}\n</mcp_server_instructions>"
		));
	}
}

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
		// Best-effort clear of any stale spill files left under this session id by
		// a previous run (truncation spill-to-disk lives in the OS temp dir).
		crate::utils::spill::clear_current_session();
		// Create system prompt based on role - use merged config for role
		let system_prompt = build_system_prompt(&current_dir, config_for_role, role).await;
		chat_session.add_system_message(&system_prompt)?;
		chat_session.system_prompt_built = true;

		// CRITICAL FIX: Apply automatic cache markers for system messages AND tool definitions
		// This ensures consistent caching behavior across all supported models
		let supports_caching = model_supports_caching(&chat_session.model);
		let has_tools = !config_for_role.mcp.servers.is_empty();

		if supports_caching {
			let cache_manager = CacheManager::new();
			cache_manager.add_automatic_cache_markers(&mut chat_session.session.messages, has_tools, supports_caching);

			log_info!("System prompt has been automatically marked for caching to save tokens in future interactions.");
			// Save the session to ensure the cached status is persisted
			if let Err(e) = chat_session.save() {
				crate::log_debug!("session save failed: {}", e);
			}
		} else {
			// Don't show warning for models that don't support caching
			log_info!("Note: This model doesn't support caching, but system prompt is still optimized.");
		}

		if is_interactive {
			// Add initial messages (welcome + instructions) using centralized function
			let initial_messages = super::utils::get_initial_messages(config_for_role, role, &current_dir).await?;
			for msg in initial_messages {
				match msg.role.as_str() {
					"assistant" => {
						chat_session.add_assistant_message(&msg.content, None, config_for_role, role)?;
					}
					"user" => {
						chat_session.add_system_managed_user_message(&msg.content)?;
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
			let welcome_message =
				process_placeholders_async_with_role(&role_config.welcome, &current_dir, Some(role)).await;

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

			// Check for AGENTS.md instructions file
			let instructions_path = current_dir.join(crate::session::chat::session::utils::AGENTS_FILE);
			if instructions_path.exists() {
				match std::fs::read_to_string(&instructions_path) {
					Ok(instructions_content) => {
						if instructions_content.trim().is_empty() {
							log_debug!(
								"Skipping empty instructions file {}",
								crate::session::chat::session::utils::AGENTS_FILE
							);
						} else {
							let processed_instructions =
								process_placeholders_async_with_role(&instructions_content, &current_dir, Some(role))
									.await;

							let wrapped = format!("<instructions>\n{}\n</instructions>", processed_instructions);
							chat_session.add_system_managed_user_message(&wrapped)?;

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
								crate::session::chat::session::utils::AGENTS_FILE
							);
						}
					}
					Err(e) => {
						log_debug!(
							"Failed to read {}: {}",
							crate::session::chat::session::utils::AGENTS_FILE,
							e
						);
					}
				}
			}
		}
	}

	// A session loaded from disk carries the system prompt of the build that
	// created it — instructions since removed, a supervisor protocol it no longer
	// parses, an outdated role prompt. Rebuild it once per load; re-binds of the
	// live session keep the built prompt so the cached prefix stays stable.
	if !chat_session.system_prompt_built {
		if let Some(first_msg) = chat_session.session.messages.first_mut() {
			if first_msg.role == "system" {
				let rebuilt = build_system_prompt(&current_dir, config_for_role, role).await;
				if !first_msg.content.starts_with(&rebuilt) {
					log_debug!("Rebuilt stale system prompt of resumed session");
					first_msg.content = rebuilt;
				}
			}
		}
		chat_session.system_prompt_built = true;
	}

	// Add compression hints to system prompt for resumed sessions
	// This informs AI about compression state to improve reasoning
	if let Some(first_msg) = chat_session.session.messages.first_mut() {
		if first_msg.role == "system" {
			crate::session::add_compression_hints_to_prompt(
				&mut first_msg.content,
				&chat_session.session.info.compression_stats,
			);
		}
	}

	Ok(())
}

#[cfg(test)]
#[path = "prompt_setup_tests.rs"]
mod tests;
