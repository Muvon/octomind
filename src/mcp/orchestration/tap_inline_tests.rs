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
use serial_test::serial;

#[test]
fn schema_has_required_action() {
	let f = get_tap_function();
	assert_eq!(f.name, "tap");
	let required = f
		.parameters
		.get("required")
		.and_then(|v| v.as_array())
		.expect("required array");
	assert!(required.iter().any(|v| v.as_str() == Some("action")));
}

#[test]
fn schema_advertises_all_actions() {
	let f = get_tap_function();
	let actions = f
		.parameters
		.get("properties")
		.and_then(|p| p.get("action"))
		.and_then(|a| a.get("enum"))
		.and_then(|e| e.as_array())
		.expect("action enum");
	let names: Vec<&str> = actions.iter().filter_map(|v| v.as_str()).collect();
	assert!(names.contains(&"run"));
	assert!(names.contains(&"list"));
	assert!(names.contains(&"stop"));
	assert!(names.contains(&"discover"));
	assert!(names.contains(&"capability"));
}

#[test]
fn schema_does_not_expose_background_choice() {
	let f = get_tap_function();
	let properties = f
		.parameters
		.get("properties")
		.and_then(|p| p.as_object())
		.expect("properties object");
	assert!(!properties.contains_key("background"));
}

// -------------------------------------------------------------------------
// Command dispatch, parameter validation, and job-registry lifecycle
// -------------------------------------------------------------------------

fn tap_call(params: serde_json::Value) -> McpToolCall {
	McpToolCall {
		tool_name: "tap".to_string(),
		parameters: params,
		tool_id: "t-tap".to_string(),
	}
}

fn test_config() -> Config {
	let mut config: Config = toml::from_str(include_str!("../../../config-templates/default.toml"))
		.expect("parse default config template");
	config.build_role_map();
	config
}

/// Point `OCTOMIND_DATA_DIR` at a fresh tempdir so tap enumeration sees an
/// empty tap set. Tests using it must be `#[serial]` (env is process-global).
struct DataDirGuard {
	previous: Option<std::ffi::OsString>,
	_dir: tempfile::TempDir,
}

impl DataDirGuard {
	fn new() -> Self {
		let previous = std::env::var_os("OCTOMIND_DATA_DIR");
		let dir = tempfile::tempdir().expect("failed to create tempdir");
		std::env::set_var("OCTOMIND_DATA_DIR", dir.path());
		Self {
			previous,
			_dir: dir,
		}
	}
}

impl Drop for DataDirGuard {
	fn drop(&mut self) {
		match self.previous.take() {
			Some(v) => std::env::set_var("OCTOMIND_DATA_DIR", v),
			None => std::env::remove_var("OCTOMIND_DATA_DIR"),
		}
	}
}

/// Register a job for the CURRENT session — must be called inside a
/// `with_session_id` scope, matching how `handle_run` registers real jobs.
fn register_test_job(id: &str, role: &str, status: TapJobStatus, started_at: SystemTime) {
	let (cancel_tx, _cancel_rx) = watch::channel(false);
	tap_runs::register_job(TapJob {
		id: id.to_string(),
		role: role.to_string(),
		workdir: "/tmp".to_string(),
		started_at,
		status: Arc::new(RwLock::new(status)),
		cancel_tx,
		live: Arc::new(RwLock::new(TapLiveState::default())),
	});
}

#[test]
fn schema_description_documents_session_resume() {
	let f = get_tap_function();
	assert!(!f.description.is_empty());
	// The resume contract ("always pass `session` back") must stay documented.
	assert!(f.description.contains("session"));
}

#[tokio::test]
async fn dispatch_missing_action_is_error() {
	let config = test_config();
	let result = execute_tap_command(&tap_call(json!({})), &config)
		.await
		.expect("dispatch");
	assert!(result.is_error());
	assert!(result
		.extract_content()
		.contains("Missing required parameter 'action'"));
}

