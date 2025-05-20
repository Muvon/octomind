// Common processor implementation for layers

use crate::config::Config;
use crate::session::{Message, Session, openrouter};
use crate::session::layers::{Layer, LayerType, LayerConfig, LayerResult};
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
    
    // Create messages for the OpenRouter API based on the current layer
    pub fn create_messages(
        &self,
        input: &str,
        session: &Session,
    ) -> Vec<Message> {
        let mut messages = Vec::new();
        
        // System message with layer-specific prompt
        // Always mark system messages as cached to save tokens
        messages.push(Message {
            role: "system".to_string(),
            content: self.config.system_prompt.clone(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            cached: true, // Mark system messages as cached by default
        });
        
        // Add appropriate user message based on layer type
        match self.config.layer_type {
            LayerType::QueryProcessor => {
                // Just pass the raw user input
                messages.push(Message {
                    role: "user".to_string(),
                    content: input.to_string(),
                    timestamp: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                    cached: false,
                });
            },
            LayerType::ContextGenerator => {
                // Pass both the original query and the processed query from QueryProcessor
                let original_query = session.messages.iter()
                    .find(|m| m.role == "user")
                    .map(|m| m.content.clone())
                    .unwrap_or_else(|| "No original query found".to_string());
                    
                messages.push(Message {
                    role: "user".to_string(),
                    content: format!("Original query: {}\n\nProcessed query: {}", 
                                     original_query, input),
                    timestamp: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                    cached: false,
                });
            },
            LayerType::Developer => {
                // For Developer, include the processed query and context from ContextGenerator
                messages.push(Message {
                    role: "user".to_string(),
                    content: format!("Task: {}\n\nContext Information: {}", 
                                    session.messages.iter()
                                        .find(|m| m.role == "assistant" && m.content.contains("Processed query:"))
                                        .map(|m| m.content.split("Processed query:").nth(1).unwrap_or(""))
                                        .unwrap_or(input),
                                    input),
                    timestamp: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                    cached: false,
                });
            },
            LayerType::Reducer => {
                // For Reducer, include the original request and the Developer's output
                let original_query = session.messages.iter()
                    .find(|m| m.role == "user")
                    .map(|m| m.content.clone())
                    .unwrap_or_else(|| "No original query found".to_string());
                
                let developer_output = input; // Developer output from previous layer
                
                messages.push(Message {
                    role: "user".to_string(),
                    content: format!("Original request: {}\n\nDeveloper solution: {}", 
                                     original_query, developer_output),
                    timestamp: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                    cached: false,
                });
            },
        }
        
        messages
    }
}

// Async implementation of the Layer trait for LayerProcessor
#[async_trait]
impl Layer for LayerProcessor {
    fn get_type(&self) -> LayerType {
        self.config.layer_type
    }
    
