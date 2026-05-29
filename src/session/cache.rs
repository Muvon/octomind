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

// Comprehensive caching system for AI providers that support it

use crate::config::Config;
use crate::session::chat::format_number;
use crate::session::{Message, Session};
use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Cache marker types to track different caching strategies
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum CacheMarkerType {
	/// System message cache marker (automatic)
	System,
	/// Tool definitions cache marker (automatic)
	Tools,
	/// User/assistant content cache marker (manual or automatic)
	Content,
}

/// Cache marker to track cached message positions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheMarker {
	/// Index in the messages array
	pub message_index: usize,
	/// Type of cache marker
	pub marker_type: CacheMarkerType,
	/// Whether this was set automatically or manually
	pub automatic: bool,
	/// Timestamp when marker was set
	pub timestamp: u64,
}

/// Comprehensive cache management system
pub struct CacheManager {
	/// Maximum number of content cache markers allowed (implements 2-marker system)
	max_content_markers: usize,
}

impl Default for CacheManager {
	fn default() -> Self {
		Self {
			max_content_markers: 2,
		}
	}
}

impl CacheManager {
	pub fn new() -> Self {
		Self::default()
	}

	/// Add automatic cache markers for system messages and tool definitions
	/// This should be called when preparing messages for API requests
	/// CRITICAL FIX: This method should only be called during session initialization,
	/// NOT during every API request conversion
	pub fn add_automatic_cache_markers(
		&self,
		messages: &mut [Message],
		has_tools: bool,
		supports_caching: bool,
	) {
		if !supports_caching {
			return;
		}

		// 1. Cache system message (first message if it's system role)
		if let Some(first_msg) = messages.first_mut() {
			if first_msg.role == "system" && !first_msg.cached {
				first_msg.cached = true;
			}
		}

		// 2. CRITICAL FIX: Tool definition caching should be handled by ensuring
		// the LAST system message (which includes tool definitions) is cached.
		// This happens automatically when system prompt is generated with tools.
		// We don't need to add additional markers here as tool definitions
		// are part of the system message in most cases.

		// Only mark additional system messages if they exist and have tools
		if has_tools {
			// Find the LAST system message - this is where tool definitions are typically included
			let mut last_system_index = None;

			for (i, msg) in messages.iter().enumerate() {
				if msg.role == "system" {
					last_system_index = Some(i);
				}
			}

			// If we found a system message and it's not already cached, cache it
			if let Some(index) = last_system_index {
				if let Some(msg) = messages.get_mut(index) {
					if !msg.cached {
						msg.cached = true;
					}
				}
			}
		}
	}

	/// Move cache marker to the latest tool/user message on every call.
	/// Replaces the previous time/token threshold logic — the provider's
	/// cache TTL governs lifetime; we just keep the marker fresh on every turn.
	/// Returns true if a marker was added/moved.
	pub fn check_and_apply_auto_cache_threshold(
		&self,
		session: &mut Session,
		_config: &Config,
		supports_caching: bool,
		_role: &str,
	) -> Result<bool> {
		if !supports_caching {
			return Ok(false);
		}

		if session.messages.is_empty() {
			return Ok(false);
		}

		// Walk backwards to find the latest tool or user message that is NOT
		// already cached. Skipping already-cached messages ensures the marker
		// always advances to the freshest uncached boundary rather than
		// returning a no-op when the previous turn's target is still marked.
		let target_index = session
			.messages
			.iter()
			.enumerate()
			.rev()
			.find(|(_, msg)| (msg.role == "tool" || msg.role == "user") && !msg.cached)
			.map(|(i, _)| i);

		if let Some(index) = target_index {
			return match self.apply_cache_to_message(session, index, supports_caching) {
				Ok(v) => Ok(v),
				Err(_) => Ok(false),
			};
		}

		Ok(false)
	}

