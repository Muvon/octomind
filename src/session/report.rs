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

// Session report generation module

use crate::log_debug;
use crate::session::chat::formatting::format_duration;
use crate::session::chat::markdown::MarkdownRenderer;
use anyhow::Result;
use serde_json::Value;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionReport {
	pub entries: Vec<ReportEntry>,
	pub totals: ReportTotals,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ReportEntry {
	pub user_request: String,
	pub cost: String,
	pub tool_calls: u32,
	pub tools_used: String,
	pub task_time: String,
	pub ai_time: String,
	pub processing_time: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ReportTotals {
	pub total_cost: f64,
	pub total_tool_calls: u32,
	pub total_task_time_ms: u64,
	pub total_ai_time_ms: u64,
	pub total_processing_time_ms: u64,
	pub total_requests: u32,
}

#[derive(Debug, Clone)]
struct RequestContext {
	pub user_request: String,
	pub start_timestamp: u64,
	pub end_timestamp: u64, // Last activity timestamp for this request
	pub cost_before: f64,
	pub cost_after: f64,
	pub tools: HashMap<String, u32>,
	pub api_time_before: u64,  // Total API time before this request
	pub api_time_after: u64,   // Total API time after this request
	pub tool_time_before: u64, // Total tool time before this request
	pub tool_time_after: u64,  // Total tool time after this request
}

impl SessionReport {
	/// Generate a session report from the session log file
	pub fn generate_from_log(session_log_path: &str) -> Result<SessionReport> {
		// Session logs are zstd-compressed JSONL — decode before reading lines.
		let file = File::open(session_log_path)?;
		let reader = BufReader::new(zstd::stream::read::Decoder::new(file)?);

		let mut contexts: Vec<RequestContext> = Vec::new();
		let mut current_context: Option<RequestContext> = None;
		let mut last_total_cost = 0.0;
		let mut last_total_api_time_ms = 0u64;
		let mut last_total_tool_time_ms = 0u64;

		// Read all log entries
		let mut all_entries: Vec<Value> = Vec::new();
		for line in reader.lines() {
			let line = line?;
			if line.trim().is_empty() {
				continue;
			}
			if let Ok(log_entry) = serde_json::from_str::<Value>(&line) {
				all_entries.push(log_entry);
			}
		}

		// Process entries and track cost/time
		for log_entry in all_entries.iter() {
			// User messages are persisted as `Message` structs with `role:"user"` (no
			// `type` field). Synthesize "USER" so the request-context machinery picks
			// them up alongside `{"type":"COMMAND",…}` entries. Without this, /report
			// only ever shows slash commands and skips real chat turns.
			let role = log_entry.get("role").and_then(|r| r.as_str()).unwrap_or("");
			let log_type = log_entry
				.get("type")
				.and_then(|t| t.as_str())
				.unwrap_or_else(|| if role == "user" { "USER" } else { "" });
			let entry_timestamp = log_entry
				.get("timestamp")
				.and_then(|t| t.as_u64())
				.unwrap_or(0);

			match log_type {
				"STATS" => {
					// Update last known totals from session stats
					if let Some(total_cost) = log_entry.get("total_cost").and_then(|c| c.as_f64()) {
						last_total_cost = total_cost;
					}
					if let Some(total_api_time) =
						log_entry.get("total_api_time_ms").and_then(|t| t.as_u64())
					{
						last_total_api_time_ms = total_api_time;
					}
					if let Some(total_tool_time) =
						log_entry.get("total_tool_time_ms").and_then(|t| t.as_u64())
					{
						last_total_tool_time_ms = total_tool_time;
					}
				}
				"USER" | "COMMAND" => {
					// Save previous context if exists
					if let Some(mut ctx) = current_context.take() {
						ctx.cost_after = last_total_cost;
						ctx.api_time_after = last_total_api_time_ms;
						ctx.tool_time_after = last_total_tool_time_ms;
						contexts.push(ctx);
					}

					// Start new context
					let content = if log_type == "USER" {
						log_entry
							.get("content")
							.and_then(|c| c.as_str())
							.unwrap_or("")
							.to_string()
					} else {
						log_entry
							.get("command")
							.and_then(|c| c.as_str())
							.unwrap_or("")
							.to_string()
					};

					current_context = Some(RequestContext {
						user_request: content,
						start_timestamp: entry_timestamp,
						end_timestamp: entry_timestamp,
						cost_before: last_total_cost,
						cost_after: last_total_cost,
						tools: HashMap::new(),
						api_time_before: last_total_api_time_ms,
						api_time_after: last_total_api_time_ms,
						tool_time_before: last_total_tool_time_ms,
						tool_time_after: last_total_tool_time_ms,
					});
				}
				"TOOL_CALL" => {
					// Track tool usage (test-only log type)
					if let Some(ref mut ctx) = current_context {
						if let Some(tool_name) = log_entry.get("tool_name").and_then(|t| t.as_str())
						{
							*ctx.tools.entry(tool_name.to_string()).or_insert(0) += 1;
						}
					}
				}
				_ => {
					// Assistant messages with tool_calls embedded (production format)
					if role == "assistant" {
						if let Some(ref mut ctx) = current_context {
							if let Some(tool_calls) =
								log_entry.get("tool_calls").and_then(|tc| tc.as_array())
							{
								for tc in tool_calls {
									if let Some(tool_name) = tc.get("name").and_then(|n| n.as_str())
									{
										*ctx.tools.entry(tool_name.to_string()).or_insert(0) += 1;
									}
								}
							}
						}
					}
					// SUMMARY entries carry the running totals via `session_info`. STATS
					// entries are only emitted in tests, so SUMMARY is the *only* place
					// real sessions get their time/cost rollup from. Pull cost AND time
					// fields here — without the time pulls, ai/processing always read 0ms.
					if let Some(session_info) = log_entry.get("session_info") {
						if let Some(total_cost) =
							session_info.get("total_cost").and_then(|c| c.as_f64())
						{
							last_total_cost = total_cost;
						}
						if let Some(api_ms) = session_info
							.get("total_api_time_ms")
							.and_then(|t| t.as_u64())
						{
							last_total_api_time_ms = api_ms;
						}
						if let Some(tool_ms) = session_info
							.get("total_tool_time_ms")
							.and_then(|t| t.as_u64())
						{
							last_total_tool_time_ms = tool_ms;
						}
					}
				}
			}

			// Track end timestamp for task time: last activity during this request
			if log_type != "USER" && log_type != "COMMAND" {
				if let Some(ref mut ctx) = current_context {
					if entry_timestamp > ctx.end_timestamp {
						ctx.end_timestamp = entry_timestamp;
					}
				}
			}
		}

		// Save the last context if exists
		if let Some(mut ctx) = current_context {
			ctx.cost_after = last_total_cost;
			ctx.api_time_after = last_total_api_time_ms;
			ctx.tool_time_after = last_total_tool_time_ms;
			contexts.push(ctx);
		}

		// Convert contexts to report entries
		let mut entries = Vec::new();
		let mut totals = ReportTotals {
			total_cost: 0.0,
			total_tool_calls: 0,
			total_task_time_ms: 0,
			total_ai_time_ms: 0,
			total_processing_time_ms: 0,
			total_requests: 0,
		};

		for (i, ctx) in contexts.iter().enumerate() {
			let tool_calls: u32 = ctx.tools.values().sum();
			let tools_used = Self::format_tools_used(&ctx.tools);
			let cost_delta = ctx.cost_after - ctx.cost_before;

			// AI Time = API latency delta from STATS entries
			let ai_time_ms = ctx.api_time_after.saturating_sub(ctx.api_time_before);

			// Processing Time = Tool execution time delta from STATS entries
			let processing_time_ms = ctx.tool_time_after.saturating_sub(ctx.tool_time_before);

			// Task time = wall-clock time from user input to last activity for this request
			let task_time_ms = if ctx.end_timestamp > ctx.start_timestamp {
				(ctx.end_timestamp - ctx.start_timestamp) * 1000 // Convert to ms
			} else if i + 1 < contexts.len() {
				// Fallback: no end_timestamp tracked (e.g. commands with no activity)
				0
			} else {
				// Last request still in progress — measure to now
				let current_timestamp = std::time::SystemTime::now()
					.duration_since(std::time::UNIX_EPOCH)
					.unwrap_or_default()
					.as_secs();

				if current_timestamp > ctx.start_timestamp {
					(current_timestamp - ctx.start_timestamp) * 1000 // Convert to ms
				} else {
					0
				}
			};

			totals.total_cost += cost_delta;
			totals.total_tool_calls += tool_calls;
			totals.total_task_time_ms += task_time_ms;
			totals.total_ai_time_ms += ai_time_ms;
			totals.total_processing_time_ms += processing_time_ms;
			totals.total_requests += 1;

			// Debug output to understand what we're getting
			log_debug!(
				"Request: '{}', Cost delta: {:.5}, AI time: {}ms, Processing time: {}ms",
				ctx.user_request,
				cost_delta,
				ai_time_ms,
				processing_time_ms
			);

			// Debug task time calculation
			log_debug!(
				"Task time calc: start={}, end={}, task_time_ms={}",
				ctx.start_timestamp,
				ctx.end_timestamp,
				task_time_ms
			);

			entries.push(ReportEntry {
				user_request: Self::truncate_request(&ctx.user_request, 35),
				cost: format!("{:.5}", cost_delta),
				tool_calls,
				tools_used,
				task_time: format_duration(task_time_ms),
				ai_time: format_duration(ai_time_ms),
				processing_time: format_duration(processing_time_ms),
			});
		}

		Ok(SessionReport { entries, totals })
	}

	/// Format tools used as "tool_name(count), tool_name(count)"
	fn format_tools_used(tools: &HashMap<String, u32>) -> String {
		if tools.is_empty() {
			return "-".to_string();
		}

		let mut tool_list: Vec<String> = tools
			.iter()
			.map(|(name, count)| format!("{}({})", name, count))
			.collect();
		tool_list.sort();
		tool_list.join(", ")
	}

	/// Truncate long user requests for table display
	fn truncate_request(request: &str, max_len: usize) -> String {
		if request.chars().count() <= max_len {
			request.to_string()
		} else {
			let truncated: String = request.chars().take(max_len - 3).collect();
			format!("{}...", truncated)
		}
	}

	/// Generate markdown table for the report
	pub fn generate_markdown_table(&self) -> String {
		let mut markdown = String::new();

		// Table header
		markdown.push_str("| User Request | Cost ($) | Tool Calls | Tools Used | Task Time | AI Time | Processing Time |\n");
		markdown.push_str("|--------------|----------|------------|------------|-----------|---------|----------------|\n");

		// Table rows
		for entry in &self.entries {
			markdown.push_str(&format!(
				"| {} | {} | {} | {} | {} | {} | {} |\n",
				self.escape_markdown(&entry.user_request),
				entry.cost,
				entry.tool_calls,
				self.escape_markdown(&entry.tools_used),
				entry.task_time,
				entry.ai_time,
				entry.processing_time
			));
		}

		// Totals row
		markdown.push_str(&format!(
			"| **TOTAL** | **{:.5}** | **{}** | **{} total calls** | **{}** | **{}** | **{}** |\n",
			self.totals.total_cost,
			self.totals.total_tool_calls,
			self.totals.total_tool_calls,
			format_duration(self.totals.total_task_time_ms),
			format_duration(self.totals.total_ai_time_ms),
			format_duration(self.totals.total_processing_time_ms)
		));

		markdown
	}

	/// Escape markdown special characters in table cells
	fn escape_markdown(&self, text: &str) -> String {
		text.replace("|", "\\|")
			.replace("\n", " ")
			.replace("\r", "")
	}

	/// Display the report with summary information using markdown rendering
	pub fn display(&self, config: &crate::config::Config) {
		// Generate the full markdown report
		let markdown_report = self.to_markdown_string();

		// Render using markdown renderer if enabled
		if config.enable_markdown_rendering {
			let theme = config.markdown_theme.parse().unwrap_or_default();
			let renderer = MarkdownRenderer::with_theme(theme);
			match renderer.render_and_print(&markdown_report) {
				Ok(_) => {
					// Successfully rendered as markdown
				}
				Err(_) => {
					// Fallback to plain text if markdown rendering fails
					self.display_plain(&markdown_report);
				}
			}
		} else {
			// Use plain text rendering
			self.display_plain(&markdown_report);
		}
	}

	/// Generate markdown report as string
	pub fn to_markdown_string(&self) -> String {
		let mut markdown_report = String::new();

		// Header
		markdown_report.push_str("# 📊 Session Usage Report\n\n");

		// Table
		markdown_report.push_str(&self.generate_markdown_table());
		markdown_report.push('\n');

		// Summary
		markdown_report.push_str(&format!(
			"## 📈 Summary\n\n**{}** requests • **${:.5}** total cost • **{}** tool calls • **{}** task time • **{}** AI time • **{}** processing time\n",
			self.totals.total_requests,
			self.totals.total_cost,
			self.totals.total_tool_calls,
			format_duration(self.totals.total_task_time_ms),
			format_duration(self.totals.total_ai_time_ms),
			format_duration(self.totals.total_processing_time_ms)
		));

		markdown_report
	}

	/// Generate plain text report (for WebSocket/API)
	pub fn to_plain_string(&self) -> String {
		let markdown = self.to_markdown_string();
		// Convert markdown to plain text
		markdown
			.replace("# ", "")
			.replace("## ", "")
			.replace("**", "")
			.replace("|", " ")
	}

	/// Generate structured JSON report (for WebSocket/API)
	pub fn to_json(&self) -> serde_json::Value {
		serde_json::json!({
			"entries": self.entries.iter().map(|e| serde_json::json!({
				"user_request": e.user_request,
				"cost": e.cost,
				"tool_calls": e.tool_calls,
				"tools_used": e.tools_used,
				"task_time": e.task_time,
				"ai_time": e.ai_time,
				"processing_time": e.processing_time
			})).collect::<Vec<_>>(),
			"totals": {
				"total_cost": self.totals.total_cost,
				"total_tool_calls": self.totals.total_tool_calls,
				"total_task_time_ms": self.totals.total_task_time_ms,
				"total_ai_time_ms": self.totals.total_ai_time_ms,
				"total_processing_time_ms": self.totals.total_processing_time_ms,
				"total_requests": self.totals.total_requests
			}
		})
	}

	/// Display report as plain text (fallback)
	fn display_plain(&self, markdown_report: &str) {
		// Convert markdown to plain text for fallback
		let plain_text = markdown_report
			.replace("# ", "")
			.replace("## ", "")
			.replace("**", "")
			.replace("*", "");
		// Use print! instead of println! to avoid extra newline since content may already have them
		print!("{}", plain_text);
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn entry(user_request: &str, tools_used: &str) -> ReportEntry {
		ReportEntry {
			user_request: user_request.to_string(),
			cost: "0.01000".to_string(),
			tool_calls: 2,
			tools_used: tools_used.to_string(),
			task_time: "1.0s".to_string(),
			ai_time: "0.5s".to_string(),
			processing_time: "0.5s".to_string(),
		}
	}

	fn report(entries: Vec<ReportEntry>) -> SessionReport {
		let total_requests = entries.len() as u32;
		SessionReport {
			entries,
			totals: ReportTotals {
				total_cost: 0.02,
				total_tool_calls: 4,
				total_task_time_ms: 2_000,
				total_ai_time_ms: 1_000,
				total_processing_time_ms: 1_000,
				total_requests,
			},
		}
	}

	#[test]
	fn tools_used_is_sorted_and_dash_when_empty() {
		assert_eq!(SessionReport::format_tools_used(&HashMap::new()), "-");

		let tools = HashMap::from([
			("shell".to_string(), 3u32),
			("read".to_string(), 1u32),
			("write".to_string(), 2u32),
		]);
		// Sorted so the same tool set always renders identically.
		assert_eq!(
			SessionReport::format_tools_used(&tools),
			"read(1), shell(3), write(2)"
		);
	}

	#[test]
	fn truncate_request_respects_the_cap_in_chars() {
		assert_eq!(SessionReport::truncate_request("short", 35), "short");
		// Exactly at the cap is untouched.
		let exact = "x".repeat(35);
		assert_eq!(SessionReport::truncate_request(&exact, 35), exact);

		let long = "y".repeat(100);
		let out = SessionReport::truncate_request(&long, 35);
		assert_eq!(out.chars().count(), 35);
		assert!(out.ends_with("..."));
	}

	#[test]
	fn truncate_request_does_not_split_multibyte_chars() {
		// A byte-based slice would panic here.
		let long = "日".repeat(100);
		let out = SessionReport::truncate_request(&long, 35);
		assert_eq!(out.chars().count(), 35);
		assert!(out.starts_with('日'));
	}

	#[test]
	fn escape_markdown_protects_table_cells() {
		let r = report(vec![]);
		// A raw pipe or newline in a cell would break the table layout.
		assert_eq!(r.escape_markdown("a|b"), "a\\|b");
		assert_eq!(r.escape_markdown("line1\nline2"), "line1 line2");
		assert_eq!(r.escape_markdown("crlf\r\n"), "crlf ");
	}

	#[test]
	fn markdown_table_escapes_every_cell_it_renders() {
		let r = report(vec![entry("fix a|b bug", "shell|read(1)")]);
		let table = r.generate_markdown_table();
		let row = table
			.lines()
			.find(|l| l.contains("fix a"))
			.expect("entry row present");
		// The row keeps exactly the 8 pipes of a 7-column table — the two
		// pipes coming from the data are escaped, not counted as separators.
		assert_eq!(row.matches("\\|").count(), 2);
		assert_eq!(row.replace("\\|", "").matches('|').count(), 8);
		assert!(table.contains("**TOTAL**"));
	}

	#[test]
	fn markdown_table_has_header_separator_and_one_row_per_entry() {
		let r = report(vec![entry("first", "read(1)"), entry("second", "-")]);
		let table = r.generate_markdown_table();
		let lines: Vec<&str> = table.lines().collect();
		// header + separator + 2 entries + totals
		assert_eq!(lines.len(), 5);
		assert!(lines[1].starts_with("|---"));
		assert!(lines[2].contains("first"));
		assert!(lines[3].contains("second"));
		assert!(lines[4].contains("**TOTAL**"));
	}

	#[test]
	fn json_report_mirrors_entries_and_totals() {
		let r = report(vec![entry("do a thing", "read(1)")]);
		let json = r.to_json();
		assert_eq!(json["entries"].as_array().unwrap().len(), 1);
		assert_eq!(json["entries"][0]["user_request"], "do a thing");
		assert_eq!(json["totals"]["total_requests"], 1);
		assert_eq!(json["totals"]["total_tool_calls"], 4);
	}

	#[test]
	fn plain_string_drops_markdown_markers() {
		let r = report(vec![entry("do a thing", "read(1)")]);
		let plain = r.to_plain_string();
		assert!(!plain.contains("**"));
		assert!(!plain.contains('|'));
		assert!(plain.contains("Session Usage Report"));
		assert!(plain.contains("do a thing"));
	}

	#[test]
	fn empty_report_still_renders_a_table_and_summary() {
		let mut r = report(vec![]);
		r.totals.total_requests = 0;
		let md = r.to_markdown_string();
		assert!(md.contains("| **TOTAL** |"));
		assert!(md.contains("**0** requests"));
	}

	#[test]
	fn generate_from_log_tracks_messages_commands_tools_cost_and_time() {
		let dir = tempfile::tempdir().expect("temp dir");
		let path = dir.path().join("report.jsonl.zst");
		let entries = [
			serde_json::json!({"role":"user","content":"build the report","timestamp":100}),
			serde_json::json!({
				"role":"assistant",
				"content":"working",
				"timestamp":102,
				"tool_calls":[{"name":"shell"},{"name":"view"}]
			}),
			serde_json::json!({
				"type":"STATS","timestamp":103,"total_cost":0.5,
				"total_api_time_ms":100,"total_tool_time_ms":30
			}),
			serde_json::json!({"type":"COMMAND","command":"/info","timestamp":110}),
			serde_json::json!({"type":"TOOL_CALL","tool_name":"schedule","timestamp":111}),
			serde_json::json!({
				"type":"SUMMARY","timestamp":112,
				"session_info":{
					"total_cost":1.0,"total_api_time_ms":160,"total_tool_time_ms":50
				}
			}),
			serde_json::json!({"role":"assistant","content":"done","timestamp":114}),
		];
		for entry in entries {
			crate::session::append_to_session_file(&path, &entry.to_string())
				.expect("append report frame");
		}

		let report = SessionReport::generate_from_log(path.to_str().unwrap()).expect("report");
		assert_eq!(report.entries.len(), 2);
		assert_eq!(report.entries[0].user_request, "build the report");
		assert_eq!(report.entries[0].tool_calls, 2);
		assert_eq!(report.entries[0].tools_used, "shell(1), view(1)");
		assert_eq!(report.entries[0].cost, "0.50000");
		assert_eq!(report.entries[1].user_request, "/info");
		assert_eq!(report.entries[1].tools_used, "schedule(1)");
		assert_eq!(report.totals.total_requests, 2);
		assert_eq!(report.totals.total_tool_calls, 3);
		assert!((report.totals.total_cost - 1.0).abs() < f64::EPSILON);
		assert_eq!(report.totals.total_ai_time_ms, 160);
		assert_eq!(report.totals.total_processing_time_ms, 50);
		assert_eq!(report.totals.total_task_time_ms, 7_000);
	}

	#[test]
	fn generate_from_log_rejects_missing_or_invalid_zstd_files() {
		let dir = tempfile::tempdir().expect("temp dir");
		let missing = dir.path().join("missing.zst");
		assert!(SessionReport::generate_from_log(missing.to_str().unwrap()).is_err());

		let invalid = dir.path().join("invalid.zst");
		std::fs::write(&invalid, b"not zstd").unwrap();
		assert!(SessionReport::generate_from_log(invalid.to_str().unwrap()).is_err());
	}
}
