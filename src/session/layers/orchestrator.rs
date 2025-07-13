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

use super::layer_trait::Layer;
use super::types::GenericLayer;
use crate::config::Config;
use crate::session::Session;
use anyhow::Result;
use colored::*;
use std::io::IsTerminal;

// Main layered orchestrator that manages the pipeline of layers
pub struct LayeredOrchestrator {
	pub layers: Vec<Box<dyn Layer + Send + Sync>>,
}

impl LayeredOrchestrator {
	// Create orchestrator from config using the new flexible system
	pub fn from_config(config: &Config, role: &str) -> Self {
		// Get role-specific configuration
		let (role_config, _, _, _, _) = config.get_role_config(role);

		// First check if layers are enabled at all
		if !role_config.enable_layers {
			// Return empty orchestrator when layers are disabled
			return Self { layers: Vec::new() };
		}

		// Get enabled layers for this role using the new system
		let enabled_layers = config.get_enabled_layers_for_role(role);

		// Create layers from configuration
		let mut layers: Vec<Box<dyn Layer + Send + Sync>> = Vec::new();

		// Create layers from enabled layer configs
		for layer_config in enabled_layers {
			layers.push(Box::new(GenericLayer::new(layer_config)));
		}

		// STRICT CONFIG: If no layers enabled for this role and layers are enabled, that's an error
		if layers.is_empty() && role_config.enable_layers {
			panic!("CRITICAL CONFIG ERROR: Role '{}' has enable_layers=true but no layers are configured or enabled. Define layers in config or set enable_layers=false.", role);
		}

		Self { layers }
	}

	// Create orchestrator from config and process system prompts (async version for session initialization)
	pub async fn from_config_with_processed_prompts(
		config: &Config,
		role: &str,
		project_dir: &std::path::Path,
	) -> Self {
		// Get role-specific configuration
		let (role_config, _, _, _, _) = config.get_role_config(role);

		// First check if layers are enabled at all
		if !role_config.enable_layers {
			// Return empty orchestrator when layers are disabled
			return Self { layers: Vec::new() };
		}

		// Get enabled layers for this role using the new system
		let enabled_layers = config.get_enabled_layers_for_role(role);

		// Create layers from configuration and process their system prompts
		let mut layers: Vec<Box<dyn Layer + Send + Sync>> = Vec::new();

		// Create layers from enabled layer configs
		for mut layer_config in enabled_layers {
			// Process and cache the system prompt for this layer using centralized method
			crate::session::helper_functions::process_layer_system_prompt(
				&mut layer_config,
				project_dir,
			)
			.await;
			layers.push(Box::new(GenericLayer::new(layer_config)));
		}

		// STRICT CONFIG: If no layers enabled for this role and layers are enabled, that's an error
		if layers.is_empty() && role_config.enable_layers {
			panic!("CRITICAL CONFIG ERROR: Role '{}' has enable_layers=true but no layers are configured or enabled. Define layers in config or set enable_layers=false.", role);
		}

		Self { layers }
	}

