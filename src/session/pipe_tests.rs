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

/// Install `toml` as `.agents/guardrails.toml` in a fresh temp workdir,
/// register the session, and run the pipe pipeline once. Each call uses a
/// unique session id — the guardrails registries are process globals shared
/// by parallel tests.
async fn run_with_rules(
	label: &str,
	toml: &str,
	role: &str,
	input: &str,
	first_message_processed: bool,
) -> Result<Option<String>> {
	let sid: SessionId = format!("pipe-tests-{label}-{}", std::process::id());
	let dir = tempfile::tempdir().expect("temp workdir");
	let agents_dir = dir.path().join(".agents");
	std::fs::create_dir_all(&agents_dir).expect("create .agents dir");
	std::fs::write(agents_dir.join("guardrails.toml"), toml).expect("write guardrails.toml");

	let out = crate::session::context::with_session_id(sid.clone(), async {
		crate::session::context::set_session_workdir(&sid, dir.path().to_path_buf());
		crate::session::guardrails::init_for_session();
		run_pipe(&sid, role, input, first_message_processed).await
	})
	.await;
	crate::session::guardrails::clear_for_session(&sid);
	out
}

#[test]
fn pipe_timeout_is_300_seconds() {
	assert_eq!(PIPE_TIMEOUT_SECS, 300);
}

#[tokio::test]
async fn run_pipe_returns_none_when_the_session_has_no_rules() {
	let sid: SessionId = format!("pipe-tests-no-rules-{}", std::process::id());
	let out = run_pipe(&sid, "developer", "hello", false).await;
	crate::session::guardrails::clear_for_session(&sid);
	assert_eq!(out.expect("no rules is not an error"), None);
}

#[tokio::test]
async fn run_pipe_skips_pipes_bound_to_other_roles() {
	let toml = r#"
[[pipe]]
name = "review-only"
command = "never-invoked.sh"
roles = ["reviewer"]
"#;
	let out = run_with_rules("role-filter", toml, "developer", "deploy", false).await;
	assert_eq!(out.expect("a filter miss is not an error"), None);
}

#[tokio::test]
async fn run_pipe_skips_pipes_whose_match_regex_does_not_hit() {
	let toml = r#"
[[pipe]]
name = "deploy-gate"
command = "never-invoked.sh"
match = "^deploy"
"#;
	let out = run_with_rules("regex-miss", toml, "developer", "just chatting", false).await;
	assert_eq!(out.expect("a filter miss is not an error"), None);
}

#[tokio::test]
async fn run_pipe_skips_first_pipes_after_the_first_message() {
	let toml = r#"
[[pipe]]
name = "onboarding"
command = "never-invoked.sh"
when = "first"
"#;
	let out = run_with_rules("when-first", toml, "developer", "hello", true).await;
	assert_eq!(out.expect("a filter miss is not an error"), None);
}

#[tokio::test]
async fn run_pipe_rejects_multiple_matching_pipes() {
	let toml = r#"
[[pipe]]
name = "one"
command = "never-invoked.sh"

[[pipe]]
name = "two"
command = "also-never-invoked.sh"
"#;
	let error = run_with_rules("multi-match", toml, "developer", "hello", false)
		.await
		.expect_err("two matches must be an error");
	assert!(error
		.to_string()
		.contains("Multiple [[pipe]] entries matched"));
}

#[tokio::test]
async fn run_pipe_reports_spawn_failure_for_a_missing_script() {
	let toml = r#"
[[pipe]]
name = "ghost"
command = "definitely-missing-script-9f3a.sh"
"#;
	let error = run_with_rules("spawn-fail", toml, "developer", "hello", false)
		.await
		.expect_err("a missing script must be an error");
	assert!(error.to_string().contains("failed to spawn"));
}

#[cfg(unix)]
fn write_script(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
	use std::os::unix::fs::PermissionsExt;
	let path = dir.join(name);
	std::fs::write(&path, body).expect("script writes");
	std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
		.expect("script becomes executable");
	path
}

