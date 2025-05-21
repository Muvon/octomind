use crate::config::Config;
use crate::session::{Message, Session, openrouter};
use super::layer_trait::{Layer, LayerConfig, LayerResult};
use anyhow::Result;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use async_trait::async_trait;
use colored::Colorize;

// Base processor that handles common functionality for all layers
pub struct LayerProcessor {
    pub config: LayerConfig,
}

impl LayerProcessor {
    pub fn new(config: LayerConfig) -> Self {
        Self { config }
    }

    // Create messages for the OpenRouter API based on the layer
    pub fn create_messages(
        &self,
        input: &str,
        _session: &Session,
    ) -> Vec<Message> {
        let mut messages = Vec::new();

        // System message with layer-specific prompt
        // Only mark system messages as cached if the model supports it
        let should_cache = crate::session::model_utils::model_supports_caching(&self.config.model);
        
        // Process placeholders in the system prompt
        let processed_prompt = if self.config.system_prompt.contains("%{") {
            // Process placeholders if they exist
            let project_dir = std::env::current_dir().unwrap_or_default();
            crate::session::process_placeholders(&self.config.system_prompt, &project_dir)
        } else {
            // No placeholders, use the prompt as is
            self.config.system_prompt.clone()
        };

        messages.push(Message {
            role: "system".to_string(),
            content: processed_prompt,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            cached: should_cache, // Only cache if model supports it
        });

        // Add user message with the input
        messages.push(Message {
            role: "user".to_string(),
            content: input.to_string(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            cached: false,
        });

        messages
    }
}

// Async implementation of the Layer trait for LayerProcessor
#[async_trait]
impl Layer for LayerProcessor {
    fn name(&self) -> &str {
        &self.config.name
    }

    fn config(&self) -> &LayerConfig {
        &self.config
    }

    async fn process(
        &self,
        input: &str,
        session: &Session,
        config: &Config,
        operation_cancelled: Arc<AtomicBool>
    ) -> Result<LayerResult> {
        // Check if operation was cancelled
        if operation_cancelled.load(Ordering::SeqCst) {
            return Err(anyhow::anyhow!("Operation cancelled"));
        }

        // Prepare input based on input_mode
        let processed_input = self.prepare_input(input, session);

        // Create messages for this layer
        let messages = self.create_messages(&processed_input, session);

        // Convert to OpenRouter format
        let or_messages = openrouter::convert_messages(&messages);

        // Call the model
        let (output, exchange) = openrouter::chat_completion(
            or_messages,
            &self.config.model,
            self.config.temperature,
            config
        ).await?;

        // Check if the layer response contains tool calls
        if config.mcp.enabled && self.config.enable_tools {
            let tool_calls = crate::session::mcp::parse_tool_calls(&output);

            // If there are tool calls, process them
            if !tool_calls.is_empty() {
                // Create a new session-like context for tool responses
                let mut tool_session = Vec::new();
                let output_clone = output.clone();

                // Process tool calls
                println!("{}", "Executing tools within layer...".yellow());

                // Execute all tool calls and collect results
                let mut tool_results = Vec::new();

                for tool_call in &tool_calls {
                    println!("{} {}", "Tool call:".yellow(), tool_call.tool_name);
                    
                    // Check if tool is allowed for this layer
                    if !self.config.allowed_tools.is_empty() && 
                       !self.config.allowed_tools.contains(&tool_call.tool_name) {
                        println!("{} {} {}", "Tool".red(), tool_call.tool_name, "not allowed for this layer".red());
                        continue;
                    }
                    
                    let result = match crate::session::mcp::execute_layer_tool_call(tool_call, config, &self.config).await {
                        Ok(res) => res,
                        Err(e) => {
                            println!("{} {}", "Tool execution error:".red(), e);
                            continue;
                        }
                    };

                    // Add result to collection
                    tool_results.push(result);
                }

                // If we have results, format them and send back to the model
                if !tool_results.is_empty() {
                    // Format the results
                    let formatted = crate::session::mcp::format_tool_results(&tool_results);
                    println!("{}", formatted);

                    // Create the format expected by the model
                    let tool_results_message = serde_json::to_string(&tool_results)
                        .unwrap_or_else(|_| "[]".to_string());

                    let tool_message = format!("<fnr>\n{}\n</fnr>",
                        tool_results_message);

                    // Add the original messages
                    tool_session.extend(messages);

                    // Add assistant's response with tool calls
                    tool_session.push(crate::session::Message {
                        role: "assistant".to_string(),
                        content: output_clone,
                        timestamp: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs(),
                        cached: false,
                    });

                    // Add tool results as user message
                    tool_session.push(crate::session::Message {
                        role: "user".to_string(),
                        content: tool_message,
                        timestamp: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs(),
                        cached: false,
                    });

                    // Convert to OpenRouter format
                    let tool_or_messages = openrouter::convert_messages(&tool_session);

                    // Call the model again with tool results
                    match openrouter::chat_completion(
                        tool_or_messages,
                        &self.config.model,
                        self.config.temperature,
                        config
                    ).await {
                        Ok((new_output, new_exchange)) => {
                            // Extract token usage if available
                            let token_usage = new_exchange.usage.clone();

                            // Return the result with the updated output
                            return Ok(LayerResult {
                                output: new_output,
                                exchange: new_exchange,
                                token_usage,
                            });
                        },
                        Err(e) => {
                            println!("{} {}", "Error processing tool results:".red(), e);
                            // Continue with the original output
                        }
                    }
                }
            }
        }

        // Extract token usage if available
        let token_usage = exchange.usage.clone();

        // Return the result
        Ok(LayerResult {
            output,
            exchange,
            token_usage,
        })
    }
}