	// Process user input through the layer architecture
	pub async fn process(
		&self,
		input: &str,
		session: &mut Session,
		config: &Config,
		operation_cancelled: tokio::sync::watch::Receiver<bool>,
	) -> Result<String> {
		// If no layers are configured (layers disabled), return input unchanged
		if self.layers.is_empty() {
			return Ok(input.to_string());
		}

		let mut current_input = input.to_string();

		// For total token/cost tracking across all layers
		let mut total_input_tokens = 0;
		let mut total_output_tokens = 0;
		let mut total_cost = 0.0;

		// Debug information for user - minimize in interactive mode to avoid interfering with animation
		if std::io::stdin().is_terminal() {
			// In interactive mode, the animation will show the main progress
			// Only show pipeline info in debug mode
			if config.get_log_level().is_debug_enabled() {
				println!(
					"{}",
					format!("Layer Processing Pipeline ({} layers)", self.layers.len())
						.bright_cyan()
				);
			}
		} else {
			// Non-interactive mode - always show pipeline info
			println!(
				"{}",
				format!("Layer Processing Pipeline ({} layers)", self.layers.len()).bright_cyan()
			);
		}

		// Process through each layer sequentially
		// Each layer operates in its own isolated session and handles its own function calls
		for layer in &self.layers {
			// Skip if operation cancelled
			if *operation_cancelled.borrow() {
				return Err(anyhow::anyhow!("Operation cancelled"));
			}

			let layer_name = layer.name();

			// Show layer progress with cost - single clean line that doesn't interfere with animation
			if std::io::stdin().is_terminal() {
				// In interactive mode, show minimal progress - animation handles the main "Generating response" display
				print!("\r{} {} ", "→".bright_yellow(), layer_name.bright_white());
				use std::io::Write;
				std::io::stdout().flush().ok();
			} else {
				// Non-interactive mode - show full progress
				println!("Processing: {} (${:.5})", layer_name, total_cost);
			}

			if !layer.config().mcp.server_refs.is_empty() {
				if std::io::stdin().is_terminal() {
					// In interactive mode, minimize tool info to avoid interfering with animation
					// Only show if debug mode is enabled
					if config.get_log_level().is_debug_enabled() {
						if layer.config().mcp.allowed_tools.is_empty() {
							println!("{}", "All tools enabled for this layer".bright_magenta());
						} else {
							println!(
								"{} {}",
								"Tools enabled:".bright_magenta(),
								layer.config().mcp.allowed_tools.join(", ")
							);
						}
					}
				} else {
					// Non-interactive mode - show full tool info
					if layer.config().mcp.allowed_tools.is_empty() {
						println!("{}", "All tools enabled for this layer".bright_magenta());
					} else {
						println!(
							"{} {}",
							"Tools enabled:".bright_magenta(),
							layer.config().mcp.allowed_tools.join(", ")
						);
					}
				}
			}

			// Process this layer with its own isolated session
			// The only input it receives is the output from the previous layer
			let result = layer
				.process(&current_input, session, config, operation_cancelled.clone())
				.await?;

			// Check for cancellation after layer processing
			if *operation_cancelled.borrow() {
				return Err(anyhow::anyhow!("Operation cancelled"));
			}

			// Display layer outputs with improved formatting
			self.display_layer_outputs(&result.outputs, layer_name);

			// Track token usage stats
			if let Some(usage) = &result.token_usage {
				// Try to get cost from the TokenUsage struct first
				if let Some(cost) = usage.cost {
					// Display compact layer cost and time info
					println!(
						"{}",
						format!(
							"${:.5} | {} tokens | {}ms",
							cost,
							usage.prompt_tokens + usage.output_tokens,
							result.total_time_ms
						)
						.bright_magenta()
					);

					// Ensure output is flushed in non-interactive mode
					use std::io::Write;
					std::io::stdout().flush().ok();

					// Add the stats to the session with time tracking
					session.add_layer_stats_with_time(
						layer_name,
						&layer.config().get_effective_model(&session.info.model),
						usage.prompt_tokens,
						usage.output_tokens,
						cost,
						result.api_time_ms,
						result.tool_time_ms,
						result.total_time_ms,
					);

					// Update totals for summary
					total_input_tokens += usage.prompt_tokens;
					total_output_tokens += usage.output_tokens;
					total_cost += cost;
				} else {
					// Try to get cost from raw response JSON if not in TokenUsage
					let cost_from_raw = result
						.exchange
						.response
						.get("usage")
						.and_then(|u| u.get("cost"))
						.and_then(|c| c.as_f64());

					if let Some(cost) = cost_from_raw {
						// Log that we had to get cost from raw response
						println!("{}", format!("Layer cost (from raw): ${:.5} (Input: {} tokens, Output: {} tokens) | Time: API {}ms, Tools {}ms, Total {}ms",
							cost, usage.prompt_tokens, usage.output_tokens,
							result.api_time_ms, result.tool_time_ms, result.total_time_ms).bright_magenta());

						// Add the stats to the session with time tracking
						session.add_layer_stats_with_time(
							layer_name,
							&layer.config().get_effective_model(&session.info.model),
							usage.prompt_tokens,
							usage.output_tokens,
							cost,
							result.api_time_ms,
							result.tool_time_ms,
							result.total_time_ms,
						);

						// Update totals for summary
						total_input_tokens += usage.prompt_tokens;
						total_output_tokens += usage.output_tokens;
						total_cost += cost;
					} else {
						// ERROR - OpenRouter did not provide cost data
						println!(
							"{} {}",
							"ERROR: Layer".bright_red(),
							layer_name.bright_yellow()
						);
						println!("{}", "OpenRouter did not provide cost data. Make sure usage.include=true is set!".bright_red());

						// Still track tokens and time
						total_input_tokens += usage.prompt_tokens;
						total_output_tokens += usage.output_tokens;

						// Add the stats to the session with time tracking but without cost
						session.add_layer_stats_with_time(
							layer_name,
							&layer.config().get_effective_model(&session.info.model),
							usage.prompt_tokens,
							usage.output_tokens,
							0.0, // No cost available
							result.api_time_ms,
							result.tool_time_ms,
							result.total_time_ms,
						);
					}
				}
			} else {
				println!(
					"{} {} | Time: API {}ms, Tools {}ms, Total {}ms",
					"ERROR: No usage data for layer".bright_red(),
					layer_name.bright_yellow(),
					result.api_time_ms,
					result.tool_time_ms,
					result.total_time_ms
				);
			}

			// Handle output_mode to determine how this layer's output affects the session
			use crate::session::layers::OutputMode;
			match layer.config().output_mode {
				OutputMode::None => {
					// Intermediate layer - just pass output to next layer, don't modify session
					println!("{}", "Output mode: none (intermediate layer)".bright_cyan());
				}
				OutputMode::Append => {
					// Add all layer outputs as assistant messages to session
					println!(
						"{}",
						"Output mode: append (adding all layer outputs)".bright_cyan()
					);
					// Add each output as a message with the configured role
					for output_text in &result.outputs {
						session.add_message(layer.config().output_role.as_str(), output_text);
					}
				}
				OutputMode::Replace => {
					// Replace entire session with all layer outputs
					println!(
						"{}",
						"Output mode: replace (replacing with all layer outputs)".bright_cyan()
					);
					// Clear existing messages and add all layer outputs with configured role
					session.messages.clear();
					for output_text in &result.outputs {
						session.add_message(layer.config().output_role.as_str(), output_text);
					}
				}
				OutputMode::Last => {
					println!(
						"{}",
						"Output mode: last (adding last layer output)".bright_cyan()
					);
					// Add last message with configured role
					let last_message = result.outputs.last().unwrap_or(&String::new()).clone();
					session.add_message(layer.config().output_role.as_str(), &last_message);
				}
				OutputMode::Restart => {
					println!(
						"{}",
						"Output mode: last (replacing with last layer output)".bright_cyan()
					);
					// Clear existing messages and add last message with configured role
					session.messages.clear();
					let last_message = result.outputs.last().unwrap_or(&String::new()).clone();
					session.add_message(layer.config().output_role.as_str(), &last_message);
				}
			};

			// Take the LAST output from this layer and use it as input for the next layer
			current_input = result.outputs.last().unwrap_or(&String::new()).clone();
		}

		// Display compact completion summary
		let total_api_time_ms = session.info.total_api_time_ms;
		let total_tool_time_ms = session.info.total_tool_time_ms;
		let total_layer_time_ms = session.info.total_layer_time_ms;

		// Clear any remaining progress line and show completion summary
		if std::io::stdin().is_terminal() {
			print!("\r                                                  \r");
		}
		println!(
			"\n{} | {} tokens | ${:.5} | {}ms",
			"Processing completed".bright_green(),
			total_input_tokens + total_output_tokens,
			total_cost,
			total_api_time_ms + total_tool_time_ms + total_layer_time_ms
		);

		// Return the final layer's output to be used as starting point for the main chat session
		// This output contains all the necessary context and information from the layer processing
		// When integrated into the main session via layered_response.rs, it becomes the foundation
		// for the entire conversation context, ensuring all the work done by the layers is preserved
		// and available for subsequent messages in the main chat flow.
		Ok(current_input)
	}

