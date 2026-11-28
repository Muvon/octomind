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
//! resume-while-busy guard, the capability no-match payload, and fresh runs
//! matched to an agent installed in a temp tap.

use super::*;
use serial_test::serial;

fn tap_call(params: serde_json::Value) -> McpToolCall {
	McpToolCall {
		tool_name: "tap".to_string(),
		parameters: params,
		tool_id: "t-tap-unit".to_string(),
	}
}

#[tokio::test]
#[serial]
async fn a_finished_workflow_banks_its_cost_on_the_session_that_ran_it() {
	// The workflow run reports its total and tap read only its output: the steps
	// billed the account while the session that ran them showed none of it.
	crate::session::context::with_session_id("wf-cost-session".to_string(), async {
		crate::session::external_spend::take();
		let (status, content) = workflow_handback(
			"digest",
			"tap-workflow-digest-000001",
			Some(Ok(crate::workflow::spawn::RunOutcome {
				output: "the brief".to_string(),
				cost: Some(0.42),
			})),
		);
		assert_eq!(status, TapJobStatus::Done);
		assert!(
			content.contains("completed") && content.contains("the brief"),
			"got: {content}"
		);
		let banked = crate::session::external_spend::take();
		assert!((banked - 0.42).abs() < 1e-9, "expected 0.42 banked, got {banked}");
	})
	.await;
}

