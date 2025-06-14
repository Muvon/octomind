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

// Session display functionality

use super::core::ChatSession;
use super::utils::format_number;
use crate::session::chat::formatting::format_duration;
use colored::*;

impl ChatSession {
	// Display detailed information about the session, including layer-specific stats
	pub fn display_session_info(&self) {
		// Display overall session metrics
		println!(
			"{}",
			"───────────── Session Information ─────────────".bright_cyan()
		);

		// Session basics
		println!(
			"{} {}",
			"Session name:".yellow(),
			self.session.info.name.bright_white()
		);
		println!(
			"{} {}",
			"Main model:".yellow(),
			self.session.info.model.bright_white()
		);

		// Total token usage
		let total_tokens = self.session.info.input_tokens
			+ self.session.info.output_tokens
			+ self.session.info.cached_tokens;
		println!(
			"{} {}",
			"Total tokens:".yellow(),
			format_number(total_tokens).bright_white()
		);
		println!(
			"{} {} input, {} output, {} cached",
			"Breakdown:".yellow(),
			format_number(self.session.info.input_tokens).bright_blue(),
			format_number(self.session.info.output_tokens).bright_green(),
			format_number(self.session.info.cached_tokens).bright_magenta()
		);

		// Cost information
		println!(
			"{} ${:.5}",
			"Total cost:".yellow(),
			self.session.info.total_cost
		);

		// Time information
		let total_time_ms = self.session.info.total_api_time_ms
			+ self.session.info.total_tool_time_ms
			+ self.session.info.total_layer_time_ms;
		if total_time_ms > 0 {
			println!(
				"{} {} (API: {}, Tools: {}, Processing: {})",
				"Total time:".yellow(),
				format_duration(total_time_ms).bright_white(),
				format_duration(self.session.info.total_api_time_ms).bright_blue(),
				format_duration(self.session.info.total_tool_time_ms).bright_green(),
				format_duration(self.session.info.total_layer_time_ms).bright_magenta()
			);
		}

		// Messages count and tool calls
		println!("{} {}", "Messages:".yellow(), self.session.messages.len());

		// Tool calls information
		if self.session.info.tool_calls > 0 {
			println!(
				"{} {}",
				"Tool calls:".yellow(),
				self.session.info.tool_calls.to_string().bright_cyan()
			);
		}

		// Display layered stats if available
		if !self.session.info.layer_stats.is_empty() {
			println!();
			println!(
				"{}",
				"───────────── Layer-by-Layer Statistics ─────────────".bright_cyan()
			);

			// Group by layer type
			let mut layer_stats: std::collections::HashMap<
				String,
				Vec<&crate::session::LayerStats>,
			> = std::collections::HashMap::new();

			// Group stats by layer type
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
				// Add special highlighting for context optimization
				let layer_display = if layer_type.as_str() == "context_optimization" {
					format!("Layer: {}", layer_type).bright_magenta()
				} else {
					format!("Layer: {}", layer_type).bright_yellow()
				};

				println!("{}", layer_display);

				// Count total tokens and cost for this layer type
				let mut total_input = 0;
				let mut total_output = 0;
				let mut total_cost = 0.0;
				let mut total_api_time = 0;
				let mut total_tool_time = 0;
				let mut total_layer_time = 0;

				// Count executions
				let executions = stats.len();

				for stat in stats.iter() {
					total_input += stat.input_tokens;
					total_output += stat.output_tokens;
					total_cost += stat.cost;
					total_api_time += stat.api_time_ms;
					total_tool_time += stat.tool_time_ms;
					total_layer_time += stat.total_time_ms;
				}

				// Print the stats
				println!("  {}: {}", "Model".blue(), stats[0].model);
				println!("  {}: {}", "Executions".blue(), executions);
				println!(
					"  {}: {} input, {} output",
					"Tokens".blue(),
					format_number(total_input).bright_white(),
					format_number(total_output).bright_white()
				);
				println!("  {}: ${:.5}", "Cost".blue(), total_cost);

				// Show time information if available
				let total_time = total_api_time + total_tool_time + total_layer_time;
				if total_time > 0 {
					println!(
						"  {}: {} (API: {}, Tools: {}, Total: {})",
						"Time".blue(),
						format_duration(total_time).bright_white(),
						format_duration(total_api_time).bright_cyan(),
						format_duration(total_tool_time).bright_green(),
						format_duration(total_layer_time).bright_magenta()
					);
				}

				// Add special note for context optimization
				if layer_type.as_str() == "context_optimization" {
					println!(
						"  {}",
						"Note: These are costs for optimizing context between interactions"
							.bright_cyan()
					);
				}

				println!();
			}

			// Print command layers separately if any exist
			if !command_layers.is_empty() {
				println!(
					"{}",
					"───────────── Command Layer Statistics ─────────────".bright_green()
				);

				for (layer_type, stats) in command_layers.iter() {
					// Extract command name from "command:name" format
					let command_name = layer_type.strip_prefix("command:").unwrap_or(layer_type);
					let layer_display = format!("Command: {}", command_name).bright_green();

					println!("{}", layer_display);

					// Count total tokens and cost for this command
					let mut total_input = 0;
					let mut total_output = 0;
					let mut total_cost = 0.0;
					let mut total_api_time = 0;
					let mut total_tool_time = 0;
					let mut total_layer_time = 0;

					// Count executions
					let executions = stats.len();

					for stat in stats.iter() {
						total_input += stat.input_tokens;
						total_output += stat.output_tokens;
						total_cost += stat.cost;
						total_api_time += stat.api_time_ms;
						total_tool_time += stat.tool_time_ms;
						total_layer_time += stat.total_time_ms;
					}

					// Print the stats
					println!("  {}: {}", "Model".blue(), stats[0].model);
					println!("  {}: {}", "Executions".blue(), executions);
					println!(
						"  {}: {} input, {} output",
						"Tokens".blue(),
						format_number(total_input).bright_white(),
						format_number(total_output).bright_white()
					);
					println!("  {}: ${:.5}", "Cost".blue(), total_cost);

					// Show time information if available
					let total_time = total_api_time + total_tool_time + total_layer_time;
					if total_time > 0 {
						println!(
							"  {}: {} (API: {}, Tools: {}, Total: {})",
							"Time".blue(),
							format_duration(total_time).bright_white(),
							format_duration(total_api_time).bright_cyan(),
							format_duration(total_tool_time).bright_green(),
							format_duration(total_layer_time).bright_magenta()
						);
					}

					println!(
						"  {}",
						"Note: Command layers don't affect session history".bright_cyan()
					);

					println!();
				}
			}
		} else {
			println!();
			println!(
				"{}",
				"No layer-specific statistics available.".bright_yellow()
			);
			println!("{}", "This may be because the session was created before layered architecture was enabled.".bright_yellow());
		}

