use crate::session::layers::layer_trait::{Layer, LayerConfig, LayerResult};
use crate::session::layers::processor::LayerProcessor;
use async_trait::async_trait;

// ContextGenerator layer that gathers and injects context information
pub struct ContextGeneratorLayer {
	processor: LayerProcessor,
}

impl ContextGeneratorLayer {
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
			model: Some("openrouter:google/gemini-2.5-flash-preview".to_string()),
			system_prompt: None, // Use built-in prompt
			temperature: 0.2,
			input_mode: crate::session::layers::layer_trait::InputMode::Last,
			mcp: crate::session::layers::layer_trait::LayerMcpConfig { 
				enabled: true, 
				servers: vec!["core".to_string()], 
				allowed_tools: vec!["text_editor".to_string(), "semantic_code".to_string()]
			},
			parameters: std::collections::HashMap::new(),
		}
	}
}

#[async_trait]
impl Layer for ContextGeneratorLayer {
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
