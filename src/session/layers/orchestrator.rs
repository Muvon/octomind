use crate::config::Config;
use crate::session::Session;
use super::layer_trait::Layer;
use super::types::*;
use anyhow::Result;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use colored::*;

// Main layered orchestrator that manages the pipeline of layers
pub struct LayeredOrchestrator {
	pub layers: Vec<Box<dyn Layer + Send + Sync>>,
}

impl LayeredOrchestrator {
	// Create orchestrator from config
	pub fn from_config(config: &Config) -> Self {
		// Check if specific layer configs are in config.toml
		if let Some(layer_configs) = &config.layers {
			// Create layers from config
			let mut layers: Vec<Box<dyn Layer + Send + Sync>> = Vec::new();

			// Load each layer if it exists and is enabled in config
			for layer_config in layer_configs {
				if layer_config.enabled {
						if layer_config.name.as_str() == "query_processor" {
							layers.push(Box::new(QueryProcessorLayer::new(layer_config.clone())));
						} else if layer_config.name.as_str() == "context_generator" {
							layers.push(Box::new(ContextGeneratorLayer::new(layer_config.clone())));
						} else if layer_config.name.as_str() == "reducer" {
							layers.push(Box::new(ReducerLayer::new(layer_config.clone())));
						} else if layer_config.name.as_str() == "developer" {
							// Skip developer layer - it's been removed from the system
							println!("{}", "The developer layer has been removed. The main session will handle development tasks directly.".yellow());
						} else {
							// Unknown layer type
							println!("{} {}", "Unknown layer type:".yellow(), layer_config.name);
						}
				}
			}

			// If no layers were configured or enabled, fall back to defaults
			if layers.is_empty() {
				return Self::create_default_layers(config);
			}

			Self { layers }
		} else {
			// No layer config section, use defaults
			Self::create_default_layers(config)
		}
	}

	// Create default layers when no config is provided
	fn create_default_layers(_config: &Config) -> Self {
		// Create 2-layer architecture (Query Processor and Context Generator)
		let mut layers: Vec<Box<dyn Layer + Send + Sync>> = Vec::new();

		// Query Processor - use the default model from its implementation
		let query_config = QueryProcessorLayer::default_config("query_processor");
		layers.push(Box::new(QueryProcessorLayer::new(query_config)));

		// Context Generator - use the default model from its implementation
		let context_config = ContextGeneratorLayer::default_config("context_generator");
		layers.push(Box::new(ContextGeneratorLayer::new(context_config)));

		Self { layers }
	}

	// Process user input through the layer architecture
	pub async fn process(
		&self,
		input: &str,
		session: &mut Session,
		config: &Config,
		operation_cancelled: Arc<AtomicBool>
	) -> Result<String> {
		let mut current_input = input.to_string();

		// For total token/cost tracking across all layers
		let mut total_input_tokens = 0;
		let mut total_output_tokens = 0;
		let mut total_cost = 0.0;

		// Debug information for user
		println!("{}", "═════════════ Layer Processing Pipeline ═════════════".bright_cyan());
		println!("{}", format!("Starting processing with {} layers", self.layers.len()).bright_green());
		println!();

		// Process through each layer sequentially
		// Each layer operates in its own isolated session and handles its own function calls
		for layer in &self.layers {
			// Skip if operation cancelled
			if operation_cancelled.load(Ordering::SeqCst) {
				return Err(anyhow::anyhow!("Operation cancelled"));
			}

			let layer_name = layer.name();
			println!("{}", format!("───── Layer: {} ─────", layer_name).bright_yellow());

			// Process the layer
			println!("{}", "Input:".bright_blue());
			println!("{}", current_input);

			// Clear any previous animation line and show current cost
			print!("\r                                                                  \r");
			println!("{} ${:.5}", "Generating response with current cost:".bright_cyan(), total_cost);

			// Debug info for model and settings
			println!("{} {} (temp: {})", "Using model:".bright_magenta(),
				layer.config().model, layer.config().temperature);

			if layer.config().enable_tools {
				if layer.config().allowed_tools.is_empty() {
					println!("{}", "All tools enabled for this layer".bright_magenta());
				} else {
					println!("{} {}", "Tools enabled:".bright_magenta(),
						layer.config().allowed_tools.join(", "));
				}
			}

			// Process this layer with its own isolated session
			// The only input it receives is the output from the previous layer
			let result = layer.process(
				&current_input,
				session,
				config,
				operation_cancelled.clone()
			).await?;

			println!("{}", "Output:".bright_green());
			println!("{}", result.output);

			// Track token usage stats
			if let Some(usage) = &result.token_usage {
				// Try to get cost from the TokenUsage struct first
				if let Some(cost) = usage.cost {
					// Display the layer cost
					println!("{}", format!("Layer cost: ${:.5} (Input: {} tokens, Output: {} tokens)",
						cost, usage.prompt_tokens, usage.completion_tokens).bright_magenta());

					// Add the stats to the session
					session.add_layer_stats(
						layer_name,
						&layer.config().model,
						usage.prompt_tokens,
						usage.completion_tokens,
						cost
					);

					// Update totals for summary
					total_input_tokens += usage.prompt_tokens;
					total_output_tokens += usage.completion_tokens;
					total_cost += cost;
				} else {
					// Try to get cost from raw response JSON if not in TokenUsage
					let cost_from_raw = result.exchange.response.get("usage")
						.and_then(|u| u.get("cost"))
						.and_then(|c| c.as_f64());

					if let Some(cost) = cost_from_raw {
						// Log that we had to get cost from raw response
						println!("{}", format!("Layer cost (from raw): ${:.5} (Input: {} tokens, Output: {} tokens)",
							cost, usage.prompt_tokens, usage.completion_tokens).bright_magenta());

						// Add the stats to the session
						session.add_layer_stats(
							layer_name,
							&layer.config().model,
							usage.prompt_tokens,
							usage.completion_tokens,
							cost
						);

						// Update totals for summary
						total_input_tokens += usage.prompt_tokens;
						total_output_tokens += usage.completion_tokens;
						total_cost += cost;
					} else {
						// ERROR - OpenRouter did not provide cost data
						println!("{} {}", "ERROR: Layer".bright_red(), layer_name.bright_yellow());
						println!("{}", "OpenRouter did not provide cost data. Make sure usage.include=true is set!".bright_red());

						// Still track tokens
						total_input_tokens += usage.prompt_tokens;
						total_output_tokens += usage.completion_tokens;
					}
				}
			} else {
				println!("{} {}", "ERROR: No usage data for layer".bright_red(), layer_name.bright_yellow());
			}

			// Take the output from this layer and use it as input for the next layer
			current_input = result.output.clone();
		}

		// Display completion info
		println!();
		println!("{}", "Processing completed".bright_green());

		// Display cumulative token usage across all layers
		println!("{}", format!("Total tokens used: {} (Input: {}, Output: {})",
			total_input_tokens + total_output_tokens,
			total_input_tokens,
			total_output_tokens).bright_blue());
		println!("{}", format!("Estimated cost for all layers: ${:.5}", total_cost).bright_blue());
		println!("{}", "Use /info for detailed cost breakdown by layer".bright_blue());

		// Return the final layer's output to be used as starting point for the main chat session
		// This output contains all the necessary context and information from the layer processing
		// When integrated into the main session via layered_response.rs, it becomes the foundation
		// for the entire conversation context, ensuring all the work done by the layers is preserved
		// and available for subsequent messages in the main chat flow.
		Ok(current_input)
	}
}
