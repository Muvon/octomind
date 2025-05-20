// Layered architecture for OctoDev session component
// This module implements multi-layer processing for session handling

use crate::config::Config;
use crate::session::{Message, Session, openrouter};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

// Layer types in the architecture
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LayerType {
    QueryProcessor,    // Improves the initial query for better instructions
    ContextGenerator,  // Gathers and injects context information
    Developer,         // Executes the actual development work with Claude model
    Summarizer,        // Summarizes what was done
    NextRequest,       // Suggests the next user input/prompting
    SessionReviewer,   // Reviews and manages token usage, etc.
}

impl LayerType {
    pub fn as_str(&self) -> &'static str {
        match self {
            LayerType::QueryProcessor => "query_processor",
            LayerType::ContextGenerator => "context_generator",
            LayerType::Developer => "developer",
            LayerType::Summarizer => "summarizer", 
            LayerType::NextRequest => "next_request",
            LayerType::SessionReviewer => "session_reviewer",
        }
    }
    
    pub fn description(&self) -> &'static str {
        match self {
            LayerType::QueryProcessor => "Processes user input to create improved instructions",
            LayerType::ContextGenerator => "Gathers context information for the task",
            LayerType::Developer => "Executes development tasks based on instructions",
            LayerType::Summarizer => "Summarizes the task results",
            LayerType::NextRequest => "Suggests next steps or requests",
            LayerType::SessionReviewer => "Reviews and manages session context and tokens",
        }
    }
    
    pub fn default_model(&self) -> &'static str {
        match self {
            LayerType::QueryProcessor => "openai/gpt-4o",
            LayerType::ContextGenerator => "openai/gpt-4o",
            LayerType::Developer => "anthropic/claude-3.7-sonnet",
            LayerType::Summarizer => "openai/gpt-4o",
            LayerType::NextRequest => "openai/gpt-4o",
            LayerType::SessionReviewer => "openai/gpt-4o",
        }
    }
    
    pub fn default_system_prompt(&self) -> String {
        match self {
            LayerType::QueryProcessor => {
                "You are an expert query processor in the OctoDev system.\
                Your job is to analyze the user's query and improve it to create clearer \
                instructions for the development team. Focus on understanding the real intent \
                behind user requests, adding specificity where needed, and formatting the \
                request in a way that will lead to the most effective implementation. \
                Create a concise yet comprehensive set of instructions.".to_string()
            },
            LayerType::ContextGenerator => {
                "You are the context gathering specialist for the OctoDev system.\
                Your task is to identify what information would be most relevant to \
                complete the user's request. Think about what files, code snippets, \
                or project details would help solve this problem. You will prepare \
                a clear context package that can be cached for efficient processing.".to_string()
            },
            LayerType::Developer => {
                "You are OctoDev's core developer AI. Using the improved instructions \
                and context provided, implement the requested changes or provide solutions \
                to the user's coding problems. Execute tool calls as needed to accomplish \
                tasks and provide clear explanations of your work.".to_string()
            },
            LayerType::Summarizer => {
                "You are the summarization expert for OctoDev. Your job is to create \
                a concise summary of the work that was done in response to the user's \
                request. Focus on what changes were made, why they were made, and what \
                the outcome was. This summary should be clear and informative.".to_string()
            },
            LayerType::NextRequest => {
                "You are the forward-thinking component of OctoDev. Based on the work \
                just completed, suggest what the user might want to do next. Provide \
                thoughtful suggestions for next steps or further improvements that would \
                be logical to pursue.".to_string()
            },
            LayerType::SessionReviewer => {
                "You are the session management specialist for OctoDev. Your job is to \
                review the current session state, identify what information should be \
                retained in context for future interactions, and what can be summarized \
                or removed to optimize token usage. Provide recommendations for session \
                management.".to_string()
            },
        }
    }
}

// Configuration for a processing layer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayerConfig {
    pub layer_type: LayerType,
    pub enabled: bool,
    pub model: String,
    pub system_prompt: String,
    pub temperature: f32,
}

impl LayerConfig {
    pub fn new(layer_type: LayerType) -> Self {
        Self {
            layer_type,
            enabled: true,
            model: layer_type.default_model().to_string(),
            system_prompt: layer_type.default_system_prompt(),
            temperature: 0.7,
        }
    }
}

// Layer processor to handle individual layer execution
pub struct LayerProcessor {
    pub config: LayerConfig,
}

impl LayerProcessor {
    pub fn new(config: LayerConfig) -> Self {
        Self { config }
    }
    
