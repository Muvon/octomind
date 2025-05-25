// Function definitions for the Developer MCP provider

use super::super::McpFunction;
use super::shell::get_shell_function;

// Get all available developer functions
pub fn get_all_functions() -> Vec<McpFunction> {
	vec![
		get_shell_function(),
	]
}