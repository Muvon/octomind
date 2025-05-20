// Session module for handling interactive coding sessions

mod openrouter; // OpenRouter API client
pub mod chat;       // Chat session logic
mod chat_helper;    // Chat command completion
pub mod mcp;        // MCP protocol support
mod layers;         // Layered architecture implementation

pub use openrouter::*;
pub use mcp::*;
pub use layers::{LayerType, LayerConfig, LayerResult, Layer, process_with_layers};

// Re-export constants
// Constants moved to config

// System prompts for layer types
pub fn get_layer_system_prompt(layer_type: layers::LayerType) -> String {
	match layer_type {
		layers::LayerType::QueryProcessor => {
			"You are an expert query processor and requirement analyst in the OctoDev system. \
				Your only job is to analyze the user's request and return an improved, clarified version of the task. \
				Transform vague or ambiguous requests into specific, actionable instructions. \
				Identify unstated requirements, technical constraints, and implementation details that would be needed. \
				Structure the output as a clear set of development tasks or requirements. \
				Include relevant technical specifics, edge cases to handle, and success criteria when possible. \
				DO NOT use tools or explore the codebase - that will be done in a later stage. \
				Return only the refined task description that clearly explains what needs to be done.".to_string()
		},
		layers::LayerType::ContextGenerator => {
			"You are the context gathering specialist for the OctoDev system. \
				\
				Your primary responsibilities are to: \
				1. Take the original query and the improved instructions from the query processor \
				2. Identify ALL information needed for task resolution \
				3. Methodically gather relevant context through available tools \
				4. Construct a comprehensive context package that will be provided to the developer \
				\
				CONTEXT IDENTIFICATION PROCESS: \
				- Determine the programming language, frameworks, and technologies involved \
				- Identify relevant files, classes, functions, configurations, or dependencies \
				- Consider what implementation patterns or architectural decisions may impact the solution \
				- Assess if environment configuration, build settings, or runtime details are relevant \
				\
				INFORMATION GATHERING GUIDELINES: \
				- USE TOOLS to explore the codebase and gather information \
				- Always check for existing implementations of similar functionality in the codebase \
				- Retrieve complete file contents when structure or relationships are important \
				- For large files, focus on the most relevant sections (class definitions, function signatures) \
				- Collect documentation, READMEs, or comments that explain design decisions \
				- When imports or dependencies are referenced, fetch their definitions if needed \
				\
				Your output should be a well-organized collection of context information that the developer can use to solve the task. \
				Begin your response with the refined task from the query processor, then include all the relevant context you've gathered.".to_string()
		},
		layers::LayerType::Developer => {
			"You are OctoDev's core developer AI. You are responsible for implementing the requested changes and providing solutions. \
				\
				You will receive: \
				1. A processed query with clear instructions on what needs to be done \
				2. Context information gathered by the context generator \
				\
				Your job is to: \
				1. Understand the task and context thoroughly \
				2. Execute the necessary actions using tools to complete the task \
				3. If the context is missing anything, use tools to gather additional information as needed \
				4. Provide clear explanations of your work and reasoning \
				5. Update documentation (README.md, CHANGES.md) when appropriate \
				6. Suggest next steps or improvements when relevant \
				\
				Your output should include: \
				- A summary of what you understood from the task \
				- Description of the changes you've implemented \
				- Code snippets or file changes you've made \
				- Explanations of your implementation choices \
				- Documentation updates \
				- Suggestions for next steps \
				\
				Maintain a clear view of the full system architecture even when working on specific components.".to_string()
		},
		layers::LayerType::Reducer => {
			"You are the session optimizer for OctoDev, responsible for consolidating information and preparing for the next interaction. \
				\
				Your responsibilities: \
				1. Review the original request and the developer's solution \
				2. Ensure documentation (README.md and CHANGES.md) is properly updated \
				3. Create a concise summary of the work that was done \
				4. Condense the context in a way that preserves essential information for future requests \
				\
				This condensed information will be cached to reduce token usage in the next iteration. \
				Focus on extracting the most important technical details while removing unnecessary verbosity. \
				Your output will be used as context for the next user interaction, so it must contain all essential information \
				while being as concise as possible.".to_string()
		},
	}
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
					msg.cached = true;
					marked = true;
					break;
				}
			}
		} else {
			// Find the last user message and mark it as a cache breakpoint
			for i in (0..self.messages.len()).rev() {
				let msg = &mut self.messages[i];
				if msg.role == "user" {
					msg.cached = true;
					marked = true;
					break;
				}
			}
		}

		Ok(marked)
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

	// Save the session to a file
	pub fn save(&self) -> Result<(), anyhow::Error> {
		if let Some(session_file) = &self.session_file {
			// Clear the file first
			let _ = File::create(session_file)?;

			// Save session info as the first line (summary)
			let info_json = serde_json::to_string(&self.info)?;
			append_to_session_file(session_file, &format!("SUMMARY: {}", info_json))?;

			// Save all messages without prefixes - simpler format
			for message in &self.messages {
				let message_json = serde_json::to_string(message)?;
				append_to_session_file(session_file, &message_json)?;
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

// Helper function to load a session from file
pub fn load_session(session_file: &PathBuf) -> Result<Session, anyhow::Error> {
	let content = fs::read_to_string(session_file)?;
	let mut session_info: Option<SessionInfo> = None;
	let mut messages = Vec::new();

	for line in content.lines() {
		if let Some(content) = line.strip_prefix("SUMMARY: ") {
			// Parse session info (from first line)
			session_info = Some(serde_json::from_str(content)?);
		} else if let Some(content) = line.strip_prefix("INFO: ") {
			// Parse old session info format for backward compatibility
			let mut old_info: SessionInfo = serde_json::from_str(content)?;
			// Add the new fields for token tracking
			old_info.input_tokens = 0;
			old_info.output_tokens = 0;
			old_info.cached_tokens = 0;  // Initialize new cached_tokens field
			old_info.total_cost = 0.0;
			old_info.duration_seconds = 0;
			old_info.layer_stats = Vec::new(); // Initialize empty layer stats
			session_info = Some(old_info);
		} else if let Some(content) = line.strip_prefix("SYSTEM: ") {
			// Parse system message
			let message: Message = serde_json::from_str(content)?;
			messages.push(message);
		} else if let Some(content) = line.strip_prefix("USER: ") {
			// Parse user message
			let message: Message = serde_json::from_str(content)?;
			messages.push(message);
		} else if let Some(content) = line.strip_prefix("ASSISTANT: ") {
			// Parse assistant message
			let message: Message = serde_json::from_str(content)?;
			messages.push(message);
		} else if !line.starts_with("EXCHANGE: ") {
			// Skip exchange lines, but try to parse anything else
			// This is a more flexible approach for future changes
			if line.contains("\"role\":") && line.contains("\"content\":") {
				// This looks like a valid message JSON - try to parse it
				if let Ok(message) = serde_json::from_str::<Message>(line) {
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
		};
		Ok(session)
	} else {
		Err(anyhow::anyhow!("Invalid session file: missing session info"))
	}
}

// Helper function to append to session file
pub fn append_to_session_file(session_file: &PathBuf, content: &str) -> Result<(), anyhow::Error> {
	let mut file = OpenOptions::new()
		.create(true)
		.append(true)
		.open(session_file)?;

	writeln!(file, "{}", content)?;
	Ok(())
}
pub async fn create_system_prompt(project_dir: &PathBuf, config: &crate::config::Config) -> String {
	// If a custom system prompt is defined in the config, use it
	if let Some(custom_prompt) = &config.system {
		return custom_prompt.clone();
	}

	// Otherwise, use the default system prompt
	let mut prompt = format!(
		"You are an Octodev – top notch AI coding assistant helping with the codebase in {}

When answering code questions:
• Provide validated, working code solutions
• Keep responses clear and concise
• Focus on practical solutions and industry best practices

Code Quality Guidelines:
• Avoid unnecessary abstractions - solve problems directly
• Balance file size and readability - don't create overly large files
• Don't over-fragment code across multiple files unnecessarily

Approach Problems Like a Developer:
1. Analyze the problem thoroughly first
2. Think through solutions sequentially
3. Solve step-by-step with a clear thought process

When working with files:
1. First understand which files you need to read/write
2. Process files efficiently, preferably in a single operation when possible
3. Utilize the provided tools for file operations",
		project_dir.display()
	);

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
