// Logger module for OctoDev

use anyhow::Result;
use serde::{Serialize, Deserialize};
use std::fs::{self, OpenOptions};
use std::path::PathBuf;
use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

/// An enumeration of possible message types in the log
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum LogMessageType {
    UserRequest,
    AssistantResponse,
    ToolRequest,
    ToolResponse,
    SystemMessage,
}

/// Structure representing a log entry for requests and responses
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct LogEntry {
    pub role: String,
    pub created: u64,
    pub content: serde_json::Value,
    pub tool_id: Option<String>,  // Included when it's a tool request or response
}

/// Returns the path to the logs directory, creating it if it doesn't exist
pub fn get_logs_dir() -> Result<PathBuf> {
    let current_dir = std::env::current_dir()?;
    let octodev_dir = current_dir.join(".octodev");
    let logs_dir = octodev_dir.join("logs");

    if !logs_dir.exists() {
        fs::create_dir_all(&logs_dir)?;
    }

    Ok(logs_dir)
}

/// Get a log file path for the current date
pub fn get_log_file() -> Result<PathBuf> {
    let logs_dir = get_logs_dir()?;
    let now = chrono::Local::now();
    let log_file = logs_dir.join(format!("session_{}.jsonl", now.format("%Y-%m-%d")));
    
    Ok(log_file)
}

/// Log a user request
pub fn log_user_request(content: &str) -> Result<LogEntry> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    
    let log_entry = LogEntry {
        role: "user".to_string(),
        created: timestamp,
        content: serde_json::json!([{
            "type": "text",
            "text": content
        }]),
        tool_id: None,
    };
    
    let log_file = get_log_file()?;
    let log_json = serde_json::to_string(&log_entry)?;
    append_to_log_file(&log_file, &log_json)?;
    
    Ok(log_entry)
}

/// Log an assistant response
pub fn log_assistant_response(content: &str) -> Result<LogEntry> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    
    let log_entry = LogEntry {
        role: "assistant".to_string(),
        created: timestamp,
        content: serde_json::json!([{
            "type": "text",
            "text": content
        }]),
        tool_id: None,
    };
    
    let log_file = get_log_file()?;
    let log_json = serde_json::to_string(&log_entry)?;
    append_to_log_file(&log_file, &log_json)?;
    
    Ok(log_entry)
}

/// Log a tool request
pub fn log_tool_request(tool_name: &str, parameters: &serde_json::Value, tool_id: &str) -> Result<LogEntry> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    
    let log_entry = LogEntry {
        role: "assistant".to_string(),
        created: timestamp,
        content: serde_json::json!([{
            "type": "toolRequest",
            "id": tool_id,
            "toolCall": {
                "status": "success",
                "value": {
                    "name": tool_name,
                    "arguments": parameters
                }
            }
        }]),
        tool_id: Some(tool_id.to_string()),
    };
    
    let log_file = get_log_file()?;
    let log_json = serde_json::to_string(&log_entry)?;
    append_to_log_file(&log_file, &log_json)?;
    
    Ok(log_entry)
}

/// Log a tool response
pub fn log_tool_response(result: &serde_json::Value, tool_id: &str) -> Result<LogEntry> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    
    let log_entry = LogEntry {
        role: "user".to_string(),
        created: timestamp,
        content: serde_json::json!([{
            "type": "toolResponse",
            "id": tool_id,
            "toolResult": {
                "status": "success",
                "value": result
            }
        }]),
        tool_id: Some(tool_id.to_string()),
    };
    
    let log_file = get_log_file()?;
    let log_json = serde_json::to_string(&log_entry)?;
    append_to_log_file(&log_file, &log_json)?;
    
    Ok(log_entry)
}

/// Log a raw exchange (request and response) from the API
pub fn log_raw_exchange(exchange: &crate::session::openrouter::OpenRouterExchange) -> Result<()> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    
    // Create a raw log file for detailed API exchanges
    let logs_dir = get_logs_dir()?;
    let raw_log_file = logs_dir.join(format!("raw_exchange_{}.jsonl", timestamp));
    
    let raw_json = serde_json::to_string(exchange)?;
    append_to_log_file(&raw_log_file, &raw_json)?;
    
    Ok(())
}

/// Helper function to append to log file
fn append_to_log_file(log_file: &PathBuf, content: &str) -> Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_file)?;

    writeln!(file, "{}", content)?;
    
    Ok(())
}