	/// Update token tracking after API response
	/// This should be called after EVERY API request to accumulate token usage
	/// for proper cache threshold calculations
	///
	/// Parameters:
	/// - input_tokens: Non-cached input tokens from API
	/// - output_tokens: Generated completion tokens
	/// - cache_read_tokens: Cached input tokens served from cache
	/// - cache_write_tokens: Cache write tokens (Anthropic-style cache creation)
	/// - reasoning_tokens: Reasoning/thinking tokens
	pub fn update_token_tracking(
		&self,
		session: &mut Session,
		input_tokens: u64,
		output_tokens: u64,
		cache_read_tokens: u64,
		cache_write_tokens: u64,
		reasoning_tokens: u64,
	) {
		// Update session totals (lifetime statistics)
		// Use values directly from API - no calculations needed
		session.info.input_tokens += input_tokens;
		session.info.output_tokens += output_tokens;
		session.info.cache_read_tokens += cache_read_tokens;
		session.info.cache_write_tokens += cache_write_tokens;
		session.info.reasoning_tokens += reasoning_tokens;

		// For threshold checking:
		// - current_total_tokens tracks all input tokens (cached + non-cached)
		// - current_non_cached_tokens tracks only non-cached input tokens
		let total_input = input_tokens + cache_read_tokens;
		session.info.current_total_tokens += total_input;
		session.info.current_non_cached_tokens += input_tokens;
	}

	/// Estimate current session tokens for threshold checking
	/// Uses accurate token counting that includes all message fields
	pub fn estimate_current_session_tokens(&self, session: &Session) -> (u64, u64) {
		let mut total_tokens = 0;
		let mut non_cached_tokens = 0;

		for msg in &session.messages {
			// Use accurate token counting that includes tool_calls, thinking, images, etc.
			let message_tokens = crate::session::estimate_message_tokens(msg) as u64;

			total_tokens += message_tokens;

			// If the message is not cached, count towards non-cached tokens
			if !msg.cached {
				non_cached_tokens += message_tokens;
			}
		}

		(total_tokens, non_cached_tokens)
	}

	/// Get cache statistics for display
	pub fn get_cache_statistics(&self, session: &Session) -> CacheStatistics {
		self.get_cache_statistics_with_config(session, None)
	}

	/// Get cache statistics for display with optional config for tool detection
	pub fn get_cache_statistics_with_config(
		&self,
		session: &Session,
		config: Option<&crate::config::Config>,
	) -> CacheStatistics {
		let mut content_markers = 0;
		let mut system_markers = 0;
		let mut tool_markers = 0;

		for msg in &session.messages {
			if msg.cached {
				match msg.role.as_str() {
					"system" => system_markers += 1,
					"user" => content_markers += 1,
					"tool" => {
						// Only count tool RESULTS as content markers, not tool definitions
						if msg.tool_call_id.is_some() {
							content_markers += 1;
						} else {
							tool_markers += 1; // Tool definitions go to tool markers
						}
					}
					"assistant" => content_markers += 1, // Always count assistant messages as content markers
					_ => {}
				}
			}
		}

		// CRITICAL FIX: Check if tool definitions should be cached based on system message caching
		// Tool definitions are not stored as messages but are cached when system messages are cached
		let has_cached_system = system_markers > 0;
		let supports_caching = crate::session::model_supports_caching(&session.info.model);

		// If system message is cached and model supports caching, tool definitions are also cached
		// This is handled automatically by the providers during API requests
		if has_cached_system && supports_caching {
			// Check if MCP servers are configured (which means tool definitions exist)
			let has_tools = if let Some(cfg) = config {
				!cfg.mcp.servers.is_empty()
			} else {
				// Fallback: infer from session usage or provider behavior
				// If we have tool calls, we definitely have tool definitions
				// If we have any input tokens but no tool calls yet, check if it's a cacheable model with system cached
				session.info.tool_calls > 0 ||
				(session.info.input_tokens > 0 && has_cached_system) ||
				// For brand new sessions with cacheable models and cached system, assume tools are available
				(session.info.input_tokens == 0 && session.info.cache_read_tokens == 0 && has_cached_system)
			};

			if has_tools && tool_markers == 0 {
				// Only add a virtual tool marker if no tool markers exist
				// This prevents artificially inflating the marker count
				tool_markers = 1; // Tool definitions cached (virtual marker)
			}
		}

		CacheStatistics {
			content_markers,
			system_markers,
			tool_markers,
			total_cache_read_tokens: session.info.cache_read_tokens,
			total_cache_write_tokens: session.info.cache_write_tokens,
			total_input_tokens: session.info.input_tokens + session.info.cache_read_tokens,
			total_output_tokens: session.info.output_tokens,
			current_non_cached_tokens: session.info.current_non_cached_tokens,
			current_total_tokens: session.info.current_total_tokens,
			cache_efficiency: if session.info.input_tokens + session.info.cache_read_tokens > 0 {
				// Cache efficiency = percentage of total input tokens that came from cache
				// This shows the overall session cache efficiency (lifetime)
				(session.info.cache_read_tokens as f64
					/ (session.info.input_tokens + session.info.cache_read_tokens) as f64)
					* 100.0
			} else {
				0.0
			},
		}
	}

