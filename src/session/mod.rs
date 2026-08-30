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
pub mod external_spend; // Spend by models outside the main loop (subagents, layers, supervisor)
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
pub mod guardrails; // Project-local deny rules evaluated before each tool call
pub mod hooks; // Post-result hooks → inbox injection
pub mod inbox; // Unified message injection queue for all session sources
pub mod inject_listener; // Unix Domain Socket listener for external message injection
pub mod pipe; // Pre-model pipe execution from guardrails
pub mod report; // Session usage reporting
pub mod share; // /share: upload session JSONL → octomind.run/r/<id>
pub mod shell_jobs; // Pending octofs background shell jobs (detached builds/tests)
pub mod smart_summarizer; // Smart text summarization for context management
pub mod tap_runs; // Registry for agents launched via the `tap` core tool
pub mod titles; // Session titles/metadata sidecar store (titles.json)
mod token_counter; // Token counting utilities // Comprehensive caching system
pub mod webhook_listener; // HTTP webhook listener for hook-to-inbox injection

// Provider system exports
pub use crate::providers::{
	AiProvider, ProviderExchange, ProviderFactory, ProviderResponse, TokenUsage,
};
pub use background_jobs::{BackgroundJobManager, CompletedJob};
pub use cache::{CacheManager, CacheStatistics};
pub use helper_functions::summarize_context;
pub use layers::{InputMode, Layer, LayerConfig, LayerResult};
pub use model_utils::{model_max_input_tokens, model_supports_caching};
pub use output::{
	detect_output_mode, JsonlSink, OutputMode, OutputSink, SilentSink, WebSocketSink,
};
pub use project_context::ProjectContext;
pub use smart_summarizer::SmartSummarizer;
pub use token_counter::{
	calculate_minimum_session_tokens, estimate_full_context_tokens, estimate_message_tokens,
	estimate_session_tokens, estimate_tokens, truncate_to_tokens, validate_session_token_threshold,
}; // Export token counting functions // Export cache management

/// Whether the current session still owns asynchronous work whose completion
/// must be delivered before a headless session may exit.
pub fn has_pending_async_work() -> bool {
	crate::mcp::orchestration::has_pending_schedules()
		|| crate::mcp::orchestration::has_running_monitors()
		|| shell_jobs::has_pending()
		|| tap_runs::has_running_jobs()
		|| context::get_job_manager_for_session().is_some_and(|manager| manager.active_count() > 0)
		|| inbox::has_inbox_messages()
}

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

/// Return the textual reasoning stored with an assistant message.
///
/// Current providers persist [`ThinkingBlock`](crate::providers::ThinkingBlock)
/// as `{ "content": ..., "tokens": ... }`. The string and legacy field
/// fallbacks keep older session logs useful without exposing accounting or
/// provider metadata as transcript prose.
pub fn message_thinking_content(message: &Message) -> Option<&str> {
	let thinking = message.thinking.as_ref()?;
	let content = thinking.as_str().or_else(|| {
		thinking
			.get("content")
			.or_else(|| thinking.get("reasoning"))
			.or_else(|| thinking.get("thinking"))
			.and_then(serde_json::Value::as_str)
	})?;
	(!content.trim().is_empty()).then_some(content.trim())
}

pub fn is_system_managed_user_content(content: &str) -> bool {
	let trimmed = content.trim_start();
	trimmed.starts_with("<instructions>")
		|| trimmed.starts_with(CONTINUATION_TAG_OPEN)
		|| trimmed.starts_with("<system-note>")
		|| crate::mcp::runtime::skill::is_skill_message(content)
		|| crate::supervisor::gate::is_supervisor_injection(content)
}

/// Enforce the system-managed contract at the injection seam: content that
/// carries no recognized marker (an inbox report, a tool-usage hint) is
/// wrapped in `<system-note>` so it can never classify as a genuine user
/// turn — the gate boundary, task resolution, recitation, and learning all
/// key on [`is_real_user_task_message`], and an unmarked injection would
/// silently become "the current user request".
pub fn ensure_system_managed(content: &str) -> std::borrow::Cow<'_, str> {
	if is_system_managed_user_content(content) {
		std::borrow::Cow::Borrowed(content)
	} else {
		std::borrow::Cow::Owned(format!("<system-note>\n{content}\n</system-note>"))
	}
}

pub fn is_real_user_task_message(message: &Message) -> bool {
	if message.role != "user" || message.content.trim().is_empty() {
		return false;
	}
	!is_system_managed_user_content(&message.content)
}

/// Open tag of the synthetic wrapper compaction inserts to carry the live
/// request forward once the raw user turns have been drained.
pub const CONTINUATION_TAG_OPEN: &str = "<continuation>";
/// Open/close tags of the model-facing resumption action embedded in a
/// continuation wrapper. Older wrappers stored the user request here too.
pub const CONTINUATION_TASK_OPEN: &str = "<task>";
pub const CONTINUATION_TASK_CLOSE: &str = "</task>";
/// Open/close tags for the exact user request that originated the active turn.
/// New wrappers keep this separate from `<task>`, which describes where the
/// already-running work should resume after compaction.
pub const CONTINUATION_REQUEST_OPEN: &str = "<request>";
pub const CONTINUATION_REQUEST_CLOSE: &str = "</request>";
/// Placeholder a continuation wrapper carries when there was no real user
/// intent to forward. Shared by the builder and every reader so they can't drift.
pub const CONTINUATION_FALLBACK_INTENT: &str = "see summary above for the active task";

