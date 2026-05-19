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

// Session display functionality

use super::core::ChatSession;
use super::utils::format_number;
use crate::session::chat::formatting::format_duration;
use crate::session::chat::tool_display::{
	block_close_err, block_close_ok, block_line, block_open, block_row, block_row_text,
	block_section, block_section_with, key_width,
};
use colored::*;

// Render aggregate stats for a slice of layer executions under the current
// `/info` block — emits model/executions/tokens/cost/time rows on the rail.
fn render_layer_stats(stats: &[&crate::session::LayerStats]) {
	if stats.is_empty() {
		return;
	}
	let kw = key_width(["model", "executions", "tokens", "cost", "time"]);
	let executions = stats.len();
	let (mut total_input, mut total_output, mut total_cost) = (0u64, 0u64, 0.0f64);
	let (mut total_api_time, mut total_tool_time, mut total_layer_time) = (0u64, 0u64, 0u64);
	for stat in stats {
		total_input += stat.input_tokens;
		total_output += stat.output_tokens;
		total_cost += stat.cost;
		total_api_time += stat.api_time_ms;
		total_tool_time += stat.tool_time_ms;
		total_layer_time += stat.total_time_ms;
	}
	let dot = "·".bright_black();
	block_row("model", &stats[0].model.bright_white().to_string(), kw);
	block_row("executions", &executions.to_string(), kw);
	block_row(
		"tokens",
		&format!(
			"{} in {} {} out",
			format_number(total_input).bright_white(),
			dot,
			format_number(total_output).bright_white(),
		),
		kw,
	);
	block_row("cost", &format!("${:.5}", total_cost), kw);
	let total_time = total_api_time + total_tool_time + total_layer_time;
	if total_time > 0 {
		block_row(
			"time",
			&format!(
				"{} {} api {} {} {} tools {} {} total",
				format_duration(total_time).bright_white(),
				dot,
				format_duration(total_api_time).bright_cyan(),
				dot,
				format_duration(total_tool_time).bright_green(),
				dot,
				format_duration(total_layer_time).bright_magenta(),
			),
			kw,
		);
	}
}

