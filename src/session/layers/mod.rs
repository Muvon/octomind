pub mod layer_trait;
pub mod processor;
pub mod types;
pub mod orchestrator;

pub use layer_trait::{Layer, LayerConfig, LayerResult, InputMode};
pub use processor::LayerProcessor;
pub use orchestrator::LayeredOrchestrator;

// Main function to process using the layered architecture
pub async fn process_with_layers(
	input: &str,
	session: &mut crate::session::Session,
	config: &crate::config::Config,
	operation_cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>
) -> anyhow::Result<String> {
	let orchestrator = LayeredOrchestrator::from_config(config);
	orchestrator.process(input, session, config, operation_cancelled).await
}
