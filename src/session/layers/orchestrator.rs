// Layer orchestration and pipeline management

use crate::config::Config;
use crate::session::{Session, get_layer_system_prompt, Message, openrouter};
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
        // Create 4-layer architecture
        let layers = vec![
            LayerProcessor::new(LayerConfig {
                layer_type: LayerType::QueryProcessor,
                enabled: true,
                model: LayerType::QueryProcessor.default_model().to_string(),
                system_prompt: get_layer_system_prompt(LayerType::QueryProcessor),
                temperature: 0.7,
                enable_tools: false, // No tools for QueryProcessor 
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
                layer_type: LayerType::Reducer,
                enabled: true,
                model: LayerType::Reducer.default_model().to_string(),
                system_prompt: get_layer_system_prompt(LayerType::Reducer),
                temperature: 0.7,
                enable_tools: false, // No tools for Reducer
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
                LayerType::Reducer => {
                    if let Some(model) = &config.openrouter.summarizer_model {
                        layer.config.model = model.clone();
                    }
                }
            }
        }
        
        orchestrator
    }
    
    // Process user input through the simplified layers
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
        println!("{}", "═════════════ Layered Processing Pipeline ═════════════".bright_cyan());
        println!("{}", "Starting simplified layered processing".bright_green());
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
                    // Note: We don't need to explicitly call reduce_session_context anymore
                    // The Reducer layer handles both documentation updates and context management
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
            
            // Special handling after Reducer layer completes
            if layer_type == LayerType::Reducer {
                // The Reducer layer output becomes the new cached context
                // Clear the session and start fresh
                let system_message = session.messages.iter()
                    .find(|m| m.role == "system")
                    .cloned();
                
                session.messages.clear();
                
                // Restore system message
                if let Some(system) = system_message {
                    session.messages.push(system);
                }
                
                // Add Reducer's output as a cached context for next iteration
                session.add_message("assistant", &result.output);
                let last_index = session.messages.len() - 1;
                session.messages[last_index].cached = true;
                
                println!("{}", "Session context optimized for next interaction".bright_green());
            }
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
    
    // Reduce session context to optimize token usage for future interactions
    async fn reduce_session_context(
        &self,
        session: &mut Session,
        config: &Config,
        developer_output: &str,
        operation_cancelled: Arc<AtomicBool>
    ) -> Result<()> {
        println!("{}", "Applying context reduction for next interaction".bright_yellow());
        
        // Get the current token count estimate
        let token_count = self.estimate_token_count(session);
        
        if token_count > self.token_threshold {
            println!("{}", "Token threshold exceeded - optimizing session context".bright_blue());
            
            // Check if operation was cancelled
            if operation_cancelled.load(Ordering::SeqCst) {
                return Err(anyhow::anyhow!("Operation cancelled"));
            }
            
            // Save the system message
            let system_message = session.messages.iter()
                .find(|m| m.role == "system")
                .cloned();
            
            // Extract key information from the developer output
            let condensed_information = format!("Previous work summary: {}", developer_output);
            
            // Create a temporary reduced session for generating optimized context
            let mut reduced_messages = Vec::new();
            
            // Add system message if exists
            if let Some(system) = &system_message {
                reduced_messages.push(system.clone());
            }
            
            // Add the condensed information as a user message
            reduced_messages.push(Message {
                role: "user".to_string(),
                content: "Please create a condensed summary of the conversation that preserves all key information for future reference. Focus on technical details, code changes, and important context.".to_string(),
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
                cached: false,
            });
            
            // Create a message with the developer output as context
            reduced_messages.push(Message {
                role: "user".to_string(),
                content: condensed_information.clone(),
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
                cached: false,
            });
            
            // Convert to OpenRouter format
            let or_messages = openrouter::convert_messages(&reduced_messages);
            
            // Check if operation was cancelled
            if operation_cancelled.load(Ordering::SeqCst) {
                return Err(anyhow::anyhow!("Operation cancelled"));
            }
            
            // Get a simpler model to optimize the context
            let model = "openai/gpt-3.5-turbo";
            
            // Call the model to generate the optimized context
            match openrouter::chat_completion(
                or_messages,
                model,
                0.7, // moderate temperature
                config
            ).await {
                Ok((optimized_context, exchange)) => {
                    // Track the token usage for this optimization step
                    if let Some(usage) = &exchange.usage {
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
                        
                        // Add context optimization stats to session
                        session.add_layer_stats(
                            "context_optimization",
                            model,
                            usage.prompt_tokens,
                            usage.completion_tokens,
                            cost
                        );
                    }
                    
                    // Clear the session and start fresh
                    session.messages.clear();
                    
                    // Restore system message
                    if let Some(system) = system_message {
                        session.messages.push(system);
                    }
                    
                    // Add optimized context as a cached message
                    session.add_message("assistant", &optimized_context);
                    let last_index = session.messages.len() - 1;
                    session.messages[last_index].cached = true;
                    
                    println!("{}", "Session context reduced and optimized for next interaction".bright_green());
                },
                Err(e) => {
                    // If optimization fails, fall back to simple context pruning
                    println!("{} {}", "Context optimization error:".red(), e);
                    println!("{}", "Falling back to simple context reduction".yellow());
                    
                    // Clear the session and start fresh
                    session.messages.clear();
                    
                    // Restore system message
                    if let Some(system) = system_message {
                        session.messages.push(system);
                    }
                    
                    // Add condensed information as a cached message
                    session.add_message("assistant", &condensed_information);
                    let last_index = session.messages.len() - 1;
                    session.messages[last_index].cached = true;
                }
            }
        } else {
            println!("{}", "Token count within threshold - preserving full context".bright_green());
        }
        
        Ok(())
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