impl ChatSession {
	// Display detailed information about the session, including layer-specific stats.
	// Uses the unified block layout (╭/│/╰) for consistency with other command outputs.
	pub fn display_session_info(&self) {
		block_open("/info", None);

		// ── session ────────────────────────────────────────────────────
		block_section_with("session", &self.session.info.name);
		let kw_sess = key_width([
			"model",
			"tokens",
			"breakdown",
			"cost",
			"time",
			"messages",
			"tool calls",
		]);
		block_row(
			"model",
			&self.session.info.model.bright_white().to_string(),
			kw_sess,
		);
		let total_tokens = self.session.info.input_tokens
			+ self.session.info.output_tokens
			+ self.session.info.cache_read_tokens
			+ self.session.info.cache_write_tokens
			+ self.session.info.reasoning_tokens;
		block_row(
			"tokens",
			&format!("{} total", format_number(total_tokens).bright_white()),
			kw_sess,
		);
		let dot = "·".bright_black();
		block_row(
			"breakdown",
			&format!(
				"{} in {} {} out {} {} cache rd {} {} cache wr {} {} reasoning",
				format_number(self.session.info.input_tokens).bright_blue(),
				dot,
				format_number(self.session.info.output_tokens).bright_green(),
				dot,
				format_number(self.session.info.cache_read_tokens).bright_magenta(),
				dot,
				format_number(self.session.info.cache_write_tokens).bright_cyan(),
				dot,
				format_number(self.session.info.reasoning_tokens).white(),
			),
			kw_sess,
		);
		block_row(
			"cost",
			&format!("${:.5}", self.session.info.total_cost),
			kw_sess,
		);

		let total_time_ms = self.session.info.total_api_time_ms
			+ self.session.info.total_tool_time_ms
			+ self.session.info.total_layer_time_ms;
		if total_time_ms > 0 {
			block_row(
				"time",
				&format!(
					"{} {} api {} {} {} tools {} {} processing",
					format_duration(total_time_ms).bright_white(),
					dot,
					format_duration(self.session.info.total_api_time_ms).bright_blue(),
					dot,
					format_duration(self.session.info.total_tool_time_ms).bright_green(),
					dot,
					format_duration(self.session.info.total_layer_time_ms).bright_magenta(),
				),
				kw_sess,
			);
		}
		block_row(
			"messages",
			&self.session.messages.len().to_string(),
			kw_sess,
		);
		if self.session.info.tool_calls > 0 {
			block_row(
				"tool calls",
				&self
					.session
					.info
					.tool_calls
					.to_string()
					.bright_cyan()
					.to_string(),
				kw_sess,
			);
		}

		// ── compression ────────────────────────────────────────────────
		let cs = &self.session.info.compression_stats;
		if cs.total_compressions() > 0 {
			block_section("compression");
			let kw = key_width([
				"task",
				"phase",
				"project",
				"conversation",
				"messages removed",
				"tokens saved",
				"avg ratio",
			]);
			if cs.task_compressions > 0 {
				block_row(
					"task",
					&format_number(cs.task_compressions as u64)
						.bright_white()
						.to_string(),
					kw,
				);
			}
			if cs.phase_compressions > 0 {
				block_row(
					"phase",
					&format_number(cs.phase_compressions as u64)
						.bright_white()
						.to_string(),
					kw,
				);
			}
			if cs.project_compressions > 0 {
				block_row(
					"project",
					&format_number(cs.project_compressions as u64)
						.bright_white()
						.to_string(),
					kw,
				);
			}
			if cs.conversation_compressions > 0 {
				block_row(
					"conversation",
					&format_number(cs.conversation_compressions as u64)
						.bright_white()
						.to_string(),
					kw,
				);
			}
			block_row(
				"messages removed",
				&format_number(cs.total_messages_removed as u64)
					.bright_green()
					.to_string(),
				kw,
			);
			block_row(
				"tokens saved",
				&format_number(cs.total_tokens_saved)
					.bright_green()
					.to_string(),
				kw,
			);
			let avg_ratio = cs.avg_compression_ratio() * 100.0;
			if avg_ratio > 0.0 {
				block_row("avg ratio", &format!("{:.1}%", avg_ratio), kw);
			}
		}

		// ── layers ─────────────────────────────────────────────────────
		if !self.session.info.layer_stats.is_empty() {
			let mut layer_stats: std::collections::HashMap<
				String,
				Vec<&crate::session::LayerStats>,
			> = std::collections::HashMap::new();
			for stat in &self.session.info.layer_stats {
				layer_stats
					.entry(stat.layer_type.clone())
					.or_default()
					.push(stat);
			}
			let mut command_layers = Vec::new();
			let mut regular_layers = Vec::new();
			for (layer_type, stats) in layer_stats.iter() {
				if layer_type.starts_with("command:") {
					command_layers.push((layer_type, stats));
				} else {
					regular_layers.push((layer_type, stats));
				}
			}

			for (layer_type, stats) in regular_layers.iter() {
				block_section_with("layer", layer_type.as_str());
				render_layer_stats(stats.as_slice());
				if layer_type.as_str() == "context_optimization" {
					block_line(
						&"Note: costs for optimizing context between interactions"
							.bright_cyan()
							.to_string(),
					);
				}
			}
			for (layer_type, stats) in command_layers.iter() {
				let name = layer_type
					.strip_prefix("command:")
					.unwrap_or(layer_type.as_str());
				block_section_with("command", name);
				render_layer_stats(stats.as_slice());
				block_line(
					&"Note: command layers don't affect session history"
						.bright_cyan()
						.to_string(),
				);
			}
		} else {
			block_line(
				&"No layer-specific statistics available."
					.bright_yellow()
					.to_string(),
			);
		}

		block_close_ok("/info", Some(&self.session.info.name));
		println!();
	}

