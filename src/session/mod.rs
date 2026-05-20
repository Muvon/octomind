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

// Session module for handling interactive coding sessions

pub mod anchor; // Persistent compaction anchor (iterative summarization)
pub mod cache;
pub mod cache_keepalive; // Idle-time prompt cache keepalive pings
pub mod cancellation; // Cancellation management
pub mod chat; // Chat session logic
mod chat_helper; // Chat command completion
pub mod context; // Session-scoped context for multi-session concurrency
pub mod dedup; // Tool result deduplication
pub mod helper_functions; // Helper functions for layers and other components
pub mod history; // Role-based history management
pub mod image; // Image processing and attachment utilities
pub mod layers; // Layered architecture implementation
pub mod logger; // Request/response logging utilities
pub mod modal; // Terminal modal overlay system
mod model_utils; // Model-specific utility functions
pub mod output; // Output abstraction for streaming messages
mod project_context;
pub mod video; // Video processing and attachment utilities // Project context collection and management
			   // Provider abstraction layer moved to src/providers
pub mod background_jobs;
pub mod inbox; // Unified message injection queue for all session sources
pub mod inject_listener; // Unix Domain Socket listener for external message injection
pub mod pipelines; // Deterministic script pipeline system
pub mod report; // Session usage reporting
pub mod share; // /share: upload session JSONL → octomind.run/r/<id>
pub mod smart_summarizer; // Smart text summarization for context management
pub mod tap_runs; // Registry for agents launched via the `tap` core tool
mod token_counter; // Token counting utilities // Comprehensive caching system
pub mod webhook_listener; // HTTP webhook listener for hook-to-inbox injection
pub mod workflows; // Workflow orchestration system // Background job tracking for async agent execution

// Provider system exports
pub use crate::providers::{
	AiProvider, ProviderExchange, ProviderFactory, ProviderResponse, TokenUsage,
};
pub use background_jobs::{BackgroundJobManager, CompletedJob};
pub use cache::{CacheManager, CacheStatistics};
pub use helper_functions::summarize_context;
pub use layers::{InputMode, Layer, LayerConfig, LayerResult};
pub use model_utils::model_supports_caching;
pub use output::{
	detect_output_mode, JsonlSink, OutputMode, OutputSink, SilentSink, WebSocketSink,
};
pub use project_context::ProjectContext;
pub use smart_summarizer::SmartSummarizer;
pub use token_counter::{
	calculate_minimum_session_tokens, estimate_full_context_tokens, estimate_message_tokens,
	estimate_session_tokens, estimate_tokens, truncate_to_tokens, validate_session_token_threshold,
}; // Export token counting functions // Export cache management

// Re-export constants
// Constants moved to config

// System prompts are now fully controlled by configuration files

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Message {
	pub role: String,
	pub content: String,
	pub timestamp: u64,
	#[serde(default = "default_cache_marker")]
	pub cached: bool, // Marks if this message is a cache breakpoint
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub cache_ttl: Option<String>, // Cache TTL override (e.g. "1h") — only Anthropic supports this
	#[serde(skip_serializing_if = "Option::is_none")]
	pub tool_call_id: Option<String>, // For tool messages: the ID of the tool call
	#[serde(skip_serializing_if = "Option::is_none")]
	pub name: Option<String>, // For tool messages: the name of the tool
	#[serde(skip_serializing_if = "Option::is_none")]
	pub tool_calls: Option<serde_json::Value>, // For assistant messages: original tool calls from API response
	#[serde(skip_serializing_if = "Option::is_none")]
	pub images: Option<Vec<crate::session::image::ImageAttachment>>, // For messages with image attachments
	#[serde(skip_serializing_if = "Option::is_none")]
	pub videos: Option<Vec<crate::session::video::VideoAttachment>>, // For messages with video attachments
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub thinking: Option<serde_json::Value>, // For assistant messages: thinking/reasoning content
	#[serde(skip_serializing_if = "Option::is_none")]
	pub id: Option<String>, // Provider's response ID (for assistant messages)
}

fn default_cache_marker() -> bool {
	false
}

fn current_timestamp() -> u64 {
	crate::utils::time::now_secs()
}

