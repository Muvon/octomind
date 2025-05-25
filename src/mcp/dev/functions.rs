// Function definitions for the Developer MCP provider

use super::super::McpFunction;
use super::shell::get_shell_function;
use super::semantic::get_semantic_code_function;
use super::graphrag::get_graphrag_function;

// Get all available developer functions
pub fn get_all_functions() -> Vec<McpFunction> {
	let mut functions = vec![
		get_shell_function(),
		get_semantic_code_function(),
	];

	// Only add GraphRAG function if the feature is enabled in the config
	let config = crate::config::Config::load().unwrap_or_default();
	if config.graphrag.enabled {
		functions.push(get_graphrag_function());
	}

	functions
}