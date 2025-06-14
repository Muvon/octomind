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

pub mod layer_trait;
pub mod orchestrator;
pub mod processor;
pub mod types; // Keep for backward compatibility

pub use layer_trait::{InputMode, Layer, LayerConfig, LayerMcpConfig, LayerResult, OutputMode};
pub use orchestrator::LayeredOrchestrator;
pub use processor::LayerProcessor;
pub use types::GenericLayer;

// Main function to process using the layered architecture
pub async fn process_with_layers(
	input: &str,
	session: &mut crate::session::Session,
	config: &crate::config::Config,
	role: &str,
	operation_cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> anyhow::Result<String> {
	let orchestrator = LayeredOrchestrator::from_config(config, role);
	orchestrator
		.process(input, session, config, operation_cancelled)
		.await
}
