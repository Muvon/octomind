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

// Plan tool tests
//
// NOTE: These tests use a global static storage (PLAN_STORAGE) which can cause race conditions
// when tests run in parallel. If tests fail intermittently, run with: cargo test test_plan -- --test-threads=1
// This is not an issue in production since each session runs independently.

#[cfg(test)]
mod tests {
	use crate::mcp::dev::plan::{clear_plan_data, execute_plan};
	use crate::mcp::{extract_mcp_content, McpToolCall};
	use serde_json::json;
	use serial_test::serial;

	// Helper function to create plan tool calls
	fn create_plan_call(
		command: &str,
		additional_params: Option<serde_json::Value>,
	) -> McpToolCall {
		let mut params = serde_json::Map::new();
		params.insert("command".to_string(), json!(command));

		if let Some(serde_json::Value::Object(map)) = additional_params {
			for (key, value) in map {
				params.insert(key, value);
			}
		}

		McpToolCall {
			tool_name: "plan".to_string(),
			parameters: serde_json::Value::Object(params),
			tool_id: "test-id".to_string(),
		}
	}

	#[serial]
	#[tokio::test]
	async fn test_plan_start_command_success() {
		// Clear any existing plan data
		let _ = clear_plan_data().await;

		let call = create_plan_call(
			"start",
			Some(json!({
				"content": "Test Plan",
				"tasks": [
					{"title": "Task 1", "description": "First task description"},
					{"title": "Task 2", "description": "Second task description"}
				]
			})),
		);
		let result = execute_plan(&call).await.unwrap();

		let output = result.result.as_object().unwrap();
		assert_eq!(output["isError"], json!(false));

		let content = extract_mcp_content(&result.result);
		assert!(content.contains("Test Plan"));
		assert!(content.contains("Task 1"));
		assert!(content.contains("Task 2"));
		assert!(content.contains("CURRENT: Task 1/2 - Task 1"));

		// Cleanup for test isolation
		let _ = clear_plan_data().await;
	}

	#[serial]
	#[tokio::test]
	async fn test_plan_start_command_validation_errors() {
		// Clear any existing plan data
		let _ = clear_plan_data().await;

		// Test missing tasks
		let call = create_plan_call(
			"start",
			Some(json!({
				"content": "Test Plan"
			})),
		);
		let result = execute_plan(&call).await.unwrap();
		let output = result.result.as_object().unwrap();
		assert_eq!(output["isError"], json!(true));
		assert!(extract_mcp_content(&result.result).contains("Missing required parameter 'tasks'"));

		// Test empty tasks array
		let call = create_plan_call(
			"start",
			Some(json!({
				"content": "Test Plan",
				"tasks": []
			})),
		);
		let result = execute_plan(&call).await.unwrap();
		let output = result.result.as_object().unwrap();
		assert_eq!(output["isError"], json!(true));
		assert!(extract_mcp_content(&result.result).contains("Tasks array cannot be empty"));

		// Test task missing title field
		let call = create_plan_call(
			"start",
			Some(json!({
				"content": "Test Plan",
				"tasks": [{"description": "No title here"}]
			})),
		);
		let result = execute_plan(&call).await.unwrap();
		let output = result.result.as_object().unwrap();
		assert_eq!(output["isError"], json!(true));
		assert!(extract_mcp_content(&result.result).contains("missing required 'title' field"));

		// Test task missing description field
		let call = create_plan_call(
			"start",
			Some(json!({
				"content": "Test Plan",
				"tasks": [{"title": "Task 1"}]
			})),
		);
		let result = execute_plan(&call).await.unwrap();
		let output = result.result.as_object().unwrap();
		assert_eq!(output["isError"], json!(true));
		assert!(
			extract_mcp_content(&result.result).contains("missing required 'description' field")
		);
	}