impl Default for Message {
	fn default() -> Self {
		Self {
			role: String::new(),
			content: String::new(),
			timestamp: current_timestamp(),
			cached: false,
			cache_ttl: None,
			tool_call_id: None,
			name: None,
			tool_calls: None,
			images: None,
			videos: None,
			thinking: None,
			id: None,
		}
	}
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct SessionInfo {
	pub name: String,
	pub created_at: u64,
	pub model: String,
	pub role: String, // Full role tag (e.g. "developer:general" or "developer")
	pub input_tokens: u64,
	pub output_tokens: u64,
	pub cache_read_tokens: u64,
	pub cache_write_tokens: u64, // Cache write tokens (Anthropic-style cache creation)
	#[serde(default)]
	pub reasoning_tokens: u64, // Tokens used for thinking/reasoning (OpenAI, MiniMax)
	pub total_cost: f64,
	pub duration_seconds: u64,
	pub layer_stats: Vec<LayerStats>, // Added to track per-layer statistics
	#[serde(default)]
	pub tool_calls: u64, // Track total number of tool calls made
	// Time tracking
	#[serde(default)]
	pub total_api_time_ms: u64, // Total time spent on API requests
	#[serde(default)]
	pub total_tool_time_ms: u64, // Total time spent executing tools
	#[serde(default)]
	pub total_layer_time_ms: u64, // Total time spent in layer processing
	// Compression tracking
	#[serde(default)]
	pub compression_stats: CompressionStats,
	// Iterative compaction anchor: structured memory that survives every
	// compaction in this session. Updated by `compress_completed_task` and
	// rendered into compressed-knowledge messages so the model gets stable
	// access to intent, decisions, and file references across compaction
	// cycles. See `src/session/anchor.rs`.
	#[serde(default)]
	pub anchor: crate::session::anchor::Anchor,
	// API call tracking for cache-aware compression
	#[serde(default)]
	pub total_api_calls: usize, // Total API calls made in this session (for cache economics)
	// Cache state tracking (Phase 1: moved from Session to SessionInfo for persistence)
	#[serde(default)]
	pub current_non_cached_tokens: u64,
	#[serde(default)]
	pub current_total_tokens: u64,
	#[serde(default = "current_timestamp")]
	pub last_cache_checkpoint_time: u64,
	// Runtime state tracking (Phase 2: ChatSession runtime state for proper resume)
	#[serde(default)]
	pub cache_next_user_message: bool,
	#[serde(default)]
	pub spending_threshold_checkpoint: f64,
	#[serde(default)]
	pub compression_hint_count: usize,
	#[serde(default)]
	pub last_compression_hint_shown: u64,
	// Conversation compression cooldown tracking (token-based)
	#[serde(default)]
	pub context_tokens_after_last_compression: usize, // 0 = no prior compression, can compress immediately
	// Self-tuning estimation tracking (for accuracy measurement)
	#[serde(default)]
	pub predicted_turns_at_last_compression: f64, // What we predicted at last compression
	#[serde(default)]
	pub api_calls_at_last_compression: usize, // API call count at last compression
	#[serde(default)]
	pub output_tokens_at_last_compression: u64, // Cumulative output tokens at last compression (for incremental growth rate)
	// Exponential compression cooldown: tracks consecutive compressions without a user message.
	// Required growth before re-compression = 10% × 2^consecutive_compressions (capped at 100%).
	// Resets to 0 on every new user message so each turn starts fresh.
	#[serde(default)]
	pub consecutive_compressions: u32,
}

#[derive(Debug, Clone)]
pub enum CompressionKind {
	Task,
	Phase,
	Project,
	Conversation,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CompressionStats {
	pub task_compressions: usize,
	pub phase_compressions: usize,
	pub project_compressions: usize,
	pub conversation_compressions: usize,
	pub total_messages_removed: usize,
	pub total_tokens_saved: u64,
}

impl CompressionStats {
	pub fn add_compression(&mut self, kind: CompressionKind, messages: usize, tokens: u64) {
		match kind {
			CompressionKind::Task => self.task_compressions += 1,
			CompressionKind::Phase => self.phase_compressions += 1,
			CompressionKind::Project => self.project_compressions += 1,
			CompressionKind::Conversation => self.conversation_compressions += 1,
		}
		self.total_messages_removed += messages;
		self.total_tokens_saved += tokens;
	}

