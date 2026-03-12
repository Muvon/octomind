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
	pub pending_video: Option<crate::session::video::VideoAttachment>, // Pending video attachment
	pub max_retries: u32,              // Maximum number of retries for provider errors
	pub was_resumed: bool, // Flag indicating if this session was resumed from an existing file

	pub pending_prompt: Option<String>, // Pending prompt text to be processed as user input
	pub initial_status_shown: bool,     // Flag to track if initial status line was displayed
	// Compression hint tracking
	pub compression_hint_count: usize, // Counter for compression hints
	pub last_compression_hint_shown: u64, // Timestamp of last compression hint
	// Token calculation cache - SINGLE SOURCE OF TRUTH for context token counting

	// This cache ensures all systems (display, compression) use identical calculations
	pub cached_tools: Option<Vec<crate::mcp::McpFunction>>, // Cached tool definitions for consistent token counting
	// First user prompt index - compression NEVER goes below this (INCLUSIVE boundary)
	// Set once when first user message is added, protects bootstrap/instructions forever
	pub first_prompt_idx: Option<usize>,
	/// Optional JSON schema for structured output (set via --schema CLI flag)
	pub schema: Option<serde_json::Value>,
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
			provider: "openrouter".to_string(),
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
			pending_prompt: None,               // Initialize pending prompt
			initial_status_shown: false,        // Initialize status display flag
			compression_hint_count: 0,          // Initialize compression hint counter
			last_compression_hint_shown: 0,     // Initialize last hint timestamp
			cached_tools: None,                 // Initialize tool cache (populated on first use)
			first_prompt_idx: None,             // Initialize first prompt index (set on first user message)
			schema: None,                       // Schema set later via CLI override
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
						pending_prompt: None,       // Initialize pending prompt
						initial_status_shown: true, // Don't show status for resumed sessions
						compression_hint_count,     // Restore from session.info
						last_compression_hint_shown: last_compression_hint, // Restore from session.info
						cached_tools: None,         // Initialize tool cache (populated on first use)
						first_prompt_idx: None,     // Will be detected from existing messages
						schema: None,               // Schema applied after init via CLI override
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

	/// Attach video from file path
	pub async fn attach_video_from_path(&mut self, path: &str) -> Result<()> {
		use crate::session::video::VideoProcessor;
		use std::path::Path;

		// Check if input is a URL
		if VideoProcessor::is_url(path) {
			println!("{}", "🌐 Downloading video from URL...".bright_cyan());

			let video_attachment = VideoProcessor::load_from_url(path).await?;

			// Show preview
			println!("{}", "🎬 Video preview:".bright_cyan());
			VideoProcessor::show_preview(&video_attachment)?;

			// Store for next message
			self.pending_video = Some(video_attachment);

			println!(
				"{}",
				"✅ Video downloaded and ready to attach!".bright_green()
			);
			return Ok(());
		}

		// Handle as file path
		let video_path = Path::new(path);

		// Check if file exists
		if !video_path.exists() {
			return Err(anyhow::anyhow!("Video file not found: {}", path));
		}

		// Check if it's a supported video format
		if !VideoProcessor::is_supported_video(video_path) {
			return Err(anyhow::anyhow!(
				"Unsupported video format. Supported: {}",
				VideoProcessor::supported_extensions().join(", ")
			));
		}

		// Load and process the video
		let video_attachment = VideoProcessor::load_from_path(video_path)?;

		// Show preview
		println!("{}", "🎬 Video preview:".bright_cyan());
		VideoProcessor::show_preview(&video_attachment)?;

		// Store for next message
		self.pending_video = Some(video_attachment);

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
		const MAX_CONTENT_MARKERS: usize = 2;
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

		let compressed_msg = Message {
			role: "assistant".to_string(),
			content,
			timestamp: std::time::SystemTime::now()
				.duration_since(std::time::UNIX_EPOCH)
				.unwrap_or_default()
				.as_secs(),
			// The compressed block is the new stable history boundary — always cached.
			// After the eviction above, total content markers stays at ≤ 2.
			cached: true,
			tool_call_id: None,
			name: Some("plan_compression".to_string()),
			tool_calls: None,
			images: None,
			videos: None,
			thinking: None,
			id: None,
		};

		self.session.messages.insert(index + 1, compressed_msg);

		crate::log_debug!(
			"Inserted compressed knowledge at index {} (cached=true)",
			index + 1
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
			model: "claude-3-5-sonnet".to_string(),
			..Default::default()
		};
		ChatSession {
			session: Session {
				info,
				messages,
				session_file: None,
			},
			last_response: String::new(),
			model: "claude-3-5-sonnet".to_string(),
			role: "developer".to_string(),
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
			pending_prompt: None,
			initial_status_shown: false,
			compression_hint_count: 0,
			last_compression_hint_shown: 0,
			cached_tools: None,
			first_prompt_idx: None,
			schema: None,
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
	// Compressed block must always get cached=true (new stable boundary).
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
		// Compressed block is at idx 2 (inserted after start_idx=1)
		assert_eq!(
			markers,
			vec![2],
			"compressed block must always be cached=true"
		);
	}

	// ── Case 2: one marker inside the range ──────────────────────────────────────
	// Marker is removed with the range. Compressed block should get cached=true.
	// No second marker needed — preserved zone will get one via normal auto-cache.
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
		assert_eq!(markers, vec![2], "compressed block must be cached=true");
	}

	// ── Case 3: two markers both inside the range ─────────────────────────────────
	// Both removed. Compressed block gets cached=true — only 1 marker needed here;
	// the second slot will be filled by normal auto-cache as the session progresses.
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
		// Compressed block at idx 2. Only 1 marker — that's correct.
		// (The second slot will be filled by normal auto-cache as session progresses.)
		assert_eq!(markers, vec![2], "compressed block must be cached=true");
	}

	// ── Case 4: marker at start_idx (kept), one inside range ─────────────────────
	// start_idx marker survives the drain (it is the kept message, not in drain range).
	// Compressed block inserted after it gets cached=true → ideal 2-marker layout.
	#[test]
	fn case4_marker_at_start_idx_and_one_inside_both_survive_correctly() {
		// idx: 0=system, 1=user(start,cached!), 2=assistant, 3=user(cached!), 4=assistant(end), 5..8=preserved
		let messages = vec![
			msg("system", false),
			msg("user", true), // start_idx=1, marker #1 (KEPT — not in drain range)
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
		// start_idx=1 (user, cached=true) + compressed block at idx=2 (cached=true)
		assert_eq!(
			markers,
			vec![1, 2],
			"start_idx marker survives + compressed block gets cached"
		);
	}
	// ── Case 5: marker at start_idx only, nothing inside range ───────────────────
	// had_cached=false from remove, but compressed block must still get cached=true.
	// Result: start_idx keeps marker (#1), compressed block gets marker (#2).
	#[test]
	fn case5_marker_at_start_idx_only_compressed_block_must_get_cached() {
		// idx: 0=system, 1=user(start,cached!), 2=assistant, 3=user, 4=assistant(end), 5..8=preserved
		let messages = vec![
			msg("system", false),
			msg("user", true), // start_idx=1, marker #1 (KEPT)
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
		// BUG (current): insert with had_cached=false → compressed block cached=false
		// CORRECT: always cached=true
		cs.insert_compressed_knowledge(1, "summary".to_string())
			.unwrap();

		let markers = content_cache_indices(&cs);
		assert_eq!(
			markers,
			vec![1, 2],
			"start_idx marker + compressed block both cached"
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
}
