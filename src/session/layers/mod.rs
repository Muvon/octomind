pub mod layer_trait;
pub mod generic_layer;  // New generic layer implementation
pub mod processor;      // Keep for backward compatibility
pub mod types;          // Keep existing types for backward compatibility
pub mod orchestrator;

pub use layer_trait::{Layer, LayerConfig, LayerResult, InputMode, LayerMcpConfig};
pub use generic_layer::GenericLayer;
pub use processor::LayerProcessor;
pub use orchestrator::LayeredOrchestrator;

// Main function to process using the layered architecture
pub async fn process_with_layers(
	input: &str,
	session: &mut crate::session::Session,
	config: &crate::config::Config,
	role: &str,
	operation_cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>
) -> anyhow::Result<String> {
	let orchestrator = LayeredOrchestrator::from_config(config, role);
	orchestrator.process(input, session, config, operation_cancelled).await
}
