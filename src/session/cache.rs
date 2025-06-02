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

// Comprehensive caching system for AI providers that support it

use serde::{Serialize, Deserialize};
use crate::session::{Message, Session};
use crate::config::Config;
use anyhow::Result;

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

		// 2. If we have tools, mark the last tool-related message for caching
		// This effectively caches all tool definitions when sent to the API
		if has_tools {
			// Find the last message before the first user message or the last system message
			// This is typically where tool definitions would be in the context
			let mut tool_cache_index = None;

			for (i, msg) in messages.iter().enumerate() {
				if msg.role == "system" {
					tool_cache_index = Some(i);
				} else if msg.role == "user" {
					// First user message found, stop looking
					break;
				}
			}

			if let Some(index) = tool_cache_index {
				if let Some(msg) = messages.get_mut(index) {
					msg.cached = true;
				}
			}
		}
	}

	/// Manage user content cache markers using 2-marker system
	/// Returns true if a marker was added/moved, false otherwise
	pub fn manage_content_cache_markers(
		&self,
		session: &mut Session,
		target_message_index: Option<usize>,
		_automatic: bool,
	) -> Result<bool> {
		let target_index = match target_message_index {
			Some(idx) => idx,
			None => {
				// Find the last user or tool message
				session
					.messages
					.iter()
					.enumerate()
					.rev()
					.find(|(_, msg)| msg.role == "user" || msg.role == "tool")
					.map(|(i, _)| i)
					.ok_or_else(|| anyhow::anyhow!("No user or tool messages found for caching"))?
			}
		};

		// Check if message exists and is eligible for caching
		let msg = session
			.messages
			.get(target_index)
			.ok_or_else(|| anyhow::anyhow!("Message index {} not found", target_index))?;

		if msg.role != "user" && msg.role != "tool" {
			return Err(anyhow::anyhow!(
			"Only user and tool messages can be marked for content caching"
		));
		}

		// Count existing content cache markers
		let mut existing_markers: Vec<usize> = session
			.messages
			.iter()
			.enumerate()
			.filter_map(|(i, msg)| {
				if msg.cached && (msg.role == "user" || msg.role == "tool") {
					Some(i)
				} else {
					None
				}
			})
			.collect();

		existing_markers.sort();

		// Check if this message is already cached
		if existing_markers.contains(&target_index) {
			return Ok(false); // Already cached
		}

		// Implement 2-marker system logic
		match existing_markers.len().cmp(&self.max_content_markers) {
			std::cmp::Ordering::Less => {
				// We have space for another marker
				if let Some(target_msg) = session.messages.get_mut(target_index) {
					target_msg.cached = true;
					return Ok(true);
				}
			}
			std::cmp::Ordering::Equal => {
				// We're at capacity, move the first marker to the new position
				if let Some(first_marker_index) = existing_markers.first() {
					// Remove cache from first marker
					if let Some(first_msg) = session.messages.get_mut(*first_marker_index) {
						first_msg.cached = false;
					}
					// Add cache to new position
					if let Some(target_msg) = session.messages.get_mut(target_index) {
						target_msg.cached = true;
						return Ok(true);
					}
				}
			}
			std::cmp::Ordering::Greater => {
				// This shouldn't happen in normal usage but handle gracefully
				// Remove excess markers starting from the first
				while existing_markers.len() > self.max_content_markers {
					if let Some(first_marker_index) = existing_markers.first() {
						if let Some(first_msg) = session.messages.get_mut(*first_marker_index) {
							first_msg.cached = false;
						}
						existing_markers.remove(0);
					}
				}
				// Now add the new marker
				if let Some(target_msg) = session.messages.get_mut(target_index) {
					target_msg.cached = true;
					return Ok(true);
				}
			}
		}

		Ok(false)
	}

	/// Check if auto-cache threshold is reached and add marker if needed
	/// Returns true if a cache marker was added
	pub fn check_and_apply_auto_cache_threshold(
		&self,
		session: &mut Session,
		config: &Config,
		supports_caching: bool,
		_role: &str,
	) -> Result<bool> {
		if !supports_caching {
			return Ok(false);
		}

		// If there are no messages, nothing to do
		if session.messages.is_empty() {
			return Ok(false);
		}

		// Check time-based threshold first (3 minutes = 180 seconds by default)
		let current_time = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.unwrap_or_default()
			.as_secs();

		let time_since_last_cache = current_time.saturating_sub(session.last_cache_checkpoint_time);

		if time_since_last_cache >= config.cache_timeout_seconds {
			// Find the LAST tool message, and if none, the LAST user message
			let target_index = session.messages.iter().enumerate().rev()
				.find(|(_, msg)| msg.role == "tool")
				.or_else(|| session.messages.iter().enumerate().rev()
					.find(|(_, msg)| msg.role == "user"))
				.map(|(i, _)| i);

			if let Some(index) = target_index {
				match self.apply_cache_to_message(session, index, supports_caching) {
					Ok(true) => {
						return Ok(true);
					}
					Ok(false) => {
						// Even if we couldn't add a marker, update the time to prevent constant attempts
						session.last_cache_checkpoint_time = current_time;
						return Ok(false);
					}
					Err(_) => {
						// Update time on error too to prevent constant attempts
						session.last_cache_checkpoint_time = current_time;
						return Ok(false); // Silently fail for auto-cache
					}
				}
			}
		}

		// Check absolute threshold (only check if > 0, meaning enabled)
		if config.cache_tokens_threshold > 0 && session.current_non_cached_tokens >= config.cache_tokens_threshold {
			// Find the LAST tool message, and if none, the LAST user message
			let target_index = session.messages.iter().enumerate().rev()
				.find(|(_, msg)| msg.role == "tool")
				.or_else(|| session.messages.iter().enumerate().rev()
					.find(|(_, msg)| msg.role == "user"))
				.map(|(i, _)| i);

			if let Some(index) = target_index {
				match self.apply_cache_to_message(session, index, supports_caching) {
					Ok(true) => return Ok(true),
					Ok(false) => return Ok(false),
					Err(_) => return Ok(false), // Silently fail for auto-cache
				}
			}
		}

		Ok(false)
	}

	/// Check if auto-cache threshold is reached on EACH tool result and add marker if needed
	/// This should be called after EACH individual tool result is processed, not after all tools
	/// Returns true if a cache marker was added
	pub fn check_and_apply_auto_cache_threshold_on_tool_result(
		&self,
		session: &mut Session,
		config: &Config,
		supports_caching: bool,
		tool_message_index: usize,
		_role: &str,
	) -> Result<bool> {
		if !supports_caching {
			return Ok(false);
		}

		// Only check thresholds if we have enough messages and the specified index is valid
		if session.messages.len() <= tool_message_index {
			return Ok(false);
		}

		// Verify the message at the index is indeed a tool message
		if let Some(msg) = session.messages.get(tool_message_index) {
			if msg.role != "tool" {
				return Ok(false);
			}
		} else {
			return Ok(false);
		}

		// Check absolute threshold (only check if > 0, meaning enabled)
		if config.cache_tokens_threshold > 0 && session.current_non_cached_tokens >= config.cache_tokens_threshold {
			match self.apply_cache_to_message(session, tool_message_index, supports_caching) {
				Ok(true) => return Ok(true),
				Ok(false) => return Ok(false),
				Err(_) => return Ok(false), // Silently fail for auto-cache
			}
		}

		// Check time-based threshold last
		let current_time = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.unwrap_or_default()
			.as_secs();

		let time_since_last_cache = current_time.saturating_sub(session.last_cache_checkpoint_time);

		if time_since_last_cache >= config.cache_timeout_seconds {
			match self.apply_cache_to_message(session, tool_message_index, supports_caching) {
				Ok(true) => return Ok(true),
				Ok(false) => {
					// Even if we couldn't add a marker, update the time to prevent constant attempts
					session.last_cache_checkpoint_time = current_time;
					return Ok(false);
				}
				Err(_) => {
					// Update time on error too to prevent constant attempts
					session.last_cache_checkpoint_time = current_time;
					return Ok(false); // Silently fail for auto-cache
				}
			}
		}

		Ok(false)
	}

	/// Update token tracking after API response
	/// This should be called after EVERY API request to accumulate token usage
	/// for proper cache threshold calculations
	pub fn update_token_tracking(
		&self,
		session: &mut Session,
		input_tokens: u64,
		output_tokens: u64,
		cached_tokens: u64,
	) {
		// Update session totals (lifetime statistics)
		session.info.input_tokens += input_tokens;
		session.info.output_tokens += output_tokens;
		session.info.cached_tokens += cached_tokens;

		// Update current interaction tracking for cache threshold logic
		// input_tokens here are already non-cached tokens (caller subtracted cached_tokens)
		// For threshold checking:
		// - total_input_tokens should include ALL INPUT tokens (non-cached + cached) - excludes output
		// - non_cached_input_tokens should only include non-cached input tokens
		// Output tokens are separate as they cannot be cached
		let total_input_tokens = input_tokens + cached_tokens; // Only INPUT tokens (cacheable)
		let non_cached_input_tokens = input_tokens; // Only non-cached input tokens

		// Add to running totals (these accumulate until a cache checkpoint is set)
		session.current_total_tokens += total_input_tokens;
		session.current_non_cached_tokens += non_cached_input_tokens;
	}

	/// Estimate current session tokens for threshold checking
	/// This provides a rough estimate of tokens in the session messages that are not cached
	pub fn estimate_current_session_tokens(&self, session: &Session) -> (u64, u64) {
		let mut total_tokens = 0;
		let mut non_cached_tokens = 0;

		for msg in &session.messages {
			// Estimate tokens for this message (roughly 4 chars per token)
			let message_tokens = (msg.content.len() / 4) as u64 + 4; // +4 for role overhead

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
		let mut content_markers = 0;
		let mut system_markers = 0;
		let mut tool_markers = 0;

		for msg in &session.messages {
			if msg.cached {
				match msg.role.as_str() {
					"system" => system_markers += 1,
					"user" => content_markers += 1,
					"tool" => tool_markers += 1,  // FIXED: Actually count tool markers
					"assistant" => {
						// Assistant messages could be either tool-related or content
						// For now, count as content markers
						content_markers += 1;
					}
					_ => {}
				}
			}
		}

		CacheStatistics {
			content_markers,
			system_markers,
			tool_markers,
			total_cached_tokens: session.info.cached_tokens,
			total_input_tokens: session.info.input_tokens + session.info.cached_tokens,
			total_output_tokens: session.info.output_tokens,
			current_non_cached_tokens: session.current_non_cached_tokens,
			current_total_tokens: session.current_total_tokens,
			cache_efficiency: if session.info.input_tokens + session.info.cached_tokens > 0 {
				// Cache efficiency = percentage of INPUT tokens that came from cache
				// Only input tokens can be cached, so exclude output tokens from this calculation
				(session.info.cached_tokens as f64 / (session.info.input_tokens + session.info.cached_tokens) as f64) * 100.0
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
			return Err(anyhow::anyhow!("Message index {} is out of bounds", message_index));
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
			session.current_non_cached_tokens = 0;
			session.current_total_tokens = 0;
			session.last_cache_checkpoint_time = std::time::SystemTime::now()
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

	/// Apply cache marker to the current tool message when auto-threshold is reached
	/// This should be called immediately when threshold is reached during tool processing
	pub fn apply_cache_to_current_tool_message(
		&self,
		session: &mut Session,
		supports_caching: bool,
	) -> Result<bool> {
		if !supports_caching {
			return Ok(false);
		}

		// Find the last tool message
		for (i, msg) in session.messages.iter().enumerate().rev() {
			if msg.role == "tool" {
				return self.apply_cache_to_message(session, i, supports_caching);
			}
		}

		// If no tool message found, fall back to last user message
		for (i, msg) in session.messages.iter().enumerate().rev() {
			if msg.role == "user" {
				return self.apply_cache_to_message(session, i, supports_caching);
			}
		}

		Err(anyhow::anyhow!("No suitable message found to cache"))
	}

	/// Validate cache configuration for a provider/model
	pub fn validate_cache_support(&self, provider: &str, model: &str) -> bool {
		match provider.to_lowercase().as_str() {
			"openrouter" => {
				(model.contains("anthropic") && model.contains("claude"))
				||
				(model.contains("google") && model.contains("gemini"))
			}
			"anthropic" | "google" => {
				true
			}
			// Do not use our markers, OpenAI uses implicit caching, for example
			_ => false,
		}
	}
}

/// Cache statistics for display and monitoring
#[derive(Debug, Clone)]
pub struct CacheStatistics {
	pub content_markers: usize,
	pub system_markers: usize,
	pub tool_markers: usize,
	pub total_cached_tokens: u64,
	pub total_input_tokens: u64,   // Total input tokens (cacheable)
	pub total_output_tokens: u64,  // Total output tokens (not cacheable)
	pub current_non_cached_tokens: u64,
	pub current_total_tokens: u64,
	pub cache_efficiency: f64, // Percentage of INPUT tokens that were cached
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

		if self.total_cached_tokens > 0 {
			output.push_str(&format!(
				"Total input tokens: {} ({} cached, {} processed)\n",
				self.total_input_tokens.to_string().bright_blue(),
				self.total_cached_tokens.to_string().bright_magenta(),
				(self.total_input_tokens - self.total_cached_tokens).to_string().bright_yellow()
			));
			output.push_str(&format!(
				"Total output tokens: {} (not cacheable)\n",
				self.total_output_tokens.to_string().bright_cyan()
			));
			output.push_str(&format!(
				"Cache efficiency: {:.1}% (of input tokens only)\n",
				self.cache_efficiency.to_string().bright_green()
			));
		} else {
			output.push_str(&format!("{}\n", "No cached tokens recorded yet".bright_black()));
		}

		if self.current_total_tokens > 0 {
			let non_cached_pct = (self.current_non_cached_tokens as f64 / self.current_total_tokens as f64) * 100.0;
			let cached_pct = 100.0 - non_cached_pct;
			output.push_str(&format!(
				"Current session input: {:.1}% cached, {:.1}% processed ({}/{} input tokens)\n",
				cached_pct.to_string().bright_green(),
				non_cached_pct.to_string().bright_yellow(),
				self.current_non_cached_tokens.to_string().bright_red(),
				self.current_total_tokens.to_string().bright_blue()
			));
		}

		output
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::session::{Session, SessionInfo};

	fn create_test_session() -> Session {
		Session {
			info: SessionInfo {
				name: "test".to_string(),
				created_at: 0,
				model: "openrouter:anthropic/claude-3.5-sonnet".to_string(),
				provider: "openrouter".to_string(),
				input_tokens: 0,
				output_tokens: 0,
				cached_tokens: 0,
				total_cost: 0.0,
				duration_seconds: 0,
				layer_stats: Vec::new(),
				tool_calls: 0,
				total_api_time_ms: 0,
				total_layer_time_ms: 0,
				total_tool_time_ms: 0,
			},
			messages: Vec::new(),
			session_file: None,
			current_non_cached_tokens: 0,
			current_total_tokens: 0,
			last_cache_checkpoint_time: 0,
		}
	}

	#[test]
	fn test_cache_manager_creation() {
		let manager = CacheManager::new();
		assert_eq!(manager.max_content_markers, 2);
	}

	#[test]
	fn test_two_marker_system() {
		let manager = CacheManager::new();
		let mut session = create_test_session();

		// Add some messages
		session.add_message("user", "First message");
		session.add_message("assistant", "First response");
		session.add_message("user", "Second message");
		session.add_message("assistant", "Second response");
		session.add_message("user", "Third message");

		// Add first marker
		let result = manager.manage_content_cache_markers(&mut session, Some(0), false);
		assert!(result.is_ok());
		assert!(result.unwrap());
		assert!(session.messages[0].cached);

		// Add second marker
		let result = manager.manage_content_cache_markers(&mut session, Some(2), false);
		assert!(result.is_ok());
		assert!(result.unwrap());
		assert!(session.messages[2].cached);

		// Add third marker - should move first marker
		let result = manager.manage_content_cache_markers(&mut session, Some(4), false);
		assert!(result.is_ok());
		assert!(result.unwrap());
		assert!(!session.messages[0].cached); // First marker removed
		assert!(session.messages[2].cached);  // Second marker remains
		assert!(session.messages[4].cached);  // Third marker added
	}

	#[test]
	fn test_cache_support_validation() {
		let manager = CacheManager::new();

		// Test OpenRouter with Claude
		assert!(manager.validate_cache_support("openrouter", "anthropic/claude-3.5-sonnet"));
		assert!(manager.validate_cache_support("openrouter", "claude-3-opus"));

		// Test OpenRouter with non-Claude
		assert!(!manager.validate_cache_support("openrouter", "openai/gpt-4"));

		// Test direct Anthropic
		assert!(manager.validate_cache_support("anthropic", "claude-3.5-sonnet"));

		// Test unsupported providers
		assert!(!manager.validate_cache_support("openai", "gpt-4"));
		assert!(!manager.validate_cache_support("google", "gemini-pro"));
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
				tool_call_id: None,
				name: None,
				tool_calls: None,
			},
			Message {
				role: "user".to_string(),
				content: "Hello".to_string(),
				timestamp: 0,
				cached: false,
				tool_call_id: None,
				name: None,
				tool_calls: None,
			},
		];

		manager.add_automatic_cache_markers(&mut messages, true, true);

		// System message should be cached
		assert!(messages[0].cached);
		// User message should not be automatically cached
		assert!(!messages[1].cached);
	}
}
