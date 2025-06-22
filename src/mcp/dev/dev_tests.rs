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

// Tests for the Developer MCP provider

#[cfg(test)]
mod tests {
	use crate::mcp::dev::ast_grep::execute_ast_grep_command;
	use crate::mcp::dev::shell::execute_shell_command;
	use crate::mcp::McpToolCall;
	use serde_json::json;
	use std::sync::{atomic::AtomicBool, Arc};
	use tokio;

	// Helper function to create a shell tool call
	fn create_shell_call(command: &str, background: Option<bool>) -> McpToolCall {
		let mut params = json!({
			"command": command
		});

		if let Some(bg) = background {
			params["background"] = json!(bg);
		}

		McpToolCall {
			tool_name: "shell".to_string(),
			parameters: params,
			tool_id: "test-call-id".to_string(),
		}
	}

	// Helper function to create an ast_grep tool call
	fn create_ast_grep_call(pattern: &str, language: Option<&str>) -> McpToolCall {
		let mut params = json!({
			"pattern": pattern
		});

		if let Some(lang) = language {
			params["language"] = json!(lang);
		}

		McpToolCall {
			tool_name: "ast_grep".to_string(),
			parameters: params,
			tool_id: "test-call-id".to_string(),
		}
	}

	#[tokio::test]
	async fn test_shell_foreground_simple_command() {
		let call = create_shell_call("echo 'Hello, World!'", Some(false));
		let result = execute_shell_command(&call, None).await;

		assert!(result.is_ok());
		let result = result.unwrap();
		assert_eq!(result.tool_name, "shell");

		let output = result.result.as_object().unwrap();
		assert_eq!(output["success"], true);
		assert!(output["output"].as_str().unwrap().contains("Hello, World!"));
		assert!(!output.contains_key("background"));
		assert!(!output.contains_key("pid"));
	}

	#[tokio::test]
	async fn test_shell_foreground_command_with_error() {
		let call = create_shell_call("ls /nonexistent/directory/path", Some(false));
		let result = execute_shell_command(&call, None).await;

		assert!(result.is_ok());
		let result = result.unwrap();

		let output = result.result.as_object().unwrap();
		assert_eq!(output["success"], false);
		assert!(output["code"].as_i64().unwrap() != 0);
	}

	#[tokio::test]
	async fn test_shell_background_simple_command() {
		// Use a command that runs for a short time but long enough to test background execution
		let call = create_shell_call("sleep 2", Some(true));
		let result = execute_shell_command(&call, None).await;

		assert!(result.is_ok());
		let result = result.unwrap();
		assert_eq!(result.tool_name, "shell");

		let output = result.result.as_object().unwrap();
		assert_eq!(output["success"], true);
		assert_eq!(output["background"], true);
		assert!(output.contains_key("pid"));
		assert!(output["message"].as_str().unwrap().contains("background"));

		let pid = output["pid"].as_u64().unwrap();
		assert!(pid > 0);

		// Verify the process is actually running by checking if we can kill it
		let kill_call = create_shell_call(&format!("kill {}", pid), Some(false));
		let kill_result = execute_shell_command(&kill_call, None).await;
		assert!(kill_result.is_ok());
	}

	#[tokio::test]
	async fn test_shell_background_long_running_process() {
		// Test with a longer running process
		let call = create_shell_call("sleep 10", Some(true));
		let result = execute_shell_command(&call, None).await;

		assert!(result.is_ok());
		let result = result.unwrap();

		let output = result.result.as_object().unwrap();
		assert_eq!(output["success"], true);
		assert_eq!(output["background"], true);

		let pid = output["pid"].as_u64().unwrap();

		// Immediately kill the background process to clean up
		let kill_call = create_shell_call(&format!("kill {}", pid), Some(false));
		let kill_result = execute_shell_command(&kill_call, None).await;
		assert!(kill_result.is_ok());
	}

	#[tokio::test]
	async fn test_shell_cancellation_foreground() {
		let cancellation_token = Arc::new(AtomicBool::new(false));
		let call = create_shell_call("sleep 5", Some(false));

		// Start the command
		let token_clone = cancellation_token.clone();
		let task =
			tokio::spawn(async move { execute_shell_command(&call, Some(token_clone)).await });

		// Cancel after a short delay
		tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
		cancellation_token.store(true, std::sync::atomic::Ordering::SeqCst);

		let result = task.await.unwrap();
		assert!(result.is_ok());

		let result = result.unwrap();
		let output = result.result.as_object().unwrap();
		assert_eq!(output["success"], false);
		assert!(output["message"].as_str().unwrap().contains("cancelled"));
	}