fn unit_config() -> Config {
	let mut config: Config =
		toml::from_str(include_str!("../../../config-templates/default.toml")).expect("parse default config template");
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

/// Install one agent in the default tap of `data_dir`.
fn install_unit_agent(data_dir: &std::path::Path, name: &str, title: &str, description: &str) {
	let agent_dir = data_dir
		.join("taps")
		.join("muvon")
		.join("octomind-tap")
		.join("agents")
		.join(name);
	std::fs::create_dir_all(&agent_dir).expect("create agent dir");
	std::fs::write(
		agent_dir.join("default.toml"),
		format!("# Title: {title}\n# Description: {description}\n[[roles]]\nname = \"{name}\"\n"),
	)
	.expect("write agent file");
}

/// A task and the only installed agent's description say the same thing, so
/// the match does not depend on where the cosine threshold sits.
const RUST_TASK: &str = "Debug a failing Rust build and fix the compile errors";
fn install_rust_agent(data_dir: &std::path::Path) {
	install_unit_agent(
		data_dir,
		"developer",
		"Rust build debugger",
		"Debugs a failing Rust build and fixes the compile errors.",
	);
}

fn register_unit_job(id: &str, role: &str, status: TapJobStatus) {
	let (cancel_tx, _cancel_rx) = watch::channel(false);
	tap_runs::register_job(TapJob {
		mode: Default::default(),
		id: id.to_string(),
		role: role.to_string(),
		workdir: "/tmp".to_string(),
		started_at: SystemTime::UNIX_EPOCH,
		status: Arc::new(RwLock::new(status)),
		error: None,
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
	for key in ["prompt", "role", "session", "workdir"] {
		let prop = props.get(key).unwrap_or_else(|| panic!("{key} missing"));
		assert_eq!(prop.get("type").and_then(|t| t.as_str()), Some("string"), "{key} type");
		assert!(
			prop.get("description")
				.and_then(|d| d.as_str())
				.is_some_and(|d| !d.is_empty()),
			"{key} description"
		);
	}
	// An agent is matched from the task or named by `role`; there is no search action.
	assert!(!props.contains_key("intent"), "intent must not be a parameter");
	assert_eq!(
		function.parameters.get("required").and_then(|r| r.as_array()),
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
		"`capability`",
		"`role`",
		"session",
		"workdir",
	] {
		assert!(function.description.contains(phrase), "description missing '{phrase}'");
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
	let result = execute_tap_command(&tap_call(json!({"action": "stop", "session": 123})), &unit_config())
		.await
		.expect("dispatch");
	assert!(result.is_error());
	assert!(result
		.extract_content()
		.contains("Missing required parameter 'session'"));
}

#[tokio::test]
async fn send_validates_parameters_and_preserves_prompt_text() {
	let config = unit_config();
	for params in [
		json!({"action": "send", "prompt": "hello"}),
		json!({"action": "send", "session": 1, "prompt": "hello"}),
		json!({"action": "send", "session": "  ", "prompt": "hello"}),
		json!({"action": "send", "session": "run"}),
		json!({"action": "send", "session": "run", "prompt": false}),
		json!({"action": "send", "session": "run", "prompt": "  "}),
	] {
		assert!(execute_tap_command(&tap_call(params), &config)
			.await
			.unwrap()
			.is_error());
	}
	let parent = "tap-send-tool".to_string();
	crate::session::context::with_session_id(parent.clone(), async {
		tap_runs::init_for_session();
		register_unit_job("run", "developer:general", TapJobStatus::Running);
		let mut prompts = tap_runs::open_prompt_channel("run").unwrap();
		let prompt = "  Keep this formatting.\nCheck cancellation too.  ";
		let result = execute_tap_command(
			&tap_call(json!({"action": "send", "session": "run", "prompt": prompt})),
			&config,
		)
		.await
		.unwrap();
		assert!(!result.is_error(), "{}", result.extract_content());
		assert_eq!(prompts.recv().await.as_deref(), Some(prompt));
		assert_eq!(tap_runs::find_job("run").unwrap().status, TapJobStatus::Running);
		let missing = execute_tap_command(
			&tap_call(json!({"action": "send", "session": "missing", "prompt": "hello"})),
			&config,
		)
		.await
		.unwrap();
		assert!(missing.is_error());
		tap_runs::clear_for_session(&parent);
	})
	.await;
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
	assert!(content.contains("already running"), "content: {content}");
	assert!(content.contains("use send to queue a prompt"), "content: {content}");
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
	let payload: serde_json::Value = serde_json::from_str(&result.extract_content()).expect("json payload");
	assert_eq!(payload["activated_capabilities"].as_array().map(Vec::len), Some(0));
	assert_eq!(payload["message"], "No capability matched the prompt.");
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
			mode: Default::default(),
			id: "tap-unit-000001".to_string(),
			role: "lawyer:us".to_string(),
			workdir: "/tmp/unit".to_string(),
			started_at: SystemTime::UNIX_EPOCH,
			status,
			error: None,
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
	assert!(id.starts_with("tap-developer-"), "id: {id}");
	let suffix = id.rsplit('-').next().expect("suffix");
	assert_eq!(suffix.len(), 6, "id: {id}");
	assert!(suffix.chars().all(|c| c.is_ascii_hexdigit()), "suffix: {suffix}");
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
	let guard = TempDataDir::new();
	install_rust_agent(guard.path());
	let session_id = "__tapunit_fresh".to_string();
	let out = crate::session::context::with_session_id(session_id.clone(), async {
		execute_tap_command(
			&tap_call(json!({
				"action": "run",
				"prompt": RUST_TASK,
				"workdir": "/tmp",
			})),
			&unit_config(),
		)
		.await
		.expect("dispatch")
	})
	.await;

	assert!(!out.is_error(), "content: {}", out.extract_content());
	let payload: serde_json::Value = serde_json::from_str(&out.extract_content()).expect("started payload is JSON");
	let id = payload["id"].as_str().expect("id").to_string();
	assert!(id.starts_with("tap-developer-"), "id: {id}");
	assert_eq!(payload["role"].as_str(), Some("developer"));
	assert_eq!(payload["title"].as_str(), Some("Rust build debugger"));
	assert_eq!(payload["workdir"].as_str(), Some("/tmp"));

	let status =
		crate::session::context::with_session_id(session_id.clone(), async { wait_terminal_status(&id).await }).await;
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
	let payload: serde_json::Value = serde_json::from_str(&out.extract_content()).expect("resume payload is JSON");
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
	let guard = TempDataDir::new();
	install_rust_agent(guard.path());
	let session_id = "__tapunit_cwd_default".to_string();
	let (out, expected) = crate::session::context::with_session_id(session_id.clone(), async {
		let expected = crate::mcp::get_thread_working_directory().to_string_lossy().to_string();
		let out = execute_tap_command(
			&tap_call(json!({
				"action": "run",
				"prompt": RUST_TASK,
			})),
			&unit_config(),
		)
		.await
		.expect("dispatch");
		(out, expected)
	})
	.await;

	assert!(!out.is_error(), "content: {}", out.extract_content());
	let payload: serde_json::Value = serde_json::from_str(&out.extract_content()).expect("started payload is JSON");
	assert_eq!(payload["workdir"].as_str(), Some(expected.as_str()));

	let id = payload["id"].as_str().expect("id").to_string();
	let status =
		crate::session::context::with_session_id(session_id.clone(), async { wait_terminal_status(&id).await }).await;
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
	let guard = TempDataDir::new();
	install_rust_agent(guard.path());
	let out = execute_tap_command(
		&tap_call(json!({
			"action": "run",
			"prompt": RUST_TASK,
			"workdir": "/tmp",
		})),
		&unit_config(),
	)
	.await
	.expect("dispatch");

	assert!(!out.is_error(), "content: {}", out.extract_content());
	let payload: serde_json::Value = serde_json::from_str(&out.extract_content()).expect("started payload is JSON");
	assert!(payload["id"].as_str().is_some_and(|id| !id.is_empty()));
	assert_eq!(payload["role"].as_str(), Some("developer"));
	assert_eq!(payload["workdir"].as_str(), Some("/tmp"));
}

// --- run: no match, named role, tap-enumeration failure ----------------------

const FOREIGN_TASK: &str = "Plan a three-day vegetarian menu for a family of four";

#[tokio::test]
#[serial]
async fn run_with_an_unrelated_task_starts_nothing_and_names_the_closest_agents() {
	let guard = TempDataDir::new();
	install_rust_agent(guard.path());
	let session_id = "__tapunit_no_match".to_string();
	let (out, runs) = crate::session::context::with_session_id(session_id.clone(), async {
		let out = execute_tap_command(
			&tap_call(json!({
				"action": "run",
				"prompt": FOREIGN_TASK,
			})),
			&unit_config(),
		)
		.await
		.expect("dispatch");
		(out, tap_runs::list_jobs().len())
	})
	.await;
	tap_runs::clear_for_session(&session_id);

	assert!(out.is_error());
	let content = out.extract_content();
	assert!(
		content.starts_with("No agent was matched to this task."),
		"content: {content}"
	);
	// The name the caller may pass back as `role`, with what that agent does.
	assert!(
		content
			.contains("- developer — Rust build debugger. Debugs a failing Rust build and fixes the compile errors."),
		"content: {content}"
	);
	assert!(content.contains("with `role` set to its name"), "content: {content}");
	assert_eq!(runs, 0);
}

#[tokio::test]
#[serial]
async fn run_with_role_starts_that_agent_for_a_task_matching_refuses() {
	let guard = TempDataDir::new();
	install_rust_agent(guard.path());
	install_unit_agent(
		guard.path(),
		"lead",
		"Developer lead",
		"Splits a development task between the developer agents.",
	);
	let session_id = "__tapunit_named_role".to_string();
	// A role names that agent.
	for (role, started) in [("developer", "developer"), ("lead", "lead")] {
		let out = crate::session::context::with_session_id(session_id.clone(), async {
			execute_tap_command(
				&tap_call(json!({
					"action": "run",
					"prompt": FOREIGN_TASK,
					"role": role,
					"workdir": "/tmp",
				})),
				&unit_config(),
			)
			.await
			.expect("dispatch")
		})
		.await;

		assert!(!out.is_error(), "content: {}", out.extract_content());
		let payload: serde_json::Value = serde_json::from_str(&out.extract_content()).expect("started payload is JSON");
		assert_eq!(payload["role"].as_str(), Some(started));
		let id = payload["id"].as_str().expect("id").to_string();
		let status =
			crate::session::context::with_session_id(session_id.clone(), async { wait_terminal_status(&id).await })
				.await;
		assert!(
			matches!(status, TapJobStatus::Failed | TapJobStatus::Cancelled),
			"status: {status:?}"
		);
	}
	tap_runs::clear_for_session(&session_id);
}

#[tokio::test]
#[serial]
async fn run_with_a_role_outside_reach_starts_nothing_and_names_the_closest_agents() {
	let guard = TempDataDir::new();
	install_rust_agent(guard.path());
	let session_id = "__tapunit_unknown_role".to_string();
	let (out, runs) = crate::session::context::with_session_id(session_id.clone(), async {
		let out = execute_tap_command(
			&tap_call(json!({
				"action": "run",
				"prompt": RUST_TASK,
				"role": "lawyer",
			})),
			&unit_config(),
		)
		.await
		.expect("dispatch");
		(out, tap_runs::list_jobs().len())
	})
	.await;
	tap_runs::clear_for_session(&session_id);

	assert!(out.is_error());
	let content = out.extract_content();
	// The task alone would match `developer`; a wrong name is refused, not rerouted.
	assert!(
		content.starts_with("`lawyer` is not an agent you can reach."),
		"content: {content}"
	);
	assert!(content.contains("You can reach: developer."), "content: {content}");
	assert_eq!(runs, 0);
}

#[tokio::test]
#[serial]
async fn run_reports_tap_enumeration_failures() {
	let guard = TempDataDir::new();
	// A malformed taps.toml breaks tap loading; run must surface the
	// enumeration failure instead of reporting that no agent fits.
	std::fs::write(guard.path().join("taps.toml"), "not [valid toml").expect("write malformed taps.toml");

	let result = execute_tap_command(
		&tap_call(json!({"action": "run", "prompt": "debug a rust build"})),
		&unit_config(),
	)
	.await
	.expect("dispatch");
	assert!(result.is_error());
	let content = result.extract_content();
	assert!(content.contains("Failed to enumerate tap agents"), "content: {content}");
}

// ---------------------------------------------------------------------------
// workflow
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn workflow_name_without_input_shows_definition_and_runs_nothing() {
	let guard = TempDataDir::new();
	// Pre-create the default tap dir so `load_taps` never tries to clone it.
	let wf_dir = guard
		.path()
		.join("taps")
		.join("muvon")
		.join("octomind-tap")
		.join("workflows");
	std::fs::create_dir_all(&wf_dir).expect("workflows dir");
	let toml = "description = \"watch a page\"\n[[steps]]\nname = \"s\"\nrole = \"developer\"\nprompt = \"Watch:\\n{{input}}\"\n";
	std::fs::write(wf_dir.join("watch-page.toml"), toml).expect("write workflow");
	let before = tap_runs::list_jobs().len();

	let result = execute_tap_command(
		&tap_call(json!({"action": "workflow", "name": "watch-page"})),
		&unit_config(),
	)
	.await
	.expect("dispatch");
	assert!(!result.is_error(), "{}", result.extract_content());
	let body: serde_json::Value = serde_json::from_str(&result.extract_content()).expect("json body");
	assert_eq!(body["name"], "watch-page");
	assert_eq!(body["source_tap"], "muvon/tap");
	assert_eq!(body["definition"], toml);
	assert_eq!(tap_runs::list_jobs().len(), before, "nothing was launched");

	let missing = execute_tap_command(&tap_call(json!({"action": "workflow", "name": "nope"})), &unit_config())
		.await
		.expect("dispatch");
	assert!(missing.is_error());
	assert!(missing.extract_content().contains("nope"));
}

#[tokio::test]
#[serial]
async fn cross_workspace_tap_does_not_open_the_parent_log() {
	use crate::session::delegation_context::DISPATCH;

	let guard = TempDataDir::new();
	install_rust_agent(guard.path());
	let parent = tempfile::tempdir().unwrap();
	let other = tempfile::tempdir().unwrap();
	let session = "delegation-other-workspace".to_owned();
	crate::session::context::set_session_workdir(&session, parent.path().to_path_buf());
	crate::session::context::with_session_id(session.clone(), async {
		crate::session::context::init_session_services("assistant", Default::default());
		let source = crate::session::logger::get_session_log_file(&session).unwrap();
		assert!(!source.exists());
		let config = unit_config();
		let call = tap_call(json!({
			"action": "run",
			"role": "developer",
			"prompt": RUST_TASK,
			"workdir": other.path(),
		}));
		let result = DISPATCH
			.scope(Some((source, None)), execute_tap_command(&call, &config))
			.await
			.unwrap();
		assert!(!result.is_error(), "{}", result.extract_content());
		let started: serde_json::Value = serde_json::from_str(&result.extract_content()).unwrap();
		assert_eq!(started["context"], "unavailable");
		wait_terminal_status(started["id"].as_str().unwrap()).await;
	})
	.await;
	crate::session::context::cleanup_session(&session);
}

// ---------------------------------------------------------------------------
// `readonly` parameter
// ---------------------------------------------------------------------------

#[test]
fn the_schema_declares_readonly_as_a_boolean() {
	let function = get_tap_function();
	assert_eq!(function.parameters["properties"]["readonly"]["type"], "boolean");
}

#[tokio::test]
async fn a_readonly_value_that_is_not_a_boolean_is_a_tool_error() {
	let config = unit_config();
	for bad in [json!("true"), json!(1), json!(null)] {
		let call = tap_call(json!({"action": "run", "prompt": "do the thing", "readonly": bad}));
		let result = execute_tap_command(&call, &config).await.unwrap();
		assert!(result.is_error(), "readonly={bad} must be refused");
		assert!(result.extract_content().contains("readonly: boolean required"));
	}
}

#[tokio::test]
#[serial]
async fn read_only_cannot_be_asked_of_a_run_that_started_writable() {
	let session_id = "__tapunit_readonly_resume".to_string();
	let result = crate::session::context::with_session_id(session_id.clone(), async {
		register_unit_job("writable-run", "developer", TapJobStatus::Done);
		execute_tap_command(
			&tap_call(json!({"action": "run", "session": "writable-run", "prompt": "again", "readonly": true})),
			&unit_config(),
		)
		.await
		.expect("dispatch")
	})
	.await;
	let still_done = crate::session::context::with_session_id(session_id.clone(), async {
		tap_runs::find_job("writable-run").map(|job| job.status)
	})
	.await;
	tap_runs::clear_for_session(&session_id);

	assert!(result.is_error());
	assert!(
		result.extract_content().contains("started writable"),
		"{}",
		result.extract_content()
	);
	// Refused before the run was claimed.
	assert_eq!(still_done, Some(TapJobStatus::Done));
}

#[tokio::test]
#[serial]
async fn authored_role_is_private_but_its_run_resumes_the_selected_manifest() {
	let data = TempDataDir::new();
	install_rust_agent(data.path());
	let config = unit_config();
	let session = "tap-author-owner".to_owned();
	let tag = "personal:review";
	let original = "Audit the API. {{ENV:OCTOMIND_TEST_MUST_NOT_BE_EXPANDED}}";
	let run_id = crate::session::context::with_session_id(session.clone(), async {
		crate::session::context::init_session_services("local", SessionMode::default());
		let created = execute_tap_command(
			&tap_call(json!({"action":"create","name":"review","system":original,"description":"Review an API"})),
			&config,
		)
		.await
		.unwrap();
		assert!(!created.is_error(), "{}", created.extract_content());
		assert_eq!(
			serde_json::from_str::<serde_json::Value>(&created.extract_content()).unwrap()["role"],
			tag
		);
		let unauthorized = execute_tap_command(&tap_call(json!({"action":"persist","role":tag})), &config)
			.await
			.unwrap();
		assert!(unauthorized.is_error());
		assert!(!crate::agent::manifest::catalog(Some("other-session"), None)
			.unwrap()
			.contains_key(tag));
		let run = execute_tap_command(&tap_call(json!({"action":"run","role":tag,"prompt":"check"})), &config)
			.await
			.unwrap();
		assert!(!run.is_error(), "{}", run.extract_content());
		let started: serde_json::Value = serde_json::from_str(&run.extract_content()).unwrap();
		let id = started["id"].as_str().unwrap().to_owned();
		wait_terminal_status(&id).await;
		let mine = crate::agent::manifest::private_role(&session, tag).unwrap();
		let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
		crate::session::context::set_notification_sender_for_session(&session, tx);
		crate::mcp::elicitation::set_answerer(&session, Some(crate::mcp::elicitation::Answerer::Client));
		let approve = async {
			let frame = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
				.await
				.unwrap()
				.unwrap();
			let crate::websocket::ServerMessage::Elicitation(q) = frame else {
				panic!("expected approval")
			};
			assert!(crate::mcp::elicitation::resolve(
				&session,
				&q.id,
				rmcp::model::ElicitResult::new(rmcp::model::ElicitationAction::Accept)
					.with_content(json!({"approve":true}))
			));
		};
		let persist = tap_call(json!({"action":"persist","role":tag}));
		let (saved, ()) = tokio::join!(execute_tap_command(&persist, &config), approve);
		let saved = saved.unwrap();
		assert!(!saved.is_error(), "{}", saved.extract_content());
		let path = crate::agent::taps::personal_tap().agent_manifest(tag).unwrap();
		let public = crate::agent::manifest::catalog(Some("other-session"), None).unwrap();
		assert_eq!(public[tag].raw, mine.raw);
		assert!(mine.persist().is_err(), "cannot overwrite on a second promotion");
		std::fs::write(path, "# changed after the run").unwrap();
		crate::session::context::cleanup_session(&session);
		id
	})
	.await;
	assert!(crate::agent::manifest::private_role(&session, tag).is_err());
	let pinned = crate::agent::manifest::Manifest::read_run(&run_id, tag).unwrap();
	let mut shadow = config.clone();
	shadow
		.roles
		.push(toml::from_str("name='personal:review'\nsystem='wrong role'\nwelcome=''").unwrap());
	shadow.build_role_map();
	let (resolved, role) = crate::agent::resolver::resolve_manifest(pinned, &shadow, None)
		.await
		.unwrap();
	assert_eq!(role, tag);
	assert_eq!(resolved.role_map[tag].config.system, original);
	let merged = resolved
		.get_merged_config_for_interactive_role(tag)
		.get_merged_config_for_role(tag);
	assert!(merged.mcp.servers.is_empty());
	assert!(crate::mcp::get_available_functions(&merged).await.is_empty());
	let guessed = crate::mcp::execute_tool_call(&tap_call(json!({"action":"workflow"})), &merged, None)
		.await
		.unwrap()
		.0;
	assert!(guessed.is_error());
}
