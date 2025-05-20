// Common processor implementation for layers

use crate::config::Config;
use crate::session::{Message, Session, openrouter};
use crate::session::layers::{Layer, LayerType, LayerConfig, LayerResult};
use anyhow::Result;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use async_trait::async_trait;

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
        messages.push(Message {
            role: "system".to_string(),
            content: self.config.system_prompt.clone(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            cached: false,
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
                // Pass both the original query and the processed query
                let original_query = session.messages.iter()
                    .find(|m| m.role == "user")
                    .map(|m| m.content.clone())
                    .unwrap_or_else(|| input.to_string());
                    
                messages.push(Message {
                    role: "user".to_string(),
                    content: format!("Original query: {}\n\nProcessed query: {}", 
                                     original_query, input),
                    timestamp: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                    cached: false, // Never use caching in layer processors except for Developer layer
                });
            },
            LayerType::Developer => {
                // For developer, include all previous context
                let mut context = String::new();
                
                // Look for context generator output
                for msg in session.messages.iter() {
                    if msg.role == "assistant" {
                        context = msg.content.clone();
                        break;
                    }
                }
                
                messages.push(Message {
                    role: "user".to_string(),
                    content: format!("Processed query: {}\n\nContext: {}", input, context),
                    timestamp: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                    cached: false,
                });
            },
            LayerType::Summarizer => {
                // For summarizer, include the developer output
                let mut developer_output = String::new();
                for msg in session.messages.iter().rev() {
                    if msg.role == "assistant" {
                        developer_output = msg.content.clone();
                        break;
                    }
                }
                
                // Find the original user query
                let original_query = session.messages.iter()
                    .find(|m| m.role == "user")
                    .map(|m| m.content.clone())
                    .unwrap_or_else(|| input.to_string());
                
                messages.push(Message {
                    role: "user".to_string(),
                    content: format!(
                        "Original query: {}\n\nDeveloper output: {}\n\nPlease create a summary and update documentation:",
                        original_query, developer_output
                    ),
                    timestamp: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                    cached: false,
                });
            },
            LayerType::NextRequest => {
                // For next request suggestions, include the summary
                let mut summary = String::new();
                for msg in session.messages.iter().rev() {
                    if msg.role == "assistant" {
                        summary = msg.content.clone();
                        break;
                    }
                }
                
                messages.push(Message {
                    role: "user".to_string(),
                    content: format!(
                        "Based on the work just completed: {}\n\nWhat are logical next steps or commands the user might want to execute?",
                        summary
                    ),
                    timestamp: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                    cached: false,
                });
            },
            LayerType::SessionReviewer => {
                // For session reviewer, include the complete history
                let mut history = String::new();
                for msg in &session.messages {
                    let role_display = match msg.role.as_str() {
                        "user" => "User",
                        "assistant" => "Assistant",
                        "system" => "System",
                        _ => continue,
                    };
                    
                    history.push_str(&format!("{}:\n{}\n\n", role_display, msg.content));
                }
                
                // Get token count estimate (rough calculation)
                let token_count = history.split_whitespace().count();
                let token_threshold = 4000; // Example threshold
                
                messages.push(Message {
                    role: "user".to_string(),
                    content: format!(
                        "Current session has approximately {} tokens. Threshold is {} tokens.\n\n\
                        Current conversation history:\n{}\n\n\
                        {}\n\nPlease create a condensed summary of this conversation that preserves all important context.",
                        token_count,
                        token_threshold,
                        history,
                        if token_count > token_threshold {
                            "TOKEN THRESHOLD EXCEEDED - Session reduction required."
                        } else {
                            "Token count is still under threshold, but please provide a summary for future reference."
                        }
                    ),
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