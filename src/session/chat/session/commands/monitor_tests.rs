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

use super::*;

fn output_data(result: CommandResult) -> serde_json::Value {
	let CommandResult::HandledWithOutput(output) = result else {
		panic!("expected monitor output");
	};
	let CommandOutput::Monitor { data } = *output else {
		panic!("expected monitor variant");
	};
	data
}

#[tokio::test]
async fn monitor_lists_pending_mcp_jobs_even_without_command_monitors() {
	let session_id = format!("monitor-command-jobs-{}", uuid::Uuid::new_v4());
	crate::session::context::with_session_id(session_id.clone(), async {
		crate::session::shell_jobs::register_for_session(
			&session_id,
			"octofs-test",
			"octofs://jobs/1234-7",
			"ssh dev 'cargo test --lib'",
		);

		let data = output_data(handle_monitor().await.expect("monitor command"));
		assert_eq!(data["subcommand"], "list");
		assert_eq!(data["job_count"], 1);
		assert_eq!(data["monitor_count"], 0);
		assert_eq!(data["jobs"][0]["server"], "octofs-test");
		assert_eq!(data["jobs"][0]["uri"], "octofs://jobs/1234-7");
		let message = data["message"].as_str().expect("rendered message");
		assert!(message.contains("MCP background jobs:"), "{message}");
		assert!(message.contains("MCP server: octofs-test"), "{message}");
		assert!(message.contains("ssh dev 'cargo test --lib'"), "{message}");
		assert!(
			message.contains("MCP connection 'octofs-test' is not active"),
			"{message}"
		);

		crate::session::shell_jobs::clear_for_session(&session_id);
	})
	.await;
}

#[tokio::test]
async fn monitor_empty_state_covers_both_activity_kinds() {
	let session_id = format!("monitor-command-empty-{}", uuid::Uuid::new_v4());
	crate::session::context::with_session_id(session_id, async {
		let data = output_data(handle_monitor().await.expect("monitor command"));
		assert_eq!(data["job_count"], 0);
		assert_eq!(data["monitor_count"], 0);
		assert_eq!(data["message"], "No background activity.");
	})
	.await;
}

#[cfg(unix)]
#[tokio::test]
async fn monitor_combines_mcp_jobs_with_command_monitors() {
	let session_id = format!("monitor-command-combined-{}", uuid::Uuid::new_v4());
	crate::session::context::with_session_id(session_id.clone(), async {
		crate::session::inbox::init_inbox_for_session();
		crate::mcp::orchestration::monitor::init_for_session();
		let call = crate::mcp::McpToolCall {
			tool_name: "monitor".to_string(),
			tool_id: "monitor-combined-start".to_string(),
			parameters: serde_json::json!({
				"action": "start",
				"command": "sleep 30",
				"description": "watch changes",
				"persistent": true,
			}),
		};
		let started = crate::mcp::orchestration::monitor::execute_monitor_tool(&call)
			.await
			.expect("start monitor");
		assert!(!started.is_error(), "{}", started.extract_content());
		crate::session::shell_jobs::register_for_session(
			&session_id,
			"generic-mcp",
			"custom://tasks/7",
			"background analysis",
		);

		let data = output_data(handle_monitor().await.expect("monitor command"));
		assert_eq!(data["job_count"], 1);
		assert_eq!(data["monitor_count"], 1);
		let message = data["message"].as_str().expect("rendered message");
		assert!(message.contains("MCP background jobs:"), "{message}");
		assert!(message.contains("Running monitors:"), "{message}");
		assert!(message.contains("watch changes"), "{message}");

		crate::session::shell_jobs::clear_for_session(&session_id);
		crate::mcp::orchestration::monitor::clear_for_session(&session_id);
	})
	.await;
}

#[test]
fn resource_status_is_bounded_and_preserves_both_ends() {
	let input = format!("HEAD{}TAIL", "x".repeat(MAX_RESOURCE_STATUS_CHARS + 100));
	let bounded = bound_resource_status(&input);
	assert!(bounded.starts_with("HEAD"));
	assert!(bounded.ends_with("TAIL"));
	assert!(bounded.contains("status characters omitted"));
}
