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

// Simplified logging module for Octomind - single JSONL session file with prefixes

use anyhow::Result;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Get the session file path for a specific session (unified JSONL approach)
pub fn get_session_log_file(session_name: &str) -> Result<PathBuf> {
	let sessions_dir = crate::directories::get_sessions_dir()?;

	// Use single JSONL file for everything - session messages + raw debug logs
	let log_file = sessions_dir.join(format!("{}.jsonl", session_name));
	Ok(log_file)
}

/// Log session stats snapshot after each request completion
pub fn log_session_stats(
	session_name: &str,
	session_info: &crate::session::SessionInfo,
) -> Result<()> {
	let log_file = get_session_log_file(session_name)?;
	let log_entry = serde_json::json!({
		"type": "STATS",
		"timestamp": get_timestamp(),
		"total_cost": session_info.total_cost,
		"input_tokens": session_info.input_tokens,
		"output_tokens": session_info.output_tokens,
		"cached_tokens": session_info.cached_tokens,
		"tool_calls": session_info.tool_calls,
		"total_api_time_ms": session_info.total_api_time_ms,
		"total_tool_time_ms": session_info.total_tool_time_ms,
		"total_layer_time_ms": session_info.total_layer_time_ms,
		"model": session_info.model,
		"provider": session_info.provider
	});
	append_to_log(&log_file, &serde_json::to_string(&log_entry)?)?;
	Ok(())
}

/// Log system message (our prompts, system setup)
pub fn log_system_message(session_name: &str, content: &str) -> Result<()> {
	let log_file = get_session_log_file(session_name)?;
	let log_entry = serde_json::json!({
		"type": "SYSTEM",
		"timestamp": get_timestamp(),
		"content": content
	});
	append_to_log(&log_file, &serde_json::to_string(&log_entry)?)?;
	Ok(())
}

/// Log user input
pub fn log_user_input(session_name: &str, content: &str) -> Result<()> {
	let log_file = get_session_log_file(session_name)?;
	let log_entry = serde_json::json!({
		"type": "USER",
		"timestamp": get_timestamp(),
		"content": content
	});
	append_to_log(&log_file, &serde_json::to_string(&log_entry)?)?;
	Ok(())
}

/// Log RAW API request (what we send to the API)
pub fn log_api_request(session_name: &str, request: &serde_json::Value) -> Result<()> {
	let log_file = get_session_log_file(session_name)?;
	let log_entry = serde_json::json!({
		"type": "API_REQUEST",
		"timestamp": get_timestamp(),
		"data": request
	});
	append_to_log(&log_file, &serde_json::to_string(&log_entry)?)?;
	Ok(())
}

/// Log RAW API response (what we get from the API) with processed usage data
pub fn log_api_response(
	session_name: &str,
	response: &serde_json::Value,
	usage: Option<&crate::providers::TokenUsage>,
) -> Result<()> {
	let log_file = get_session_log_file(session_name)?;
	let log_entry = serde_json::json!({
		"type": "API_RESPONSE",
		"timestamp": get_timestamp(),
		"data": response,
		"usage": usage
	});
	append_to_log(&log_file, &serde_json::to_string(&log_entry)?)?;
	Ok(())
}

/// Log tool call request
pub fn log_tool_call(
	session_name: &str,
	tool_name: &str,
	tool_id: &str,
	parameters: &serde_json::Value,
) -> Result<()> {
	let log_file = get_session_log_file(session_name)?;
	let log_entry = serde_json::json!({
		"type": "TOOL_CALL",
		"timestamp": get_timestamp(),
		"tool_name": tool_name,
		"tool_id": tool_id,
		"parameters": parameters
	});
	append_to_log(&log_file, &serde_json::to_string(&log_entry)?)?;
	Ok(())
}

/// Log tool response result with execution timing
pub fn log_tool_result(
	session_name: &str,
	tool_id: &str,
	result: &serde_json::Value,
	execution_time_ms: u64,
) -> Result<()> {
	let log_file = get_session_log_file(session_name)?;
	let log_entry = serde_json::json!({
		"type": "TOOL_RESULT",
		"timestamp": get_timestamp(),
		"tool_id": tool_id,
		"result": result,
		"execution_time_ms": execution_time_ms
	});
	append_to_log(&log_file, &serde_json::to_string(&log_entry)?)?;
	Ok(())
}