    fn get_config(&self) -> &LayerConfig {
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
        
        // Create messages for this layer
        let messages = self.create_messages(input, session);
        
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
                    // Ensure usage parameter is included for consistent cost tracking
                    match openrouter::chat_completion(
                        tool_or_messages,
                        &self.config.model,
                        self.config.temperature,
                        config
                    ).await {
                                        Ok((new_output, new_exchange)) => {
                                            // Log cost information for debugging
                                            if let Some(usage) = &new_exchange.usage {
                                                if let Some(cost) = usage.cost {
                                                    println!("{} ${:.5}", "Tool call response cost:".bright_magenta(), cost);
                                                } else {
                                                    // Try to get cost from raw response
                                                    let cost_from_raw = new_exchange.response.get("usage")
                                                        .and_then(|u| u.get("cost"))
                                                        .and_then(|c| c.as_f64());
                                                        
                                                    if let Some(cost) = cost_from_raw {
                                                        println!("{} ${:.5} (from raw response)", "Tool call response cost:".bright_magenta(), cost);
                                                    } else {
                                                        println!("{}", "ERROR: OpenRouter did not provide cost data for tool call response".bright_red());
                                                        println!("{}", "Make sure usage.include=true is set!".bright_red());
                                                        
                                                        // Check if usage tracking was explicitly requested
                                                        let has_usage_flag = new_exchange.request.get("usage")
                                                            .and_then(|u| u.get("include"))
                                                            .and_then(|i| i.as_bool())
                                                            .unwrap_or(false);
                                                            
                                                        println!("{} {}", "Request had usage.include flag:".bright_yellow(), has_usage_flag);
                                                    }
                                                }
                                            } else {
                                                println!("{}", "ERROR: No usage data for tool call response".bright_red());
                                            }
                            
                            // Check if the new output contains more tool calls
                            let new_tool_calls = crate::session::mcp::parse_tool_calls(&new_output);
                            
                            if !new_tool_calls.is_empty() {
                                // Process recursive tool calls
                                println!("{}", "Found recursive tool calls, processing...".yellow());
                                
                                // Create a new session for tool responses
                                let mut recursive_tool_session = Vec::new();
                                let recursive_output_clone = new_output.clone();
                                
                                // Add all messages up to this point
                                recursive_tool_session.extend(tool_session);
                                
                                // Add assistant's response with new tool calls
                                recursive_tool_session.push(crate::session::Message {
                                    role: "assistant".to_string(),
                                    content: recursive_output_clone,
                                    timestamp: std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .unwrap_or_default()
                                        .as_secs(),
                                    cached: false,
                                });
                                
                                // Execute all tool calls and collect results
                                let mut recursive_tool_results = Vec::new();
                                
                                for tool_call in &new_tool_calls {
                                    println!("{} {}", "Recursive tool call:".yellow(), tool_call.tool_name);
                                    let result = match crate::session::mcp::execute_layer_tool_call(tool_call, config, &self.config).await {
                                        Ok(res) => res,
                                        Err(e) => {
                                            println!("{} {}", "Tool execution error:".red(), e);
                                            continue;
                                        }
                                    };
                                    
                                    // Add result to collection
                                    recursive_tool_results.push(result);
                                }
                                
                                // If we have results, format them and send back to the model
                                if !recursive_tool_results.is_empty() {
                                    // Format the results
                                    let recursive_formatted = crate::session::mcp::format_tool_results(&recursive_tool_results);
                                    println!("{}", recursive_formatted);
                                    
                                    // Create the format expected by the model
                                    let recursive_results_message = serde_json::to_string(&recursive_tool_results)
                                        .unwrap_or_else(|_| "[]".to_string());
                                    
                                    let recursive_tool_message = format!("<fnr>\n{}\n</fnr>",
                                        recursive_results_message);
                                    
                                    // Add tool results as user message
                                    recursive_tool_session.push(crate::session::Message {
                                        role: "user".to_string(),
                                        content: recursive_tool_message,
                                        timestamp: std::time::SystemTime::now()
                                            .duration_since(std::time::UNIX_EPOCH)
                                            .unwrap_or_default()
                                            .as_secs(),
                                        cached: false,
                                    });
                                    
                                    // Convert to OpenRouter format
                                    let recursive_or_messages = openrouter::convert_messages(&recursive_tool_session);
                                    
                                    // Call the model again with recursive tool results
                                    // Ensure usage parameter is included for consistent cost tracking
                                    match openrouter::chat_completion(
                                        recursive_or_messages,
                                        &self.config.model,
                                        self.config.temperature,
                                        config
                                    ).await {
                                        Ok((final_output, final_exchange)) => {
                                            // Extract token usage if available
                                            let token_usage = final_exchange.usage.clone();
                                            
                                            // Log cost information for debugging
                                            if let Some(usage) = &final_exchange.usage {
                                                if let Some(cost) = usage.cost {
                                                    println!("{} ${:.5}", "Recursive tool call cost:".bright_magenta(), cost);
                                                } else {
                                                    // Try to get cost from raw response
                                                    let cost_from_raw = final_exchange.response.get("usage")
                                                        .and_then(|u| u.get("cost"))
                                                        .and_then(|c| c.as_f64());
                                                        
                                                    if let Some(cost) = cost_from_raw {
                                                        println!("{} ${:.5} (from raw response)", "Recursive tool call cost:".bright_magenta(), cost);
                                                    } else {
                                                        println!("{}", "ERROR: OpenRouter did not provide cost data for recursive tool call".bright_red());
                                                        println!("{}", "Make sure usage.include=true is set!".bright_red());
                                                        
                                                        // Check if usage tracking was explicitly requested
                                                        let has_usage_flag = final_exchange.request.get("usage")
                                                            .and_then(|u| u.get("include"))
                                                            .and_then(|i| i.as_bool())
                                                            .unwrap_or(false);
                                                            
                                                        println!("{} {}", "Request had usage.include flag:".bright_yellow(), has_usage_flag);
                                                    }
                                                }
                                            } else {
                                                println!("{}", "ERROR: No usage data for recursive tool call".bright_red());
                                            }
                                            
                                            // Return the result with the final output
                                            return Ok(LayerResult {
                                                output: final_output,
                                                exchange: final_exchange,
                                                token_usage,
                                            });
                                        },
                                        Err(e) => {
                                            println!("{} {}", "Error processing recursive tool results:".red(), e);
                                            // Fall back to the non-recursive output
                                        }
                                    }
                                }
                            }
                            
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