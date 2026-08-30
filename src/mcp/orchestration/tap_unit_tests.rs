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

//! Additional unit tests for the `tap` orchestration tool — complements the
//! inline `mod tests` with schema-contract checks, action trimming, the
//! resume-while-busy guard, the capability no-match payload, and the
//! discover-with-agents-but-no-embeddings path.

use super::*;
use serial_test::serial;

fn tap_call(params: serde_json::Value) -> McpToolCall {
	McpToolCall {
		tool_name: "tap".to_string(),
		parameters: params,
		tool_id: "t-tap-unit".to_string(),
	}
}

fn unit_config() -> Config {
	let mut config: Config = toml::from_str(include_str!("../../../config-templates/default.toml"))
		.expect("parse default config template");
	config.build_role_map();
	config
}

/// Point `OCTOMIND_DATA_DIR` at a fresh temp dir for the lifetime of the guard,
/// restoring the previous value on drop. `get_octomind_data_dir()` reads the
/// env var on every call, so this fully sandboxes tap discovery.
struct TempDataDir {
	previous: Option<std::ffi::OsString>,
	dir: tempfile::TempDir,
}

impl TempDataDir {
	fn new() -> Self {
		let previous = std::env::var_os("OCTOMIND_DATA_DIR");
		let dir = tempfile::tempdir().expect("create temp data dir");
		std::env::set_var("OCTOMIND_DATA_DIR", dir.path());
		Self { previous, dir }
	}

	fn path(&self) -> &std::path::Path {
		self.dir.path()
	}
}

impl Drop for TempDataDir {
	fn drop(&mut self) {
		match self.previous.take() {
			Some(old) => std::env::set_var("OCTOMIND_DATA_DIR", old),
			None => std::env::remove_var("OCTOMIND_DATA_DIR"),
		}
	}
}

fn register_unit_job(id: &str, role: &str, status: TapJobStatus) {
	let (cancel_tx, _cancel_rx) = watch::channel(false);
	tap_runs::register_job(TapJob {
		id: id.to_string(),
		role: role.to_string(),
		workdir: "/tmp".to_string(),
		started_at: SystemTime::UNIX_EPOCH,
		status: Arc::new(RwLock::new(status)),
		cancel_tx,
		live: Arc::new(RwLock::new(TapLiveState::default())),
	});
}

// ---------------------------------------------------------------------------
// Schema contract
// ---------------------------------------------------------------------------

#[test]
fn schema_documents_every_parameter() {
	let function = get_tap_function();
	let props = function
		.parameters
		.get("properties")
		.and_then(|p| p.as_object())
		.expect("properties object");
	for key in ["role", "prompt", "session", "workdir", "intent"] {
		let prop = props.get(key).unwrap_or_else(|| panic!("{key} missing"));
		assert_eq!(
			prop.get("type").and_then(|t| t.as_str()),
			Some("string"),
			"{key} type"
		);
		assert!(
			prop.get("description")
				.and_then(|d| d.as_str())
				.is_some_and(|d| !d.is_empty()),
			"{key} description"
		);
	}
	assert_eq!(
		function
			.parameters
			.get("required")
			.and_then(|r| r.as_array()),
		Some(&vec![json!("action")])
	);
}

#[test]
fn schema_description_documents_actions_and_resume_contract() {
	let function = get_tap_function();
	for phrase in [
		"`run`",
		"`list`",
		"`stop`",
		"`discover`",
		"`capability`",
		"session",
		"workdir",
	] {
		assert!(
			function.description.contains(phrase),
			"description missing '{phrase}'"
		);
	}
}

// ---------------------------------------------------------------------------
// Dispatcher
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dispatch_trims_surrounding_whitespace_from_action() {
	let result = execute_tap_command(&tap_call(json!({"action": "  list  "})), &unit_config())
		.await
		.expect("dispatch");
	assert!(!result.is_error());
	assert_eq!(result.extract_content(), "No tap-runs in this session.");
}

#[tokio::test]
async fn stop_with_non_string_session_is_error() {
	let result = execute_tap_command(
		&tap_call(json!({"action": "stop", "session": 123})),
		&unit_config(),
	)
	.await
	.expect("dispatch");
	assert!(result.is_error());
	assert!(result
		.extract_content()
		.contains("Missing required parameter 'session'"));
}

// ---------------------------------------------------------------------------
// handle_run — resume guard
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn run_resume_while_busy_is_rejected() {
	let session_id = "__tapunit_busy".to_string();
	let result = crate::session::context::with_session_id(session_id.clone(), async {
		register_unit_job("tap-unit-busy", "developer:general", TapJobStatus::Running);
		execute_tap_command(
			&tap_call(json!({
				"action": "run",
				"prompt": "continue the work",
				"session": "tap-unit-busy",
			})),
			&unit_config(),
		)
		.await
		.expect("dispatch")
	})
	.await;
	tap_runs::clear_for_session(&session_id);
	assert!(result.is_error());
	let content = result.extract_content();
	assert!(
		content.contains("busy with a previous turn"),
		"content: {content}"
	);
}

