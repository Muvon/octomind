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
    pub token_threshold: usize,
}

impl LayeredOrchestrator {
    pub fn new() -> Self {
        // Create default configuration with all layers
        let layers = vec![
            LayerProcessor::new(LayerConfig {
                layer_type: LayerType::QueryProcessor,
                enabled: true,
                model: LayerType::QueryProcessor.default_model().to_string(),
                system_prompt: get_layer_system_prompt(LayerType::QueryProcessor),
                temperature: 0.7,
                enable_tools: false, // By default, only enable tools for Developer
                allowed_tools: Vec::new(),
            }),
            LayerProcessor::new(LayerConfig {
                layer_type: LayerType::ContextGenerator,
                enabled: true,
                model: LayerType::ContextGenerator.default_model().to_string(),
                system_prompt: get_layer_system_prompt(LayerType::ContextGenerator),
                temperature: 0.7,
                enable_tools: true, // Enable tools for context gathering
                allowed_tools: vec!["shell".to_string(), "text_editor".to_string(), "list_files".to_string()],
            }),
            LayerProcessor::new(LayerConfig {
                layer_type: LayerType::Developer,
                enabled: true,
                model: LayerType::Developer.default_model().to_string(),
                system_prompt: get_layer_system_prompt(LayerType::Developer),
                temperature: 0.7,
                enable_tools: true, // Enable tools for main development
                allowed_tools: Vec::new(), // All tools available
            }),
            LayerProcessor::new(LayerConfig {
                layer_type: LayerType::Summarizer,
                enabled: true,
                model: LayerType::Summarizer.default_model().to_string(),
                system_prompt: get_layer_system_prompt(LayerType::Summarizer),
                temperature: 0.7,
                enable_tools: false,
                allowed_tools: Vec::new(),
            }),
            LayerProcessor::new(LayerConfig {
                layer_type: LayerType::NextRequest,
                enabled: true,
                model: LayerType::NextRequest.default_model().to_string(),
                system_prompt: get_layer_system_prompt(LayerType::NextRequest),
                temperature: 0.7,
                enable_tools: false,
                allowed_tools: Vec::new(),
            }),
            LayerProcessor::new(LayerConfig {
                layer_type: LayerType::SessionReviewer,
                enabled: true,
                model: LayerType::SessionReviewer.default_model().to_string(),
                system_prompt: get_layer_system_prompt(LayerType::SessionReviewer),
                temperature: 0.7,
                enable_tools: false,
                allowed_tools: Vec::new(),
            }),
        ];
        
        Self { 
            layers,
            token_threshold: 4000, // Default token threshold for session reduction
        }
    }
    
    // Create from config
    pub fn from_config(config: &Config) -> Self {
        let mut orchestrator = Self::new();
        
        // Update models based on config
        for layer in &mut orchestrator.layers {
            match layer.get_type() {
                LayerType::QueryProcessor => {
                    if let Some(model) = &config.openrouter.query_processor_model {
                        layer.config.model = model.clone();
                    }
                },
                LayerType::ContextGenerator => {
                    if let Some(model) = &config.openrouter.context_generator_model {
                        layer.config.model = model.clone();
                    }
                },
                LayerType::Developer => {
                    if let Some(model) = &config.openrouter.developer_model {
                        layer.config.model = model.clone();
                    } else {
                        // Use the main model if no specific developer model
                        layer.config.model = config.openrouter.model.clone();
                    }
                },
                LayerType::Summarizer => {
                    if let Some(model) = &config.openrouter.summarizer_model {
                        layer.config.model = model.clone();
                    }
                },
                LayerType::NextRequest => {
                    if let Some(model) = &config.openrouter.next_request_model {
                        layer.config.model = model.clone();
                    }
                },
                LayerType::SessionReviewer => {
                    if let Some(model) = &config.openrouter.session_reviewer_model {
                        layer.config.model = model.clone();
                    }
                },
            }
        }
        
        orchestrator
    }
    
    // Process user input through all layers
    pub async fn process(
        &self,
        input: &str,
        session: &mut Session,
        config: &Config,
        operation_cancelled: Arc<AtomicBool>
    ) -> Result<String> {
        let mut current_input = input.to_string();
        let mut outputs = Vec::new();
        let mut developer_output = String::new();
        
        // For total token/cost tracking across all layers
        let mut total_input_tokens = 0;
        let mut total_output_tokens = 0;
        let mut total_cost = 0.0;
        
        // Debug information for user
        println!("{}", "═════════════ Layered Processing Pipeline ═════════════".bright_cyan());
        println!("{}", "Starting layered processing with modular AI architecture".bright_green());
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
            
            // For Context Generator, print a message indicating successful context gathering
            if layer_type == LayerType::ContextGenerator {
                println!("{}", "Context gathered successfully".bright_green());
            }
            // For Developer layer, indicate we're now executing the main work
            else if layer_type == LayerType::Developer {
                println!("{}", "Executing developer tasks...".bright_yellow());
            }
            
            // Silently track layer stats without displaying
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
            
            // Store the result for this layer
            outputs.push((layer_type, result.output.clone()));
            
            // Special handling for the Developer layer
            if layer_type == LayerType::Developer {
                developer_output = result.output.clone();
            }
            
            // Update input for next layer
            current_input = result.output;
            
            // Apply session review/reduction if token threshold exceeded
            if layer_type == LayerType::SessionReviewer {
                // Get token count estimate
                let token_count = self.estimate_token_count(session);
                
                if token_count > self.token_threshold {
                    println!("{}", "Token threshold exceeded - applying session reduction".bright_red());
                    
                    // Replace session with condensed version
                    let condensed_summary = current_input.clone();
                    
                    // Clear most messages but keep system and the summary
                    let system_message = session.messages.iter()
                        .find(|m| m.role == "system")
                        .cloned();
                    
                    // Create a new reduced session
                    session.messages.clear();
                    
                    // Restore system message
                    if let Some(system) = system_message {
                        session.messages.push(system);
                    }
                    
                    // Add summary as cached context
                    session.add_message("assistant", &condensed_summary);
                    let last_index = session.messages.len() - 1;
                    session.messages[last_index].cached = true;
                    
                    println!("{}", "Session reduced successfully - context preserved and cached".bright_green());
                }
            }
        }
        
        // Display minimal completion info
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
    
    // Estimate token count for a session (rough approximation)
    fn estimate_token_count(&self, session: &Session) -> usize {
        let mut count = 0;
        
        for msg in &session.messages {
            // Count words as a rough approximation of tokens
            count += msg.content.split_whitespace().count();
            
            // Add overhead for message format
            count += 4; // Role, timestamp, etc.
        }
        
        count
    }
}