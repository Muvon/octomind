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

// Chat session implementation

use super::utils::format_number;
use crate::config::Config;
use crate::mcp::dev::plan;
use crate::session::{
	estimate_full_context_tokens, get_sessions_dir, load_session, CompressionStats, Session,
};
use anyhow::Result;
use chrono::{DateTime, Utc};
use colored::Colorize;
use std::fs::File;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

/// Parameters for chat session initialization
///
/// This struct groups all parameters needed for creating or resuming a chat session,
/// following best practices for parameter passing and future extensibility.
pub struct SessionInitParams<'a> {
	/// Optional session name (if None, generates UUID)
	pub name: Option<String>,
	/// Optional session ID to resume
	pub resume: Option<String>,
	/// Resume the most recent session for the current project
	pub resume_recent: bool,
	/// Optional model override
	pub model: Option<String>,
	/// Optional temperature override
	pub temperature: Option<f32>,
	/// Optional max tokens override
	pub max_tokens: Option<u32>,
	/// Optional max retries override
	pub max_retries: Option<u32>,
	/// Output mode: plain or jsonl (for CLI suppression)
	pub output_mode: Option<String>,
	/// Configuration object
	pub config: &'a Config,
	/// Role for the session
	pub role: &'a str,
}

impl<'a> SessionInitParams<'a> {
	/// Create new session initialization parameters with required fields
	pub fn new(config: &'a Config, role: &'a str) -> Self {
		Self {
			name: None,
			resume: None,
			resume_recent: false,
			model: None,
			temperature: None,
			max_tokens: None,
			max_retries: None,
			output_mode: None,
			config,
			role,
		}
	}

	/// Set session name
	pub fn with_name(mut self, name: String) -> Self {
		self.name = Some(name);
		self
	}

	/// Set session to resume
	pub fn with_resume(mut self, resume: String) -> Self {
		self.resume = Some(resume);
		self
	}

	/// Set resume recent flag
	pub fn with_resume_recent(mut self, resume_recent: bool) -> Self {
		self.resume_recent = resume_recent;
		self
	}

	/// Set model override
	pub fn with_model(mut self, model: String) -> Self {
		self.model = Some(model);
		self
	}

	/// Set temperature override
	pub fn with_temperature(mut self, temperature: f32) -> Self {
		self.temperature = Some(temperature);
		self
	}

	/// Set max tokens override
	pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
		self.max_tokens = Some(max_tokens);
		self
	}

	/// Set max retries override
	pub fn with_max_retries(mut self, max_retries: u32) -> Self {
		self.max_retries = Some(max_retries);
		self
	}

	/// Set output mode (plain or jsonl)
	pub fn with_output_mode(mut self, output_mode: String) -> Self {
		self.output_mode = Some(output_mode);
		self
	}
}

// Generate a session name in format: YYMMDD-HHMMSS-basename-uuid
fn generate_session_name() -> String {
	let now = chrono::Local::now();
	let date_str = now.format("%y%m%d").to_string();
	let time_str = now.format("%H%M%S").to_string();

	// Get current directory basename
	let current_dir = std::env::current_dir().unwrap_or_default();
	let basename = current_dir
		.file_name()
		.unwrap_or_default()
		.to_string_lossy()
		.to_string();

	// Generate a short UUID (first 8 characters)
	let uuid = Uuid::new_v4().to_string();
	let short_uuid: String = uuid.chars().take(8).collect();

	format!("{}-{}-{}-{}", date_str, time_str, basename, short_uuid)
}