	#[serial]
	#[tokio::test]
	async fn test_plan_step_command() {
		// Clear any existing plan data and setup plan first
		let _ = clear_plan_data().await;
		let start_call = create_plan_call(
			"start",
			Some(json!({
				"title": "Test Plan",
				"tasks": [
					{"title": "Task 1", "description": "First task description"},
					{"title": "Task 2", "description": "Second task description"}
				]
			})),
		);
		let _ = execute_plan(&start_call).await;

		// Test adding step details
		let step_call = create_plan_call(
			"step",
			Some(json!({
				"content": "Working on authentication logic"
			})),
		);
		let result = execute_plan(&step_call).await.unwrap();
		let output = result.result.as_object().unwrap();
		assert_eq!(output["isError"], json!(false));
		assert!(extract_mcp_content(&result.result).contains("Step details added to Task"));

		// Test getting step details (no content parameter)
		let get_call = create_plan_call("step", None);
		let result = execute_plan(&get_call).await.unwrap();
		let output = result.result.as_object().unwrap();
		assert_eq!(output["isError"], json!(false));
		let content = extract_mcp_content(&result.result);
		assert!(content.contains("CURRENT TASK"));
		assert!(content.contains("Working on authentication logic"));

		// Test empty content error
		let empty_call = create_plan_call(
			"step",
			Some(json!({
				"content": ""
			})),
		);
		let result = execute_plan(&empty_call).await.unwrap();
		let output = result.result.as_object().unwrap();
		assert_eq!(output["isError"], json!(true));
		assert!(extract_mcp_content(&result.result).contains("Content parameter cannot be empty"));

		// Cleanup for test isolation
		let _ = clear_plan_data().await;
	}

	#[serial]
	#[tokio::test]
	async fn test_plan_list_command() {
		// Clear any existing plan data and setup plan with some progress
		let _ = clear_plan_data().await;
		let start_call = create_plan_call(
			"start",
			Some(json!({
				"content": "Development Tasks",
				"tasks": [
					{"title": "Design", "description": "Design the system"},
					{"title": "Implement", "description": "Implement the features"},
					{"title": "Test", "description": "Test the implementation"},
					{"title": "Deploy", "description": "Deploy to production"}
				]
			})),
		);
		let _ = execute_plan(&start_call).await;

		// Complete first task
		let next_call = create_plan_call(
			"next",
			Some(json!({
				"content": "Design completed"
			})),
		);
		let _ = execute_plan(&next_call).await;

		// Test list command
		let list_call = create_plan_call("list", None);
		let result = execute_plan(&list_call).await.unwrap();
		let output = result.result.as_object().unwrap();
		assert_eq!(output["isError"], json!(false));

		let content = extract_mcp_content(&result.result);
		assert!(content.contains("Development Tasks"));
		assert!(content.contains("✅ 1. Design"));
		assert!(content.contains("🔄 2. Implement (IN PROGRESS)"));
		assert!(content.contains("⏳ 3. Test"));
		assert!(content.contains("⏳ 4. Deploy"));

		// Cleanup for test isolation
		let _ = clear_plan_data().await;
	}

	#[serial]
	#[tokio::test]
	async fn test_plan_done_command() {
		// Clear any existing plan data and setup plan
		let _ = clear_plan_data().await;
		let start_call = create_plan_call(
			"start",
			Some(json!({
				"content": "Simple Task",
				"tasks": [{"title": "Complete project", "description": "Finish all remaining work"}]
			})),
		);
		let _ = execute_plan(&start_call).await;

		// Test done command
		let done_call = create_plan_call(
			"done",
			Some(json!({
				"content": "Project completed successfully"
			})),
		);
		let result = execute_plan(&done_call).await.unwrap();
		let output = result.result.as_object().unwrap();
		assert_eq!(output["isError"], json!(false));

		let content = extract_mcp_content(&result.result);
		assert!(content.contains("PLAN COMPLETED"));
		assert!(content.contains("Simple Task"));

		// Cleanup for test isolation
		let _ = clear_plan_data().await;
	}

