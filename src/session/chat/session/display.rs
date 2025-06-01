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
use colored::*;

// Utility function to format time in a human-readable format
fn format_duration(milliseconds: u64) -> String {
	if milliseconds == 0 {
		return "0ms".to_string();
	}

	let ms = milliseconds % 1000;
	let seconds = (milliseconds / 1000) % 60;
	let minutes = (milliseconds / 60000) % 60;
	let hours = milliseconds / 3600000;

	let mut parts = Vec::new();

	if hours > 0 {
		parts.push(format!("{}h", hours));
	}
	if minutes > 0 {
		parts.push(format!("{}m", minutes));
	}
	if seconds > 0 {
		parts.push(format!("{}s", seconds));
	}
	if ms > 0 || parts.is_empty() {
		if parts.is_empty() {
			parts.push(format!("{}ms", ms));
		} else if ms >= 100 {
			// Only show milliseconds if >= 100ms when other units are present
			parts.push(format!("{}ms", ms));
		}
	}

	parts.join(" ")
}

impl ChatSession {
	// Display detailed information about the session, including layer-specific stats
	pub fn display_session_info(&self) {
		// Display overall session metrics
		println!("{}", "───────────── Session Information ─────────────".bright_cyan());

		// Session basics
		println!("{} {}", "Session name:".yellow(), self.session.info.name.bright_white());
		println!("{} {}", "Main model:".yellow(), self.session.info.model.bright_white());

		// Total token usage
		let total_tokens = self.session.info.input_tokens + self.session.info.output_tokens + self.session.info.cached_tokens;
		println!("{} {}", "Total tokens:".yellow(), total_tokens.to_string().bright_white());
		println!("{} {} input, {} output, {} cached",
			"Breakdown:".yellow(),
			self.session.info.input_tokens.to_string().bright_blue(),
			self.session.info.output_tokens.to_string().bright_green(),
			self.session.info.cached_tokens.to_string().bright_magenta());

		// Cost information
		println!("{} ${:.5}", "Total cost:".yellow(), self.session.info.total_cost);

		// Time information
		let total_time_ms = self.session.info.total_api_time_ms + self.session.info.total_tool_time_ms + self.session.info.total_layer_time_ms;
		if total_time_ms > 0 {
			println!("{} {} (API: {}, Tools: {}, Processing: {})",
				"Total time:".yellow(),
				format_duration(total_time_ms).bright_white(),
				format_duration(self.session.info.total_api_time_ms).bright_blue(),
				format_duration(self.session.info.total_tool_time_ms).bright_green(),
				format_duration(self.session.info.total_layer_time_ms).bright_magenta());
		}

		// Messages count and tool calls
		println!("{} {}", "Messages:".yellow(), self.session.messages.len());

		// Tool calls information
		if self.session.info.tool_calls > 0 {
			println!("{} {}", "Tool calls:".yellow(), self.session.info.tool_calls.to_string().bright_cyan());
		}

		// Display layered stats if available
		if !self.session.info.layer_stats.is_empty() {
			println!();
			println!("{}", "───────────── Layer-by-Layer Statistics ─────────────".bright_cyan());

			// Group by layer type
			let mut layer_stats: std::collections::HashMap<String, Vec<&crate::session::LayerStats>> = std::collections::HashMap::new();

			// Group stats by layer type
			for stat in &self.session.info.layer_stats {
				layer_stats.entry(stat.layer_type.clone())
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
				println!("  {}: {} input, {} output",
					"Tokens".blue(),
					total_input.to_string().bright_white(),
					total_output.to_string().bright_white());
				println!("  {}: ${:.5}", "Cost".blue(), total_cost);

				// Show time information if available
				let total_time = total_api_time + total_tool_time + total_layer_time;
				if total_time > 0 {
					println!("  {}: {} (API: {}, Tools: {}, Total: {})",
						"Time".blue(),
						format_duration(total_time).bright_white(),
						format_duration(total_api_time).bright_cyan(),
						format_duration(total_tool_time).bright_green(),
						format_duration(total_layer_time).bright_magenta());
				}

				// Add special note for context optimization
				if layer_type.as_str() == "context_optimization" {
					println!("  {}", "Note: These are costs for optimizing context between interactions".bright_cyan());
				}

				println!();
			}

			// Print command layers separately if any exist
			if !command_layers.is_empty() {
				println!("{}", "───────────── Command Layer Statistics ─────────────".bright_green());

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
					println!("  {}: {} input, {} output",
						"Tokens".blue(),
						total_input.to_string().bright_white(),
						total_output.to_string().bright_white());
					println!("  {}: ${:.5}", "Cost".blue(), total_cost);

					// Show time information if available
					let total_time = total_api_time + total_tool_time + total_layer_time;
					if total_time > 0 {
						println!("  {}: {} (API: {}, Tools: {}, Total: {})",
							"Time".blue(),
							format_duration(total_time).bright_white(),
							format_duration(total_api_time).bright_cyan(),
							format_duration(total_tool_time).bright_green(),
							format_duration(total_layer_time).bright_magenta());
					}

					println!("  {}", "Note: Command layers don't affect session history".bright_cyan());

					println!();
				}
			}
		} else {
			println!();
			println!("{}", "No layer-specific statistics available.".bright_yellow());
			println!("{}", "This may be because the session was created before layered architecture was enabled.".bright_yellow());
		}

		println!();
	}
}