	/// Display layer outputs with improved formatting to match main loop tool rendering style
	fn display_layer_outputs(&self, outputs: &[String], layer_name: &str) {
		for (i, output) in outputs.iter().enumerate() {
			// For multiple outputs, show which output this is
			if outputs.len() > 1 {
				println!("--- Output {} ---", i + 1);
			}

			// Check if this output contains assistant response text that should be formatted
			if !output.trim().is_empty() {
				// Display the output with assistant response formatting
				self.display_formatted_assistant_output(output, layer_name, i + 1);
			}
		}
	}

	/// Display assistant output with formatting similar to main loop style
	fn display_formatted_assistant_output(
		&self,
		output: &str,
		layer_name: &str,
		output_index: usize,
	) {
		use colored::Colorize;

		// Create a header similar to tool execution style for assistant responses
		let title = format!(" Assistant Response | {} ", layer_name);
		let separator_length = 70.max(title.len() + 4);
		let dashes = "─".repeat(separator_length - title.len());
		let separator = format!("──{}{}──", title.bright_cyan(), dashes.dimmed());

		println!("{}", separator);

		// Display the content with smart formatting
		self.display_assistant_content_smart(output);

		// Add completion indicator
		println!(
			"{}",
			format!("✓ Layer '{}' output {} completed", layer_name, output_index).bright_green()
		);
		println!("──────────────────");

		// Ensure output is flushed in non-interactive mode
		use std::io::Write;
		std::io::stdout().flush().ok();
	}

	/// Display assistant content with smart formatting (similar to tool output formatting)
	fn display_assistant_content_smart(&self, content: &str) {
		let lines: Vec<&str> = content.lines().collect();

		if lines.len() <= 50 && content.chars().count() <= 5000 {
			// Reasonable size: show as-is
			println!("{}", content);
		} else if lines.len() > 50 {
			// Many lines: show first 40 lines + summary
			for line in lines.iter().take(40) {
				println!("{}", line);
			}
			println!(
				"{}",
				format!("... [+{} more lines]", lines.len().saturating_sub(40)).bright_black()
			);
		} else {
			// Long content: truncate with indication
			let truncated: String = content.chars().take(4997).collect();
			println!("{}...", truncated);
			println!("{}", "[Content truncated for display]".bright_black());
		}
	}
}
