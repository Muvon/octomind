// Session module for handling interactive coding sessions

mod openrouter; // OpenRouter API client
pub mod chat;       // Chat session logic
mod chat_helper;    // Chat command completion

pub use chat::*;
pub use openrouter::*;

// Re-export constants
pub const CLAUDE_MODEL: &str = "anthropic/claude-3-sonnet-20240229";

use std::fs::{self, OpenOptions, File};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use serde::{Serialize, Deserialize};
use std::io::Write;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Message {
    pub role: String,
    pub content: String,
    pub timestamp: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SessionInfo {
    pub name: String,
    pub created_at: u64,
    pub model: String,
    pub provider: String,
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
        };

        self.messages.push(message.clone());
        message
    }

    // Save the session to a file
    pub fn save(&self) -> Result<(), anyhow::Error> {
        if let Some(session_file) = &self.session_file {
            // Clear the file first
            let _ = File::create(session_file)?;

            // Save session info
            let info_json = serde_json::to_string(&self.info)?;
            append_to_session_file(session_file, &format!("INFO: {}", info_json))?;

            // Save all messages
            for message in &self.messages {
                let message_json = serde_json::to_string(message)?;
                let prefix = message.role.to_uppercase();
                append_to_session_file(session_file, &format!("{}: {}", prefix, message_json))?;
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

// Helper function to load a session from file
pub fn load_session(session_file: &PathBuf) -> Result<Session, anyhow::Error> {
    let content = fs::read_to_string(session_file)?;
    let mut session_info: Option<SessionInfo> = None;
    let mut messages = Vec::new();

    for line in content.lines() {
        if let Some(content) = line.strip_prefix("INFO: ") {
            // Parse session info
            session_info = Some(serde_json::from_str(content)?);
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
pub fn create_system_prompt(_project_dir: &PathBuf) -> String {
	format!("You are an AI coding assistant")
}

