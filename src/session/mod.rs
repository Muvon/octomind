// Session module for handling interactive coding sessions

mod openrouter; // OpenRouter API client
pub mod chat;       // Chat session logic
mod chat_helper;    // Chat command completion
pub mod mcp;        // MCP protocol support

pub use openrouter::*;
pub use mcp::*;

// Re-export constants
// Constants moved to config

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
	let mut prompt = format!("You are an AI coding assistant helping with the codebase in {}", project_dir.display());

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