#[tokio::test]
async fn dispatch_blank_or_non_string_action_is_error() {
	let config = test_config();
	for params in [json!({"action": "   "}), json!({"action": 42})] {
		let result = execute_tap_command(&tap_call(params), &config)
			.await
			.expect("dispatch");
		assert!(result.is_error());
		assert!(result
			.extract_content()
			.contains("Missing required parameter 'action'"));
	}
}

#[tokio::test]
async fn dispatch_unknown_action_is_error() {
	let config = test_config();
	let result = execute_tap_command(&tap_call(json!({"action": "explode"})), &config)
		.await
		.expect("dispatch");
	assert!(result.is_error());
	assert!(result
		.extract_content()
		.contains("Unknown action 'explode'"));
}

#[tokio::test]
async fn list_without_session_reports_no_runs() {
	let config = test_config();
	let result = execute_tap_command(&tap_call(json!({"action": "list"})), &config)
		.await
		.expect("dispatch");
	assert!(!result.is_error());
	assert_eq!(result.extract_content(), "No tap-runs in this session.");
}

#[tokio::test]
#[serial]
async fn list_returns_registered_jobs_newest_first() {
	let config = test_config();
	let sid = "__taptest_list".to_string();
	let out = crate::session::context::with_session_id(sid.clone(), async {
		register_test_job(
			"tap-list-older",
			"developer:general",
			TapJobStatus::Failed,
			SystemTime::UNIX_EPOCH,
		);
		register_test_job(
			"tap-list-newer",
			"lawyer:us",
			TapJobStatus::Done,
			SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(10),
		);
		execute_tap_command(&tap_call(json!({"action": "list"})), &config)
			.await
			.expect("dispatch")
	})
	.await;
	tap_runs::clear_for_session(&sid);

	assert!(!out.is_error());
	let payload: serde_json::Value =
		serde_json::from_str(&out.extract_content()).expect("list payload is JSON");
	assert_eq!(payload["count"].as_u64(), Some(2));
	let runs = payload["runs"].as_array().expect("runs array");
	assert_eq!(runs.len(), 2);
	// Newest first, with the full job-info shape.
	assert_eq!(runs[0]["id"].as_str(), Some("tap-list-newer"));
	assert_eq!(runs[0]["role"].as_str(), Some("lawyer:us"));
	assert_eq!(runs[0]["status"].as_str(), Some("done"));
	assert_eq!(runs[1]["id"].as_str(), Some("tap-list-older"));
	assert_eq!(runs[1]["status"].as_str(), Some("failed"));
}

#[tokio::test]
async fn stop_requires_session_param() {
	let config = test_config();
	for params in [
		json!({"action": "stop"}),
		json!({"action": "stop", "session": "  "}),
	] {
		let result = execute_tap_command(&tap_call(params), &config)
			.await
			.expect("dispatch");
		assert!(result.is_error());
		assert!(result
			.extract_content()
			.contains("Missing required parameter 'session'"));
	}
}

#[tokio::test]
async fn stop_unknown_session_is_error() {
	let config = test_config();
	let result = execute_tap_command(
		&tap_call(json!({"action": "stop", "session": "tap-ghost-000000"})),
		&config,
	)
	.await
	.expect("dispatch");
	assert!(result.is_error());
	assert!(result
		.extract_content()
		.contains("No tap-run with id 'tap-ghost-000000'"));
}

#[tokio::test]
#[serial]
async fn stop_running_job_cancels_it() {
	let config = test_config();
	let sid = "__taptest_stop_running".to_string();
	let out = crate::session::context::with_session_id(sid.clone(), async {
		register_test_job(
			"tap-stop-me",
			"developer:general",
			TapJobStatus::Running,
			SystemTime::UNIX_EPOCH,
		);
		execute_tap_command(
			&tap_call(json!({"action": "stop", "session": "tap-stop-me"})),
			&config,
		)
		.await
		.expect("dispatch")
	})
	.await;
	tap_runs::clear_for_session(&sid);

	assert!(!out.is_error());
	let payload: serde_json::Value =
		serde_json::from_str(&out.extract_content()).expect("stop payload is JSON");
	assert_eq!(payload["id"].as_str(), Some("tap-stop-me"));
	assert_eq!(payload["status"].as_str(), Some("cancelled"));
}

