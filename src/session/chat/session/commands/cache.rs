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
use super::{CommandOutput, CommandResult};
use crate::config::Config;
use anyhow::Result;

pub async fn handle_cache(
	session: &mut ChatSession,
	config: &Config,
	params: &[&str],
) -> Result<CommandResult> {
	// Parse cache command arguments for advanced functionality
	if params.is_empty() {
		// Default behavior - set flag to cache the NEXT user message
		let supports_caching = crate::session::model_supports_caching(&session.session.info.model);
		if !supports_caching {
			return Ok(CommandResult::HandledWithOutput(CommandOutput::Cache {
				cache_command: "check_support".to_string(),
				data: serde_json::json!({
					"supports_caching": false,
					"message": "This model does not support caching."
				}),
			}));
		}

		// Set the flag to cache the next user message
		session.cache_next_user_message = true;

		// Log the command execution
		if let Some(session_file) = &session.session.session_file {
			if let Some(session_name) = session_file.file_stem().and_then(|s| s.to_str()) {
				let command_line = "/cache".to_string();
				let _ = crate::session::logger::log_session_command(session_name, &command_line);
			}
		}

		// Show cache statistics
		let cache_manager = crate::session::cache::CacheManager::new();
		let stats = cache_manager.get_cache_statistics_with_config(&session.session, Some(config));

		// Save the session with updated runtime state
		if let Err(e) = session.save() {
			crate::log_debug!("Warning: Could not save session: {}", e);
		}

		Ok(CommandResult::HandledWithOutput(CommandOutput::Cache {
			cache_command: "cache_next_message".to_string(),
			data: serde_json::json!({
				"cache_next_user_message": true,
				"statistics": {
					"system_markers": stats.system_markers,
					"tool_markers": stats.tool_markers,
					"content_markers": stats.content_markers,
				"total_cache_read_tokens": stats.total_cache_read_tokens,
				"total_cache_write_tokens": stats.total_cache_write_tokens,
					"current_non_cached_tokens": stats.current_non_cached_tokens
				}
			}),
		}))
	} else {
		match params[0] {
			"stats" => {
				// Show detailed cache statistics
				let cache_manager = crate::session::cache::CacheManager::new();
				let stats =
					cache_manager.get_cache_statistics_with_config(&session.session, Some(config));

				Ok(CommandResult::HandledWithOutput(CommandOutput::Cache {
					cache_command: "stats".to_string(),
					data: serde_json::json!({
						"statistics": {
							"system_markers": stats.system_markers,
							"tool_markers": stats.tool_markers,
						"total_cache_read_tokens": stats.total_cache_read_tokens,
						"total_cache_write_tokens": stats.total_cache_write_tokens,
							"current_non_cached_tokens": stats.current_non_cached_tokens
						}
					}),
				}))
			}
			"clear" => {
				// Clear content cache markers (but keep system markers)
				let cache_manager = crate::session::cache::CacheManager::new();
				let cleared = cache_manager.clear_content_cache_markers(&mut session.session);

				if cleared > 0 {
					let _ = session.save();
				}

				Ok(CommandResult::HandledWithOutput(CommandOutput::Cache {
					cache_command: "clear".to_string(),
					data: serde_json::json!({
						"cleared_markers": cleared
					}),
				}))
			}
			"threshold" => Ok(CommandResult::HandledWithOutput(CommandOutput::Cache {
				cache_command: "threshold".to_string(),
				data: serde_json::json!({
					"cache_tokens_threshold": config.cache_tokens_threshold,
					"cache_timeout_seconds": config.cache_timeout_seconds
				}),
			})),
			_ => Ok(CommandResult::HandledWithOutput(CommandOutput::Cache {
				cache_command: "error".to_string(),
				data: serde_json::json!({
					"error": "Invalid cache subcommand",
					"usage": [
						"/cache - Add cache checkpoint at last user message",
						"/cache stats - Show detailed cache statistics",
						"/cache clear - Clear content cache markers",
						"/cache threshold - Show auto-cache threshold settings"
					]
				}),
			})),
		}
	}
}
