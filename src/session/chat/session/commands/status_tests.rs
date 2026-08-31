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
		panic!("expected status output");
	};
	let CommandOutput::Status { data } = *output else {
		panic!("expected status variant");
	};
	data
}

#[tokio::test]
async fn status_lists_pending_mcp_jobs_without_scheme_assumptions() {
	let session_id = format!("status-command-jobs-{}", uuid::Uuid::new_v4());
	crate::session::context::with_session_id(session_id.clone(), async {
		crate::session::shell_jobs::register_for_session(
			&session_id,
			"octofs-test",
			"octofs://jobs/1234-7",
			"ssh dev 'cargo test --lib'",
		);

		let data = output_data(handle_status(&[]).await.expect("status command"));
		assert_eq!(data["view"], "overview");
		assert_eq!(data["active"], 1);
		assert_eq!(data["jobs"][0]["server"], "octofs-test");
		assert_eq!(data["jobs"][0]["uri"], "octofs://jobs/1234-7");
		assert_eq!(data["jobs"][0]["label"], "ssh dev 'cargo test --lib'");
		assert!(data["monitors"].as_array().unwrap().is_empty());

		crate::session::shell_jobs::clear_for_session(&session_id);
	})
	.await;
}

#[tokio::test]
async fn status_empty_state_covers_all_activity_kinds() {
	let session_id = format!("status-command-empty-{}", uuid::Uuid::new_v4());
	crate::session::context::with_session_id(session_id, async {
		let data = output_data(handle_status(&[]).await.expect("status command"));
		assert_eq!(data["view"], "overview");
		assert_eq!(data["active"], 0);
		assert!(data["agents"].as_array().unwrap().is_empty());
		assert!(data["jobs"].as_array().unwrap().is_empty());
		assert!(data["monitors"].as_array().unwrap().is_empty());
	})
	.await;
}

#[cfg(unix)]
#[tokio::test]
async fn status_combines_mcp_jobs_with_command_monitors() {
	let session_id = format!("status-command-combined-{}", uuid::Uuid::new_v4());
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

		let overview = output_data(handle_status(&[]).await.expect("status command"));
		assert_eq!(overview["active"], 2);
		assert_eq!(overview["jobs"].as_array().unwrap().len(), 1);
		assert_eq!(overview["monitors"].as_array().unwrap().len(), 1);

		let monitors = output_data(handle_status(&["monitors"]).await.expect("monitor status"));
		assert_eq!(monitors["view"], "monitors");
		assert_eq!(monitors["monitors"][0]["description"], "watch changes");

		let jobs = output_data(handle_status(&["jobs"]).await.expect("job status"));
		assert_eq!(jobs["view"], "jobs");
		assert_eq!(jobs["jobs"][0]["server"], "generic-mcp");
		assert!(jobs["jobs"][0]["resource_status"]
			.as_str()
			.unwrap_or_default()
			.contains("is not active"));

		crate::session::shell_jobs::clear_for_session(&session_id);
		crate::mcp::orchestration::monitor::clear_for_session(&session_id);
	})
	.await;
}

#[tokio::test]
async fn status_rejects_unknown_filters() {
	let session_id = format!("status-command-error-{}", uuid::Uuid::new_v4());
	crate::session::context::with_session_id(session_id, async {
		let data = output_data(handle_status(&["unknown"]).await.expect("status command"));
		assert_eq!(data["view"], "error");
		assert!(data["message"]
			.as_str()
			.unwrap()
			.contains("Unknown status filter"));
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
