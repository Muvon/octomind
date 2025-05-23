use crate::session::layers::layer_trait::{Layer, LayerConfig, LayerResult};
use crate::session::layers::processor::LayerProcessor;
use async_trait::async_trait;

// QueryProcessor layer that improves the initial query for better instructions
pub struct QueryProcessorLayer {
	processor: LayerProcessor,
}

impl QueryProcessorLayer {
	pub fn new(config: LayerConfig) -> Self {
		Self {
			processor: LayerProcessor::new(config),
		}
	}

	// Default configuration with reasonable defaults
	pub fn default_config(name: &str) -> LayerConfig {
		LayerConfig {
			name: name.to_string(),
			enabled: true,
			model: "openrouter:openai/gpt-4.1-nano".to_string(),
			system_prompt: crate::session::helper_functions::get_raw_system_prompt("query_processor"),
			temperature: 0.2,
			enable_tools: false, // No tools for QueryProcessor
			allowed_tools: Vec::new(),
			input_mode: crate::session::layers::layer_trait::InputMode::Last,
		}
	}
}

#[async_trait]
impl Layer for QueryProcessorLayer {
	fn name(&self) -> &str {
		self.processor.name()
	}

	fn config(&self) -> &LayerConfig {
		self.processor.config()
	}

	async fn process(
		&self,
		input: &str,
		session: &crate::session::Session,
		config: &crate::config::Config,
		operation_cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>
	) -> anyhow::Result<LayerResult> {
		// Process using the base processor
		self.processor.process(input, session, config, operation_cancelled).await
	}
}