/// The exact user request a continuation wrapper carries, borrowed from
/// `<request>` in current wrappers or `<task>` in older persisted wrappers.
///
/// `None` when `content` is not a wrapper, carries no request, or holds only
/// the synthetic placeholder — i.e. whenever there is no real intent to read.
pub fn continuation_task(content: &str) -> Option<&str> {
	let trimmed = content.trim_start();
	if !trimmed.starts_with(CONTINUATION_TAG_OPEN) {
		return None;
	}
	let (open, close) = if find_block_open(trimmed, CONTINUATION_REQUEST_OPEN).is_some() {
		(CONTINUATION_REQUEST_OPEN, CONTINUATION_REQUEST_CLOSE)
	} else {
		// Backward compatibility for sessions compacted before request and
		// resumption action became separate fields.
		(CONTINUATION_TASK_OPEN, CONTINUATION_TASK_CLOSE)
	};
	let start = find_block_open(trimmed, open)? + open.len();
	let end = trimmed[start..].find(close)? + start;
	let task = trimmed[start..end].trim();
	if task.is_empty() || task == CONTINUATION_FALLBACK_INTENT {
		return None;
	}
	Some(task)
}

/// Index of `tag` where it opens its own line — the block form the wrapper
/// builder emits. The wrapper preamble names both tags inline ("The
/// `<request>` block preserves…"), so a bare `find` would match that prose
/// mention instead of the block and swallow everything up to the real
/// closing tag.
fn find_block_open(content: &str, tag: &str) -> Option<usize> {
	content.find(&format!("\n{tag}")).map(|index| index + 1)
}

/// Index of the message that opens the current turn: the most recent genuine
/// user turn, or — once a compaction has drained them all — the continuation
/// wrapper carrying the live request forward.
///
/// Without the fallback every consumer (lesson recall, task resolution, the
/// verify-gate's turn window, constraint recitation) goes blind for the rest of
/// the turn immediately after a compaction.
pub fn latest_task_turn_index(messages: &[Message]) -> Option<usize> {
	messages
		.iter()
		.rposition(is_real_user_task_message)
		.or_else(|| {
			messages
				.iter()
				.rposition(|m| m.role == "user" && continuation_task(&m.content).is_some())
		})
}

/// The live user request — the text at [`latest_task_turn_index`], unwrapped
/// when it is a continuation wrapper. The two always resolve to the same
/// message, so an index and its content can never disagree.
pub fn latest_real_user_task_content(messages: &[Message]) -> Option<&str> {
	let message = messages.get(latest_task_turn_index(messages)?)?;
	if is_real_user_task_message(message) {
		Some(message.content.as_str())
	} else {
		continuation_task(&message.content)
	}
}

/// Timestamp of the live user request — the message at
/// [`latest_task_turn_index`]. Plan staleness compares the plan's last model
/// engagement against this: timestamps survive compaction, message indices
/// don't.
pub fn latest_task_timestamp(messages: &[Message]) -> Option<u64> {
	messages
		.get(latest_task_turn_index(messages)?)
		.map(|message| message.timestamp)
}

/// Completed genuine turns whose call counts feed the fold pace estimate.
pub const TURN_HISTORY: usize = 16;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LearningSessionStats {
	pub packs: u64,
	pub items: u64,
	pub tokens: u64,
	pub used: u64,
	pub credit_positive: u64,
	pub credit_negative: u64,
	pub used_without_verdict: u64,
}

impl LearningSessionStats {
	pub fn record_pack(&mut self, items: u64, tokens: u64) {
		self.packs += 1;
		self.items += items;
		self.tokens += tokens;
	}

	pub fn record_use(&mut self, delta: f64) {
		self.used += 1;
		if delta > 0.0 {
			self.credit_positive += 1;
		} else if delta < 0.0 {
			self.credit_negative += 1;
		} else {
			self.used_without_verdict += 1;
		}
	}
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct TurnTimingStats {
	pub completed: u64,
	pub total_time_ms: u64,
	pub last_time_ms: u64,
}

impl TurnTimingStats {
	pub fn record(&mut self, elapsed: std::time::Duration) {
		let elapsed_ms = elapsed.as_millis().min(u64::MAX as u128) as u64;
		self.completed = self.completed.saturating_add(1);
		self.total_time_ms = self.total_time_ms.saturating_add(elapsed_ms);
		self.last_time_ms = elapsed_ms;
	}