	// Get session information as structured JSON (for WebSocket/API use)
	pub fn get_session_info_json(&self) -> serde_json::Value {
		// Group layer stats
		let mut layer_stats_map: std::collections::HashMap<
			String,
			Vec<&crate::session::LayerStats>,
		> = std::collections::HashMap::new();

		for stat in &self.session.info.layer_stats {
			layer_stats_map
				.entry(stat.layer_type.clone())
				.or_default()
				.push(stat);
		}

		// Separate command layers from regular layers
		let mut command_layers = Vec::new();
		let mut regular_layers = Vec::new();

		for (layer_type, stats) in layer_stats_map.iter() {
			let mut total_input = 0;
			let mut total_output = 0;
			let mut total_cost = 0.0;
			let mut total_api_time = 0;
			let mut total_tool_time = 0;
			let mut total_layer_time = 0;
			let executions = stats.len();

			for stat in stats.iter() {
				total_input += stat.input_tokens;
				total_output += stat.output_tokens;
				total_cost += stat.cost;
				total_api_time += stat.api_time_ms;
				total_tool_time += stat.tool_time_ms;
				total_layer_time += stat.total_time_ms;
			}

			let layer_data = serde_json::json!({
				"layer_type": layer_type,
				"model": stats[0].model,
				"executions": executions,
				"tokens": {
					"input": total_input,
					"output": total_output
				},
				"cost": total_cost,
				"time": {
					"api_ms": total_api_time,
					"tool_ms": total_tool_time,
					"total_ms": total_layer_time
				}
			});

			if layer_type.starts_with("command:") {
				command_layers.push(layer_data);
			} else {
				regular_layers.push(layer_data);
			}
		}

		let total_tokens = self.session.info.input_tokens
			+ self.session.info.output_tokens
			+ self.session.info.cache_read_tokens
			+ self.session.info.cache_write_tokens
			+ self.session.info.reasoning_tokens;

		let total_time_ms = self.session.info.total_api_time_ms
			+ self.session.info.total_tool_time_ms
			+ self.session.info.total_layer_time_ms;

		serde_json::json!({
			"session_name": self.session.info.name,
			"model": self.session.info.model,
			"tokens": {
				"total": total_tokens,
				"input": self.session.info.input_tokens,
				"output": self.session.info.output_tokens,
				"cache_read": self.session.info.cache_read_tokens,
				"cache_write": self.session.info.cache_write_tokens,
				"reasoning": self.session.info.reasoning_tokens
			},
			"cost": self.session.info.total_cost,
			"time": {
				"total_ms": total_time_ms,
				"api_ms": self.session.info.total_api_time_ms,
				"tool_ms": self.session.info.total_tool_time_ms,
				"processing_ms": self.session.info.total_layer_time_ms
			},
			"messages": self.session.messages.len(),
			"tool_calls": self.session.info.tool_calls,
			"layers": {
				"regular": regular_layers,
				"commands": command_layers
			}
		})
	}

