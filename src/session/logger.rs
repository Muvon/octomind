// Simplified RAW logging module for Octodev - single session log with prefixes

use anyhow::Result;
use std::fs::{self, OpenOptions};
use std::path::PathBuf;
use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

/// Get the raw log file path for a specific session
pub fn get_session_raw_log(session_name: &str) -> Result<PathBuf> {
	let current_dir = std::env::current_dir()?;
	let octodev_dir = current_dir.join(".octodev");
	let logs_dir = octodev_dir.join("sessions");

	if !logs_dir.exists() {
		fs::create_dir_all(&logs_dir)?;
	}

	// Use session name for the raw log file
	let log_file = logs_dir.join(format!("{}.raw.log", session_name));
	Ok(log_file)
}

/// Log session summary (first line) - tokens, cost, model info
pub fn log_session_summary(session_name: &str, session_info: &crate::session::SessionInfo) -> Result<()> {
	let log_file = get_session_raw_log(session_name)?;
	let summary_json = serde_json::to_string(session_info)?;
	
	// Only write summary if file doesn't exist or is empty
	if !log_file.exists() || std::fs::metadata(&log_file)?.len() == 0 {
		append_to_log(&log_file, &format!("SUMMARY: {}", summary_json))?;
	}
	
	Ok(())
}

/// Log system message (our prompts, system setup)
pub fn log_system_message(session_name: &str, content: &str) -> Result<()> {
	let log_file = get_session_raw_log(session_name)?;
	let timestamp = get_timestamp();
	append_to_log(&log_file, &format!("SYSTEM: {} | {}", timestamp, content))?;
	Ok(())
}

/// Log user input
pub fn log_user_input(session_name: &str, content: &str) -> Result<()> {
	let log_file = get_session_raw_log(session_name)?;
	let timestamp = get_timestamp();
	append_to_log(&log_file, &format!("USER: {} | {}", timestamp, content))?;
	Ok(())
}

/// Log RAW API request (what we send to the API)
pub fn log_api_request(session_name: &str, request: &serde_json::Value) -> Result<()> {
	let log_file = get_session_raw_log(session_name)?;
	let timestamp = get_timestamp();
	let request_json = serde_json::to_string(request)?;
	append_to_log(&log_file, &format!("API_REQUEST: {} | {}", timestamp, request_json))?;
	Ok(())
}

/// Log RAW API response (what we get from the API)
pub fn log_api_response(session_name: &str, response: &serde_json::Value) -> Result<()> {
	let log_file = get_session_raw_log(session_name)?;
	let timestamp = get_timestamp();
	let response_json = serde_json::to_string(response)?;
	append_to_log(&log_file, &format!("API_RESPONSE: {} | {}", timestamp, response_json))?;
	Ok(())
}

/// Log tool call request
pub fn log_tool_call(session_name: &str, tool_name: &str, tool_id: &str, parameters: &serde_json::Value) -> Result<()> {
	let log_file = get_session_raw_log(session_name)?;
	let timestamp = get_timestamp();
	let tool_data = serde_json::json!({
		"tool_name": tool_name,
		"tool_id": tool_id,
		"parameters": parameters
	});
	append_to_log(&log_file, &format!("TOOL_CALL: {} | {}", timestamp, tool_data))?;
	Ok(())
}

/// Log tool response result
pub fn log_tool_result(session_name: &str, tool_id: &str, result: &serde_json::Value) -> Result<()> {
	let log_file = get_session_raw_log(session_name)?;
	let timestamp = get_timestamp();
	let result_data = serde_json::json!({
		"tool_id": tool_id,
		"result": result
	});
	append_to_log(&log_file, &format!("TOOL_RESULT: {} | {}", timestamp, result_data))?;
	Ok(())
}

/// Log assistant response (final cleaned response shown to user)
pub fn log_assistant_response(session_name: &str, content: &str) -> Result<()> {
	let log_file = get_session_raw_log(session_name)?;
	let timestamp = get_timestamp();
	append_to_log(&log_file, &format!("ASSISTANT: {} | {}", timestamp, content))?;
	Ok(())
}

