// Copyright 2025 Muvon Un Limited
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

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
			model: Some("openrouter:google/gemini-2.5-flash-preview".to_string()),
			system_prompt: None, // Use built-in prompt
			temperature: 0.2,
			input_mode: crate::session::layers::layer_trait::InputMode::Last,
			mcp: crate::session::layers::layer_trait::LayerMcpConfig {
				server_refs: vec!["developer".to_string(), "filesystem".to_string()],
				allowed_tools: vec!["text_editor".to_string(), "list_files".to_string()],
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
		operation_cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
	) -> anyhow::Result<LayerResult> {
		// Process using the base processor
		self.processor
			.process(input, session, config, operation_cancelled)
			.await
	}
}
