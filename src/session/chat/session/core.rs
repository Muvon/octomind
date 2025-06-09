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
use std::fs::File;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

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
	let short_uuid = &uuid[..8];

	format!("{}-{}-{}-{}", date_str, time_str, basename, short_uuid)
}

// Chat session manager for interactive coding sessions
pub struct ChatSession {
	pub session: Session,
	pub last_response: String,
	pub model: String,
	pub temperature: f32,
	pub estimated_cost: f64,
	pub cache_next_user_message: bool, // Flag to cache the next user message
	pub spending_threshold_checkpoint: f64, // Track spending at last threshold check
}

impl ChatSession {
	// Create a new chat session
	pub fn new(
		name: String,
		model: Option<String>,
		temperature: Option<f32>,
		config: &Config,
	) -> Self {
		let model_name = model.unwrap_or_else(|| config.get_effective_model());
		let temperature_value = temperature.unwrap_or(0.7); // Default to 0.7 instead of 0.2

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
			estimated_cost: 0.0,                // Initialize estimated cost as zero
			cache_next_user_message: false,     // Initialize cache flag
			spending_threshold_checkpoint: 0.0, // Initialize spending checkpoint
		}
	}

	// Initialize a new chat session or load existing one
	pub fn initialize(
		name: Option<String>,
		resume: Option<String>,
		model: Option<String>,
		temperature: Option<f32>,
		config: &Config,
	) -> Result<Self> {
		let sessions_dir = get_sessions_dir()?;

		// Determine session name
		let session_name = if let Some(name_arg) = &name {
			name_arg.clone()
		} else if let Some(resume_name) = &resume {
			resume_name.clone()
		} else {
			// Generate a name using the new format
			generate_session_name()
		};

		let session_file = sessions_dir.join(format!("{}.jsonl", session_name));

		// Check if we should load or create a session
		let should_resume = (resume.is_some() || (name.is_some() && session_file.exists()))
			&& session_file.exists();

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
						model: restored_model, // Use restored model from session
						temperature: 0.2,
						estimated_cost: 0.0,
						cache_next_user_message: false,     // Initialize cache flag
						spending_threshold_checkpoint: 0.0, // Initialize spending checkpoint
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
					// If loading fails, inform the user and create a new session
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
						model.clone(),
						temperature,
						config,
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

			let mut chat_session =
				ChatSession::new(session_name.clone(), model, temperature, config);
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
}