	/// Clear all content cache markers (but keep system/tool markers)
	pub fn clear_content_cache_markers(&self, session: &mut Session) -> usize {
		let mut cleared = 0;
		for msg in &mut session.messages {
			if msg.cached && (msg.role == "user" || msg.role == "tool" || msg.role == "assistant") {
				// Don't clear system messages
				if msg.role != "system" {
					msg.cached = false;
					cleared += 1;
				}
			}
		}
		cleared
	}

	/// Apply cache marker to a specific message immediately
	/// This is used when /cache command is used or auto-cache threshold is reached
	pub fn apply_cache_to_message(
		&self,
		session: &mut Session,
		message_index: usize,
		supports_caching: bool,
	) -> Result<bool> {
		if !supports_caching {
			return Ok(false);
		}

		// Check if message exists
		if message_index >= session.messages.len() {
			return Err(anyhow::anyhow!(
				"Message index {} is out of bounds",
				message_index
			));
		}

		// Check if already cached
		if let Some(msg) = session.messages.get(message_index) {
			if msg.cached {
				return Ok(false); // Already cached
			}
		}

		// Count existing content cache markers and find first marker to potentially remove
		let mut existing_markers: Vec<usize> = Vec::new();
		let mut first_marker_to_remove: Option<usize> = None;

		for (i, msg) in session.messages.iter().enumerate() {
			if msg.cached && (msg.role == "user" || msg.role == "tool" || msg.role == "assistant") {
				existing_markers.push(i);
			}
		}

		existing_markers.sort();

		// Check if this message is already cached
		if existing_markers.contains(&message_index) {
			return Ok(false); // Already cached
		}

		// Determine if we need to remove a marker due to 2-marker limit
		if existing_markers.len() >= self.max_content_markers {
			first_marker_to_remove = existing_markers.first().copied();
		}

		// Apply changes to the session
		// First remove the old marker if needed
		if let Some(first_marker_index) = first_marker_to_remove {
			if let Some(first_msg) = session.messages.get_mut(first_marker_index) {
				first_msg.cached = false;
			}
		}

		// Then apply the new cache marker
		if let Some(msg) = session.messages.get_mut(message_index) {
			msg.cached = true;

			// Reset token counters when adding a cache checkpoint
			session.info.current_non_cached_tokens = 0;
			session.info.current_total_tokens = 0;
			session.info.last_cache_checkpoint_time = std::time::SystemTime::now()
				.duration_since(std::time::UNIX_EPOCH)
				.unwrap_or_default()
				.as_secs();

			return Ok(true);
		}

		Ok(false)
	}

	/// Apply cache marker to the current user message when /cache command is used
	/// This should be called AFTER the user message is added but BEFORE the API request
	pub fn apply_cache_to_current_user_message(
		&self,
		session: &mut Session,
		supports_caching: bool,
	) -> Result<bool> {
		if !supports_caching {
			return Ok(false);
		}

		// Find the last user message
		for (i, msg) in session.messages.iter().enumerate().rev() {
			if msg.role == "user" {
				return self.apply_cache_to_message(session, i, supports_caching);
			}
		}

		Err(anyhow::anyhow!("No user message found to cache"))
	}

}

