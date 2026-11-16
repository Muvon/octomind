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

use super::{Message, Session, SessionInfo};
use anyhow::{Context, Result};
use serde::de::{DeserializeOwned, IgnoredAny};
use serde::{Deserialize, Deserializer};
use std::collections::VecDeque;
use std::fs::{self as std_fs, File, OpenOptions};
use std::io::{BufRead, BufReader, Cursor, ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use zstd::stream::read::Decoder as ZstdDecoder;

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

		if path.is_file() && path.extension().is_some_and(|ext| ext == "zst") {
			// Scan first few lines for SUMMARY entry (may not be line 1 in older files)
			if let Ok(log) = SessionLog::open(&path) {
				// file_stem() gives "name.jsonl" for "name.jsonl.zst"; strip the .jsonl suffix
				let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
				let name = stem.strip_suffix(".jsonl").unwrap_or(stem).to_string();

				for line in log.take(10) {
					let Ok(line) = line else { break };

					// Try new JSON format first
					if let Ok(json_value) = serde_json::from_str::<serde_json::Value>(&line) {
						if json_value.get("type").and_then(|t| t.as_str()) == Some("SUMMARY") {
							if let Some(session_info_value) = json_value.get("session_info") {
								if let Ok(info) = serde_json::from_value::<SessionInfo>(session_info_value.clone()) {
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
pub fn find_most_recent_session_for_project(project_dir: &Path) -> Result<Option<String>, anyhow::Error> {
	let sessions_dir = get_sessions_dir()?;

	if !sessions_dir.exists() {
		return Ok(None);
	}

	// Get the basename of the current project directory
	let project_basename = project_dir.file_name().and_then(|n| n.to_str()).unwrap_or("");

	if project_basename.is_empty() {
		return Ok(None);
	}

	let mut matching_sessions: Vec<(String, u64)> = Vec::new();

	for entry in std_fs::read_dir(sessions_dir)? {
		let entry = entry?;
		let path = entry.path();

		if path.is_file() && path.extension().is_some_and(|ext| ext == "zst") {
			// file_stem() gives "name.jsonl" for "name.jsonl.zst"; strip the .jsonl suffix
			let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
			let name = stem.strip_suffix(".jsonl").unwrap_or(stem);

			// Session name format: YYMMDD-HHMMSS-basename-uuid. Match the basename
			// as a dash-delimited segment, not a raw substring — otherwise project
			// "app" also matches sessions of "myapp"/"application".
			if name.contains(&format!("-{project_basename}-")) {
				// Get file modification time for sorting
				if let Ok(metadata) = std_fs::metadata(&path) {
					if let Ok(modified) = metadata.modified() {
						if let Ok(duration) = modified.duration_since(std::time::SystemTime::UNIX_EPOCH) {
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
				if let Ok(tool_calls) = serde_json::from_value::<Vec<serde_json::Value>>(tool_calls_value.clone()) {
					for tool_call in tool_calls {
						if let Some(call_id) = tool_call.get("id").and_then(|id| id.as_str()) {
							// Look for a tool message with this call_id AFTER the assistant message
							let has_response = messages.iter().skip(i + 1).any(|response_msg| {
								response_msg.role == "tool"
									&& response_msg.tool_call_id.as_ref() == Some(&call_id.to_string())
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
				if let Ok(tool_calls) = serde_json::from_value::<Vec<serde_json::Value>>(tool_calls_value.clone()) {
					for tool_call in tool_calls {
						let call_id = tool_call.get("id").and_then(|id| id.as_str()).unwrap_or("").to_string();
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
							response_msg.role == "tool" && response_msg.tool_call_id.as_ref() == Some(&call_id)
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
				provider_items: None,
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
	total_cost: f64,
	saved_supervisor: crate::session::accounting::SupervisorAccounting,
	main_cost: Option<Option<f64>>,
	agent_runs: Vec<crate::session::tap_runs::TapJobInfo>,
	background_supervisor: Vec<(String, crate::session::accounting::SupervisorAccounting)>,
	session_info: Option<SessionInfo>,
	messages: Vec<Message>,
	restoration_messages: Vec<Message>,
	restoration_point_found: bool,
	runtime_state: SessionRuntimeState,
	plan: Option<crate::mcp::core::plan::storage::ExecutionPlan>,
	schedule: Option<Vec<crate::mcp::orchestration::schedule::storage::ScheduleEntry>>,
}

/// Field value when it has the expected type, `None` otherwise: one odd field
/// in a log entry must not fail the whole resume.
fn lenient<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
	D: Deserializer<'de>,
	T: DeserializeOwned,
{
	Ok(T::deserialize(serde_json::Value::deserialize(deserializer)?).ok())
}

/// `Some(value)` whenever the key is there, even as `null`. With `#[serde(default)]`
/// an absent key stays `None`: a snapshot without `main_cost` says nothing about it.
fn present<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Option<f64>>, D::Error> {
	Option::<f64>::deserialize(deserializer).map(Some)
}

fn fold_main_cost(folded: &mut Option<Option<f64>>, incoming: Option<Option<f64>>) {
	if let Some(incoming) = incoming {
		*folded = Some(match (*folded, incoming) {
			(None, value) => value,
			(Some(Some(old)), Some(new)) => Some(old.max(new)),
			_ => None,
		});
	}
}

#[derive(Deserialize)]
struct BackgroundSupervisorEntry {
	id: String,
	accounting: crate::session::accounting::SupervisorAccounting,
}

#[derive(Deserialize)]
struct SummaryEntry {
	#[serde(default, deserialize_with = "lenient")]
	timestamp: Option<u64>,
	session_info: Option<SessionInfo>,
}

/// Which snapshot fields a SUMMARY actually carries: `SessionInfo` fills absent
/// ones with defaults, and folding a default over saved accounting would erase it.
#[derive(Deserialize)]
struct SummaryPresence {
	session_info: SnapshotPresence,
}

#[derive(Deserialize)]
struct SnapshotPresence {
	#[serde(default, deserialize_with = "present")]
	main_cost: Option<Option<f64>>,
	#[serde(default)]
	supervisor_accounting: Option<IgnoredAny>,
}

/// A STATS checkpoint: the snapshot fields plus the counters it raises.
#[derive(Deserialize)]
struct StatsEntry {
	#[serde(default, deserialize_with = "lenient")]
	timestamp: Option<u64>,
	#[serde(default)]
	total_cost: Option<f64>,
	#[serde(default)]
	supervisor_accounting: Option<crate::session::accounting::SupervisorAccounting>,
	#[serde(default, deserialize_with = "present")]
	main_cost: Option<Option<f64>>,
	#[serde(default)]
	agent_runs: Option<Vec<crate::session::tap_runs::TapJobInfo>>,
	#[serde(default, deserialize_with = "lenient")]
	input_tokens: Option<u64>,
	#[serde(default, deserialize_with = "lenient")]
	output_tokens: Option<u64>,
	#[serde(default, deserialize_with = "lenient")]
	cache_read_tokens: Option<u64>,
	#[serde(default, deserialize_with = "lenient")]
	cache_write_tokens: Option<u64>,
	#[serde(default, deserialize_with = "lenient")]
	reasoning_tokens: Option<u64>,
	#[serde(default, deserialize_with = "lenient")]
	tool_calls: Option<u64>,
	#[serde(default, deserialize_with = "lenient")]
	total_api_time_ms: Option<u64>,
	#[serde(default, deserialize_with = "lenient")]
	total_tool_time_ms: Option<u64>,
	#[serde(default, deserialize_with = "lenient")]
	total_layer_time_ms: Option<u64>,
}

impl StatsEntry {
	/// Only ever raises: a cached-only request reports 0 non-cached tokens and must
	/// not overwrite the totals accumulated in the SUMMARY.
	fn raise_counters(&self, info: &mut SessionInfo) {
		if let Some(accounting) = &self.supervisor_accounting {
			info.supervisor_accounting.merge_max(accounting);
		}
		if let Some(total_cost) = self.total_cost {
			if total_cost > info.total_cost {
				info.total_cost = total_cost;
			}
		}
		info.input_tokens = info.input_tokens.max(self.input_tokens.unwrap_or(0));
		info.output_tokens = info.output_tokens.max(self.output_tokens.unwrap_or(0));
		info.cache_read_tokens = info.cache_read_tokens.max(self.cache_read_tokens.unwrap_or(0));
		info.cache_write_tokens = info.cache_write_tokens.max(self.cache_write_tokens.unwrap_or(0));
		info.reasoning_tokens = info.reasoning_tokens.max(self.reasoning_tokens.unwrap_or(0));
		info.tool_calls = info.tool_calls.max(self.tool_calls.unwrap_or(0));
		info.total_api_time_ms = info.total_api_time_ms.max(self.total_api_time_ms.unwrap_or(0));
		info.total_tool_time_ms = info.total_tool_time_ms.max(self.total_tool_time_ms.unwrap_or(0));
		info.total_layer_time_ms = info.total_layer_time_ms.max(self.total_layer_time_ms.unwrap_or(0));
	}
}

#[derive(Deserialize)]
struct CompressionPointEntry {
	#[serde(default, deserialize_with = "lenient")]
	compression_type: Option<String>,
	#[serde(default, deserialize_with = "lenient")]
	messages_removed: Option<u64>,
}

#[derive(Deserialize)]
struct TruncationPointEntry {
	#[serde(default, deserialize_with = "lenient")]
	message_count: Option<u64>,
}

/// COMMAND and OUTPUT_MODE_* entries both record the command line that caused them.
#[derive(Deserialize)]
struct CommandEntry {
	#[serde(default, deserialize_with = "lenient")]
	command: Option<String>,
}

#[derive(Deserialize)]
struct KnowledgeEntry {
	#[serde(default, deserialize_with = "lenient")]
	content: Option<String>,
}

#[derive(Deserialize)]
struct ToolCallEntry {
	#[serde(default, deserialize_with = "lenient")]
	tool_name: Option<String>,
	#[serde(default, deserialize_with = "lenient")]
	tool_id: Option<String>,
	#[serde(default, deserialize_with = "lenient")]
	parameters: Option<serde_json::Value>,
}

/// Parse a session log file line by line, extracting messages and session metadata.
///
/// Handles both the current JSON format and the legacy prefix-based format.
/// Returns the raw parsed state — callers decide which messages to use.
fn parse_log_lines(lines: impl Iterator<Item = Result<String>>) -> Result<ParsedLogLines> {
	let mut session_info: Option<SessionInfo> = None;
	let mut last_summary_timestamp: u64 = 0;
	let mut background_supervisor = Vec::new();
	let mut total_cost: f64 = 0.0;
	let mut saved_supervisor = crate::session::accounting::SupervisorAccounting::default();
	let mut main_cost: Option<Option<f64>> = None;
	let mut agent_runs = Vec::new();
	let mut messages: Vec<Message> = Vec::new();
	let mut restoration_point_found = false;
	let mut restoration_messages = Vec::new();
	let mut pending_tool_calls: Vec<serde_json::Value> = Vec::new();
	let mut runtime_state = SessionRuntimeState::default();
	let mut plan: Option<crate::mcp::core::plan::storage::ExecutionPlan> = None;
	let mut schedule: Option<Vec<crate::mcp::orchestration::schedule::storage::ScheduleEntry>> = None;

	// Process the file line by line to avoid loading the entire file into memory
	for line in lines {
		let line = line?;

		// Dispatch on the entry type alone: SUMMARY and STATS lines run to hundreds of KB
		// and most entry types are not read here, so only the arm that consumes a line parses it.
		if let Some(log_type) = entry_kind(&line) {
			runtime_state.apply(log_type, &line)?;
			match log_type {
				"BACKGROUND_SUPERVISOR" => {
					let entry: BackgroundSupervisorEntry =
						serde_json::from_str(&line).context("BACKGROUND_SUPERVISOR entry malformed")?;
					background_supervisor.push((entry.id, entry.accounting));
				}
				"SUMMARY" => {
					// SUMMARY is the source of truth - it contains complete session state
					let entry: SummaryEntry = serde_json::from_str(&line)?;
					if let Some(mut info) = entry.session_info {
						let presence: SummaryPresence = serde_json::from_str(&line)?;
						total_cost = total_cost.max(info.total_cost);
						if presence.session_info.supervisor_accounting.is_some() {
							saved_supervisor.merge_max(&info.supervisor_accounting);
						}
						fold_main_cost(&mut main_cost, presence.session_info.main_cost);
						// Moved, not copied: the load merges the folded history back into the
						// session, and a snapshot's own list is only part of it.
						crate::session::tap_runs::merge_history(&mut agent_runs, std::mem::take(&mut info.agent_runs));
						session_info = Some(info);
						// Track SUMMARY timestamp to ignore older STATS entries
						last_summary_timestamp = entry.timestamp.unwrap_or(0);
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
					let entry: CompressionPointEntry = serde_json::from_str(&line)?;
					if let (Some(comp_type), Some(msgs_removed)) = (entry.compression_type, entry.messages_removed) {
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
					let entry: TruncationPointEntry = serde_json::from_str(&line)?;
					if let Some(message_count) = entry.message_count {
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
				// Replace and Restart both rebuild the session from scratch: clear prior
				// messages here so the rebuilt snapshot (persisted as message lines after
				// this marker by command_executor) is the only state that survives reload.
				"OUTPUT_MODE_REPLACE" | "OUTPUT_MODE_RESTART" => {
					if restoration_point_found {
						restoration_messages.clear();
					} else {
						messages.clear();
					}
					pending_tool_calls.clear(); // Clear stale tool calls from before replace

					let entry: CommandEntry = serde_json::from_str(&line)?;
					if let Some(command) = entry.command {
						crate::log_debug!("Session restoration: Found {} from command '{}'", log_type, command);
					}
				}
				"STATS" => {
					// STATS entries provide incremental updates during a session
					let mut stats: StatsEntry = serde_json::from_str(&line)?;
					if let Some(stats_total_cost) = stats.total_cost {
						total_cost = total_cost.max(stats_total_cost);
					}
					if let Some(accounting) = &stats.supervisor_accounting {
						saved_supervisor.merge_max(accounting);
					}
					fold_main_cost(&mut main_cost, stats.main_cost);
					if let Some(runs) = stats.agent_runs.take() {
						crate::session::tap_runs::merge_history(&mut agent_runs, runs);
					}
					// Only apply STATS that are NEWER than the last SUMMARY: SUMMARY (written
					// on save/exit) is the source of truth, and old STATS must not overwrite it.
					if stats.timestamp.unwrap_or(0) > last_summary_timestamp {
						if let Some(info) = &mut session_info {
							stats.raise_counters(info);
						}
					}
				}
				"TOOL_CALL" => {
					// Collect tool calls to reconstruct assistant message with tool_calls
					let entry: ToolCallEntry = serde_json::from_str(&line)?;
					if let (Some(tool_name), Some(tool_id), Some(parameters)) =
						(entry.tool_name, entry.tool_id, entry.parameters)
					{
						// Store tool call for later reconstruction
						pending_tool_calls.push(serde_json::json!({
							"id": tool_id,
							"type": "function",
							"function": {
								"name": tool_name,
								"arguments": serde_json::to_string(&parameters).unwrap_or_default()
							}
						}));
					}
				}
				// Plan and schedule snapshots are small and rare: parsed as they arrive, the latest wins.
				"PLAN_SNAPSHOT" => {
					#[derive(Deserialize)]
					struct PlanSnapshot {
						plan: Option<crate::mcp::core::plan::storage::ExecutionPlan>,
					}
					match serde_json::from_str::<PlanSnapshot>(&line) {
						Ok(PlanSnapshot { plan: Some(latest) }) => plan = Some(latest),
						Ok(PlanSnapshot { plan: None }) => {}
						Err(e) => crate::log_debug!("restore_plan_for_session: deserialize failed: {}", e),
					}
				}
				"PLAN_CLEARED" => plan = None,
				"SCHEDULE_SNAPSHOT" => {
					#[derive(Deserialize)]
					struct ScheduleSnapshot {
						entries: Option<Vec<crate::mcp::orchestration::schedule::storage::ScheduleEntry>>,
					}
					match serde_json::from_str::<ScheduleSnapshot>(&line) {
						Ok(ScheduleSnapshot { entries: Some(latest) }) => schedule = Some(latest),
						Ok(ScheduleSnapshot { entries: None }) => {}
						Err(e) => crate::log_debug!("restore_schedule_for_session: deserialize failed: {}", e),
					}
				}
				// Everything else is irrelevant to message reconstruction: debug log
				// entries (API_REQUEST/RESPONSE, TOOL_RESULT, CACHE, ERROR,
				// SYSTEM/USER/ASSISTANT), markers consumed by other readers (COMMAND and
				// KNOWLEDGE_ENTRY feed the runtime state above; OUTPUT_MODE_APPEND/LAST only
				// add messages that follow them), and any unknown future types.
				_ => {}
			}
		} else if line.starts_with('{') && line.contains("\"role\":") && line.contains("\"content\":") {
			// This is a regular message JSON line
			let parsed = serde_json::from_str::<Message>(&line);
			if let Err(ref e) = parsed {
				// Don't silently drop — a lost line is silent history loss on
				// resume. Best-effort skip (one bad line must not nuke the whole
				// session) but make it visible.
				crate::log_error!("Skipping unparseable session message line on resume: {}", e);
			}
			if let Ok(message) = parsed {
				if let Some(run) = crate::session::tap_runs::recover_job(&message) {
					if !agent_runs.iter().any(|saved| saved.id == run.id) {
						agent_runs.push(run);
					}
				}
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
							tool_calls: Some(serde_json::Value::Array(pending_tool_calls.clone())),
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
		} else if !line.starts_with('{') {
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
		total_cost,
		saved_supervisor,
		main_cost,
		agent_runs,
		background_supervisor,
		session_info,
		messages,
		restoration_messages,
		restoration_point_found,
		runtime_state,
		plan,
		schedule,
	})
}

/// Build a Session from parsed log data when a SUMMARY entry was found.
///
/// Applies runtime state overrides (e.g. model changes from `/model` commands),
/// cleans up any interrupted tool calls, and returns the final Session.
fn reconstruct_messages(
	mut info: SessionInfo,
	final_messages: Vec<Message>,
	session_file: &Path,
	runtime_state: &SessionRuntimeState,
) -> Result<Session> {
	if let Some(model) = runtime_state.model.clone() {
		info.model = model;
	}

	let mut cleaned_messages = final_messages;
	if has_incomplete_tool_calls(&cleaned_messages) {
		clean_interrupted_tool_calls(&mut cleaned_messages, "Session restoration");
	}

	Ok(Session {
		info,
		messages: cleaned_messages,
		session_file: Some(session_file.to_path_buf()),
	})
}

/// Build a Session when no SUMMARY entry was found (legacy or corrupted session files).
///
/// Synthesises a default SessionInfo from the file path and any STATS entries,
/// then applies runtime state overrides.
fn restore_session_info(
	final_messages: Vec<Message>,
	session_file: &Path,
	runtime_state: &SessionRuntimeState,
) -> Result<Session> {
	// file_stem() on "name.jsonl.zst" yields "name.jsonl" — strip the residual
	// .jsonl so the session name matches what the logger writes under. Leaving it
	// forks all subsequent log writes to a differently-named file.
	let stem = session_file.file_stem().and_then(|s| s.to_str()).unwrap_or("unknown");
	let session_name = stem.strip_suffix(".jsonl").unwrap_or(stem).to_string();

	let default_model = "openrouter:anthropic/claude-sonnet-4".to_string();

	let created_at = session_file
		.metadata()
		.and_then(|meta| meta.created().ok().ok_or(std::io::Error::other("No creation time")))
		.and_then(|time| {
			time.duration_since(std::time::UNIX_EPOCH)
				.ok()
				.ok_or(std::io::Error::other("Invalid time"))
		})
		.map(|duration| duration.as_secs())
		.unwrap_or_else(|_| crate::utils::time::now_secs());

	let mut info = SessionInfo {
		name: session_name,
		created_at,
		model: default_model,
		last_cache_checkpoint_time: crate::utils::time::now_secs(),
		..Default::default()
	};

	if let Some(model) = runtime_state.model.clone() {
		info.model = model;
	}

	// Apply any STATS entries found in the file (best-effort token/cost recovery)
	for line in SessionLog::open(session_file)? {
		let line = line?;
		if entry_kind(&line) != Some("STATS") {
			continue;
		}
		if let Ok(json_value) = serde_json::from_str::<serde_json::Value>(&line) {
			if json_value.get("type").and_then(|t| t.as_str()) == Some("STATS") {
				if let Some(value) = json_value.get("supervisor_accounting") {
					info.supervisor_accounting
						.merge_max(&serde_json::from_value(value.clone())?);
				}
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
				if let Some(v) = json_value.get("cache_write_tokens").and_then(|t| t.as_u64()) {
					info.cache_write_tokens = v;
				}
				if let Some(v) = json_value.get("reasoning_tokens").and_then(|t| t.as_u64()) {
					info.reasoning_tokens = v;
				}
				if let Some(v) = json_value.get("tool_calls").and_then(|t| t.as_u64()) {
					info.tool_calls = v;
				}
				if let Some(v) = json_value.get("total_api_time_ms").and_then(|t| t.as_u64()) {
					info.total_api_time_ms = v;
				}
				if let Some(v) = json_value.get("total_tool_time_ms").and_then(|t| t.as_u64()) {
					info.total_tool_time_ms = v;
				}
				if let Some(v) = json_value.get("total_layer_time_ms").and_then(|t| t.as_u64()) {
					info.total_layer_time_ms = v;
				}
			}
		}
	}

	// stderr: stdout carries the run's data (`--format jsonl` events, plain answers).
	eprintln!("⚠️  Session loaded with default metadata (SUMMARY was missing)");
	Ok(Session {
		info,
		messages: final_messages,
		session_file: Some(session_file.to_path_buf()),
	})
}

/// A session read back from its log, with the runtime state (model, role, effort,
/// knowledge) found in the same pass.
pub struct LoadedSession {
	pub session: Session,
	pub runtime_state: SessionRuntimeState,
	/// Latest plan snapshot in the log; `None` when absent or cleared.
	pub plan: Option<crate::mcp::core::plan::storage::ExecutionPlan>,
	/// Entries of the latest schedule snapshot; `None` when the log has none.
	pub schedule: Option<Vec<crate::mcp::orchestration::schedule::storage::ScheduleEntry>>,
}

pub fn load_session(session_file: &PathBuf) -> Result<Session, anyhow::Error> {
	Ok(load_session_with_runtime_state(session_file)?.session)
}

pub fn load_session_with_runtime_state(session_file: &PathBuf) -> Result<LoadedSession, anyhow::Error> {
	if !session_file.exists() {
		return Err(anyhow::anyhow!("Session file does not exist"));
	}

	// Exclusive, and held through the cut: a torn final frame is cut under the same
	// lock that read it, so no append can land in between and bury it mid-log.
	let file = OpenOptions::new().read(true).write(true).open(session_file)?;
	file.lock()?;
	let mut log = SessionLog::new(BufReader::new(&file));
	let parsed = parse_log_lines(&mut log)?;
	if let Some(complete_len) = log.torn_at() {
		let torn_len = file.metadata()?.len() - complete_len;
		file.set_len(complete_len)?;
		file.sync_data()?;
		crate::log_info!(
			"Session log {}: cut a torn final frame ({torn_len} bytes) left by an interrupted append",
			session_file.display()
		);
	}
	drop(log);
	drop(file);

	let final_messages = if parsed.restoration_point_found && !parsed.restoration_messages.is_empty() {
		parsed.restoration_messages
	} else {
		parsed.messages
	};

	let mut session = if let Some(info) = parsed.session_info {
		reconstruct_messages(info, final_messages, session_file, &parsed.runtime_state)?
	} else {
		restore_session_info(final_messages, session_file, &parsed.runtime_state)?
	};
	session.info.total_cost = session.info.total_cost.max(parsed.total_cost);
	session.info.supervisor_accounting.merge_max(&parsed.saved_supervisor);
	if let Some(main_cost) = parsed.main_cost {
		session.info.main_cost = main_cost;
	}
	crate::session::tap_runs::merge_history(&mut session.info.agent_runs, parsed.agent_runs);
	for (id, accounting) in parsed.background_supervisor {
		session.info.apply_background_usage(id, &accounting);
	}
	Ok(LoadedSession {
		session,
		runtime_state: parsed.runtime_state,
		plan: parsed.plan,
		schedule: parsed.schedule,
	})
}

/// Runtime state extracted from session snapshots and commands
#[derive(Debug, Default)]
pub struct SessionRuntimeState {
	pub model: Option<String>,
	pub role: Option<String>, // Track runtime role changes
	pub reasoning_effort: Option<crate::config::ReasoningEffortConfig>,
	pub critical_knowledge: Vec<String>, // Knowledge entries from compressions
}

/// Extract runtime state from session log file
pub fn extract_runtime_state_from_log(session_file: &Path) -> Result<SessionRuntimeState> {
	let mut state = SessionRuntimeState::default();

	for line in SessionLog::open(session_file)? {
		let line = line?;
		if let Some(kind) = entry_kind(&line) {
			state.apply(kind, &line)?;
		}
	}
	Ok(state)
}

#[derive(Deserialize)]
struct RuntimeSummary {
	session_info: Option<RuntimeSummaryInfo>,
}

/// The two fields of `SessionInfo` the runtime state reads, so a SUMMARY line is
/// not built into a whole `SessionInfo` just for them.
#[derive(Deserialize)]
struct RuntimeSummaryInfo {
	model: String,
	// Legacy snapshots may predate persisted role metadata.
	#[serde(default)]
	role: String,
}

impl SessionRuntimeState {
	/// Fold one log entry into the state; entry kinds that do not affect it are ignored.
	fn apply(&mut self, kind: &str, line: &str) -> Result<()> {
		match kind {
			"SUMMARY" => {
				let entry: RuntimeSummary = serde_json::from_str(line)?;
				if let Some(info) = entry.session_info {
					// A saved snapshot supersedes earlier model and role commands.
					self.model = Some(info.model);
					if !info.role.is_empty() {
						self.role = Some(info.role);
					}
				}
			}
			// Reset state tracking after restoration point
			"RESTORATION_POINT" => *self = Self::default(),
			// Restore critical knowledge entries from compression cycles
			"KNOWLEDGE_ENTRY" => {
				let entry: KnowledgeEntry = serde_json::from_str(line)?;
				if let Some(content) = entry.content {
					self.critical_knowledge.push(content);
				}
			}
			// Process all commands to get the final state
			"COMMAND" => {
				let entry: CommandEntry = serde_json::from_str(line)?;
				if let Some(command) = entry.command {
					apply_command_to_runtime_state(self, &command);
				}
			}
			_ => {}
		}
		Ok(())
	}
}

/// The role a session should come back as when resumed without an explicit one.
///
/// Resuming is not starting: the session already knows what it was, so falling
/// back to the config default silently switches the agent (and its model, tools
/// and system prompt) out from under a conversation already in progress.
///
/// Uses the latest saved role or subsequent `/role` switch recorded in the log.
/// `None` when the session file is missing, unreadable, or records no role, in
/// which case the caller keeps its own default.
pub fn resume_role(session_name: &str) -> Option<String> {
	let session_file = get_sessions_dir().ok()?.join(format!("{}.jsonl.zst", session_name));
	if !session_file.exists() {
		return None;
	}
	if let Some(role) = extract_runtime_state_from_log(&session_file)
		.ok()
		.and_then(|state| state.role)
	{
		return Some(role);
	}
	// Fall back to the creation role in the SUMMARY header. Only the first few
	// lines are read — the whole log is not needed for this.
	for line in SessionLog::open(&session_file).ok()?.take(10) {
		let Ok(line) = line else { break };
		let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
			continue;
		};
		if value.get("type").and_then(|t| t.as_str()) != Some("SUMMARY") {
			continue;
		}
		let role = value
			.get("session_info")
			.and_then(|i| i.get("role"))
			.and_then(|r| r.as_str())?;
		if !role.is_empty() {
			return Some(role.to_string());
		}
	}
	None
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
		"/effort" if parts.len() > 1 => {
			if let Some(e) = crate::config::ReasoningEffortConfig::parse(parts[1]) {
				state.reasoning_effort = Some(e);
			}
		}
		_ => {
			// Unknown command, ignore
		}
	}
}

/// Build the SUMMARY log entry that snapshots full session metadata.
/// SUMMARY is the source of truth for session state on reload, so every save
/// path (initial file creation and incremental saves) writes one through here.
pub fn summary_log_entry(info: &SessionInfo) -> serde_json::Value {
	let mut info = info.clone();
	info.capture_agent_runs();
	summary_entry_of(&info)
}

/// The SUMMARY entry for exactly the metadata given, nothing captured or added.
pub(crate) fn summary_entry_of(info: &SessionInfo) -> serde_json::Value {
	serde_json::json!({
		"type": "SUMMARY",
		"timestamp": crate::utils::time::now_secs(),
		"session_info": info,
	})
}

// Helper function to append to session file as an independent zstd frame.
// Each call writes one complete frame; the decoder reads all frames sequentially on load.
//
// A frame lands whole or not at all, and is durable once this returns: it is
// compressed in memory, written in one call and synced before the lock is
// released, and any failure cuts the file back to its previous length. Without
// the sync, a write the storage lost (an NFS server that never received it)
// surfaced too late: later appends already sat past it, and the lost frame
// stayed behind as zero bytes that made the whole session unloadable.
pub fn append_to_session_file(session_file: &PathBuf, content: &str) -> Result<(), anyhow::Error> {
	// Ensure content is on a single line - replace any newlines with spaces
	let line = format!("{}\n", content.replace(['\n', '\r'], " "));
	let frame = zstd::encode_all(line.as_bytes(), 1)?;

	// Not append mode: the end is found under the lock instead, which also
	// yields the length a failed write is cut back to.
	let mut file = OpenOptions::new()
		.create(true)
		.truncate(false)
		.write(true)
		.open(session_file)?;
	// A detached distiller can append while the parent saves its final SUMMARY.
	// Keep each compressed frame intact across both writers.
	file.lock()?;

	let length = file.seek(SeekFrom::End(0))?;
	if let Err(error) = file.write_all(&frame).and_then(|()| file.sync_data()) {
		file.set_len(length).and_then(|()| file.sync_data()).with_context(|| {
			format!(
				"append to {} failed ({error}) and the partial frame could not be removed",
				session_file.display()
			)
		})?;
		return Err(error).with_context(|| format!("append to {} failed", session_file.display()));
	}
	Ok(())
}

/// Every zstd frame starts with these bytes (RFC 8878 §3.1.1).
const ZSTD_FRAME_MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];

/// The `type` of a log entry, read without building the entry: serde skips
/// every other field. `None` for a message line (it has no `type`) and for a
/// line that is not a JSON object. Borrowed, so a `type` holding an escape also
/// reads as `None`; every writer emits a plain ASCII literal.
pub(crate) fn entry_kind(line: &str) -> Option<&str> {
	#[derive(Deserialize)]
	struct Kind<'a> {
		#[serde(borrow, rename = "type")]
		kind: Option<&'a str>,
	}
	serde_json::from_str::<Kind>(line).ok()?.kind
}

/// A session log, read line by line and frame by frame.
///
/// Every append is one zstd frame, acknowledged only once synced (see
/// `append_to_session_file`). A frame cut short by the end of the file is an
/// append still being written, or one a crash stopped before its sync — never
/// acknowledged, so the log ends before it. Anything else that fails to
/// decode is corruption and is returned as an error: no reader skips past it.
pub struct SessionLog<R> {
	input: R,
	lines: VecDeque<String>,
	// Bytes of a line still open at the end of the last frame read.
	unterminated: Vec<u8>,
	// Where the last complete frame ends.
	complete_len: u64,
	torn: bool,
	finished: bool,
}

impl SessionLog<BufReader<File>> {
	pub fn open(session_file: &Path) -> Result<Self> {
		let file = File::open(session_file)
			.with_context(|| format!("failed to open session log {}", session_file.display()))?;
		Ok(Self::new(BufReader::new(file)))
	}
}

impl<R: BufRead + Seek> SessionLog<R> {
	pub fn new(input: R) -> Self {
		Self {
			input,
			lines: VecDeque::new(),
			unterminated: Vec::new(),
			complete_len: 0,
			torn: false,
			finished: false,
		}
	}

	/// Length of the log's complete frames, when it ended on a torn final frame.
	pub fn torn_at(&self) -> Option<u64> {
		self.torn.then_some(self.complete_len)
	}

	/// Decode the next frame into `lines`; false once the log has ended.
	fn read_frame(&mut self) -> Result<bool> {
		if self.input.fill_buf()?.is_empty() {
			return Ok(false);
		}
		let mut frame = Vec::new();
		let decoded = ZstdDecoder::with_buffer(&mut self.input)
			.map(|decoder| decoder.single_frame())
			.and_then(|mut decoder| decoder.read_to_end(&mut frame));
		if let Err(error) = decoded {
			// zstd reports UnexpectedEof only once its input ran out mid-frame. That is a
			// torn final frame unless a later frame starts inside the bytes it read: then
			// a damaged header swallowed acknowledged frames, which is corruption.
			if error.kind() == ErrorKind::UnexpectedEof && !self.frame_starts_after_last_complete()? {
				self.torn = true;
				return Ok(false);
			}
			return Err(
				anyhow::Error::new(error).context(format!("corrupt session log frame at byte {}", self.complete_len))
			);
		}
		self.complete_len = self.input.stream_position()?;

		self.unterminated.extend_from_slice(&frame);
		let mut start = 0;
		while let Some(end) = self.unterminated[start..].iter().position(|&byte| byte == b'\n') {
			let line = &self.unterminated[start..start + end];
			let line = line.strip_suffix(b"\r").unwrap_or(line);
			self.lines.push_back(String::from_utf8(line.to_vec())?);
			start += end + 1;
		}
		self.unterminated.drain(..start);
		Ok(true)
	}

	/// Whether the bytes read past the last complete frame hold the start of
	/// another one. Only what the failed decode consumed is scanned, so an append
	/// landing since cannot change the answer for a reader that holds no lock.
	fn frame_starts_after_last_complete(&mut self) -> Result<bool> {
		let read_to = self.input.stream_position()?;
		self.input.seek(SeekFrom::Start(self.complete_len))?;
		let mut consumed = vec![0; usize::try_from(read_to - self.complete_len)?];
		self.input.read_exact(&mut consumed)?;
		Ok(consumed
			.windows(ZSTD_FRAME_MAGIC.len())
			.skip(1)
			.any(|window| window == ZSTD_FRAME_MAGIC))
	}
}

impl<R: BufRead + Seek> Iterator for SessionLog<R> {
	type Item = Result<String>;

	fn next(&mut self) -> Option<Self::Item> {
		loop {
			if let Some(line) = self.lines.pop_front() {
				return Some(Ok(line));
			}
			if self.finished {
				return None;
			}
			match self.read_frame() {
				Ok(true) => {}
				Ok(false) => {
					self.finished = true;
					// A last line written without its newline (older logs) is still a line.
					if !self.unterminated.is_empty() {
						return Some(String::from_utf8(std::mem::take(&mut self.unterminated)).map_err(Into::into));
					}
				}
				Err(error) => {
					self.finished = true;
					return Some(Err(error));
				}
			}
		}
	}
}

/// The session log as plain JSONL, one line per entry — what `/share` and
/// `/analyze` hand out.
pub fn session_log_jsonl(compressed: &[u8]) -> Result<Vec<u8>> {
	let mut jsonl = Vec::new();
	for line in SessionLog::new(Cursor::new(compressed)) {
		jsonl.extend_from_slice(line?.as_bytes());
		jsonl.push(b'\n');
	}
	Ok(jsonl)
}

#[cfg(test)]
#[path = "persistence_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "persistence_runtime_tests.rs"]
mod runtime_tests;

#[cfg(test)]
#[path = "persistence_unit_tests.rs"]
mod unit_tests;