// Chat session manager for interactive coding sessions
pub struct ChatSession {
	pub session: Session,
	pub last_response: String,
	pub model: String,
	pub role: String, // Role for the session
	pub temperature: f32,
	pub top_p: f32, // Top-p nucleus sampling parameter
	pub top_k: u32, // Top-k sampling parameter
	pub max_tokens: u32,
	pub estimated_cost: f64,
	pub cache_next_user_message: bool, // Flag to cache the next user message
	pub spending_threshold_checkpoint: f64, // Track spending at last threshold check
	pub request_spending_checkpoint: f64, // Track spending at start of current request
	pub pending_image: Option<crate::session::image::ImageAttachment>, // Pending image attachment
	pub max_retries: u32,              // Maximum number of retries for provider errors
	pub continuation_pending: bool,    // Flag for session continuation state
	pub continuation_disabled: bool,   // Flag to temporarily disable continuation triggers
	pub was_resumed: bool, // Flag indicating if this session was resumed from an existing file
	pub pending_prompt: Option<String>, // Pending prompt text to be processed as user input
	pub initial_status_shown: bool, // Flag to track if initial status line was displayed
	// Compression hint tracking
	pub compression_hint_count: usize, // Counter for compression hints
	pub last_compression_hint_shown: u64, // Timestamp of last compression hint
	// Token calculation cache - SINGLE SOURCE OF TRUTH for context token counting
	// This cache ensures all systems (display, compression, continuation) use identical calculations
	pub cached_tools: Option<Vec<crate::mcp::McpFunction>>, // Cached tool definitions for consistent token counting
}

/// Parameters for creating a new ChatSession
pub struct ChatSessionParams<'a> {
	pub name: String,
	pub model: Option<String>,
	pub temperature: Option<f32>,
	pub top_p: Option<f32>,
	pub top_k: Option<u32>,
	pub max_tokens: Option<u32>,
	pub max_retries: Option<u32>,
	pub config: &'a Config,
	pub role: &'a str,
}

impl ChatSession {
	// Create a new chat session
	pub fn new(params: ChatSessionParams<'_>) -> Self {
		let model_name = params
			.model
			.unwrap_or_else(|| params.config.get_effective_model());
		// STRICT: temperature should always be provided from role config, no fallbacks
		let temperature_value = params
			.temperature
			.expect("Temperature must be provided from role config");
		// STRICT: top_p should always be provided from role config, no fallbacks
		let top_p_value = params
			.top_p
			.expect("Top_p must be provided from role config");
		// STRICT: top_k should always be provided from role config, no fallbacks
		let top_k_value = params
			.top_k
			.expect("Top_k must be provided from role config");
		// STRICT: max_tokens should always be provided from role config, no fallbacks
		let max_tokens_value = params
			.max_tokens
			.expect("Max tokens must be provided from role config");
		// max_retries defaults to 0 if not provided (runtime-only parameter)
		let max_retries_value = params.max_retries.unwrap_or(0);

		// Create a new session with initial info
		let timestamp = SystemTime::now()
			.duration_since(UNIX_EPOCH)
			.unwrap_or_default()
			.as_secs();

		let session_info = crate::session::SessionInfo {
			name: params.name.clone(),
			created_at: timestamp,
			model: model_name.clone(),
			provider: "openrouter".to_string(),
			input_tokens: 0,
			output_tokens: 0,
			cached_tokens: 0,
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
			continuation_pending: false,
			continuation_disabled: false,
			compression_hint_count: 0,
			last_compression_hint_shown: 0,
		};

		let session = Session {
			info: session_info,
			messages: Vec::new(),
			session_file: None,
		};

		Self {
			session,
			last_response: String::new(),
			model: model_name,
			role: params.role.to_string(),
			temperature: temperature_value,     // Use the provided temperature
			top_p: top_p_value,                 // Use the provided top_p
			top_k: top_k_value,                 // Use the provided top_k
			max_tokens: max_tokens_value,       // Use the provided max_tokens
			estimated_cost: 0.0,                // Initialize estimated cost as zero
			cache_next_user_message: false,     // Initialize cache flag
			spending_threshold_checkpoint: 0.0, // Initialize spending checkpoint
			request_spending_checkpoint: 0.0,   // Initialize request spending checkpoint
			pending_image: None,                // Initialize pending image
			max_retries: max_retries_value,     // Set max retries value
			continuation_pending: false,        // Initialize continuation state
			continuation_disabled: false,       // Initialize continuation control flag
			was_resumed: false,                 // This is a new session
			pending_prompt: None,               // Initialize pending prompt
			initial_status_shown: false,        // Initialize status display flag
			compression_hint_count: 0,          // Initialize compression hint counter
			last_compression_hint_shown: 0,     // Initialize last hint timestamp
			cached_tools: None,                 // Initialize tool cache (populated on first use)
		}
	}