	pub fn total_compressions(&self) -> usize {
		self.task_compressions
			+ self.phase_compressions
			+ self.project_compressions
			+ self.conversation_compressions
	}

	pub fn avg_compression_ratio(&self) -> f64 {
		if self.total_compressions() == 0 {
			0.0
		} else {
			self.total_tokens_saved as f64 / (self.total_tokens_saved as f64 + 10000.0)
		}
	}
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct LayerStats {
	pub layer_type: String,
	pub model: String,
	pub input_tokens: u64,
	pub output_tokens: u64,
	pub cost: f64,
	pub timestamp: u64,
	// Time tracking
	#[serde(default)]
	pub api_time_ms: u64, // Time spent on API requests for this layer
	#[serde(default)]
	pub tool_time_ms: u64, // Time spent executing tools for this layer
	#[serde(default)]
	pub total_time_ms: u64, // Total time for this layer processing
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Session {
	pub info: SessionInfo,
	pub messages: Vec<Message>,
	pub session_file: Option<PathBuf>,
}

impl Session {
	// Create a new session
	pub fn new(name: String, model: String) -> Self {
		let timestamp = SystemTime::now()
			.duration_since(UNIX_EPOCH)
			.unwrap_or_default()
			.as_secs();

		Self {
			info: SessionInfo {
				name,
				created_at: timestamp,
				model,
				role: String::new(),
				input_tokens: 0,
				output_tokens: 0,
				cache_read_tokens: 0,
				cache_write_tokens: 0,
				reasoning_tokens: 0,
				total_cost: 0.0,
				duration_seconds: 0,
				layer_stats: Vec::new(), // Initialize empty layer stats
				tool_calls: 0,           // Initialize tool call counter
				// Initialize time tracking fields
				total_api_time_ms: 0,
				total_tool_time_ms: 0,
				total_layer_time_ms: 0,
				compression_stats: CompressionStats::default(),
				anchor: crate::session::anchor::Anchor::default(),
				total_api_calls: 0,
				// Initialize cache state
				current_non_cached_tokens: 0,
				current_total_tokens: 0,
				last_cache_checkpoint_time: timestamp,
				// Initialize runtime state
				cache_next_user_message: false,
				spending_threshold_checkpoint: 0.0,

				compression_hint_count: 0,
				last_compression_hint_shown: 0,
				context_tokens_after_last_compression: 0,
				predicted_turns_at_last_compression: 0.0,
				api_calls_at_last_compression: 0,
				output_tokens_at_last_compression: 0,
				consecutive_compressions: 0,
			},

			messages: Vec::new(),
			session_file: None,
		}
	}

	// Add a message to the session
	pub fn add_message(&mut self, role: &str, content: &str) -> Message {
		let message = Self::build_message(role, content);
		self.messages.push(message.clone());
		message
	}

	// Build a Message without pushing it to the session.
	// Used by atomic-add paths that must persist BEFORE pushing to in-memory Vec.
	pub fn build_message(role: &str, content: &str) -> Message {
		Message {
			role: role.to_string(),
			content: content.to_string(),
			timestamp: SystemTime::now()
				.duration_since(UNIX_EPOCH)
				.unwrap_or_default()
				.as_secs(),
			cached: false,
			..Default::default()
		}
	}

	// Add a cache checkpoint - simplified to only handle system messages automatically
	// Content cache markers should use the CacheManager directly for better control
	pub fn add_cache_checkpoint(&mut self, system: bool) -> Result<bool, anyhow::Error> {
		if system {
			// Find the first system message and mark it
			for msg in self.messages.iter_mut() {
				if msg.role == "system" {
					// Only mark as cached if the model supports it
					msg.cached = crate::session::model_supports_caching(&self.info.model);
					if msg.cached {
						// Reset token counters when adding a cache checkpoint
						self.info.current_non_cached_tokens = 0;
						self.info.current_total_tokens = 0;
						return Ok(true);
					}
					return Ok(false);
				}
			}
			// If we couldn't find a system message, return false
			Ok(false)
		} else {
			// For content cache markers, direct users to use CacheManager
			Err(anyhow::anyhow!(
				"Use CacheManager for content cache markers instead of add_cache_checkpoint"
			))
		}
	}

	// Add statistics for a specific layer
	pub fn add_layer_stats(
		&mut self,
		layer_type: &str,
		model: &str,
		input_tokens: u64,
		output_tokens: u64,
		cost: f64,
	) {
		self.add_layer_stats_with_time(
			layer_type,
			model,
			input_tokens,
			output_tokens,
			cost,
			0,
			0,
			0,
		);
	}

	// Add statistics for a specific layer with time tracking
	#[allow(clippy::too_many_arguments)]
	pub fn add_layer_stats_with_time(
		&mut self,
		layer_type: &str,
		model: &str,
		input_tokens: u64,
		output_tokens: u64,
		cost: f64,
		api_time_ms: u64,
		tool_time_ms: u64,
		total_time_ms: u64,
	) {
		// Create the layer stats entry
		let stats = LayerStats {
			layer_type: layer_type.to_string(),
			model: model.to_string(),
			input_tokens,
			output_tokens,
			cost,
			timestamp: SystemTime::now()
				.duration_since(UNIX_EPOCH)
				.unwrap_or_default()
				.as_secs(),
			api_time_ms,
			tool_time_ms,
			total_time_ms,
		};

		// Add to the session info
		self.info.layer_stats.push(stats);

		// Also update the overall session totals
		self.info.input_tokens += input_tokens;
		self.info.output_tokens += output_tokens;
		self.info.total_cost += cost;

		// Update time tracking totals
		self.info.total_api_time_ms += api_time_ms;
		self.info.total_tool_time_ms += tool_time_ms;
		self.info.total_layer_time_ms += total_time_ms;
	}

	// Save the session to a file - append-only approach
	pub fn save(&self) -> Result<(), anyhow::Error> {
		if let Some(session_file) = &self.session_file {
			// In append-only design, individual messages are already saved when added
			// This method just ensures session metadata is up to date
			// We append an updated SUMMARY entry to reflect current session state
			let summary_entry = serde_json::json!({
				"type": "SUMMARY",
				"timestamp": std::time::SystemTime::now()
					.duration_since(std::time::UNIX_EPOCH)
					.unwrap_or_default()
					.as_secs(),
				"session_info": &self.info
			});
			append_to_session_file(session_file, &serde_json::to_string(&summary_entry)?)?;
			Ok(())
		} else {
			Err(anyhow::anyhow!("No session file specified"))
		}
	}
}

pub mod persistence;
pub use persistence::{
	append_to_session_file, clean_interrupted_tool_calls, extract_runtime_state_from_log,
	find_most_recent_session_for_project, get_sessions_dir, list_available_sessions, load_session,
	SessionRuntimeState,
};
pub mod prompt;
pub use prompt::{add_compression_hints_to_prompt, create_system_prompt};
pub mod completion;
pub use completion::{
	chat_completion_with_provider, chat_completion_with_validation, ChatCompletionProviderParams,
	ChatCompletionWithValidationParams,
};

#[cfg(test)]
mod tests {
	use super::*;
	use crate::session::persistence::has_incomplete_tool_calls;
	use serde_json::json;

	fn create_test_message(
		role: &str,
		content: &str,
		tool_calls: Option<serde_json::Value>,
		tool_call_id: Option<String>,
	) -> Message {
		Message {
			role: role.to_string(),
			content: content.to_string(),
			timestamp: 1234567890,
			cached: false,
			cache_ttl: None,
			tool_call_id,
			name: None,
			tool_calls,
			images: None,
			videos: None,
			thinking: None,
			id: None,
		}
	}
	#[test]
	fn test_has_incomplete_tool_calls_complete_sequence() {
		// Test complete tool call sequence: assistant -> tool_calls -> tool_response
		let messages = vec![
			create_test_message("user", "List files", None, None),
			create_test_message(
				"assistant",
				"I'll list the files for you.",
				Some(
					json!([{"id": "call_123", "name": "list_files", "arguments": {"directory": "."}}]),
				),
				None,
			),
			create_test_message(
				"tool",
				"file1.txt\nfile2.txt",
				None,
				Some("call_123".to_string()),
			),
			create_test_message(
				"assistant",
				"Here are the files in the directory.",
				None,
				None,
			),
		];

		// This should NOT be considered incomplete
		assert!(!has_incomplete_tool_calls(&messages));
	}

	#[test]
	fn test_has_incomplete_tool_calls_incomplete_sequence() {
		// Test incomplete tool call sequence: assistant -> tool_calls -> [missing tool response]
		let messages = vec![
			create_test_message("user", "List files", None, None),
			create_test_message(
				"assistant",
				"I'll list the files for you.",
				Some(
					json!([{"id": "call_123", "name": "list_files", "arguments": {"directory": "."}}]),
				),
				None,
			),
			// Missing tool response - this should be detected as incomplete
		];

		// This SHOULD be considered incomplete
		assert!(has_incomplete_tool_calls(&messages));
	}

	#[test]
	fn test_has_incomplete_tool_calls_multiple_calls_partial() {
		// Test multiple tool calls where some have responses and some don't
		let messages = vec![
			create_test_message("user", "Do multiple things", None, None),
			create_test_message(
				"assistant",
				"I'll do multiple things.",
				Some(json!([
					{"id": "call_123", "name": "list_files", "arguments": {"directory": "."}},
					{"id": "call_456", "name": "shell", "arguments": {"command": "pwd"}}
				])),
				None,
			),
			create_test_message(
				"tool",
				"file1.txt\nfile2.txt",
				None,
				Some("call_123".to_string()),
			),
			// Missing response for call_456 - this should be detected as incomplete
		];

		// This SHOULD be considered incomplete (call_456 has no response)
		assert!(has_incomplete_tool_calls(&messages));
	}

	#[test]
	fn test_has_incomplete_tool_calls_no_tool_calls() {
		// Test messages with no tool calls
		let messages = vec![
			create_test_message("user", "Hello", None, None),
			create_test_message("assistant", "Hello! How can I help you?", None, None),
		];

		// This should NOT be considered incomplete
		assert!(!has_incomplete_tool_calls(&messages));
	}

	#[test]
	fn test_clean_interrupted_tool_calls_preserves_complete() {
		// Test that complete sequences are preserved
		let mut messages = vec![
			create_test_message("user", "List files", None, None),
			create_test_message(
				"assistant",
				"I'll list the files for you.",
				Some(
					json!([{"id": "call_123", "name": "list_files", "arguments": {"directory": "."}}]),
				),
				None,
			),
			create_test_message(
				"tool",
				"file1.txt\nfile2.txt",
				None,
				Some("call_123".to_string()),
			),
			create_test_message("assistant", "Here are the files.", None, None),
		];

		let original_count = messages.len();
		let cleaned = clean_interrupted_tool_calls(&mut messages, "Test");

		// Should not clean anything (complete sequence)
		assert!(!cleaned);
		assert_eq!(messages.len(), original_count);
	}

	#[test]
	fn test_clean_interrupted_tool_calls_inserts_synthetic_result() {
		// Test that incomplete sequences get a synthetic tool result instead of truncation
		let mut messages = vec![
			create_test_message("user", "List files", None, None),
			create_test_message(
				"assistant",
				"I'll list the files for you.",
				Some(
					json!([{"id": "call_123", "function": {"name": "list_files", "arguments": "{\"directory\": \".\"}"}}]),
				),
				None,
			),
			// Missing tool response - a synthetic result should be inserted
		];

		let cleaned = clean_interrupted_tool_calls(&mut messages, "Test");

		// Should insert a synthetic tool result, preserving all messages
		assert!(cleaned);
		assert_eq!(messages.len(), 3); // user + assistant + synthetic tool result
		assert_eq!(messages[0].role, "user");
		assert_eq!(messages[1].role, "assistant");
		assert_eq!(messages[2].role, "tool");
		assert_eq!(messages[2].tool_call_id.as_deref(), Some("call_123"));
		assert!(messages[2].content.contains("interrupted"));
	}

	#[test]
	fn test_session_loading_preserves_stats_from_summary() {
		// Test that SUMMARY is the source of truth and old STATS don't overwrite it
		use std::io::Write;
		use tempfile::NamedTempFile;

		// Create a temporary session file
		let mut temp_file = NamedTempFile::new().expect("Failed to create temp file");

		// Write initial SUMMARY with some stats
		writeln!(
			temp_file,
			"{}",
			serde_json::to_string(&json!({
				"type": "SUMMARY",
				"timestamp": 1000,
				"session_info": {
					"name": "test-session",
					"created_at": 1000,
					"model": "openrouter:anthropic/claude-sonnet-4",
					"role": "developer",
					"provider": "openrouter",
					"input_tokens": 100,
					"output_tokens": 50,
					"cache_read_tokens": 20,
					"cache_write_tokens": 5,
					"total_cost": 0.001,
					"duration_seconds": 10,
					"layer_stats": [
						{
							"layer_type": "main",
							"model": "openrouter:anthropic/claude-sonnet-4",
							"input_tokens": 100,
							"output_tokens": 50,
							"cost": 0.001,
							"timestamp": 1000,
							"api_time_ms": 500,
							"tool_time_ms": 100,
							"total_time_ms": 600
						}
					],
					"tool_calls": 5,
					"total_api_time_ms": 500,
					"total_tool_time_ms": 100,
					"total_layer_time_ms": 600,
					"compression_stats": {
						"task_compressions": 0,
						"phase_compressions": 0,
						"project_compressions": 0,
						"conversation_compressions": 0,
						"total_messages_removed": 0,
						"total_tokens_saved": 0
					},

					"total_api_calls": 1,
					"current_non_cached_tokens": 0,
					"current_total_tokens": 0,
					"last_cache_checkpoint_time": 1000,
					"cache_next_user_message": false,
					"spending_threshold_checkpoint": 0.0,

					"compression_hint_count": 0,
					"last_compression_hint_shown": 0
				}
			}))
			.unwrap()
		)
		.expect("Failed to write SUMMARY");

		// Write some STATS entries with OLDER timestamps (should be ignored)
		writeln!(
			temp_file,
			"{}",
			serde_json::to_string(&json!({
				"type": "STATS",
				"timestamp": 900, // OLDER than SUMMARY
				"total_cost": 0.0,
				"input_tokens": 0,
				"output_tokens": 0,
				"cache_read_tokens": 0,
				"cache_write_tokens": 0,
				"tool_calls": 0,
				"total_api_time_ms": 0,
				"total_tool_time_ms": 0,
				"total_layer_time_ms": 0,
				"model": "openrouter:anthropic/claude-sonnet-4",
				"provider": "openrouter"
			}))
			.unwrap()
		)
		.expect("Failed to write old STATS");

		// Write a user message
		writeln!(
			temp_file,
			"{}",
			serde_json::to_string(&json!({
				"role": "user",
				"content": "Hello",
				"timestamp": 1100,
				"cached": false
			}))
			.unwrap()
		)
		.expect("Failed to write message");

		// Write final SUMMARY with updated stats (should be used)
		writeln!(
			temp_file,
			"{}",
			serde_json::to_string(&json!({
				"type": "SUMMARY",
				"timestamp": 2000, // NEWER timestamp
				"session_info": {
					"name": "test-session",
					"created_at": 1000,
					"model": "openrouter:anthropic/claude-sonnet-4",
					"role": "developer",
					"provider": "openrouter",
					"input_tokens": 200, // Updated values
					"output_tokens": 100,
					"cache_read_tokens": 40,
					"cache_write_tokens": 10,
					"total_cost": 0.002,
					"duration_seconds": 20,
					"layer_stats": [
						{
							"layer_type": "main",
							"model": "openrouter:anthropic/claude-sonnet-4",
							"input_tokens": 200,
							"output_tokens": 100,
							"cost": 0.002,
							"timestamp": 2000,
							"api_time_ms": 1000,
							"tool_time_ms": 200,
							"total_time_ms": 1200
						}
					],
					"tool_calls": 10,
					"total_api_time_ms": 1000,
					"total_tool_time_ms": 200,
					"total_layer_time_ms": 1200,
					"compression_stats": {
						"task_compressions": 0,
						"phase_compressions": 0,
						"project_compressions": 0,
						"conversation_compressions": 0,
						"total_messages_removed": 0,
						"total_tokens_saved": 0
					},

					"total_api_calls": 2,
					"current_non_cached_tokens": 0,
					"current_total_tokens": 0,
					"last_cache_checkpoint_time": 2000,
					"cache_next_user_message": false,
					"spending_threshold_checkpoint": 0.0,

					"compression_hint_count": 0,
					"last_compression_hint_shown": 0
				}
			}))
			.unwrap()
		)
		.expect("Failed to write final SUMMARY");

		temp_file.flush().expect("Failed to flush temp file");

		// Load the session
		let session =
			load_session(&temp_file.path().to_path_buf()).expect("Failed to load session");

		// Verify that the FINAL SUMMARY values are used, not the old STATS
		assert_eq!(
			session.info.input_tokens, 200,
			"Input tokens should be from final SUMMARY"
		);
		assert_eq!(
			session.info.output_tokens, 100,
			"Output tokens should be from final SUMMARY"
		);
		assert_eq!(
			session.info.cache_read_tokens, 40,
			"Cache read tokens should be from final SUMMARY"
		);
		assert_eq!(
			session.info.total_cost, 0.002,
			"Total cost should be from final SUMMARY"
		);
		assert_eq!(
			session.info.tool_calls, 10,
			"Tool calls should be from final SUMMARY"
		);
		assert_eq!(
			session.info.total_api_time_ms, 1000,
			"API time should be from final SUMMARY"
		);
		assert_eq!(
			session.info.total_tool_time_ms, 200,
			"Tool time should be from final SUMMARY"
		);
		assert_eq!(
			session.info.total_layer_time_ms, 1200,
			"Layer time should be from final SUMMARY"
		);

		// CRITICAL: Verify layer_stats are preserved
		assert_eq!(
			session.info.layer_stats.len(),
			1,
			"Layer stats should be preserved"
		);
		assert_eq!(
			session.info.layer_stats[0].input_tokens, 200,
			"Layer stats should match final SUMMARY"
		);
		assert_eq!(
			session.info.layer_stats[0].output_tokens, 100,
			"Layer stats should match final SUMMARY"
		);
		assert_eq!(
			session.info.layer_stats[0].cost, 0.002,
			"Layer stats cost should match final SUMMARY"
		);

		// Verify messages are loaded
		assert_eq!(session.messages.len(), 1, "Should have 1 message");
		assert_eq!(
			session.messages[0].role, "user",
			"Message should be user message"
		);

		// Verify model is preserved from SUMMARY
		assert_eq!(
			session.info.model, "openrouter:anthropic/claude-sonnet-4",
			"Model should be from SUMMARY"
		);
	}

	#[test]
	fn test_session_loading_restores_model_from_command() {
		// Test that model changes via /model command are properly restored
		use std::io::Write;
		use tempfile::NamedTempFile;

		let mut temp_file = NamedTempFile::new().expect("Failed to create temp file");

		// Write initial SUMMARY with original model
		writeln!(
			temp_file,
			"{}",
			serde_json::to_string(&json!({
				"type": "SUMMARY",
				"timestamp": 1000,
				"session_info": {
					"name": "test-session",
					"created_at": 1000,
					"model": "openrouter:anthropic/claude-sonnet-4",
					"role": "developer",
					"provider": "openrouter",
					"input_tokens": 100,
					"output_tokens": 50,
					"cache_read_tokens": 20,
					"cache_write_tokens": 5,
					"total_cost": 0.001,
					"duration_seconds": 10,
					"layer_stats": [],
					"tool_calls": 5,
					"total_api_time_ms": 500,
					"total_tool_time_ms": 100,
					"total_layer_time_ms": 600,
					"compression_stats": {
						"task_compressions": 0,
						"phase_compressions": 0,
						"project_compressions": 0,
						"conversation_compressions": 0,
						"total_messages_removed": 0,
						"total_tokens_saved": 0
					},
					"total_api_calls": 1,
					"current_non_cached_tokens": 0,
					"current_total_tokens": 0,
					"last_cache_checkpoint_time": 1000,
					"cache_next_user_message": false,
					"spending_threshold_checkpoint": 0.0,

					"compression_hint_count": 0,
					"last_compression_hint_shown": 0
				}
			}))
			.unwrap()
		)
		.expect("Failed to write SUMMARY");

		// Write a /model command that changes the model
		writeln!(
			temp_file,
			"{}",
			serde_json::to_string(&json!({
				"type": "COMMAND",
				"timestamp": 1500,
				"command": "/model openrouter:openai/gpt-4o"
			}))
			.unwrap()
		)
		.expect("Failed to write COMMAND");

		// Write a user message
		writeln!(
			temp_file,
			"{}",
			serde_json::to_string(&json!({
				"role": "user",
				"content": "Hello with new model",
				"timestamp": 1600,
				"cached": false
			}))
			.unwrap()
		)
		.expect("Failed to write message");

		// Write final SUMMARY with the changed model
		writeln!(
			temp_file,
			"{}",
			serde_json::to_string(&json!({
				"type": "SUMMARY",
				"timestamp": 2000,
				"session_info": {
					"name": "test-session",
					"created_at": 1000,
					"model": "openrouter:openai/gpt-4o",
					"role": "developer",
					"provider": "openrouter",
					"input_tokens": 200,
					"output_tokens": 100,
					"cache_read_tokens": 40,
					"cache_write_tokens": 10,
					"total_cost": 0.002,
					"duration_seconds": 20,
					"layer_stats": [],
					"tool_calls": 10,
					"total_api_time_ms": 1000,
					"total_tool_time_ms": 200,
					"total_layer_time_ms": 1200,
					"compression_stats": {
						"task_compressions": 0,
						"phase_compressions": 0,
						"project_compressions": 0,
						"conversation_compressions": 0,
						"total_messages_removed": 0,
						"total_tokens_saved": 0
					},
					"total_api_calls": 2,
					"current_non_cached_tokens": 0,
					"current_total_tokens": 0,
					"last_cache_checkpoint_time": 2000,
					"cache_next_user_message": false,
					"spending_threshold_checkpoint": 0.0,

					"compression_hint_count": 0,
					"last_compression_hint_shown": 0
				}
			}))
			.unwrap()
		)
		.expect("Failed to write final SUMMARY");

		temp_file.flush().expect("Failed to flush temp file");

		// Load the session
		let session =
			load_session(&temp_file.path().to_path_buf()).expect("Failed to load session");

		// Verify that the changed model is restored
		// The /model command should be detected and applied
		assert_eq!(
			session.info.model, "openrouter:openai/gpt-4o",
			"Model should be restored from /model command and final SUMMARY"
		);

		// Verify stats are also correct
		assert_eq!(session.info.input_tokens, 200);
		assert_eq!(session.info.total_cost, 0.002);
	}

	#[test]
	fn test_session_loading_model_without_command() {
		// Test that model is restored from SUMMARY when no /model command was used
		use std::io::Write;
		use tempfile::NamedTempFile;

		let mut temp_file = NamedTempFile::new().expect("Failed to create temp file");

		// Write SUMMARY with a specific model (no /model command in session)
		writeln!(
			temp_file,
			"{}",
			serde_json::to_string(&json!({
				"type": "SUMMARY",
				"timestamp": 1000,
				"session_info": {
					"name": "test-session",
					"created_at": 1000,
					"model": "openrouter:google/gemini-2.0-flash-exp:free",
					"role": "developer",
					"provider": "openrouter",
					"input_tokens": 100,
					"output_tokens": 50,
					"cache_read_tokens": 20,
					"cache_write_tokens": 5,
					"total_cost": 0.001,
					"duration_seconds": 10,
					"layer_stats": [],
					"tool_calls": 5,
					"total_api_time_ms": 500,
					"total_tool_time_ms": 100,
					"total_layer_time_ms": 600,
					"compression_stats": {
						"task_compressions": 0,
						"phase_compressions": 0,
						"project_compressions": 0,
						"conversation_compressions": 0,
						"total_messages_removed": 0,
						"total_tokens_saved": 0
					},
					"total_api_calls": 1,
					"current_non_cached_tokens": 0,
					"current_total_tokens": 0,
					"last_cache_checkpoint_time": 1000,
					"cache_next_user_message": false,
					"spending_threshold_checkpoint": 0.0,

					"compression_hint_count": 0,
					"last_compression_hint_shown": 0
				}
			}))
			.unwrap()
		)
		.expect("Failed to write SUMMARY");

		// Write a user message (no /model command)
		writeln!(
			temp_file,
			"{}",
			serde_json::to_string(&json!({
				"role": "user",
				"content": "Hello",
				"timestamp": 1100,
				"cached": false
			}))
			.unwrap()
		)
		.expect("Failed to write message");

		temp_file.flush().expect("Failed to flush temp file");

		// Load the session
		let session =
			load_session(&temp_file.path().to_path_buf()).expect("Failed to load session");

		// Verify that the model from SUMMARY is preserved
		assert_eq!(
			session.info.model, "openrouter:google/gemini-2.0-flash-exp:free",
			"Model should be restored from SUMMARY when no /model command exists"
		);
	}
}
