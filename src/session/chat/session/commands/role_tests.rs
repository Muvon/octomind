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

//! Handler-level tests for the `/role` session command: bare listing,
//! plain-role validation, same-role no-op, tap-tag resolution failure under a
//! throwaway data dir, graceful revert on a file-less session, and a full
//! switch persisted to a real session file.

use super::*;
use serial_test::serial;

fn template_config() -> Config {
	let mut config: Config =
		toml::from_str(include_str!("../../../../../config-templates/default.toml"))
			.expect("parse default config template");
	config.build_role_map();
	config
}

struct TestDataDir {
	previous: Option<std::ffi::OsString>,
	_dir: tempfile::TempDir,
}

impl TestDataDir {
	fn new() -> Self {
		let dir = tempfile::tempdir().expect("temporary data dir");
		let previous = std::env::var_os("OCTOMIND_DATA_DIR");
		std::env::set_var("OCTOMIND_DATA_DIR", dir.path());
		Self {
			previous,
			_dir: dir,
		}
	}
}

impl Drop for TestDataDir {
	fn drop(&mut self) {
		match self.previous.take() {
			Some(value) => std::env::set_var("OCTOMIND_DATA_DIR", value),
			None => std::env::remove_var("OCTOMIND_DATA_DIR"),
		}
	}
}

/// Run one `/role` invocation and return its typed output.
async fn run(session: &mut ChatSession, config: &mut Config, params: &[&str]) -> CommandOutput {
	let result = handle_role(session, config, params)
		.await
		.unwrap_or_else(|e| panic!("role {params:?} errored: {e}"));
	let CommandResult::HandledWithOutput(output) = result else {
		panic!("expected typed output");
	};
	*output
}

#[tokio::test]
async fn test_bare_command_shows_current_and_available_roles() {
	let mut session = ChatSession::for_tests(Vec::new());
	session.role = "task_refiner".to_string();
	let mut config = template_config();

	let output = run(&mut session, &mut config, &[]).await;
	let CommandOutput::Role {
		old_role,
		new_role,
		current_role,
		available_roles,
		changed,
		..
	} = output
	else {
		panic!("expected Role output");
	};
	assert_eq!(old_role, None);
	assert_eq!(new_role, "task_refiner");
	assert_eq!(current_role.as_deref(), Some("task_refiner"));
	assert_eq!(changed, false);
	let roles = available_roles.expect("available roles");
	for expected in ["assistant", "task_refiner", "task_researcher", "reduce"] {
		assert!(roles.contains(&expected.to_string()), "roles: {roles:?}");
	}
}

#[tokio::test]
async fn test_invalid_plain_role_is_rejected_up_front() {
	let mut session = ChatSession::for_tests(Vec::new());
	session.role = "task_refiner".to_string();
	let mut config = template_config();

	let output = run(&mut session, &mut config, &["no-such-role"]).await;
	let CommandOutput::Error { error, context } = output else {
		panic!("expected Error output");
	};
	assert_eq!(error, "Invalid role: no-such-role");
	let context = context.expect("context");
	let roles = context["available_roles"]
		.as_array()
		.expect("available_roles array");
	assert!(
		roles.iter().any(|r| r == "assistant"),
		"context must list valid roles: {roles:?}"
	);
	assert_eq!(session.role, "task_refiner", "role must stay unchanged");
}

#[tokio::test]
async fn test_same_role_is_a_no_op() {
	let mut session = ChatSession::for_tests(Vec::new());
	session.role = "assistant".to_string();
	let mut config = template_config();

	let output = run(&mut session, &mut config, &["assistant"]).await;
	let CommandOutput::Role {
		old_role,
		new_role,
		current_role,
		changed,
		..
	} = output
	else {
		panic!("expected Role output");
	};
	assert_eq!(
		old_role, None,
		"no switch happened, nothing to report as old"
	);
	assert_eq!(new_role, "assistant");
	assert_eq!(current_role.as_deref(), Some("assistant"));
	assert_eq!(changed, false);
}

#[tokio::test]
#[serial]
async fn test_tap_tag_resolution_failure_leaves_session_untouched() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	// Throwaway data dir: no taps installed, no manifest cache — the resolver
	// must fail fast offline instead of hitting the network.
	let _data = TestDataDir::new();
	let mut session = ChatSession::for_tests(Vec::new());
	session.role = "task_refiner".to_string();
	let mut config = template_config();

	let output = run(
		&mut session,
		&mut config,
		&["no-such-domain:no-such-variant"],
	)
	.await;
	let CommandOutput::Error { error, context } = output else {
		panic!("expected Error output");
	};
	assert!(
		error.starts_with("Failed to resolve role 'no-such-domain:no-such-variant'"),
		"error: {error}"
	);
	assert_eq!(context, None);
	assert_eq!(session.role, "task_refiner", "role must stay unchanged");
}

#[tokio::test]
async fn test_switch_without_session_file_fails_and_reverts() {
	let mut session = ChatSession::for_tests(Vec::new());
	session.role = "task_refiner".to_string();
	let mut config = template_config();
	let servers_before = config.mcp.servers.len();
	let (model_before, temperature_before) = (session.model.clone(), session.temperature);

	let output = run(&mut session, &mut config, &["assistant"]).await;
	let CommandOutput::Error { error, context } = output else {
		panic!("expected Error output");
	};
	assert!(error.starts_with("Failed to switch role"), "error: {error}");
	assert_eq!(
		context.as_ref().and_then(|c| c["reverted"].as_bool()),
		Some(true),
		"must report the revert"
	);
	// Nothing half-switched: session and config are back to their snapshots
	assert_eq!(session.role, "task_refiner");
	assert_eq!(session.model, model_before);
	assert_eq!(session.temperature, temperature_before);
	assert_eq!(config.mcp.servers.len(), servers_before);
}

#[tokio::test]
#[serial]
async fn test_switch_with_session_file_persists_new_role() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let data = TestDataDir::new();
	let sessions_dir = data._dir.path().join("sessions");
	std::fs::create_dir_all(&sessions_dir).expect("create sessions dir");

	let mut session = ChatSession::for_tests(Vec::new());
	session.role = "task_refiner".to_string();
	session.session.info.name = "role-switch-cmd-test".to_string();
	session.session.session_file = Some(sessions_dir.join("role-switch-cmd-test.jsonl.zst"));
	let mut config = template_config();

	let output = run(&mut session, &mut config, &["assistant"]).await;
	let CommandOutput::Role {
		old_role,
		new_role,
		changed,
		saved,
		save_error,
		..
	} = output
	else {
		panic!("expected Role output");
	};
	assert_eq!(old_role.as_deref(), Some("task_refiner"));
	assert_eq!(new_role, "assistant");
	assert!(changed);
	assert_eq!(saved, Some(true), "save_error: {save_error:?}");
	assert_eq!(save_error, None);

	assert_eq!(session.role, "assistant");
	// The rebuilt system prompt replaced the (previously empty) message list
	let first = session.session.messages.first().expect("system message");
	assert_eq!(first.role, "system");
	assert!(!first.content.is_empty());
	// The committed config is the merged view for the new role
	assert!(
		config.mcp.servers.iter().any(|s| s.name() == "core"),
		"merged config must carry the builtin servers"
	);
	// The switch persisted a SUMMARY snapshot to the session file
	assert!(
		session.session.session_file.as_ref().unwrap().exists(),
		"session file must exist after save"
	);
}
