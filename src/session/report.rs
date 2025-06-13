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

// Session report generation module

use crate::log_debug;
use crate::session::chat::formatting::format_duration;
use crate::session::chat::markdown::MarkdownRenderer;
use anyhow::Result;
use serde_json::Value;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};

#[derive(Debug, Clone)]
pub struct SessionReport {
	pub entries: Vec<ReportEntry>,
	pub totals: ReportTotals,
}

#[derive(Debug, Clone)]
pub struct ReportEntry {
	pub user_request: String,
	pub cost: String,
	pub tool_calls: u32,
	pub tools_used: String,
	pub human_time: String,
	pub ai_time: String,
	pub processing_time: String,
}

#[derive(Debug, Clone)]
pub struct ReportTotals {
	pub total_cost: f64,
	pub total_tool_calls: u32,
	pub total_human_time_ms: u64,
	pub total_ai_time_ms: u64,
	pub total_processing_time_ms: u64,
	pub total_requests: u32,
}

#[derive(Debug, Clone)]
struct RequestContext {
	pub user_request: String,
	pub start_timestamp: u64,
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
		let file = File::open(session_log_path)?;
		let reader = BufReader::new(file);

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
			let log_type = log_entry.get("type").and_then(|t| t.as_str()).unwrap_or("");

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

					let timestamp = log_entry
						.get("timestamp")
						.and_then(|t| t.as_u64())
						.unwrap_or(0);

					current_context = Some(RequestContext {
						user_request: content,
						start_timestamp: timestamp,
						cost_before: last_total_cost,
						cost_after: last_total_cost,
						tools: HashMap::new(),
						api_time_before: last_total_api_time_ms,
						api_time_after: last_total_api_time_ms,
						tool_time_before: last_total_tool_time_ms,
						tool_time_after: last_total_tool_time_ms,
					});
				}
				"API_RESPONSE" => {
					// API responses are now tracked via STATS entries for timing
					// We don't need to extract timing here anymore
				}
				"TOOL_CALL" => {
					// Track tool usage
					if let Some(ref mut ctx) = current_context {
						if let Some(tool_name) = log_entry.get("tool_name").and_then(|t| t.as_str())
						{
							*ctx.tools.entry(tool_name.to_string()).or_insert(0) += 1;
						}
					}
				}
				"TOOL_RESULT" => {
					// Tool execution time is now tracked via STATS entries
					// We don't need to extract timing here anymore
				}
				_ => {
					// Check for any other entries that might contain session cost updates
					if let Some(session_info) = log_entry.get("session_info") {
						if let Some(total_cost) =
							session_info.get("total_cost").and_then(|c| c.as_f64())
						{
							last_total_cost = total_cost;
						}
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
			total_human_time_ms: 0,
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

			// Calculate human time (time until next request or current time)
			let human_time_ms = if i + 1 < contexts.len() {
				// Time to next request
				let next_ctx = &contexts[i + 1];
				if next_ctx.start_timestamp > ctx.start_timestamp {
					(next_ctx.start_timestamp - ctx.start_timestamp) * 1000 // Convert to ms
				} else {
					0
				}
			} else {
				// Last request - calculate time from request to current time
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
			totals.total_human_time_ms += human_time_ms;
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

			// Debug human time calculation
			log_debug!(
				"Human time calc: timestamp={}, next_timestamp={}, human_time_ms={}",
				ctx.start_timestamp,
				if i + 1 < contexts.len() {
					contexts[i + 1].start_timestamp
				} else {
					0
				},
				human_time_ms
			);

			entries.push(ReportEntry {
				user_request: Self::truncate_request(&ctx.user_request, 35),
				cost: format!("{:.5}", cost_delta),
				tool_calls,
				tools_used,
				human_time: format_duration(human_time_ms),
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
		markdown.push_str("| User Request | Cost ($) | Tool Calls | Tools Used | Human Time | AI Time | Processing Time |\n");
		markdown.push_str("|--------------|----------|------------|------------|------------|---------|----------------|\n");

		// Table rows
		for entry in &self.entries {
			markdown.push_str(&format!(
				"| {} | {} | {} | {} | {} | {} | {} |\n",
				self.escape_markdown(&entry.user_request),
				entry.cost,
				entry.tool_calls,
				self.escape_markdown(&entry.tools_used),
				entry.human_time,
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
			format_duration(self.totals.total_human_time_ms),
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
		let mut markdown_report = String::new();

		// Header
		markdown_report.push_str("# 📊 Session Usage Report\n\n");

		// Table
		markdown_report.push_str(&self.generate_markdown_table());
		markdown_report.push('\n');

		// Summary
		markdown_report.push_str(&format!(
			"## 📈 Summary\n\n**{}** requests • **${:.5}** total cost • **{}** tool calls • **{}** human time • **{}** AI time • **{}** processing time\n",
			self.totals.total_requests,
			self.totals.total_cost,
			self.totals.total_tool_calls,
			format_duration(self.totals.total_human_time_ms),
			format_duration(self.totals.total_ai_time_ms),
			format_duration(self.totals.total_processing_time_ms)
		));

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

	/// Display report as plain text (fallback)
	fn display_plain(&self, markdown_report: &str) {
		// Convert markdown to plain text for fallback
		let plain_text = markdown_report
			.replace("# ", "")
			.replace("## ", "")
			.replace("**", "")
			.replace("*", "");
		println!("{}", plain_text);
	}
}