	// Initialize a new chat session or load existing one
	pub async fn initialize(params: SessionInitParams<'_>) -> Result<Self> {
		let sessions_dir = get_sessions_dir()?;

		// Handle resume_recent flag
		let effective_resume = if params.resume_recent {
			// Get current working directory
			let current_dir = std::env::current_dir()?;

			// Find the most recent session for this project
			match crate::session::find_most_recent_session_for_project(&current_dir)? {
				Some(session_name) => {
					use colored::*;
					println!(
						"{}",
						format!(
							"✓ Found recent session for current project: {}",
							session_name
						)
						.bright_green()
					);
					Some(session_name)
				}
				None => {
					use colored::*;
					println!(
						"{}",
						"⚠ No recent session found for current project. Creating new session."
							.yellow()
					);
					None
				}
			}
		} else {
			params.resume.clone()
		};

		// Determine session name
		let session_name = if let Some(name_arg) = &params.name {
			name_arg.clone()
		} else if let Some(resume_name) = &effective_resume {
			resume_name.clone()
		} else {
			// Generate a name using the new format
			generate_session_name()
		};

		let session_file = sessions_dir.join(format!("{}.jsonl", session_name));

		// Get temperature from role config if not provided via command line
		let effective_temperature = if let Some(temp) = params.temperature {
			temp // Use command line override
		} else {
			// Read from role configuration - STRICT: assume it exists
			let (role_config, _, _, _, _) = params.config.get_role_config(params.role);
			role_config.temperature
		};

		// Get top_p and top_k from role config - STRICT: always from config, no command line override
		let (role_config, _, _, _, _) = params.config.get_role_config(params.role);
		let effective_top_p = role_config.top_p;
		let effective_top_k = role_config.top_k;

		// Get max_tokens from root config if not provided via command line
		let effective_max_tokens = if let Some(tokens) = params.max_tokens {
			tokens // Use command line override
		} else {
			// Read from root configuration - STRICT: assume it exists
			params.config.get_effective_max_tokens()
		};

		// Check if we should load or create a session
		let should_resume = if effective_resume.is_some() {
			// Explicit resume request - session MUST exist
			if !session_file.exists() {
				return Err(anyhow::anyhow!(
					"Session '{}' not found. Cannot resume non-existent session.",
					session_name
				));
			}
			true
		} else if params.name.is_some() && session_file.exists() {
			// Named session that exists - resume it
			true
		} else {
			// Create new session
			false
		};

		if should_resume {
			use colored::*;

			// Try to load session
			match load_session(&session_file) {
				Ok(session) => {
					// Extract runtime state from session log
					let runtime_state =
						crate::session::extract_runtime_state_from_log(&session_file)
							.unwrap_or_default();
					// When session is loaded successfully, show its info
					println!(
						"{}",
						format!("✓ Resuming session: {}", session_name).bright_green()
					);

					// Show a brief summary of the session
					let created_time =
						DateTime::<Utc>::from_timestamp(session.info.created_at as i64, 0)
							.map(|dt| dt.naive_local().format("%Y-%m-%d %H:%M:%S").to_string())
							.unwrap_or_else(|| "Unknown".to_string());

					// Simplify model name
					let model_parts: Vec<&str> = session.info.model.split('/').collect();
					let model_name = if model_parts.len() > 1 {
						model_parts[1]
					} else {
						&session.info.model
					};

					// Calculate total tokens
					let total_tokens = session.info.input_tokens
						+ session.info.output_tokens
						+ session.info.cached_tokens;

					println!("{} {}", "Created:".blue(), created_time.white());
					println!("{} {}", "Model:".blue(), model_name.yellow());
					println!(
						"{} {}",
						"Messages:".blue(),
						session.messages.len().to_string().white()
					);
					println!(
						"{} {}",
						"Tokens:".blue(),
						format_number(total_tokens).bright_blue()
					);
					println!(
						"{} ${:.5}",
						"Cost:".blue(),
						session.info.total_cost.to_string().bright_magenta()
					);

					// Create chat session from loaded session
					let restored_model = session.info.model.clone(); // Extract model before moving session
					let restored_cost = session.info.total_cost; // Extract cost before moving session

					// Restore runtime state from session.info
					let cache_next = session.info.cache_next_user_message;
					let spending_checkpoint = session.info.spending_threshold_checkpoint;
					let continuation_pending = session.info.continuation_pending;
					let continuation_disabled = session.info.continuation_disabled;
					let compression_hint_count = session.info.compression_hint_count;
					let last_compression_hint = session.info.last_compression_hint_shown;

					let mut chat_session = ChatSession {
						session,
						last_response: String::new(),
						model: restored_model,               // Use restored model from session
						role: params.role.to_string(),       // Add role from params
						temperature: effective_temperature,  // Use config-based temperature
						top_p: effective_top_p,              // Use config-based top_p
						top_k: effective_top_k,              // Use config-based top_k
						max_tokens: effective_max_tokens,    // Use config-based max_tokens
						estimated_cost: restored_cost,       // FIXED: Use actual cost from session
						cache_next_user_message: cache_next, // Restore from session.info
						spending_threshold_checkpoint: spending_checkpoint, // Restore from session.info
						request_spending_checkpoint: 0.0,    // Initialize request spending checkpoint
						pending_image: None,                 // Initialize pending image
						max_retries: params.max_retries.unwrap_or(0), // Use provided max_retries or default to 0
						continuation_pending,                // Restore from session.info
						continuation_disabled,               // Restore from session.info
						was_resumed: true,                   // This session was resumed from file
						pending_prompt: None,                // Initialize pending prompt
						initial_status_shown: true,          // Don't show status for resumed sessions
						compression_hint_count,              // Restore from session.info
						last_compression_hint_shown: last_compression_hint, // Restore from session.info
						cached_tools: None,                  // Initialize tool cache (populated on first use)
					};

					// Apply runtime state from session log (legacy support)
					if runtime_state.cache_next_message {
						chat_session.cache_next_user_message = true;
					}

					// Apply restored role if available
					if let Some(restored_role) = runtime_state.role {
						// Validate that the restored role still exists in config
						if params.config.roles.iter().any(|r| r.name == restored_role) {
							chat_session.role = restored_role;
							// Update temperature from the restored role config
							let (role_config, _, _, _, _) =
								params.config.get_role_config(&chat_session.role);
							chat_session.temperature = role_config.temperature;
						}
					}

					// CRITICAL FIX: Recalculate token tracking from actual messages
					// After compression, persisted counters are reset to 0, but messages remain.
					// On resume, we must recalculate from actual message content to restore correct state.
					// This ensures cache thresholds and token counts reflect reality, not stale persisted values.
					let cache_manager = crate::session::cache::CacheManager::new();
					let (total_tokens, non_cached_tokens) =
						cache_manager.estimate_current_session_tokens(&chat_session.session);
					chat_session.session.info.current_total_tokens = total_tokens;
					chat_session.session.info.current_non_cached_tokens = non_cached_tokens;

					crate::log_debug!(
					"Session resume: Recalculated token state - total: {}, non-cached: {} (from {} messages)",
					total_tokens,
					non_cached_tokens,
					chat_session.session.messages.len()
				);

					// Get last assistant response if any
					for msg in chat_session.session.messages.iter().rev() {
						if msg.role == "assistant" {
							chat_session.last_response = msg.content.clone();
							break;
						}
					}

					Ok(chat_session)
				}
				Err(e) => {
					// If this was an explicit resume request, return the error
					if params.resume.is_some() {
						return Err(anyhow::anyhow!(
							"Failed to load session '{}': {}. Cannot resume corrupted or invalid session.",
							session_name,
							e
						));
					}

					// If loading fails for named session, inform the user and create a new session
					println!(
						"{}: {}",
						format!("Failed to load session {}", session_name).bright_red(),
						e
					);
					println!("{}", "Creating a new session instead...".yellow());

					// Generate a new unique session name using the new format
					let new_session_name = generate_session_name();
					let new_session_file = sessions_dir.join(format!("{}.jsonl", new_session_name));

					// Skip CLI output in JSONL mode
					if params.output_mode.as_deref() != Some("jsonl") {
						println!(
							"{}",
							format!("Starting new session: {}", new_session_name).bright_green()
						);
					}

					// Create file if it doesn't exist
					if !new_session_file.exists() {
						let file = File::create(&new_session_file)?;
						drop(file);
					}

					let mut chat_session = ChatSession::new(ChatSessionParams {
						name: new_session_name.clone(),
						model: params.model.clone(),
						temperature: Some(effective_temperature), // Use config-based temperature
						top_p: Some(effective_top_p),             // Use config-based top_p
						top_k: Some(effective_top_k),             // Use config-based top_k
						max_tokens: Some(effective_max_tokens),   // Use config-based max_tokens
						max_retries: params.max_retries,          // Pass max_retries through
						config: params.config,
						role: params.role, // Add role parameter
					});
					chat_session.session.session_file = Some(new_session_file);

					// Immediately save the session info in new JSON format
					let summary_entry = serde_json::json!({
						"type": "SUMMARY",
						"timestamp": std::time::SystemTime::now()
						.duration_since(std::time::UNIX_EPOCH)
						.unwrap_or_default()
						.as_secs(),
						"session_info": &chat_session.session.info
					});
					crate::session::append_to_session_file(
						chat_session.session.session_file.as_ref().unwrap(),
						&serde_json::to_string(&summary_entry)?,
					)?;

					Ok(chat_session)
				}
			}
		} else {
			// Create new session - skip CLI output in JSONL mode
			if params.output_mode.as_deref() != Some("jsonl") {
				use colored::*;
				println!(
					"{}",
					format!("Starting new session: {}", session_name).bright_green()
				);
			}

			// Create session file if it doesn't exist
			if !session_file.exists() {
				let file = File::create(&session_file)?;
				drop(file);
			}

			let mut chat_session = ChatSession::new(ChatSessionParams {
				name: session_name.clone(),
				model: params.model,
				temperature: Some(effective_temperature),
				top_p: Some(effective_top_p),
				top_k: Some(effective_top_k),
				max_tokens: Some(effective_max_tokens),
				max_retries: params.max_retries,
				config: params.config,
				role: params.role,
			});
			chat_session.session.session_file = Some(session_file);

			// Immediately save the session info in new JSON format
			let summary_entry = serde_json::json!({
				"type": "SUMMARY",
				"timestamp": std::time::SystemTime::now()
				.duration_since(std::time::UNIX_EPOCH)
				.unwrap_or_default()
				.as_secs(),
				"session_info": &chat_session.session.info
			});
			crate::session::append_to_session_file(
				chat_session.session.session_file.as_ref().unwrap(),
				&serde_json::to_string(&summary_entry)?,
			)?;

			Ok(chat_session)
		}
	}

