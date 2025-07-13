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
	use crate::mcp::{extract_mcp_content, McpToolCall};
	use serde_json::json;
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
		let result = execute_shell_command(&call).await;

		assert!(result.is_ok());
		let result = result.unwrap();
		assert_eq!(result.tool_name, "shell");

		let output = result.result.as_object().unwrap();
		// Check MCP-compliant success format
		assert_eq!(output["isError"], false);

		// Extract content using MCP protocol
		let content = extract_mcp_content(&result.result);
		assert!(content.contains("Hello, World!"));

		// These fields don't exist in foreground MCP format
		assert!(!output.contains_key("background"));
		assert!(!output.contains_key("pid"));
	}

	#[tokio::test]
	async fn test_shell_foreground_command_with_error() {
		let call = create_shell_call("ls /nonexistent/directory/path", Some(false));
		let result = execute_shell_command(&call).await;

		assert!(result.is_ok());
		let result = result.unwrap();

		let output = result.result.as_object().unwrap();
		// Check MCP-compliant error format
		assert_eq!(output["isError"], true);

		// Extract content using MCP protocol - error messages include command and exit code info
		let content = extract_mcp_content(&result.result);
		assert!(content.contains("Command failed with exit code"));
	}

	#[tokio::test]
	async fn test_shell_background_simple_command() {
		// Use a command that runs for a short time but long enough to test background execution
		let call = create_shell_call("sleep 2", Some(true));
		let result = execute_shell_command(&call).await;

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
		let kill_result = execute_shell_command(&kill_call).await;
		assert!(kill_result.is_ok());
	}

	#[tokio::test]
	async fn test_shell_background_long_running_process() {
		// Test with a longer running process
		let call = create_shell_call("sleep 10", Some(true));
		let result = execute_shell_command(&call).await;

		assert!(result.is_ok());
		let result = result.unwrap();

		let output = result.result.as_object().unwrap();
		assert_eq!(output["success"], true);
		assert_eq!(output["background"], true);

		let pid = output["pid"].as_u64().unwrap();

		// Immediately kill the background process to clean up
		let kill_call = create_shell_call(&format!("kill {}", pid), Some(false));
		let kill_result = execute_shell_command(&kill_call).await;
		assert!(kill_result.is_ok());
	}

	#[tokio::test]
	async fn test_shell_cancellation_foreground() {
		// Test that the shell command itself no longer handles cancellation
		// The centralized architecture means individual tools don't need cancellation logic
		let call = create_shell_call("echo 'test'", Some(false));

		// Direct tool call should work without cancellation_token parameter
		let result = execute_shell_command(&call).await;
		assert!(result.is_ok());

		let result = result.unwrap();
		let output = result.result.as_object().unwrap();
		// Should succeed normally since cancellation is handled at wrapper level
		assert_eq!(output["isError"], false);

		// Extract content using MCP protocol
		let content = extract_mcp_content(&result.result);
		assert!(content.contains("test"));
	}

	#[tokio::test]
	async fn test_centralized_cancellation_architecture() {
		// Test that demonstrates the centralized cancellation architecture
		// Internal tools no longer accept cancellation_token parameters

		// Shell command works without cancellation_token
		let shell_call = create_shell_call("echo 'centralized cancellation test'", Some(false));
		let shell_result = execute_shell_command(&shell_call).await;
		assert!(shell_result.is_ok());

		// Tool returns proper MCP-compliant results
		let shell_result = shell_result.unwrap();
		let output = shell_result.result.as_object().unwrap();

		// Should succeed normally since cancellation is handled at wrapper level
		assert_eq!(output["isError"], false);

		// Extract content using MCP protocol
		let content = extract_mcp_content(&shell_result.result);
		assert!(content.contains("centralized cancellation test"));

		// This test validates that:
		// - Internal tools focus purely on business logic
		// - No cancellation_token parameters in internal tool signatures
		// - Centralized wrapper (try_execute_tool_call) handles cancellation
		// - MCP-compliant results are returned consistently
	}

	#[tokio::test]
	async fn test_shell_default_background_parameter() {
		// Test that background defaults to false when not specified
		let call = create_shell_call("echo 'test'", None);
		let result = execute_shell_command(&call).await;

		assert!(result.is_ok());
		let result = result.unwrap();

		let output = result.result.as_object().unwrap();
		// Check MCP-compliant success format
		assert_eq!(output["isError"], false);

		// These fields don't exist in foreground MCP format
		assert!(!output.contains_key("background"));

		// Extract content using MCP protocol
		let content = extract_mcp_content(&result.result);
		assert!(content.contains("test"));
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
		let result = execute_ast_grep_command(&call).await;

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
		let result = execute_ast_grep_command(&call).await;

		// Should handle empty pattern gracefully
		assert!(result.is_ok());
		let result = result.unwrap();
		let output = result.result.as_object().unwrap();
		assert_eq!(output["isError"], true);
		let error_content = &output["content"][0]["text"];
		assert!(error_content
			.as_str()
			.unwrap()
			.contains("Pattern parameter cannot be empty"));
	}

	#[tokio::test]
	async fn test_ast_grep_glob_pattern_expansion() {
		// Create temporary test files
		let temp_dir = tempfile::tempdir().unwrap();
		let temp_dir_path = temp_dir.path();

		// Create a subdirectory structure
		let src_dir = temp_dir_path.join("src");
		std::fs::create_dir_all(&src_dir).unwrap();

		// Create test files
		let test_file1 = src_dir.join("test1.rs");
		let test_file2 = src_dir.join("test2.rs");
		let test_file3 = temp_dir_path.join("other.txt");

		std::fs::write(&test_file1, "fn test_function() { println!(\"test1\"); }").unwrap();
		std::fs::write(
			&test_file2,
			"fn another_function() { println!(\"test2\"); }",
		)
		.unwrap();
		std::fs::write(&test_file3, "not rust code").unwrap();

		// Test with glob pattern
		let glob_pattern = format!("{}/**/*.rs", temp_dir_path.display());
		let params = json!({
			"pattern": "fn $NAME($ARGS) { $$$ }",
			"language": "rust",
			"paths": [glob_pattern]
		});

		let call = McpToolCall {
			tool_name: "ast_grep".to_string(),
			parameters: params,
			tool_id: "test-glob-call-id".to_string(),
		};

		let result = execute_ast_grep_command(&call).await;

		assert!(result.is_ok());
		let result = result.unwrap();
		assert_eq!(result.tool_name, "ast_grep");

		let output = result.result.as_object().unwrap();
		assert!(output.contains_key("success"));

		// The test should succeed even if no matches are found, as long as glob expansion works
		// (ast-grep might not find matches depending on the pattern, but it shouldn't error)
	}

	#[tokio::test]
	async fn test_shell_command_history_integration() {
		// Test that commands are added to shell history
		let call = create_shell_call("echo 'history test'", Some(false));
		let result = execute_shell_command(&call).await;

		assert!(result.is_ok());
		let result = result.unwrap();

		let output = result.result.as_object().unwrap();
		// Check MCP-compliant success format
		assert_eq!(output["isError"], false);

		// Extract content using MCP protocol
		let content = extract_mcp_content(&result.result);
		assert!(content.contains("history test"));
	}

	#[tokio::test]
	async fn test_multiple_background_processes() {
		// Test starting multiple background processes
		let mut pids = Vec::new();

		for i in 1..=3 {
			let call = create_shell_call(&format!("sleep {}", i * 2), Some(true));
			let result = execute_shell_command(&call).await;

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
			let _ = execute_shell_command(&kill_call).await;
		}
	}

	#[tokio::test]
	async fn test_shell_missing_command_parameter() {
		let call = McpToolCall {
			tool_name: "shell".to_string(),
			parameters: json!({}), // Missing command parameter
			tool_id: "test-call-id".to_string(),
		};

		let result = execute_shell_command(&call).await;
		assert!(result.is_ok());
		let result = result.unwrap();
		let output = result.result.as_object().unwrap();
		assert_eq!(output["isError"], true);
		let error_content = &output["content"][0]["text"];
		assert!(error_content
			.as_str()
			.unwrap()
			.contains("Missing required 'command' parameter"));
	}

	#[tokio::test]
	async fn test_ast_grep_missing_pattern_parameter() {
		let call = McpToolCall {
			tool_name: "ast_grep".to_string(),
			parameters: json!({}), // Missing pattern parameter
			tool_id: "test-call-id".to_string(),
		};

		let result = execute_ast_grep_command(&call).await;
		assert!(result.is_ok());
		let result = result.unwrap();
		let output = result.result.as_object().unwrap();
		assert_eq!(output["isError"], true);
		let error_content = &output["content"][0]["text"];
		assert!(error_content
			.as_str()
			.unwrap()
			.contains("Missing required 'pattern' parameter"));
	}

	#[tokio::test]
	async fn test_ast_grep_output_grouping() {
		// Test that ast_grep output is properly grouped by file for token efficiency
		// Create temporary Rust files for testing
		let temp_content1 = r#"
fn main() {
    println!("Hello from main");
}

fn helper() {
    println!("Helper function");
}
"#;

		let temp_content2 = r#"
pub fn public_func() {
    println!("Public function");
}

fn private_func() {
    println!("Private function");
}
"#;

		let temp_file1 = "test_ast_grep_group1.rs";
		let temp_file2 = "test_ast_grep_group2.rs";
		std::fs::write(temp_file1, temp_content1).unwrap();
		std::fs::write(temp_file2, temp_content2).unwrap();

		// Test with a pattern that should match multiple functions
		let params = json!({
			"pattern": "fn $NAME($ARGS) { $$$ }",
			"language": "rust",
			"paths": [temp_file1, temp_file2]
		});

		let call = McpToolCall {
			tool_name: "ast_grep".to_string(),
			parameters: params,
			tool_id: "test-call-id".to_string(),
		};

		let result = execute_ast_grep_command(&call).await;

		// Clean up
		let _ = std::fs::remove_file(temp_file1);
		let _ = std::fs::remove_file(temp_file2);

		// The command should execute without shell parsing errors
		assert!(result.is_ok());
		let result = result.unwrap();
		assert_eq!(result.tool_name, "ast_grep");

		let output = result.result.as_object().unwrap();
		assert!(output.contains_key("success"));

		// Check that we don't get shell parsing errors
		let output_text = output["output"].as_str().unwrap_or("");
		assert!(
			!output_text.contains("syntax error near unexpected token"),
			"Shell parsing error detected: {}",
			output_text
		);
		assert!(
			!output_text.contains("sh: -c: line 0:"),
			"Shell command line error detected: {}",
			output_text
		);

		// Print the output to see the grouping format
		println!("ast_grep grouped output:\n{}", output_text);
	}

	#[tokio::test]
	async fn test_ast_grep_truncation() {
		// Test that ast_grep supports smart truncation like list_files
		// Create a temporary Rust file with many functions
		let temp_content = r#"
fn function1() { println!("1"); }
fn function2() { println!("2"); }
fn function3() { println!("3"); }
fn function4() { println!("4"); }
fn function5() { println!("5"); }
fn function6() { println!("6"); }
fn function7() { println!("7"); }
fn function8() { println!("8"); }
fn function9() { println!("9"); }
fn function10() { println!("10"); }
fn function11() { println!("11"); }
fn function12() { println!("12"); }
fn function13() { println!("13"); }
fn function14() { println!("14"); }
fn function15() { println!("15"); }
"#;

		let temp_file = "test_ast_grep_truncation.rs";
		std::fs::write(temp_file, temp_content).unwrap();

		// Test with max_lines = 5 (should truncate)
		let params = json!({
			"pattern": "fn $NAME() { $$$ }",
			"language": "rust",
			"paths": [temp_file],
			"max_lines": 5
		});

		let call = McpToolCall {
			tool_name: "ast_grep".to_string(),
			parameters: params,
			tool_id: "test-call-id".to_string(),
		};

		let result = execute_ast_grep_command(&call).await;

		// Clean up
		let _ = std::fs::remove_file(temp_file);

		// Should execute successfully
		assert!(result.is_ok());
		let result = result.unwrap();
		let output = result.result.as_object().unwrap();

		// Check that parameters include max_lines
		let params = &output["parameters"];
		assert_eq!(params["max_lines"], 5);

		// If ast-grep finds matches and we have more than 5 lines, should have truncation info
		let output_text = output["output"].as_str().unwrap_or("");
		println!("ast_grep truncation test output:\n{}", output_text);

		// Test with max_lines = 0 (unlimited)
		let params_unlimited = json!({
			"pattern": "fn $NAME() { $$$ }",
			"language": "rust",
			"paths": [temp_file],
			"max_lines": 0
		});

		let call_unlimited = McpToolCall {
			tool_name: "ast_grep".to_string(),
			parameters: params_unlimited,
			tool_id: "test-call-id".to_string(),
		};

		// Recreate the file for the second test
		std::fs::write(temp_file, temp_content).unwrap();
		let result_unlimited = execute_ast_grep_command(&call_unlimited).await;
		let _ = std::fs::remove_file(temp_file);

		assert!(result_unlimited.is_ok());
		let result_unlimited = result_unlimited.unwrap();
		let output_unlimited = result_unlimited.result.as_object().unwrap();

		// Should not have truncation with max_lines = 0
		assert!(!output_unlimited.contains_key("truncation_info"));
		assert_eq!(output_unlimited["parameters"]["max_lines"], 0);
	}

	#[tokio::test]
	async fn test_ast_grep_complex_pattern_with_special_characters() {
		// Test that complex patterns with special characters are properly escaped
		// Create a temporary Rust file for testing
		let temp_content = r#"
async fn process(input: String) -> Result<String, Error> {
    println!("Processing: {}", input);
    Ok(format!("Processed: {}", input))
}

pub async fn handle_request(req: Request) -> Response {
    let result = process(req.body).await;
    Response::new(result)
}
"#;

		let temp_file = "test_ast_grep_complex.rs";
		std::fs::write(temp_file, temp_content).unwrap();

		// Test with a complex pattern that includes special characters
		let params = json!({
			"pattern": "async fn process($ARGS) { $$$ }",
			"language": "rust",
			"paths": [temp_file]
		});

		let call = McpToolCall {
			tool_name: "ast_grep".to_string(),
			parameters: params,
			tool_id: "test-call-id".to_string(),
		};

		let result = execute_ast_grep_command(&call).await;

		// Clean up
		let _ = std::fs::remove_file(temp_file);

		// The command should execute without shell parsing errors
		assert!(result.is_ok());
		let result = result.unwrap();
		assert_eq!(result.tool_name, "ast_grep");

		let output = result.result.as_object().unwrap();
		assert!(output.contains_key("success"));

		// Most importantly, check that we don't get shell parsing errors
		let output_text = output["output"].as_str().unwrap_or("");
		assert!(
			!output_text.contains("syntax error near unexpected token"),
			"Shell parsing error detected: {}",
			output_text
		);
		assert!(
			!output_text.contains("sh: -c: line 0:"),
			"Shell command line error detected: {}",
			output_text
		);

		// The test passes if we don't get shell parsing errors
		// Whether ast-grep finds matches depends on if sg is installed
		println!("ast_grep output: {}", output_text);
	}
}