/// Cache statistics for display and monitoring
#[derive(Debug, Clone)]
pub struct CacheStatistics {
	pub content_markers: usize,
	pub system_markers: usize,
	pub tool_markers: usize,
	pub total_cache_read_tokens: u64,
	pub total_cache_write_tokens: u64,
	pub total_input_tokens: u64,  // Total input tokens (cacheable)
	pub total_output_tokens: u64, // Total output tokens (not cacheable)
	pub current_non_cached_tokens: u64,
	pub current_total_tokens: u64,
	pub cache_efficiency: f64, // Percentage of INPUT tokens that were cached (read)
}

impl CacheStatistics {
	/// Format statistics for user display
	pub fn format_for_display(&self) -> String {
		use colored::Colorize;

		let mut output = String::new();

		output.push_str(&format!("{}\n", "── Cache Statistics ──".bright_cyan()));

		if self.content_markers > 0 || self.system_markers > 0 || self.tool_markers > 0 {
			output.push_str(&format!(
				"Active markers: {} content, {} system, {} tool\n",
				self.content_markers.to_string().bright_blue(),
				self.system_markers.to_string().bright_green(),
				self.tool_markers.to_string().bright_yellow()
			));
		} else {
			output.push_str(&format!("{}\n", "No active cache markers".bright_black()));
		}

		if self.total_cache_read_tokens > 0 || self.total_cache_write_tokens > 0 {
			output.push_str(&format!(
				"Total input tokens: {} ({} cache read, {} cache write, {} processed)\n",
				format_number(self.total_input_tokens).bright_blue(),
				format_number(self.total_cache_read_tokens).bright_magenta(),
				format_number(self.total_cache_write_tokens).bright_yellow(),
				format_number(self.total_input_tokens - self.total_cache_read_tokens).bright_cyan()
			));
			output.push_str(&format!(
				"Total output tokens: {} (not cacheable)\n",
				format_number(self.total_output_tokens).bright_cyan()
			));
			output.push_str(&format!(
				"Overall cache efficiency: {:.1}% (lifetime session average)\n",
				self.cache_efficiency.to_string().bright_green()
			));
		} else {
			output.push_str(&format!(
				"{}\n",
				"No cached tokens recorded yet".bright_black()
			));
		}

		// Show session-wide cache efficiency in a clearer way
		if self.total_input_tokens > 0 {
			let session_cached_pct =
				(self.total_cache_read_tokens as f64 / self.total_input_tokens as f64) * 100.0;
			let session_processed_pct = 100.0 - session_cached_pct;
			output.push_str(&format!(
				"Session totals: {:.1}% cache read, {:.1}% processed ({}/{} total input tokens)\n",
				session_cached_pct.to_string().bright_green(),
				session_processed_pct.to_string().bright_yellow(),
				format_number(self.total_cache_read_tokens).bright_magenta(),
				format_number(self.total_input_tokens).bright_blue()
			));
		}
		output
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_cache_manager_creation() {
		let manager = CacheManager::new();
		assert_eq!(manager.max_content_markers, 2);
	}

	#[test]
	fn test_automatic_cache_markers() {
		let manager = CacheManager::new();
		let mut messages = vec![
			Message {
				role: "system".to_string(),
				content: "You are an AI assistant".to_string(),
				timestamp: 0,
				cached: false,
				cache_ttl: None,
				tool_call_id: None,
				name: None,
				tool_calls: None,
				images: None,
				videos: None,
				thinking: None,
				id: None,
			},
			Message {
				role: "user".to_string(),
				content: "Hello".to_string(),
				timestamp: 0,
				cached: false,
				cache_ttl: None,
				tool_call_id: None,
				name: None,
				tool_calls: None,
				images: None,
				videos: None,
				thinking: None,
				id: None,
			},
		];

		manager.add_automatic_cache_markers(&mut messages, true, true);

		// System message should be cached
		assert!(messages[0].cached);
		// User message should not be automatically cached
		assert!(!messages[1].cached);
	}
}