	/// Get the effective model for this session (uses session.info.model directly)
	pub fn get_effective_model(&self) -> &str {
		&self.session.info.model
	}

	/// Attach image from file path
	pub async fn attach_image_from_path(&mut self, path: &str) -> Result<()> {
		use crate::session::image::ImageProcessor;
		use std::path::Path;

		// Check if input is a URL
		if ImageProcessor::is_url(path) {
			println!("{}", "🌐 Downloading image from URL...".bright_cyan());

			let image_attachment = ImageProcessor::load_from_url(path).await?;

			// Show preview
			println!("{}", "📸 Image preview:".bright_cyan());
			ImageProcessor::show_preview(&image_attachment)?;

			// Store for next message
			self.pending_image = Some(image_attachment);

			println!(
				"{}",
				"✅ Image downloaded and ready to attach!".bright_green()
			);
			return Ok(());
		}

		// Handle as file path
		let image_path = Path::new(path);

		// Check if file exists
		if !image_path.exists() {
			return Err(anyhow::anyhow!("Image file not found: {}", path));
		}

		// Check if it's a supported image format
		if !ImageProcessor::is_supported_image(image_path) {
			return Err(anyhow::anyhow!(
				"Unsupported image format. Supported: {}",
				ImageProcessor::supported_extensions().join(", ")
			));
		}

		// Load and process the image
		let image_attachment = ImageProcessor::load_from_path(image_path)?;

		// Show preview
		println!("{}", "📸 Image preview:".bright_cyan());
		ImageProcessor::show_preview(&image_attachment)?;

		// Store for next message
		self.pending_image = Some(image_attachment);

		Ok(())
	}

