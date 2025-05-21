use crate::session::layers::layer_trait::{Layer, LayerConfig, LayerResult};
use crate::session::layers::processor::LayerProcessor;
use async_trait::async_trait;

// Developer layer that executes the actual development work
pub struct DeveloperLayer {
	processor: LayerProcessor,
}

impl DeveloperLayer {
	pub fn new(config: LayerConfig) -> Self {
		Self {
			processor: LayerProcessor::new(config),
		}
	}

	// Default configuration with reasonable defaults
	pub fn default_config(name: &str, default_model: &str) -> LayerConfig {
		LayerConfig {
			name: name.to_string(),
			enabled: true,
			model: default_model.to_string(),
			system_prompt: crate::session::helper_functions::get_raw_system_prompt("developer"),
			temperature: 0.2,
			enable_tools: true, // Enable tools for main development
			allowed_tools: Vec::new(), // All tools available
			input_mode: crate::session::layers::layer_trait::InputMode::All, // Get all context from previous layer
		}
	}
}

#[async_trait]
impl Layer for DeveloperLayer {
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
