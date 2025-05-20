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
				// Calculate cost if available, or estimate it
				let cost = if let Some(cost_credits) = usage.cost {
					// Convert from credits to dollars (100,000 credits = $1)
					cost_credits as f64 / 100000.0
				} else {
					// Fallback to estimating cost using model pricing
					let input_price = config.openrouter.pricing.input_price;
					let output_price = config.openrouter.pricing.output_price;
					let input_cost = usage.prompt_tokens as f64 * input_price;
					let output_cost = usage.completion_tokens as f64 * output_price;
					input_cost + output_cost
				};

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
		println!("{}", format!("Estimated cost: ${:.4}", total_cost).bright_blue());

		// The Developer layer's output is what we want to return to the user
		Ok(developer_output)
	}
}