/// Log restoration point for /done command
pub fn log_restoration_point(session_name: &str, user_message: &str, assistant_response: &str) -> Result<()> {
	let log_file = get_session_raw_log(session_name)?;
	let timestamp = get_timestamp();
	let restoration_data = serde_json::json!({
		"user_message": user_message,
		"assistant_response": assistant_response,
		"timestamp": timestamp
	});
	append_to_log(&log_file, &format!("RESTORATION_POINT: {} | {}", timestamp, restoration_data))?;
	Ok(())
}

/// Log cache operations for debugging
pub fn log_cache_operation(session_name: &str, operation: &str, details: &str) -> Result<()> {
	let log_file = get_session_raw_log(session_name)?;
	let timestamp = get_timestamp();
	append_to_log(&log_file, &format!("CACHE: {} | {} | {}", timestamp, operation, details))?;
	Ok(())
}

/// Log errors for debugging
pub fn log_error(session_name: &str, error: &str) -> Result<()> {
	let log_file = get_session_raw_log(session_name)?;
	let timestamp = get_timestamp();
	append_to_log(&log_file, &format!("ERROR: {} | {}", timestamp, error))?;
	Ok(())
}

/// Update session summary (overwrite first line)
pub fn update_session_summary(session_name: &str, session_info: &crate::session::SessionInfo) -> Result<()> {
	let log_file = get_session_raw_log(session_name)?;
	
	if !log_file.exists() {
		return log_session_summary(session_name, session_info);
	}

	// Read all lines except the first one
	let content = std::fs::read_to_string(&log_file)?;
	let lines: Vec<&str> = content.lines().collect();
	
	// Write new summary + all existing lines except first
	let summary_json = serde_json::to_string(session_info)?;
	let mut new_content = format!("SUMMARY: {}\n", summary_json);
	
	// Add all lines except the first (old summary)
	for (i, line) in lines.iter().enumerate() {
		if i > 0 { // Skip first line (old summary)
			new_content.push_str(line);
			new_content.push('\n');
		}
	}
	
	std::fs::write(&log_file, new_content)?;
	Ok(())
}

/// Helper to get timestamp
fn get_timestamp() -> u64 {
	SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.unwrap_or_default()
		.as_secs()
}

/// Helper to append to log file
fn append_to_log(log_file: &PathBuf, content: &str) -> Result<()> {
	let mut file = OpenOptions::new()
		.create(true)
		.append(true)
		.open(log_file)?;

	writeln!(file, "{}", content)?;
	Ok(())
}

// Legacy functions for compatibility - redirect to new system
pub fn log_user_request(content: &str) -> Result<()> {
	// We need session name - for now use "default" but this should be passed properly
	log_user_input("default", content)
}

pub fn log_raw_exchange(exchange: &crate::session::openrouter::OpenRouterExchange) -> Result<()> {
	// Extract session name if available, otherwise use "default"
	let session_name = "default"; // TODO: Extract from context
	
	// Log both request and response separately for easier debugging
	log_api_request(session_name, &exchange.request)?;
	log_api_response(session_name, &exchange.response)?;
	Ok(())
}

/// Get session raw log file path for external use
pub fn get_session_log_path(session_name: &str) -> Result<PathBuf> {
	get_session_raw_log(session_name)
}

/// Legacy function for compatibility
pub fn get_log_file() -> Result<PathBuf> {
	let current_dir = std::env::current_dir()?;
	let octodev_dir = current_dir.join(".octodev");
	let logs_dir = octodev_dir.join("logs");

	if !logs_dir.exists() {
		fs::create_dir_all(&logs_dir)?;
	}

	let now = chrono::Local::now();
	let log_file = logs_dir.join(format!("session_{}.jsonl", now.format("%Y-%m-%d")));
	Ok(log_file)
}