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

// Chat session implementation

use super::utils::format_number;
use crate::config::Config;
use crate::mcp::core::plan;
use crate::session::{
	estimate_full_context_tokens, get_sessions_dir, load_session, CompressionStats, Session,
};
use anyhow::Result;
use chrono::{DateTime, Utc};
use colored::Colorize;
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
	/// Output mode: plain or jsonl (for CLI suppression)
	pub output_mode: Option<String>,
	/// Configuration object
	pub config: &'a Config,
	/// Role for the session
	pub role: &'a str,
	/// Optional JSON schema for structured output
	pub schema: Option<serde_json::Value>,
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
			schema: None,
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

	/// Set JSON schema for structured output
	pub fn with_schema(mut self, schema: serde_json::Value) -> Self {
		self.schema = Some(schema);
		self
	}
}

// Generate a session name in format: YYMMDD-basename-HHMM-uuid4
fn generate_session_name() -> String {
	let now = chrono::Local::now();
	let date_str = now.format("%y%m%d").to_string();
	let time_str = now.format("%H%M").to_string();

	// Get current directory basename - use thread-local if set (ACP/WebSocket sessions), otherwise process cwd
	let current_dir = crate::mcp::get_thread_working_directory();
	let basename = current_dir
		.file_name()
		.unwrap_or_default()
		.to_string_lossy()
		.to_string();

	// Generate a short UUID (first 4 characters)
	let uuid = Uuid::new_v4().to_string();
	let short_uuid: String = uuid.chars().take(4).collect();

	format!("{}-{}-{}-{}", date_str, basename, time_str, short_uuid)
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
	pub pending_video: Option<crate::session::video::VideoAttachment>, // Pending video attachment
	pub max_retries: u32,              // Maximum number of retries for provider errors
	pub was_resumed: bool, // Flag indicating if this session was resumed from an existing file

	pub initial_status_shown: bool, // Flag to track if initial status line was displayed
	// Compression hint tracking
	pub compression_hint_count: usize, // Counter for compression hints
	pub last_compression_hint_shown: u64, // Timestamp of last compression hint
	// Token calculation cache - SINGLE SOURCE OF TRUTH for context token counting

	// This cache ensures all systems (display, compression) use identical calculations
	pub cached_tools: Option<Vec<crate::mcp::McpFunction>>, // Cached tool definitions for consistent token counting
	// First user prompt index - compression NEVER goes below this (INCLUSIVE boundary)
	// Set once when first user message is added, protects bootstrap/instructions forever
	pub first_prompt_idx: Option<usize>,
	/// Optional JSON schema for structured output (set via WebSocket/ACP protocol)
	pub schema: Option<serde_json::Value>,
	/// Critical knowledge entries extracted from compressions — persisted across cycles.
	/// Capped at `config.compression.knowledge_retention` entries (FIFO).
	pub critical_knowledge: Vec<String>,
	/// Whether learning lessons have been injected into the system prompt (once per session).
	pub learning_injected: bool,
	/// Whether learning extraction already ran for this session (prevents double extraction on exit).
	pub learning_extracted: bool,
	/// Runtime override for reasoning effort (set via /effort). None = use config default.
	pub reasoning_effort: Option<crate::config::ReasoningEffortConfig>,
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
		// max_retries falls back to config value if not explicitly overridden via CLI
		let max_retries_value = params.max_retries.unwrap_or(params.config.max_retries);

		// Create a new session with initial info
		let timestamp = SystemTime::now()
			.duration_since(UNIX_EPOCH)
			.unwrap_or_default()
			.as_secs();

		let session_info = crate::session::SessionInfo {
			name: params.name.clone(),
			created_at: timestamp,
			model: model_name.clone(),
			role: params.role.to_string(),
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
			pending_video: None,                // Initialize pending video
			max_retries: max_retries_value,     // Set max retries value
			was_resumed: false,                 // This is a new session
			initial_status_shown: false,        // Initialize status display flag
			compression_hint_count: 0,          // Initialize compression hint counter
			last_compression_hint_shown: 0,     // Initialize last hint timestamp
			cached_tools: None,                 // Initialize tool cache (populated on first use)
			first_prompt_idx: None,             // Initialize first prompt index (set on first user message)
			schema: None,                       // Schema set later via CLI override
			critical_knowledge: Vec::new(),     // Populated from session log on resume
			learning_injected: false,
			learning_extracted: false,
			reasoning_effort: None,
		}
	}

	// Initialize a new chat session or load existing one
	pub async fn initialize(params: SessionInitParams<'_>) -> Result<Self> {
		let sessions_dir = get_sessions_dir()?;

		// Handle resume_recent flag
		let effective_resume = if params.resume_recent {
			// Get current working directory - use thread-local if set (ACP/WebSocket), otherwise process cwd
			let current_dir = crate::mcp::get_thread_working_directory();
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

		// Get role config once — used for temperature, top_p, top_k, and optional model override
		let (role_config, _, _, _, _) = params.config.get_role_config(params.role);

		// CLI model overrides role model which overrides global config model
		// Priority: CLI --model > role.model > config.model
		let effective_model = params
			.model
			.clone()
			.or_else(|| role_config.model.clone())
			.unwrap_or_else(|| params.config.get_effective_model());

		// Get temperature from role config if not provided via command line
		let effective_temperature = if let Some(temp) = params.temperature {
			temp // Use command line override
		} else {
			role_config.temperature
		};

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

					// Skip CLI output in structured output modes (websocket, jsonl)
					let suppress = crate::session::output::OutputMode::from_runtime_mode(
						params.output_mode.as_deref().unwrap_or("plain"),
					)
					.should_suppress_cli_output();

					if !suppress {
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
							+ session.info.cache_read_tokens
							+ session.info.cache_write_tokens;

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
					}

					// Create chat session from loaded session
					let restored_model = session.info.model.clone(); // Extract model before moving session
					let restored_cost = session.info.total_cost; // Extract cost before moving session

					// Restore runtime state from session.info
					let cache_next = session.info.cache_next_user_message;
					let spending_checkpoint = session.info.spending_threshold_checkpoint;
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
						pending_video: None,                 // Initialize pending video
						max_retries: params.max_retries.unwrap_or(params.config.max_retries), // Use provided max_retries or fall back to config
						was_resumed: true,          // This session was resumed from file
						initial_status_shown: true, // Don't show status for resumed sessions
						compression_hint_count,     // Restore from session.info
						last_compression_hint_shown: last_compression_hint, // Restore from session.info
						cached_tools: None,         // Initialize tool cache (populated on first use)
						first_prompt_idx: None,     // Will be detected from existing messages
						schema: None,               // Schema applied after init via CLI override
						critical_knowledge: Vec::new(), // Will be restored from session log below
						learning_injected: false,
						learning_extracted: false,
						reasoning_effort: None,
					};
					// Keep session.info.role in sync with the active role
					chat_session.session.info.role = params.role.to_string();

					// Apply runtime state from session log (legacy support)
					if runtime_state.cache_next_message {
						chat_session.cache_next_user_message = true;
					}

					// Apply restored role if available
					if let Some(restored_role) = runtime_state.role {
						// Validate that the restored role still exists in config
						if params.config.roles.iter().any(|r| r.name == restored_role) {
							chat_session.role = restored_role;
							// Update temperature and model from the restored role config
							let (role_config, _, _, _, _) =
								params.config.get_role_config(&chat_session.role);
							chat_session.temperature = role_config.temperature;
							if let Some(role_model) = role_config.model.clone() {
								chat_session.model = role_model;
							}
						}
					}

					// Restore critical knowledge entries from session log
					if !runtime_state.critical_knowledge.is_empty() {
						chat_session.critical_knowledge = runtime_state.critical_knowledge;
						crate::log_debug!(
							"Session resume: Restored {} critical knowledge entries",
							chat_session.critical_knowledge.len()
						);
					}

					// Restore runtime reasoning effort override
					if let Some(effort) = runtime_state.reasoning_effort {
						chat_session.reasoning_effort = Some(effort);
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

					// Detect first_prompt_idx from existing messages.
					// Bootstrap pattern: system[0] → assistant(welcome)[1] → optional user(instructions)[2]
					// The first real user prompt is the first user message after bootstrap.
					{
						let msgs = &chat_session.session.messages;
						let system_idx = msgs.iter().position(|m| m.role == "system").unwrap_or(0);
						let mut idx = system_idx + 1;
						// Skip welcome message (assistant immediately after system, WITHOUT tool_calls)
						let has_welcome = idx < msgs.len()
							&& msgs[idx].role == "assistant"
							&& msgs[idx].tool_calls.is_none();
						if has_welcome {
							idx += 1;
						}
						// Skip instructions file ONLY if welcome was present.
						// Bootstrap: system → assistant(welcome) → user(instructions).
						// Without welcome, the first user message is a real prompt.
						if has_welcome
							&& idx < msgs.len() && msgs[idx].role == "user"
							&& (idx + 1 >= msgs.len() || msgs[idx + 1].role == "assistant")
						{
							idx += 1;
						}
						// idx now points to the first real user prompt (or past end if none)
						if idx < msgs.len() {
							chat_session.first_prompt_idx = Some(idx);
							crate::log_debug!(
								"Session resume: Detected first_prompt_idx={} from message history",
								idx
							);
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

					// Skip CLI output in structured output modes
					let suppress = crate::session::output::OutputMode::from_runtime_mode(
						params.output_mode.as_deref().unwrap_or("plain"),
					)
					.should_suppress_cli_output();
					if !suppress {
						println!(
							"{}",
							format!("Starting new session: {}", new_session_name).bright_green()
						);
					}

					let mut chat_session = ChatSession::new(ChatSessionParams {
						name: new_session_name.clone(),
						model: Some(effective_model.clone()),
						temperature: Some(effective_temperature), // Use config-based temperature
						top_p: Some(effective_top_p),             // Use config-based top_p
						top_k: Some(effective_top_k),             // Use config-based top_k
						max_tokens: Some(effective_max_tokens),   // Use config-based max_tokens
						max_retries: params.max_retries,          // Pass max_retries through
						config: params.config,
						role: params.role, // Add role parameter
					});
					chat_session.session.session_file = Some(new_session_file);

					Ok(chat_session)
				}
			}
		} else {
			// Create new session - skip CLI output in structured output modes
			let suppress = crate::session::output::OutputMode::from_runtime_mode(
				params.output_mode.as_deref().unwrap_or("plain"),
			)
			.should_suppress_cli_output();
			if !suppress {
				use colored::*;
				println!(
					"{}",
					format!("Starting new session: {}", session_name).bright_green()
				);
			}

			let mut chat_session = ChatSession::new(ChatSessionParams {
				name: session_name.clone(),
				model: Some(effective_model),
				temperature: Some(effective_temperature),
				top_p: Some(effective_top_p),
				top_k: Some(effective_top_k),
				max_tokens: Some(effective_max_tokens),
				max_retries: params.max_retries,
				config: params.config,
				role: params.role,
			});
			chat_session.session.session_file = Some(session_file);

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

		if ImageProcessor::is_url(path) {
			println!("{}", "🌐 Downloading image from URL...".bright_cyan());
			let attachment = ImageProcessor::load_from_url(path).await?;
			println!("{}", "📸 Image preview:".bright_cyan());
			ImageProcessor::show_preview(&attachment)?;
			self.pending_image = Some(attachment);
			println!(
				"{}",
				"✅ Image downloaded and ready to attach!".bright_green()
			);
			return Ok(());
		}

		let file_path = Path::new(path);
		if !file_path.exists() {
			return Err(anyhow::anyhow!("Image file not found: {}", path));
		}
		if !ImageProcessor::is_supported_image(file_path) {
			return Err(anyhow::anyhow!(
				"Unsupported image format. Supported: {}",
				ImageProcessor::supported_extensions().join(", ")
			));
		}

		let attachment = ImageProcessor::load_from_path(file_path)?;
		println!("{}", "📸 Image preview:".bright_cyan());
		ImageProcessor::show_preview(&attachment)?;
		self.pending_image = Some(attachment);
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

	/// Attach video from file path
	pub async fn attach_video_from_path(&mut self, path: &str) -> Result<()> {
		use crate::session::video::VideoProcessor;
		use std::path::Path;

		if VideoProcessor::is_url(path) {
			println!("{}", "🌐 Downloading video from URL...".bright_cyan());
			let attachment = VideoProcessor::load_from_url(path).await?;
			println!("{}", "🎬 Video preview:".bright_cyan());
			VideoProcessor::show_preview(&attachment)?;
			self.pending_video = Some(attachment);
			println!(
				"{}",
				"✅ Video downloaded and ready to attach!".bright_green()
			);
			return Ok(());
		}

		let file_path = Path::new(path);
		if !file_path.exists() {
			return Err(anyhow::anyhow!("Video file not found: {}", path));
		}
		if !VideoProcessor::is_supported_video(file_path) {
			return Err(anyhow::anyhow!(
				"Unsupported video format. Supported: {}",
				VideoProcessor::supported_extensions().join(", ")
			));
		}

		let attachment = VideoProcessor::load_from_path(file_path)?;
		println!("{}", "🎬 Video preview:".bright_cyan());
		VideoProcessor::show_preview(&attachment)?;
		self.pending_video = Some(attachment);
		Ok(())
	}

	/// Check if there's a pending video attachment
	pub fn has_pending_video(&self) -> bool {
		self.pending_video.is_some()
	}

	/// Take the pending video (consumes it)
	pub fn take_pending_video(&mut self) -> Option<crate::session::video::VideoAttachment> {
		self.pending_video.take()
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
	/// - had_cached_messages: True if any removed message had cached=true (informational only)
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

		// Reset tool-result dedup state. Any of our placeholders that
		// referenced messages in the just-drained range now point at
		// vanished content; future identical results must be kept verbatim
		// again. Centralized here so every caller that drops messages
		// (task / phase / project / conversation compaction, manual
		// truncate, manual summarize, future paths) gets it for free
		// without needing to remember a separate cleanup call.
		crate::session::dedup::clear_current_session();

		crate::log_debug!(
			"Compressed {} messages (range {}-{}), had_cached={}",
			messages_to_remove,
			start_index,
			end_index,
			had_cached
		);

		Ok((messages_to_remove, had_cached))
	}

	/// Insert compressed knowledge entry as assistant message
	///
	/// This injects a structured summary of completed work into the session history,
	/// replacing the detailed tool calls and intermediate steps.
	///
	/// The compressed block is always marked `cached=true` — it is the new stable
	/// cache boundary for Anthropic's 2-marker system. Any surviving marker at
	/// `index` (start_idx kept message) remains untouched, giving us up to 2 markers.
	///
	/// # Arguments
	/// * `index` - Position to insert after (the kept start_idx message)
	/// * `content` - Formatted summary content
	pub fn insert_compressed_knowledge(&mut self, index: usize, content: String) -> Result<()> {
		use crate::session::Message;

		if index >= self.session.messages.len() {
			return Err(anyhow::anyhow!(
				"Invalid index: {} (total messages: {})",
				index,
				self.session.messages.len()
			));
		}

		// Enforce the 2-marker limit BEFORE inserting the compressed block.
		// Count existing non-system content markers; if already at 2, evict the
		// oldest one so the new compressed block can take its slot.  This prevents
		// exceeding Anthropic's hard limit of 4 cache_control blocks per request
		// (system + tools + 2 content markers).
		// Skip cache marker management entirely when the model does not support caching.
		const MAX_CONTENT_MARKERS: usize = 2;
		let supports_caching = crate::session::model_supports_caching(&self.session.info.model);

		if supports_caching {
			let existing: Vec<usize> = self
				.session
				.messages
				.iter()
				.enumerate()
				.filter(|(_, m)| m.cached && m.role != "system")
				.map(|(i, _)| i)
				.collect();

			if existing.len() >= MAX_CONTENT_MARKERS {
				// Evict the oldest marker to make room for the compressed block.
				if let Some(oldest) = existing.first() {
					if let Some(m) = self.session.messages.get_mut(*oldest) {
						m.cached = false;
					}
				}
			}
		}

		let compressed_msg = Message {
			role: "assistant".to_string(),
			content,
			timestamp: std::time::SystemTime::now()
				.duration_since(std::time::UNIX_EPOCH)
				.unwrap_or_default()
				.as_secs(),
			// The compressed block is the new stable history boundary — cached only
			// when the model actually supports cache markers.
			cached: supports_caching,
			cache_ttl: None,
			tool_call_id: None,
			name: Some("plan_compression".to_string()),
			tool_calls: None,
			images: None,
			videos: None,
			thinking: None,
			id: None,
		};

		self.session.messages.insert(index + 1, compressed_msg);
		let compressed_idx = index + 1;

		crate::log_debug!(
			"Inserted compressed knowledge at index {} (cached={})",
			compressed_idx,
			supports_caching
		);

		// Ensure we always have 2 content markers after compression:
		// marker #1 = compressed block (just inserted above),
		// marker #2 = last eligible user/tool message in the preserved zone.
		// Without this, compression can destroy the second marker leaving only 1,
		// which means the entire preserved tail is sent uncached on the next API call.
		// Skip entirely when the model does not support caching.
		if !supports_caching {
			return Ok(());
		}

		let last_eligible = self
			.session
			.messages
			.iter()
			.enumerate()
			.rev()
			.find(|(i, m)| *i > compressed_idx && (m.role == "user" || m.role == "tool"))
			.map(|(i, _)| i);

		if let Some(target_idx) = last_eligible {
			if !self.session.messages[target_idx].cached {
				let markers: Vec<usize> = self
					.session
					.messages
					.iter()
					.enumerate()
					.filter(|(_, m)| m.cached && m.role != "system")
					.map(|(i, _)| i)
					.collect();

				if markers.len() >= MAX_CONTENT_MARKERS {
					let marker_to_remove = markers
						.iter()
						.copied()
						.find(|i| *i != compressed_idx)
						.or_else(|| markers.first().copied());

					if let Some(index) = marker_to_remove {
						if let Some(m) = self.session.messages.get_mut(index) {
							m.cached = false;
						}
					}
				}

				if let Some(m) = self.session.messages.get_mut(target_idx) {
					m.cached = true;
					crate::log_debug!(
						"Placed second cache marker at index {} (role={})",
						target_idx,
						m.role
					);
				}
			}
		}

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

		// Check if compression is not disabled (no continuation_disabled check needed)

		// Calculate context pressure
		let current_tokens = estimate_full_context_tokens(&self.session.messages, None);
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
		// Get current directory for system prompt processing - use thread-local if set (ACP/WebSocket), otherwise process cwd
		let current_dir = crate::mcp::get_thread_working_directory();
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
				// Replace the system message content.
				// Persistence: the new system prompt is reflected on the next session save
				// (or by the caller's own session-file append logic for mutations).
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
	/// * `config` - Configuration to fetch tools if not cached
	///
	/// # Returns
	/// Total context tokens including messages + system prompt + tool definitions
	pub async fn get_full_context_tokens(&mut self, config: &Config) -> usize {
		// Fetch and cache tools if not already cached
		if self.cached_tools.is_none() {
			self.cached_tools = Some(crate::mcp::get_available_functions(config).await);
		}

		// System prompt is already included in session.messages (first message with role="system")
		// No need to pass it separately - estimate_full_context_tokens counts all messages
		estimate_full_context_tokens(&self.session.messages, self.cached_tools.as_deref())
	}

	/// Invalidate tool cache (call when MCP configuration changes)
	pub fn invalidate_tool_cache(&mut self) {
		self.cached_tools = None;
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::session::{Message, Session, SessionInfo};

	/// Build a minimal ChatSession with the given messages for testing compression primitives.
	/// No config, no async, no providers needed.
	fn make_session(messages: Vec<Message>) -> ChatSession {
		let info = SessionInfo {
			name: "test".to_string(),
			model: "anthropic/claude-3-5-sonnet".to_string(),
			..Default::default()
		};
		ChatSession {
			session: Session {
				info,
				messages,
				session_file: None,
			},
			last_response: String::new(),
			model: "anthropic/claude-3-5-sonnet".to_string(),
			role: "core".to_string(),
			temperature: 0.7,
			top_p: 1.0,
			top_k: 0,
			max_tokens: 4096,
			estimated_cost: 0.0,
			cache_next_user_message: false,
			spending_threshold_checkpoint: 0.0,
			request_spending_checkpoint: 0.0,
			pending_image: None,
			pending_video: None,
			max_retries: 0,

			was_resumed: false,
			initial_status_shown: false,
			compression_hint_count: 0,
			last_compression_hint_shown: 0,
			cached_tools: None,
			first_prompt_idx: None,
			schema: None,
			critical_knowledge: Vec::new(),
			learning_injected: false,
			learning_extracted: false,
			reasoning_effort: None,
		}
	}

	fn msg(role: &str, cached: bool) -> Message {
		Message {
			role: role.to_string(),
			cached,
			..Default::default()
		}
	}

	/// Collect indices of all content-cached messages (user/assistant/tool with cached=true).
	/// System markers are excluded — they are managed separately and never touched by compression.
	fn content_cache_indices(session: &ChatSession) -> Vec<usize> {
		session
			.session
			.messages
			.iter()
			.enumerate()
			.filter(|(_, m)| m.cached && m.role != "system")
			.map(|(i, _)| i)
			.collect()
	}

	// ── Case 1: no cache markers anywhere ────────────────────────────────────────
	// Compressed block gets cached=true (marker #1) and the last eligible user/tool
	// message in the preserved zone gets marker #2 automatically.
	#[test]
	fn case1_no_markers_compressed_block_gets_cached() {
		// idx: 0=system, 1=user(start), 2=assistant, 3=user, 4=assistant(end), 5..8=preserved
		let messages = vec![
			msg("system", false),
			msg("user", false), // start_idx=1 (kept)
			msg("assistant", false),
			msg("user", false),
			msg("assistant", false), // end_idx=4
			msg("user", false),
			msg("assistant", false),
			msg("user", false),
			msg("assistant", false),
		];
		let mut cs = make_session(messages);

		let (_, had_cached) = cs.remove_messages_in_range(1, 4).unwrap();
		assert!(!had_cached);
		cs.insert_compressed_knowledge(1, "summary".to_string())
			.unwrap();

		let markers = content_cache_indices(&cs);
		// After drain+insert: [sys(0), user(1), COMP(2), user(3), asst(4), user(5), asst(6)]
		// Compressed block at idx 2 (marker #1) + last user at idx 5 (marker #2)
		assert_eq!(
			markers.len(),
			2,
			"must have 2 markers: compressed block + last eligible message"
		);
		assert!(markers.contains(&2), "compressed block must be cached");
		assert_eq!(*markers.last().unwrap(), 5, "marker #2 on last user");
	}

	// ── Case 2: one marker inside the range ──────────────────────────────────────
	// Marker destroyed by drain. Compressed block gets marker #1, last eligible
	// message gets marker #2 — always 2 markers after compression.
	#[test]
	fn case2_one_marker_inside_range_compressed_block_gets_cached() {
		// idx: 0=system, 1=user(start), 2=assistant, 3=user(cached!), 4=assistant(end), 5..8=preserved
		let messages = vec![
			msg("system", false),
			msg("user", false), // start_idx=1 (kept)
			msg("assistant", false),
			msg("user", true),       // marker #1 — inside range, will be removed
			msg("assistant", false), // end_idx=4
			msg("user", false),
			msg("assistant", false),
			msg("user", false),
			msg("assistant", false),
		];
		let mut cs = make_session(messages);

		let (_, had_cached) = cs.remove_messages_in_range(1, 4).unwrap();
		assert!(had_cached, "should detect the removed marker");
		cs.insert_compressed_knowledge(1, "summary".to_string())
			.unwrap();

		let markers = content_cache_indices(&cs);
		assert_eq!(
			markers.len(),
			2,
			"must have 2 markers: compressed block + last eligible message"
		);
		assert!(markers.contains(&2), "compressed block must be cached");
	}

	// ── Case 3: two markers both inside the range ─────────────────────────────────
	// Both destroyed by drain. Compressed block gets marker #1, last eligible
	// message gets marker #2 — always 2 markers after compression.
	#[test]
	fn case3_two_markers_inside_range_compressed_block_gets_cached() {
		// idx: 0=system, 1=user(start), 2=user(cached!), 3=assistant, 4=user(cached!), 5=assistant(end), 6..9=preserved
		let messages = vec![
			msg("system", false),
			msg("user", false), // start_idx=1 (kept)
			msg("user", true),  // marker #1 — inside range
			msg("assistant", false),
			msg("user", true),       // marker #2 — inside range
			msg("assistant", false), // end_idx=5
			msg("user", false),
			msg("assistant", false),
			msg("user", false),
			msg("assistant", false),
		];
		let mut cs = make_session(messages);

		let (_, had_cached) = cs.remove_messages_in_range(1, 5).unwrap();
		assert!(had_cached);
		cs.insert_compressed_knowledge(1, "summary".to_string())
			.unwrap();

		let markers = content_cache_indices(&cs);
		// After drain+insert: [sys(0), user(1), COMP(2), user(3), asst(4), user(5), asst(6)]
		assert_eq!(
			markers.len(),
			2,
			"must have 2 markers: compressed block + last eligible message"
		);
		assert!(markers.contains(&2), "compressed block must be cached");
	}

	// ── Case 4: marker at start_idx, one inside range ───────────────────────────
	// start_idx marker survives the drain initially, but is redundant once the
	// compressed block is inserted. The second marker moves to the latest preserved
	// user/tool boundary so the preserved tail remains cached.
	#[test]
	fn case4_marker_at_start_idx_and_one_inside_moves_to_latest_preserved_boundary() {
		// idx: 0=system, 1=user(start,cached!), 2=assistant, 3=user(cached!), 4=assistant(end), 5..8=preserved
		let messages = vec![
			msg("system", false),
			msg("user", true), // start_idx=1, marker #1 (KEPT by drain, later evicted)
			msg("assistant", false),
			msg("user", true),       // marker #2 — inside range, removed
			msg("assistant", false), // end_idx=4
			msg("user", false),
			msg("assistant", false),
			msg("user", false),
			msg("assistant", false),
		];
		let mut cs = make_session(messages);

		let (_, had_cached) = cs.remove_messages_in_range(1, 4).unwrap();
		assert!(had_cached);
		cs.insert_compressed_knowledge(1, "summary".to_string())
			.unwrap();

		let markers = content_cache_indices(&cs);
		// compressed block at idx=2 + latest preserved user at idx=5
		assert_eq!(
			markers,
			vec![2, 5],
			"compressed block + latest preserved boundary are cached"
		);
	}
	// ── Case 5: marker at start_idx only, nothing inside range ───────────────────
	// had_cached=false from remove, but compressed block must still get cached=true.
	// start_idx marker is then evicted so marker #2 can cover the preserved tail.
	#[test]
	fn case5_marker_at_start_idx_only_moves_to_latest_preserved_boundary() {
		// idx: 0=system, 1=user(start,cached!), 2=assistant, 3=user, 4=assistant(end), 5..8=preserved
		let messages = vec![
			msg("system", false),
			msg("user", true), // start_idx=1, marker #1 (KEPT by drain, later evicted)
			msg("assistant", false),
			msg("user", false),
			msg("assistant", false), // end_idx=4
			msg("user", false),
			msg("assistant", false),
			msg("user", false),
			msg("assistant", false),
		];
		let mut cs = make_session(messages);

		let (_, had_cached) = cs.remove_messages_in_range(1, 4).unwrap();
		assert!(!had_cached, "nothing inside range was cached");
		cs.insert_compressed_knowledge(1, "summary".to_string())
			.unwrap();

		let markers = content_cache_indices(&cs);
		assert_eq!(
			markers,
			vec![2, 5],
			"compressed block + latest preserved boundary are cached"
		);
	}

	// ── Case 6: marker in preserved zone (after end_idx) — untouched ─────────────
	// Compression should not disturb markers that are beyond the compressed range.
	#[test]
	fn case6_marker_in_preserved_zone_stays_untouched() {
		// idx: 0=system, 1=user(start), 2=assistant, 3=user(end), 4=user, 5=assistant, 6=user(cached!), 7=assistant
		let messages = vec![
			msg("system", false),
			msg("user", false), // start_idx=1 (kept)
			msg("assistant", false),
			msg("user", false), // end_idx=3
			msg("user", false), // preserved zone starts
			msg("assistant", false),
			msg("user", true), // marker in preserved zone
			msg("assistant", false),
		];
		let mut cs = make_session(messages);

		let (_, had_cached) = cs.remove_messages_in_range(1, 3).unwrap();
		assert!(!had_cached);
		cs.insert_compressed_knowledge(1, "summary".to_string())
			.unwrap();

		let markers = content_cache_indices(&cs);
		// Compressed block at idx 2 (cached=true) + preserved zone marker shifted to idx 5
		assert!(markers.contains(&2), "compressed block must be cached");
		// The preserved zone marker should still exist somewhere after the compressed block
		let preserved_marker_exists = cs.session.messages[3..]
			.iter()
			.any(|m| m.cached && m.role != "system");
		assert!(
			preserved_marker_exists,
			"preserved zone marker must survive untouched"
		);
	}

	// ── Case 7: system marker never touched ──────────────────────────────────────
	// System message cached=true must never be affected by compression.
	#[test]
	fn case7_system_marker_never_touched_by_compression() {
		let messages = vec![
			msg("system", true), // system marker — must never change
			msg("user", false),  // start_idx=1 (kept)
			msg("assistant", false),
			msg("user", true),       // content marker inside range
			msg("assistant", false), // end_idx=4
			msg("user", false),
			msg("assistant", false),
			msg("user", false),
			msg("assistant", false),
		];
		let mut cs = make_session(messages);

		let (_, _) = cs.remove_messages_in_range(1, 4).unwrap();
		cs.insert_compressed_knowledge(1, "summary".to_string())
			.unwrap();

		assert!(
			cs.session.messages[0].cached,
			"system marker must remain cached=true"
		);
	}

	// ── Case 8: two markers already in preserved zone ────────────────────────────
	// This is the bug introduced in commit 659992f: insert_compressed_knowledge
	// unconditionally sets cached=true on the compressed block even when 2 content
	// markers already exist in the preserved zone.  That produces 3 content markers
	// (system + tools + 3 content = 5 cache_control blocks) which Anthropic rejects
	// with "A maximum of 4 blocks with cache_control may be provided. Found 5."
	//
	// Correct behaviour: when 2 content markers already exist outside the compressed
	// range, the compressed block must NOT add a third one.  Instead it should evict
	// the oldest surviving content marker so the total stays at ≤ 2.
	#[test]
	fn case8_two_markers_in_preserved_zone_compressed_block_must_not_exceed_two_content_markers() {
		// idx: 0=system, 1=user(start), 2=assistant, 3=user(end),
		//      4=user(cached!), 5=assistant, 6=user(cached!), 7=assistant  ← preserved zone
		let messages = vec![
			msg("system", false),
			msg("user", false), // start_idx=1 (kept)
			msg("assistant", false),
			msg("user", false), // end_idx=3
			msg("user", true),  // marker #1 in preserved zone
			msg("assistant", false),
			msg("user", true), // marker #2 in preserved zone
			msg("assistant", false),
		];
		let mut cs = make_session(messages);

		let (_, had_cached) = cs.remove_messages_in_range(1, 3).unwrap();
		assert!(!had_cached, "nothing inside range was cached");
		cs.insert_compressed_knowledge(1, "summary".to_string())
			.unwrap();

		// After compression the total number of non-system cached messages must be ≤ 2.
		// Before the fix this was 3 (compressed block + 2 preserved markers), which
		// causes Anthropic to reject the request.
		let markers = content_cache_indices(&cs);
		assert!(
			markers.len() <= 2,
			"must have at most 2 content cache markers after compression, got {}: {:?}",
			markers.len(),
			markers
		);
	}

	// ── Case 9: THE BUG — markers disappear after compression ────────────────────
	// Regression test for the core bug: before the fix, when markers existed inside
	// the compressed range they were destroyed by drain, and insert_compressed_knowledge
	// only placed marker #1 on the compressed block.  Marker #2 was never restored,
	// so the entire preserved zone was sent uncached on the next API call.
	//
	// This test simulates a realistic session: 2 markers exist (one mid-conversation,
	// one near the end), compression removes the range containing the first marker.
	// After compression we MUST have exactly 2 markers:
	//   - marker #1: the compressed block (stable history boundary)
	//   - marker #2: the last eligible user/tool message (moving boundary)
	#[test]
	fn case9_markers_must_not_disappear_after_compression() {
		// Realistic session layout:
		// idx: 0=system, 1=user(start), 2=assistant, 3=user(cached!), 4=assistant,
		//      5=user, 6=assistant(end),
		//      7=user, 8=assistant, 9=user, 10=assistant, 11=user(cached!), 12=assistant
		let messages = vec![
			msg("system", false),
			msg("user", false), // start_idx=1 (anchor, kept)
			msg("assistant", false),
			msg("user", true), // marker #1 — inside compression range
			msg("assistant", false),
			msg("user", false),
			msg("assistant", false), // end_idx=6
			// preserved zone:
			msg("user", false),
			msg("assistant", false),
			msg("user", false),
			msg("assistant", false),
			msg("user", true), // marker #2 — in preserved zone
			msg("assistant", false),
		];
		let mut cs = make_session(messages);

		// Verify pre-compression state: exactly 2 content markers
		let markers_before = content_cache_indices(&cs);
		assert_eq!(
			markers_before.len(),
			2,
			"pre-compression: must have 2 markers, got {:?}",
			markers_before
		);

		// Compress: drain indices 2..=6, insert compressed block
		let (removed, had_cached) = cs.remove_messages_in_range(1, 6).unwrap();
		assert!(removed > 0);
		assert!(had_cached, "marker #1 was inside the range");
		cs.insert_compressed_knowledge(1, "compressed summary".to_string())
			.unwrap();

		// Post-compression: MUST still have exactly 2 content markers
		let markers_after = content_cache_indices(&cs);
		assert_eq!(
			markers_after.len(),
			2,
			"post-compression: must have exactly 2 markers, got {:?}. \
			 BUG: markers disappeared after compression!",
			markers_after
		);

		// Verify marker #1 is the compressed block (always at start_idx+1)
		let compressed_idx = 2; // inserted after start_idx=1
		assert!(
			markers_after.contains(&compressed_idx),
			"marker #1 must be the compressed block at idx {}",
			compressed_idx
		);

		// Verify marker #2 is NOT the compressed block (it's somewhere in preserved zone)
		let marker2 = markers_after
			.iter()
			.find(|&&i| i != compressed_idx)
			.unwrap();
		assert!(
			*marker2 > compressed_idx,
			"marker #2 must be after the compressed block"
		);

		// Verify the message at marker #2 is user or tool (eligible for caching)
		let marker2_msg = &cs.session.messages[*marker2];
		assert!(
			marker2_msg.role == "user" || marker2_msg.role == "tool",
			"marker #2 must be on a user or tool message, got role='{}'",
			marker2_msg.role
		);
	}

	// ── Case 10: no markers before compression — both created fresh ──────────────
	// When a session has never had any cache markers (e.g. caching was disabled or
	// the session is very short), compression must still establish the full 2-marker
	// layout from scratch.
	#[test]
	fn case10_no_markers_before_compression_both_created() {
		let messages = vec![
			msg("system", false),
			msg("user", false), // start_idx=1
			msg("assistant", false),
			msg("user", false),
			msg("assistant", false), // end_idx=4
			msg("user", false),
			msg("assistant", false),
			msg("user", false), // last user — should become marker #2
			msg("assistant", false),
		];
		let mut cs = make_session(messages);

		// Pre-compression: zero markers
		let markers_before = content_cache_indices(&cs);
		assert_eq!(markers_before.len(), 0, "no markers before compression");

		let (_, _) = cs.remove_messages_in_range(1, 4).unwrap();
		cs.insert_compressed_knowledge(1, "summary".to_string())
			.unwrap();

		let markers_after = content_cache_indices(&cs);
		assert_eq!(
			markers_after.len(),
			2,
			"must create both markers from scratch, got {:?}",
			markers_after
		);
	}

	// ── Case 11: marker #2 on tool message in preserved zone ─────────────────────
	// Tool messages are also eligible for marker #2.
	#[test]
	fn case11_marker2_placed_on_tool_message() {
		let messages = vec![
			msg("system", false),
			msg("user", false), // start_idx=1
			msg("assistant", false),
			msg("user", false), // end_idx=3
			// preserved zone:
			msg("user", false),
			msg("assistant", false),
			msg("tool", false), // last eligible — should become marker #2
			msg("assistant", false),
		];
		let mut cs = make_session(messages);

		let (_, _) = cs.remove_messages_in_range(1, 3).unwrap();
		cs.insert_compressed_knowledge(1, "summary".to_string())
			.unwrap();

		let markers = content_cache_indices(&cs);
		assert_eq!(markers.len(), 2, "must have 2 markers, got {:?}", markers);

		// The second marker should be on the tool message
		let last_marker_idx = *markers.last().unwrap();
		assert_eq!(
			cs.session.messages[last_marker_idx].role, "tool",
			"marker #2 should be on the tool message"
		);
	}

	#[test]
	fn case12_compression_moves_second_marker_to_latest_preserved_message() {
		let messages = vec![
			msg("system", false),
			msg("user", false), // start_idx=1
			msg("assistant", false),
			msg("user", false), // end_idx=3
			msg("user", true),  // stale marker in preserved zone
			msg("assistant", false),
			msg("user", false), // freshest eligible — must become marker #2
			msg("assistant", false),
		];
		let mut cs = make_session(messages);

		let (_, _) = cs.remove_messages_in_range(1, 3).unwrap();
		cs.insert_compressed_knowledge(1, "summary".to_string())
			.unwrap();

		let markers = content_cache_indices(&cs);
		assert_eq!(markers.len(), 2, "must have exactly 2 markers");
		assert_eq!(cs.session.messages[markers[0]].content, "summary");
		assert_eq!(markers[1], 5, "marker #2 must move to freshest user");
		assert!(
			!cs.session.messages[3].cached,
			"stale preserved marker must be evicted"
		);
	}

	#[test]
	fn generate_session_name_format() {
		let name = generate_session_name();
		let parts: Vec<&str> = name.split('-').collect();
		assert!(
			parts.len() >= 4,
			"session name should have at least 4 dash-separated parts, got: {name}"
		);

		// First part: YYMMDD (6 digits)
		let date_part = parts[0];
		assert_eq!(
			date_part.len(),
			6,
			"date part should be 6 chars, got: {date_part}"
		);
		assert!(
			date_part.chars().all(|c| c.is_ascii_digit()),
			"date part should be all digits, got: {date_part}"
		);

		// Second part: basename (directory name, non-empty)
		let basename_part = parts[1];
		assert!(!basename_part.is_empty(), "basename should not be empty");

		// Third part: HHMM (4 digits)
		let time_part = parts[2];
		assert_eq!(
			time_part.len(),
			4,
			"time part should be 4 chars, got: {time_part}"
		);
		assert!(
			time_part.chars().all(|c| c.is_ascii_digit()),
			"time part should be all digits, got: {time_part}"
		);

		// Fourth part: uuid4 (4 hex chars)
		let uuid_part = parts[3];
		assert_eq!(
			uuid_part.len(),
			4,
			"uuid part should be 4 chars, got: {uuid_part}"
		);
		assert!(
			uuid_part.chars().all(|c| c.is_ascii_hexdigit()),
			"uuid part should be hex, got: {uuid_part}"
		);
	}
}