#[cfg(unix)]
#[tokio::test]
#[serial_test::serial]
async fn run_pipe_feeds_stdin_and_returns_stdout() {
	let sid: SessionId = format!("pipe-tests-echo-{}", std::process::id());
	let dir = tempfile::tempdir().expect("temp workdir");
	let script = write_script(dir.path(), "upper.sh", "#!/bin/sh\ntr a-z A-Z\n");
	let toml = format!(
		"[[pipe]]\nname = \"upper\"\ncommand = {:?}\n",
		script.display().to_string()
	);
	std::fs::create_dir_all(dir.path().join(".agents")).expect("create .agents dir");
	std::fs::write(dir.path().join(".agents/guardrails.toml"), toml)
		.expect("write guardrails.toml");

	let out = crate::session::context::with_session_id(sid.clone(), async {
		crate::session::context::set_session_workdir(&sid, dir.path().to_path_buf());
		crate::session::guardrails::init_for_session();
		run_pipe(&sid, "developer", "hello pipe", false).await
	})
	.await;
	crate::session::guardrails::clear_for_session(&sid);

	assert_eq!(
		out.expect("echo pipe succeeds"),
		Some("HELLO PIPE".to_string())
	);
}

#[cfg(unix)]
#[tokio::test]
#[serial_test::serial]
async fn run_pipe_hard_stops_on_a_nonzero_exit() {
	let sid: SessionId = format!("pipe-tests-reject-{}", std::process::id());
	let dir = tempfile::tempdir().expect("temp workdir");
	let script = write_script(
		dir.path(),
		"reject.sh",
		"#!/bin/sh\necho 'forbidden word detected' >&2\nexit 3\n",
	);
	let toml = format!(
		"[[pipe]]\nname = \"reject\"\ncommand = {:?}\n",
		script.display().to_string()
	);
	std::fs::create_dir_all(dir.path().join(".agents")).expect("create .agents dir");
	std::fs::write(dir.path().join(".agents/guardrails.toml"), toml)
		.expect("write guardrails.toml");

	let out = crate::session::context::with_session_id(sid.clone(), async {
		crate::session::context::set_session_workdir(&sid, dir.path().to_path_buf());
		crate::session::guardrails::init_for_session();
		run_pipe(&sid, "developer", "hello", false).await
	})
	.await;
	crate::session::guardrails::clear_for_session(&sid);

	let error = out.expect_err("a non-zero exit must hard-stop");
	assert!(error.to_string().contains("rejected input"));
	assert!(error.to_string().contains("forbidden word detected"));
}
#[cfg(unix)]
#[tokio::test]
#[serial_test::serial]
#[ignore = "waits out the real 300s PIPE_TIMEOUT_SECS wall clock; run with `cargo test -- --ignored`"]
async fn run_pipe_times_out_and_kills_a_script_that_never_exits() {
	// No injection point exists for PIPE_TIMEOUT_SECS (private const, 300s)
	// and tokio's `test-util` feature — which a paused-clock shortcut needs —
	// is not enabled, so this genuinely waits 300 seconds. The path is real:
	// spawn → script hangs → timeout error → kill_on_drop reaps the child.
	let sid: SessionId = format!("pipe-tests-timeout-{}", std::process::id());
	let dir = tempfile::tempdir().expect("temp workdir");
	// `exec` so the spawned process IS sleep: kill_on_drop reaps it directly
	// instead of orphaning it under a waiting shell.
	let script = write_script(dir.path(), "hang.sh", "#!/bin/sh\nexec sleep 400\n");
	let toml = format!(
		"[[pipe]]\nname = \"hang\"\ncommand = {:?}\n",
		script.display().to_string()
	);
	std::fs::create_dir_all(dir.path().join(".agents")).expect("create .agents dir");
	std::fs::write(dir.path().join(".agents/guardrails.toml"), toml)
		.expect("write guardrails.toml");

	let out = crate::session::context::with_session_id(sid.clone(), async {
		crate::session::context::set_session_workdir(&sid, dir.path().to_path_buf());
		crate::session::guardrails::init_for_session();
		run_pipe(&sid, "developer", "hello", false).await
	})
	.await;
	crate::session::guardrails::clear_for_session(&sid);

	let error = out.expect_err("a script that never exits must hit the timeout");
	let message = error.to_string();
	assert!(message.contains("timed out after 300s"), "got: {message}");
}