	/// Try to attach image from clipboard
	pub async fn try_attach_from_clipboard(&mut self) -> Result<bool> {
		use crate::session::image::ImageProcessor;

		match ImageProcessor::load_from_clipboard()? {
			Some(image_attachment) => {
				println!("{}", "📋 Image detected in clipboard!".bright_cyan());

				// Show preview
				println!("{}", "📸 Image preview:".bright_cyan());
				ImageProcessor::show_preview(&image_attachment)?;

				// Store for next message
				self.pending_image = Some(image_attachment);

				println!("{}", "✅ Clipboard image ready to attach!".bright_green());
				Ok(true)
			}
			None => Ok(false),
		}
	}

	/// Check if there's a pending image attachment
	pub fn has_pending_image(&self) -> bool {
		self.pending_image.is_some()
	}

	/// Take the pending image (consumes it)
	pub fn take_pending_image(&mut self) -> Option<crate::session::image::ImageAttachment> {
		self.pending_image.take()
	}

	/// Process user commands
	pub async fn process_command(
		&mut self,
		input: &str,
		config: &mut Config,
		role: &str,
		operation_cancelled: tokio::sync::watch::Receiver<bool>,
	) -> Result<super::commands::CommandResult> {
		super::commands::process_command(self, input, config, role, operation_cancelled).await
	}

