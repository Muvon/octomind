// Copyright 2026 Muvon Un Limited
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

// Session persistence: auto-save/load/list session files

use super::{CompressionStats, Message, Session, SessionInfo};
use anyhow::Result;
use std::fs::{self as std_fs, File, OpenOptions};
use std::io::Write;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

// Get sessions directory path
pub fn get_sessions_dir() -> Result<PathBuf, anyhow::Error> {
	crate::directories::get_sessions_dir()
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

		if path.is_file() && path.extension().is_some_and(|ext| ext == "jsonl") {
			// Scan first few lines for SUMMARY entry (may not be line 1 in older files)
			if let Ok(file) = File::open(&path) {
				let reader = BufReader::new(file);
				let name = path
					.file_stem()
					.and_then(|s| s.to_str())
					.unwrap_or_default()
					.to_string();

				for line in reader.lines().take(10) {
					let Ok(line) = line else { break };

					// Try new JSON format first
					if let Ok(json_value) = serde_json::from_str::<serde_json::Value>(&line) {
						if json_value.get("type").and_then(|t| t.as_str()) == Some("SUMMARY") {
							if let Some(session_info_value) = json_value.get("session_info") {
								if let Ok(info) = serde_json::from_value::<SessionInfo>(
									session_info_value.clone(),
								) {
									sessions.push((name.clone(), info));
									break;
								}
							}
						}
					} else if let Some(content) = line.strip_prefix("SUMMARY: ") {
						// Fallback to legacy format
						if let Ok(info) = serde_json::from_str::<SessionInfo>(content) {
							sessions.push((name.clone(), info));
							break;
						}
					}
				}
			}
		}
	}

	// Sort sessions by creation time (newest first)
	sessions.sort_by_key(|b| std::cmp::Reverse(b.1.created_at));

	Ok(sessions)
}

// Find the most recent session for a specific project directory
// This works by checking the session name which includes the project basename
pub fn find_most_recent_session_for_project(
	project_dir: &Path,
) -> Result<Option<String>, anyhow::Error> {
	let sessions_dir = get_sessions_dir()?;

	if !sessions_dir.exists() {
		return Ok(None);
	}

	// Get the basename of the current project directory
	let project_basename = project_dir
		.file_name()
		.and_then(|n| n.to_str())
		.unwrap_or("");

	if project_basename.is_empty() {
		return Ok(None);
	}

	let mut matching_sessions: Vec<(String, u64)> = Vec::new();

	for entry in std_fs::read_dir(sessions_dir)? {
		let entry = entry?;
		let path = entry.path();

		if path.is_file() && path.extension().is_some_and(|ext| ext == "jsonl") {
			let name = path
				.file_stem()
				.and_then(|s| s.to_str())
				.unwrap_or_default();

			// Session name format: YYMMDD-HHMMSS-basename-uuid
			// Check if the session name contains the project basename
			if name.contains(project_basename) {
				// Get file modification time for sorting
				if let Ok(metadata) = std_fs::metadata(&path) {
					if let Ok(modified) = metadata.modified() {
						if let Ok(duration) =
							modified.duration_since(std::time::SystemTime::UNIX_EPOCH)
						{
							matching_sessions.push((name.to_string(), duration.as_secs()));
						}
					}
				}
			}
		}
	}

	// Sort by modification time (newest first)
	matching_sessions.sort_by_key(|b| std::cmp::Reverse(b.1));

	// Return the most recent session name
	Ok(matching_sessions.first().map(|(name, _)| name.clone()))
}