	#[serial]
	#[tokio::test]
	async fn test_plan_reset_command() {
		// Clear any existing plan data and setup plan first
		let _ = clear_plan_data().await;
		let start_call = create_plan_call(
			"start",
			Some(json!({
				"title": "Test Plan",
				"tasks": [{"title": "Task 1", "description": "First task description"}]
			})),
		);
		let _ = execute_plan(&start_call).await;

		// Test reset command
		let reset_call = create_plan_call("reset", None);
		let result = execute_plan(&reset_call).await.unwrap();
		let output = result.result.as_object().unwrap();
		assert_eq!(output["isError"], json!(false));
		assert!(extract_mcp_content(&result.result).contains("Plan data cleared successfully"));

		// Verify plan is cleared
		let list_call = create_plan_call("list", None);
		let result = execute_plan(&list_call).await.unwrap();
		let output = result.result.as_object().unwrap();
		assert_eq!(output["isError"], json!(true));
		assert!(extract_mcp_content(&result.result).contains("No active plan"));
	}

	#[serial]
	#[tokio::test]
	async fn test_plan_invalid_command() {
		let call = create_plan_call("invalid_command", None);
		let result = execute_plan(&call).await.unwrap();
		let output = result.result.as_object().unwrap();
		assert_eq!(output["isError"], json!(true));
		assert!(extract_mcp_content(&result.result).contains("Unknown command 'invalid_command'"));
	}

	#[serial]
	#[tokio::test]
	async fn test_plan_step_vs_next_behavior() {
		// Clear any existing plan data
		let _ = clear_plan_data().await;

		// Create a plan with 2 tasks
		let start_call = create_plan_call(
			"start",
			Some(json!({
				"title": "Behavior Test",
				"tasks": [
					{"title": "Task 1", "description": "First task description"},
					{"title": "Task 2", "description": "Second task description"}
				]
			})),
		);
		let _ = execute_plan(&start_call).await;

		// Add step details - should NOT complete the task
		let step_call = create_plan_call(
			"step",
			Some(json!({
				"content": "Working on task 1"
			})),
		);
		let _ = execute_plan(&step_call).await;

		// Check that we're still on task 1
		let list_call = create_plan_call("list", None);
		let result = execute_plan(&list_call).await.unwrap();
		let content = extract_mcp_content(&result.result);
		assert!(content.contains("🔄 1. Task 1 (IN PROGRESS)")); // Still in progress
		assert!(content.contains("⏳ 2. Task 2")); // Still pending

		// Now use next to complete task 1
		let next_call = create_plan_call(
			"next",
			Some(json!({
				"content": "Task 1 completed"
			})),
		);
		let _ = execute_plan(&next_call).await;

		// Check that task 1 is completed and we moved to task 2
		let list_call = create_plan_call("list", None);
		let result = execute_plan(&list_call).await.unwrap();
		let content = extract_mcp_content(&result.result);
		assert!(content.contains("✅ 1. Task 1")); // Now completed
		assert!(content.contains("🔄 2. Task 2 (IN PROGRESS)")); // Now current

		// Cleanup for test isolation
		let _ = clear_plan_data().await;
	}