// ---------------------------------------------------------------------------
// handle_capability — no-match payload
// ---------------------------------------------------------------------------

#[tokio::test]
async fn capability_with_low_signal_prompt_reports_no_match() {
	// "ok" is below the activation signal threshold, so the outcome is
	// deterministic without any embedding model or installed capability.
	let result = execute_tap_command(
		&tap_call(json!({"action": "capability", "prompt": "ok"})),
		&unit_config(),
	)
	.await
	.expect("dispatch");
	assert!(!result.is_error());
	let payload: serde_json::Value =
		serde_json::from_str(&result.extract_content()).expect("json payload");
	assert_eq!(
		payload["activated_capabilities"].as_array().map(Vec::len),
		Some(0)
	);
	assert_eq!(payload["message"], "No capability matched the prompt.");
}

// ---------------------------------------------------------------------------
// handle_discover — agents installed but embeddings unavailable
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn discover_with_installed_agents_requires_embeddings() {
	let guard = TempDataDir::new();
	if crate::embeddings::is_ready() {
		eprintln!("skipping: embedding provider already initialized by a prior test");
		return;
	}
	// Fabricate one agent inside the default tap's agents tree so
	// list_all_tap_agents() finds it, then rely on the embedding model not
	// being warmed in the unit-test binary.
	let agents_dir = guard
		.path()
		.join("taps")
		.join("muvon")
		.join("octomind-tap")
		.join("agents")
		.join("developer");
	std::fs::create_dir_all(&agents_dir).expect("create agents dir");
	std::fs::write(
		agents_dir.join("general.toml"),
		"# Title: Unit Test Developer\n# Description: Fabricated specialist for tests.\n[[roles]]\nname = \"general\"\n",
	)
	.expect("write agent file");

	let result = execute_tap_command(
		&tap_call(json!({"action": "discover", "intent": "debug a failing rust build"})),
		&unit_config(),
	)
	.await
	.expect("dispatch");
	assert!(result.is_error());
	let content = result.extract_content();
	assert!(
		content.contains("requires the embedding model"),
		"content: {content}"
	);
}

// ---------------------------------------------------------------------------
// format_job_info + generate_id
// ---------------------------------------------------------------------------

#[test]
fn format_job_info_maps_remaining_status_variants() {
	for (status, expected) in [
		(TapJobStatus::Running, "running"),
		(TapJobStatus::Cancelled, "cancelled"),
		(TapJobStatus::Failed, "failed"),
	] {
		let info = TapJobInfo {
			id: "tap-unit-000001".to_string(),
			role: "lawyer:us".to_string(),
			workdir: "/tmp/unit".to_string(),
			started_at: SystemTime::UNIX_EPOCH,
			status,
			live: TapLiveState::default(),
		};
		assert_eq!(
			format_job_info(&info)["status"].as_str(),
			Some(expected),
			"status {expected}"
		);
	}
}

#[test]
fn generate_id_formats_role_slug_with_hex_suffix() {
	let id = tap_runs::generate_id("Developer:General");
	assert!(id.starts_with("tap-developer-general-"), "id: {id}");
	let suffix = id.rsplit('-').next().expect("suffix");
	assert_eq!(suffix.len(), 6, "id: {id}");
	assert!(
		suffix.chars().all(|c| c.is_ascii_hexdigit()),
		"suffix: {suffix}"
	);
	// Consecutive calls differ: the counter is mixed through an odd
	// multiplier, so distinct counters cannot collide in the low 24 bits.
	assert_ne!(
		tap_runs::generate_id("developer:general"),
		tap_runs::generate_id("developer:general")
	);
}

// ---------------------------------------------------------------------------
// handle_run — fresh run and resume both reach a terminal state
// ---------------------------------------------------------------------------

/// The spawned ACP child is the test binary itself (`current_exe()`), which
/// rejects the `--name` flag and exits immediately — so every background
/// turn is short-lived and lands in a terminal state. Poll until it does.
async fn wait_terminal_status(id: &str) -> TapJobStatus {
	let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
	loop {
		if let Some(info) = tap_runs::find_job(id) {
			if info.status != TapJobStatus::Running {
				return info.status;
			}
		}
		assert!(
			std::time::Instant::now() < deadline,
			"tap-run '{id}' never left Running"
		);
		tokio::time::sleep(std::time::Duration::from_millis(20)).await;
	}
}