/// Check if there are incomplete tool calls that need cleanup
///
/// A tool call sequence is incomplete if:
/// 1. There's an assistant message with tool_calls
/// 2. AND there are tool calls without corresponding tool response messages
///
/// This correctly distinguishes between:
/// - Complete sequences: assistant -> tool_calls -> tool_responses -> (optional final assistant)
/// - Incomplete sequences: assistant -> tool_calls -> [interrupted, no tool responses]
pub(crate) fn has_incomplete_tool_calls(messages: &[Message]) -> bool {
	// Check ALL assistant messages with tool_calls, not just the last one
	for (i, msg) in messages.iter().enumerate() {
		if msg.role == "assistant" && msg.tool_calls.is_some() {
			if let Some(tool_calls_value) = &msg.tool_calls {
				// Parse the tool calls to get their IDs
				if let Ok(tool_calls) =
					serde_json::from_value::<Vec<serde_json::Value>>(tool_calls_value.clone())
				{
					for tool_call in tool_calls {
						if let Some(call_id) = tool_call.get("id").and_then(|id| id.as_str()) {
							// Look for a tool message with this call_id AFTER the assistant message
							let has_response = messages.iter().skip(i + 1).any(|response_msg| {
								response_msg.role == "tool"
									&& response_msg.tool_call_id.as_ref()
										== Some(&call_id.to_string())
							});

							if !has_response {
								return true; // Found a tool call without a response
							}
						}
					}
				}
			}
		}
	}

	false
}

/// Clean up interrupted tool calls by inserting synthetic results.
///
/// Instead of truncating the entire conversation from the first incomplete tool call,
/// this inserts a synthetic "[Tool execution was interrupted]" result for each missing
/// tool response. This preserves all valid conversation history and only patches the gaps.
pub fn clean_interrupted_tool_calls(messages: &mut Vec<Message>, context: &str) -> bool {
	if messages.is_empty() {
		return false;
	}

	// Collect (insert_after_index, call_id, tool_name) for each missing tool response.
	// We scan all assistant messages with tool_calls and check for missing responses.
	let mut insertions: Vec<(usize, String, String)> = Vec::new();

	for (i, msg) in messages.iter().enumerate() {
		if msg.role == "assistant" && msg.tool_calls.is_some() {
			if let Some(tool_calls_value) = &msg.tool_calls {
				if let Ok(tool_calls) =
					serde_json::from_value::<Vec<serde_json::Value>>(tool_calls_value.clone())
				{
					for tool_call in tool_calls {
						let call_id = tool_call
							.get("id")
							.and_then(|id| id.as_str())
							.unwrap_or("")
							.to_string();
						if call_id.is_empty() {
							continue;
						}
						let tool_name = tool_call
							.get("function")
							.and_then(|f| f.get("name"))
							.and_then(|n| n.as_str())
							.unwrap_or("unknown")
							.to_string();

						let has_response = messages.iter().skip(i + 1).any(|response_msg| {
							response_msg.role == "tool"
								&& response_msg.tool_call_id.as_ref() == Some(&call_id)
						});

						if !has_response {
							insertions.push((i, call_id, tool_name));
						}
					}
				}
			}
		}
	}

	if insertions.is_empty() {
		return false;
	}

	let count = insertions.len();

	// Insert in reverse order so earlier indices remain valid
	for (after_idx, call_id, tool_name) in insertions.into_iter().rev() {
		// Insert right after the assistant message (or after existing tool responses)
		// Find the correct insertion point: after the last tool response for this assistant msg
		let mut insert_at = after_idx + 1;
		while insert_at < messages.len() && messages[insert_at].role == "tool" {
			insert_at += 1;
		}

		messages.insert(
			insert_at,
			Message {
				role: "tool".to_string(),
				content: "[Tool execution was interrupted by user]".to_string(),
				timestamp: crate::utils::time::now_secs(),
				cached: false,
				cache_ttl: None,
				tool_call_id: Some(call_id),
				name: Some(tool_name),
				tool_calls: None,
				images: None,
				videos: None,
				thinking: None,
				id: None,
			},
		);
	}

	crate::log_debug!(
		"🔧 {}: Inserted {} synthetic tool results for interrupted calls",
		context,
		count
	);

	true
}

// Helper function to load a session from file - optimized to use streams
/// Intermediate result of parsing a session log file line by line.
struct ParsedLogLines {
	session_info: Option<SessionInfo>,
	messages: Vec<Message>,
	restoration_messages: Vec<Message>,
	restoration_point_found: bool,
}