	#[serial]
	#[tokio::test]
	async fn test_plan_start_prevents_overwrite() {
		// Clear any existing plan data
		let _ = clear_plan_data().await;

		// Create first plan
		let start_call1 = create_plan_call(
			"start",
			Some(json!({
				"content": "First Plan",
				"tasks": [
					{"title": "Task A", "description": "Task A description"},
					{"title": "Task B", "description": "Task B description"}
				]
			})),
		);
		let result1 = execute_plan(&start_call1).await.unwrap();
		let output1 = result1.result.as_object().unwrap();
		assert_eq!(output1["isError"], json!(false));
		let content1 = extract_mcp_content(&result1.result);
		assert!(content1.contains("First Plan"));
		assert!(content1.contains("Task A"));

		// Add some progress to first plan
		let step_call = create_plan_call(
			"step",
			Some(json!({
				"content": "Working on Task A"
			})),
		);
		let _ = execute_plan(&step_call).await;

		// Try to create second plan - should ERROR with clear message
		let start_call2 = create_plan_call(
			"start",
			Some(json!({
				"content": "Second Plan",
				"tasks": [
					{"title": "Task X", "description": "Task X description"},
					{"title": "Task Y", "description": "Task Y description"},
					{"title": "Task Z", "description": "Task Z description"}
				]
			})),
		);
		let result2 = execute_plan(&start_call2).await.unwrap();
		let output2 = result2.result.as_object().unwrap();
		assert_eq!(output2["isError"], json!(true)); // Should fail
		let error_content = extract_mcp_content(&result2.result);
		assert!(error_content.contains("Active plan already exists"));
		assert!(error_content.contains("'done' to complete current plan"));
		assert!(error_content.contains("'reset' to clear it"));
		assert!(error_content.contains("'list' to view current progress"));

		// Verify the first plan is still intact
		let list_call = create_plan_call("list", None);
		let result = execute_plan(&list_call).await.unwrap();
		let content = extract_mcp_content(&result.result);
		assert!(content.contains("First Plan"));
		assert!(content.contains("Task A"));
		assert!(!content.contains("Second Plan")); // Second plan was NOT created
		assert!(!content.contains("Task X"));

		// Cleanup for test isolation
		let _ = clear_plan_data().await;
	}

	#[serial]
	#[tokio::test]
	async fn test_plan_start_after_done_works() {
		// Clear any existing plan data
		let _ = clear_plan_data().await;

		// Create and complete first plan
		let start_call1 = create_plan_call(
			"start",
			Some(json!({
				"content": "First Plan",
				"tasks": [{"title": "Task A", "description": "Task A description"}]
			})),
		);
		let _ = execute_plan(&start_call1).await;

		let done_call = create_plan_call(
			"done",
			Some(json!({
				"content": "First plan completed"
			})),
		);
		let _ = execute_plan(&done_call).await;

		// Now start second plan should work
		let start_call2 = create_plan_call(
			"start",
			Some(json!({
				"content": "Second Plan",
				"tasks": [
					{"title": "Task X", "description": "Task X description"},
					{"title": "Task Y", "description": "Task Y description"}
				]
			})),
		);
		let result2 = execute_plan(&start_call2).await.unwrap();
		let output2 = result2.result.as_object().unwrap();
		assert_eq!(output2["isError"], json!(false)); // Should succeed
		let content2 = extract_mcp_content(&result2.result);
		assert!(content2.contains("Second Plan"));
		assert!(content2.contains("Task X"));

		// Cleanup for test isolation
		let _ = clear_plan_data().await;
	}

	#[serial]
	#[tokio::test]
	async fn test_plan_start_after_reset_works() {
		// Clear any existing plan data
		let _ = clear_plan_data().await;

		// Create first plan with progress
		let start_call1 = create_plan_call(
			"start",
			Some(json!({
				"content": "First Plan",
				"tasks": [
					{"title": "Task A", "description": "Task A description"},
					{"title": "Task B", "description": "Task B description"}
				]
			})),
		);
		let _ = execute_plan(&start_call1).await;

		let step_call = create_plan_call(
			"step",
			Some(json!({
				"content": "Working on Task A"
			})),
		);
		let _ = execute_plan(&step_call).await;

		// Reset the plan
		let reset_call = create_plan_call("reset", None);
		let _ = execute_plan(&reset_call).await;

		// Now start second plan should work
		let start_call2 = create_plan_call(
			"start",
			Some(json!({
				"content": "Second Plan",
				"tasks": [
					{"title": "Task X", "description": "Task X description"},
					{"title": "Task Y", "description": "Task Y description"}
				]
			})),
		);
		let result2 = execute_plan(&start_call2).await.unwrap();
		let output2 = result2.result.as_object().unwrap();
		assert_eq!(output2["isError"], json!(false)); // Should succeed
		let content2 = extract_mcp_content(&result2.result);
		assert!(content2.contains("Second Plan"));
		assert!(content2.contains("Task X"));

		// Cleanup for test isolation
		let _ = clear_plan_data().await;
	}
}