    // Process a specific layer and return the result
    pub async fn process(
        &self,
        input: &str,
        session: &Session,
        global_config: &Config,
        operation_cancelled: Arc<AtomicBool>
    ) -> Result<(String, openrouter::OpenRouterExchange)> {
        // Create messages array with system prompt and user input
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
        
        // For some layers, we want to provide context from previous layers
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
                messages.push(Message {
                    role: "user".to_string(),
                    content: format!("Original query: {}\n\nProcessed query: {}", 
                                     session.messages.first().map(|m| &m.content).map_or(input, |v| v), 
                                     input),
                    timestamp: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                    cached: false,
                });
            },
            LayerType::Developer => {
                // For developer, include all previous context
                // Find the last message from context generator
                let mut context = String::new();
                let processed_query = input.to_string();
                
                for msg in &session.messages {
                    if msg.role == "assistant" {
                        // This is where we'd determine which layer produced this message
                        // For now, assume second assistant message is from context generator
                        context = msg.content.clone();
                        break;
                    }
                }
                
                messages.push(Message {
                    role: "user".to_string(),
                    content: format!("Processed query: {}\n\nContext: {}", processed_query, context),
                    timestamp: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                    cached: false,
                });
            },
            LayerType::Summarizer | LayerType::NextRequest | LayerType::SessionReviewer => {
                // These layers need the full conversation to work with
                // Find the most recent developer output
                let mut developer_output = String::new();
                for msg in session.messages.iter().rev() {
                    if msg.role == "assistant" {
                        // For now just use the most recent assistant message
                        developer_output = msg.content.clone();
                        break;
                    }
                }
                
                messages.push(Message {
                    role: "user".to_string(),
                    content: format!("Original query: {}\n\nDeveloper output: {}", input, developer_output),
                    timestamp: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                    cached: false,
                });
            },
        }
        
        // Check if operation was cancelled
        if operation_cancelled.load(Ordering::SeqCst) {
            return Err(anyhow::anyhow!("Operation cancelled"));
        }
        
        // Convert to OpenRouter format
        let or_messages = openrouter::convert_messages(&messages);
        
        // Call the model
        openrouter::chat_completion(
            or_messages,
            &self.config.model,
            self.config.temperature,
            global_config
        ).await
    }
}

// Main layered processor that orchestrates all layers
pub struct LayeredProcessor {
    pub layers: Vec<LayerConfig>,
}

impl LayeredProcessor {
    pub fn new() -> Self {
        // Create default configuration with all layers
        let layers = vec![
            LayerConfig::new(LayerType::QueryProcessor),
            LayerConfig::new(LayerType::ContextGenerator),
            LayerConfig::new(LayerType::Developer),
            LayerConfig::new(LayerType::Summarizer),
            LayerConfig::new(LayerType::NextRequest),
            LayerConfig::new(LayerType::SessionReviewer),
        ];
        
        Self { layers }
    }
    
    // Create from config
    pub fn from_config(config: &Config) -> Self {
        // Check if the config has layer configurations
        let mut layers = Vec::new();
        
        // Create query processor
        let mut qp = LayerConfig::new(LayerType::QueryProcessor);
        if let Some(model) = &config.openrouter.query_processor_model {
            qp.model = model.clone();
        }
        layers.push(qp);
        
        // Create context generator
        let mut cg = LayerConfig::new(LayerType::ContextGenerator);
        if let Some(model) = &config.openrouter.context_generator_model {
            cg.model = model.clone();
        }
        layers.push(cg);
        
        // Create developer
        let mut dev = LayerConfig::new(LayerType::Developer);
        if let Some(model) = &config.openrouter.developer_model {
            dev.model = model.clone();
        } else {
            // Use the main model if no specific developer model
            dev.model = config.openrouter.model.clone();
        }
        layers.push(dev);
        
        // Create summarizer
        let mut summ = LayerConfig::new(LayerType::Summarizer);
        if let Some(model) = &config.openrouter.summarizer_model {
            summ.model = model.clone();
        }
        layers.push(summ);
        
        // Create next request
        let mut next = LayerConfig::new(LayerType::NextRequest);
        if let Some(model) = &config.openrouter.next_request_model {
            next.model = model.clone();
        }
        layers.push(next);
        
        // Create session reviewer
        let mut rev = LayerConfig::new(LayerType::SessionReviewer);
        if let Some(model) = &config.openrouter.session_reviewer_model {
            rev.model = model.clone();
        }
        layers.push(rev);
        
        Self { layers }
    }
    
    // Process through all enabled layers
    pub async fn process(
        &self,
        input: &str,
        session: &mut Session,
        config: &Config,
        operation_cancelled: Arc<AtomicBool>
    ) -> Result<String> {
        let mut current_input = input.to_string();
        let mut final_output = String::new();
        
        // Process through each enabled layer
        for layer_config in &self.layers {
            if !layer_config.enabled {
                continue;
            }
            
            // Skip if operation cancelled
            if operation_cancelled.load(Ordering::SeqCst) {
                return Err(anyhow::anyhow!("Operation cancelled"));
            }
            
            // Create and process layer
            let processor = LayerProcessor::new(layer_config.clone());
            let (output, _exchange) = processor.process(
                &current_input, 
                session, 
                config,
                operation_cancelled.clone()
            ).await?;
            
            // For the Developer layer, this is our final user-facing output
            if layer_config.layer_type == LayerType::Developer {
                final_output = output.clone();
            }
            
            // Store result in session if needed
            // For now we're not storing intermediate layers in the session
            
            // Update input for next layer
            current_input = output;
        }
        
        Ok(final_output)
    }
}