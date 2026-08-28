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

	#[test]
	fn rolling_content_markers_preserve_previous_and_advance_to_current() {
		let manager = CacheManager::new();
		let mut session = Session::new(
			"cache-roll".to_string(),
			"anthropic:claude-sonnet-4-6".to_string(),
		);
		session.messages = vec![
			Message::default(),
			Message {
				role: "user".to_string(),
				content: "old boundary".to_string(),
				cached: true,
				..Default::default()
			},
			Message {
				role: "user".to_string(),
				content: "previous boundary".to_string(),
				cached: true,
				..Default::default()
			},
			Message {
				role: "assistant".to_string(),
				content: "work".to_string(),
				..Default::default()
			},
			Message {
				role: "user".to_string(),
				content: "current boundary".to_string(),
				..Default::default()
			},
		];

		assert!(manager
			.apply_cache_to_message(&mut session, 4, true)
			.unwrap());
		let markers: Vec<usize> = session
			.messages
			.iter()
			.enumerate()
			.filter(|(_, message)| message.cached && message.role != "system")
			.map(|(index, _)| index)
			.collect();

		assert_eq!(markers, vec![2, 4]);
		assert!(!session.messages[1].cached, "oldest boundary must advance");
	}

	// ── CacheMarker / CacheMarkerType ───────────────────────────────────────────

	#[test]
	fn cache_manager_default_matches_new() {
		assert_eq!(
			CacheManager::default().max_content_markers,
			CacheManager::new().max_content_markers
		);
	}

	#[test]
	fn cache_marker_type_equality_and_distinction() {
		assert_eq!(CacheMarkerType::System, CacheMarkerType::System);
		assert_ne!(CacheMarkerType::System, CacheMarkerType::Tools);
		assert_ne!(CacheMarkerType::Tools, CacheMarkerType::Content);
		assert_ne!(CacheMarkerType::System, CacheMarkerType::Content);
		assert!(format!("{:?}", CacheMarkerType::Content).contains("Content"));
	}

	#[test]
	fn cache_marker_serde_roundtrip_preserves_all_fields() {
		let marker = CacheMarker {
			message_index: 7,
			marker_type: CacheMarkerType::Content,
			automatic: false,
			timestamp: 1_700_000_000,
		};
		let json = serde_json::to_string(&marker).expect("serialize marker");
		let back: CacheMarker = serde_json::from_str(&json).expect("deserialize marker");
		assert_eq!(back.message_index, 7);
		assert_eq!(back.marker_type, CacheMarkerType::Content);
		assert!(!back.automatic);
		assert_eq!(back.timestamp, 1_700_000_000);
	}

	#[test]
	fn cache_marker_type_serde_uses_variant_names() {
		assert_eq!(
			serde_json::to_string(&CacheMarkerType::System).unwrap(),
			"\"System\""
		);
		assert_eq!(
			serde_json::to_string(&CacheMarkerType::Tools).unwrap(),
			"\"Tools\""
		);
		assert_eq!(
			serde_json::to_string(&CacheMarkerType::Content).unwrap(),
			"\"Content\""
		);
		let parsed: CacheMarkerType = serde_json::from_str("\"Tools\"").unwrap();
		assert_eq!(parsed, CacheMarkerType::Tools);
	}

	// ── Helpers ──────────────────────────────────────────────────────────────────

	fn msg(role: &str, content: &str, cached: bool) -> Message {
		Message {
			role: role.to_string(),
			content: content.to_string(),
			cached,
			..Default::default()
		}
	}

	fn test_session(model: &str) -> Session {
		Session::new("cache-tests".to_string(), model.to_string())
	}

	fn test_config() -> Config {
		toml::from_str(include_str!("../../config-templates/default.toml"))
			.expect("parse default config template")
	}

	// ── add_automatic_cache_markers ──────────────────────────────────────────────

	#[test]
	fn automatic_markers_require_caching_support() {
		let manager = CacheManager::new();
		let mut messages = vec![msg("system", "sys prompt", false), msg("user", "hi", false)];
		manager.add_automatic_cache_markers(&mut messages, true, false);
		assert!(
			!messages[0].cached,
			"no markers when the provider cannot cache"
		);
		assert!(!messages[1].cached);
	}

	#[test]
	fn automatic_markers_on_empty_messages_is_noop() {
		let manager = CacheManager::new();
		let mut messages: Vec<Message> = Vec::new();
		manager.add_automatic_cache_markers(&mut messages, true, true);
		assert!(messages.is_empty());
	}

	#[test]
	fn automatic_markers_cache_first_and_last_system_message_with_tools() {
		let manager = CacheManager::new();
		let mut messages = vec![
			msg("system", "base prompt", false),
			msg("user", "hi", false),
			msg("system", "prompt + tool definitions", false),
		];

		// Without tools only the first system message is cached.
		manager.add_automatic_cache_markers(&mut messages, false, true);
		assert!(messages[0].cached);
		assert!(
			!messages[2].cached,
			"last system only cached when tools exist"
		);

		// With tools the LAST system message (tool definitions) is cached too.
		messages[0].cached = false;
		manager.add_automatic_cache_markers(&mut messages, true, true);
		assert!(messages[0].cached);
		assert!(
			messages[2].cached,
			"last system message must be cached with tools"
		);
		assert!(!messages[1].cached, "user content is never auto-cached");
	}

	#[test]
	fn automatic_markers_skip_non_system_first_message() {
		let manager = CacheManager::new();
		let mut messages = vec![
			msg("user", "hi", false),
			msg("system", "late system", false),
		];
		manager.add_automatic_cache_markers(&mut messages, true, true);
		assert!(
			!messages[0].cached,
			"first-message rule only applies to the system role"
		);
		assert!(
			messages[1].cached,
			"last system message still cached with tools"
		);
	}

	#[test]
	fn automatic_markers_are_idempotent_on_cached_system() {
		let manager = CacheManager::new();
		let mut messages = vec![msg("system", "sys", true), msg("user", "hi", false)];
		manager.add_automatic_cache_markers(&mut messages, true, true);
		assert!(messages[0].cached);
		assert!(!messages[1].cached);
	}

	// ── check_and_apply_auto_cache_threshold ─────────────────────────────────────

	#[test]
	fn auto_threshold_noop_when_disabled_or_no_messages() {
		let manager = CacheManager::new();
		let config = test_config();
		let mut session = test_session("anthropic/claude-sonnet-4-6");
		assert!(!manager
			.check_and_apply_auto_cache_threshold(&mut session, &config, false, "developer")
			.unwrap());
		session.messages = vec![msg("user", "hello", false)];
		assert!(!manager
			.check_and_apply_auto_cache_threshold(&mut session, &config, false, "developer")
			.unwrap());
		assert!(!session.messages[0].cached);
	}

	#[test]
	fn auto_threshold_marks_latest_uncached_boundary_then_settles() {
		let manager = CacheManager::new();
		let config = test_config();
		let mut session = test_session("anthropic/claude-sonnet-4-6");
		session.messages = vec![
			msg("system", "sys", true),
			msg("user", "old", true),
			msg("assistant", "work", false),
			msg("user", "current", false),
		];

		assert!(manager
			.check_and_apply_auto_cache_threshold(&mut session, &config, true, "developer")
			.unwrap());
		assert!(
			session.messages[3].cached,
			"latest uncached user message becomes the boundary"
		);

		// Every user/tool message is now cached: no target left, no-op.
		assert!(!manager
			.check_and_apply_auto_cache_threshold(&mut session, &config, true, "developer")
			.unwrap());
	}

	#[test]
	fn auto_threshold_prefers_the_latest_tool_message() {
		let manager = CacheManager::new();
		let config = test_config();
		let mut session = test_session("anthropic/claude-sonnet-4-6");
		session.messages = vec![
			msg("user", "request", false),
			msg("assistant", "calling", false),
			msg("tool", "tool output", false),
		];
		assert!(manager
			.check_and_apply_auto_cache_threshold(&mut session, &config, true, "developer")
			.unwrap());
		assert!(
			session.messages[2].cached,
			"walk-back must stop at the latest tool message"
		);
		assert!(!session.messages[0].cached);
	}

	// ── update_token_tracking / estimate_current_session_tokens ──────────────────

	#[test]
	fn update_token_tracking_accumulates_lifetime_and_current_counters() {
		let manager = CacheManager::new();
		let mut session = test_session("anthropic/claude-sonnet-4-6");

		manager.update_token_tracking(&mut session, 100, 40, 500, 20, 5);
		manager.update_token_tracking(&mut session, 50, 10, 250, 0, 3);

		assert_eq!(session.info.input_tokens, 150);
		assert_eq!(session.info.output_tokens, 50);
		assert_eq!(session.info.cache_read_tokens, 750);
		assert_eq!(session.info.cache_write_tokens, 20);
		assert_eq!(session.info.reasoning_tokens, 8);
		// current_total counts cached + non-cached input; non-cached only raw input.
		assert_eq!(session.info.current_total_tokens, 900);
		assert_eq!(session.info.current_non_cached_tokens, 150);
	}

	#[test]
	fn estimate_current_session_tokens_splits_cached_and_uncached() {
		let manager = CacheManager::new();
		let mut session = test_session("anthropic/claude-sonnet-4-6");
		session.messages = vec![
			msg("system", "cached system prompt", true),
			msg("user", "uncached user turn", false),
			msg("assistant", "uncached reply", false),
		];

		let (total, non_cached) = manager.estimate_current_session_tokens(&session);
		let expected_total: u64 = session
			.messages
			.iter()
			.map(|m| crate::session::estimate_message_tokens(m) as u64)
			.sum();
		let expected_uncached: u64 = session
			.messages
			.iter()
			.filter(|m| !m.cached)
			.map(|m| crate::session::estimate_message_tokens(m) as u64)
			.sum();
		assert_eq!(total, expected_total);
		assert_eq!(non_cached, expected_uncached);
		assert!(
			non_cached < total,
			"cached message must be excluded from the non-cached estimate"
		);
	}

	// ── apply_cache_to_message ───────────────────────────────────────────────────

	#[test]
	fn apply_cache_disabled_is_noop() {
		let manager = CacheManager::new();
		let mut session = test_session("anthropic/claude-sonnet-4-6");
		session.messages = vec![msg("user", "hi", false)];
		session.info.current_total_tokens = 42;
		assert!(!manager
			.apply_cache_to_message(&mut session, 0, false)
			.unwrap());
		assert!(!session.messages[0].cached);
		assert_eq!(
			session.info.current_total_tokens, 42,
			"disabled path must not touch counters"
		);
	}

	#[test]
	fn apply_cache_out_of_bounds_is_error() {
		let manager = CacheManager::new();
		let mut session = test_session("anthropic/claude-sonnet-4-6");
		session.messages = vec![msg("user", "hi", false)];
		let err = manager
			.apply_cache_to_message(&mut session, 1, true)
			.unwrap_err()
			.to_string();
		assert!(err.contains("out of bounds"), "unexpected error: {err}");
	}

	#[test]
	fn apply_cache_already_cached_returns_false_without_counter_reset() {
		let manager = CacheManager::new();
		let mut session = test_session("anthropic/claude-sonnet-4-6");
		session.messages = vec![msg("user", "hi", true)];
		session.info.current_total_tokens = 42;
		session.info.current_non_cached_tokens = 7;
		assert!(!manager
			.apply_cache_to_message(&mut session, 0, true)
			.unwrap());
		assert_eq!(session.info.current_total_tokens, 42);
		assert_eq!(session.info.current_non_cached_tokens, 7);
	}

	#[test]
	fn apply_cache_marks_message_and_resets_current_counters() {
		let manager = CacheManager::new();
		let mut session = test_session("anthropic/claude-sonnet-4-6");
		session.messages = vec![msg("user", "hi", false)];
		session.info.current_total_tokens = 500;
		session.info.current_non_cached_tokens = 300;
		let before = session.info.last_cache_checkpoint_time;

		assert!(manager
			.apply_cache_to_message(&mut session, 0, true)
			.unwrap());
		assert!(session.messages[0].cached);
		assert_eq!(
			session.info.current_total_tokens, 0,
			"checkpoint must reset the rolling total"
		);
		assert_eq!(
			session.info.current_non_cached_tokens, 0,
			"checkpoint must reset the rolling non-cached total"
		);
		assert!(session.info.last_cache_checkpoint_time >= before);
	}

	#[test]
	fn apply_cache_keeps_existing_markers_below_the_two_marker_limit() {
		let manager = CacheManager::new();
		let mut session = test_session("anthropic/claude-sonnet-4-6");
		session.messages = vec![
			msg("user", "boundary one", true),
			msg("assistant", "work", false),
			msg("user", "boundary two target", false),
		];
		assert!(manager
			.apply_cache_to_message(&mut session, 2, true)
			.unwrap());
		assert!(
			session.messages[0].cached,
			"one existing marker is below the limit and must survive"
		);
		assert!(session.messages[2].cached);
	}

	// ── apply_cache_to_current_user_message ──────────────────────────────────────

	#[test]
	fn apply_cache_to_current_user_targets_the_last_user_message() {
		let manager = CacheManager::new();
		let mut session = test_session("anthropic/claude-sonnet-4-6");
		session.messages = vec![
			msg("user", "first turn", false),
			msg("assistant", "reply", false),
			msg("user", "latest turn", false),
		];
		assert!(manager
			.apply_cache_to_current_user_message(&mut session, true)
			.unwrap());
		assert!(!session.messages[0].cached);
		assert!(
			session.messages[2].cached,
			"the LAST user message is the cacheable boundary"
		);
	}

	#[test]
	fn apply_cache_to_current_user_without_user_message_is_error() {
		let manager = CacheManager::new();
		let mut session = test_session("anthropic/claude-sonnet-4-6");
		session.messages = vec![msg("system", "sys", false), msg("assistant", "hi", false)];
		let err = manager
			.apply_cache_to_current_user_message(&mut session, true)
			.unwrap_err()
			.to_string();
		assert!(err.contains("No user message"), "unexpected error: {err}");
	}

	#[test]
	fn apply_cache_to_current_user_respects_disabled_caching() {
		let manager = CacheManager::new();
		let mut session = test_session("anthropic/claude-sonnet-4-6");
		session.messages = vec![msg("user", "hi", false)];
		assert!(!manager
			.apply_cache_to_current_user_message(&mut session, false)
			.unwrap());
		assert!(!session.messages[0].cached);
	}

	// ── clear_content_cache_markers ──────────────────────────────────────────────

	#[test]
	fn clear_content_markers_clears_content_roles_but_keeps_system() {
		let manager = CacheManager::new();
		let mut session = test_session("anthropic/claude-sonnet-4-6");
		session.messages = vec![
			msg("system", "sys", true),
			msg("user", "u", true),
			msg("assistant", "a", true),
			Message {
				tool_call_id: Some("call-1".into()),
				..msg("tool", "t", true)
			},
			msg("user", "already plain", false),
		];

		let cleared = manager.clear_content_cache_markers(&mut session);
		assert_eq!(
			cleared, 3,
			"user + assistant + tool markers are content markers"
		);
		assert!(session.messages[0].cached, "system marker must survive");
		assert!(!session.messages[1].cached);
		assert!(!session.messages[2].cached);
		assert!(!session.messages[3].cached);
	}

	// ── get_cache_statistics / get_cache_statistics_with_config ──────────────────

	#[test]
	fn statistics_count_markers_by_role() {
		let manager = CacheManager::new();
		let mut session = test_session("anthropic/claude-sonnet-4-6");
		session.messages = vec![
			msg("system", "sys", true),
			msg("user", "u", true),
			msg("assistant", "a", true),
			Message {
				tool_call_id: Some("call-1".into()),
				..msg("tool", "tool result", true)
			},
			// Tool message WITHOUT tool_call_id is a tool definition, not a result.
			// Its presence also keeps the virtual-marker branch inert (tool_markers != 0).
			Message {
				tool_call_id: None,
				..msg("tool", "tool definition", true)
			},
			msg("user", "uncached", false),
		];

		let stats = manager.get_cache_statistics(&session);
		assert_eq!(stats.system_markers, 1);
		assert_eq!(
			stats.content_markers, 3,
			"user + assistant + tool RESULT are content markers"
		);
		assert_eq!(
			stats.tool_markers, 1,
			"tool message without tool_call_id is a tool marker"
		);
	}

	#[test]
	fn statistics_virtual_tool_marker_requires_cached_system_and_tools() {
		let manager = CacheManager::new();
		let config = test_config();
		assert!(
			!config.mcp.servers.is_empty(),
			"default template ships builtin servers"
		);

		// Cached system + caching model + configured servers → virtual tool marker.
		let mut session = test_session("anthropic/claude-sonnet-4-6");
		session.messages = vec![msg("system", "sys", true), msg("user", "u", false)];
		let stats = manager.get_cache_statistics_with_config(&session, Some(&config));
		assert_eq!(
			stats.tool_markers, 1,
			"tool definitions are cached alongside the system message"
		);

		// Without config a fresh cacheable session (no tokens yet) still infers tools.
		let stats = manager.get_cache_statistics(&session);
		assert_eq!(stats.tool_markers, 1);

		// No cached system message → nothing to piggyback tool caching on.
		session.messages[0].cached = false;
		let stats = manager.get_cache_statistics_with_config(&session, Some(&config));
		assert_eq!(stats.tool_markers, 0);
		assert_eq!(stats.system_markers, 0);

		// Config with zero servers → no tool definitions to cache.
		session.messages[0].cached = true;
		let mut empty = test_config();
		empty.mcp.servers.clear();
		let stats = manager.get_cache_statistics_with_config(&session, Some(&empty));
		assert_eq!(
			stats.tool_markers, 0,
			"no servers configured means no tool definitions to cache"
		);
	}

	#[test]
	fn statistics_report_token_totals_and_cache_efficiency() {
		let manager = CacheManager::new();
		let mut session = test_session("anthropic/claude-sonnet-4-6");
		session.info.input_tokens = 100;
		session.info.output_tokens = 60;
		session.info.cache_read_tokens = 300;
		session.info.cache_write_tokens = 25;
		session.info.current_non_cached_tokens = 100;
		session.info.current_total_tokens = 400;

		let stats = manager.get_cache_statistics(&session);
		assert_eq!(stats.total_input_tokens, 400, "input + cache read");
		assert_eq!(stats.total_output_tokens, 60);
		assert_eq!(stats.total_cache_read_tokens, 300);
		assert_eq!(stats.total_cache_write_tokens, 25);
		assert_eq!(stats.current_non_cached_tokens, 100);
		assert_eq!(stats.current_total_tokens, 400);
		assert!(
			(stats.cache_efficiency - 75.0).abs() < 1e-9,
			"300 of 400 input tokens cached = 75%"
		);

		// Zero tokens must not divide by zero.
		let empty = test_session("anthropic/claude-sonnet-4-6");
		let stats = manager.get_cache_statistics(&empty);
		assert_eq!(stats.cache_efficiency, 0.0);
		assert_eq!(stats.total_input_tokens, 0);
	}

	// ── CacheStatistics::format_for_display ──────────────────────────────────────

	#[test]
	fn format_for_display_renders_empty_and_populated_states() {
		let empty = CacheStatistics {
			content_markers: 0,
			system_markers: 0,
			tool_markers: 0,
			total_cache_read_tokens: 0,
			total_cache_write_tokens: 0,
			total_input_tokens: 0,
			total_output_tokens: 0,
			current_non_cached_tokens: 0,
			current_total_tokens: 0,
			cache_efficiency: 0.0,
		};
		let text = empty.format_for_display();
		assert!(text.contains("No active cache markers"));
		assert!(text.contains("No cached tokens recorded yet"));

		let populated = CacheStatistics {
			content_markers: 2,
			system_markers: 1,
			tool_markers: 1,
			total_cache_read_tokens: 300,
			total_cache_write_tokens: 25,
			total_input_tokens: 400,
			total_output_tokens: 60,
			current_non_cached_tokens: 100,
			current_total_tokens: 400,
			cache_efficiency: 75.0,
		};
		let text = populated.format_for_display();
		assert!(text.contains("Active markers:"));
		assert!(text.contains("Overall cache efficiency"));
		assert!(text.contains("Session totals:"));
	}
}
