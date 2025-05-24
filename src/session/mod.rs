// Session module for handling interactive coding sessions

mod openrouter; // Legacy OpenRouter API client (for backward compatibility)
pub mod providers; // Provider abstraction layer
pub mod chat;       // Chat session logic
mod chat_helper;    // Chat command completion
pub mod layers;         // Layered architecture implementation
mod project_context; // Project context collection and management
mod token_counter;  // Token counting utilities
pub mod logger;     // Request/response logging utilities
mod model_utils;    // Model-specific utility functions
mod helper_functions; // Helper functions for layers and other components
pub mod indexer;    // Indexer integration for sessions
pub mod cache;      // Comprehensive caching system

// Legacy exports for backward compatibility
pub use openrouter::*;
// New provider system exports
pub use providers::{ProviderFactory, AiProvider, ProviderResponse, ProviderExchange, TokenUsage};
pub use layers::{Layer, LayerConfig, LayerResult, InputMode, process_with_layers};
pub use project_context::ProjectContext;
pub use token_counter::{estimate_tokens, estimate_message_tokens}; // Export token counting functions
pub use model_utils::model_supports_caching;
pub use helper_functions::{get_layer_system_prompt_for_type, process_placeholders, summarize_context};
pub use cache::{CacheManager, CacheStatistics}; // Export cache management

// Re-export constants
// Constants moved to config

// System prompts for layer types
// This function is now replaced by helper_functions::get_layer_system_prompt_for_type
// It's kept for backward compatibility with existing code
pub fn get_layer_system_prompt(layer_type_str: &str) -> String {
	helper_functions::get_layer_system_prompt_for_type(layer_type_str)
}

use std::fs::{self as std_fs, OpenOptions, File};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use std::io::{BufRead, BufReader};
use serde::{Serialize, Deserialize};
use std::io::Write;
use anyhow::Result;
use crate::config::Config;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Message {
	pub role: String,
	pub content: String,
	pub timestamp: u64,
	#[serde(default = "default_cache_marker")]
	pub cached: bool,  // Marks if this message is a cache breakpoint
	#[serde(skip_serializing_if = "Option::is_none")]
	pub tool_call_id: Option<String>, // For tool messages: the ID of the tool call
	#[serde(skip_serializing_if = "Option::is_none")]
	pub name: Option<String>, // For tool messages: the name of the tool
	#[serde(skip_serializing_if = "Option::is_none")]
	pub tool_calls: Option<serde_json::Value>, // For assistant messages: original tool calls from API response
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

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SessionInfo {
	pub name: String,
	pub created_at: u64,
	pub model: String,
	pub provider: String,
	pub input_tokens: u64,
	pub output_tokens: u64,
	pub cached_tokens: u64,  // Added to track cached tokens separately
	pub total_cost: f64,
	pub duration_seconds: u64,
	pub layer_stats: Vec<LayerStats>, // Added to track per-layer statistics
	#[serde(default)]
	pub tool_calls: u64, // Track total number of tool calls made
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct LayerStats {
	pub layer_type: String,
	pub model: String,
	pub input_tokens: u64,
	pub output_tokens: u64,
	pub cost: f64,
	pub timestamp: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Session {
	pub info: SessionInfo,
	pub messages: Vec<Message>,
	pub session_file: Option<PathBuf>,
	// Track token counts for non-cached messages in current interaction
	pub current_non_cached_tokens: u64,
	pub current_total_tokens: u64,
	// Track last cache checkpoint time for time-based auto-caching
	#[serde(default = "current_timestamp")]
	pub last_cache_checkpoint_time: u64,
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
				cached_tokens: 0,
				total_cost: 0.0,
				duration_seconds: 0,
				layer_stats: Vec::new(), // Initialize empty layer stats
				tool_calls: 0, // Initialize tool call counter
			},
			messages: Vec::new(),
			session_file: None,
			current_non_cached_tokens: 0,
			current_total_tokens: 0,
			last_cache_checkpoint_time: timestamp,
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
			cached: false,  // Default to not cached
			tool_call_id: None, // Default to no tool_call_id
			name: None, // Default to no name
			tool_calls: None, // Default to no tool_calls
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
						self.current_non_cached_tokens = 0;
						self.current_total_tokens = 0;
						return Ok(true);
					}
					return Ok(false);
				}
			}
			// If we couldn't find a system message, return false
			Ok(false)
		} else {
			// For content cache markers, direct users to use CacheManager
			Err(anyhow::anyhow!("Use CacheManager for content cache markers instead of add_cache_checkpoint"))
		}
	}

	// Add statistics for a specific layer
	pub fn add_layer_stats(&mut self,
		layer_type: &str,
		model: &str,
		input_tokens: u64,
		output_tokens: u64,
		cost: f64
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
		};

		// Add to the session info
		self.info.layer_stats.push(stats);

		// Also update the overall session totals
		self.info.input_tokens += input_tokens;
		self.info.output_tokens += output_tokens;
		self.info.total_cost += cost;
	}

	// Save the session to a file - optimized for efficiency
	pub fn save(&self) -> Result<(), anyhow::Error> {
		if let Some(session_file) = &self.session_file {
			// Check if the file already exists - if not, create it
			let create_new = !session_file.exists();

			if create_new {
				// Create the file and write all messages
				let _ = File::create(session_file)?;

				// Save session info as the first line (summary)
				let info_json = serde_json::to_string(&self.info)?;
				append_to_session_file(session_file, &format!("SUMMARY: {}", info_json))?;

				// Save all messages without prefixes - simpler format
				for message in &self.messages {
					let message_json = serde_json::to_string(message)?;
					append_to_session_file(session_file, &message_json)?;
				}
			} else {
				// Optimized approach - only overwrite if the file structure needs to change
				// Read the first line to check if summary needs updating
				let mut needs_full_rewrite = true;

				if let Ok(file) = File::open(session_file) {
					let reader = BufReader::new(file);
					if let Some(Ok(first_line)) = reader.lines().next() {
						if first_line.starts_with("SUMMARY: ") {
							let info_json = serde_json::to_string(&self.info)?;
							let summary_line = format!("SUMMARY: {}", info_json);

							// We can just update the first line and append messages
							needs_full_rewrite = false;

							// Create a temporary file with the updated summary
							let temp_path = session_file.with_extension("jsonl.tmp");
							let mut temp_file = File::create(&temp_path)?;
							writeln!(temp_file, "{}\r", summary_line)?;

							// Copy all lines except the first from the original file
							let file = File::open(session_file)?;
							let reader = BufReader::new(file);
							let mut lines = reader.lines();
							let _ = lines.next(); // Skip the first line (old summary)

							// Count messages from file for efficient comparison
							let mut message_count = 0;
							for line in lines {
								if let Ok(line) = line {
									if !line.is_empty() && !line.starts_with("EXCHANGE: ") {
										message_count += 1;
										writeln!(temp_file, "{}\r", line)?;
									}
								}
							}

							// Append any new messages that weren't in the file
							if self.messages.len() > message_count {
								for message in &self.messages[message_count..] {
									let message_json = serde_json::to_string(message)?;
									writeln!(temp_file, "{}\r", message_json)?;
								}
							}

							// Replace the original file with the temporary file
							std_fs::rename(temp_path, session_file)?;
						}
					}
				}

				// If we need to rewrite the whole file, do so
				if needs_full_rewrite {
					let _ = File::create(session_file)?;

					// Save session info as the first line (summary)
					let info_json = serde_json::to_string(&self.info)?;
					append_to_session_file(session_file, &format!("SUMMARY: {}", info_json))?;

					// Save all messages without prefixes - simpler format
					for message in &self.messages {
						let message_json = serde_json::to_string(message)?;
						append_to_session_file(session_file, &message_json)?;
					}
				}
			}

			Ok(())
		} else {
			Err(anyhow::anyhow!("No session file specified"))
		}
	}
}

