// Copyright 2026 Muvon Un Limited
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

use super::layer_trait::{Layer, LayerConfig, LayerResult};
use crate::mcp::agent::functions::run_acp_command;
use crate::session::Session;
use anyhow::Result;
use async_trait::async_trait;

// Base processor that handles common functionality for all layers
pub struct LayerProcessor {
	pub config: LayerConfig,
}

impl LayerProcessor {
	pub fn new(config: LayerConfig) -> Self {
		Self { config }
	}
}

// Async implementation of the Layer trait for LayerProcessor
#[async_trait]
impl Layer for LayerProcessor {
	fn name(&self) -> &str {
		&self.config.name
	}

	fn config(&self) -> &LayerConfig {
		&self.config
	}

	async fn process(
		&self,
		input: &str,
		session: &Session,
		_config: &crate::config::Config,
		operation_cancelled: tokio::sync::watch::Receiver<bool>,
	) -> Result<LayerResult> {
		// Prepare input based on input_mode
		let task = self.prepare_input(input, session);

		// Resolve workdir relative to session's working directory
		let session_workdir = crate::mcp::get_thread_working_directory();
		let workdir = self.config.get_resolved_workdir(&session_workdir);

		// Execute via ACP protocol
		let start = std::time::Instant::now();
		let output =
			run_acp_command(&self.config.command, &task, &workdir, operation_cancelled).await?;

		// Return result with timing info
		// Note: exchange/token_usage come from the ACP session's role config
		Ok(LayerResult {
			outputs: vec![output],
			total_time_ms: start.elapsed().as_millis() as u64,
		})
	}
}