#[tokio::test]
#[serial]
async fn run_fresh_registers_job_and_reports_started_payload() {
	let session_id = "__tapunit_fresh".to_string();
	let out = crate::session::context::with_session_id(session_id.clone(), async {
		execute_tap_command(
			&tap_call(json!({
				"action": "run",
				"role": "developer:general",
				"prompt": "do a tiny thing",
				"workdir": "/tmp",
			})),
			&unit_config(),
		)
		.await
		.expect("dispatch")
	})
	.await;

	assert!(!out.is_error(), "content: {}", out.extract_content());
	let payload: serde_json::Value =
		serde_json::from_str(&out.extract_content()).expect("started payload is JSON");
	let id = payload["id"].as_str().expect("id").to_string();
	assert!(id.starts_with("tap-developer-general-"), "id: {id}");
	assert_eq!(payload["role"].as_str(), Some("developer:general"));
	assert_eq!(payload["workdir"].as_str(), Some("/tmp"));

	let status = crate::session::context::with_session_id(session_id.clone(), async {
		wait_terminal_status(&id).await
	})
	.await;
	assert!(
		matches!(status, TapJobStatus::Failed | TapJobStatus::Cancelled),
		"status: {status:?}"
	);
	tap_runs::clear_for_session(&session_id);
}

#[tokio::test]
#[serial]
async fn run_resume_restarts_a_finished_job_with_its_original_role() {
	let session_id = "__tapunit_resume".to_string();
	let out = crate::session::context::with_session_id(session_id.clone(), async {
		register_unit_job("tap-unit-resume", "lawyer:us", TapJobStatus::Done);
		execute_tap_command(
			&tap_call(json!({
				"action": "run",
				"prompt": "continue",
				"session": "tap-unit-resume",
			})),
			&unit_config(),
		)
		.await
		.expect("dispatch")
	})
	.await;

	assert!(!out.is_error(), "content: {}", out.extract_content());
	let payload: serde_json::Value =
		serde_json::from_str(&out.extract_content()).expect("resume payload is JSON");
	// Resume reuses the registered job's role and workdir, not the caller's.
	assert_eq!(payload["id"].as_str(), Some("tap-unit-resume"));
	assert_eq!(payload["role"].as_str(), Some("lawyer:us"));
	assert_eq!(payload["workdir"].as_str(), Some("/tmp"));

	let status = crate::session::context::with_session_id(session_id.clone(), async {
		wait_terminal_status("tap-unit-resume").await
	})
	.await;
	assert!(
		matches!(status, TapJobStatus::Failed | TapJobStatus::Cancelled),
		"status: {status:?}"
	);
	tap_runs::clear_for_session(&session_id);
}

#[tokio::test]
#[serial]
async fn run_without_workdir_defaults_to_thread_working_directory() {
	let session_id = "__tapunit_cwd_default".to_string();
	let (out, expected) = crate::session::context::with_session_id(session_id.clone(), async {
		let expected = crate::mcp::get_thread_working_directory()
			.to_string_lossy()
			.to_string();
		let out = execute_tap_command(
			&tap_call(json!({
				"action": "run",
				"role": "developer:general",
				"prompt": "do a tiny thing",
			})),
			&unit_config(),
		)
		.await
		.expect("dispatch");
		(out, expected)
	})
	.await;

	assert!(!out.is_error(), "content: {}", out.extract_content());
	let payload: serde_json::Value =
		serde_json::from_str(&out.extract_content()).expect("started payload is JSON");
	assert_eq!(payload["workdir"].as_str(), Some(expected.as_str()));

	let id = payload["id"].as_str().expect("id").to_string();
	let status = crate::session::context::with_session_id(session_id.clone(), async {
		wait_terminal_status(&id).await
	})
	.await;
	assert!(
		matches!(status, TapJobStatus::Failed | TapJobStatus::Cancelled),
		"status: {status:?}"
	);
	tap_runs::clear_for_session(&session_id);
}

#[tokio::test]
#[serial]
async fn run_outside_session_scope_still_starts_and_reports() {
	// Outside a session scope the job is not registered (register_job
	// no-ops), but the run still starts and reports its payload; the
	// background turn executes on the bare future (no with_session_id
	// wrapper). The child is the test binary, which rejects `--name` and
	// exits immediately, so nothing outlives the test.
	let out = execute_tap_command(
		&tap_call(json!({
			"action": "run",
			"role": "lawyer:us",
			"prompt": "do a tiny thing",
			"workdir": "/tmp",
		})),
		&unit_config(),
	)
	.await
	.expect("dispatch");

	assert!(!out.is_error(), "content: {}", out.extract_content());
	let payload: serde_json::Value =
		serde_json::from_str(&out.extract_content()).expect("started payload is JSON");
	assert!(payload["id"].as_str().is_some_and(|id| !id.is_empty()));
	assert_eq!(payload["role"].as_str(), Some("lawyer:us"));
	assert_eq!(payload["workdir"].as_str(), Some("/tmp"));
}