	/// Disable continuation triggers temporarily
	pub fn disable_continuation(&mut self) {
		self.continuation_disabled = true;
	}

	/// Re-enable continuation triggers
	pub fn enable_continuation(&mut self) {
		self.continuation_disabled = false;
	}

	/// Check if continuation is currently disabled
	pub fn is_continuation_disabled(&self) -> bool {
		self.continuation_disabled
	}

	/// Get current message count (for plan compression tracking)
	pub fn get_message_count(&self) -> usize {
		self.session.messages.len()
	}

	/// Remove messages in specified range for compression
	///
	/// This method safely removes messages between start_index (exclusive) and end_index (inclusive).
	/// It preserves the message at start_index and removes everything up to and including end_index.
	/// The compressed summary will be inserted at start_index + 1.
	///
	/// # Index Semantics (CRITICAL)
	///
	/// - Uses **inclusive range** for removal: `drain(start_index + 1..=end_index)`
	/// - `end_index` must be **< messages.len()** (last valid index is `len() - 1`)
	/// - `end_index >= messages.len()` will return an error (out of bounds for inclusive range)
	///
	/// # Arguments
	/// * `start_index` - Start of range (this message is kept)
	/// * `end_index` - End of range (messages up to and including this are removed)
	///
	/// # Returns
	/// Tuple of (messages_removed, had_cached_messages)
	/// - messages_removed: Number of messages actually removed
	/// - had_cached_messages: True if any removed message had cached=true (for cache preservation)
	///
	/// # Example
	///
	/// If start_index=5 and end_index=10:
	/// - Message 5 is kept (e.g., "Let me investigate...")
	/// - Messages 6, 7, 8, 9, 10 are removed (tool results, plan result)
	/// - Compressed summary inserted after message 5
	///
	/// # Common Pitfall
	///
	/// **DO NOT** use `messages.len()` as end_index - it will fail!
	/// - WRONG: `session.remove_messages_in_range(start, session.get_message_count());`
	/// - CORRECT: `session.remove_messages_in_range(start, session.get_message_count() - 1);`
	pub fn remove_messages_in_range(
		&mut self,
		start_index: usize,
		end_index: usize,
	) -> Result<(usize, bool)> {
		// Validate range
		if start_index >= self.session.messages.len() {
			return Err(anyhow::anyhow!(
				"Invalid start_index: {} (total messages: {})",
				start_index,
				self.session.messages.len()
			));
		}

		if end_index >= self.session.messages.len() {
			return Err(anyhow::anyhow!(
				"Invalid end_index: {} (total messages: {}). end_index must be less than total messages since removal uses inclusive range.",
				end_index,
				self.session.messages.len()
			));
		}

		if start_index >= end_index {
			return Err(anyhow::anyhow!(
				"Invalid range: start_index ({}) must be less than end_index ({})",
				start_index,
				end_index
			));
		}

		// Calculate how many messages to remove (inclusive end_index)
		let messages_to_remove = end_index - start_index;

		if messages_to_remove == 0 {
			crate::log_debug!(
				"No messages to remove in range {}-{}",
				start_index,
				end_index
			);
			return Ok((0, false));
		}

		// CRITICAL: Check if any messages being removed have cached=true
		// This preserves the 2-marker cache system during compression
		let had_cached = self.session.messages[start_index + 1..=end_index]
			.iter()
			.any(|msg| msg.cached);

		// Remove messages from start_index+1 through end_index (inclusive)
		// Using ..= for inclusive end index
		self.session.messages.drain(start_index + 1..=end_index);

		crate::log_debug!(
			"Compressed {} messages (range {}-{}), had_cached={}",
			messages_to_remove,
			start_index,
			end_index,
			had_cached
		);

		Ok((messages_to_remove, had_cached))
	}