	// Get session information as a string (for WebSocket/API use)
	pub fn get_session_info_string(&self) -> String {
		let mut output = String::new();

		// Session basics
		output.push_str(&format!("Session name: {}\n", self.session.info.name));
		output.push_str(&format!("Main model: {}\n", self.session.info.model));

		let total_tokens = self.session.info.input_tokens
			+ self.session.info.output_tokens
			+ self.session.info.cache_read_tokens
			+ self.session.info.cache_write_tokens;
		output.push_str(&format!("Total tokens: {}\n", format_number(total_tokens)));
		output.push_str(&format!(
			"Breakdown: {} input, {} output, {} cache read, {} cache write\n",
			format_number(self.session.info.input_tokens),
			format_number(self.session.info.output_tokens),
			format_number(self.session.info.cache_read_tokens),
			format_number(self.session.info.cache_write_tokens)
		));

		// Cost information
		output.push_str(&format!(
			"Total cost: ${:.5}\n",
			self.session.info.total_cost
		));

		// Time information
		let total_time_ms = self.session.info.total_api_time_ms
			+ self.session.info.total_tool_time_ms
			+ self.session.info.total_layer_time_ms;
		if total_time_ms > 0 {
			output.push_str(&format!(
				"Total time: {} (API: {}, Tools: {}, Processing: {})\n",
				format_duration(total_time_ms),
				format_duration(self.session.info.total_api_time_ms),
				format_duration(self.session.info.total_tool_time_ms),
				format_duration(self.session.info.total_layer_time_ms)
			));
		}

		// Messages count and tool calls
		output.push_str(&format!("Messages: {}\n", self.session.messages.len()));

		if self.session.info.tool_calls > 0 {
			output.push_str(&format!("Tool calls: {}\n", self.session.info.tool_calls));
		}

		// Layer statistics if available
		if !self.session.info.layer_stats.is_empty() {
			output.push_str("\n─── Layer Statistics ───\n");

			// Group by layer type
			let mut layer_stats: std::collections::HashMap<
				String,
				Vec<&crate::session::LayerStats>,
			> = std::collections::HashMap::new();

			for stat in &self.session.info.layer_stats {
				layer_stats
					.entry(stat.layer_type.clone())
					.or_default()
					.push(stat);
			}

			// Separate command layers from regular layers
			let mut command_layers = Vec::new();
			let mut regular_layers = Vec::new();

			for (layer_type, stats) in layer_stats.iter() {
				if layer_type.starts_with("command:") {
					command_layers.push((layer_type, stats));
				} else {
					regular_layers.push((layer_type, stats));
				}
			}

			// Print regular layers first
			for (layer_type, stats) in regular_layers.iter() {
				output.push_str(&format!("\nLayer: {}\n", layer_type));

				let mut total_input = 0;
				let mut total_output = 0;
				let mut total_cost = 0.0;
				let executions = stats.len();

				for stat in stats.iter() {
					total_input += stat.input_tokens;
					total_output += stat.output_tokens;
					total_cost += stat.cost;
				}

				output.push_str(&format!("  Model: {}\n", stats[0].model));
				output.push_str(&format!("  Executions: {}\n", executions));
				output.push_str(&format!(
					"  Tokens: {} input, {} output\n",
					format_number(total_input),
					format_number(total_output)
				));
				output.push_str(&format!("  Cost: ${:.5}\n", total_cost));
			}

			// Print command layers if any
			if !command_layers.is_empty() {
				output.push_str("\n─── Command Layers ───\n");
				for (layer_type, stats) in command_layers.iter() {
					let command_name = layer_type.strip_prefix("command:").unwrap_or(layer_type);
					output.push_str(&format!("\nCommand: {}\n", command_name));

					let mut total_input = 0;
					let mut total_output = 0;
					let mut total_cost = 0.0;
					let executions = stats.len();

					for stat in stats.iter() {
						total_input += stat.input_tokens;
						total_output += stat.output_tokens;
						total_cost += stat.cost;
					}

					output.push_str(&format!("  Model: {}\n", stats[0].model));
					output.push_str(&format!("  Executions: {}\n", executions));
					output.push_str(&format!(
						"  Tokens: {} input, {} output\n",
						format_number(total_input),
						format_number(total_output)
					));
					output.push_str(&format!("  Cost: ${:.5}\n", total_cost));
				}
			}
		}

		output
	}

	// Display current session context that would be sent to AI
	pub async fn display_session_context(&mut self, config: &crate::config::Config) {
		// Use the filtered version with "all" filter for backward compatibility
		self.display_session_context_filtered(config, "all").await;
	}

