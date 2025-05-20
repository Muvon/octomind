// Base layer trait and types

pub mod processor;
pub mod orchestrator;

use crate::config::Config;
use crate::session::{Session, openrouter};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use async_trait::async_trait;

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
            LayerType::Summarizer => "Summarizes the task results and updates documentation",
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
            system_prompt: super::get_layer_system_prompt(layer_type),
            temperature: 0.7,
        }
    }
}

// Result from a layer's processing
pub struct LayerResult {
    pub output: String,
    pub exchange: openrouter::OpenRouterExchange,
    pub token_usage: Option<openrouter::TokenUsage>,
}

// Trait that all layers must implement
#[async_trait]
pub trait Layer {
    fn get_type(&self) -> LayerType;
    fn get_config(&self) -> &LayerConfig;
    
    async fn process(
        &self, 
        input: &str,
        session: &Session,
        config: &Config,
        operation_cancelled: Arc<AtomicBool>
    ) -> Result<LayerResult>;
}

// Main function to process using the layered architecture
pub async fn process_with_layers(
    input: &str,
    session: &mut Session,
    config: &Config,
    operation_cancelled: Arc<AtomicBool>
) -> Result<String> {
    use crate::session::layers::orchestrator::LayeredOrchestrator;
    let orchestrator = LayeredOrchestrator::from_config(config);
    orchestrator.process(input, session, config, operation_cancelled).await
}