	/// Insert compressed knowledge entry as system message
	///
	/// This injects a structured summary of completed work into the session history,
	/// replacing the detailed tool calls and intermediate steps.
	///
	/// # Arguments
	/// * `index` - Position to insert (after this index)
	/// * `content` - Formatted summary content
	/// * `preserve_cache` - If true, mark compressed message as cached (preserves 2-marker system)
	pub fn insert_compressed_knowledge(
		&mut self,
		index: usize,
		content: String,
		preserve_cache: bool,
	) -> Result<()> {
		use crate::session::Message;

		if index >= self.session.messages.len() {
			return Err(anyhow::anyhow!(
				"Invalid index: {} (total messages: {})",
				index,
				self.session.messages.len()
			));
		}

		let compressed_msg = Message {
			role: "assistant".to_string(), // AI-generated summary content
			content,
			timestamp: std::time::SystemTime::now()
				.duration_since(std::time::UNIX_EPOCH)
				.unwrap_or_default()
				.as_secs(),
			cached: preserve_cache, // Preserve cache marker if any removed message was cached (maintains 2-marker system)
			tool_call_id: None,
			name: Some("plan_compression".to_string()),
			tool_calls: None,
			images: None,
			thinking: None,
			id: None,
		};

		self.session.messages.insert(index + 1, compressed_msg);

		crate::log_debug!(
			"Inserted compressed knowledge at index {} (cached={})",
			index + 1,
			preserve_cache
		);

		Ok(())
	}

	/// Check if compression hint should be shown based on context pressure
	pub fn should_show_compression_hint(&mut self, config: &Config) -> bool {
		// Only suggest if there's an active plan
		if !plan::has_active_plan() {
			return false;
		}

		// Check if hints are enabled in config
		if !config.compression.hints_enabled {
			return false;
		}

		// Check if compression is not disabled
		if self.continuation_disabled {
			return false;
		}

		// Calculate context pressure
		let current_tokens = estimate_full_context_tokens(&self.session.messages, None, None);
		let max_tokens = config.max_session_tokens_threshold;

		if max_tokens == 0 {
			return false; // Threshold disabled
		}

		let pressure = current_tokens as f64 / max_tokens as f64;

		// Only suggest at configured threshold
		if pressure < config.compression.hints_pressure_threshold {
			return false;
		}

		// Rate limit hints - only show every N tool executions
		self.compression_hint_count += 1;
		self.compression_hint_count % config.compression.hints_min_interval == 1
	}

