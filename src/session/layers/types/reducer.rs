use crate::session::layers::layer_trait::{Layer, LayerConfig, LayerResult};
use crate::session::layers::processor::LayerProcessor;
use async_trait::async_trait;

// Reducer layer that optimizes context and documentation for next interaction
pub struct ReducerLayer {
	processor: LayerProcessor,
}

impl ReducerLayer {
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
			model: "openai/o4-mini".to_string(),
			system_prompt: crate::session::helper_functions::get_raw_system_prompt("reducer"),
			temperature: 0.2,
			enable_tools: false, // No tools for Reducer
			allowed_tools: Vec::new(),
			input_mode: crate::session::layers::layer_trait::InputMode::Summary, // Use summarized data
		}
	}
}

#[async_trait]
impl Layer for ReducerLayer {
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