		println!();
	}

	// Display current session context that would be sent to AI
	pub fn display_session_context(&self, config: &crate::config::Config) {
		// Use the filtered version with "all" filter for backward compatibility
		self.display_session_context_filtered(config, "all");
	}

	// Display current session context with filtering options
	pub fn display_session_context_filtered(&self, config: &crate::config::Config, filter: &str) {
		// Check if debug mode is enabled
		let is_debug = config.log_level.is_debug_enabled();

		// Display header with filter info
		println!(
			"{}",
			format!(
				"───────────── Session Context ({}) ─────────────",
				filter.to_uppercase()
			)
			.bright_cyan()
		);

		if self.session.messages.is_empty() {
			println!("{}", "No messages in current session.".yellow());
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
					let median = if token_counts.len() % 2 == 0 {
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
				println!(
					"{}",
					format!(
						"Unknown filter '{}'. Available filters: all, assistant, user, tool, large",
						filter
					)
					.bright_red()
				);
				println!(
					"{}",
					"Usage: /context [all|assistant|user|tool|large]".bright_yellow()
				);
				println!();
				return;
			}
		};

		if filtered_messages.is_empty() {
			println!(
				"{}",
				format!("No messages match the '{}' filter.", filter).yellow()
			);
			println!();
			return;
		}

		// Build markdown content for the filtered context
		let mut markdown_content = String::new();
		markdown_content.push_str("# Session Context\n\n");
		markdown_content.push_str(&format!("**Session:** {}\n", self.session.info.name));
		markdown_content.push_str(&format!("**Model:** {}\n", self.session.info.model));
		markdown_content.push_str(&format!(
			"**Messages:** {} total, {} shown (filter: {})\n\n",
			self.session.messages.len(),
			filtered_messages.len(),
			filter
		));

		// Content length limits
		let content_limit = if is_debug { None } else { Some(200) };

		// Calculate total session tokens for percentage calculation
		let total_session_tokens = self.session.info.input_tokens
			+ self.session.info.output_tokens
			+ self.session.info.cached_tokens;

		// Add median and std dev info for large filter
		if filter == "large" && !self.session.messages.is_empty() {
			let mut token_counts: Vec<f64> = self
				.session
				.messages
				.iter()
				.map(|msg| crate::session::token_counter::estimate_tokens(&msg.content) as f64)
				.collect();

			// Calculate median
			token_counts.sort_by(|a, b| a.partial_cmp(b).unwrap());
			let median = if token_counts.len() % 2 == 0 {
				(token_counts[token_counts.len() / 2 - 1] + token_counts[token_counts.len() / 2])
					/ 2.0
			} else {
				token_counts[token_counts.len() / 2]
			};

			// Calculate standard deviation
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

			markdown_content.push_str(&format!(
				"**Filter Info:** Showing messages > {:.0} tokens (median: {:.0}, std dev: {:.0}, threshold: median + 2σ)\\n\\n",
				threshold, median, std_dev
			));
		}

		// Process each filtered message
		for (original_index, message) in &filtered_messages {
			// Calculate tokens for this message
			let message_tokens = crate::session::token_counter::estimate_tokens(&message.content);
			let percentage = if total_session_tokens > 0 {
				(message_tokens as f64 / total_session_tokens as f64) * 100.0
			} else {
				0.0
			};

			markdown_content.push_str(&format!(
				"## Message {} - {}\n\n",
				*original_index + 1,
				message.role.to_uppercase()
			));

			// Add timestamp
			if let Some(datetime) = chrono::DateTime::from_timestamp(message.timestamp as i64, 0) {
				markdown_content.push_str(&format!(
					"**Time:** {}\n",
					datetime.format("%Y-%m-%d %H:%M:%S UTC")
				));
			}

			// Add token information
			markdown_content.push_str(&format!(
				"**Tokens:** {} ({:.2}%)\n",
				crate::session::chat::format_number(message_tokens as u64),
				percentage
			));

			// Add cached status
			if message.cached {
				markdown_content.push_str("**Cached:** ✅ Yes\n");
			}

			// Add tool call ID if present
			if let Some(ref tool_call_id) = message.tool_call_id {
				markdown_content.push_str(&format!("**Tool Call ID:** {}\n", tool_call_id));
			}

			// Add tool name if present
			if let Some(ref name) = message.name {
				markdown_content.push_str(&format!("**Tool:** {}\n", name));
			}

			// Add content
			let content = if let Some(limit) = content_limit {
				if message.content.len() > limit {
					format!("{}...\n\n*[Content truncated - {} total chars. Use debug mode (/loglevel debug) for full content]*",
						&message.content[..limit], message.content.len())
				} else {
					message.content.clone()
				}
			} else {
				message.content.clone()
			};

			markdown_content.push_str("**Content:**\n");
			markdown_content.push_str("```\n");
			markdown_content.push_str(&content);
			markdown_content.push_str("\n```\n");

			// Add tool calls if present
			if let Some(ref tool_calls) = message.tool_calls {
				markdown_content.push_str("**Tool Calls:**\n");
				markdown_content.push_str("```json\n");
				markdown_content.push_str(
					&serde_json::to_string_pretty(tool_calls)
						.unwrap_or_else(|_| "Invalid JSON".to_string()),
				);
				markdown_content.push_str("\n```\n");
			}

			// Add images if present
			if let Some(ref images) = message.images {
				markdown_content.push_str(&format!("**Images:** {} attachment(s)\n", images.len()));
				for (i, image) in images.iter().enumerate() {
					markdown_content.push_str(&format!(
						"  {}. Type: {}\n",
						i + 1,
						image.media_type
					));
				}
			}

			markdown_content.push_str("\n---\n\n");
		}

		// Add summary
		markdown_content.push_str("## Summary\n\n");
		markdown_content.push_str(&format!(
			"- **Total Messages:** {}\n",
			self.session.messages.len()
		));
		markdown_content.push_str(&format!(
			"- **Filtered Messages:** {}\n",
			filtered_messages.len()
		));
		markdown_content.push_str(&format!(
			"- **Total Tokens:** {}\n",
			crate::session::chat::format_number(
				self.session.info.input_tokens
					+ self.session.info.output_tokens
					+ self.session.info.cached_tokens
			)
		));
		markdown_content.push_str(&format!(
			"- **Total Cost:** ${:.5}\n",
			self.session.info.total_cost
		));

		if is_debug {
			markdown_content.push_str("\n*Debug mode: Showing full content*\n");
		} else {
			markdown_content.push_str(
				"\n*Compact mode: Content truncated. Use `/loglevel debug` to toggle full content*\n",
			);
		}

		// Render using existing markdown renderer
		crate::session::chat::assistant_output::print_assistant_response(
			&markdown_content,
			config,
			"assistant", // role parameter for markdown rendering
		);

		println!();
	}
}
