// Session module for handling interactive coding sessions

mod openrouter; // OpenRouter API client
pub mod chat;       // Chat session logic
mod chat_helper;    // Chat command completion
pub mod mcp;        // MCP protocol support
pub mod layers;         // Layered architecture implementation
mod project_context; // Project context collection and management
mod token_counter;  // Token counting utilities
pub mod logger;     // Request/response logging utilities
mod model_utils;    // Model-specific utility functions
mod helper_functions; // Helper functions for layers and other components
pub mod indexer;    // Indexer integration for sessions

pub use openrouter::*;
pub use mcp::*;
pub use layers::{Layer, LayerConfig, LayerResult, InputMode, process_with_layers};
pub use project_context::ProjectContext;
pub use token_counter::{estimate_tokens, estimate_message_tokens}; // Export token counting functions
pub use model_utils::model_supports_caching;
pub use helper_functions::{get_layer_system_prompt_for_type, process_placeholders, summarize_context};

// Re-export constants
// Constants moved to config

// System prompts for layer types
// This function is now replaced by helper_functions::get_layer_system_prompt_for_type
// It's kept for backward compatibility with existing code
pub fn get_layer_system_prompt(layer_type_str: &str) -> String {
	helper_functions::get_layer_system_prompt_for_type(layer_type_str)
}

use std::fs::{self, OpenOptions, File};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use std::io::{BufRead, BufReader};
use serde::{Serialize, Deserialize};
use std::io::Write;

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
}

fn default_cache_marker() -> bool {
	false
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
}

impl Session {
	// Create a new session
	pub fn new(name: String, model: String, provider: String) -> Self {
		Self {
			info: SessionInfo {
				name,
				created_at: SystemTime::now()
					.duration_since(UNIX_EPOCH)
					.unwrap_or_default()
					.as_secs(),
				model,
				provider,
				input_tokens: 0,
				output_tokens: 0,
				cached_tokens: 0,
				total_cost: 0.0,
				duration_seconds: 0,
				layer_stats: Vec::new(), // Initialize empty layer stats
			},
			messages: Vec::new(),
			session_file: None,
			current_non_cached_tokens: 0,
			current_total_tokens: 0,
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
		};

		self.messages.push(message.clone());
		message
	}

	// Add a cache checkpoint - marks a message as a cache breakpoint
	// By default, it targets the last user message, but system=true targets the system message
	pub fn add_cache_checkpoint(&mut self, system: bool) -> Result<bool, anyhow::Error> {
		// Only user or system messages can be marked as cache breakpoints
		let mut marked = false;

		if system {
			// Find the first system message and mark it
			for msg in self.messages.iter_mut() {
				if msg.role == "system" {
					// Only mark as cached if the model supports it
					msg.cached = crate::session::model_supports_caching(&self.info.model);
					marked = true;
					break;
				}
			}

			// If we couldn't find a system message, return a specific error
			if !marked {
				return Ok(false); // No system message found
			}
		} else {
			// Find the last user message and mark it as a cache breakpoint
			for i in (0..self.messages.len()).rev() {
				let msg = &mut self.messages[i];
				if msg.role == "user" {
					// Only mark as cached if the model supports it
					msg.cached = crate::session::model_supports_caching(&self.info.model);
					marked = true;
					break;
				}
			}
		}

		// Reset token counters when adding a cache checkpoint
		if marked {
			self.current_non_cached_tokens = 0;
			self.current_total_tokens = 0;

			// After adding a cache checkpoint, make sure we explicitly save the state
			// to ensure proper synchronization between cache flags and token tracking
			if let Some(_) = &self.session_file {
				let _ = self.save();
			}
		}

		Ok(marked)
	}

	// Add a cache checkpoint if the token threshold is reached
	// Returns true if a checkpoint was added, false otherwise
	pub fn check_auto_cache_threshold(&mut self, config: &crate::config::Config) -> Result<bool, anyhow::Error> {
		// Check if the threshold is 0 or 100, which disables auto-cache
		let threshold = config.openrouter.cache_tokens_pct_threshold;
		if threshold == 0 || threshold == 100 {
			return Ok(false);
		}

		// If there are no messages or if we haven't tracked any tokens yet, nothing to do
		if self.messages.is_empty() || self.current_total_tokens == 0 {
			return Ok(false);
		}

		// Calculate the percentage of non-cached tokens
		let non_cached_percentage = (self.current_non_cached_tokens as f64 / self.current_total_tokens as f64) * 100.0;

		// Check if we've reached the threshold
		if non_cached_percentage as u8 >= threshold {
			// Add a cache checkpoint at the last user message
			let result = self.add_cache_checkpoint(false);

			// If successful, reset the token counters
			if let Ok(true) = result {
				self.current_non_cached_tokens = 0;
				self.current_total_tokens = 0;

				// After adding a cache checkpoint, make sure state is properly saved
				// This is critical for preventing state inconsistencies
				if let Some(_) = &self.session_file {
					let _ = self.save();
				}
				return Ok(true);
			}
		}

		Ok(false)
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
							fs::rename(temp_path, session_file)?;
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
		fs::create_dir_all(&sessions_dir)?;
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

	for entry in fs::read_dir(sessions_dir)? {
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
				session_info = Some(old_info);
			}
		} else if !line.starts_with("EXCHANGE: ") && !line.is_empty() {
			// Try different formats, prioritizing standard JSONL
			if line.contains("\"role\":") && line.contains("\"content\":") {
				// This looks like a message JSON - try to parse it
				if let Ok(message) = serde_json::from_str::<Message>(&line) {
					messages.push(message);
					continue;
				}
			}

			// Try legacy prefixed formats if JSON parsing fails
			if let Some(content) = line.strip_prefix("SYSTEM: ") {
				if let Ok(message) = serde_json::from_str::<Message>(content) {
					messages.push(message);
				}
			} else if let Some(content) = line.strip_prefix("USER: ") {
				if let Ok(message) = serde_json::from_str::<Message>(content) {
					messages.push(message);
				}
			} else if let Some(content) = line.strip_prefix("ASSISTANT: ") {
				if let Ok(message) = serde_json::from_str::<Message>(content) {
					messages.push(message);
				}
			}
		}
	}

	if let Some(info) = session_info {
		let session = Session {
			info,
			messages,
			session_file: Some(session_file.clone()),
			current_non_cached_tokens: 0,
			current_total_tokens: 0,
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

pub async fn create_system_prompt(project_dir: &PathBuf, config: &crate::config::Config) -> String {
	// If a custom system prompt is defined in the config, use it
	if let Some(custom_prompt) = &config.system {
		return custom_prompt.clone();
	}

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
	if config.mcp.enabled {
		let functions = mcp::get_available_functions(config).await;
		if !functions.is_empty() {
			prompt.push_str("\n\nYou have access to the following tools:");

			for function in &functions {
				prompt.push_str(&format!("\n\n- {} - {}", function.name, function.description));
			}
		}
	}

	prompt
}
