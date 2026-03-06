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

// Session module for handling interactive coding sessions

pub mod cache;
pub mod cancellation; // Cancellation management
pub mod chat; // Chat session logic
mod chat_helper; // Chat command completion
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
pub mod report; // Session usage reporting
pub mod smart_summarizer; // Smart text summarization for context management
mod token_counter; // Token counting utilities // Comprehensive caching system
pub mod workflows; // Workflow orchestration system // Background job tracking for async agent execution

// Provider system exports
pub use crate::providers::{
	AiProvider, ProviderExchange, ProviderFactory, ProviderResponse, TokenUsage,
};
pub use background_jobs::{BackgroundJob, BackgroundJobManager, BackgroundJobStatus};
pub use cache::{CacheManager, CacheStatistics};
pub use helper_functions::summarize_context;
pub use layers::{InputMode, Layer, LayerConfig, LayerMcpConfig, LayerResult};
pub use model_utils::model_supports_caching;
pub use output::{
	detect_output_mode, JsonlSink, OutputMode, OutputSink, SilentSink, WebSocketSink,
};
pub use project_context::ProjectContext;
pub use smart_summarizer::SmartSummarizer;
pub use token_counter::{
	calculate_minimum_session_tokens, estimate_full_context_tokens, estimate_message_tokens,
	estimate_session_tokens, estimate_tokens, validate_session_token_threshold,
}; // Export token counting functions // Export cache management

// Re-export constants
// Constants moved to config

// System prompts are now fully controlled by configuration files

use crate::config::Config;
use crate::providers::ChatCompletionParams;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs::{self as std_fs, File, OpenOptions};
use std::io::Write;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::watch;

/// Parameters for chat completion with validation
///
/// This struct groups all parameters needed for validated chat completion calls,
/// following best practices for parameter passing and future extensibility.
pub struct ChatCompletionWithValidationParams<'a> {
	/// Array of conversation messages
	pub messages: &'a [Message],
	/// Model identifier (e.g., "claude-3-5-sonnet", "gpt-4")
	pub model: &'a str,
	/// Sampling temperature (0.0 to 2.0)
	pub temperature: f32,
	/// Top-p nucleus sampling (0.0 to 1.0)
	pub top_p: f32,
	/// Top-k sampling (1 to infinity)
	pub top_k: u32,
	/// Maximum tokens to generate (0 = no limit)
	pub max_tokens: u32,
	/// Maximum retry attempts on failure
	pub max_retries: u32,
	/// Configuration object
	pub config: &'a Config,
	/// Optional chat session for context management
	pub chat_session: Option<&'a mut crate::session::chat::session::ChatSession>,
	/// Cancellation token for request abortion
	pub cancellation_token: Option<tokio::sync::watch::Receiver<bool>>,
	/// Optional JSON schema for structured output
	pub schema: Option<serde_json::Value>,
}

impl<'a> ChatCompletionWithValidationParams<'a> {
	/// Create new chat completion with validation parameters
	pub fn new(
		messages: &'a [Message],
		model: &'a str,
		temperature: f32,
		top_p: f32,
		top_k: u32,
		max_tokens: u32,
		config: &'a Config,
	) -> Self {
		Self {
			messages,
			model,
			temperature,
			top_p,
			top_k,
			max_tokens,
			max_retries: 0,
			config,
			chat_session: None,
			cancellation_token: None,
			schema: None,
		}
	}

	/// Set maximum retry attempts
	pub fn with_max_retries(mut self, max_retries: u32) -> Self {
		self.max_retries = max_retries;
		self
	}

	/// Set chat session for context management
	pub fn with_chat_session(
		mut self,
		chat_session: &'a mut crate::session::chat::session::ChatSession,
	) -> Self {
		self.chat_session = Some(chat_session);
		self
	}

	/// Set cancellation token
	pub fn with_cancellation_token(mut self, token: watch::Receiver<bool>) -> Self {
		self.cancellation_token = Some(token);
		self
	}