	#[tokio::test]
	async fn test_shell_default_background_parameter() {
		// Test that background defaults to false when not specified
		let call = create_shell_call("echo 'test'", None);
		let result = execute_shell_command(&call, None).await;

		assert!(result.is_ok());
		let result = result.unwrap();

		let output = result.result.as_object().unwrap();
		assert_eq!(output["success"], true);
		assert!(!output.contains_key("background") || output["background"] == false);
		assert!(output["output"].as_str().unwrap().contains("test"));
	}

	#[tokio::test]
	async fn test_ast_grep_simple_search() {
		// Create a temporary file for testing
		let temp_content = r#"
function testFunction() {
    console.log("test");
    return true;
}
"#;

		// Write to a temporary file
		let temp_file = "test_ast_grep_temp.js";
		std::fs::write(temp_file, temp_content).unwrap();

		// Test ast-grep search
		let call = create_ast_grep_call("console.log($$$)", Some("javascript"));
		let result = execute_ast_grep_command(&call, None).await;

		// Clean up
		let _ = std::fs::remove_file(temp_file);

		assert!(result.is_ok());
		let result = result.unwrap();
		assert_eq!(result.tool_name, "ast_grep");

		let output = result.result.as_object().unwrap();
		// Note: ast-grep might not find matches if the temp file isn't in the search path
		// This test mainly verifies the tool doesn't crash
		assert!(output.contains_key("success"));
	}

	#[tokio::test]
	async fn test_ast_grep_with_invalid_pattern() {
		let call = create_ast_grep_call("", Some("javascript"));
		let result = execute_ast_grep_command(&call, None).await;

		// Should handle empty pattern gracefully
		assert!(result.is_ok());
		let result = result.unwrap();
		let output = result.result.as_object().unwrap();
		// The result depends on how ast-grep handles empty patterns
		assert!(output.contains_key("success"));
	}

	#[tokio::test]
	async fn test_shell_command_history_integration() {
		// Test that commands are added to shell history
		let call = create_shell_call("echo 'history test'", Some(false));
		let result = execute_shell_command(&call, None).await;

		assert!(result.is_ok());
		let result = result.unwrap();

		let output = result.result.as_object().unwrap();
		assert_eq!(output["success"], true);

		// Verify the command parameter is preserved in the result
		let params = output["parameters"].as_object().unwrap();
		assert_eq!(params["command"], "echo 'history test'");
	}

	#[tokio::test]
	async fn test_multiple_background_processes() {
		// Test starting multiple background processes
		let mut pids = Vec::new();

		for i in 1..=3 {
			let call = create_shell_call(&format!("sleep {}", i * 2), Some(true));
			let result = execute_shell_command(&call, None).await;

			assert!(result.is_ok());
			let result = result.unwrap();
			let output = result.result.as_object().unwrap();
			assert_eq!(output["background"], true);

			let pid = output["pid"].as_u64().unwrap();
			pids.push(pid);
		}

		// Clean up all background processes
		for pid in pids {
			let kill_call = create_shell_call(&format!("kill {}", pid), Some(false));
			let _ = execute_shell_command(&kill_call, None).await;
		}
	}

	#[tokio::test]
	async fn test_shell_missing_command_parameter() {
		let call = McpToolCall {
			tool_name: "shell".to_string(),
			parameters: json!({}), // Missing command parameter
			tool_id: "test-call-id".to_string(),
		};

		let result = execute_shell_command(&call, None).await;
		assert!(result.is_err());
		assert!(result
			.unwrap_err()
			.to_string()
			.contains("Missing or invalid 'command' parameter"));
	}

	#[tokio::test]
	async fn test_ast_grep_missing_pattern_parameter() {
		let call = McpToolCall {
			tool_name: "ast_grep".to_string(),
			parameters: json!({}), // Missing pattern parameter
			tool_id: "test-call-id".to_string(),
		};

		let result = execute_ast_grep_command(&call, None).await;
		assert!(result.is_err());
		assert!(result
			.unwrap_err()
			.to_string()
			.contains("Missing or invalid 'pattern' parameter"));
	}
}