	/// Get compression hint message if applicable
	pub fn get_compression_hint(&mut self, config: &Config) -> Option<String> {
		if self.should_show_compression_hint(config) {
			Some(
				"\n\n💡 Hint: Consider using `/plan next` to compress completed tasks and free up context space for remaining work.".to_string(),
			)
		} else {
			None
		}
	}

	/// Reinitialize session for new role - updates system prompt and MCP servers
	pub async fn reinitialize_for_role(
		&mut self,
		new_role: &str,
		config: &crate::config::Config,
	) -> anyhow::Result<()> {
		use crate::session::create_system_prompt;
		use colored::Colorize;

		// Get current directory for system prompt processing
		let current_dir = std::env::current_dir()?;

		// Get merged configuration for the new role
		let config_for_role = config.get_merged_config_for_role(new_role);

		// Shutdown existing MCP servers first
		if let Err(e) = crate::mcp::process::stop_all_servers() {
			println!(
				"{}: {}",
				"Warning: Failed to stop existing MCP servers".bright_yellow(),
				e
			);
		}

		// SIMPLIFIED: Use the same initialization logic as startup
		// This handles both server initialization AND tool map update
		if let Err(e) = crate::mcp::initialize_mcp_for_role(new_role, config).await {
			println!(
				"{}: {}",
				"Warning: Failed to initialize MCP for new role".bright_yellow(),
				e
			);
			println!("{}", "Some tools may not be available".yellow());
		} else {
			println!(
				"{}",
				"✓ MCP servers and tools updated for new role".bright_green()
			);
		}

		// Create new system prompt for the role (AFTER MCP servers are initialized)
		// This ensures the tools definition reflects the new role's available tools
		let new_system_prompt =
			create_system_prompt(&current_dir, &config_for_role, new_role).await;

		// Find and replace the first system message (should be index 0)
		if let Some(first_msg) = self.session.messages.first_mut() {
			if first_msg.role == "system" {
				// Log the system message replacement
				let _ = crate::session::logger::log_system_message(
					&self.session.info.name,
					&new_system_prompt,
				);

				// Replace the system message content
				first_msg.content = new_system_prompt;

				println!(
					"{}",
					"✓ System prompt updated with new role's tools".bright_green()
				);
			} else {
				// This shouldn't happen in normal operation, but handle gracefully
				return Err(anyhow::anyhow!(
					"Expected first message to be system message, found: {}",
					first_msg.role
				));
			}
		} else {
			// No messages yet - add system message (shouldn't happen for role switching)
			self.add_system_message(&new_system_prompt)?;
			println!(
				"{}",
				"✓ System prompt initialized for new role".bright_green()
			);
		}

		// Save the session to persist the changes
		self.save()?;

		Ok(())
	}

	/// UNIFIED TOKEN CALCULATION - SINGLE SOURCE OF TRUTH
	///
	/// This method ensures ALL systems (display, compression, continuation, etc.) use
	/// IDENTICAL token calculations by:
	/// 1. Caching tool definitions to avoid repeated async fetches
	/// 2. Using the exact same estimate_full_context_tokens() function
	/// 3. Including system prompt + tools for accurate context size
	///
	/// **CRITICAL**: This is the ONLY method that should be used for context token counting.
	/// Direct calls to estimate_full_context_tokens() should be replaced with this method.
	///
	/// # Arguments
	/// * `config` - Configuration to get system prompt and fetch tools if not cached
	///
	/// # Returns
	/// Total context tokens including messages + system prompt + tool definitions
	pub async fn get_full_context_tokens(&mut self, config: &Config) -> usize {
		// Fetch and cache tools if not already cached
		if self.cached_tools.is_none() {
			self.cached_tools = Some(crate::mcp::get_available_functions(config).await);
		}

		// Get system prompt for the role
		let (_, _, _, _, system_prompt) = config.get_role_config(&self.role);

		// Use the unified calculation with cached tools
		estimate_full_context_tokens(
			&self.session.messages,
			Some(system_prompt),
			self.cached_tools.as_deref(),
		)
	}

	/// Invalidate tool cache (call when MCP configuration changes)
	pub fn invalidate_tool_cache(&mut self) {
		self.cached_tools = None;
	}
}