#[tokio::test]
#[serial]
async fn stop_finished_job_reports_terminal_status() {
	let config = test_config();
	let sid = "__taptest_stop_done".to_string();
	let out = crate::session::context::with_session_id(sid.clone(), async {
		register_test_job(
			"tap-already-done",
			"developer:general",
			TapJobStatus::Done,
			SystemTime::UNIX_EPOCH,
		);
		execute_tap_command(
			&tap_call(json!({"action": "stop", "session": "tap-already-done"})),
			&config,
		)
		.await
		.expect("dispatch")
	})
	.await;
	tap_runs::clear_for_session(&sid);

	assert!(!out.is_error());
	let payload: serde_json::Value =
		serde_json::from_str(&out.extract_content()).expect("stop payload is JSON");
	assert_eq!(payload["status"].as_str(), Some("done"));
}

#[tokio::test]
async fn discover_requires_intent() {
	let config = test_config();
	for params in [
		json!({"action": "discover"}),
		json!({"action": "discover", "intent": "  "}),
	] {
		let result = execute_tap_command(&tap_call(params), &config)
			.await
			.expect("dispatch");
		assert!(result.is_error());
		assert!(result
			.extract_content()
			.contains("Missing required parameter 'intent'"));
	}
}

#[tokio::test]
#[serial]
async fn discover_with_no_taps_installed_succeeds() {
	let _guard = DataDirGuard::new();
	let config = test_config();
	let result = execute_tap_command(
		&tap_call(json!({"action": "discover", "intent": "review a contract"})),
		&config,
	)
	.await
	.expect("dispatch");
	assert!(!result.is_error());
	assert_eq!(result.extract_content(), "No tap agents installed.");
}

#[tokio::test]
async fn capability_requires_prompt() {
	let config = test_config();
	for params in [
		json!({"action": "capability"}),
		json!({"action": "capability", "prompt": "   "}),
	] {
		let result = execute_tap_command(&tap_call(params), &config)
			.await
			.expect("dispatch");
		assert!(result.is_error());
		assert!(result
			.extract_content()
			.contains("Missing required parameter 'prompt'"));
	}
}

#[tokio::test]
async fn run_requires_prompt() {
	let config = test_config();
	let result = execute_tap_command(
		&tap_call(json!({"action": "run", "role": "developer:general"})),
		&config,
	)
	.await
	.expect("dispatch");
	assert!(result.is_error());
	assert!(result
		.extract_content()
		.contains("Missing required parameter 'prompt'"));
}

#[tokio::test]
async fn run_requires_role_or_session() {
	let config = test_config();
	let result = execute_tap_command(
		&tap_call(json!({"action": "run", "prompt": "do work"})),
		&config,
	)
	.await
	.expect("dispatch");
	assert!(result.is_error());
	assert!(result
		.extract_content()
		.contains("Missing 'role' for new run"));
}

#[tokio::test]
async fn run_with_unknown_session_is_error() {
	let config = test_config();
	let result = execute_tap_command(
		&tap_call(json!({"action": "run", "prompt": "hi", "session": "tap-ghost-000000"})),
		&config,
	)
	.await
	.expect("dispatch");
	assert!(result.is_error());
	assert!(result
		.extract_content()
		.contains("No tap-run with id 'tap-ghost-000000'"));
}

#[test]
fn format_job_info_contains_all_fields() {
	let info = TapJobInfo {
		id: "tap-x-000001".to_string(),
		role: "developer:general".to_string(),
		workdir: "/tmp/proj".to_string(),
		started_at: SystemTime::UNIX_EPOCH,
		status: TapJobStatus::Done,
		live: TapLiveState::default(),
	};
	let v = format_job_info(&info);
	assert_eq!(v["id"].as_str(), Some("tap-x-000001"));
	assert_eq!(v["role"].as_str(), Some("developer:general"));
	assert_eq!(v["workdir"].as_str(), Some("/tmp/proj"));
	assert_eq!(v["status"].as_str(), Some("done"));
	assert_eq!(v["started_at"].as_u64(), Some(0));
}
