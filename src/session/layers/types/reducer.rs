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
			model: Some("openrouter:openai/o4-mini".to_string()),
			system_prompt: None, // Use built-in prompt
			temperature: 0.2,
			input_mode: crate::session::layers::layer_trait::InputMode::Summary, // Use summarized data
			mcp: crate::session::layers::layer_trait::LayerMcpConfig {
				server_refs: vec![],
				allowed_tools: vec![],
			},
			parameters: std::collections::HashMap::new(),
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
		operation_cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
	) -> anyhow::Result<LayerResult> {
		// Process using the base processor
		self.processor
			.process(input, session, config, operation_cancelled)
			.await
	}
}