	// Display current session context with filtering options
	pub async fn display_session_context_filtered(
		&mut self,
		config: &crate::config::Config,
		filter: &str,
	) {
		// Check if debug mode is enabled
		let is_debug = config.log_level.is_debug_enabled();

		// Open /context block — the markdown body prints between the corners
		// without a rail prefix (rail conflicts with code blocks / lists).
		block_open("/context", Some(&filter.to_uppercase()));

		if self.session.messages.is_empty() {
			block_line(&"No messages in current session.".yellow().to_string());
			block_close_ok("/context", Some("empty"));
			println!();
			return;
		}

		// Filter messages based on the filter parameter
		let filtered_messages: Vec<(usize, &crate::session::Message)> = match filter {
			"all" => self.session.messages.iter().enumerate().collect(),
			"assistant" => self
				.session
				.messages
				.iter()
				.enumerate()
				.filter(|(_, msg)| msg.role == "assistant")
				.collect(),
			"user" => self
				.session
				.messages
				.iter()
				.enumerate()
				.filter(|(_, msg)| msg.role == "user")
				.collect(),
			"tool" => self
				.session
				.messages
				.iter()
				.enumerate()
				.filter(|(_, msg)| {
					msg.role == "tool" || msg.tool_calls.is_some() || msg.tool_call_id.is_some()
				})
				.collect(),
			"large" => {
				// Calculate median and standard deviation for robust outlier detection
				let mut token_counts: Vec<f64> = self
					.session
					.messages
					.iter()
					.map(|msg| crate::session::token_counter::estimate_tokens(&msg.content) as f64)
					.collect();

				if token_counts.is_empty() {
					Vec::new()
				} else {
					// Calculate median
					token_counts.sort_by(|a, b| a.partial_cmp(b).unwrap());
					let median = if token_counts.len().is_multiple_of(2) {
						(token_counts[token_counts.len() / 2 - 1]
							+ token_counts[token_counts.len() / 2])
							/ 2.0
					} else {
						token_counts[token_counts.len() / 2]
					};

					// Calculate standard deviation
					let variance: f64 = self
						.session
						.messages
						.iter()
						.map(|msg| {
							let tokens =
								crate::session::token_counter::estimate_tokens(&msg.content) as f64;
							(tokens - median).powi(2)
						})
						.sum::<f64>() / self.session.messages.len() as f64;
					let std_dev = variance.sqrt();

					// Filter messages > 2 standard deviations from median
					let threshold = median + (2.0 * std_dev);
					self.session
						.messages
						.iter()
						.enumerate()
						.filter(|(_, msg)| {
							let msg_tokens =
								crate::session::token_counter::estimate_tokens(&msg.content) as f64;
							msg_tokens > threshold
						})
						.collect()
				}
			}
			_ => {
				block_line(
					&"Available filters: all, assistant, user, tool, large"
						.dimmed()
						.to_string(),
				);
				block_close_err("/context", &format!("unknown filter '{}'", filter));
				println!();
				return;
			}
		};

		if filtered_messages.is_empty() {
			block_line(
				&format!("No messages match the '{}' filter.", filter)
					.yellow()
					.to_string(),
			);
			block_close_ok("/context", Some("empty"));
			println!();
			return;
		}

		// Header section — session + model + filter summary, all on the rail.
		block_section_with("session", &self.session.info.name);
		let head_kw = key_width(["model", "messages"]);
		block_row(
			"model",
			&self.session.info.model.bright_white().to_string(),
			head_kw,
		);
		block_row(
			"messages",
			&format!(
				"{} total, {} shown (filter: {})",
				self.session.messages.len(),
				filtered_messages.len(),
				filter
			),
			head_kw,
		);

		// Content length limits
		let content_limit = if is_debug { None } else { Some(200) };
		// Calculate total session tokens for percentage calculation
		let total_session_tokens = self.session.info.input_tokens
			+ self.session.info.output_tokens
			+ self.session.info.cache_read_tokens
			+ self.session.info.cache_write_tokens;
		// "large" filter — show median / stddev / threshold stats as a section.
		if filter == "large" && !self.session.messages.is_empty() {
			let mut token_counts: Vec<f64> = self
				.session
				.messages
				.iter()
				.map(|msg| crate::session::token_counter::estimate_tokens(&msg.content) as f64)
				.collect();
			token_counts.sort_by(|a, b| a.partial_cmp(b).unwrap());
			let median = if token_counts.len().is_multiple_of(2) {
				(token_counts[token_counts.len() / 2 - 1] + token_counts[token_counts.len() / 2])
					/ 2.0
			} else {
				token_counts[token_counts.len() / 2]
			};
			let variance: f64 =
				self.session
					.messages
					.iter()
					.map(|msg| {
						let tokens =
							crate::session::token_counter::estimate_tokens(&msg.content) as f64;
						(tokens - median).powi(2)
					})
					.sum::<f64>() / self.session.messages.len() as f64;
			let std_dev = variance.sqrt();
			let threshold = median + (2.0 * std_dev);

			block_section("filter (large)");
			let kw = key_width(["median", "std dev", "threshold"]);
			block_row("median", &format!("{:.0}", median), kw);
			block_row("std dev", &format!("{:.0}", std_dev), kw);
			block_row(
				"threshold",
				&format!("{:.0} tokens (median + 2σ)", threshold),
				kw,
			);
		}

		// Per-message sections — each as `│ #N · ROLE` + indented rows / content.
		for (original_index, message) in &filtered_messages {
			let message_tokens = crate::session::token_counter::estimate_tokens(&message.content);
			let percentage = if total_session_tokens > 0 {
				(message_tokens as f64 / total_session_tokens as f64) * 100.0
			} else {
				0.0
			};

			let role_label = message.role.to_uppercase();
			let section_title = format!("#{}", *original_index + 1);
			block_section_with(&section_title, &role_label);

			let kw = key_width([
				"time",
				"tokens",
				"cached",
				"tool call id",
				"tool",
				"images",
				"content",
				"tool calls",
			]);

			if let Some(datetime) = chrono::DateTime::from_timestamp(message.timestamp as i64, 0) {
				block_row(
					"time",
					&datetime
						.format("%Y-%m-%d %H:%M:%S UTC")
						.to_string()
						.dimmed()
						.to_string(),
					kw,
				);
			}

			block_row(
				"tokens",
				&format!(
					"{} ({:.2}%)",
					crate::session::chat::format_number(message_tokens as u64).bright_white(),
					percentage
				),
				kw,
			);

			if message.cached {
				block_row("cached", &"yes".bright_green().to_string(), kw);
			}

			if let Some(ref tool_call_id) = message.tool_call_id {
				block_row("tool call id", &tool_call_id.dimmed().to_string(), kw);
			}

			if let Some(ref name) = message.name {
				block_row("tool", &name.bright_cyan().to_string(), kw);
			}

			if let Some(ref images) = message.images {
				block_row("images", &format!("{} attachment(s)", images.len()), kw);
				for (i, image) in images.iter().enumerate() {
					block_row_text(
						&format!("  {}. type: {}", i + 1, image.media_type)
							.dimmed()
							.to_string(),
					);
				}
			}

			// Content body — truncated unless debug mode.
			let (content, truncated_msg) = if let Some(limit) = content_limit {
				if message.content.chars().count() > limit {
					let truncated: String = message.content.chars().take(limit).collect();
					(
						format!("{}…", truncated),
						Some(format!(
							"truncated — {} total chars (use /loglevel debug for full)",
							message.content.chars().count()
						)),
					)
				} else {
					(message.content.clone(), None)
				}
			} else {
				(message.content.clone(), None)
			};
			block_row("content", "", kw);
			for line in content.lines() {
				block_row_text(line);
			}
			if let Some(msg) = truncated_msg {
				block_row_text(&msg.dimmed().to_string());
			}

			if let Some(ref tool_calls) = message.tool_calls {
				block_row("tool calls", "", kw);
				let json = serde_json::to_string_pretty(tool_calls)
					.unwrap_or_else(|_| "Invalid JSON".to_string());
				for line in json.lines() {
					block_row_text(line);
				}
			}
		}

		// Snapshot counts BEFORE the mutable borrow below — `filtered_messages` aliases
		// `self.session.messages`, and `get_full_context_tokens` takes `&mut self`.
		// NLL ends the borrow once `filtered_messages` is no longer used after this point.
		let filtered_count = filtered_messages.len();
		let total_count = self.session.messages.len();

		// Calculate current session context tokens using UNIFIED calculation
		// This ensures consistency with compression, continuation, and all other systems
		let current_context_tokens = self.get_full_context_tokens(config).await;
		let total_tokens_sum = self.session.info.input_tokens
			+ self.session.info.output_tokens
			+ self.session.info.cache_read_tokens
			+ self.session.info.cache_write_tokens;
		let total_cost = self.session.info.total_cost;

		// Summary section — totals + mode footer, on the rail.
		block_section("summary");
		let kw = key_width([
			"total messages",
			"filtered",
			"context tokens",
			"total tokens",
			"total cost",
			"mode",
		]);
		block_row("total messages", &total_count.to_string(), kw);
		block_row("filtered", &filtered_count.to_string(), kw);
		block_row(
			"context tokens",
			&format!(
				"{} (system prompt + tools)",
				crate::session::chat::format_number(current_context_tokens as u64).bright_white()
			),
			kw,
		);
		block_row(
			"total tokens",
			&crate::session::chat::format_number(total_tokens_sum)
				.bright_white()
				.to_string(),
			kw,
		);
		block_row(
			"total cost",
			&format!("${:.5}", total_cost).bright_yellow().to_string(),
			kw,
		);
		block_row(
			"mode",
			&if is_debug {
				"debug — full content".bright_green().to_string()
			} else {
				"compact — content truncated (/loglevel debug for full)"
					.dimmed()
					.to_string()
			},
			kw,
		);

		block_close_ok(
			"/context",
			Some(&format!("{} of {} message(s)", filtered_count, total_count)),
		);
		println!();
	}
}