/// Parse a session log file line by line, extracting messages and session metadata.
///
/// Handles both the current JSON format and the legacy prefix-based format.
/// Returns the raw parsed state — callers decide which messages to use.
fn parse_log_lines(reader: BufReader<File>) -> Result<ParsedLogLines> {
	let mut session_info: Option<SessionInfo> = None;
	let mut last_summary_timestamp: u64 = 0;
	let mut messages: Vec<Message> = Vec::new();
	let mut restoration_point_found = false;
	let mut restoration_messages = Vec::new();
	let mut pending_tool_calls: Vec<serde_json::Value> = Vec::new();

	// Process the file line by line to avoid loading the entire file into memory
	for line in reader.lines() {
		let line = line?;

		// Try to parse as JSON first (new format)
		if let Ok(json_value) = serde_json::from_str::<serde_json::Value>(&line) {
			if let Some(log_type) = json_value.get("type").and_then(|t| t.as_str()) {
				match log_type {
					"SUMMARY" => {
						// Extract session info from JSON log entry
						// SUMMARY is the source of truth - it contains complete session state
						if let Some(session_info_value) = json_value.get("session_info") {
							session_info =
								Some(serde_json::from_value(session_info_value.clone())?);
							// Track SUMMARY timestamp to ignore older STATS entries
							last_summary_timestamp = json_value
								.get("timestamp")
								.and_then(|t| t.as_u64())
								.unwrap_or(0);
						}
					}
					"RESTORATION_POINT" => {
						// Found a restoration point - this means the session was optimized with /done
						restoration_point_found = true;
						messages.clear();
						restoration_messages.clear();
						pending_tool_calls.clear(); // Clear stale tool calls from before restoration
					}
					"COMPRESSION_POINT" => {
						// Found a compression point - messages before this were compressed
						// Clear messages like RESTORATION_POINT to reflect compressed state
						if restoration_point_found {
							restoration_messages.clear();
						} else {
							messages.clear();
						}
						pending_tool_calls.clear(); // Clear stale tool calls from before compression

						// Log compression restoration for debugging
						if let (Some(comp_type), Some(msgs_removed)) = (
							json_value.get("compression_type").and_then(|t| t.as_str()),
							json_value.get("messages_removed").and_then(|m| m.as_u64()),
						) {
							crate::log_debug!(
								"Session restoration: Found COMPRESSION_POINT ({}, {} messages removed)",
								comp_type,
								msgs_removed
							);
						}
					}
					"TRUNCATION_POINT" => {
						// Found a truncation point - this means messages were removed due to Ctrl+C cleanup
						// Truncate to the specified message count to reflect the cleaned state
						if let Some(message_count) =
							json_value.get("message_count").and_then(|m| m.as_u64())
						{
							let target_count = message_count as usize;
							if restoration_point_found {
								restoration_messages.truncate(target_count);
								crate::log_debug!(
									"Session restoration: Found TRUNCATION_POINT - truncated restoration messages to {}",
									target_count
								);
							} else {
								messages.truncate(target_count);
								crate::log_debug!(
									"Session restoration: Found TRUNCATION_POINT - truncated messages to {}",
									target_count
								);
							}
						}
						pending_tool_calls.clear(); // Clear stale tool calls from before truncation
					}
					"COMMAND" => {
						// Commands are processed separately in extract_runtime_state_from_log
						continue;
					}
					"OUTPUT_MODE_REPLACE" => {
						// Handle Replace mode operations during session restoration
						// This clears messages like a restoration point but from a command
						if restoration_point_found {
							restoration_messages.clear();
						} else {
							messages.clear();
						}
						pending_tool_calls.clear(); // Clear stale tool calls from before replace

						// Log the replace operation for debugging
						if let Some(command) = json_value.get("command").and_then(|c| c.as_str()) {
							println!(
								"Session restoration: Found OUTPUT_MODE_REPLACE from command '{}'",
								command
							);
						}
					}
					"OUTPUT_MODE_APPEND" => {
						// Handle Append mode operations during session restoration
						// These are tracked but don't need special handling since the messages
						// are already in the session file as regular assistant messages
						continue;
					}
					"STATS" => {
						// STATS entries provide incremental updates during a session
						// BUT: Only apply STATS that are NEWER than the last SUMMARY
						// This ensures SUMMARY (written on save/exit) is the source of truth
						let stats_timestamp = json_value
							.get("timestamp")
							.and_then(|t| t.as_u64())
							.unwrap_or(0);

						// Only apply STATS if it's newer than the last SUMMARY
						// This prevents old STATS from overwriting fresh SUMMARY data on resume
						if stats_timestamp > last_summary_timestamp {
							if let Some(info) = &mut session_info {
								// CRITICAL FIX: Only apply STATS values if they're greater than current values
								// This prevents cached-only requests (where non-cached tokens = 0) from
								// overwriting the accumulated token counts from the SUMMARY
								if let Some(total_cost) =
									json_value.get("total_cost").and_then(|c| c.as_f64())
								{
									if total_cost > info.total_cost {
										info.total_cost = total_cost;
									}
								}
								if let Some(input_tokens) =
									json_value.get("input_tokens").and_then(|t| t.as_u64())
								{
									if input_tokens > info.input_tokens {
										info.input_tokens = input_tokens;
									}
								}
								if let Some(output_tokens) =
									json_value.get("output_tokens").and_then(|t| t.as_u64())
								{
									if output_tokens > info.output_tokens {
										info.output_tokens = output_tokens;
									}
								}
								if let Some(cache_read_tokens) =
									json_value.get("cache_read_tokens").and_then(|t| t.as_u64())
								{
									if cache_read_tokens > info.cache_read_tokens {
										info.cache_read_tokens = cache_read_tokens;
									}
								}
								if let Some(cache_write_tokens) = json_value
									.get("cache_write_tokens")
									.and_then(|t| t.as_u64())
								{
									if cache_write_tokens > info.cache_write_tokens {
										info.cache_write_tokens = cache_write_tokens;
									}
								}

								if let Some(tool_calls) =
									json_value.get("tool_calls").and_then(|t| t.as_u64())
								{
									if tool_calls > info.tool_calls {
										info.tool_calls = tool_calls;
									}
								}
								if let Some(api_time) =
									json_value.get("total_api_time_ms").and_then(|t| t.as_u64())
								{
									if api_time > info.total_api_time_ms {
										info.total_api_time_ms = api_time;
									}
								}
								if let Some(tool_time) = json_value
									.get("total_tool_time_ms")
									.and_then(|t| t.as_u64())
								{
									if tool_time > info.total_tool_time_ms {
										info.total_tool_time_ms = tool_time;
									}
								}
								if let Some(layer_time) = json_value
									.get("total_layer_time_ms")
									.and_then(|t| t.as_u64())
								{
									if layer_time > info.total_layer_time_ms {
										info.total_layer_time_ms = layer_time;
									}
								}
							}
						}
					}
					"TOOL_CALL" => {
						// Collect tool calls to reconstruct assistant message with tool_calls
						if let (Some(tool_name), Some(tool_id), Some(parameters)) = (
							json_value.get("tool_name").and_then(|n| n.as_str()),
							json_value.get("tool_id").and_then(|id| id.as_str()),
							json_value.get("parameters"),
						) {
							// Store tool call for later reconstruction
							pending_tool_calls.push(serde_json::json!({
								"id": tool_id,
								"type": "function",
								"function": {
									"name": tool_name,
									"arguments": serde_json::to_string(parameters).unwrap_or_default()
								}
							}));
						}
					}
					"API_REQUEST" | "API_RESPONSE" | "TOOL_RESULT" | "CACHE" | "ERROR"
					| "SYSTEM" | "USER" | "ASSISTANT" => {
						// Skip debug log entries during message parsing
						continue;
					}
					_ => {
						// Unknown log type, skip
						continue;
					}
				}
			} else if line.contains("\"role\":") && line.contains("\"content\":") {
				// This is a regular message JSON line
				if let Ok(message) = serde_json::from_str::<Message>(&line) {
					// If this is the first tool message and we have pending tool calls,
					// reconstruct the assistant message with tool_calls ONLY if not already present
					if message.role == "tool" && !pending_tool_calls.is_empty() {
						// Check if the last message is already an assistant message with tool_calls
						let last_is_assistant_with_tool_calls = if restoration_point_found {
							restoration_messages.last()
						} else {
							messages.last()
						}
						.map(|m| m.role == "assistant" && m.tool_calls.is_some())
						.unwrap_or(false);

						// Only reconstruct if the assistant message doesn't already exist
						// This prevents losing thinking content when the Message JSON was already parsed
						if !last_is_assistant_with_tool_calls {
							let assistant_with_tool_calls = Message {
								role: "assistant".to_string(),
								content: "".to_string(), // Empty content for tool call messages
								tool_calls: Some(serde_json::Value::Array(
									pending_tool_calls.clone(),
								)),
								timestamp: message.timestamp,
								cached: false,
								..Default::default()
							};

							if restoration_point_found {
								restoration_messages.push(assistant_with_tool_calls);
							} else {
								messages.push(assistant_with_tool_calls);
							}
						}

						// Clear pending tool calls since we've reconstructed the assistant message
						pending_tool_calls.clear();
					}

					if restoration_point_found {
						restoration_messages.push(message);
					} else {
						messages.push(message);
					}
				}
			}
		} else {
			// Fallback to legacy prefix-based format for backward compatibility
			if line.starts_with("SUMMARY: ") {
				if let Some(content) = line.strip_prefix("SUMMARY: ") {
					session_info = Some(serde_json::from_str(content)?);
				}
			} else if line.starts_with("INFO: ") {
				if let Some(content) = line.strip_prefix("INFO: ") {
					let mut old_info: SessionInfo = serde_json::from_str(content)?;
					old_info.input_tokens = 0;
					old_info.output_tokens = 0;
					old_info.cache_read_tokens = 0;
					old_info.cache_write_tokens = 0;
					old_info.total_cost = 0.0;
					old_info.duration_seconds = 0;
					old_info.layer_stats = Vec::new();
					old_info.tool_calls = 0;
					// Initialize time tracking for legacy sessions
					old_info.total_api_time_ms = 0;
					old_info.total_tool_time_ms = 0;
					old_info.total_layer_time_ms = 0;
					session_info = Some(old_info);
				}
			} else if line.starts_with("RESTORATION_POINT: ") {
				restoration_point_found = true;
				messages.clear();
				restoration_messages.clear();
			} else if !line.starts_with("API_REQUEST: ")
				&& !line.starts_with("API_RESPONSE: ")
				&& !line.starts_with("TOOL_CALL: ")
				&& !line.starts_with("TOOL_RESULT: ")
				&& !line.starts_with("CACHE: ")
				&& !line.starts_with("ERROR: ")
				&& !line.starts_with("EXCHANGE: ")
				&& !line.is_empty()
			{
				// Try to parse as message JSON or legacy prefixed formats
				if line.contains("\"role\":") && line.contains("\"content\":") {
					if let Ok(message) = serde_json::from_str::<Message>(&line) {
						if restoration_point_found {
							restoration_messages.push(message);
						} else {
							messages.push(message);
						}
					}
				} else if let Some(content) = line.strip_prefix("SYSTEM: ") {
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
	}

	Ok(ParsedLogLines {
		session_info,
		messages,
		restoration_messages,
		restoration_point_found,
	})
}

/// Build a Session from parsed log data when a SUMMARY entry was found.
///
/// Applies runtime state overrides (e.g. model changes from `/model` commands),
/// cleans up any interrupted tool calls, and returns the final Session.
fn reconstruct_messages(
	mut info: SessionInfo,
	final_messages: Vec<Message>,
	session_file: &PathBuf,
) -> Result<Session> {
	let runtime_state = extract_runtime_state_from_log(session_file)?;
	if let Some(model) = runtime_state.model {
		info.model = model;
	}

	let mut cleaned_messages = final_messages;
	if has_incomplete_tool_calls(&cleaned_messages) {
		clean_interrupted_tool_calls(&mut cleaned_messages, "Session restoration");
	}

	Ok(Session {
		info,
		messages: cleaned_messages,
		session_file: Some(session_file.clone()),
	})
}

/// Build a Session when no SUMMARY entry was found (legacy or corrupted session files).
///
/// Synthesises a default SessionInfo from the file path and any STATS entries,
/// then applies runtime state overrides.
fn restore_session_info(final_messages: Vec<Message>, session_file: &PathBuf) -> Result<Session> {
	let session_name = session_file
		.file_stem()
		.and_then(|s| s.to_str())
		.unwrap_or("unknown")
		.to_string();

	let default_model = "openrouter:anthropic/claude-sonnet-4".to_string();

	let created_at = session_file
		.metadata()
		.and_then(|meta| {
			meta.created()
				.ok()
				.ok_or(std::io::Error::other("No creation time"))
		})
		.and_then(|time| {
			time.duration_since(std::time::UNIX_EPOCH)
				.ok()
				.ok_or(std::io::Error::other("Invalid time"))
		})
		.map(|duration| duration.as_secs())
		.unwrap_or_else(|_| crate::utils::time::now_secs());

	let provider = if default_model.starts_with("openrouter:") {
		"openrouter"
	} else if default_model.starts_with("anthropic:") {
		"anthropic"
	} else if default_model.starts_with("openai:") {
		"openai"
	} else {
		"unknown"
	}
	.to_string();

	let mut info = SessionInfo {
		name: session_name,
		created_at,
		model: default_model,
		provider,
		input_tokens: 0,
		output_tokens: 0,
		cache_read_tokens: 0,
		cache_write_tokens: 0,
		reasoning_tokens: 0,
		total_cost: 0.0,
		duration_seconds: 0,
		layer_stats: Vec::new(),
		tool_calls: 0,
		total_api_time_ms: 0,
		total_tool_time_ms: 0,
		total_layer_time_ms: 0,
		compression_stats: CompressionStats::default(),
		total_api_calls: 0,
		current_non_cached_tokens: 0,
		current_total_tokens: 0,
		last_cache_checkpoint_time: crate::utils::time::now_secs(),
		cache_next_user_message: false,
		spending_threshold_checkpoint: 0.0,
		compression_hint_count: 0,
		last_compression_hint_shown: 0,
		context_tokens_after_last_compression: 0,
		predicted_turns_at_last_compression: 0.0,
		api_calls_at_last_compression: 0,
		output_tokens_at_last_compression: 0,
		consecutive_compressions: 0,
	};

	let runtime_state = extract_runtime_state_from_log(session_file)?;
	if let Some(model) = runtime_state.model {
		info.model = model;
	}

	// Apply any STATS entries found in the file (best-effort token/cost recovery)
	let file = File::open(session_file)?;
	let reader = BufReader::new(file);
	for line in reader.lines() {
		let line = line?;
		if let Ok(json_value) = serde_json::from_str::<serde_json::Value>(&line) {
			if json_value.get("type").and_then(|t| t.as_str()) == Some("STATS") {
				if let Some(v) = json_value.get("total_cost").and_then(|c| c.as_f64()) {
					info.total_cost = v;
				}
				if let Some(v) = json_value.get("input_tokens").and_then(|t| t.as_u64()) {
					info.input_tokens = v;
				}
				if let Some(v) = json_value.get("output_tokens").and_then(|t| t.as_u64()) {
					info.output_tokens = v;
				}
				if let Some(v) = json_value.get("cache_read_tokens").and_then(|t| t.as_u64()) {
					info.cache_read_tokens = v;
				}
				if let Some(v) = json_value
					.get("cache_write_tokens")
					.and_then(|t| t.as_u64())
				{
					info.cache_write_tokens = v;
				}
				if let Some(v) = json_value.get("tool_calls").and_then(|t| t.as_u64()) {
					info.tool_calls = v;
				}
				if let Some(v) = json_value.get("total_api_time_ms").and_then(|t| t.as_u64()) {
					info.total_api_time_ms = v;
				}
				if let Some(v) = json_value
					.get("total_tool_time_ms")
					.and_then(|t| t.as_u64())
				{
					info.total_tool_time_ms = v;
				}
				if let Some(v) = json_value
					.get("total_layer_time_ms")
					.and_then(|t| t.as_u64())
				{
					info.total_layer_time_ms = v;
				}
			}
		}
	}

	println!("⚠️  Session loaded with default metadata (SUMMARY was missing)");
	Ok(Session {
		info,
		messages: final_messages,
		session_file: Some(session_file.clone()),
	})
}

pub fn load_session(session_file: &PathBuf) -> Result<Session, anyhow::Error> {
	if !session_file.exists() {
		return Err(anyhow::anyhow!("Session file does not exist"));
	}

	let reader = BufReader::new(File::open(session_file)?);
	let parsed = parse_log_lines(reader)?;

	let final_messages =
		if parsed.restoration_point_found && !parsed.restoration_messages.is_empty() {
			parsed.restoration_messages
		} else {
			parsed.messages
		};

	if let Some(info) = parsed.session_info {
		reconstruct_messages(info, final_messages, session_file)
	} else {
		restore_session_info(final_messages, session_file)
	}
}

/// Runtime state extracted from session commands
#[derive(Debug, Default)]
pub struct SessionRuntimeState {
	pub model: Option<String>,
	pub layers_enabled: Option<bool>,
	pub cache_next_message: bool,
	pub role: Option<String>,            // Track runtime role changes
	pub critical_knowledge: Vec<String>, // Knowledge entries from compressions
}

/// Extract runtime state from session log file
pub fn extract_runtime_state_from_log(session_file: &PathBuf) -> Result<SessionRuntimeState> {
	let file = File::open(session_file)?;
	let reader = BufReader::new(file);
	let mut state = SessionRuntimeState::default();

	for line in reader.lines() {
		let line = line?;

		if let Ok(json_value) = serde_json::from_str::<serde_json::Value>(&line) {
			if let Some(log_type) = json_value.get("type").and_then(|t| t.as_str()) {
				match log_type {
					"RESTORATION_POINT" => {
						// Reset state tracking after restoration point
						state = SessionRuntimeState::default();
					}
					"KNOWLEDGE_ENTRY" => {
						// Restore critical knowledge entries from compression cycles
						if let Some(content) = json_value.get("content").and_then(|c| c.as_str()) {
							state.critical_knowledge.push(content.to_string());
						}
					}
					"COMMAND" => {
						// Process all commands to get the final state
						if let Some(command) = json_value.get("command").and_then(|c| c.as_str()) {
							apply_command_to_runtime_state(&mut state, command);
						}
					}
					_ => {}
				}
			}
		}
	}
	Ok(state)
}

/// Apply a command to runtime state (for state extraction)
fn apply_command_to_runtime_state(state: &mut SessionRuntimeState, command_line: &str) {
	let parts: Vec<&str> = command_line.split_whitespace().collect();
	if parts.is_empty() {
		return;
	}

	match parts[0] {
		"/model" if parts.len() > 1 => {
			let new_model = parts[1..].join(" ");
			state.model = Some(new_model);
		}
		"/role" if parts.len() > 1 => {
			let new_role = parts[1].to_string();
			state.role = Some(new_role);
		}
		"/layers" if parts.len() > 1 => {
			let state_str = parts[1];
			state.layers_enabled = Some(state_str == "enabled");
		}
		"/cache" => {
			// Set cache next message flag
			state.cache_next_message = true;
		}
		_ => {
			// Unknown command, ignore
		}
	}
}

// Helper function to append to session file ensuring single lines
pub fn append_to_session_file(session_file: &PathBuf, content: &str) -> Result<(), anyhow::Error> {
	let mut file = OpenOptions::new()
		.create(true)
		.append(true)
		.open(session_file)?;

	// Ensure content is on a single line - replace any newlines with spaces
	let single_line_content = content.replace(['\n', '\r'], " ");
	writeln!(file, "{}", single_line_content)?;
	Ok(())
}
