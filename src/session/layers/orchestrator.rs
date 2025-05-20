// Layer orchestration and pipeline management

use crate::config::Config;
use crate::session::{Session, get_layer_system_prompt};
use super::{LayerType, LayerConfig, Layer};
use super::processor::LayerProcessor;
use anyhow::Result;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use colored::*;

// Main layered orchestrator that manages the pipeline of layers
pub struct LayeredOrchestrator {
	pub layers: Vec<LayerProcessor>,
}

impl LayeredOrchestrator {
	// Create from config
	pub fn from_config(config: &Config) -> Self {
		// Create 3-layer architecture (excluding Reducer which is now manual only via /done)
		let layers = vec![
			LayerProcessor::new(LayerConfig {
				layer_type: LayerType::QueryProcessor,
				enabled: true,
				model: if let Some(model) = &config.openrouter.query_processor_model {
					model.clone()
				} else {
					LayerType::QueryProcessor.default_model().to_string()
				},
				system_prompt: get_layer_system_prompt(LayerType::QueryProcessor),
				temperature: 0.7,
				enable_tools: false, // No tools for QueryProcessor
				allowed_tools: Vec::new(),
			}),
			LayerProcessor::new(LayerConfig {
				layer_type: LayerType::ContextGenerator,
				enabled: true,
				model: if let Some(model) = &config.openrouter.context_generator_model {
					model.clone()
				} else {
					LayerType::ContextGenerator.default_model().to_string()
				},
				system_prompt: get_layer_system_prompt(LayerType::ContextGenerator),
				temperature: 0.7,
				enable_tools: true, // Enable tools for context gathering
				allowed_tools: vec!["shell".to_string(), "text_editor".to_string(), "list_files".to_string()],
			}),
			LayerProcessor::new(LayerConfig {
				layer_type: LayerType::Developer,
				enabled: true,
				model: if let Some(model) = &config.openrouter.developer_model {
					model.clone()
				} else {
					config.openrouter.model.clone()
				},
				system_prompt: get_layer_system_prompt(LayerType::Developer),
				temperature: 0.7,
				enable_tools: true, // Enable tools for main development
				allowed_tools: Vec::new(), // All tools available
			}),
		];

		Self {
			layers,
		}
	}

	// Process user input through the 3-layer architecture
	pub async fn process(
		&self,
		input: &str,
		session: &mut Session,
		config: &Config,
		operation_cancelled: Arc<AtomicBool>
	) -> Result<String> {
		let mut current_input = input.to_string();
		let mut developer_output = String::new();

		// For total token/cost tracking across all layers
		let mut total_input_tokens = 0;
		let mut total_output_tokens = 0;
		let mut total_cost = 0.0;

		// Debug information for user
		println!("{}", "═════════════ 3-Layer Processing Pipeline ═════════════".bright_cyan());
		println!("{}", "Starting 3-layer processing".bright_green());
		println!();

		// Process through each layer sequentially
		for layer in &self.layers {
			// Skip if operation cancelled
			if operation_cancelled.load(Ordering::SeqCst) {
				return Err(anyhow::anyhow!("Operation cancelled"));
			}

			let layer_type = layer.get_type();
			println!("{}", format!("───── Layer: {} ─────", layer_type.as_str()).bright_yellow());

			// Process the layer
			println!("{}", "Input:".bright_blue());
			println!("{}", current_input);
			
			// Clear any previous animation line and show current cost
			print!("\r                                                                  \r");
			println!("{} ${:.5}", "Generating response with current cost:".bright_cyan(), total_cost);
			
			// Debug info for caching
			println!("{}", "System message will be cached for this layer".bright_magenta());

			let result = layer.process(
				&current_input,
				session,
				config,
				operation_cancelled.clone()
			).await?;

			println!("{}", "Output:".bright_green());
			println!("{}", result.output);

			// Layer-specific messages
			match layer_type {
				LayerType::QueryProcessor => {
					println!("{}", "Query processed and improved".bright_green());
				},
				LayerType::ContextGenerator => {
					println!("{}", "Context gathered successfully".bright_green());
				},
				LayerType::Developer => {
					println!("{}", "Development tasks completed".bright_green());
					// Store the developer output to return to the user
					developer_output = result.output.clone();
				},
				LayerType::Reducer => {
					println!("{}", "Documentation updated and context optimized".bright_green());
				}
			}

			// Track token usage stats
			if let Some(usage) = &result.token_usage {
				// Try to get cost from the TokenUsage struct first
				if let Some(cost) = usage.cost {
					// Display the layer cost
					println!("{}", format!("Layer cost: ${:.5} (Input: {} tokens, Output: {} tokens)", 
						cost, usage.prompt_tokens, usage.completion_tokens).bright_magenta());

					// Add the stats to the session
					session.add_layer_stats(
						layer_type.as_str(),
						&layer.config.model,
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
							layer_type.as_str(),
							&layer.config.model,
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
						println!("{} {}", "ERROR: Layer".bright_red(), layer_type.as_str().bright_yellow());
						println!("{}", "OpenRouter did not provide cost data. Make sure usage.include=true is set!".bright_red());
						
						// Still track tokens
						total_input_tokens += usage.prompt_tokens;
						total_output_tokens += usage.completion_tokens;
						
						// Print the raw response for debugging
						if config.openrouter.debug {
							println!("{}", "Raw OpenRouter response for debug:".bright_red());
							if let Ok(resp_str) = serde_json::to_string_pretty(&result.exchange.response) {
								println!("{}", resp_str);
							}
						}
					}
				}
			} else {
				println!("{} {}", "ERROR: No usage data for layer".bright_red(), layer_type.as_str().bright_yellow());
			}
			
			// If no usage data was returned at all, that's an error
			let layer_has_usage = if let Some(_) = &result.token_usage {
				true
			} else {
				false
			};

			if !layer_has_usage {
				println!("{} {}", "ERROR: No token usage or cost information for layer".bright_red(), layer_type.as_str().bright_yellow());
			}

			// Update input for next layer
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

		// The Developer layer's output is what we want to return to the user
		Ok(developer_output)
	}
}