/// Log assistant response (final cleaned response shown to user)
pub fn log_assistant_response(session_name: &str, content: &str) -> Result<()> {
	let log_file = get_session_log_file(session_name)?;
	let log_entry = serde_json::json!({
		"type": "ASSISTANT",
		"timestamp": get_timestamp(),
		"content": content
	});
	append_to_log(&log_file, &serde_json::to_string(&log_entry)?)?;
	Ok(())
}

/// Log restoration point for /done command
pub fn log_restoration_point(
	session_name: &str,
	user_message: &str,
	assistant_response: &str,
) -> Result<()> {
	let log_file = get_session_log_file(session_name)?;
	let log_entry = serde_json::json!({
		"type": "RESTORATION_POINT",
		"timestamp": get_timestamp(),
		"user_message": user_message,
		"assistant_response": assistant_response
	});
	append_to_log(&log_file, &serde_json::to_string(&log_entry)?)?;
	Ok(())
}

/// Log session command execution (runtime-only commands like /model, /cache, etc.)
pub fn log_session_command(session_name: &str, command_line: &str) -> Result<()> {
	let log_file = get_session_log_file(session_name)?;
	let log_entry = serde_json::json!({
		"type": "COMMAND",
		"timestamp": get_timestamp(),
		"command": command_line
	});
	append_to_log(&log_file, &serde_json::to_string(&log_entry)?)?;
	Ok(())
}

/// Log cache operations for debugging
pub fn log_cache_operation(session_name: &str, operation: &str, details: &str) -> Result<()> {
	let log_file = get_session_log_file(session_name)?;
	let log_entry = serde_json::json!({
		"type": "CACHE",
		"timestamp": get_timestamp(),
		"operation": operation,
		"details": details
	});
	append_to_log(&log_file, &serde_json::to_string(&log_entry)?)?;
	Ok(())
}

/// Log errors for debugging
pub fn log_error(session_name: &str, error: &str) -> Result<()> {
	let log_file = get_session_log_file(session_name)?;
	let log_entry = serde_json::json!({
		"type": "ERROR",
		"timestamp": get_timestamp(),
		"error": error
	});
	append_to_log(&log_file, &serde_json::to_string(&log_entry)?)?;
	Ok(())
}

/// Helper to get timestamp
fn get_timestamp() -> u64 {
	SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.unwrap_or_default()
		.as_secs()
}

/// Helper to append to log file ensuring single lines
fn append_to_log(log_file: &PathBuf, content: &str) -> Result<()> {
	let mut file = OpenOptions::new()
		.create(true)
		.append(true)
		.open(log_file)?;

	// Ensure content is on a single line - replace any newlines with spaces
	let single_line_content = content.replace(['\n', '\r'], " ");
	writeln!(file, "{}", single_line_content)?;
	Ok(())
}

// Legacy functions for compatibility - redirect to new system
pub fn log_user_request(content: &str) -> Result<()> {
	// We need session name - for now use "default" but this should be passed properly
	log_user_input("default", content)
}

pub fn log_raw_exchange(exchange: &crate::session::ProviderExchange) -> Result<()> {
	// Extract session name if available, otherwise use "default"
	let session_name = "default"; // TODO: Extract from context

	// Log both request and response separately for easier debugging
	log_api_request(session_name, &exchange.request)?;
	log_api_response(session_name, &exchange.response, exchange.usage.as_ref())?;
	Ok(())
}

/// Get session log file path for external use
pub fn get_session_log_path(session_name: &str) -> Result<PathBuf> {
	get_session_log_file(session_name)
}

/// Legacy function for compatibility
pub fn get_log_file() -> Result<PathBuf> {
	let logs_dir = crate::directories::get_logs_dir()?;

	let now = chrono::Local::now();
	let log_file = logs_dir.join(format!("session_{}.jsonl", now.format("%Y-%m-%d")));
	Ok(log_file)
}
