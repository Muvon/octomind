use crate::config::Config;
use crate::session::{Session, openrouter};
use anyhow::Result;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

// Layer result that contains data returned from a layer's processing
pub struct LayerResult {
	pub output: String,
	pub exchange: openrouter::OpenRouterExchange,
	pub token_usage: Option<openrouter::TokenUsage>,
}

// Input mode determines what part of the previous layer's output will be used
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputMode {
	Last,    // Only the last message from the previous layer
	All,     // All messages/data from the previous layer
	Summary, // A summarized version of all data from the previous layer
}

impl Default for InputMode {
	fn default() -> Self {
		Self::Last
	}
}

impl InputMode {
	pub fn as_str(&self) -> &'static str {
		match self {
			InputMode::Last => "last",
			InputMode::All => "all",
			InputMode::Summary => "summary",
		}
	}

	pub fn from_str(s: &str) -> Self {
		match s.to_lowercase().as_str() {
			"last" => InputMode::Last,
			"all" => InputMode::All,
			"summary" => InputMode::Summary,
			_ => InputMode::Last, // Default to Last if unknown
		}
	}
}

// Common configuration properties for all layers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayerConfig {
	pub name: String,
	pub enabled: bool,
	pub model: String,
	pub system_prompt: String,
	pub temperature: f32,
	pub enable_tools: bool,
	pub allowed_tools: Vec<String>, // Empty means all tools are allowed
	pub input_mode: InputMode,
}

// Trait that all layers must implement
#[async_trait]
pub trait Layer {
	fn name(&self) -> &str;
	fn config(&self) -> &LayerConfig;

	async fn process(
		&self,
		input: &str,
		session: &Session,
		config: &Config,
		operation_cancelled: Arc<AtomicBool>
	) -> Result<LayerResult>;

	// Helper function to prepare input based on input_mode
	fn prepare_input(&self, input: &str, session: &Session) -> String {
		match self.config().input_mode {
			InputMode::Last => input.to_string(),
			InputMode::All => {
				// For "all" mode, we'd include session context and history
				// This is a simplified example, actual implementation may be more complex
				let history = session.messages.iter()
					.filter(|m| m.role == "assistant")
					.map(|m| m.content.clone())
					.collect::<Vec<_>>()
					.join("\n\n");

				format!("Task:\n{}\n\nContext:\n{}", input, history)
			},
			InputMode::Summary => {
				// For summary mode, we'd use some logic to summarize previous context
				// This would typically call a function to generate a summary
				crate::session::summarize_context(session, input)
			}
		}
	}
}