	pub fn average_time_ms(&self) -> u64 {
		self.total_time_ms.checked_div(self.completed).unwrap_or(0)
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
	/// User-perceived latency for completed genuine user turns. The runtime
	/// timer is monotonic and process-local; only completed aggregates persist.
	#[serde(default)]
	pub turn_timing: TurnTimingStats,
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
	// Exact post-compression context watermark used by the adaptive controller.
	#[serde(default)]
	pub context_tokens_after_last_compression: usize, // 0 = no prior compression, can compress immediately
	// API call counts of completed genuine user turns, most recent last, capped
	// at TURN_HISTORY. The fold amortization estimate runs on this pace.
	#[serde(default)]
	pub turn_call_counts: Vec<u32>,
	// API call count when the current genuine user turn started.
	#[serde(default)]
	pub api_calls_at_turn_start: usize,
	#[serde(default)]
	pub api_calls_at_last_compression: usize, // API call count at last compression
	#[serde(default)]
	pub output_tokens_at_last_compression: u64, // Cumulative output tokens at last compression (for incremental growth rate)
	// Consecutive autonomous compressions. Each cycle doubles the desired quiet
	// runway; a genuine user turn resets it so new work gets a short horizon.
	#[serde(default)]
	pub consecutive_compressions: u32,
	/// Persisted learning usage for this named session. Active pack contents and
	/// pack-local IDs remain runtime-only in `ChatSession`.
	#[serde(default)]
	pub learning_stats: LearningSessionStats,
	/// Standing user policy for assistant-run verification. Updated from genuine
	/// user turns only; persisted independently of detector streaks and context
	/// compression.
	#[serde(default)]
	pub verification_policy: crate::supervisor::VerificationPolicy,
	/// Verify-gate evidence ledger snapshot for the still-open turn. Synced on
	/// every save and restored on resume, so the gate's ground truth survives a
	/// process restart the same way the task request does — without it, resumed
	/// turns re-derive evidence conditions from the persisted request but judge
	/// them against an empty ledger, producing false verification gaps.
	#[serde(default)]
	pub evidence: crate::supervisor::gate::EvidenceLedger,
}

impl SessionInfo {
	/// Close the previous genuine turn's call count and open a new one. A turn
	/// that made no calls (answered from context) carries no pace signal.
	pub fn note_turn_start(&mut self) {
		let calls = self
			.total_api_calls
			.saturating_sub(self.api_calls_at_turn_start);
		if calls > 0 {
			self.turn_call_counts.push(calls as u32);
			if self.turn_call_counts.len() > TURN_HISTORY {
				self.turn_call_counts.remove(0);
			}
		}
		self.api_calls_at_turn_start = self.total_api_calls;
	}
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
	// The compression decision model's own spend — a separate model from the
	// agent, so `/info` can break it out while total_cost stays the overall sum.
	#[serde(default)]
	pub input_tokens: u64,
	#[serde(default)]
	pub output_tokens: u64,
	#[serde(default)]
	pub reasoning_tokens: u64,
	#[serde(default)]
	pub cost: f64,
	/// Wall time of the compression model's own API requests, for throughput.
	#[serde(default)]
	pub api_time_ms: u64,
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
				turn_timing: TurnTimingStats::default(),
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

				context_tokens_after_last_compression: 0,
				turn_call_counts: Vec::new(),
				api_calls_at_turn_start: 0,
				api_calls_at_last_compression: 0,
				output_tokens_at_last_compression: 0,
				consecutive_compressions: 0,
				learning_stats: LearningSessionStats::default(),
				verification_policy: crate::supervisor::VerificationPolicy::default(),
				evidence: crate::supervisor::gate::EvidenceLedger::default(),
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

	/// Fold spend by models that ran outside the main loop (subagents, layers,
	/// supervisor) into the session total, so `total_cost` is the session's real
	/// bill rather than just the main agent's share. Their per-source token
	/// breakdowns stay in their own accumulators for `/info` to show.
	pub fn fold_external_spend(&mut self) {
		self.info.total_cost += external_spend::take();
	}

	// Save the session to a file - append-only approach
	pub fn save(&self) -> Result<(), anyhow::Error> {
		if let Some(session_file) = &self.session_file {
			// Append-only design: individual messages are persisted when added.
			// This just appends an updated SUMMARY snapshot of the current session state.
			let summary_entry = persistence::summary_log_entry(&self.info);
			append_to_session_file(session_file, &serde_json::to_string(&summary_entry)?)?;
			Ok(())
		} else {
			Err(anyhow::anyhow!("No session file specified"))
		}
	}
}

pub mod persistence;

pub mod picker; // Interactive fuzzy session picker (bare `octomind` in a terminal)
pub use persistence::{
	append_to_session_file, clean_interrupted_tool_calls, extract_runtime_state_from_log,
	find_most_recent_session_for_project, get_sessions_dir, list_available_sessions, load_session,
	resume_role, SessionRuntimeState,
};
pub mod prompt;
pub use prompt::{add_compression_hints_to_prompt, create_system_prompt};
pub mod completion;
pub use completion::{
	chat_completion_with_provider, chat_completion_with_validation,
	ensure_structured_output_support, load_structured_output_schema, ChatCompletionProviderParams,
	ChatCompletionWithValidationParams,
};

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