	/// Set JSON schema for structured output
	pub fn with_schema(mut self, schema: serde_json::Value) -> Self {
		self.schema = Some(schema);
		self
	}
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Message {
	pub role: String,
	pub content: String,
	pub timestamp: u64,
	#[serde(default = "default_cache_marker")]
	pub cached: bool, // Marks if this message is a cache breakpoint
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
	SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.unwrap_or_default()
		.as_secs()
}

impl Default for Message {
	fn default() -> Self {
		Self {
			role: String::new(),
			content: String::new(),
			timestamp: current_timestamp(),
			cached: false,
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
	pub provider: String,
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
	// Conversation compression cooldown tracking
	#[serde(default)]
	pub next_conversation_compression_at_api_call: usize, // 0 = can compress immediately
	// Self-tuning estimation tracking (for accuracy measurement)
	#[serde(default)]
	pub predicted_turns_at_last_compression: f64, // What we predicted at last compression
	#[serde(default)]
	pub api_calls_at_last_compression: usize, // API call count at last compression
	#[serde(default)]
	pub output_tokens_at_last_compression: u64, // Cumulative output tokens at last compression (for incremental growth rate)
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
	pub fn add_task_compression(&mut self, messages: usize, tokens: u64) {
		self.task_compressions += 1;
		self.total_messages_removed += messages;
		self.total_tokens_saved += tokens;
	}

	pub fn add_phase_compression(&mut self, messages: usize, tokens: u64) {
		self.phase_compressions += 1;
		self.total_messages_removed += messages;
		self.total_tokens_saved += tokens;
	}

	pub fn add_project_compression(&mut self, messages: usize, tokens: u64) {
		self.project_compressions += 1;
		self.total_messages_removed += messages;
		self.total_tokens_saved += tokens;
	}

	pub fn add_conversation_compression(&mut self, messages: usize, tokens: u64) {
		self.conversation_compressions += 1;
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
	pub fn new(name: String, model: String, provider: String) -> Self {
		let timestamp = SystemTime::now()
			.duration_since(UNIX_EPOCH)
			.unwrap_or_default()
			.as_secs();

		Self {
			info: SessionInfo {
				name,
				created_at: timestamp,
				model,
				provider,
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
				next_conversation_compression_at_api_call: 0,
				predicted_turns_at_last_compression: 0.0,
				api_calls_at_last_compression: 0,
				output_tokens_at_last_compression: 0,
			},

			messages: Vec::new(),
			session_file: None,
		}
	}

	// Add a message to the session
	pub fn add_message(&mut self, role: &str, content: &str) -> Message {
		let message = Message {
			role: role.to_string(),
			content: content.to_string(),
			timestamp: SystemTime::now()
				.duration_since(UNIX_EPOCH)
				.unwrap_or_default()
				.as_secs(),
			cached: false,
			..Default::default()
		};

		self.messages.push(message.clone());
		message
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

// Get sessions directory path
pub fn get_sessions_dir() -> Result<PathBuf, anyhow::Error> {
	crate::directories::get_sessions_dir()
}

// Get a list of available sessions
pub fn list_available_sessions() -> Result<Vec<(String, SessionInfo)>, anyhow::Error> {
	let sessions_dir = get_sessions_dir()?;
	let mut sessions = Vec::new();

	if !sessions_dir.exists() {
		return Ok(sessions);
	}

	for entry in std_fs::read_dir(sessions_dir)? {
		let entry = entry?;
		let path = entry.path();

		if path.is_file() && path.extension().is_some_and(|ext| ext == "jsonl") {
			// Read just the first line to get session info
			if let Ok(file) = File::open(&path) {
				let reader = BufReader::new(file);
				let first_line = reader.lines().next();

				if let Some(Ok(line)) = first_line {
					// Try new JSON format first
					if let Ok(json_value) = serde_json::from_str::<serde_json::Value>(&line) {
						if let Some(log_type) = json_value.get("type").and_then(|t| t.as_str()) {
							if log_type == "SUMMARY" {
								if let Some(session_info_value) = json_value.get("session_info") {
									if let Ok(info) = serde_json::from_value::<SessionInfo>(
										session_info_value.clone(),
									) {
										let name = path
											.file_stem()
											.and_then(|s| s.to_str())
											.unwrap_or_default()
											.to_string();
										sessions.push((name, info));
									}
								}
							}
						}
					} else if let Some(content) = line.strip_prefix("SUMMARY: ") {
						// Fallback to legacy format
						if let Ok(info) = serde_json::from_str::<SessionInfo>(content) {
							let name = path
								.file_stem()
								.and_then(|s| s.to_str())
								.unwrap_or_default()
								.to_string();
							sessions.push((name, info));
						}
					}
				}
			}
		}
	}

	// Sort sessions by creation time (newest first)
	sessions.sort_by(|a, b| b.1.created_at.cmp(&a.1.created_at));

	Ok(sessions)
}

// Find the most recent session for a specific project directory
// This works by checking the session name which includes the project basename
pub fn find_most_recent_session_for_project(
	project_dir: &Path,
) -> Result<Option<String>, anyhow::Error> {
	let sessions_dir = get_sessions_dir()?;

	if !sessions_dir.exists() {
		return Ok(None);
	}

	// Get the basename of the current project directory
	let project_basename = project_dir
		.file_name()
		.and_then(|n| n.to_str())
		.unwrap_or("");

	if project_basename.is_empty() {
		return Ok(None);
	}

	let mut matching_sessions: Vec<(String, u64)> = Vec::new();

	for entry in std_fs::read_dir(sessions_dir)? {
		let entry = entry?;
		let path = entry.path();

		if path.is_file() && path.extension().is_some_and(|ext| ext == "jsonl") {
			let name = path
				.file_stem()
				.and_then(|s| s.to_str())
				.unwrap_or_default();

			// Session name format: YYMMDD-HHMMSS-basename-uuid
			// Check if the session name contains the project basename
			if name.contains(project_basename) {
				// Get file modification time for sorting
				if let Ok(metadata) = std_fs::metadata(&path) {
					if let Ok(modified) = metadata.modified() {
						if let Ok(duration) =
							modified.duration_since(std::time::SystemTime::UNIX_EPOCH)
						{
							matching_sessions.push((name.to_string(), duration.as_secs()));
						}
					}
				}
			}
		}
	}

	// Sort by modification time (newest first)
	matching_sessions.sort_by(|a, b| b.1.cmp(&a.1));

	// Return the most recent session name
	Ok(matching_sessions.first().map(|(name, _)| name.clone()))
}

/// Check if there are incomplete tool calls that need cleanup
///
/// A tool call sequence is incomplete if:
/// 1. There's an assistant message with tool_calls
/// 2. AND there are tool calls without corresponding tool response messages
///
/// This correctly distinguishes between:
/// - Complete sequences: assistant -> tool_calls -> tool_responses -> (optional final assistant)
/// - Incomplete sequences: assistant -> tool_calls -> [interrupted, no tool responses]
fn has_incomplete_tool_calls(messages: &[Message]) -> bool {
	// Check ALL assistant messages with tool_calls, not just the last one
	for (i, msg) in messages.iter().enumerate() {
		if msg.role == "assistant" && msg.tool_calls.is_some() {
			if let Some(tool_calls_value) = &msg.tool_calls {
				// Parse the tool calls to get their IDs
				if let Ok(tool_calls) =
					serde_json::from_value::<Vec<serde_json::Value>>(tool_calls_value.clone())
				{
					for tool_call in tool_calls {
						if let Some(call_id) = tool_call.get("id").and_then(|id| id.as_str()) {
							// Look for a tool message with this call_id AFTER the assistant message
							let has_response = messages.iter().skip(i + 1).any(|response_msg| {
								response_msg.role == "tool"
									&& response_msg.tool_call_id.as_ref()
										== Some(&call_id.to_string())
							});

							if !has_response {
								return true; // Found a tool call without a response
							}
						}
					}
				}
			}
		}
	}

	false
}

/// Clean up interrupted tool calls - simple chronological truncation approach
///
/// When incomplete tool calls are detected, truncates the message history from the
/// first incomplete assistant message to the end, ensuring a clean conversation state.
pub fn clean_interrupted_tool_calls(
	messages: &mut Vec<Message>,
	session_name: &str,
	context: &str,
) -> bool {
	if messages.is_empty() {
		return false;
	}

	// Find the FIRST incomplete tool call sequence scanning from the BEGINNING
	let mut truncate_from_index = None;

	// Scan forward to find the FIRST assistant message with incomplete tool calls
	for (i, msg) in messages.iter().enumerate() {
		if msg.role == "assistant" && msg.tool_calls.is_some() {
			// Check if this assistant message has incomplete tool calls
			if let Some(tool_calls_value) = &msg.tool_calls {
				if let Ok(tool_calls) =
					serde_json::from_value::<Vec<serde_json::Value>>(tool_calls_value.clone())
				{
					let mut has_incomplete_calls = false;

					for tool_call in tool_calls {
						if let Some(call_id) = tool_call.get("id").and_then(|id| id.as_str()) {
							// Look for tool response AFTER this assistant message
							let has_response = messages.iter().skip(i + 1).any(|response_msg| {
								response_msg.role == "tool"
									&& response_msg.tool_call_id.as_ref()
										== Some(&call_id.to_string())
							});

							if !has_response {
								has_incomplete_calls = true;
								break;
							}
						}
					}

					if has_incomplete_calls {
						truncate_from_index = Some(i);
						break; // Found the FIRST incomplete sequence, truncate from here
					}
				}
			}
		}
	}

	// If we found an incomplete sequence, TRUNCATE from that point
	if let Some(truncate_index) = truncate_from_index {
		let original_len = messages.len();
		messages.truncate(truncate_index);
		let removed_count = original_len - messages.len();

		if removed_count > 0 {
			eprintln!(
				"🔧 {}: Truncated {} messages from incomplete tool sequence",
				context, removed_count
			);

			// Log the cleanup
			let _ = crate::session::logger::log_system_message(
				session_name,
				&format!(
					"{}: Truncated {} messages from incomplete tool sequence",
					context, removed_count
				),
			);

			return true;
		}
	}

	false
}

// Helper function to load a session from file - optimized to use streams
pub fn load_session(session_file: &PathBuf) -> Result<Session, anyhow::Error> {
	// Ensure the file exists
	if !session_file.exists() {
		return Err(anyhow::anyhow!("Session file does not exist"));
	}

	// Open the file
	let file = File::open(session_file)?;
	let reader = BufReader::new(file);
	let mut session_info: Option<SessionInfo> = None;
	let mut last_summary_timestamp: u64 = 0; // Track last SUMMARY timestamp
	let mut messages: Vec<Message> = Vec::new();
	let mut restoration_point_found = false;
	let mut restoration_messages = Vec::new();
	let mut pending_tool_calls = Vec::new(); // Collect tool calls for reconstruction

	// Process the file line by line to avoid loading the entire file into memory
	for line in reader.lines() {
		let line = line?;

		// Try to parse as JSON first (new format)
		if let Ok(json_value) = serde_json::from_str::<serde_json::Value>(&line) {
			if let Some(log_type) = json_value.get("type").and_then(|t| t.as_str()) {
				match log_type {
					"SUMMARY" => {
						// Extract session info from JSON log entry
						// SUMMARY is the source of truth - it contains complete session state
						if let Some(session_info_value) = json_value.get("session_info") {
							session_info =
								Some(serde_json::from_value(session_info_value.clone())?);
							// Track SUMMARY timestamp to ignore older STATS entries
							last_summary_timestamp = json_value
								.get("timestamp")
								.and_then(|t| t.as_u64())
								.unwrap_or(0);
						}
					}
					"RESTORATION_POINT" => {
						// Found a restoration point - this means the session was optimized with /done
						restoration_point_found = true;
						messages.clear();
						restoration_messages.clear();
						pending_tool_calls.clear(); // Clear stale tool calls from before restoration
					}
					"COMPRESSION_POINT" => {
						// Found a compression point - messages before this were compressed
						// Clear messages like RESTORATION_POINT to reflect compressed state
						if restoration_point_found {
							restoration_messages.clear();
						} else {
							messages.clear();
						}
						pending_tool_calls.clear(); // Clear stale tool calls from before compression

						// Log compression restoration for debugging
						if let (Some(comp_type), Some(msgs_removed)) = (
							json_value.get("compression_type").and_then(|t| t.as_str()),
							json_value.get("messages_removed").and_then(|m| m.as_u64()),
						) {
							crate::log_debug!(
								"Session restoration: Found COMPRESSION_POINT ({}, {} messages removed)",
								comp_type,
								msgs_removed
							);
						}
					}
					"TRUNCATION_POINT" => {
						// Found a truncation point - this means messages were removed due to Ctrl+C cleanup
						// Truncate to the specified message count to reflect the cleaned state
						if let Some(message_count) =
							json_value.get("message_count").and_then(|m| m.as_u64())
						{
							let target_count = message_count as usize;
							if restoration_point_found {
								restoration_messages.truncate(target_count);
								crate::log_debug!(
									"Session restoration: Found TRUNCATION_POINT - truncated restoration messages to {}",
									target_count
								);
							} else {
								messages.truncate(target_count);
								crate::log_debug!(
									"Session restoration: Found TRUNCATION_POINT - truncated messages to {}",
									target_count
								);
							}
						}
						pending_tool_calls.clear(); // Clear stale tool calls from before truncation
					}
					"COMMAND" => {
						// Commands are processed separately in extract_runtime_state_from_log
						continue;
					}
					"OUTPUT_MODE_REPLACE" => {
						// Handle Replace mode operations during session restoration
						// This clears messages like a restoration point but from a command
						if restoration_point_found {
							restoration_messages.clear();
						} else {
							messages.clear();
						}
						pending_tool_calls.clear(); // Clear stale tool calls from before replace

						// Log the replace operation for debugging
						if let Some(command) = json_value.get("command").and_then(|c| c.as_str()) {
							println!(
								"Session restoration: Found OUTPUT_MODE_REPLACE from command '{}'",
								command
							);
						}
					}
					"OUTPUT_MODE_APPEND" => {
						// Handle Append mode operations during session restoration
						// These are tracked but don't need special handling since the messages
						// are already in the session file as regular assistant messages
						continue;
					}
					"STATS" => {
						// STATS entries provide incremental updates during a session
						// BUT: Only apply STATS that are NEWER than the last SUMMARY
						// This ensures SUMMARY (written on save/exit) is the source of truth
						let stats_timestamp = json_value
							.get("timestamp")
							.and_then(|t| t.as_u64())
							.unwrap_or(0);

						// Only apply STATS if it's newer than the last SUMMARY
						// This prevents old STATS from overwriting fresh SUMMARY data on resume
						if stats_timestamp > last_summary_timestamp {
							if let Some(info) = &mut session_info {
								// CRITICAL FIX: Only apply STATS values if they're greater than current values
								// This prevents cached-only requests (where non-cached tokens = 0) from
								// overwriting the accumulated token counts from the SUMMARY
								if let Some(total_cost) =
									json_value.get("total_cost").and_then(|c| c.as_f64())
								{
									if total_cost > info.total_cost {
										info.total_cost = total_cost;
									}
								}
								if let Some(input_tokens) =
									json_value.get("input_tokens").and_then(|t| t.as_u64())
								{
									if input_tokens > info.input_tokens {
										info.input_tokens = input_tokens;
									}
								}
								if let Some(output_tokens) =
									json_value.get("output_tokens").and_then(|t| t.as_u64())
								{
									if output_tokens > info.output_tokens {
										info.output_tokens = output_tokens;
									}
								}
								if let Some(cache_read_tokens) =
									json_value.get("cache_read_tokens").and_then(|t| t.as_u64())
								{
									if cache_read_tokens > info.cache_read_tokens {
										info.cache_read_tokens = cache_read_tokens;
									}
								}
								if let Some(cache_write_tokens) = json_value
									.get("cache_write_tokens")
									.and_then(|t| t.as_u64())
								{
									if cache_write_tokens > info.cache_write_tokens {
										info.cache_write_tokens = cache_write_tokens;
									}
								}

								if let Some(tool_calls) =
									json_value.get("tool_calls").and_then(|t| t.as_u64())
								{
									if tool_calls > info.tool_calls {
										info.tool_calls = tool_calls;
									}
								}
								if let Some(api_time) =
									json_value.get("total_api_time_ms").and_then(|t| t.as_u64())
								{
									if api_time > info.total_api_time_ms {
										info.total_api_time_ms = api_time;
									}
								}
								if let Some(tool_time) = json_value
									.get("total_tool_time_ms")
									.and_then(|t| t.as_u64())
								{
									if tool_time > info.total_tool_time_ms {
										info.total_tool_time_ms = tool_time;
									}
								}
								if let Some(layer_time) = json_value
									.get("total_layer_time_ms")
									.and_then(|t| t.as_u64())
								{
									if layer_time > info.total_layer_time_ms {
										info.total_layer_time_ms = layer_time;
									}
								}
							}
						}
					}
					"TOOL_CALL" => {
						// Collect tool calls to reconstruct assistant message with tool_calls
						if let (Some(tool_name), Some(tool_id), Some(parameters)) = (
							json_value.get("tool_name").and_then(|n| n.as_str()),
							json_value.get("tool_id").and_then(|id| id.as_str()),
							json_value.get("parameters"),
						) {
							// Store tool call for later reconstruction
							pending_tool_calls.push(serde_json::json!({
								"id": tool_id,
								"type": "function",
								"function": {
									"name": tool_name,
									"arguments": serde_json::to_string(parameters).unwrap_or_default()
								}
							}));
						}
					}
					"API_REQUEST" | "API_RESPONSE" | "TOOL_RESULT" | "CACHE" | "ERROR"
					| "SYSTEM" | "USER" | "ASSISTANT" => {
						// Skip debug log entries during message parsing
						continue;
					}
					_ => {
						// Unknown log type, skip
						continue;
					}
				}
			} else if line.contains("\"role\":") && line.contains("\"content\":") {
				// This is a regular message JSON line
				if let Ok(message) = serde_json::from_str::<Message>(&line) {
					// If this is the first tool message and we have pending tool calls,
					// reconstruct the assistant message with tool_calls ONLY if not already present
					if message.role == "tool" && !pending_tool_calls.is_empty() {
						// Check if the last message is already an assistant message with tool_calls
						let last_is_assistant_with_tool_calls = if restoration_point_found {
							restoration_messages.last()
						} else {
							messages.last()
						}
						.map(|m| m.role == "assistant" && m.tool_calls.is_some())
						.unwrap_or(false);

						// Only reconstruct if the assistant message doesn't already exist
						// This prevents losing thinking content when the Message JSON was already parsed
						if !last_is_assistant_with_tool_calls {
							let assistant_with_tool_calls = Message {
								role: "assistant".to_string(),
								content: "".to_string(), // Empty content for tool call messages
								tool_calls: Some(serde_json::Value::Array(
									pending_tool_calls.clone(),
								)),
								timestamp: message.timestamp,
								cached: false,
								..Default::default()
							};

							if restoration_point_found {
								restoration_messages.push(assistant_with_tool_calls);
							} else {
								messages.push(assistant_with_tool_calls);
							}
						}

						// Clear pending tool calls since we've reconstructed the assistant message
						pending_tool_calls.clear();
					}

					if restoration_point_found {
						restoration_messages.push(message);
					} else {
						messages.push(message);
					}
				}
			}
		} else {
			// Fallback to legacy prefix-based format for backward compatibility
			if line.starts_with("SUMMARY: ") {
				if let Some(content) = line.strip_prefix("SUMMARY: ") {
					session_info = Some(serde_json::from_str(content)?);
				}
			} else if line.starts_with("INFO: ") {
				if let Some(content) = line.strip_prefix("INFO: ") {
					let mut old_info: SessionInfo = serde_json::from_str(content)?;
					old_info.input_tokens = 0;
					old_info.output_tokens = 0;
					old_info.cache_read_tokens = 0;
					old_info.cache_write_tokens = 0;
					old_info.total_cost = 0.0;
					old_info.duration_seconds = 0;
					old_info.layer_stats = Vec::new();
					old_info.tool_calls = 0;
					// Initialize time tracking for legacy sessions
					old_info.total_api_time_ms = 0;
					old_info.total_tool_time_ms = 0;
					old_info.total_layer_time_ms = 0;
					session_info = Some(old_info);
				}
			} else if line.starts_with("RESTORATION_POINT: ") {
				restoration_point_found = true;
				messages.clear();
				restoration_messages.clear();
			} else if !line.starts_with("API_REQUEST: ")
				&& !line.starts_with("API_RESPONSE: ")
				&& !line.starts_with("TOOL_CALL: ")
				&& !line.starts_with("TOOL_RESULT: ")
				&& !line.starts_with("CACHE: ")
				&& !line.starts_with("ERROR: ")
				&& !line.starts_with("EXCHANGE: ")
				&& !line.is_empty()
			{
				// Try to parse as message JSON or legacy prefixed formats
				if line.contains("\"role\":") && line.contains("\"content\":") {
					if let Ok(message) = serde_json::from_str::<Message>(&line) {
						if restoration_point_found {
							restoration_messages.push(message);
						} else {
							messages.push(message);
						}
					}
				} else if let Some(content) = line.strip_prefix("SYSTEM: ") {
					if let Ok(message) = serde_json::from_str::<Message>(content) {
						if restoration_point_found {
							restoration_messages.push(message);
						} else {
							messages.push(message);
						}
					}
				} else if let Some(content) = line.strip_prefix("USER: ") {
					if let Ok(message) = serde_json::from_str::<Message>(content) {
						if restoration_point_found {
							restoration_messages.push(message);
						} else {
							messages.push(message);
						}
					}
				} else if let Some(content) = line.strip_prefix("ASSISTANT: ") {
					if let Ok(message) = serde_json::from_str::<Message>(content) {
						if restoration_point_found {
							restoration_messages.push(message);
						} else {
							messages.push(message);
						}
					}
				}
			}
		}
	}

	// Use restoration messages if we found a restoration point, otherwise use all messages
	let final_messages = if restoration_point_found && !restoration_messages.is_empty() {
		restoration_messages
	} else {
		messages
	};

	if let Some(mut info) = session_info {
		// Extract runtime state from log file
		let runtime_state = extract_runtime_state_from_log(session_file)?;

		// Apply runtime state to session info
		if let Some(model) = runtime_state.model {
			info.model = model;
		}

		// Clean up interrupted tool calls only if there are actually incomplete tool calls
		let mut cleaned_messages = final_messages;
		if has_incomplete_tool_calls(&cleaned_messages) {
			clean_interrupted_tool_calls(&mut cleaned_messages, &info.name, "Session restoration");
		}

		let session = Session {
			info,
			messages: cleaned_messages,
			session_file: Some(session_file.clone()),
		};

		Ok(session)
	} else {
		// Fallback: Create default session info when SUMMARY is missing
		// This allows loading of older sessions or sessions with missing metadata

		// Extract session name from file path
		let session_name = session_file
			.file_stem()
			.and_then(|s| s.to_str())
			.unwrap_or("unknown")
			.to_string();

		// Try to infer model from messages or use default
		let default_model = final_messages
			.iter()
			.find_map(|_msg| {
				// Look for any model information in message metadata
				// This is a best-effort attempt
				None::<String> // For now, we'll use a default
			})
			.unwrap_or_else(|| "openrouter:anthropic/claude-sonnet-4".to_string());

		// Get file creation time as fallback for created_at
		let created_at = session_file
			.metadata()
			.and_then(|meta| {
				meta.created()
					.ok()
					.ok_or(std::io::Error::other("No creation time"))
			})
			.and_then(|time| {
				time.duration_since(std::time::UNIX_EPOCH)
					.ok()
					.ok_or(std::io::Error::other("Invalid time"))
			})
			.map(|duration| duration.as_secs())
			.unwrap_or_else(|_| {
				std::time::SystemTime::now()
					.duration_since(std::time::UNIX_EPOCH)
					.unwrap_or_default()
					.as_secs()
			});

		// Create default session info
		let default_info = SessionInfo {
			name: session_name,
			created_at,
			model: default_model.clone(),
			provider: if default_model.starts_with("openrouter:") {
				"openrouter".to_string()
			} else if default_model.starts_with("anthropic:") {
				"anthropic".to_string()
			} else if default_model.starts_with("openai:") {
				"openai".to_string()
			} else {
				"unknown".to_string()
			},
			input_tokens: 0,
			output_tokens: 0,
			cache_read_tokens: 0,
			cache_write_tokens: 0,
			reasoning_tokens: 0,
			total_cost: 0.0,
			duration_seconds: 0,
			layer_stats: Vec::new(),
			tool_calls: 0,
			total_api_time_ms: 0,
			total_tool_time_ms: 0,
			total_layer_time_ms: 0,
			compression_stats: CompressionStats::default(),
			total_api_calls: 0,
			// Initialize cache state
			current_non_cached_tokens: 0,
			current_total_tokens: 0,
			last_cache_checkpoint_time: current_timestamp(),
			// Initialize runtime state
			cache_next_user_message: false,
			spending_threshold_checkpoint: 0.0,

			compression_hint_count: 0,
			last_compression_hint_shown: 0,
			next_conversation_compression_at_api_call: 0,
			predicted_turns_at_last_compression: 0.0,
			api_calls_at_last_compression: 0,
			output_tokens_at_last_compression: 0,
		};

		// Extract runtime state from log file

		let runtime_state = extract_runtime_state_from_log(session_file)?;

		// Apply runtime state to default session info
		let mut info = default_info;
		if let Some(model) = runtime_state.model {
			info.model = model;
		}

		// Extract cost and stats information from STATS entries in fallback mode
		let file = File::open(session_file)?;
		let reader = BufReader::new(file);
		for line in reader.lines() {
			let line = line?;
			if let Ok(json_value) = serde_json::from_str::<serde_json::Value>(&line) {
				if let Some(log_type) = json_value.get("type").and_then(|t| t.as_str()) {
					if log_type == "STATS" {
						// Extract cost and token information from STATS entries
						if let Some(total_cost) =
							json_value.get("total_cost").and_then(|c| c.as_f64())
						{
							info.total_cost = total_cost;
						}
						if let Some(input_tokens) =
							json_value.get("input_tokens").and_then(|t| t.as_u64())
						{
							info.input_tokens = input_tokens;
						}
						if let Some(output_tokens) =
							json_value.get("output_tokens").and_then(|t| t.as_u64())
						{
							info.output_tokens = output_tokens;
						}
						if let Some(cache_read_tokens) =
							json_value.get("cache_read_tokens").and_then(|t| t.as_u64())
						{
							info.cache_read_tokens = cache_read_tokens;
						}
						if let Some(cache_write_tokens) = json_value
							.get("cache_write_tokens")
							.and_then(|t| t.as_u64())
						{
							info.cache_write_tokens = cache_write_tokens;
						}
						if let Some(tool_calls) =
							json_value.get("tool_calls").and_then(|t| t.as_u64())
						{
							info.tool_calls = tool_calls;
						}
						if let Some(api_time) =
							json_value.get("total_api_time_ms").and_then(|t| t.as_u64())
						{
							info.total_api_time_ms = api_time;
						}
						if let Some(tool_time) = json_value
							.get("total_tool_time_ms")
							.and_then(|t| t.as_u64())
						{
							info.total_tool_time_ms = tool_time;
						}
						if let Some(layer_time) = json_value
							.get("total_layer_time_ms")
							.and_then(|t| t.as_u64())
						{
							info.total_layer_time_ms = layer_time;
						}
					}
				}
			}
		}

		let session = Session {
			info,
			messages: final_messages,
			session_file: Some(session_file.clone()),
		};

		// Save a SUMMARY entry to fix the session file for future loads
		// DISABLED: This was causing session corruption by appending multiple SUMMARY entries
		// let summary_entry = serde_json::json!({
		// 	"type": "SUMMARY",
		// 	"timestamp": std::time::SystemTime::now()
		// 		.duration_since(std::time::UNIX_EPOCH)
		// 		.unwrap_or_default()
		// 		.as_secs(),
		// 	"session_info": &session.info
		// });
		// let _ = append_to_session_file(session_file, &serde_json::to_string(&summary_entry)?);

		println!("⚠️  Session loaded with default metadata (SUMMARY was missing)");
		Ok(session)
	}
}

/// Runtime state extracted from session commands
#[derive(Debug, Default)]
pub struct SessionRuntimeState {
	pub model: Option<String>,
	pub layers_enabled: Option<bool>,
	pub cache_next_message: bool,
	pub role: Option<String>, // Track runtime role changes
}

/// Extract runtime state from session log file
pub fn extract_runtime_state_from_log(session_file: &PathBuf) -> Result<SessionRuntimeState> {
	let file = File::open(session_file)?;
	let reader = BufReader::new(file);
	let mut state = SessionRuntimeState::default();

	for line in reader.lines() {
		let line = line?;

		if let Ok(json_value) = serde_json::from_str::<serde_json::Value>(&line) {
			if let Some(log_type) = json_value.get("type").and_then(|t| t.as_str()) {
				match log_type {
					"RESTORATION_POINT" => {
						// Reset state tracking after restoration point
						state = SessionRuntimeState::default();
					}
					"COMMAND" => {
						// Process all commands to get the final state
						if let Some(command) = json_value.get("command").and_then(|c| c.as_str()) {
							apply_command_to_runtime_state(&mut state, command);
						}
					}
					_ => {}
				}
			}
		}
	}
	Ok(state)
}

/// Apply a command to runtime state (for state extraction)
fn apply_command_to_runtime_state(state: &mut SessionRuntimeState, command_line: &str) {
	let parts: Vec<&str> = command_line.split_whitespace().collect();
	if parts.is_empty() {
		return;
	}

	match parts[0] {
		"/model" => {
			if parts.len() > 1 {
				let new_model = parts[1..].join(" ");
				state.model = Some(new_model);
			}
		}
		"/role" => {
			if parts.len() > 1 {
				let new_role = parts[1].to_string();
				state.role = Some(new_role);
			}
		}
		"/layers" => {
			// Parse the actual state from the logged command
			if parts.len() > 1 {
				let state_str = parts[1];
				state.layers_enabled = Some(state_str == "enabled");
			}
		}
		"/cache" => {
			// Set cache next message flag
			state.cache_next_message = true;
		}
		_ => {
			// Unknown command, ignore
		}
	}
}

// Helper function to append to session file ensuring single lines
pub fn append_to_session_file(session_file: &PathBuf, content: &str) -> Result<(), anyhow::Error> {
	let mut file = OpenOptions::new()
		.create(true)
		.append(true)
		.open(session_file)?;

	// Ensure content is on a single line - replace any newlines with spaces
	let single_line_content = content.replace(['\n', '\r'], " ");
	writeln!(file, "{}", single_line_content)?;
	Ok(())
}

pub async fn create_system_prompt(
	project_dir: &Path,
	config: &crate::config::Config,
	mode: &str,
) -> String {
	// Get mode-specific configuration
	let (_, mcp_config, _, _, system_prompt) = config.get_role_config(mode);

	// For developer role, process placeholders to add project context
	let mut prompt = helper_functions::process_placeholders_async(system_prompt, project_dir).await;

	// Add MCP tools information if enabled
	if !mcp_config.server_refs.is_empty() {
		let config_for_role = config.get_merged_config_for_role(mode);
		let functions = crate::mcp::get_available_functions(&config_for_role).await;
		if !functions.is_empty() {
			prompt.push_str("\n\nYou have access to the following tools:");

			for function in &functions {
				prompt.push_str(&format!(
					"\n\n- {} - {}",
					function.name, function.description
				));
			}
		}
	}

	prompt
}

/// Add compression context hints to system prompt for resumed sessions
/// This informs the AI about compression state to improve reasoning with compressed context
pub fn add_compression_hints_to_prompt(
	prompt: &mut String,
	compression_stats: &crate::session::CompressionStats,
) {
	if compression_stats.total_compressions() == 0 {
		return;
	}

	prompt.push_str(&format!(
		"\n\n## CONTEXT COMPRESSION ACTIVE\n\
		- {} compressions performed\n\
		- {} tokens saved ({:.1}% reduction)\n\
		- Compressed sections marked with [COMPRESSED: id]\n\
		- Technical details preserved verbatim in TECHNICAL sections\n\
		- Focus on recent uncompressed messages for current context",
		compression_stats.total_compressions(),
		compression_stats.total_tokens_saved,
		compression_stats.avg_compression_ratio() * 100.0
	));
}

/// High-level function to send a chat completion with input validation and context management
/// This function checks input size and prompts user for handling when limits are exceeded
pub async fn chat_completion_with_validation(
	params: ChatCompletionWithValidationParams<'_>,
) -> Result<ProviderResponse> {
	// Check for cancellation before starting
	if let Some(ref token) = params.cancellation_token {
		if *token.borrow() {
			return Err(anyhow::anyhow!("Request cancelled before validation"));
		}
	}

	// Parse the model string and get the appropriate provider
	let (provider, actual_model) = ProviderFactory::get_provider_for_model(params.model)?;

	// Get maximum input tokens for this provider/model (actual context window)
	let max_input_tokens = provider.get_max_input_tokens(&actual_model);

	// Calculate EXACTLY what we're about to send to the API using enhanced token counting
	let total_input_tokens = if params.chat_session.is_some() {
		// Use enhanced token counting that includes system prompt + tools
		let tools = crate::mcp::get_available_functions(params.config).await;
		estimate_full_context_tokens(
			params.messages,
			if tools.is_empty() { None } else { Some(&tools) },
		)
	} else {
		// Fallback for cases without chat session - use basic counting
		estimate_session_tokens(params.messages)
	};
	if total_input_tokens > max_input_tokens {
		return Err(anyhow::anyhow!(
			"Input size ({} tokens) exceeds provider limit ({} tokens) for {} {}",
			total_input_tokens,
			max_input_tokens,
			provider.name(),
			actual_model
		));
	}

	// Check for cancellation before API call
	if let Some(ref token) = params.cancellation_token {
		if *token.borrow() {
			return Err(anyhow::anyhow!("Request cancelled before API call"));
		}
	}

	// Input size is acceptable, proceed with API call
	let chat_params = ChatCompletionParams::new(
		params.messages,
		&actual_model,
		params.temperature,
		params.top_p,
		params.top_k,
		params.max_tokens,
		params.config,
	)
	.with_max_retries(params.max_retries);

	let chat_params = if let Some(schema) = params.schema {
		chat_params.with_schema(schema)
	} else {
		chat_params
	};

	let chat_params = if let Some(token) = params.cancellation_token {
		chat_params.with_cancellation_token(token)
	} else {
		chat_params
	};

	// Convert to octolib params and call provider
	let octolib_params = chat_params
		.to_octolib_params()
		.await
		.map_err(|e| anyhow::anyhow!("Failed to convert message parameters: {}", e))?;
	let octolib_response = provider.chat_completion(octolib_params).await?;

	// Convert response back to Octomind format
	Ok(crate::providers::convert_response_from_octolib(
		octolib_response,
	))
}

/// Parameters for chat completion with provider
pub struct ChatCompletionProviderParams<'a> {
	pub messages: &'a [Message],
	pub model: &'a str,
	pub temperature: f32,
	pub top_p: f32,
	pub top_k: u32,
	pub max_tokens: u32,
	pub config: &'a Config,
	pub max_retries: u32,
	pub cancellation_token: Option<watch::Receiver<bool>>,
	/// Optional JSON schema for structured output
	pub schema: Option<serde_json::Value>,
}

/// High-level function to send a chat completion using the provider abstraction
/// This function handles model parsing and provider selection automatically
pub async fn chat_completion_with_provider(
	params: ChatCompletionProviderParams<'_>,
) -> Result<ProviderResponse> {
	// Parse the model string and get the appropriate provider
	let (provider, actual_model) = ProviderFactory::get_provider_for_model(params.model)?;

	// Fail fast if schema requested but provider doesn't support structured output
	if params.schema.is_some() && !provider.supports_structured_output(&actual_model) {
		return Err(anyhow::anyhow!(
			"Provider '{}' does not support structured output for model '{}'. Remove --schema or use a compatible provider.",
			provider.name(),
			actual_model
		));
	}

	let chat_params = ChatCompletionParams::new(
		params.messages,
		&actual_model,
		params.temperature,
		params.top_p,
		params.top_k,
		params.max_tokens,
		params.config,
	)
	.with_max_retries(params.max_retries);

	let chat_params = if let Some(schema) = params.schema {
		chat_params.with_schema(schema)
	} else {
		chat_params
	};

	// Convert to octolib params and call provider
	let octolib_params = chat_params
		.to_octolib_params()
		.await
		.map_err(|e| anyhow::anyhow!("Failed to convert message parameters: {}", e))?;
	let octolib_response = provider.chat_completion(octolib_params).await?;

	// Convert response back to Octomind format
	Ok(crate::providers::convert_response_from_octolib(
		octolib_response,
	))
}

#[cfg(test)]
mod tests {
	use super::*;
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
		let cleaned = clean_interrupted_tool_calls(&mut messages, "test_session", "Test");

		// Should not clean anything (complete sequence)
		assert!(!cleaned);
		assert_eq!(messages.len(), original_count);
	}

	#[test]
	fn test_clean_interrupted_tool_calls_removes_incomplete() {
		// Test that incomplete sequences are removed
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
			// Missing tool response - this assistant message should be removed
		];

		let cleaned = clean_interrupted_tool_calls(&mut messages, "test_session", "Test");

		// Should clean the incomplete assistant message
		assert!(cleaned);
		assert_eq!(messages.len(), 1); // Only user message should remain
		assert_eq!(messages[0].role, "user");
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
