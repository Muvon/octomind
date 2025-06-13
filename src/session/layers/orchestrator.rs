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

use super::generic_layer::GenericLayer;
use super::layer_trait::{Layer, LayerConfig};
use crate::config::Config;
use crate::session::Session;
use anyhow::Result;
use colored::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

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

		// If no layers were configured or enabled, fall back to defaults
		if layers.is_empty() {
			layers = Self::create_default_system_layers();
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
			// Process and cache the system prompt for this layer
			layer_config
				.process_and_cache_system_prompt(project_dir)
				.await;
			layers.push(Box::new(GenericLayer::new(layer_config)));
		}

		// If no layers were configured or enabled, fall back to defaults
		if layers.is_empty() {
			let default_layers = Self::create_default_system_layers_configs();
			for mut layer_config in default_layers {
				layer_config
					.process_and_cache_system_prompt(project_dir)
					.await;
				layers.push(Box::new(GenericLayer::new(layer_config)));
			}
		}

		Self { layers }
	}

	// Create default system layers using the new generic layer approach
	fn create_default_system_layers() -> Vec<Box<dyn Layer + Send + Sync>> {
		let mut layers: Vec<Box<dyn Layer + Send + Sync>> = Vec::new();

		// Create default system layers using LayerConfig::create_system_layer
		let query_config = LayerConfig::create_system_layer("query_processor");
		layers.push(Box::new(GenericLayer::new(query_config)));

		let context_config = LayerConfig::create_system_layer("context_generator");
		layers.push(Box::new(GenericLayer::new(context_config)));

		layers
	}

	// Create default system layer configs (for async processing)
	fn create_default_system_layers_configs() -> Vec<LayerConfig> {
		vec![
			LayerConfig::create_system_layer("query_processor"),
			LayerConfig::create_system_layer("context_generator"),
		]
	}

	// Process user input through the layer architecture
	pub async fn process(
		&self,
		input: &str,
		session: &mut Session,
		config: &Config,
		operation_cancelled: Arc<AtomicBool>,
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

		// Debug information for user
		println!(
			"{}",
			"═════════════ Layer Processing Pipeline ═════════════".bright_cyan()
		);
		println!(
			"{}",
			format!("Starting processing with {} layers", self.layers.len()).bright_green()
		);
		println!();

		// Process through each layer sequentially
		// Each layer operates in its own isolated session and handles its own function calls
		for layer in &self.layers {
			// Skip if operation cancelled
			if operation_cancelled.load(Ordering::SeqCst) {
				return Err(anyhow::anyhow!("Operation cancelled"));
			}

			let layer_name = layer.name();
			println!(
				"{}",
				format!("───── Layer: {} ─────", layer_name).bright_yellow()
			);

			// Process the layer
			println!("{}", "Input:".bright_blue());
			println!("{}", current_input);

			// Clear any previous animation line and show current cost
			print!("\r                                                                  \r");
			println!(
				"{} ${:.5}",
				"Generating response with current cost:".bright_cyan(),
				total_cost
			);

			// Debug info for model and settings
			println!(
				"{} {} (temp: {})",
				"Using model:".bright_magenta(),
				layer.config().get_effective_model(&session.info.model),
				layer.config().temperature
			);

			if !layer.config().mcp.server_refs.is_empty() {
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

			// Process this layer with its own isolated session
			// The only input it receives is the output from the previous layer
			let result = layer
				.process(&current_input, session, config, operation_cancelled.clone())
				.await?;

			println!("{}", "Output:".bright_green());
			println!("{}", result.output);

			// Track token usage stats
			if let Some(usage) = &result.token_usage {
				// Try to get cost from the TokenUsage struct first
				if let Some(cost) = usage.cost {
					// Display the layer cost with time information
					println!("{}", format!("Layer cost: ${:.5} (Input: {} tokens, Output: {} tokens) | Time: API {}ms, Tools {}ms, Total {}ms",
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

			// Take the output from this layer and use it as input for the next layer
			current_input = result.output.clone();
		}

		// Display completion info
		println!();
		println!("{}", "Processing completed".bright_green());

		// Calculate total time across all layers
		let total_api_time_ms = session.info.total_api_time_ms;
		let total_tool_time_ms = session.info.total_tool_time_ms;
		let total_layer_time_ms = session.info.total_layer_time_ms;

		// Display cumulative token usage across all layers
		println!(
			"{}",
			format!(
				"Total tokens used: {} (Input: {}, Output: {})",
				total_input_tokens + total_output_tokens,
				total_input_tokens,
				total_output_tokens
			)
			.bright_blue()
		);
		println!(
			"{}",
			format!("Estimated cost for all layers: ${:.5}", total_cost).bright_blue()
		);
		println!(
			"{}",
			format!(
				"Total time: {}ms (API: {}ms, Tools: {}ms, Layer Processing: {}ms)",
				total_api_time_ms + total_tool_time_ms + total_layer_time_ms,
				total_api_time_ms,
				total_tool_time_ms,
				total_layer_time_ms
			)
			.bright_blue()
		);
		println!(
			"{}",
			"Use /info for detailed cost breakdown by layer".bright_blue()
		);

		// Return the final layer's output to be used as starting point for the main chat session
		// This output contains all the necessary context and information from the layer processing
		// When integrated into the main session via layered_response.rs, it becomes the foundation
		// for the entire conversation context, ensuring all the work done by the layers is preserved
		// and available for subsequent messages in the main chat flow.
		Ok(current_input)
	}
}