// Get sessions directory path
pub fn get_sessions_dir() -> Result<PathBuf, anyhow::Error> {
	let current_dir = std::env::current_dir()?;
	let octodev_dir = current_dir.join(".octodev");
	let sessions_dir = octodev_dir.join("sessions");

	if !sessions_dir.exists() {
		std_fs::create_dir_all(&sessions_dir)?;
	}

	Ok(sessions_dir)
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

		if path.is_file() && path.extension().map_or(false, |ext| ext == "jsonl") {
			// Read just the first line to get session info
			if let Ok(file) = File::open(&path) {
				let reader = BufReader::new(file);
				let first_line = reader.lines().next();

				if let Some(Ok(line)) = first_line {
					if let Some(content) = line.strip_prefix("SUMMARY: ") {
						if let Ok(info) = serde_json::from_str::<SessionInfo>(content) {
							let name = path.file_stem()
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
	let mut messages = Vec::new();
	let mut restoration_point_found = false;
	let mut restoration_messages = Vec::new();

	// Process the file line by line to avoid loading the entire file into memory
	for line in reader.lines() {
		let line = line?;

		if line.starts_with("SUMMARY: ") {
			// Parse session info (from first line)
			if let Some(content) = line.strip_prefix("SUMMARY: ") {
				session_info = Some(serde_json::from_str(content)?);
			}
		} else if line.starts_with("INFO: ") {
			// Parse old session info format for backward compatibility
			if let Some(content) = line.strip_prefix("INFO: ") {
				let mut old_info: SessionInfo = serde_json::from_str(content)?;
				// Add the new fields for token tracking
				old_info.input_tokens = 0;
				old_info.output_tokens = 0;
				old_info.cached_tokens = 0;  // Initialize new cached_tokens field
				old_info.total_cost = 0.0;
				old_info.duration_seconds = 0;
				old_info.layer_stats = Vec::new(); // Initialize empty layer stats
				old_info.tool_calls = 0; // Initialize tool call counter
				session_info = Some(old_info);
			}
		} else if line.starts_with("RESTORATION_POINT: ") {
			// Found a restoration point - this means the session was optimized with /done
			// We should restore from this point instead of loading all messages
			restoration_point_found = true;
			// Clear messages collected so far and start fresh from restoration point
			messages.clear();
			restoration_messages.clear();
			// Continue processing to find messages after this restoration point
		} else if !line.starts_with("EXCHANGE: ") && !line.is_empty() {
			// Try different formats, prioritizing standard JSONL
			if line.contains("\"role\":") && line.contains("\"content\":") {
				// This looks like a message JSON - try to parse it
				if let Ok(message) = serde_json::from_str::<Message>(&line) {
					if restoration_point_found {
						restoration_messages.push(message);
					} else {
						messages.push(message);
					}
					continue;
				}
			}

			// Try legacy prefixed formats if JSON parsing fails
			if let Some(content) = line.strip_prefix("SYSTEM: ") {
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

	// Use restoration messages if we found a restoration point, otherwise use all messages
	let final_messages = if restoration_point_found && !restoration_messages.is_empty() {
		restoration_messages
	} else {
		messages
	};

	if let Some(info) = session_info {
		let session = Session {
			info,
			messages: final_messages,
			session_file: Some(session_file.clone()),
			current_non_cached_tokens: 0,
			current_total_tokens: 0,
			last_cache_checkpoint_time: current_timestamp(), // Initialize to current time for existing sessions
		};
		Ok(session)
	} else {
		Err(anyhow::anyhow!("Invalid session file: missing session info"))
	}
}

// Helper function to append to session file with optimized line endings
pub fn append_to_session_file(session_file: &PathBuf, content: &str) -> Result<(), anyhow::Error> {
	let mut file = OpenOptions::new()
		.create(true)
		.append(true)
		.open(session_file)?;

	// Use a consistent line ending regardless of platform
	writeln!(file, "{}\n", content)?;
	Ok(())
}

pub async fn create_system_prompt(project_dir: &PathBuf, config: &crate::config::Config, mode: &str) -> String {
	// Get mode-specific configuration
	let (_, mcp_config, _, custom_system) = config.get_mode_config(mode);

	// If a custom system prompt is defined for this mode, use it
	if let Some(custom_prompt) = custom_system {
		return custom_prompt.clone();
	}

	// For chat mode, use a simple system prompt
	if mode == "chat" {
		return "You are a helpful assistant.".to_string();
	}

	// For agent mode (default), build the complex system prompt with project context
	// Collect project context information (README.md, CHANGES.md, git info, file tree)
	let project_context = ProjectContext::collect(project_dir);

	// Build the base system prompt
	let mut prompt = format!(
		"You are an Octodev – top notch fully autonomous AI developer.\n\
			Current working dir: {}\n\
			**DEVELOPMENT APPROACH:**\n\
			1. Analyze problems thoroughly first\n\
			2. Think through solutions step-by-step\n\
			3. Execute necessary changes directly using available tools\n\
			4. Test your implementations when possible\n\n\
			**CODE QUALITY GUIDELINES:**\n\
			• Provide validated, working solutions\n\
			• Keep code clear and concise\n\
			• Focus on practical solutions and industry best practices\n\
			• Avoid unnecessary abstractions - solve problems directly\n\
			• Balance file size and readability\n\
			• Don't over-fragment code across multiple files\n\n\
			**MISSING CONTEXT COLLECTION CHECKLIST:**\n\
			1. Examine key project files to understand the codebase structure \
			2. Use semantic_code view to understand interfaces and code signatures \
			2. If needed, use semantic_code search for relevant implementation patterns \
			3. As a last resort, use text_editor to view specific file contents \
			**WHEN WORKING WITH FILES:**\n\
			1. First understand which files you need to read/write\n\
			2. Process files efficiently, preferably in a single operation\n\
			3. Utilize the provided tools proactively without asking if you should use them\n\n\
			Right now you are *NOT* in the chat only mode and have access to tool use and system.",
		project_dir.display()
	);

	// Add Project Context Information
	let context_info = project_context.format_for_prompt();
	if !context_info.is_empty() {
		prompt.push_str("\n\n==== PROJECT CONTEXT ====\n\n");
		prompt.push_str(&context_info);
		prompt.push_str("\n\n==== END PROJECT CONTEXT ====\n");
	}

	// Add MCP tools information if enabled
	if mcp_config.enabled {
		let mode_config = config.get_merged_config_for_mode(mode);
		let functions = crate::mcp::get_available_functions(&mode_config).await;
		if !functions.is_empty() {
			prompt.push_str("\n\nYou have access to the following tools:");

			for function in &functions {
				prompt.push_str(&format!("\n\n- {} - {}", function.name, function.description));
			}
		}
	}

	prompt
}

/// High-level function to send a chat completion using the provider abstraction
/// This function handles model parsing and provider selection automatically
pub async fn chat_completion_with_provider(
	messages: &[Message],
	model: &str,
	temperature: f32,
	config: &Config,
) -> Result<ProviderResponse> {
	// Parse the model string and get the appropriate provider
	let (provider, actual_model) = ProviderFactory::get_provider_for_model(model)?;

	// Call the provider's chat completion method
	provider.chat_completion(messages, &actual_model, temperature, config).await
}
