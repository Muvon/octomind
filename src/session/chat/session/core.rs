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
use crate::session::{get_sessions_dir, load_session, Session};
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
	/// Optional model override
	pub model: Option<String>,
	/// Optional temperature override
	pub temperature: Option<f32>,
	/// Optional max tokens override
	pub max_tokens: Option<u32>,
	/// Optional max retries override
	pub max_retries: Option<u32>,
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
			model: None,
			temperature: None,
			max_tokens: None,
			max_retries: None,
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
	pub temperature: f32,
	pub max_tokens: u32,
	pub estimated_cost: f64,
	pub cache_next_user_message: bool, // Flag to cache the next user message
	pub spending_threshold_checkpoint: f64, // Track spending at last threshold check
	pub pending_image: Option<crate::session::image::ImageAttachment>, // Pending image attachment
	pub max_retries: u32,              // Maximum number of retries for provider errors
	pub continuation_pending: bool,    // Flag for session continuation state
}

impl ChatSession {
	// Create a new chat session
	pub fn new(
		name: String,
		model: Option<String>,
		temperature: Option<f32>,
		max_tokens: Option<u32>,
		max_retries: Option<u32>,
		config: &Config,
	) -> Self {
		let model_name = model.unwrap_or_else(|| config.get_effective_model());
		// STRICT: temperature should always be provided from role config, no fallbacks
		let temperature_value = temperature.expect("Temperature must be provided from role config");
		// STRICT: max_tokens should always be provided from role config, no fallbacks
		let max_tokens_value = max_tokens.expect("Max tokens must be provided from role config");
		// max_retries defaults to 0 if not provided (runtime-only parameter)
		let max_retries_value = max_retries.unwrap_or(0);

		// Create a new session with initial info
		let session_info = crate::session::SessionInfo {
			name: name.clone(),
			created_at: SystemTime::now()
				.duration_since(UNIX_EPOCH)
				.unwrap_or_default()
				.as_secs(),
			model: model_name.clone(),
			provider: "openrouter".to_string(),
			input_tokens: 0,
			output_tokens: 0,
			cached_tokens: 0,
			total_cost: 0.0,
			duration_seconds: 0,
			layer_stats: Vec::new(), // Initialize empty layer stats
			tool_calls: 0,           // Initialize tool call counter
			// Initialize time tracking fields
			total_api_time_ms: 0,
			total_tool_time_ms: 0,
			total_layer_time_ms: 0,
		};

		Self {
			session: Session {
				info: session_info,
				messages: Vec::new(),
				session_file: None,
				current_non_cached_tokens: 0,
				current_total_tokens: 0,
				last_cache_checkpoint_time: SystemTime::now()
					.duration_since(UNIX_EPOCH)
					.unwrap_or_default()
					.as_secs(),
			},
			last_response: String::new(),
			model: model_name,
			temperature: temperature_value,     // Use the provided temperature
			max_tokens: max_tokens_value,       // Use the provided max_tokens
			estimated_cost: 0.0,                // Initialize estimated cost as zero
			cache_next_user_message: false,     // Initialize cache flag
			spending_threshold_checkpoint: 0.0, // Initialize spending checkpoint
			pending_image: None,                // Initialize pending image
			max_retries: max_retries_value,     // Set max retries value
			continuation_pending: false,        // Initialize continuation state
		}
	}

	// Initialize a new chat session or load existing one
	pub fn initialize(params: SessionInitParams<'_>) -> Result<Self> {
		let sessions_dir = get_sessions_dir()?;

		// Determine session name
		let session_name = if let Some(name_arg) = &params.name {
			name_arg.clone()
		} else if let Some(resume_name) = &params.resume {
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

		// Get max_tokens from root config if not provided via command line
		let effective_max_tokens = if let Some(tokens) = params.max_tokens {
			tokens // Use command line override
		} else {
			// Read from root configuration - STRICT: assume it exists
			params.config.get_effective_max_tokens()
		};

		// Check if we should load or create a session
		let should_resume = if params.resume.is_some() {
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
					let mut chat_session = ChatSession {
						session,
						last_response: String::new(),
						model: restored_model,              // Use restored model from session
						temperature: effective_temperature, // Use config-based temperature
						max_tokens: effective_max_tokens,   // Use config-based max_tokens
						estimated_cost: 0.0,
						cache_next_user_message: false,     // Initialize cache flag
						spending_threshold_checkpoint: 0.0, // Initialize spending checkpoint
						pending_image: None,                // Initialize pending image
						max_retries: params.max_retries.unwrap_or(0), // Use provided max_retries or default to 0
						continuation_pending: false,        // Initialize continuation state
					};

					// Update the estimated cost from the loaded session
					chat_session.estimated_cost = chat_session.session.info.total_cost;
					// Initialize spending threshold checkpoint for loaded sessions
					chat_session.spending_threshold_checkpoint = 0.0;

					// Apply runtime state from session log
					chat_session.cache_next_user_message = runtime_state.cache_next_message;

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

					println!(
						"{}",
						format!("Starting new session: {}", new_session_name).bright_green()
					);

					// Create file if it doesn't exist
					if !new_session_file.exists() {
						let file = File::create(&new_session_file)?;
						drop(file);
					}

					let mut chat_session = ChatSession::new(
						new_session_name.clone(),
						params.model.clone(),
						Some(effective_temperature), // Use config-based temperature
						Some(effective_max_tokens),  // Use config-based max_tokens
						params.max_retries,          // Pass max_retries through
						params.config,
					);
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
			// Create new session
			use colored::*;
			println!(
				"{}",
				format!("Starting new session: {}", session_name).bright_green()
			);

			// Create session file if it doesn't exist
			if !session_file.exists() {
				let file = File::create(&session_file)?;
				drop(file);
			}

			let mut chat_session = ChatSession::new(
				session_name.clone(),
				params.model,
				Some(effective_temperature),
				Some(effective_max_tokens),
				params.max_retries,
				params.config,
			);
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
	) -> Result<bool> {
		super::commands::process_command(self, input, config, role).await
	}
}
